"""Historical bars and a scanner subscription.

Historical bars (one-shot + keepUpToDate), scanner parameters, and scanner
subscription. Exercises bar aggregation, time-range parsing, and large
response decompression.

Usage:
    IB_USERNAME=xxx IB_PASSWORD=xxx python examples/historical_scanner.py
"""

import os
import sys
import time
import threading
from ibx import EWrapper, EClient, Contract

MSFT_CON_ID = 272093


class ScannerSubscription:
    """Plain object matching the attribute interface IBX reads via getattr."""
    def __init__(self):
        self.instrument = "STK"
        self.locationCode = "STK.US.MAJOR"
        self.scanCode = "TOP_PERC_GAIN"
        self.numberOfRows = 25


class Wrapper(EWrapper):
    def __init__(self):
        super().__init__()

        # Historical bars (keyed by req_id)
        self.bars = {}            # req_id -> [bar]
        self.hist_end = {}        # req_id -> Event
        self.hist_range = {}      # req_id -> (start, end)

        # Live updating bars
        self.live_bars = {}       # req_id -> [bar]
        self.got_live_bar = {}    # req_id -> Event

        # Scanner
        self.scanner_xml = None
        self.got_scanner_params = threading.Event()
        self.scanner_results = []  # (req_id, rank, contract_details)
        self.got_scanner_end = threading.Event()

    def _ensure_req(self, req_id):
        if req_id not in self.bars:
            self.bars[req_id] = []
            self.hist_end[req_id] = threading.Event()
            self.live_bars[req_id] = []
            self.got_live_bar[req_id] = threading.Event()

    def managed_accounts(self, accounts_list):
        pass

    def connect_ack(self):
        pass

    def historical_data(self, req_id, bar):
        self._ensure_req(req_id)
        self.bars[req_id].append(bar)

    def historical_data_end(self, req_id, start, end):
        self._ensure_req(req_id)
        self.hist_range[req_id] = (start, end)
        self.hist_end[req_id].set()

    def historical_data_update(self, req_id, bar):
        self._ensure_req(req_id)
        self.live_bars[req_id].append(bar)
        self.got_live_bar[req_id].set()

    def scanner_parameters(self, xml):
        self.scanner_xml = xml
        self.got_scanner_params.set()

    def scanner_data(self, req_id, rank, contract_details, distance,
                     benchmark, projection, legs_str):
        self.scanner_results.append((req_id, rank, contract_details))

    def scanner_data_end(self, req_id):
        self.got_scanner_end.set()

    def error(self, req_id, error_time, error_code, error_string, advanced_order_reject_json=""):
        if error_code not in (2104, 2106, 2158):
            print(f"  [error] reqId={req_id} code={error_code}: {error_string}")


def make_msft():
    c = Contract()
    c.con_id = MSFT_CON_ID
    c.symbol = "MSFT"
    c.sec_type = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


