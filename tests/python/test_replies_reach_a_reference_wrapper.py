"""The answers to a request reach a wrapper written for the reference client.

Callbacks fired while an event is dispatched go through the name resolver, but
the ones a request answers itself did not: they called this client's own
spelling directly. A wrapper written against the reference client defines the
other spelling, so those calls landed on the do-nothing default this base class
supplies and the caller's own code never ran. Nothing said so, which is the
whole of the fault.
"""

from ibx import EClient, EWrapper


class ReferenceStyle(EWrapper):
    """Named the way the reference client names its callbacks, and no other."""

    def __init__(self):
        super().__init__()
        self.seen = []
        self.errors = []

    def openOrder(self, orderId, contract, order, orderState):
        self.seen.append(("openOrder", orderId))

    def orderStatus(self, orderId, status, filled, remaining, avgFillPrice,
                    permId, parentId, lastFillPrice, clientId, whyHeld,
                    mktCapPrice=0.0):
        self.seen.append(("orderStatus", orderId))

    def accountUpdateMulti(self, reqId, account, modelCode, key, value, currency):
        self.seen.append(("accountUpdateMulti", key))

    def positionMulti(self, reqId, account, modelCode, contract, pos, avgCost):
        self.seen.append(("positionMulti", reqId))

    def execDetails(self, reqId, contract, execution):
        self.seen.append(("execDetails", reqId))

    def error(self, reqId, errorTime, errorCode, errorString, advancedOrderRejectJson=""):
        self.errors.append(errorCode)


def _connected():
    w = ReferenceStyle()
    c = EClient(w)
    c._test_connect("T")
    return w, c


def test_open_orders_reach_a_reference_wrapper():
    w, c = _connected()
    c._test_track_order(7, 1, "SPY", "BUY", 5.0, 10.0, 0)
    c.req_open_orders()
    assert ("openOrder", 7) in w.seen, f"the open order reached nothing: {w.seen}"
    assert ("orderStatus", 7) in w.seen, f"its status reached nothing: {w.seen}"


def test_account_updates_multi_reach_a_reference_wrapper():
    w, c = _connected()
    c._test_note_account_value("NetLiquidation", "1.00", "USD")
    c.req_account_updates_multi(3, "DU1", "", False)
    keys = [key for name, key in w.seen if name == "accountUpdateMulti"]
    assert keys, f"the account figures reached nothing: {w.seen}"


def test_positions_multi_reach_a_reference_wrapper():
    w, c = _connected()
    c._test_note_account_value("NetLiquidation", "1.00", "USD")
    c._test_set_position(756733, 10.0, 100.0)
    c.req_positions_multi(4, "DU1", "")
    assert ("positionMulti", 4) in w.seen, f"the holding reached nothing: {w.seen}"


def test_executions_reach_a_reference_wrapper():
    w, c = _connected()
    c._test_track_order(7, 1, "SPY", "BUY", 5.0, 10.0, 0)
    c._test_push_fill(1, 7, "BUY", 10.0, 5, 0, 1.25)
    c._test_dispatch_once()
    w.seen.clear()
    c.req_executions(9)
    assert ("execDetails", 9) in w.seen, f"the fill reached nothing: {w.seen}"


def test_open_orders_wait_for_what_the_venue_is_still_naming():
    """The venue names the orders already working unprompted after a connect.
    Answered before that lands, a strategy asking what it has on at startup is
    told nothing, and places the same order twice."""
    import threading
    import time

    w = ReferenceStyle()
    c = EClient(w)
    c._test_connect("T", replay_done=False)

    def name_it_late():
        time.sleep(0.2)
        c._test_track_order(7, 1, "SPY", "BUY", 5.0, 10.0, 0)
        c._test_finish_order_replay()

    threading.Thread(target=name_it_late, daemon=True).start()
    c.req_open_orders()
    assert ("openOrder", 7) in w.seen, f"answered before the venue had finished: {w.seen}"
