"""An unmodified ib_async program, running on this engine.

ib_async is the client most Python programs for this venue are written
against. What this proves is that such a program runs here with one line
changed — the attach — and no gateway process anywhere.

Skipped where ib_async is not installed: it is not a dependency of this
package, and is not vendored. The live parts need credentials.
"""

import os

import pytest

ib_async = pytest.importorskip("ib_async")

import ibx.ib_async  # noqa: E402

LIVE = bool(os.environ.get("IB_USERNAME") and os.environ.get("IB_PASSWORD"))
needs_venue = pytest.mark.skipif(not LIVE, reason="IB_USERNAME/IB_PASSWORD not set")


def test_attach_replaces_only_the_transport():
    ib = ibx.ib_async.attach(ib_async.IB())
    # Their object, their wrapper, their events — this client underneath.
    assert isinstance(ib, ib_async.IB)
    assert isinstance(ib.wrapper, ib_async.wrapper.Wrapper)
    assert ib.client.__class__ is ibx.ib_async.IbxClient
    assert not ib.isConnected()


def test_a_request_their_client_carries_and_this_one_does_not_says_so():
    ib = ibx.ib_async.attach(ib_async.IB())
    with pytest.raises(NotImplementedError, match="not carried"):
        ib.client.reqSomethingNobodyCarries(1)


@needs_venue
def test_an_unmodified_program_runs_on_this_engine():
    ib = ibx.ib_async.attach(ib_async.IB())
    ib.connect("no gateway", 0, clientId=1)
    try:
        assert ib.isConnected()
        assert ib.managedAccounts(), "the session names its account"

        spy = ib_async.Stock("SPY", "SMART", "USD")
        (qualified,) = ib.qualifyContracts(spy)
        assert qualified.conId > 0, "their qualify, answered here"

        bars = ib.reqHistoricalData(spy, "", "2 D", "1 hour", "TRADES", useRTH=True)
        assert len(bars) > 1, "their bars, in their own type"
        assert bars[-1].close > 0

        # Their event system, which is what their programs are built on.
        heard = []
        ib.pendingTickersEvent += lambda tickers: heard.append(len(tickers))
        ib.reqMktData(qualified)
        ib.sleep(4)
        ib.cancelMktData(qualified)
        assert sum(heard) > 0, "their tickers reached their event"
    finally:
        ib.disconnect()


@needs_venue
def test_an_order_lives_its_whole_life_through_their_api():
    """Placed, changed and withdrawn, in their objects.

    A limit far under the market, so it rests and nothing trades.
    """
    ib = ibx.ib_async.attach(ib_async.IB())
    ib.connect("no gateway", 0, clientId=1)
    try:
        ib.RequestTimeout = 15
        spy = ib_async.Stock("SPY", "SMART", "USD")
        (spy,) = ib.qualifyContracts(spy)

        state = ib.whatIfOrder(spy, ib_async.LimitOrder("BUY", 1, 1.00))
        assert state.status, "the venue prices it without placing it"
        assert state.commission < 1e300, "their unset is not a commission"

        order = ib_async.LimitOrder("BUY", 10, 100.00)
        trade = ib.placeOrder(spy, order)
        ib.sleep(3)
        assert trade.orderStatus.status in ("PreSubmitted", "Submitted")

        order.lmtPrice = 101.00
        ib.placeOrder(spy, order)
        ib.sleep(3)
        assert trade.order.lmtPrice == 101.00

        ib.cancelOrder(order)
        ib.sleep(3)
        assert trade.orderStatus.status == "Cancelled"
        assert [entry.status for entry in trade.log][-1] == "Cancelled"
    finally:
        ib.disconnect()


def test_a_bar_is_dated_the_way_their_parser_reads_one():
    """Their own first documented example is `util.df(bars)`.

    ib_async parses the date itself, and decides the shape from the string: a
    day from eight digits, a moment in seconds from all digits, and an aware
    moment from a date, a time and a zone separated by single spaces. Anything
    else it reads as naive, and their frame conversion refuses a naive one.
    This engine states the date and time joined by a dash, with the zone beside
    them.
    """
    from ib_async.util import parseIBDatetime

    from ibx.ib_async import _as_their_moment

    day = _as_their_moment("20260812", "US/Eastern")
    assert parseIBDatetime(day).isoformat() == "2026-08-12"

    minute = _as_their_moment("20260812-13:30:00", "US/Eastern")
    read = parseIBDatetime(minute)
    assert read.tzinfo is not None, "a naive moment cannot be converted to a zone"
    assert read.isoformat() == "2026-08-12T13:30:00-04:00"

    # A moment stated in seconds, and anything not a date, are handed over as
    # they stand.
    assert _as_their_moment("1786109400", "") == "1786109400"
    assert _as_their_moment("", "US/Eastern") == ""


def test_ending_a_session_is_not_a_session_that_went_away():
    """Their wrapper's `connectionClosed` fails every waiting request and
    raises on their global error event. That is a socket that dropped, not a
    caller who asked to stop, and their own client does not call it here."""
    import inspect

    from ibx.ib_async import IbxClient

    ends = inspect.getsource(IbxClient.disconnect)
    assert "connectionClosed" not in ends.split('"""')[-1]
