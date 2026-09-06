"""A condition states what kind it is and how it joins the next.

The reference client's conditions carry `condType`, answer it on `type()`,
and join the next condition with `And()` or `Or()`, each handing the same
condition back so a program can chain them. None of that was here: a program
comparing `condType` against `OrderCondition.Price`, or writing
`PriceCondition().And()`, raised AttributeError.

Run: pytest tests/python/test_a_condition_names_its_kind.py -v
"""

import pytest
from ibx import (
    ExecutionCondition,
    MarginCondition,
    PercentChangeCondition,
    PriceCondition,
    TimeCondition,
    VolumeCondition,
    order_condition,
)

KINDS = [
    (PriceCondition, order_condition.OrderCondition.Price),
    (TimeCondition, order_condition.OrderCondition.Time),
    (MarginCondition, order_condition.OrderCondition.Margin),
    (ExecutionCondition, order_condition.OrderCondition.Execution),
    (VolumeCondition, order_condition.OrderCondition.Volume),
    (PercentChangeCondition, order_condition.OrderCondition.PercentChange),
]


@pytest.mark.parametrize("cls, kind", KINDS)
def test_a_condition_names_its_kind(cls, kind):
    made = cls()
    assert made.condType == kind
    assert made.cond_type == kind
    assert made.type() == kind


@pytest.mark.parametrize("cls, kind", KINDS)
def test_the_joins_set_the_connection_and_hand_the_condition_back(cls, kind):
    made = cls()
    assert made.Or() is made
    assert made.isConjunctionConnection is False
    assert made.And() is made
    assert made.isConjunctionConnection is True
