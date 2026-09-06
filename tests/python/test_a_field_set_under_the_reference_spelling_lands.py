"""A field set under the reference client's spelling lands.

Every class handed back by a callback answered a read under that client's
spelling, `details.longName`, and refused the same name written: the write
raised AttributeError where the read had answered, so a program that set a
field on what it was handed, or built one of these by hand the way that
client's samples do, stopped there. Contract and Order carry a setter per
name; these classes resolve the spelling instead, for reads and now writes.

Run: pytest tests/python/test_a_field_set_under_the_reference_spelling_lands.py -v
"""

import pytest
from ibx import (
    BarData,
    ContractDescription,
    ContractDetails,
    DepthMktDataDescription,
    Execution,
    OrderAllocation,
    OrderState,
    SmartComponent,
)

MADE = [
    ("BarData", BarData),
    ("Execution", Execution),
    ("ContractDetails", ContractDetails),
    ("OrderAllocation", OrderAllocation),
    ("OrderState", OrderState),
    ("ContractDescription", lambda: ContractDescription(0, "", "", "", "", [])),
    ("DepthMktDataDescription", lambda: DepthMktDataDescription("", "", "", "", 0)),
    ("SmartComponent", SmartComponent),
]


def _reference_spelling(snake):
    head, *rest = snake.split("_")
    return head + "".join(word[:1].upper() + word[1:] for word in rest)


def _other(value):
    if isinstance(value, bool):
        return not value
    if isinstance(value, int):
        return value + 1
    if isinstance(value, float):
        return 1.5
    if isinstance(value, str):
        return "set"
    return None


@pytest.mark.parametrize("name, make", MADE, ids=[name for name, _ in MADE])
def test_a_field_set_under_the_reference_spelling_lands(name, make):
    made = make()
    probed = 0
    for snake in dir(made):
        if snake.startswith("_") or "_" not in snake or callable(getattr(made, snake)):
            continue
        value = _other(getattr(made, snake))
        if value is None:
            continue
        try:
            setattr(made, snake, value)
        except AttributeError:
            continue  # not writable under either spelling
        again = _other(value)
        setattr(made, _reference_spelling(snake), again)
        assert getattr(made, snake) == again, snake
        probed += 1
    assert probed, "nothing probed"


def test_a_name_neither_spelling_carries_still_raises():
    with pytest.raises(AttributeError):
        ContractDetails().noSuchField = 1
