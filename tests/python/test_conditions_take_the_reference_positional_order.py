"""A condition built positionally takes its arguments in the reference client's
order. That client's `PriceCondition(triggerMethod, conId, exch, isMore, price)`
and the others each have their own order; built positionally here in a
different order, the fields landed on the wrong attributes.

Run: pytest tests/python/test_conditions_take_the_reference_positional_order.py -v
"""

from ibx import (
    ExecutionCondition, MarginCondition, PercentChangeCondition,
    PriceCondition, TimeCondition, VolumeCondition,
)


def test_price_condition_positional_order():
    p = PriceCondition(8, 265598, "SMART", False, 200.0)
    assert (p.triggerMethod, p.conId, p.exch, p.isMore, p.price) == (8, 265598, "SMART", False, 200.0)


def test_time_condition_positional_order():
    t = TimeCondition(False, "20260101 09:30:00")
    assert (t.isMore, t.time) == (False, "20260101 09:30:00")


def test_margin_condition_positional_order():
    m = MarginCondition(True, 25)
    assert (m.isMore, m.percent) == (True, 25)


def test_execution_condition_positional_order():
    e = ExecutionCondition("STK", "SMART", "AAPL")
    assert (e.secType, e.exch, e.symbol) == ("STK", "SMART", "AAPL")


def test_volume_condition_positional_order():
    v = VolumeCondition(265598, "SMART", True, 1_000_000)
    assert (v.conId, v.exch, v.isMore, v.volume) == (265598, "SMART", True, 1_000_000)


def test_percent_change_condition_positional_order():
    c = PercentChangeCondition(265598, "SMART", False, 5.0)
    assert (c.conId, c.exch, c.isMore, c.changePercent) == (265598, "SMART", False, 5.0)
