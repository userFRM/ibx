"""A program built the way the reference client's sample is built is collected.

``class App(EWrapper, EClient)`` wired with ``EClient.__init__(self, wrapper=self)``
holds itself through the client's wrapper. That reference lived in a field the
cyclic collector could not see, so the object read as held from outside and was
never collected: the client's drop never ran, and with it the logout, the
shutdown and the join of the engine thread — a program that builds an app per
reconnect attempt leaked a thread per attempt. The wrapper is now visited on
traversal and released on clear, so the cycle is found and broken.
"""

import gc
import weakref

import pytest
from ibx import EClient, EWrapper

HAVE_GC = 1 << 14   # Py_TPFLAGS_HAVE_GC


class App(EWrapper, EClient):
    """The sample's shape, cut down to the wiring."""

    def __init__(self):
        EWrapper.__init__(self)
        EClient.__init__(self, wrapper=self)


def test_the_client_type_takes_part_in_collection():
    assert EClient.__flags__ & HAVE_GC, "EClient carries no Py_TPFLAGS_HAVE_GC"


def test_an_app_that_holds_itself_is_collected():
    app = App()
    gone = weakref.ref(app)
    del app
    gc.collect()
    assert gone() is None, "the app is still alive after a collection"


class Raises(EWrapper):
    """A handler that raises what it is given, on a refusal and on the close."""

    def __init__(self, what):
        super().__init__()
        self.what = what

    def error(self, reqId, errorTime, code, msg, advanced=""):
        raise self.what

    def connectionClosed(self):
        raise self.what


def test_an_interrupt_raised_by_a_handler_ends_the_call_that_reached_it():
    """A refusal is reported on ``error`` and the close on ``connectionClosed``.
    The handler raising an ordinary exception is its own problem and stays
    inside the call; an interrupt ends the call, as it ends a pass of the
    dispatch loop. On these two paths it was swallowed, or left set for the
    interpreter to report as a ``SystemError`` at whatever call came next."""
    client = EClient(Raises(KeyboardInterrupt()))
    client._test_connect("DU0000000")
    with pytest.raises(KeyboardInterrupt):
        client.setServerLogLevel(9)
    client._test_push_stopped_event()
    with pytest.raises(KeyboardInterrupt):
        client.poll()

    client = EClient(Raises(RuntimeError("what the caller wrote")))
    client._test_connect("DU0000000")
    client.setServerLogLevel(9)
    client._test_push_stopped_event()
    client.poll()


class InterruptsOnTheFirstTick(EWrapper):
    def __init__(self):
        super().__init__()
        self.ticks = 0
        self.closed = 0

    def tickPrice(self, reqId, tickType, price, attrib):
        self.ticks += 1
        if self.ticks == 1:
            raise KeyboardInterrupt()

    def connectionClosed(self):
        self.closed += 1


def test_a_pass_an_interrupt_ended_runs_no_more_of_the_callers_code():
    """The session's end is read before the quotes on a pass, so a quote
    handler raising an interrupt raises it out of ``poll`` with the close
    still unsaid. ``run`` already left it unsaid; ``poll`` said it on the same
    pass, which ran the close handler after the caller had asked to stop and
    let what that handler raised stand in for the interrupt. The close is
    said on the next pass, once."""
    wrapper = InterruptsOnTheFirstTick()
    client = EClient(wrapper)
    client._test_connect("DU0000000")
    client._test_map_instrument(1, 7)
    client._test_push_stopped_event()
    client._test_push_quote(7, 412.0, 412.1, 412.05, 10, 10, 1, 1000, 0.0, 0.0, 0.0, 0.0)
    with pytest.raises(KeyboardInterrupt):
        client.poll()
    assert (wrapper.ticks, wrapper.closed) == (1, 0), "the close was said on the pass the interrupt ended"
    client.poll()
    assert wrapper.closed == 1, "the close was not said on the pass after"
