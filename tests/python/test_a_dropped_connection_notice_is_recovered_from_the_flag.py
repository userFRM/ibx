"""A connection notice the bounded event channel drops is still delivered.

The engine records every transition in a lossless flag and also sends it as an
event. The event channel is bounded and drops what a consumer too far behind
cannot drain, so the event that would announce a loss or a recovery can be
gone while the flag that recorded it is not. The reference client never loses
this notice: its dispatch queue is unbounded. So the callback is sent from the
flag when the event is not there, in the final state the flags hold.
`isConnected()` reads the flags throughout and stays correct; only the
callback was ever at risk.
"""

import ibx


class Notices(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.codes = []

    def error(self, reqId, errorTime, errorCode, errorString, advancedOrderRejectJson=""):
        self.codes.append(errorCode)


def _connected():
    w = Notices()
    c = ibx.EClient(w)
    c._test_connect("T")
    return w, c


def test_a_loss_whose_event_was_dropped_is_still_announced():
    """Flag set, event dropped: the 1100 is sent from the flag."""
    w, c = _connected()
    c._test_set_connection_lost()   # the flag the engine always sets
    # deliberately NO _test_push_disconnect_event — the bounded channel dropped it
    c.poll()
    assert w.codes == [1100], f"a dropped loss event left the caller uninformed: {w.codes}"
    assert not c.isConnected()


def test_a_present_event_is_not_doubled_by_the_flag():
    """Flag set AND event present: exactly one 1100, not two."""
    w, c = _connected()
    c._test_set_connection_lost()
    c._test_push_disconnect_event()
    c.poll()
    assert w.codes == [1100], f"the loss was announced twice: {w.codes}"


def test_a_restore_whose_event_was_dropped_is_still_announced():
    """A recovery the channel dropped is sent from the flag."""
    w, c = _connected()
    c._test_set_connection_lost()
    c.poll()
    assert w.codes == [1100], w.codes

    c._test_set_connection_restored()   # flag only, event dropped
    c.poll()
    assert 1102 in w.codes, f"a dropped restore event left the caller down: {w.codes}"
    assert c.isConnected()
