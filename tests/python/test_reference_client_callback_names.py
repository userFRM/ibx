"""A wrapper written for the reference client must receive callbacks.

The reference client names its callbacks with the words run together; this one
names them with underscores. A caller who brings a wrapper written against the
reference client defines its names, and every call used to land on the do-nothing
default this base class supplies — so their callbacks never ran, and nothing said
so. Silence is the whole of the fault.
"""
import threading
from ibx import EClient, EWrapper


def _watchdog(seconds=5.0):
    return threading.Timer(seconds, lambda: (_ for _ in ()).throw(SystemExit(1)))


def test_a_wrapper_named_the_reference_way_is_called():
    """orderStatus, not order_status — the names genuinely differ here."""
    class ReferenceStyle(EWrapper):
        def __init__(self):
            self.seen = []

        def orderStatus(self, orderId, status, filled, remaining, avgFillPrice,
                        permId, parentId, lastFillPrice, clientId, whyHeld,
                        mktCapPrice=0.0):
            self.seen.append(("orderStatus", orderId, status))

        def error(self, reqId, errorCode, errorString, advancedOrderRejectJson=""):
            self.seen.append(("error", errorCode))

    w = ReferenceStyle()
    c = EClient(w)
    c._test_connect("T")
    c._test_push_fill(0, 42, "BUY", 100.0, 5, 0, 1.25)

    t = _watchdog()
    t.start()
    c._test_dispatch_once()
    t.cancel()

    names = [e[0] for e in w.seen]
    assert "orderStatus" in names, (
        f"a wrapper named the reference client's way received nothing: {w.seen}"
    )


def test_a_wrapper_named_this_way_still_is():
    """The name this client has always used keeps working."""
    class OurStyle(EWrapper):
        def __init__(self):
            self.seen = []

        def order_status(self, order_id, status, filled, remaining, avg_fill_price,
                         perm_id, parent_id, last_fill_price, client_id, why_held,
                         mkt_cap_price=0.0):
            self.seen.append(("order_status", order_id, status))

    w = OurStyle()
    c = EClient(w)
    c._test_connect("T")
    c._test_push_fill(0, 42, "BUY", 100.0, 5, 0, 1.25)

    t = _watchdog()
    t.start()
    c._test_dispatch_once()
    t.cancel()

    names = [e[0] for e in w.seen]
    assert "order_status" in names, (
        f"the name this client has always used stopped working: {w.seen}"
    )


def test_the_methods_answer_to_the_reference_clients_names():
    """Code written for the reference client calls its method names.

    Every method here is named with underscores, so that code stopped at its
    first call with the method simply not there. The run-together name is
    translated back to the one this client has.
    """
    c = EClient(EWrapper())
    for name in (
        "placeOrder", "cancelOrder", "reqMktData", "cancelMktData",
        "reqContractDetails", "reqHistoricalData", "reqIds", "reqPositions",
        "reqAccountSummary", "reqExecutions", "reqOpenOrders", "reqMktDepth",
    ):
        assert hasattr(c, name), f"code written for the reference client cannot call {name}"


def test_a_name_that_is_no_method_is_still_refused():
    """Translating a name must not invent one."""
    c = EClient(EWrapper())
    try:
        c.thisIsNotAMethod
    except AttributeError:
        return
    raise AssertionError("a name that names no method was answered")


def test_the_base_wrapper_answers_to_the_reference_clients_names():
    """Code written for the reference client asks the base class about its
    callbacks — whether a subclass overrode one, or by calling the default
    through super(). Under this class those names were simply absent."""
    w = EWrapper()
    for name in (
        "nextValidId", "managedAccounts", "connectionClosed", "currentTime",
        "contractDetails", "contractDetailsEnd", "execDetails", "orderStatus",
        "tickPrice", "tickSize", "historicalData", "accountSummary",
        "position", "openOrder", "completedOrder", "updateAccountValue",
    ):
        assert hasattr(w, name), f"the base class does not answer to {name}"


def test_the_base_wrapper_still_refuses_a_name_that_is_no_callback():
    w = EWrapper()
    try:
        w.thisIsNotACallback
    except AttributeError:
        return
    raise AssertionError("a name that names no callback was answered with a do-nothing")


def test_a_callback_payload_answers_to_the_reference_clients_field_names():
    """These objects are handed to a caller by a callback and only ever read.

    Code written for the reference client reads the run-together names. Under
    these classes they were absent, so the object arrived carrying everything
    and answered nothing.
    """
    import ibx

    published_execution_fields = [
        "orderId", "clientId", "execId", "time", "acctNumber", "exchange",
        "side", "shares", "price", "permId", "liquidation", "cumQty",
        "avgPrice", "orderRef", "evRule", "evMultiplier", "modelCode",
        "lastLiquidity", "pendingPriceRevision",
    ]
    e = ibx.Execution()
    for f in published_execution_fields:
        assert hasattr(e, f), f"an execution does not answer to {f}"

    d = ibx.ContractDetails()
    for f in ("marketName", "minTick", "longName", "priceMagnifier", "contractMonth",
              "industry", "category", "subcategory", "bondType", "couponType",
              "nextOptionDate", "fundName", "fundFamily", "fundManagementFee",
              "fundClosedForNewMoney", "realExpirationDate", "cusip"):
        assert hasattr(d, f), f"contract details do not answer to {f}"


def test_a_payload_still_refuses_a_name_that_is_no_field():
    import ibx
    try:
        ibx.Execution().thisIsNotAField
    except AttributeError:
        return
    raise AssertionError("a name that names no field was answered")
