"""ADJUSTED_LAST is served on the callback API, not refused before it is sent.

The venue carries no adjusted series: what it serves is raw trades, and an
adjusted series is those trades folded with the contract's own corporate
actions. The engine holds the raw bars until the actions are in hand and then
folds them; the waiting call `historical_data` hands the series back in one
piece and the callback path delivers it bar by bar. A caller on the callback
API — which is the API — could not get an adjusted series at all before, and
can now.

The fold itself is exercised at the engine, where the two replies land and are
correlated, so both surfaces deliver the same adjusted bars. Here is the
callback surface's own half: it accepts the request rather than refusing it,
and it refuses the two shapes it cannot fold — a contract with no venue id, and
a request kept up to date, which never completes into a whole series to fold.
"""

import ibx


class _Recorder(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.errors = []

    def error(self, reqId, errorTime, code, msg, advanced=""):
        self.errors.append((reqId, code, msg))


def _client():
    w = _Recorder()
    c = ibx.EClient(w)
    c._test_connect("DU0000000")
    return w, c


def _spy():
    c = ibx.Contract()
    c.symbol, c.secType, c.exchange, c.currency = "SPY", "STK", "SMART", "USD"
    c.conId = 756733
    return c


def test_adjusted_last_is_accepted_on_the_callback_path():
    """A qualified contract asked for adjusted is not refused: the request is
    made, and the fold happens as its bars arrive."""
    w, c = _client()
    c.req_historical_data(
        1, _spy(), end_date_time="", duration_str="1 Y",
        bar_size_setting="1 day", what_to_show="ADJUSTED_LAST", use_rth=1,
    )
    assert w.errors == [], w.errors


def test_adjusted_last_without_the_venue_id_is_refused():
    """The actions are asked for by the venue's id for the contract. Named by
    anything else the fold cannot be made, so the request is refused rather
    than answered with raw trades under an adjusted name."""
    w, c = _client()
    unqualified = ibx.Contract()
    unqualified.symbol, unqualified.secType = "SPY", "STK"
    unqualified.exchange, unqualified.currency = "SMART", "USD"
    c.req_historical_data(
        2, unqualified, end_date_time="", duration_str="1 Y",
        bar_size_setting="1 day", what_to_show="ADJUSTED_LAST", use_rth=1,
    )
    assert [e for e in w.errors if "venue's id" in e[2]], w.errors


def test_adjusted_last_kept_up_to_date_is_refused():
    """Kept up to date the request never completes, so there is no whole series
    to fold onto one scale: refused rather than answered with one that cannot
    be adjusted."""
    w, c = _client()
    c.req_historical_data(
        3, _spy(), end_date_time="", duration_str="1 D",
        bar_size_setting="5 mins", what_to_show="ADJUSTED_LAST", use_rth=0,
        keep_up_to_date=True,
    )
    assert [e for e in w.errors if "kept up to date" in e[2]], w.errors
