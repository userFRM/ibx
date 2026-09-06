"""The connectivity callbacks arrive in the order the batch holds them.

The engine writes down where the connection ended in flags that say only the
net state, and the events say how it got there. A recovery then a re-drop in
one pump used to be announced 1100 then 1102 — two independent passes over the
batch in a fixed order — so the last word on a session that had gone again was
"restored". Walked in order, it is 1102 then 1100.

Run: pytest tests/python/test_the_connectivity_callbacks_follow_the_batch_order.py -v
"""

import ibx


class Notices(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.codes = []

    def error(self, req_id, error_time, code, message, advanced=""):
        if code in (1100, 1102):
            self.codes.append(code)


def _connected():
    w = Notices()
    c = ibx.EClient(w)
    c._test_connect("T")
    return w, c


def test_a_recovery_then_a_redrop_in_one_pass_is_1102_then_1100():
    w, c = _connected()
    c._test_push_reconnect_event()
    c._test_push_disconnect_event()
    c._test_dispatch_once()
    assert w.codes == [1102, 1100], (
        f"the batch ended on the loss, so its last word is 1100, not restored: {w.codes}"
    )


def test_a_drop_then_a_recovery_in_one_pass_is_1100_then_1102():
    w, c = _connected()
    c._test_push_disconnect_event()
    c._test_push_reconnect_event()
    c._test_dispatch_once()
    assert w.codes == [1100, 1102], f"in the order they happened: {w.codes}"
