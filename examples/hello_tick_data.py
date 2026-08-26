"""Recipe: stream SPY market data for 5 seconds, print latest bid/ask/last.

Usage:
    IB_USERNAME=... IB_PASSWORD=... python examples/hello_tick_data.py
"""

import os
import threading
import time

from ibx import EClient, EWrapper, Contract


class TickWrapper(EWrapper):
    def __init__(self):
        self.bid = 0.0
        self.ask = 0.0
        self.last = 0.0
        self.ticks = 0

    def tick_price(self, req_id, tick_type, price, attrib):
        self.ticks += 1
        if tick_type == 1:
            self.bid = price
        elif tick_type == 2:
            self.ask = price
        elif tick_type == 4:
            self.last = price

    def tick_size(self, req_id, tick_type, size):
        self.ticks += 1

    def error(self, req_id, code, msg, advanced=""):
        if code not in (2104, 2106, 2158):
            print(f"[error] {code}: {msg}")


w = TickWrapper()
c = EClient(w)
c.connect(
    username=os.environ["IB_USERNAME"],
    password=os.environ["IB_PASSWORD"],
    host="cdc1.ibllc.com",
    paper=True,
)
threading.Thread(target=c.run, daemon=True).start()

spy = Contract()
spy.con_id = 756733
spy.symbol = "SPY"
spy.sec_type = "STK"
spy.exchange = "SMART"
spy.currency = "USD"

req_id = 1
print("streaming SPY for 5s…")
c.req_mkt_data(req_id, spy, "", False, False)
deadline = time.monotonic() + 5
while time.monotonic() < deadline:
    time.sleep(0.1)
c.cancel_mkt_data(req_id)

print(f"ticks: {w.ticks}  bid={w.bid:.2f}  ask={w.ask:.2f}  last={w.last:.2f}")
c.disconnect()
