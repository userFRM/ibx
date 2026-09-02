"""Every request made before a connection is answered under 504 and returns.

The reference client opens every request with the same check: no connection,
`error(id, 504, "Not connected")` on the wrapper, return. Nothing raises and
nothing goes quiet, so a program written against it handles the case in one
place, the error callback, and a reconnect loop that asks for its open orders
after a drop is told rather than thrown at.

One row per request the reference client checks that way, so the contract
cannot drift back one method at a time.
"""

import pytest

import ibx
from conftest import NotConnectedProbe

CT = ibx.Contract()

REQUESTS = {
    "reqMktData": (1, CT, "", False, False, []),
    "cancelMktData": (1,),
    "reqMarketDataType": (3,),
    "reqTickByTickData": (1, CT, "Last", 0, False),
    "cancelTickByTickData": (1,),
    "reqMktDepth": (1, CT, 5, False, []),
    "cancelMktDepth": (1, False),
    "reqMktDepthExchanges": (),
    "reqSmartComponents": (1, "a6"),
    "reqRealTimeBars": (1, CT, 5, "TRADES", False, []),
    "cancelRealTimeBars": (1,),
    "reqHistoricalData": (1, CT, "", "1 D", "1 min", "TRADES", 1, 1, False, []),
    "cancelHistoricalData": (1,),
    "reqHeadTimeStamp": (1, CT, "TRADES", 0, 1),
    "cancelHeadTimeStamp": (1,),
    "reqHistoricalTicks": (1, CT, "", "", 10, "TRADES", 1, False, []),
    "reqHistogramData": (1, CT, False, "3 days"),
    "cancelHistogramData": (1,),
    "placeOrder": (1, CT, ibx.Order()),
    "cancelOrder": (1, ""),
    "reqOpenOrders": (),
    "reqAllOpenOrders": (),
    "reqAutoOpenOrders": (True,),
    "reqIds": (-1,),
    "reqGlobalCancel": (),
    "reqCompletedOrders": (False,),
    "reqExecutions": (1, None),
    "reqAccountUpdates": (True, ""),
    "reqAccountSummary": (1, "All", "NetLiquidation"),
    "cancelAccountSummary": (1,),
    "reqPositions": (),
    "cancelPositions": (),
    "reqPnL": (1, "", ""),
    "cancelPnL": (1,),
    "reqPnLSingle": (1, "", "", 1),
    "cancelPnLSingle": (1,),
    "reqManagedAccts": (),
    "reqAccountUpdatesMulti": (1, "", "", False),
    "cancelAccountUpdatesMulti": (1,),
    "reqPositionsMulti": (1, "", ""),
    "cancelPositionsMulti": (1,),
    "reqContractDetails": (1, CT),
    "reqMatchingSymbols": (1, "IB"),
    "reqMarketRule": (26,),
    "reqScannerParameters": (),
    "reqScannerSubscription": (1, ibx.ScannerSubscription(), [], []),
    "cancelScannerSubscription": (1,),
    "reqNewsProviders": (),
    "reqNewsArticle": (1, "BRFG", "id", []),
    "reqHistoricalNews": (1, 8314, "BRFG", "", "", 10, []),
    "reqNewsBulletins": (True,),
    "cancelNewsBulletins": (),
    "reqFundamentalData": (1, CT, "ReportsFinSummary", []),
    "cancelFundamentalData": (1,),
    "calculateImpliedVolatility": (1, CT, 0.5, 55.0, []),
    "cancelCalculateImpliedVolatility": (1,),
    "calculateOptionPrice": (1, CT, 0.6, 55.0, []),
    "cancelCalculateOptionPrice": (1,),
    "exerciseOptions": (1, CT, 1, 1, "", 1),
    "reqSecDefOptParams": (1, "IBM", "", "STK", 8314),
    "reqSoftDollarTiers": (1,),
    "reqFamilyCodes": (),
    "reqUserInfo": (1,),
    "requestFA": (1,),
    "replaceFA": (1, 1, "<x/>"),
    "queryDisplayGroups": (1,),
    "subscribeToGroupEvents": (1, 1),
    "updateDisplayGroup": (1, "8314@SMART"),
    "unsubscribeFromGroupEvents": (1,),
    "reqWshMetaData": (1,),
    "cancelWshMetaData": (1,),
    "reqWshEventData": (1, None),
    "cancelWshEventData": (1,),
    "reqCurrentTime": (),
    "reqCurrentTimeInMillis": (),
    "setServerLogLevel": (1,),
    "startApi": (),
}


@pytest.mark.parametrize("name,args", sorted(REQUESTS.items()))
def test_a_request_before_a_connection_is_answered_under_504(name, args):
    probe = NotConnectedProbe()
    client = ibx.EClient(probe)
    getattr(client, name)(*args)
    assert probe.not_connected, f"{name} reported {probe.errors}"


def test_the_unsubscribe_form_is_answered_too():
    # The guard sat inside `if subscribe`, so only half the call was answered.
    probe = NotConnectedProbe()
    ibx.EClient(probe).reqAccountUpdates(False, "")
    assert probe.not_connected, probe.errors
