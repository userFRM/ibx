"""Hello-world recipe: connect, wait for next_valid_id, disconnect.

Usage:
    IB_USERNAME=... IB_PASSWORD=... python examples/hello_login.py
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
    username=os.environ["IB_USERNAME"],
    password=os.environ["IB_PASSWORD"],
    paper=True,
)
threading.Thread(target=c.run, daemon=True).start()

if not w.ready.wait(timeout=15):
    raise RuntimeError("did not receive next_valid_id")

print(f"logged in. next_valid_id = {w.order_id}")

c.disconnect()
