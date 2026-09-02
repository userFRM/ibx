"""A program built the way the reference client's own sample is built runs here.

That sample is ``class TestApp(TestWrapper, TestClient)``, with
``TestWrapper(EWrapper)`` and ``TestClient(EClient)``, constructed with no
arguments and wired by ``EClient.__init__(self, wrapper=self)``. Under this
module the class could not be defined: both bases were extension types with a
layout of their own, and the interpreter refuses a class whose bases lay an
instance out two ways. And ``EClient.__init__`` was ``object.__init__``, which
takes no wrapper, so ``TestClient`` alone could not be built either.

The classes below are the sample's, cut down to what drives them. Their
spellings are the sample's too, which is why each says it is not a test.
"""

import threading

from conftest import NotConnectedProbe
from ibx import EClient, EWrapper


class TestClient(EClient):
    __test__ = False

    def __init__(self, wrapper):
        EClient.__init__(self, wrapper)


class TestWrapper(EWrapper):
    __test__ = False

    def __init__(self):
        EWrapper.__init__(self)


class TestApp(TestWrapper, TestClient):
    __test__ = False

    def __init__(self):
        TestWrapper.__init__(self)
        TestClient.__init__(self, wrapper=self)
        self.started = False
        self.nextValidOrderId = None
        self.errors = []
        self.times = []

    def error(self, reqId, errorTime, errorCode, errorString, advancedOrderRejectJson=""):
        self.errors.append((reqId, errorCode))

    def nextValidId(self, orderId: int):
        super().nextValidId(orderId)
        self.nextValidOrderId = orderId
        self.start()

    def start(self):
        if self.started:
            return
        self.started = True
        self.reqCurrentTime()

    def currentTime(self, time):
        self.times.append(time)


def test_a_subclass_constructor_reaches_the_base_constructor():
    """``TestClient(wrapper)`` on its own: ``EClient.__init__`` takes the wrapper."""
    probe = NotConnectedProbe()
    client = TestClient(probe)
    client.reqCurrentTime()
    assert probe.not_connected, probe.errors


def test_the_app_is_one_object_on_both_bases():
    app = TestApp()
    assert isinstance(app, EWrapper) and isinstance(app, EClient)
    # A request before connecting is answered on the app itself, which is what
    # ``wrapper=self`` asked for.
    app.reqCurrentTime()
    assert app.errors == [(-1, NotConnectedProbe.NOT_CONNECTED)]
    # The client's own aliases resolve on the app, although the attribute
    # hook the interpreter asks for the whole instance is the wrapper's.
    assert app.eConnect.__name__ == "connect"
    assert app.eDisconnect.__name__ == "disconnect"


def test_the_clients_other_spellings_resolve_on_the_app():
    """The wrapper's attribute hook serves the whole app, so the client's
    run-together names have to come through it — not only the two aliases."""
    app = TestApp()
    for name in (
        "reqPnL", "cancelPnLSingle", "requestFA", "replaceFA", "reqSecDefOptParams",
        "reqHeadTimeStamp", "updateMktDepthL2", "reqMktDepthExchanges",
        "reqManagedAccts", "reqCurrentTimeInMillis",
    ):
        assert callable(getattr(app, name)), name


def test_a_wrapper_alone_answers_to_none_of_the_clients_names():
    """The hook carries the client's aliases, but a wrapper with no client
    behind it still refuses them, and still refuses a name that names nothing."""
    wrapper = TestWrapper()
    for name in ("eConnect", "eDisconnect", "reqMktData", "noSuchThing"):
        try:
            getattr(wrapper, name)
        except AttributeError:
            continue
        raise AssertionError(f"a bare wrapper answered to {name}")
    try:
        TestApp().noSuchThing
    except AttributeError:
        pass
    else:
        raise AssertionError("the app answered to a name that names nothing")


def test_a_second_wrapper_is_refused():
    """The wrapper is bound once. The sample never rebinds it, and a callback
    may already be on its way to the first."""
    app = TestApp()
    EClient.__init__(app, app)   # the same one again is nothing new
    try:
        EClient.__init__(app, TestWrapper())
    except TypeError as e:
        assert "already has a wrapper" in str(e)
    else:
        raise AssertionError("a second wrapper was accepted")


def test_the_app_is_driven_as_the_sample_drives_it():
    """Connect, take the first order id on ``nextValidId`` and make the
    requests from there, then ``run()`` until the session ends."""
    app = TestApp()
    app._test_connect("DU0000000")
    app.reqIds(-1)
    app._test_dispatch_once()
    assert app.nextValidOrderId is not None and app.started
    app._test_dispatch_once()
    assert len(app.times) == 1, app.times

    loop = threading.Thread(target=app.run)
    loop.start()
    app._test_push_stopped_event()
    loop.join(timeout=5)
    assert not loop.is_alive(), "run() did not return when the session ended"
