#!/usr/bin/env python3
"""Call every read on the Python client against the venue, and say what came back.

The offline suites prove each call is carried. They cannot prove the venue
answers it, and an answer of nothing looks the same as a market with nothing in
it. This runs one session, asks for everything a program written against the
reference client asks for, and prints what arrived.

Nothing here places an order. The two order calls it does make are previews,
which the venue prices and does not place.

    IB_USERNAME=… IB_PASSWORD=… python3 scripts/sdk_sweep.py
"""

import os
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent / "python"))

import ibx  # noqa: E402


def main() -> int:
    ib = ibx.IB()
    ib.connect(
        username=os.environ["IB_USERNAME"],
        password=os.environ["IB_PASSWORD"],
        paper=True,
    )
    said: list[tuple[int, str]] = []
    ib.wrapper.error = lambda r, code, m, a="": said.append((code, m[:70]))

    stock = ibx.Contract(symbol="SPY", secType="STK", exchange="SMART", currency="USD")
    spy = ib.reqContractDetails(stock)[0].contract
    fx = ib.reqContractDetails(
        ibx.Contract(symbol="EUR", secType="CASH", exchange="IDEALPRO", currency="USD")
    )[0].contract

    def ask(what, call, shape=len):
        said.clear()
        try:
            answer = call()
        except Exception as e:  # noqa: BLE001 — the point is to report it
            print(f"  {what:26} raised {type(e).__name__}: {str(e).splitlines()[0][:60]}")
            return
        heard = [s for s in said if s[0] not in (2104, 2106, 2107, 2119, 2158, 2100)]
        size = "—" if answer is None else shape(answer)
        print(f"  {what:26} {size} {heard[:1] if heard else ''}")

    print("\nreference data")
    ask("contract details", lambda: ib.reqContractDetails(stock))
    ask("option chains", lambda: ib.reqSecDefOptParams("SPY", "", "STK", spy.conId))
    ask("symbol search", lambda: ib.reqMatchingSymbols("APP"))
    ask("head timestamp", lambda: ib.reqHeadTimeStamp(spy, "TRADES", True), len)
    ask("histogram", lambda: ib.reqHistogramData(spy, True, "3 days"))
    ask("fundamentals", lambda: ib.reqFundamentalData(spy, "ReportsFinSummary"), len)
    ask("trading schedule", lambda: ib.reqHistoricalSchedule(spy, "", "1 W", True), lambda s: len(s.sessions))
    ask("headlines", lambda: ib.reqHistoricalNews(spy.conId, "BRFG", "", "", 5))

    print("\nmarket data")
    ask("bars", lambda: ib.reqHistoricalData(stock, "", "2 D", "1 hour", "TRADES", True))
    ask("bars, unqualified", lambda: ib.reqHistoricalData(
        ibx.Contract(symbol="AAPL", secType="STK", exchange="SMART", currency="USD"),
        "", "1 D", "1 hour", "TRADES", True))
    ask("tickers", lambda: ib.reqTickers(spy, timeout=8), lambda t: f"bid {t[0].bid}")
    ask("currency tickers", lambda: ib.reqTickers(fx, timeout=8), lambda t: f"bid {t[0].bid}")

    def stream(kind, contract, hook, secs=8):
        got: list = []
        setattr(ib.wrapper, hook, lambda *a: got.append(a))
        ib.reqTickByTickData(contract, kind, 0, False)
        time.sleep(secs)
        ib.cancelTickByTickData(contract)
        return got

    ask("every trade", lambda: stream("AllLast", spy, "tickByTickAllLast"))
    ask("the exchange's trades", lambda: stream("Last", spy, "tickByTickAllLast"))
    ask("quote changes", lambda: stream("BidAsk", fx, "tickByTickBidAsk"))

    print("\nsubscriptions")

    def gather(hook, start, stop, secs=8):
        got: list = []
        setattr(ib.wrapper, hook, lambda *a: got.append(a))
        start()
        time.sleep(secs)
        stop()
        return got

    ask("depth of book", lambda: gather(
        "updateMktDepth", lambda: ib.reqMktDepth(spy, 5), lambda: ib.cancelMktDepth(spy)))
    ask("five-second bars", lambda: gather(
        "realtimeBar", lambda: ib.reqRealTimeBars(spy, 5, "TRADES", False),
        lambda: ib.cancelRealTimeBars(spy), 12))
    ask("profit and loss", lambda: gather(
        "pnl", lambda: ib.reqPnL(), lambda: None, 5))

    print("\naccount")
    ask("account values", lambda: ib.accountSummary())
    ask("positions", lambda: ib.positions())
    ask("managed accounts", lambda: ib.managedAccounts())
    ask("open orders", lambda: ib.reqAllOpenOrders() or ib.openOrders())
    ask("completed orders", lambda: ib.reqCompletedOrders(False) or ib.trades())

    print("\norders the venue prices and does not place")
    preview = ibx.Order(action="BUY", orderType="LMT", totalQuantity=1, lmtPrice=1.0)
    ask("a share", lambda: ib.whatIfOrder(spy, preview), lambda s: f"{s.status} {s.commissionAndFees}")
    ask("a currency pair", lambda: ib.whatIfOrder(fx, ibx.Order(
        action="BUY", orderType="LMT", totalQuantity=20000, lmtPrice=0.5)),
        lambda s: f"{s.status} {s.commissionAndFees}")

    ib.disconnect()
    return 0


if __name__ == "__main__":
    sys.exit(main())
