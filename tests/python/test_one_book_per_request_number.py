"""One request number holds one book, and a withdrawal says when it holds none.

Depth is routed by records the engine keeps, so neither surface could see that
a number already held a book: two contracts' rows arrived interleaved under one
number with nothing to tell them apart, the withdrawal named only the later
contract and left the earlier one being served, and a reconnect brought back
one book where there had been two.
"""

import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.seen.append((req_id, code))


def contract(con_id, symbol):
    c = ibx.Contract()
    c.conId = con_id
    c.symbol = symbol
    c.secType = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


def test_withdrawing_a_book_that_is_not_held_says_so():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")

    c.cancelMktDepth(7, False)

    assert w.seen == [(7, 310)], f"nothing is held under that number: {w.seen}"


def test_a_second_book_under_a_live_number_is_refused():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")

    c.reqMktDepth(7, contract(756733, "SPY"), 5, False, [])
    assert w.seen == [], f"the first book is asked for: {w.seen}"

    c.reqMktDepth(7, contract(320227571, "QQQ"), 5, False, [])

    assert w.seen == [(7, 102)], f"the number already holds a book: {w.seen}"

    # Withdrawn, the number is the caller's again.
    w.seen.clear()
    c.cancelMktDepth(7, False)
    c.reqMktDepth(7, contract(320227571, "QQQ"), 5, False, [])
    assert w.seen == [], f"the number is free again: {w.seen}"
