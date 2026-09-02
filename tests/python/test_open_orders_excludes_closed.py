"""Req_open_orders must not return closed orders.

Regression: order_cache was never purged and the terminal-status filter was a
3-string blacklist, so cancelled / filled orders leaked into subsequent
req_open_orders responses.

Run: pytest tests/python/test_open_orders_excludes_closed.py -v --timeout=120
"""

import os
import time
import pytest
import threading
from ibx import EWrapper, EClient, Contract, Order


pytestmark = pytest.mark.skipif(
    not (os.environ.get("IB_USERNAME") and os.environ.get("IB_PASSWORD")),
    reason="IB_USERNAME and IB_PASSWORD not set",
)

QQQ_CON_ID = 320227571


class CollectorWrapper(EWrapper):
    def __init__(self):
        super().__init__()
        self.connected = threading.Event()
        self.next_id = 0
        self.last_price = 0.0
        self.got_tick = threading.Event()
        self.statuses = {}
        self.got_cancelled = threading.Event()
        self.open_orders_batch = []
        self.got_open_order_end = threading.Event()

    def next_valid_id(self, order_id):
        self.next_id = order_id
        self.connected.set()

    def managed_accounts(self, accounts_list):
        pass

    def connect_ack(self):
        pass

    def tick_price(self, req_id, tick_type, price, attrib):
        if price > 0 and tick_type in (1, 2, 4):
            self.last_price = price
            self.got_tick.set()

    def tick_size(self, req_id, tick_type, size):
        pass

    def order_status(self, order_id, status, filled, remaining,
                     avg_fill_price, perm_id, parent_id,
                     last_fill_price, client_id, why_held, mkt_cap_price):
        self.statuses.setdefault(order_id, []).append(status)
        if status in ("Cancelled", "ApiCancelled", "Inactive"):
            self.got_cancelled.set()

    def open_order(self, order_id, contract, order, order_state):
        self.open_orders_batch.append((order_id, order_state.status))

    def open_order_end(self):
        self.got_open_order_end.set()

    def error(self, req_id, error_time, error_code, error_string, advanced_order_reject_json=""):
        if error_code not in (2104, 2106, 2158, 202):
            print(f"  [error] oid={req_id} code={error_code}: {error_string}")


def make_qqq():
    c = Contract()
    c.con_id = QQQ_CON_ID
    c.symbol = "QQQ"
    c.sec_type = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


class TestOpenOrdersExcludesClosed:
    @pytest.fixture(autouse=True)
    def setup_connection(self):
        self.wrapper = CollectorWrapper()
        self.client = EClient(self.wrapper)
        self.client.connect(
            username=os.environ["IB_USERNAME"],
            password=os.environ["IB_PASSWORD"],
            host=os.environ.get("IB_HOST", "cdc1.ibllc.com"),
            paper=True,
        )
        self.thread = threading.Thread(target=self.client.run, daemon=True)
        self.thread.start()
        assert self.wrapper.connected.wait(timeout=15), "Connection failed"
        yield
        self.client.disconnect()
        self.thread.join(timeout=5)

    def _snapshot_open(self):
        self.wrapper.open_orders_batch = []
        self.wrapper.got_open_order_end.clear()
        self.client.req_open_orders()
        assert self.wrapper.got_open_order_end.wait(timeout=15), "open_order_end never fired"
        return list(self.wrapper.open_orders_batch)

    def test_cancelled_order_not_returned_by_req_open_orders(self):
        """Place a far-from-market LMT, cancel it, then req_open_orders must not list it."""
        qqq = make_qqq()
        # Tick to anchor a sane LMT price
        self.client.req_mkt_data(7777, qqq, "", False)
        got_tick = self.wrapper.got_tick.wait(timeout=30)
        self.client.cancel_mkt_data(7777)
        if not got_tick or self.wrapper.last_price <= 0:
            pytest.skip("No price data — market may be closed")
        price = self.wrapper.last_price

        oid = self.wrapper.next_id

        # BUY LMT 30% below market — won't fill, easy to cancel
        order = Order()
        order.action = "BUY"
        order.total_quantity = 1
        order.order_type = "LMT"
        order.lmt_price = round(price * 0.70, 2)
        order.tif = "GTC"
        order.outside_rth = True
        order.transmit = True

        self.client.place_order(oid, qqq, order)
        # Wait for it to land in the open set first
        deadline = time.time() + 15
        while time.time() < deadline:
            statuses = self.wrapper.statuses.get(oid, [])
            if any(s in ("Submitted", "PreSubmitted") for s in statuses):
                break
            time.sleep(0.2)

        # Sanity: the open snapshot includes the order while it is still working
        live = self._snapshot_open()
        assert any(o[0] == oid for o in live), \
            f"Order {oid} should appear in req_open_orders before cancel; got {live}"

        # Cancel and wait for terminal status
        self.client.cancel_order(oid, "")
        assert self.wrapper.got_cancelled.wait(timeout=15), \
            f"Order {oid} never reached a cancelled/inactive status"
        # Give the engine a moment to drain CCP updates
        time.sleep(1.0)

        # First snapshot: cancelled order MUST NOT be present
        after = self._snapshot_open()
        assert not any(o[0] == oid for o in after), \
            f"Cancelled order {oid} leaked into req_open_orders: {after}"

        # Second snapshot: still gone (the cache leak made repeats grow)
        again = self._snapshot_open()
        assert not any(o[0] == oid for o in again), \
            f"Cancelled order {oid} reappeared on second req_open_orders: {again}"

        # Defense in depth: nothing returned should be in a terminal state
        for returned_oid, status in after + again:
            assert status not in ("Filled", "Cancelled", "ApiCancelled", "Inactive"), \
                f"req_open_orders returned terminal order {returned_oid} with status {status}"
