"""A condition states how it joins the next one.

The reference client's conditions carry `isConjunctionConnection`: True joins
the next condition with AND, False with OR, and its own samples set it on
every condition they build. None of the classes here carried it, so each of
those samples raised AttributeError, and an order joined by OR could not be
stated at all.
"""

import pytest
from ibx import (
    ExecutionCondition,
    MarginCondition,
    PercentChangeCondition,
    PriceCondition,
    TimeCondition,
    VolumeCondition,
)

CLASSES = [
    PriceCondition,
    TimeCondition,
    MarginCondition,
    ExecutionCondition,
    VolumeCondition,
    PercentChangeCondition,
]


@pytest.mark.parametrize("cls", CLASSES)
def test_a_condition_joins_with_and_unless_told_otherwise(cls):
    made = cls()
    assert made.isConjunctionConnection is True, "the reference client's default"
    assert made.is_conjunction_connection is True
    made.isConjunctionConnection = False
    assert made.is_conjunction_connection is False, "one attribute under two spellings"
    assert cls(isConjunctionConnection=False).isConjunctionConnection is False, "and as a keyword"
