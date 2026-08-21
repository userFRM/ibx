"""A price below zero is a market, and only minus one is an absence.

The venue says "there is no such price" with minus one. Every other negative
number is a real quote: a spread priced at minus thirty-five cents, an oil
future in a month people paid to be rid of it, a tick index that lives on both
sides of zero. Dropped as absences, those instruments never had a quote at all.
"""

import ibx
from ibx._state import LiveState


def _quote(price):
    state = LiveState()
    state.tickPrice(1, ibx.TickTypeEnum.BID, price)
    return state.ticker_for(1).bid


def test_the_missing_price_sentinel_is_an_absence():
    assert _quote(-1) is None


def test_a_genuinely_negative_price_is_kept():
    assert _quote(-0.35) == -0.35
    assert _quote(-37.63) == -37.63


def test_zero_and_above_are_kept():
    assert _quote(0.0) == 0.0
    assert _quote(412.5) == 412.5
