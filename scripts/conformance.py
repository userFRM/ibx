#!/usr/bin/env python3
"""Ask the venue the same questions from the Python client as the Rust one.

`cargo run --features dev-tools --bin capture_conformance` prints one block;
this prints the other.
Run with `--compare` and it runs both and reports where they differ, which is
the only way to catch the two clients agreeing offline and answering
differently in front of a real server.

    IB_USERNAME=… IB_PASSWORD=… python3 scripts/conformance.py --compare
"""

import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "python"))


def answers() -> dict[str, str]:
    import ibx

    ib = ibx.IB()
    ib.connect(
        username=os.environ["IB_USERNAME"],
        password=os.environ["IB_PASSWORD"],
        paper=True,
    )
    try:
        asked = ibx.Contract(symbol="SPY", secType="STK", exchange="SMART", currency="USD")
        details = ib.reqContractDetails(asked)
        spy = details[0].contract
        out = {
            "con_id": str(spy.conId),
            "listed_on": spy.primaryExchange,
            "min_tick": str(details[0].minTick),
            "trading_class": spy.tradingClass,
        }

        bars = ib.reqHistoricalData(asked, "", "2 D", "1 hour", "TRADES", True)
        out["bars"] = str(len(bars))
        out["first_bar"] = bars[0].date if bars else ""

        chains = ib.reqSecDefOptParams("SPY", "", "STK", spy.conId)
        out["chain_exchanges"] = ",".join(sorted(c.exchange for c in chains))

        out["symbol_matches"] = str(len(ib.reqMatchingSymbols("APP")))

        order = ibx.Order(action="BUY", orderType="LMT", totalQuantity=1, lmtPrice=1.0)
        state = ib.whatIfOrder(spy, order)
        out["preview_status"] = state.status
        out["preview_commission"] = str(state.commissionAndFees)
        return out
    finally:
        ib.disconnect()


def parse(block: str) -> dict[str, str]:
    return dict(
        line.split("=", 1)
        for line in block.splitlines()
        if "=" in line and not line.startswith("[")
    )


def main() -> int:
    mine = answers()
    if "--compare" not in sys.argv:
        for key, value in mine.items():
            print(f"{key}={value}")
        return 0

    run = subprocess.run(
        ["cargo", "run", "--quiet", "--bin", "capture_conformance"],
        cwd=ROOT, capture_output=True, text=True,
    )
    theirs = parse(run.stdout)
    if not theirs:
        print("the Rust client answered nothing:", run.stderr[-400:])
        return 1

    def alike(a: str, b: str) -> bool:
        """Same answer, whichever client wrote the number down.

        One prints a zero as `0` and the other as `0.0`; that is two spellings
        of one answer and not a disagreement.
        """
        if a == b:
            return True
        try:
            return float(a) == float(b)
        except ValueError:
            return False

    differ = []
    for key in sorted(set(mine) | set(theirs)):
        ours, rust = mine.get(key, "—"), theirs.get(key, "—")
        same = alike(ours, rust)
        print(f"{'    ' if same else '  ! '}{key:20} rust={rust!r:24} python={ours!r}")
        if not same:
            differ.append(key)

    # Bars are asked twice, seconds apart, so an hour can tick over between
    # them. That one is reported and not counted; everything else must match.
    counted = [k for k in differ if k != "bars"]
    print()
    if counted:
        print(f"the two clients disagree on: {', '.join(counted)}")
        return 1
    print("both clients answered the venue the same")
    return 0


if __name__ == "__main__":
    sys.exit(main())
