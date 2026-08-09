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

from .ibx import Contract, EClient, EWrapper


class _Collector(EWrapper):
    """A wrapper that keeps what arrives instead of acting on it.

    The facade's calls take their own answers out of the session by request id,
    so this exists to satisfy the client's constructor and to keep a record of
    anything that arrives unbidden — above all the venue's errors, which a
    caller will want after the fact.
    """

    def __init__(self) -> None:
        super().__init__()
        self.errors: list[tuple[int, int, str, str]] = []

    def error(self, reqId, errorCode, errorString, advancedOrderRejectJson=""):
        self.errors.append((reqId, errorCode, errorString, advancedOrderRejectJson))


class IB:
    """One session, asked questions directly."""

    def __init__(self) -> None:
        self.wrapper = _Collector()
        self.client = EClient(self.wrapper)

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
        return self

    def disconnect(self) -> None:
        self.client.disconnect()

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
    "waitOnUpdate", "loopUntil", "setTimeout", "managedAccounts", "accountValues",
    "accountSummary", "portfolio", "positions", "pnl", "pnlSingle", "trades",
    "openTrades", "orders", "openOrders", "fills", "executions", "ticker",
    "tickers", "pendingTickers", "realtimeBars", "newsTicks", "newsBulletins",
    "reqTickers", "bracketOrder", "oneCancelsAll", "whatIfOrder", "placeOrder",
    "cancelOrder", "reqGlobalCancel", "reqCurrentTime", "reqAccountUpdates",
    "reqAccountUpdatesMulti", "reqAccountSummary", "reqAutoOpenOrders",
    "reqOpenOrders", "reqAllOpenOrders", "reqCompletedOrders", "reqExecutions",
    "reqPositions", "reqPnL", "cancelPnL", "reqPnLSingle", "cancelPnLSingle",
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
