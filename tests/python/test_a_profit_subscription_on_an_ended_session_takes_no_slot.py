"""A profit subscription on an ended session takes no slot.

Taken before the session was checked, a refused request held the one slot
there is: the next request under another number was refused as a duplicate of
one that never went.
"""

import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.seen.append((req_id, code))


def test_a_profit_subscription_on_an_ended_session_takes_no_slot():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_end_session()
    c.reqPnL(9, "", "")
    c.reqPnL(10, "", "")
    codes = [code for _, code in w.seen]
    assert codes.count(504) == 2, f"both refused for the session, neither as a duplicate: {w.seen}"
