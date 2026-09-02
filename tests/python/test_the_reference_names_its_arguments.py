"""A request takes its arguments under the reference client's names for them.

That client's documentation gives its arguments by name — ten of them for one
historical request — and a program written from it passes them that way. Every
one of those names was refused, so calling the reference spelling of a method
with the reference spelling of its arguments raised `TypeError` before anything
was sent.

The names below are the reference client's own, copied from its signatures.
They differ from this client's in underscores and capitals, which is settled by
letters alone, except for three that are different words entirely.
"""

import ibx


def _client():
    return ibx.EClient(ibx.EWrapper())


# (method, the reference client's keyword arguments for it). Values are only
# well-typed enough to reach the call; nothing is connected, so each request
# reports 504 rather than going anywhere.
CALLS = [
    ("reqMktData", dict(reqId=1, contract=ibx.Contract(), genericTickList="",
                        snapshot=False, regulatorySnapshot=False, mktDataOptions=[])),
    ("cancelMktData", dict(reqId=1)),
    ("reqMktDepth", dict(reqId=2, contract=ibx.Contract(), numRows=5,
                         isSmartDepth=False, mktDepthOptions=[])),
    ("reqHistoricalData", dict(reqId=3, contract=ibx.Contract(), endDateTime="",
                               durationStr="1 D", barSizeSetting="1 min",
                               whatToShow="TRADES", useRTH=True, formatDate=1,
                               keepUpToDate=False, chartOptions=[])),
    ("reqRealTimeBars", dict(reqId=4, contract=ibx.Contract(), barSize=5,
                             whatToShow="TRADES", useRTH=True, realTimeBarsOptions=[])),
    ("reqContractDetails", dict(reqId=5, contract=ibx.Contract())),
    ("reqAccountSummary", dict(reqId=6, groupName="All", tags="NetLiquidation")),
    ("reqAccountUpdates", dict(subscribe=True, acctCode="")),
    ("reqIds", dict(numIds=-1)),
    ("reqExecutions", dict(reqId=7, execFilter=ibx.ExecutionFilter())),
    ("reqHeadTimeStamp", dict(reqId=8, contract=ibx.Contract(), whatToShow="TRADES",
                              useRTH=True, formatDate=1)),
    ("reqHistogramData", dict(tickerId=9, contract=ibx.Contract(), useRTH=True,
                              timePeriod="1 week")),
    ("cancelHistogramData", dict(tickerId=9)),
    ("reqTickByTickData", dict(reqId=10, contract=ibx.Contract(), tickType="Last",
                               numberOfTicks=0, ignoreSize=False)),
    ("reqMarketDataType", dict(marketDataType=1)),
    ("reqPnL", dict(reqId=11, account="", modelCode="")),
    ("reqPnLSingle", dict(reqId=12, account="", modelCode="", conId=756733)),
    ("calculateImpliedVolatility", dict(reqId=13, contract=ibx.Contract(),
                                        optionPrice=1.0, underPrice=100.0,
                                        implVolOptions=[])),
    ("calculateOptionPrice", dict(reqId=14, contract=ibx.Contract(), volatility=0.2,
                                  underPrice=100.0, optPrcOptions=[])),
    ("reqSecDefOptParams", dict(reqId=15, underlyingSymbol="SPY",
                                futFopExchange="", underlyingSecType="STK",
                                underlyingConId=756733)),
    ("reqMatchingSymbols", dict(reqId=16, pattern="SP")),
    ("reqFundamentalData", dict(reqId=17, contract=ibx.Contract(),
                                reportType="ReportsFinSummary", fundamentalDataOptions=[])),
    ("reqNewsArticle", dict(reqId=18, providerCode="BRFG", articleId="x",
                            newsArticleOptions=[])),
    ("reqScannerSubscription", dict(reqId=19, subscription=ibx.ScannerSubscription(),
                                    scannerSubscriptionOptions=[],
                                    scannerSubscriptionFilterOptions=[])),
]


def test_every_request_takes_the_reference_clients_names_for_its_arguments():
    client = _client()
    refused = []
    for method, kwargs in CALLS:
        call = getattr(client, method, None)
        if call is None:
            refused.append(f"{method}: absent")
            continue
        try:
            call(**kwargs)
        except TypeError as why:
            refused.append(f"{method}: {why}")
    assert not refused, "\n".join(refused)


def test_a_capitalised_acronym_is_the_same_argument():
    # `useRTH` and `use_rth` are one parameter. A rule that puts a capital
    # after each underscore makes `useRth`, which is not what a caller writes.
    client = _client()
    client.reqHistoricalData(1, ibx.Contract(), "", "1 D", "1 min", "TRADES",
                             useRTH=True, formatDate=1, keepUpToDate=False,
                             chartOptions=[])
    client.req_historical_data(2, ibx.Contract(), "", "1 D", "1 min", "TRADES",
                               use_rth=True, format_date=1, keep_up_to_date=False,
                               chart_options=[])


def test_this_clients_own_names_still_answer():
    # The translation stands in front of nothing: a keyword this client names
    # is passed through as it was written.
    client = _client()
    client.reqMktData(req_id=1, contract=ibx.Contract(), generic_tick_list="",
                      snapshot=False, regulatory_snapshot=False, mkt_data_options=[])


def test_a_keyword_that_names_no_argument_is_still_refused():
    # Nothing here invents a parameter: an unknown name reaches the method and
    # is refused there, as it was before.
    client = _client()
    try:
        client.reqMktData(reqId=1, contract=ibx.Contract(), nonsense=True)
    except TypeError:
        return
    raise AssertionError("a name that names no argument was accepted")
