"""Recipe: req_contract_details for AAPL on paper, print con_id and primary exchange.

Usage:
    IB_USERNAME=... IB_PASSWORD=... python examples/hello_contract_details.py
"""

import os
import threading

from ibx import EClient, EWrapper, Contract


class DetailsWrapper(EWrapper):
    def __init__(self):
        self.rows = []
        self.done = threading.Event()

    def contract_details(self, req_id, details):
        self.rows.append(details)

    def contract_details_end(self, req_id):
        self.done.set()

    def error(self, req_id, error_time, code, msg, advanced=""):
        if code not in (2104, 2106, 2158):
            print(f"[error] {code}: {msg}")


w = DetailsWrapper()
c = EClient(w)
c.connect(
    username=os.environ["IB_USERNAME"],
    password=os.environ["IB_PASSWORD"],
    host="cdc1.ibllc.com",
    paper=True,
)
threading.Thread(target=c.run, daemon=True).start()

aapl = Contract()
aapl.symbol = "AAPL"
aapl.sec_type = "STK"
aapl.exchange = "SMART"
aapl.currency = "USD"
c.req_contract_details(1, aapl)

if not w.done.wait(timeout=15):
    raise RuntimeError("contract_details_end not received")

print(f"matches: {len(w.rows)}")
for d in w.rows:
    print(f"  con_id={d.contract.con_id:>8}  "
          f"primary={d.contract.primary_exchange:<10}  "
          f"trading_class={d.contract.trading_class}")

c.disconnect()
