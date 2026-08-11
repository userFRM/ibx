#!/usr/bin/env python3
"""An order's whole life through the Python client, against the venue.

`sdk_sweep.py` asks the venue for things. This tells it to do one: place an
order, change it, and withdraw it — the three a trading program does, in the
order it does them, through the same calls a program written against the
reference client uses.

The order is a buy far under the market on a paper account, so it rests and
nothing trades. It is withdrawn before this returns.

    IB_USERNAME=… IB_PASSWORD=… python3 scripts/sdk_lifecycle.py
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
    said: list[str] = []
    ib.wrapper.error = lambda r, code, m, a="": (
        said.append(f"{code}: {m[:70]}") if code not in (2104, 2106, 2158, 2100) else None
    )

    spy = ib.reqContractDetails(
        ibx.Contract(symbol="SPY", secType="STK", exchange="SMART", currency="USD")
    )[0].contract

    def settle(seconds=3.0):
        time.sleep(seconds)
        heard, said[:] = list(said), []
        return heard

    order = ibx.Order(action="BUY", orderType="LMT", totalQuantity=10, lmtPrice=100.0)
    trade = ib.placeOrder(spy, order)
    heard = settle()
    print(f"placed     {trade.orderStatus.status:12} "
          f"filled={trade.orderStatus.filled} remaining={trade.orderStatus.remaining} {heard}")
    print(f"           open trades {len(ib.openTrades())}, orders {len(ib.orders())}")

    order.lmtPrice = 101.0
    ib.placeOrder(spy, order)
    heard = settle()
    resting = [t for t in ib.openTrades() if t.order.orderId == order.orderId]
    print(f"changed    {resting[0].orderStatus.status if resting else 'gone':12} "
          f"at {resting[0].order.lmtPrice if resting else '—'} {heard}")

    ib.cancelOrder(order)
    heard = settle()
    print(f"withdrawn  {trade.orderStatus.status:12} "
          f"open trades {len(ib.openTrades())} {heard}")
    print(f"           the venue said: {[str(s) for s in getattr(trade, 'log', [])][-3:]}")

    ib.disconnect()
    return 0 if trade.orderStatus.status in ("Cancelled", "ApiCancelled") else 1


if __name__ == "__main__":
    sys.exit(main())
