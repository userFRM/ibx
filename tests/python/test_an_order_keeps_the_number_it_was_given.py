"""An order number is the caller's, or it is a refusal.

An id at or below zero names no order the venue will hold. One was handed out
in its place, so the order reached the market under a number the caller had
never seen: every status about it arrived under an id they were not watching,
and their own cancel named nothing. A negative id on a cancel was read as
unsigned and went out as a number above nine quintillion.
"""

import ibx


def _client():
    class W(ibx.EWrapper):
        def __init__(self):
            super().__init__()
            self.errors = []

        def error(self, reqId, code, msg, advanced=""):
            self.errors.append((reqId, code, msg))

    w = W()
    c = ibx.EClient(w)
    c._test_connect("DU0000000")
    return w, c


def _spy():
    c = ibx.Contract()
    c.symbol, c.secType, c.exchange, c.currency = "SPY", "STK", "SMART", "USD"
    c.conId = 756733
    return c


def _market_order():
    o = ibx.Order()
    o.action, o.orderType, o.totalQuantity = "BUY", "MKT", 1.0
    return o


def test_an_order_numbered_zero_is_refused_not_renumbered():
    w, c = _client()
    c.place_order(0, _spy(), _market_order())
    assert w.errors, "an order number of zero must be reported, not replaced"
    assert w.errors[-1][0] == 0
    assert "order_id 0" in w.errors[-1][2]


def test_an_order_numbered_below_zero_is_refused():
    w, c = _client()
    c.place_order(-5, _spy(), _market_order())
    assert w.errors and "order_id -5" in w.errors[-1][2]


def test_a_cancel_numbered_below_zero_is_refused():
    w, c = _client()
    c.cancel_order(-5, "")
    assert w.errors and "order_id -5" in w.errors[-1][2]


def test_a_contract_id_below_zero_does_not_wrap():
    """Read as unsigned it named a contract above four billion, and the
    request went out asking about it."""
    _, c = _client()
    bad = ibx.Contract()
    bad.conId = -1
    for call in (
        lambda: c.req_fundamental_data(1, bad, "ReportsFinSummary"),
        lambda: c.req_histogram_data(2, bad, True, "3 days"),
        lambda: c.req_historical_news(3, -1, "BRFG", "", "", 10),
    ):
        try:
            call()
        except RuntimeError as why:
            assert "outside the range" in str(why), why
        else:
            raise AssertionError("a contract id below zero must be refused")
