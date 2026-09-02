"""Recipe: stream L2 market depth on AAPL + TSLA, print top-of-book on update.

Usage:
    IB_USERNAME=... IB_PASSWORD=... python examples/hello_l2.py
"""

import os
import threading
import time

from ibx import EClient, EWrapper, Contract


class Book:
    def __init__(self, symbol):
        self.symbol = symbol
        self.bids = {}   # position -> (price, size, mm)
        self.asks = {}
        self.updates = 0

    def apply(self, position, mm, operation, side, price, size):
        self.updates += 1
        book = self.bids if side == 1 else self.asks
        if operation in (0, 1):
            book[position] = (price, size, mm)
        elif operation == 2:
            book.pop(position, None)

    def top(self):
        bid = max(self.bids.values(), default=None, key=lambda v: v[0])
        ask = min(self.asks.values(), default=None, key=lambda v: v[0])
        return bid, ask


class L2Wrapper(EWrapper):
    def __init__(self, books):
        self.books = books          # req_id -> Book

    def update_mkt_depth_l2(self, req_id, position, market_maker,
                            operation, side, price, size, is_smart_depth):
        if req_id in self.books:
            self.books[req_id].apply(position, market_maker, operation, side, price, size)

    def error(self, req_id, error_time, code, msg, advanced=""):
        if code not in (2104, 2106, 2158):
            print(f"[error] {code}: {msg}")


def make_stk(con_id, symbol):
    c = Contract()
    c.con_id = con_id
    c.symbol = symbol
    c.sec_type = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


SUBSCRIPTIONS = [
    (1, make_stk(265598, "AAPL")),
    (2, make_stk(76792991, "TSLA")),
]
DURATION_SECS = int(os.environ.get("DURATION_SECS", "15"))

books = {req_id: Book(c.symbol) for req_id, c in SUBSCRIPTIONS}
w = L2Wrapper(books)
c = EClient(w)
c.connect(
    username=os.environ["IB_USERNAME"],
    password=os.environ["IB_PASSWORD"],
    host="cdc1.ibllc.com",
    paper=True,
)
threading.Thread(target=c.run, daemon=True).start()

for req_id, contract in SUBSCRIPTIONS:
    c.req_mkt_depth(req_id, contract, num_rows=5)

print(f"streaming L2 for {DURATION_SECS}s…")
time.sleep(DURATION_SECS)

for req_id, _ in SUBSCRIPTIONS:
    c.cancel_mkt_depth(req_id)

for book in books.values():
    bid, ask = book.top()
    bid_s = f"{bid[0]:.2f} x {bid[1]:.0f}" if bid else "—"
    ask_s = f"{ask[0]:.2f} x {ask[1]:.0f}" if ask else "—"
    print(f"  {book.symbol:<5} updates={book.updates:>5}  bid={bid_s:>14}   ask={ask_s:<14}")

c.disconnect()
