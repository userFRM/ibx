"""A fill is on record before its order's status is announced.

Asking for executions from inside the status callback is ordinary, and the
store was written after it, so a program that asked there was answered
without the fill it was being told about.
"""

import ibx


class AsksFromTheStatus(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.client = None
        self.replayed = []

    def orderStatus(self, orderId, status, *rest):
        self.client.reqExecutions(77, ibx.ExecutionFilter())

    def execDetails(self, reqId, contract, execution):
        if reqId == 77:
            self.replayed.append(execution.execId)

    def error(self, *a):
        pass


def test_a_fill_is_stored_before_its_status_is_announced():
    w = AsksFromTheStatus()
    c = ibx.EClient(w)
    w.client = c
    c._test_connect("T")
    c._test_track_order(86, 0, "SPY", "BUY", 1, 100.0)
    c._test_push_venue_order(86, "SPY", "BUY", 1, 100.0)
    c._test_push_fill(0, 86, "BUY", 100.0, 1, 0)
    c._test_dispatch_once()
    c._test_dispatch_once()
    assert len(w.replayed) == 1, f"the fill was on record when its status was announced: {w.replayed}"
