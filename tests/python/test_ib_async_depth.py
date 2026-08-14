"""A book has to reach their ticker, not just their callback.

A program written against ib_async reads `ticker.domBids`. Every check on
depth drove this client's own wrapper instead, so the whole of that path —
their reqId registry, their DOM lists, the name their callback is declared
under — was never exercised without a session. A book that arrives on one
surface and not the other is invisible to a test that only asks one.

Needs no session: the level is pushed into the same place the engine pushes it.
"""

from ib_async import IB, Stock

import ibx.ib_async


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
