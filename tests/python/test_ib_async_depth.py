"""A book has to reach their ticker, not just their callback.

A program written against ib_async reads `ticker.domBids`. Every check on
depth drove this client's own wrapper instead, so the whole of that path —
their reqId registry, their DOM lists, the name their callback is declared
under — was never exercised without a session. A book that arrives on one
surface and not the other is invisible to a test that only asks one.

Needs no session: the level is pushed into the same place the engine pushes it.
"""

import pytest

ib_async = pytest.importorskip("ib_async")

from ib_async import IB, Stock  # noqa: E402

import ibx.ib_async  # noqa: E402


def _attached():
    ib = ibx.ib_async.attach(IB(), username="u", password="p")
    ib.client._client._test_connect("DU000000", False)
    return ib


def test_a_book_level_reaches_their_ticker():
    ib = _attached()
    spy = Stock("SPY", "IEX", "USD")
    spy.conId = 756733

    ticker = ib.reqMktDepth(spy, numRows=10, isSmartDepth=False)
    asked_under = next(
        (rid for rid, t in ib.wrapper.reqId2Ticker.items() if t is ticker), None
    )
    assert asked_under is not None, "their request registered no ticker to fill"

    # One bid, as the engine states it: level 0, inserted, 100.25 for 300.
    ib.client._client._test_push_depth(asked_under, 0, "IEX", 0, 1, 100.25, 300.0, False)
    ib.client._client.poll()

    assert len(ticker.domBids) == 1, "the level never reached their ticker"
    assert ticker.domBids[0].price == 100.25
    assert ticker.domBids[0].size == 300.0
    assert ticker.domBids[0].marketMaker == "IEX"
    assert not ticker.domAsks, "a bid is not an ask"


def test_the_other_side_and_a_deeper_level():
    ib = _attached()
    spy = Stock("SPY", "IEX", "USD")
    spy.conId = 756733
    ticker = ib.reqMktDepth(spy, numRows=10, isSmartDepth=False)
    asked_under = next(
        rid for rid, t in ib.wrapper.reqId2Ticker.items() if t is ticker
    )

    client = ib.client._client
    client._test_push_depth(asked_under, 0, "IEX", 0, 0, 100.30, 100.0, False)
    client._test_push_depth(asked_under, 1, "IEX", 0, 0, 100.31, 200.0, False)
    client.poll()

    assert [row.price for row in ticker.domAsks] == [100.30, 100.31]
    assert not ticker.domBids


def test_a_commission_report_reaches_them_as_their_own_type():
    """What a trade cost has to arrive as something their library can read.

    Their record was named before the venue charged fees through it, and this
    client names it for what it carries now. Handed over under this name it is
    not a type they can read at all, and their own code raises on every fill —
    which is every time money moves.
    """
    import dataclasses

    from ib_async.objects import CommissionReport

    import ibx

    ours = ibx.CommissionAndFeesReport()
    ours.execId = "0000e1a7.68a1b2c3.01.01"
    ours.commission = 1.25
    ours.currency = "USD"
    ours.realizedPNL = -2.0
    ours.yield_ = 0.5

    theirs = ibx.ib_async._as_theirs(ours)

    assert isinstance(theirs, CommissionReport), "handed over as their own record"
    assert dataclasses.is_dataclass(theirs), "which their code requires it to be"
    assert theirs.execId == ours.execId
    assert theirs.commission == 1.25
    assert theirs.realizedPNL == -2.0
    assert theirs.yield_ == 0.5


def test_a_seeded_order_id_is_the_next_one_their_client_issues():
    """The persisted counter has to reach the id `placeOrder` actually uses.

    Their wrapper's `nextValidId` is a no-op; their client seeds its own
    counter and `placeOrder` takes the id from there. A counter announced but
    not seeded numbers the first order from one, which is the duplicate the
    counter exists to prevent.
    """
    pytest.importorskip("ib_async")
    import ibx.ib_async

    client = ibx.ib_async.IbxClient.__new__(ibx.ib_async.IbxClient)
    client._reqIdSeq = 1

    client.updateReqId(1_700_000_000)
    assert client.getReqId() == 1_700_000_000, "the seeded id is issued, not the one after it"
    assert client.getReqId() == 1_700_000_001

    client.updateReqId(5)
    assert client.getReqId() == 1_700_000_002, "a lower seed does not move the counter back"


def test_a_historical_tick_is_handed_over_as_their_own_record():
    """Their historical ticks are record tuples, not dataclasses.

    A conversion that only knew dataclasses handed ours straight through, so a
    caller reading `tick.priceBid` off what it was given found a field spelled
    the other way — and `tick.time` was a number where their record declares a
    datetime.
    """
    pytest.importorskip("ib_async")
    from ib_async.objects import HistoricalTickBidAsk, HistoricalTickLast

    import ibx
    import ibx.ib_async

    quote = ibx.HistoricalTickBidAsk(
        time=1_786_795_200, price_bid=1.5, price_ask=1.6, size_bid=10.0, size_ask=20.0,
    )
    theirs = ibx.ib_async._as_theirs(quote)
    assert isinstance(theirs, HistoricalTickBidAsk), "their own record"
    assert theirs.priceBid == 1.5
    assert theirs.priceAsk == 1.6
    assert theirs.sizeAsk == 20.0
    assert theirs.time.year == 2026, "and a moment, not a number"
    assert theirs.time.tzinfo is not None, "aware, which their frame conversion needs"

    trade = ibx.HistoricalTickLast(
        time=1_786_795_200, price=2.5, size=3.0, exchange="ARCA",
    )
    theirs = ibx.ib_async._as_theirs(trade)
    assert isinstance(theirs, HistoricalTickLast)
    assert theirs.price == 2.5
    assert theirs.exchange == "ARCA"