def run_example():
    username = os.environ.get("IB_USERNAME", "")
    password = os.environ.get("IB_PASSWORD", "")
    if not username or not password:
        print("Set IB_USERNAME and IB_PASSWORD")
        sys.exit(1)

    w = Wrapper()
    c = EClient(w)
    c.connect(username=username, password=password,
              host=os.environ.get("IB_HOST", "cdc1.ibllc.com"), paper=True)
    t = threading.Thread(target=c.run, daemon=True)
    t.start()
    print("Connected.")

    msft = make_msft()

    # ── Step 1: Historical bars — 1D of 5-min (one-shot) ─────────────────
    print("\n=== Step 1: Historical 5-min bars (1 day, one-shot) ===")
    w._ensure_req(1)
    c.req_historical_data(
        1, msft,
        end_date_time="",
        duration_str="1 D",
        bar_size_setting="5 mins",
        what_to_show="TRADES",
        use_rth=1,
    )
    assert w.hist_end[1].wait(timeout=30), "historical_data_end not received"
    bars1 = w.bars[1]
    assert len(bars1) > 0, "Should have bars"
    print(f"  Bars received: {len(bars1)}")

    # OHLC sanity on first bar
    b = bars1[0]
    assert hasattr(b, "open") and hasattr(b, "high") and hasattr(b, "low") and hasattr(b, "close")
    assert b.high >= b.low, f"high ({b.high}) < low ({b.low})"
    assert b.high >= b.open and b.high >= b.close
    assert b.low <= b.open and b.low <= b.close
    assert 100 < b.close < 1000, f"MSFT close {b.close} out of range"
    print(f"  First bar: O={b.open} H={b.high} L={b.low} C={b.close} V={b.volume}")

    # ── Step 2: Historical bars — 5D of 1-hour ───────────────────────────
    print("\n=== Step 2: Historical 1-hour bars (5 days) ===")
    w._ensure_req(2)
    c.req_historical_data(
        2, msft,
        end_date_time="",
        duration_str="5 D",
        bar_size_setting="1 hour",
        what_to_show="TRADES",
        use_rth=1,
    )
    assert w.hist_end[2].wait(timeout=30), "historical_data_end not received"
    bars2 = w.bars[2]
    assert len(bars2) > 0, "Should have hourly bars"
    print(f"  Hourly bars received: {len(bars2)}")

    # ── Step 3: Historical bars with keepUpToDate ─────────────────────────
    print("\n=== Step 3: Historical 5-min bars (keepUpToDate) ===")
    w._ensure_req(3)
    c.req_historical_data(
        3, msft,
        end_date_time="",
        duration_str="1 D",
        bar_size_setting="5 mins",
        what_to_show="TRADES",
        use_rth=0,
        keep_up_to_date=True,
    )
    assert w.hist_end[3].wait(timeout=30), "historical_data_end not received for keepUpToDate"
    init_bars = w.bars[3]
    assert len(init_bars) > 0, "Should have initial bars"
    print(f"  Initial bars: {len(init_bars)}")

    got_live = w.got_live_bar[3].wait(timeout=30)
    c.cancel_historical_data(3)
    if got_live:
        live = w.live_bars[3]
        print(f"  Live bar updates: {len(live)}")
    else:
        print("  No live updates — market may be closed (initial batch verified)")

    # ── Step 4: Scanner parameters ────────────────────────────────────────
    print("\n=== Step 4: reqScannerParameters ===")
    c.req_scanner_parameters()
    assert w.got_scanner_params.wait(timeout=30), "scanner_parameters not received"
    xml = w.scanner_xml
    assert xml is not None and len(xml) > 100, \
        f"Scanner params XML too short: {len(xml) if xml else 0}"
    print(f"  Scanner params XML: {len(xml)} chars")

    # ── Step 5: Scanner subscription ──────────────────────────────────────
    print("\n=== Step 5: reqScannerSubscription (TOP_PERC_GAIN) ===")
    sub = ScannerSubscription()
    sub.numberOfRows = 10
    c.req_scanner_subscription(4, sub)

    got = w.got_scanner_end.wait(timeout=30)
    c.cancel_scanner_subscription(4)
    if not got:
        print("  No scanner data — market may be closed.")
    else:
        results = [r for r in w.scanner_results if r[0] == 4]
        assert len(results) > 0, "Should have scanner results"
        print(f"  Scanner returned {len(results)} instruments")

        # Verify rank ordering
        ranks = [r[1] for r in results]
        assert ranks == sorted(ranks), f"Ranks not sorted: {ranks}"
        print(f"  Ranks: {ranks}")

        # Print top results
        for r in results[:5]:
            cd = r[2]
            print(f"    #{r[1]}: {cd.contract.symbol} ({cd.contract.primary_exchange})")

    c.disconnect()
    t.join(timeout=5)
    print("\n✓ Example complete")


if __name__ == "__main__":
    run_example()
