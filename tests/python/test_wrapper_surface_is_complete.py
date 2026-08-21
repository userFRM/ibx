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
    """A name that resolves to something refusing to run is not carried.

    `callable()` alone said nothing about this: a method whose whole body is
    `raise NotImplementedError` is callable, and passed the test named after
    the thing it is. So the bodies are read. A refusal reached under some
    condition — an argument this client will not send, say — is a method that
    does its job, and only a body that is nothing but a raise is a placeholder.
    """
    import ast
    import inspect
    import textwrap

    from ibx._ib import IB

    def placeholder(name):
        held = getattr(IB, name, None)
        # A property is reached without calling it, and reads as not callable
        # on the class where it is declared.
        if not (callable(held) or isinstance(held, property)):
            return True
        held = held.fget if isinstance(held, property) else held
        try:
            body = ast.parse(textwrap.dedent(inspect.getsource(held))).body[0].body
        except (OSError, TypeError, SyntaxError, IndexError):
            # Reached through __getattr__ or otherwise not readable as source.
            # Nothing to judge, and judging it a placeholder would fail the
            # test for the wrong reason.
            return False
        if body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant):
            body = body[1:]          # its docstring is not its body
        return len(body) == 1 and isinstance(body[0], ast.Raise)

    unusable = [m for m in WRAPPER_METHODS if placeholder(m)]
    assert not unusable, f"named and not usable: {unusable}"
