"""A book reset is delivered before the levels that follow it.

The engine says a book has been reset and then the venue's first new levels
land. Delivered levels first, a pass that ran after both had arrived handed
over the new book and then the order to empty it, and the caller emptied the
book it had just filled.
"""
import ibx


class Sequence(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        if code == 317:
            self.seen.append("reset")

    def updateMktDepth(self, reqId, position, operation, side, price, size):
        self.seen.append("level")


def spy():
    c = ibx.Contract()
    c.conId = 756733
    c.symbol = "SPY"
    c.secType = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


def test_a_book_reset_is_delivered_before_the_levels_that_follow_it():
    w = Sequence()
    c = ibx.EClient(w)
    c._test_connect("T")
    c.reqMktDepth(7, spy(), 5, False, [])
    c._test_push_historical_error(7, 317, "Market depth data has been RESET")
    c._test_push_depth(7, 0, "", 0, 1, 100.0, 5.0)
    c._test_dispatch_once()
    assert w.seen == ["reset", "level"], f"the order to empty the book comes first: {w.seen}"
