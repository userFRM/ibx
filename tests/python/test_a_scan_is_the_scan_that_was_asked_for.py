"""A scan runs on what the caller described, or it does not run.

The fields naming a scan were read with a fallback, so one that could not be
read became a different scan entirely: the top gaining US stocks, under the
caller's own request id and answered as though it were theirs. A field left
off is still the default, which is what the reference client does with one.
"""

import ib_async

import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.errors = []

    def error(self, reqId, errorTime, errorCode, errorString, advancedOrderRejectJson=""):
        self.errors.append((errorCode, errorString))


def _client():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    return w, c


def test_an_ordinary_subscription_is_sent():
    """Their own object states no rows as a negative number, which is not an
    unreadable value: it means the venue picks."""
    w, c = _client()
    c.req_scanner_subscription(1, ib_async.ScannerSubscription())
    assert w.errors == [], w.errors


def test_a_field_stated_and_unreadable_is_refused():
    w, c = _client()
    sub = ib_async.ScannerSubscription()
    sub.scanCode = 42
    c.req_scanner_subscription(2, sub)
    assert [code for code, _ in w.errors] == [321], w.errors
    assert "scanCode" in w.errors[0][1]


def test_a_field_left_off_takes_the_default():
    """Anything shaped like a subscription works, as the reference client's
    own duck-typing allows."""
    class Sparse:
        scanCode = "HOT_BY_VOLUME"

    w, c = _client()
    c.req_scanner_subscription(3, Sparse())
    assert w.errors == [], w.errors
