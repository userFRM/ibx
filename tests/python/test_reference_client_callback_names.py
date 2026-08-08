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
