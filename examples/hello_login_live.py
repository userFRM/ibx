"""Live-account login recipe: connect with paper=False, wait for next_valid_id,
disconnect. Read-only — no orders, no market data.

When the live login triggers a second-factor push, approve it on your mobile
authenticator. The connect call blocks until the gate clears.

Usage:
    IB_LIVE_USERNAME=... IB_LIVE_PASSWORD=... python examples/hello_login_live.py
"""

import os
import threading

from ibx import EClient, EWrapper


class LoginWrapper(EWrapper):
    def __init__(self):
        self.ready = threading.Event()
        self.order_id = None

    def next_valid_id(self, order_id):
        self.order_id = order_id
        self.ready.set()


w = LoginWrapper()
c = EClient(w)
c.connect(
    username=os.environ["IB_LIVE_USERNAME"],
    password=os.environ["IB_LIVE_PASSWORD"],
    host=os.environ.get("IB_HOST", ""),
    paper=False,
)
threading.Thread(target=c.run, daemon=True).start()

if not w.ready.wait(timeout=60):
    raise RuntimeError("did not receive next_valid_id")

print(f"logged in LIVE. next_valid_id = {w.order_id}")

c.disconnect()
