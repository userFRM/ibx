"""A withdrawal names an order this client is working, and says so when it does not.

A withdrawal of a number naming nothing was sent anyway, under an order name
this client invented, and the caller learnt from the venue rather than from the
number. The dangerous half is the other one: the account's working set arrives
asynchronously after connect, so a withdrawal read before it lands must not
refuse an order that is genuinely live -- a refusal there leaves a real order
working.
"""

import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.seen.append((req_id, code))

    def orderStatus(self, *a):
        pass


def spy():
    c = ibx.Contract()
    c.conId = 756733
    c.symbol = "SPY"
    c.secType = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


def limit_order():
    o = ibx.Order()
    o.action = "BUY"
    o.totalQuantity = 1
    o.orderType = "LMT"
    o.lmtPrice = 100.0
    o.tif = "DAY"
    return o


def test_a_withdrawal_naming_nothing_says_so():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")

    c.cancelOrder(42, "")

    assert w.seen == [(42, 135)], f"no order is working under that number: {w.seen}"
    assert not c._test_take_commands(), "and nothing was sent under it"


def test_a_withdrawal_of_an_order_this_session_placed_goes():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_con_id(756733, 0)

    c.placeOrder(42, spy(), limit_order())
    c._test_take_commands()
    w.seen.clear()

    c.cancelOrder(42, "")

    assert w.seen == [], f"the withdrawal is not refused: {w.seen}"
    assert any("Cancel" in cmd for cmd in c._test_take_commands()), (
        "and it reaches the engine"
    )
