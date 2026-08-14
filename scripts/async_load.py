"""Drive the async half of ib_async, concurrently, and fail if it goes quiet.

Their library is two libraries. Every call has a blocking form that runs the
event loop for you and an `...Async` form that returns a coroutine, and the
blocking one is written in terms of the other. A client that only satisfies the
blocking form satisfies half of what a program written against theirs uses —
and the half that is left is the half concurrent programs are built on.

This asks many contracts at once through the async forms, and watches the
events that are supposed to fire while it does. Nothing here places an order.

    IB_USERNAME=… IB_PASSWORD=… python scripts/async_load.py [--rounds 3]
"""

from __future__ import annotations

import argparse
import asyncio
import os
import sys

from ib_async import IB, Stock, Forex
import ibx.ib_async

#: Enough contracts to make the requests overlap, across more than one market
#: so a single quiet venue cannot make the whole run look healthy.
SUBJECTS = [
    Stock("SPY", "SMART", "USD"),
    Stock("AAPL", "SMART", "USD"),
    Stock("TSLA", "SMART", "USD"),
    Stock("QQQ", "SMART", "USD"),
    Stock("MSFT", "SMART", "USD"),
    Forex("EURUSD"),
]


async def drive(ib: IB, rounds: int) -> int:
    seen = {"tickers": 0, "errors": 0, "bars": 0}
    ib.pendingTickersEvent += lambda _tickers: seen.__setitem__("tickers", seen["tickers"] + 1)
    ib.errorEvent += lambda *_: seen.__setitem__("errors", seen["errors"] + 1)

    # Qualifying concurrently is the first thing any of their programs does,
    # and it is the call most likely to be answered out of order.
    qualified = await ib.qualifyContractsAsync(*SUBJECTS)
    named = [c for c in qualified if getattr(c, "conId", 0)]
    print(f"qualified {len(named)}/{len(SUBJECTS)} concurrently")
    if len(named) != len(SUBJECTS):
        print("  a contract came back unnamed; the rest of this proves nothing")
        return 1

    failures = []
    for round_no in range(1, rounds + 1):
        # Every contract at once, through the async form. Answered in series
        # this takes as long as the sum; the point is that it does not.
        bars = await asyncio.gather(*[
            ib.reqHistoricalDataAsync(c, "", "1 D", "1 hour", "TRADES", useRTH=False)
            for c in named
        ], return_exceptions=True)

        raised = [b for b in bars if isinstance(b, BaseException)]
        counts = [len(b) for b in bars if not isinstance(b, BaseException)]
        seen["bars"] += sum(counts)
        print(f"[round {round_no}] bars per contract: {counts}"
              + (f"  raised: {[type(e).__name__ for e in raised]}" if raised else ""))
        if raised:
            failures.append(f"round {round_no}: {len(raised)} request(s) raised")

        # The same contracts asked for as tickers, which is their other
        # concurrent entry point and goes through a different path.
        tickers = await ib.reqTickersAsync(*named, regulatorySnapshot=False)
        priced = [t for t in tickers if t and (t.last == t.last or t.close == t.close)]
        print(f"[round {round_no}] tickers priced: {len(priced)}/{len(named)}")

        await asyncio.sleep(2)

    print(f"\nevents: {seen}")
    if seen["tickers"] == 0:
        failures.append("no pending-ticker event fired in the whole run")
    if not failures:
        print("every async form answered, and the events fired")
    for what in failures:
        print(f"FAILED: {what}")
    return 1 if failures else 0


def main() -> int:
    parsed = argparse.ArgumentParser(description=__doc__)
    parsed.add_argument("--rounds", type=int, default=3)
    asked = parsed.parse_args()

    username = os.environ.get("IB_USERNAME", "")
    password = os.environ.get("IB_PASSWORD", "")
    if not username or not password:
        print("IB_USERNAME and IB_PASSWORD are unset; this needs a session.")
        return 2

    ib = ibx.ib_async.attach(IB(), username=username, password=password)
    ib.connect()

    other = ib.client.competing_session() if hasattr(ib.client, "competing_session") else None
    if other:
        print(f"another session already holds this account: {other}")
        ib.disconnect()
        return 2

    try:
        return ib.run(drive(ib, asked.rounds))
    finally:
        ib.disconnect()


if __name__ == "__main__":
    sys.exit(main())
