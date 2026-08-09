"""The shape of the widely used asynchronous wrapper, over this client.

Method names, argument names and return types follow that wrapper so a program
written against it runs here. Where it returns a list, this returns a list; a
method it spells ``reqHistoricalData`` is spelled that here too.

This is a facade over ``EClient``, not a second client. It holds one session and
answers from it. What it cannot yet answer it says so plainly rather than
returning an empty list, because an empty list is indistinguishable from a
market with nothing in it.
"""

from __future__ import annotations

import threading

from ._state import LiveState
from .ibx import Contract, EClient


class IB:
    """One session, asked questions directly."""

    def __init__(self) -> None:
        self.wrapper = LiveState()
        self.client = EClient(self.wrapper)
        self._pump: threading.Thread | None = None
        self._stop = threading.Event()

    # -- session ---------------------------------------------------------

    def connect(
        self,
        host="",
        port=0,
        clientId=1,
        timeout=4,
        readonly=False,
        account="",
        username="",
        password="",
        paper=True,
    ):
        """Open the session.

        The host and port name a local process in the wrapper this follows.
        There is no local process here — this client logs in itself — so they
        are accepted and ignored, and the credentials are given here instead.
        A program that already holds a session needs no edit; one that relied
        on a running gateway to have logged in supplies the login here.

        ``readonly`` is carried through: a read-only session refuses to send
        anything that changes a position, and says so rather than appearing to
        have sent it.
        """
        del host, port, timeout, account
        self.client.connect(
            client_id=clientId,
            username=username,
            password=password,
            paper=paper,
            readonly=readonly,
        )
        self._start_pump()
        return self

    def _start_pump(self) -> None:
        """Keep dispatch running so the live state stays current.

        A daemon thread, so it never holds a program open. It owns dispatch;
        the calls that answer take their replies by request id and are left
        alone by it, so the two run together rather than competing.
        """
        if self._pump is not None and self._pump.is_alive():
            return
        self._stop.clear()

        def pump():
            try:
                # Loops until the session closes, releasing the interpreter
                # lock while it waits, so it costs a thread and not a core.
                self.client.run()
            except Exception:
                # A session that has gone ends the pump. connect() starts a new
                # one; nothing here tries to reconnect behind the caller's back.
                return

        self._pump = threading.Thread(target=pump, name="ibx-pump", daemon=True)
        self._pump.start()

    def disconnect(self) -> None:
        self._stop.set()
        self.client.disconnect()
        if self._pump is not None:
            self._pump.join(timeout=2.0)
            self._pump = None

    # -- what the session currently holds --------------------------------

    def positions(self):
        return self.wrapper.snapshot_positions()

    def portfolio(self):
        return self.wrapper.snapshot_portfolio()

    def accountValues(self):
        return self.wrapper.snapshot_account_values()

    def trades(self):
        return self.wrapper.snapshot_trades()

    def openTrades(self):
        return [t for t in self.wrapper.snapshot_trades() if t.isActive()]

    def orders(self):
        return [t.order for t in self.wrapper.snapshot_trades() if t.order is not None]

    def openOrders(self):
        return [t.order for t in self.openTrades() if t.order is not None]

    def fills(self):
        return self.wrapper.snapshot_fills()

    def executions(self):
        return [f.execution for f in self.wrapper.snapshot_fills()]

    def managedAccounts(self):
        return self.wrapper.snapshot_accounts()

    # -- asking the venue to start sending it ----------------------------

    def reqPositions(self):
        self.client.req_positions()
        return self.positions()

    def reqAccountUpdates(self, account=""):
        self.client.req_account_updates(True, account)
        return self.accountValues()

    def reqOpenOrders(self):
        self.client.req_open_orders()
        return self.openTrades()

    def reqAllOpenOrders(self):
        self.client.req_all_open_orders()
        return self.trades()

    def reqExecutions(self, execFilter=None):
        del execFilter
        self.client.req_executions(1)
        return self.fills()

    def reqManagedAccts(self):
        self.client.req_managed_accts()
        return self.managedAccounts()

    def isConnected(self) -> bool:
        return self.client.isConnected()

    # -- reference data --------------------------------------------------

    def reqContractDetails(self, contract: Contract):
        return self.client.contract_details(contract)

    def qualifyContracts(self, *contracts: Contract):
        """Fill in what the venue knows about each contract, above all its id.

        Returns the contracts it could resolve, and updates each argument in
        place, which is what the wrapper this follows does.
        """
        resolved = []
        for c in contracts:
            filled = self.client.qualify_contract(c)
            for name in (
                "conId", "symbol", "secType", "lastTradeDateOrContractMonth",
                "strike", "right", "multiplier", "exchange", "primaryExchange",
                "currency", "localSymbol", "tradingClass",
            ):
                try:
                    setattr(c, name, getattr(filled, name))
                except AttributeError:
                    pass
            resolved.append(c)
        return resolved

    def reqMatchingSymbols(self, pattern: str):
        return self.client.matching_symbols(pattern)

    # -- historical data -------------------------------------------------

    def reqHistoricalData(
        self,
        contract: Contract,
        endDateTime="",
        durationStr="1 D",
        barSizeSetting="1 min",
        whatToShow="TRADES",
        useRTH=True,
        formatDate=1,
        keepUpToDate=False,
        chartOptions=None,
    ):
        del formatDate, keepUpToDate, chartOptions
        return self.client.historical_data(
            contract,
            endDateTime,
            durationStr,
            barSizeSetting,
            whatToShow,
            1 if useRTH else 0,
        )

    def reqHeadTimeStamp(self, contract: Contract, whatToShow="TRADES", useRTH=True, formatDate=1):
        del formatDate
        return self.client.head_timestamp(contract, whatToShow, 1 if useRTH else 0)

    def reqHistogramData(self, contract: Contract, useRTH=True, period="3 days"):
        return self.client.histogram_data(contract, useRTH, period)

    def reqFundamentalData(self, contract: Contract, reportType: str, fundamentalDataOptions=None):
        del fundamentalDataOptions
        return self.client.fundamental_data(contract, reportType)

    # -- what has not been carried across yet ----------------------------

    def __getattr__(self, name: str):
        """Say plainly that a call is not carried yet.

        The wrapper this follows has ninety methods. A name it has and this does
        not raises where it is called, naming itself — rather than resolving to
        something that quietly returns nothing.
        """
        if name in _NOT_YET:
            def unavailable(*args, **kwargs):
                raise NotImplementedError(
                    f"IB.{name}() is not carried on this client yet; "
                    f"EClient carries the request under the reference client's name"
                )

            return unavailable
        raise AttributeError(f"'IB' object has no attribute '{name}'")


