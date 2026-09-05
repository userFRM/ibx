"""A number the venue has finished an order under does not place another.

An order that finishes leaves the book, so its number stops naming anything and
a placement under it read as a new order rather than as the duplicate it is.
The venue refuses a repeated number only while it is still working one, so
after a fill it takes the placement as a new order -- and a caller retrying
what it believed had failed was given a second live order.
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


def test_placing_again_under_a_finished_number_is_refused():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_con_id(756733, 0)

    c.placeOrder(83, spy(), limit_order())
    c._test_push_order_update(83, 0, "Filled", 1, 0)
    c._test_dispatch_once()
    w.seen.clear()

    c.placeOrder(83, spy(), limit_order())

    assert w.seen == [(83, 103)], (
        f"the number has already been worked: {w.seen}"
    )


def test_a_withdrawal_of_a_finished_number_is_not_cancellable():
    """This client saw the order finish, so a withdrawal of it is refused as not
    cancellable rather than as a number nobody has heard of."""
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_con_id(756733, 0)
    c.placeOrder(84, spy(), limit_order())
    c._test_push_order_update(84, 0, "Filled", 1, 0)
    c._test_dispatch_once()
    c._test_take_commands()
    w.seen.clear()
    c.cancelOrder(84, "")
    assert w.seen == [(84, 161)], f"the order finished under this client's eyes: {w.seen}"
    assert not c._test_take_commands(), "and nothing was sent under it"

