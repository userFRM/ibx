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


def _off_by_a_bar(mine: dict[str, str], theirs: dict[str, str]) -> bool:
    """Whether the two bar counts differ by the one bar an hour ticking over adds."""
    try:
        return abs(int(mine["bars"]) - int(theirs["bars"])) <= 1
    except (KeyError, ValueError):
        return False


def _window_slid(mine: dict[str, str], theirs: dict[str, str]) -> bool:
    """Whether the oldest bar differs because the window moved, not the client.

    The two clients are asked seconds apart, so an hour boundary between them
    adds a bar at one end and drops one at the other: the counts differ by one
    and the oldest bar differs with them. Equal counts and a different oldest
    bar is the two clients disagreeing about the same window, which is a real
    difference and is counted.
    """
    try:
        return int(mine["bars"]) != int(theirs["bars"]) and _off_by_a_bar(mine, theirs)
    except (KeyError, ValueError):
        return False


def _selftest() -> int:
    assert _off_by_a_bar({"bars": "14"}, {"bars": "15"}), "an hour ticked over"
    assert _off_by_a_bar({"bars": "14"}, {"bars": "14"})
    assert not _off_by_a_bar({"bars": "2"}, {"bars": "50"}), "that is a disagreement"
    assert not _off_by_a_bar({"bars": "14"}, {}), "a missing answer is not agreement"
    assert _window_slid({"bars": "14"}, {"bars": "15"}), "the window moved by one bar"
    assert not _window_slid({"bars": "14"}, {"bars": "14"}), (
        "the same window: a different oldest bar is the clients disagreeing"
    )
    assert not _window_slid({"bars": "2"}, {"bars": "50"}), "that is a disagreement"
    assert not _window_slid({"bars": "14"}, {}), "a missing answer is not agreement"
    assert not _off_by_a_bar({"bars": "—"}, {"bars": "14"})
    print("ok")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return _selftest()
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
    # them and one client see a bar the other did not. That is the whole of
    # what is excused: the count was excused outright, so fifty bars against
    # two went uncounted too, and the field most likely to expose a real
    # disagreement was the one field that could not fail.
    counted = [
        k
        for k in differ
        if not (k == "bars" and _off_by_a_bar(mine, theirs))
        and not (k == "first_bar" and _window_slid(mine, theirs))
    ]
    print()
    if counted:
        print(f"the two clients disagree on: {', '.join(counted)}")
        return 1
    print("both clients answered the venue the same")
    return 0


if __name__ == "__main__":
    sys.exit(main())