#: Every method the wrapper this follows has, that this facade does not carry
#: yet. Listed rather than inferred so that the count is a fact and not an
#: impression, and so a caller gets a named refusal instead of an AttributeError
#: that reads like a typo.
_NOT_YET = frozenset({
    "waitOnUpdate", "loopUntil", "setTimeout", 
    "accountSummary", "pnl", "pnlSingle", 
    "ticker",
    "tickers", "pendingTickers", "realtimeBars", "newsTicks", "newsBulletins",
    "reqTickers", "bracketOrder", "oneCancelsAll", "whatIfOrder", "placeOrder",
    "cancelOrder", "reqGlobalCancel", "reqCurrentTime", 
    "reqAccountUpdatesMulti", "reqAccountSummary", "reqAutoOpenOrders",
    "reqCompletedOrders", 
    "reqPnL", "cancelPnL", "reqPnLSingle", "cancelPnLSingle",
    "reqMarketRule", "reqRealTimeBars", "cancelRealTimeBars",
    "cancelHistoricalData", "reqHistoricalSchedule", "reqHistoricalTicks",
    "reqMarketDataType", "reqMktData", "cancelMktData", "reqTickByTickData",
    "cancelTickByTickData", "reqSmartComponents", "reqMktDepthExchanges",
    "reqMktDepth", "cancelMktDepth", "reqScannerData", "reqScannerSubscription",
    "cancelScannerSubscription", "reqScannerParameters",
    "calculateImpliedVolatility", "calculateOptionPrice", "reqSecDefOptParams",
    "exerciseOptions", "reqNewsProviders", "reqNewsArticle", "reqHistoricalNews",
    "reqNewsBulletins", "cancelNewsBulletins", "requestFA", "replaceFA",
    "reqWshMetaData", "cancelWshMetaData", "reqWshEventData", "cancelWshEventData",
    "getWshMetaData", "getWshEventData", "reqUserInfo",
})
