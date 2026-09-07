"""A request the engine cannot be sent is answered on the error callback.

The reference client answers a request it refuses on `error` and returns, so a
program written against it handles refusals there and nowhere else — it puts no
try/except around `cancelHistoricalData`. This surface says so in its own words
and half of it did something else: where the engine stopped between the check a
call makes and the send it then does, twelve calls raised out of the call
instead, at exactly the moment the caller expects a callback.

The window is narrow in a real session and the inconsistency is not: the same
failure had two answers depending on which method you called.
"""

import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.seen.append((req_id, code, msg))


def _session(setup=None):
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    # Whatever the withdrawal under test needs to be holding, opened while
    # there is still an engine to open it with.
    if setup is not None:
        setup(c)
        w.seen.clear()
    # The engine goes away, leaving a session that reports itself connected.
    c._test_drop_engine()
    return w, c


def test_a_withdrawal_reports_rather_than_raises():
    w, c = _session()
    # Returns normally, as the reference client's does.
    assert c.cancelHistoricalData(42) is None
    assert w.seen, "the caller is told on the callback it was written to read"
    req_id, code, _ = w.seen[-1]
    assert req_id == 42, f"under the number it asked with: {w.seen}"
    assert code == 504, f"not connected: {w.seen}"


def test_every_withdrawal_answers_the_same_way():
    """One method per command that used to raise."""
    def _a_book(c):
        book = ibx.Contract()
        book.conId = 756733
        book.symbol = "SPY"
        book.secType = "STK"
        book.exchange = "SMART"
        book.currency = "USD"
        c.reqMktDepth(6, book, 5, False, [])

    for call, expect_id, setup in [
        (lambda c: c.cancelHistoricalData(1), 1, None),
        (lambda c: c.cancelHeadTimeStamp(2), 2, None),
        (lambda c: c.cancelScannerSubscription(3), 3, None),
        (lambda c: c.cancelFundamentalData(4), 4, None),
        (lambda c: c.cancelHistogramData(5), 5, None),
        (lambda c: c.cancelMktDepth(6, False), 6, _a_book),
        (lambda c: c.cancelRealTimeBars(7), 7, None),
        (lambda c: c.reqScannerParameters(), -1, None),
        (lambda c: c.reqMktDepthExchanges(), -1, None),
    ]:
        w, c = _session(setup)
        assert call(c) is None, "the call returns rather than raising"
        assert w.seen, f"nothing was reported for the call expecting {expect_id}"
        assert w.seen[-1][0] == expect_id, w.seen
        assert w.seen[-1][1] == 504, w.seen


def test_a_slot_taken_for_a_request_that_never_went_goes_back():
    """A book slot and the P&L slot are reserved before the send.

    Kept when the send fails, the number stayed held against a request the
    venue never heard: the caller's retry under it was refused as a duplicate
    of that one, and only a rebuilt session freed it.
    """
    book = ibx.Contract()
    book.conId = 756733
    book.symbol = "SPY"
    book.secType = "STK"
    book.exchange = "SMART"
    book.currency = "USD"

    w, c = _session()
    c.reqMktDepth(11, book, 5, False, [])
    assert w.seen and w.seen[-1][1] == 504, w.seen
    w.seen.clear()
    # The slot is free, so a withdrawal finds nothing held rather than a book.
    c.cancelMktDepth(11, False)
    assert w.seen and w.seen[-1][1] != 504, (
        "the slot was released, so this is 'no book held', not 'not connected'"
    )

    w, c = _session()
    c.reqPnL(12, "DU1", "")
    assert w.seen and w.seen[-1][1] == 504, w.seen
    w.seen.clear()
    # And the one P&L slot is free for the next number to take.
    c.reqPnL(13, "DU1", "")
    assert w.seen and w.seen[-1][1] == 504, (
        f"a retry is refused as not connected, not as a duplicate: {w.seen}"
    )


def test_no_book_is_taken_on_a_feed_that_is_over():
    """The other surface refuses this and this one did not.

    A book rides the quote feed, so a feed the engine has given up on serves
    none. Accepted, the request took a slot and reached a sender with no
    connection to write it to — and a book that never arrives is what a market
    with nothing to say looks like, so nothing told the two apart.
    """
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_say_the_feed_is_over("the venue would not take the connection back")

    book = ibx.Contract()
    book.conId = 756733
    book.symbol = "SPY"
    book.secType = "STK"
    book.exchange = "SMART"
    book.currency = "USD"
    assert c.reqMktDepth(21, book, 5, False, []) is None
    assert w.seen, "the caller is told"
    assert w.seen[-1][1] == 504, w.seen
