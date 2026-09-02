"""What the session holds, kept current while it runs."""

import ibx
from ibx._state import LiveState


class FakeContract:
    def __init__(self, con_id):
        self.conId = con_id


class FakeExecution:
    def __init__(self, exec_id, order_id=1, time="20260101 09:30:00"):
        self.execId = exec_id
        self.orderId = order_id
        self.time = time


def test_a_position_is_recorded_and_read_back():
    s = LiveState()
    s.position("DU1", FakeContract(756733), 100.0, 400.5)
    held = s.snapshot_positions()
    assert len(held) == 1
    assert held[0].position == 100.0
    assert held[0].avgCost == 400.5


def test_a_position_closed_to_zero_is_removed_not_kept_as_zero():
    """A holding of nothing is not a holding. Keeping it means a caller
    iterating positions trades something it does not own."""
    s = LiveState()
    s.position("DU1", FakeContract(756733), 100.0, 400.5)
    s.position("DU1", FakeContract(756733), 0.0, 0.0)
    assert s.snapshot_positions() == []


def test_a_trade_changes_under_the_caller_as_its_status_moves():
    s = LiveState()
    s.openOrder(7, FakeContract(1), object(), object())
    s.orderStatus(7, "Submitted", 0.0, 5.0, 0.0, 111, 0, 0.0, 1, "")
    trade = s.snapshot_trades()[0]
    assert trade.isActive() and not trade.isDone()

    s.orderStatus(7, "Filled", 5.0, 0.0, 10.0, 111, 0, 10.0, 1, "")
    trade = s.snapshot_trades()[0]
    assert trade.isDone()
    assert trade.filled() == 5.0
    assert trade.log == ["Submitted", "Filled"]


def test_a_report_stating_no_perm_id_keeps_the_one_already_learned():
    """Zero is unstated on a report. Rebuilt from whatever arrived, a later
    report carrying none erased the name the order is known by across
    sessions, and the average a fill had stated went the same way."""
    s = LiveState()
    s.orderStatus(7, "Filled", 5.0, 0.0, 10.0, 77, 0, 10.0, 1, "")
    s.orderStatus(7, "Filled", 5.0, 0.0, 0.0, 0, 0, 0.0, 1, "")
    status = s.trade_for(7).orderStatus
    assert status.permId == 77, status
    assert status.avgFillPrice == 10.0, status


def test_a_fill_is_attached_to_the_order_it_belongs_to():
    s = LiveState()
    s.openOrder(7, FakeContract(1), object(), object())
    s.execDetails(1, FakeContract(1), FakeExecution("e1", order_id=7))
    assert len(s.snapshot_trades()[0].fills) == 1


def test_an_unknown_commission_stays_unknown_rather_than_becoming_zero():
    """A zero commission and an unknown one are different facts."""
    s = LiveState()
    s.execDetails(1, FakeContract(1), FakeExecution("e1"))
    assert s.snapshot_fills()[0].commissionReport is None


def test_a_commission_finds_its_own_fill():
    s = LiveState()
    s.execDetails(1, FakeContract(1), FakeExecution("e1"))
    s.execDetails(1, FakeContract(1), FakeExecution("e2"))

    class Report:
        execId = "e1"
        commission = 1.25

    s.commissionAndFeesReport(Report())
    by_id = {f.execution.execId: f for f in s.snapshot_fills()}
    assert by_id["e1"].commissionReport.commission == 1.25
    assert by_id["e2"].commissionReport is None


def test_a_reader_gets_a_snapshot_not_the_live_list():
    """A caller iterating while the pump appends would see it change."""
    s = LiveState()
    s.position("DU1", FakeContract(1), 1.0, 1.0)
    held = s.snapshot_positions()
    s.position("DU1", FakeContract(2), 2.0, 2.0)
    assert len(held) == 1


def test_the_facade_exposes_the_live_state():
    ib = ibx.IB()
    ib.wrapper.position("DU1", FakeContract(756733), 100.0, 400.5)
    assert ib.positions()[0].position == 100.0
    assert ib.openTrades() == []


