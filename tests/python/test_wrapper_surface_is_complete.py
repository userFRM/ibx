"""Every call the widely used asynchronous wrapper has, this has.

The list is written out rather than derived, so it cannot shrink to match what
happens to be implemented. It is that wrapper's public synchronous surface.
"""

from ibx._ib import IB

WRAPPER_METHODS = [
    "connect", "disconnect", "isConnected", "waitOnUpdate", "loopUntil",
    "setTimeout", "managedAccounts", "accountValues", "accountSummary",
    "portfolio", "positions", "pnl", "pnlSingle", "trades", "openTrades",
    "orders", "openOrders", "fills", "executions", "ticker", "tickers",
    "pendingTickers", "realtimeBars", "newsTicks", "newsBulletins",
    "reqTickers", "qualifyContracts", "bracketOrder", "oneCancelsAll",
    "whatIfOrder", "placeOrder", "cancelOrder", "reqGlobalCancel",
    "reqCurrentTime", "reqAccountUpdates", "reqAccountUpdatesMulti",
    "reqAccountSummary", "reqAutoOpenOrders", "reqOpenOrders",
    "reqAllOpenOrders", "reqCompletedOrders", "reqExecutions", "reqPositions",
    "reqPnL", "cancelPnL", "reqPnLSingle", "cancelPnLSingle",
    "reqContractDetails", "reqMatchingSymbols", "reqMarketRule",
    "reqRealTimeBars", "cancelRealTimeBars", "reqHistoricalData",
    "cancelHistoricalData", "reqHistoricalSchedule", "reqHistoricalTicks",
    "reqMarketDataType", "reqHeadTimeStamp", "reqMktData", "cancelMktData",
    "reqTickByTickData", "cancelTickByTickData", "reqSmartComponents",
    "reqMktDepthExchanges", "reqMktDepth", "cancelMktDepth",
    "reqHistogramData", "reqFundamentalData", "reqScannerData",
    "reqScannerSubscription", "cancelScannerSubscription",
    "reqScannerParameters", "calculateImpliedVolatility",
    "calculateOptionPrice", "reqSecDefOptParams", "exerciseOptions",
    "reqNewsProviders", "reqNewsArticle", "reqHistoricalNews",
    "reqNewsBulletins", "cancelNewsBulletins", "requestFA", "replaceFA",
    "reqWshMetaData", "cancelWshMetaData", "reqWshEventData",
    "cancelWshEventData", "getWshMetaData", "getWshEventData", "reqUserInfo",
]


def test_the_wrapper_has_ninety_calls_and_they_are_all_here():
    assert len(WRAPPER_METHODS) == 90
    missing = [m for m in WRAPPER_METHODS if not hasattr(IB, m)]
    assert not missing, f"not carried: {missing}"


def test_none_of_them_is_a_placeholder_that_only_raises():
    """A name that resolves to something refusing to run is not carried."""
    from ibx._ib import _NOT_YET

    assert not (set(WRAPPER_METHODS) & _NOT_YET)
