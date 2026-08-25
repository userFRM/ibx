"""A request that cannot be made is reported, not answered with another one.

A regulatory snapshot names a separate, chargeable one-shot request. Taken and
dropped, the caller was subscribed to an ordinary stream instead: a different
request than the one they asked for, at a different price, with nothing to say
which they were reading.

The kind of trade stream a tick-by-tick subscription asks for is stated back on
every print. Stated as the exchange's own whichever stream it came from, a
caller subscribed to every print — those reported away from the exchange
included — was told each of them happened on the exchange.
"""

import ibx


class _Recorder(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.errors = []
        self.prints = []

    def error(self, reqId, code, msg, advanced=""):
        self.errors.append((reqId, code, msg))

    def tickByTickAllLast(self, reqId, tickType, time, price, size,
                          tickAttribLast, exchange, specialConditions):
        self.prints.append((reqId, tickType))


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


def test_a_regulatory_snapshot_is_asked_for_rather_than_refused():
    """The chargeable snapshot is one this client makes, not one it declines.

    It was refused here, and reported as a request that could not be made. The
    venue serves it under a request type of its own, so asking for it is the
    whole of what is needed and a caller that wants one gets one.
    """
    w, c = _client()
    c.req_mkt_data(11, _spy(), "", False, True, [])
    assert not [e for e in w.errors if "regulatory_snapshot" in e[2]], w.errors


def test_an_ordinary_subscription_is_untouched():
    w, c = _client()
    c.req_mkt_data(12, _spy(), "", False, False, [])
    assert not [e for e in w.errors if "regulatory_snapshot" in e[2]]


def test_every_print_states_the_stream_it_came_from():
    w, c = _client()
    c._test_set_instrument_count(1)
    c._test_map_instrument(0, 0)
    # Each subscription registers the kind it asked for under its request id.
    c._test_set_tbt_kind(0, "AllLast")
    c._test_push_tbt_trade(0, 412.55, 100, "NYSE")
    c._test_dispatch_once()
    assert w.prints == [(0, 2)], f"AllLast must be stated as 2, got {w.prints}"

    w.prints.clear()
    c._test_set_tbt_kind(0, "Last")
    c._test_push_tbt_trade(0, 412.60, 50, "NYSE")
    c._test_dispatch_once()
    assert w.prints == [(0, 1)], f"Last must be stated as 1, got {w.prints}"


def test_a_log_level_that_is_not_one_is_reported():
    """It used to be described back as `warn`, so a caller asking for a level
    that does not exist was told it had one."""
    w, c = _client()
    c.set_server_log_level(9)
    assert w.errors and "not a log level" in w.errors[-1][2]

    w.errors.clear()
    for level in (1, 2, 3, 4, 5):
        c.set_server_log_level(level)
    assert not w.errors, f"the five levels must be taken: {w.errors}"
