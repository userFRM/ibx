"""Regression tests for the Python execution filter.

`clientId` and `time` were parsed by the Rust core but never extracted from
the Python filter object, so executions the caller had explicitly filtered
out — another client's fills, or ones before the requested cutoff — were
replayed anyway.
"""
from ibx import EClient, EWrapper


class _Filter:
    """Stand-in for ibapi's ExecutionFilter: plain attributes, read by name."""

    def __init__(self, **kw):
        self.clientId, self.acctCode, self.time = 0, "", ""
        self.symbol, self.secType, self.exchange, self.side = "", "", "", ""
        for k, v in kw.items():
            setattr(self, k, v)


class _Counter(EWrapper):
    def __init__(self):
        super().__init__()
        self.rows = 0

    def exec_details(self, req_id, contract, execution):
        self.rows += 1


def _client_with_one_execution():
    w = _Counter()
    c = EClient(w)
    c._test_connect("T")
    c._test_map_instrument(90, 0)
    c._test_push_fill(0, 1, "BUY", 10.0, 5, 0)
    c._test_dispatch_once()
    w.rows = 0
    return c, w


def test_an_unfiltered_request_still_replays():
    c, w = _client_with_one_execution()
    c.req_executions(1, _Filter())
    c._test_dispatch_once()
    assert w.rows == 1, "the baseline request must replay the stored execution"


def test_a_foreign_client_id_filters_everything_out():
    c, w = _client_with_one_execution()
    c.req_executions(1, _Filter(clientId=999999))
    c._test_dispatch_once()
    assert w.rows == 0, f"another client's request must replay nothing, got {w.rows}"


def test_a_cutoff_filters_on_the_time_the_venue_stated():
    """A bound on time reaches an execution the venue timed, and no other.

    The fixture's fill carries no report, so the venue stated no time for it.
    Composing one from this client's clock made every such fill sort before
    every bound a caller can write — so a caller asking for today's executions
    was shown none of them. An execution nobody timed cannot be placed either
    side of a bound, and is kept rather than hidden.
    """
    c, w = _client_with_one_execution()
    c.req_executions(1, _Filter(time="20990101-00:00:00"))
    c._test_dispatch_once()
    assert w.rows == 1, f"an execution the venue never timed is not hidden, got {w.rows}"


def test_a_side_filter_states_the_order_action():
    """A filter names the side the way an order does; a stored execution names
    it the way the venue does. The two vocabularies must still meet."""
    c, w = _client_with_one_execution()
    c.req_executions(1, _Filter(side="BUY"))
    c._test_dispatch_once()
    assert w.rows == 1, f"a buy filter must replay the buy, got {w.rows}"
    w.rows = 0
    c.req_executions(2, _Filter(side="SELL"))
    c._test_dispatch_once()
    assert w.rows == 0, f"a sell filter must replay nothing, got {w.rows}"
