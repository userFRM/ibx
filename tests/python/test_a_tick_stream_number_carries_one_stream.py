"""One tick stream per request number, and a withdrawal that says what it found.

Two streams under one number stamp that number on every record, so a caller was
handed one contract's trades and another's quotes with nothing to tell them
apart and the tick kind of whichever asked last. The withdrawal reads one
record, so it reached the second only -- and with that record gone, a second
withdrawal reached nothing and the first stream ran under a cancelled number
for the life of the session.

A withdrawal naming a stream this client does not hold is answered too: said
nothing, it reads exactly like one that worked.
"""

import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.seen.append((req_id, code, msg))


def contract(con_id, symbol):
    c = ibx.Contract()
    c.conId = con_id
    c.symbol = symbol
    c.secType = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


def test_a_second_stream_under_a_live_number_is_refused():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_tbt(5, 3)

    c.reqTickByTickData(5, contract(320227571, "QQQ"), "BidAsk", 0, False)

    assert [(r, code) for r, code, _ in w.seen] == [(5, 102)], (
        f"the number is already carrying a stream: {w.seen}"
    )


def test_withdrawing_a_stream_that_is_not_held_says_so():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")

    c.cancelTickByTickData(999)

    assert [(r, code) for r, code, _ in w.seen] == [(999, 300)], (
        f"nothing is held under that number: {w.seen}"
    )
