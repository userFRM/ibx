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
