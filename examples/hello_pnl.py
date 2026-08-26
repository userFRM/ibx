"""Recipe: subscribe to account-level PnL, take one update, then cancel.

Usage:
    IB_USERNAME=... IB_PASSWORD=... python examples/hello_pnl.py
"""

import os
import threading

from ibx import EClient, EWrapper


class PnlWrapper(EWrapper):
    def __init__(self):
        self.got_pnl = threading.Event()
        self.pnl_data = None

    def pnl(self, req_id, daily_pnl, unrealized_pnl, realized_pnl):
        self.pnl_data = (daily_pnl, unrealized_pnl, realized_pnl)
        self.got_pnl.set()

    def error(self, req_id, code, msg, advanced=""):
        if code not in (2104, 2106, 2158):
            print(f"[error] {code}: {msg}")


w = PnlWrapper()
c = EClient(w)
c.connect(
    username=os.environ["IB_USERNAME"],
    password=os.environ["IB_PASSWORD"],
    host="cdc1.ibllc.com",
    paper=True,
)
threading.Thread(target=c.run, daemon=True).start()


account = c.get_account_id()
print(f"account: {account}")

req_id = 1
c.req_pnl(req_id, account)

if w.got_pnl.wait(timeout=15):
    daily, unrealized, realized = w.pnl_data
    print(f"daily={daily:.2f}  unrealized={unrealized:.2f}  realized={realized:.2f}")
else:
    print("no pnl update within 15s — try again with an open position")

c.cancel_pnl(req_id)
c.disconnect()
