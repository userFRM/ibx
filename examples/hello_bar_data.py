"""Recipe: fetch 1 day of 5-minute SPY bars, print first/last bar.

Usage:
    IB_USERNAME=... IB_PASSWORD=... python examples/hello_bar_data.py
"""

import os
import threading

from ibx import EClient, EWrapper, Contract


class BarsWrapper(EWrapper):
    def __init__(self):
        self.bars = []
        self.done = threading.Event()

    def historical_data(self, req_id, bar):
        self.bars.append(bar)

    def historical_data_end(self, req_id, start, end):
        self.done.set()

    def error(self, req_id, code, msg, advanced=""):
        if code not in (2104, 2106, 2158):
            print(f"[error] {code}: {msg}")


w = BarsWrapper()
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

c.req_historical_data(
    1, spy,
    end_date_time="",
    duration_str="1 D",
    bar_size_setting="5 mins",
    what_to_show="TRADES",
    use_rth=1,
)

if not w.done.wait(timeout=30):
    raise RuntimeError("historical_data_end not received")

print(f"bars: {len(w.bars)}")
if w.bars:
    first, last = w.bars[0], w.bars[-1]
    print(f"  first: {first.date}  O={first.open} H={first.high} L={first.low} C={first.close} V={first.volume}")
    print(f"  last : {last.date}  O={last.open} H={last.high} L={last.low} C={last.close} V={last.volume}")

c.disconnect()
