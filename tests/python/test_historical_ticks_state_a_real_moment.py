"""A tick whose moment cannot be read is left out, not dated to 1970.

The reference client states a tick's moment as a whole number of seconds, with
no way to say "unreadable". A stamp the venue writes in a shape this client
cannot parse became zero, so a caller charting the series saw a print half a
century before the market they asked about, mixed in with the real ones.
"""

import ibx


class Ticks(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.ticks = []
        self.errors = []

    def historicalTicksLast(self, reqId, ticks, done):
        self.ticks.extend(ticks)

    def error(self, reqId, errorTime, code, msg, advanced=""):
        self.errors.append((reqId, code, msg))


def _ticks_for(times):
    w = Ticks()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_push_historical_ticks(1, times)
    c._test_dispatch_once()
    return w


def test_a_readable_moment_comes_through():
    w = _ticks_for(["20260227-14:30:00"])
    assert len(w.ticks) == 1
    assert w.ticks[0].time > 1_700_000_000
    assert not w.errors, f"a series with nothing wrong with it was reported on: {w.errors}"


def test_a_moment_nobody_can_read_is_left_out():
    w = _ticks_for(["20260227-14:30:00", "not a moment", "20260227-14:30:01"])
    assert [t.time for t in w.ticks if t.time == 0] == [], "a tick was dated to 1970"
    assert len(w.ticks) == 2, f"the readable ticks were lost too: {w.ticks}"


def test_a_tick_left_out_is_said_out_loud():
    """A shortened series and a complete one look the same to a program
    charting it, and the difference is prints that happened and are not there.
    This test swallowed every error and looked only at the shortened list."""
    w = _ticks_for(["20260227-14:30:00", "not a moment"])
    assert w.errors, "a tick was dropped and nothing said so"
    req_id, code, msg = w.errors[-1]
    assert req_id == 1 and code == 321
    assert "1 historical tick" in msg, msg
