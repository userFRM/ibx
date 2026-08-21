"""A connection the engine rebuilds is one the caller is told about.

The engine keeps retrying a lost connection long after it announces the loss,
and announces 1102 when the transports carry again. Both notices arrive on the
same pump the caller is driving, so a pump that stopped at the loss could never
carry the recovery: the caller stood down on 1100 and stayed down on a session
that had come back, with its subscriptions re-established and nothing to say so.
"""

import threading
import time

import ibx


class Notices(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.codes = []

    def error(self, reqId, errorCode, errorString, advancedOrderRejectJson=""):
        self.codes.append(errorCode)

    def connectionClosed(self):
        self.codes.append("closed")


def _within(seconds, condition):
    """Whether something became true in time, rather than waiting for ever."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if condition():
            return True
        time.sleep(0.01)
    return False


def _connected():
    w = Notices()
    c = ibx.EClient(w)
    c._test_connect("T")
    return w, c


def test_the_recovery_arrives_on_a_later_pass():
    """The two notices are two passes apart in life, not one drain apart."""
    w, c = _connected()

    c._test_push_disconnect_event()
    c.poll()
    assert w.codes == [1100], w.codes
    assert not c.isConnected()

    c._test_push_reconnect_event()
    c.poll()
    assert 1102 in w.codes, f"the recovery reached nothing: {w.codes}"
    assert c.isConnected()


def test_a_run_loop_survives_a_loss_and_delivers_the_recovery():
    w, c = _connected()

    runner = threading.Thread(target=c.run, daemon=True)
    runner.start()

    c._test_push_disconnect_event()
    assert _within(5, lambda: 1100 in w.codes), f"the loss reached nothing: {w.codes}"

    c._test_push_reconnect_event()
    assert _within(5, lambda: 1102 in w.codes), f"the loop stopped at the loss: {w.codes}"

    # And a session the caller ends still ends the loop.
    c._test_push_stopped_event()
    runner.join(timeout=5)
    assert not runner.is_alive(), "the loop outlived the session"
    assert "closed" in w.codes, w.codes
