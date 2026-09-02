"""Recipe: place a BUY STP on SPY far above market, watch Submitted, then cancel.

Usage:
    IB_USERNAME=... IB_PASSWORD=... python examples/hello_stop_order.py
"""

import os
import threading

from ibx import EClient, EWrapper, Contract, Order


class OrderWrapper(EWrapper):
    def __init__(self):
        self.next_id = None
        self.submitted = threading.Event()
        self.cancelled = threading.Event()

    def next_valid_id(self, order_id):
        self.next_id = order_id

    def order_status(self, order_id, status, filled, remaining,
                     avg_fill_price, perm_id, parent_id,
                     last_fill_price, client_id, why_held, mkt_cap_price):
        print(f"[status] oid={order_id} status={status}")
        if status == "Submitted":
            self.submitted.set()
        if status == "Cancelled":
            self.cancelled.set()

    def error(self, req_id, error_time, code, msg, advanced=""):
        if code not in (2104, 2106, 2158):
            print(f"[error] {code}: {msg}")


w = OrderWrapper()
c = EClient(w)
c.connect(
    username=os.environ["IB_USERNAME"],
    password=os.environ["IB_PASSWORD"],
    host="cdc1.ibllc.com",
    paper=True,
)
threading.Thread(target=c.run, daemon=True).start()


order_id = w.next_id

spy = Contract()
spy.con_id = 756733
spy.symbol = "SPY"
spy.sec_type = "STK"
spy.exchange = "SMART"
spy.currency = "USD"

order = Order()
order.action = "BUY"
order.total_quantity = 1
order.order_type = "STP"
order.aux_price = 9999.0
order.tif = "GTC"
order.outside_rth = True

print(f"placing BUY 1 SPY STP 9999.00 (oid={order_id})")
c.place_order(order_id, spy, order)
w.submitted.wait(timeout=15)

print(f"cancelling oid={order_id}")
c.cancel_order(order_id, "")
w.cancelled.wait(timeout=15)

c.disconnect()