def test_a_halt_the_venue_states_reaches_the_quote():
    """`Ticker.halted` and the tick map entry for it both existed, and nothing
    routed a generic tick to either. A halted contract went on showing the
    prices standing when it stopped, with nothing to say they were stale."""
    s = LiveState()
    s.tickGeneric(1, ibx.TickTypeEnum.HALTED, 1.0)
    assert s.ticker_for(1).halted == 1.0


def test_one_execution_delivered_twice_is_one_fill():
    """A fill arrives live and again in the answer to reqExecutions, both
    carrying the venue's execution id. Counted twice, a caller adding up
    `Trade.fills` sees twice the quantity it holds."""
    s = LiveState()
    s.execDetails(-1, FakeContract(1), FakeExecution("0001.01.01", order_id=7))
    s.execDetails(9, FakeContract(1), FakeExecution("0001.01.01", order_id=7))
    assert len(s.snapshot_fills()) == 1


def test_two_different_executions_are_two_fills():
    s = LiveState()
    s.execDetails(-1, FakeContract(1), FakeExecution("0001.01.01"))
    s.execDetails(-1, FakeContract(1), FakeExecution("0001.01.02"))
    assert len(s.snapshot_fills()) == 2


def test_a_portfolio_holding_closed_to_zero_is_removed():
    """The sibling position callback already evicts. Kept here, the portfolio
    showed an instrument the account no longer held for the whole session."""
    s = LiveState()
    s.updatePortfolio(FakeContract(756733), 100.0, 4.0, 400.0, 4.0, 0.0, 0.0, "DU1")
    s.updatePortfolio(FakeContract(756733), 0.0, 4.0, 0.0, 4.0, 0.0, 0.0, "DU1")
    assert s.snapshot_portfolio() == []


def test_a_scan_row_keeps_everything_the_venue_stated():
    s = LiveState()
    s.scannerData(3, 1, "details", "0.5", "SPX", "proj", "legs")
    row = s.take_scanner(3)[0]
    assert (row.rank, row.distance, row.benchmark, row.projection, row.legsStr) == (
        1, "0.5", "SPX", "proj", "legs",
    )


def test_a_news_tick_keeps_its_extra_data():
    """The scores a caller filters on ride in the last field."""
    s = LiveState()
    s.tickNews(1, 1700000000, "BRFG", "a1", "headline", "sentiment:1.0;score:0.9")
    assert s.snapshot_news_ticks()[0][-1] == "sentiment:1.0;score:0.9"


def test_the_end_of_the_account_download_is_recorded():
    """The figures arrive one at a time and this is the only thing that says
    the rest have landed. Dropped into the base no-op, a reader that stopped at
    the first had one field of an account and read the same as a whole one."""
    s = LiveState()
    assert not s.account_download_finished()
    s.updateAccountValue("NetLiquidation", "1000", "USD", "DU1")
    assert not s.account_download_finished(), "one figure is not the whole account"
    s.accountDownloadEnd("DU1")
    assert s.account_download_finished()


def test_a_completed_order_is_recorded_where_the_caller_reads_it():
    """It arrives on a callback of its own, which fell into the inherited
    no-op, so a request for them handed back the session's ordinary trades and
    delivery read exactly like silence."""
    s = LiveState()
    assert not s.completed_orders_finished()
    s.completedOrder(FakeContract(756733), "the order", "the state")
    s.completedOrdersEnd()
    assert s.completed_orders_finished()
    (only,) = s.snapshot_completed()
    assert (only.contract.conId, only.order, only.orderState) == (
        756733, "the order", "the state",
    )
    s.forget_completed()
    assert s.snapshot_completed() == [] and not s.completed_orders_finished()


def test_a_tick_with_no_field_here_still_counts_as_the_venue_speaking():
    """`waitOnUpdate` asks whether anything arrived. A tick this class has no
    field for returned without counting, so a contract ticking steadily read
    as a dead feed."""
    s = LiveState()
    before = s.updates()
    s.tickPrice(1, 9999, 4.25)          # no field of its own here
    assert s.updates() > before
