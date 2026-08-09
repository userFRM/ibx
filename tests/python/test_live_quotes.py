"""A quote does not arrive as a quote. It arrives as the ticks that make it."""

import ibx
from ibx._state import LiveState

T = ibx.TickTypeEnum


def test_a_quote_is_assembled_from_the_ticks_that_make_it():
    s = LiveState()
    s.tickPrice(1, T.BID, 100.0)
    s.tickSize(1, T.BID_SIZE, 300)
    s.tickPrice(1, T.ASK, 100.5)
    s.tickSize(1, T.ASK_SIZE, 200)
    t = s.ticker_for(1)
    assert (t.bid, t.bidSize, t.ask, t.askSize) == (100.0, 300, 100.5, 200)
    assert t.hasBidAsk()
    assert t.midpoint() == 100.25


def test_a_field_nobody_sent_stays_unknown_rather_than_zero():
    """A bid of zero and no bid at all are different markets, and the
    difference decides whether an order should be sent."""
    s = LiveState()
    s.tickPrice(1, T.ASK, 100.5)
    t = s.ticker_for(1)
    assert t.bid is None
    assert not t.hasBidAsk()
    assert t.midpoint() is None


def test_the_previous_price_is_kept_when_a_new_one_arrives():
    s = LiveState()
    s.tickPrice(1, T.BID, 100.0)
    s.tickPrice(1, T.BID, 101.0)
    t = s.ticker_for(1)
    assert (t.prevBid, t.bid) == (100.0, 101.0)


def test_a_last_trade_outside_the_spread_is_not_used_to_value_a_holding():
    """The market moved away from it. Valuing at it marks a position to a
    price nobody would deal at."""
    s = LiveState()
    s.tickPrice(1, T.BID, 100.0)
    s.tickPrice(1, T.ASK, 100.5)
    s.tickPrice(1, T.LAST, 100.2)
    assert s.ticker_for(1).marketPrice() == 100.2

    s.tickPrice(1, T.LAST, 90.0)
    assert s.ticker_for(1).marketPrice() == 100.25


def test_with_no_market_at_all_the_close_is_used():
    s = LiveState()
    s.tickPrice(1, T.CLOSE, 99.0)
    assert s.ticker_for(1).marketPrice() == 99.0


def test_only_the_quotes_that_changed_are_pending_and_reading_clears_them():
    s = LiveState()
    s.tickPrice(1, T.BID, 100.0)
    s.tickPrice(2, T.BID, 50.0)
    assert len(s.take_pending()) == 2
    assert s.take_pending() == []

    s.tickPrice(2, T.ASK, 50.5)
    assert len(s.take_pending()) == 1


def test_a_tick_this_does_not_carry_is_not_an_error():
    s = LiveState()
    s.tickPrice(1, 9999, 1.0)
    assert s.ticker_for(1) is None or s.ticker_for(1).bid is None


def test_the_time_of_the_last_trade_is_recorded():
    s = LiveState()
    s.tickString(1, T.LAST_TIMESTAMP, "1767000000")
    assert s.ticker_for(1).time == "1767000000"
