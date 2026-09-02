"""One contract can carry several streams, and each is withdrawn on its own.

A quote, a book, bars and a tick stream on the same contract are four
subscriptions. Held in one slot per contract, the newest overwrote the rest:
cancelling any of them sent the wrong kind of cancel under another request's
id, so the venue withdrew a subscription the caller still wanted and kept
sending the one they had asked to stop.
"""

import ibx


def _contract():
    c = ibx.Contract()
    c.symbol = "SPY"
    c.secType = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    c.conId = 756733
    return c


class Cancels:
    """Stands in for the client, recording which cancel went out and under
    which request. Nothing reaches a venue; the subscriptions are what is
    under test."""

    def __init__(self):
        self.cancelled = []

    def cancel_mkt_data(self, req_id):
        self.cancelled.append(("quote", req_id))

    def cancel_mkt_depth(self, req_id, is_smart_depth=False):
        self.cancelled.append(("depth", req_id))

    def cancel_real_time_bars(self, req_id):
        self.cancelled.append(("bars", req_id))

    def cancel_tick_by_tick_data(self, req_id):
        self.cancelled.append(("ticks", req_id))

    def req_mkt_data(self, *a):
        pass

    def req_mkt_depth(self, *a):
        pass

    def req_real_time_bars(self, *a):
        pass

    def req_tick_by_tick_data(self, *a):
        pass


def _session():
    ib = ibx.IB()
    ib.client = Cancels()
    return ib


def test_each_stream_is_withdrawn_under_its_own_request():
    ib = _session()
    contract = _contract()

    ib.reqMktData(contract)
    # `reqMktData` hands back the quote rather than its request id, so the id
    # comes from the registry the cancel reads. Left out, the one stream this
    # file is named after was the one never checked.
    quote = ib._recall("quote", contract)
    depth = ib.reqMktDepth(contract)
    bars = ib.reqRealTimeBars(contract)
    ticks = ib.reqTickByTickData(contract)
    assert len({quote, depth, bars, ticks}) == 4, "the four requests are not four requests"

    ib.cancelMktData(contract)
    ib.cancelMktDepth(contract)
    ib.cancelRealTimeBars(contract)
    ib.cancelTickByTickData(contract)

    kinds = dict(ib.client.cancelled)
    assert kinds["quote"] == quote
    assert kinds["depth"] == depth
    assert kinds["bars"] == bars
    assert kinds["ticks"] == ticks
    assert len(ib.client.cancelled) == 4, ib.client.cancelled


def test_cancelling_one_leaves_the_others_running():
    ib = _session()
    contract = _contract()
    ib.reqMktData(contract)
    quote = ib._recall("quote", contract)
    depth = ib.reqMktDepth(contract)

    # The book is checked while it is still meant to be running. Cancelling it
    # first would hide a quote cancel that takes the book with it.
    ib.cancelMktData(contract)
    assert ib.client.cancelled == [("quote", quote)], ib.client.cancelled
    assert ib._recall("depth", contract) == depth, "the book lost its id"

    ib.cancelMktDepth(contract)
    assert ("depth", depth) in ib.client.cancelled, ib.client.cancelled
