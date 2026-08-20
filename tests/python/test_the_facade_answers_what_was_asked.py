"""The answers a session hands back are the answers to the question asked.

Each of these sent a request and then returned something else: whatever had
already accumulated, or the first fragment of an answer still arriving. The
caller could not tell a complete answer from a partial one, or the answer to
their question from the answer to another.
"""

import threading
import time

import pytest

import ibx
from ibx._state import LiveState


def _session():
    ib = ibx.IB()
    ib.client._test_connect("DU1")
    return ib


def test_waiting_for_an_update_can_fail():
    """A liveness check that always says yes is not a liveness check."""
    ib = _session()
    assert ib.waitOnUpdate(timeout=0.05) is False


def test_waiting_for_an_update_ends_when_one_arrives():
    """Paced by the looking: the session speaks on the second look, so what is
    measured is that the wait notices, not how a loaded machine schedules."""
    class Speaks(LiveState):
        def __init__(self):
            super().__init__()
            self._looks = 0

        def updates(self):
            self._looks += 1
            return self._looks // 2

    ib = ibx.IB()
    ib.wrapper = Speaks()
    assert ib.waitOnUpdate(timeout=5) is True


def test_the_timeout_that_was_set_is_the_one_that_is_used():
    ib = _session()
    ib.setTimeout(0.05)
    started = time.monotonic()
    assert ib.waitOnUpdate() is False
    assert time.monotonic() - started < 1.0, "the timeout nobody read was used"


def test_a_scan_waits_for_every_row():
    """The rows arrive one at a time. Answering at the first is a scan of one.

    Paced by the reading rather than by a clock: one more row is named every
    time the session looks, so which row it stops at is the whole of what this
    measures and a loaded machine cannot change the answer.
    """
    class Paced(LiveState):
        rows = 5

        def __init__(self):
            super().__init__()
            self._named = 0

        def _name_one_more(self, req_id):
            if self._named >= self.rows:
                return
            self.scannerData(req_id, self._named, f"row-{self._named}", "", "", "", "")
            self._named += 1
            if self._named == self.rows:
                self.scannerDataEnd(req_id)

        def take_scanner(self, req_id):
            self._name_one_more(req_id)
            return super().take_scanner(req_id)

        def scanner_finished(self, req_id):
            self._name_one_more(req_id)
            return super().scanner_finished(req_id)

    ib = ibx.IB()
    ib.wrapper = Paced()
    ib.client._test_connect("DU1")

    rows = ib.reqScannerData(_subscription(), timeout=5)
    # Whole rows, as the wrapper this follows hands them back. Narrowed to the
    # contract, the rank the scan was ordered by and the distance, benchmark,
    # projection and legs beside it were dropped at the last step.
    assert [row.contractDetails for row in rows] == [
        f"row-{r}" for r in range(Paced.rows)
    ], rows


def _subscription():
    class Sub:
        instrument = "STK"
        locationCode = "STK.US.MAJOR"
        scanCode = "TOP_PERC_GAIN"
        numberOfRows = 5
    return Sub()


def test_a_summary_is_the_answer_to_the_summary_request():
    """The request is answered on its own callback. Returning the running
    account feed instead handed back a different set entirely, while the rows
    asked for reached nothing."""
    class Answers(LiveState):
        """Answers on the second look, so no clock decides the result."""

        def __init__(self):
            super().__init__()
            self._looks = 0

        def account_summary_finished(self, req_id):
            self._looks += 1
            if self._looks == 2:
                self.accountSummary(req_id, "DU1", "NetLiquidation", "12345.67", "EUR")
                self.accountSummaryEnd(req_id)
            return super().account_summary_finished(req_id)

    ib = ibx.IB()
    ib.wrapper = Answers()
    ib.client._test_connect("DU1")
    # The running account feed, which is a different set and must not be the
    # answer to this.
    ib.wrapper.updateAccountValue("Cushion", "0.9", "", "DU1")
    rows = ib.reqAccountSummary(timeout=5)
    assert [(r.tag, r.value, r.currency) for r in rows] == [
        ("NetLiquidation", "12345.67", "EUR")
    ], rows


def test_a_series_that_keeps_updating_is_refused_rather_than_faked():
    ib = _session()
    contract = ibx.Contract()
    contract.symbol = "SPY"
    contract.conId = 756733
    with pytest.raises(NotImplementedError, match="keepUpToDate"):
        ib.reqHistoricalData(contract, keepUpToDate=True)
    with pytest.raises(NotImplementedError, match="formatDate"):
        ib.reqHistoricalData(contract, formatDate=2)


def test_two_accounts_holding_one_instrument_are_two_holdings():
    """Keyed by the contract alone, an advisor saw whichever arrived last."""
    state = ibx.IB().wrapper
    contract = ibx.Contract()
    contract.conId = 756733
    state.updatePortfolio(contract, 100.0, 1.0, 100.0, 1.0, 0.0, 0.0, "DU1")
    state.updatePortfolio(contract, 200.0, 1.0, 200.0, 1.0, 0.0, 0.0, "DU2")
    held = {(p.account, p.position) for p in state.snapshot_portfolio()}
    assert held == {("DU1", 100.0), ("DU2", 200.0)}, held


def test_a_fill_replayed_names_the_client_that_placed_it():
    """Stored without the client or the order's permanent number, a request
    filtered by client matched nothing and the replay reported both as zero."""
    seen = []

    class Fills(ibx.EWrapper):
        def execDetails(self, reqId, contract, execution):
            seen.append(execution)

        def error(self, *a):
            pass

    class Filter:
        def __init__(self):
            self.symbol = self.secType = self.exchange = self.side = ""
            self.acctCode = self.time = ""
            self.clientId = 3

    w = Fills()
    c = ibx.EClient(w)
    c._test_connect("DU1")
    c._test_set_client_id(3)
    c._test_track_order(7, 1, "SPY", "BUY", 5.0, 10.0, 0)
    c._test_push_fill(1, 7, "BUY", 10.0, 5, 0, 1.25)
    c._test_dispatch_once()
    assert seen, "the fill reached nothing live"
    assert seen[0].clientId == 3

    seen.clear()
    c.req_executions(9, Filter())
    assert seen, "a request filtered by the client that placed it matched nothing"
    assert seen[0].clientId == 3
