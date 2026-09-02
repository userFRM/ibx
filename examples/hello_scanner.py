"""Recipe: subscribe to TOP_PERC_GAIN scanner, print first 10 results.

Usage:
    IB_USERNAME=... IB_PASSWORD=... python examples/hello_scanner.py
"""

import os
import threading

from ibx import EClient, EWrapper


class ScannerSubscription:
    def __init__(self):
        self.instrument = "STK"
        self.locationCode = "STK.US.MAJOR"
        self.scanCode = "TOP_PERC_GAIN"
        self.numberOfRows = 25
        self.abovePrice = 5.0


class ScannerWrapper(EWrapper):
    def __init__(self):
        self.rows = []
        self.done = threading.Event()

    def scanner_data(self, req_id, rank, contract_details,
                     distance, benchmark, projection, legs_str):
        self.rows.append((rank, contract_details))

    def scanner_data_end(self, req_id):
        self.done.set()

    def error(self, req_id, error_time, code, msg, advanced=""):
        if code not in (2104, 2106, 2158):
            print(f"[error] {code}: {msg}")


w = ScannerWrapper()
c = EClient(w)
c.connect(
    username=os.environ["IB_USERNAME"],
    password=os.environ["IB_PASSWORD"],
    host="cdc1.ibllc.com",
    paper=True,
)
threading.Thread(target=c.run, daemon=True).start()

req_id = 1
print("subscribing TOP_PERC_GAIN, STK.US.MAJOR…")
c.req_scanner_subscription(req_id, ScannerSubscription())

if not w.done.wait(timeout=30):
    print(f"(scanner_data_end not received; got {len(w.rows)} rows so far)")

w.rows.sort(key=lambda r: r[0])
print(f"results: {len(w.rows)}")
for rank, d in w.rows[:10]:
    print(f"  #{rank:<3}  {d.contract.symbol:<8} {d.contract.primary_exchange:<6} con_id={d.contract.con_id}")

c.cancel_scanner_subscription(req_id)
c.disconnect()
