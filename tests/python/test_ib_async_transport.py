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
