"""Example #98: Account Updates + Live P&L Subscription (Portfolio).

Account summary, portfolio positions, live P&L, and pnlSingle.
Exercises cache coordination, account streaming, and signed value encoding.

Usage:
    IB_USERNAME=xxx IB_PASSWORD=xxx python examples/account_pnl.py

Ref: https://github.com/deepentropy/ib-agent/issues/98
"""

import os
import sys
import time
import threading
from ibx import EWrapper, EClient, Contract

EXPECTED_ACCOUNT_KEYS = {"NetLiquidation", "TotalCashValue", "BuyingPower"}


class Wrapper(EWrapper):
    def __init__(self):
        super().__init__()
        self.connected = threading.Event()
        self.account_id = ""

        # Account values
        self.account_values = {}  # key -> (value, currency)
        self.got_account_download_end = threading.Event()

        # Portfolio
        self.portfolio = []
        self.got_portfolio = threading.Event()

        # Positions
        self.positions = []
        self.got_position_end = threading.Event()

        # P&L
        self.pnl_data = None
        self.got_pnl = threading.Event()
        self.pnl_single_data = None
        self.got_pnl_single = threading.Event()

        # Account summary
        self.summary = {}
        self.got_summary_end = threading.Event()

    def next_valid_id(self, order_id):
        self.connected.set()

    def managed_accounts(self, accounts_list):
        self.account_id = accounts_list

    def connect_ack(self):
        pass

    def update_account_value(self, key, value, currency, account_name):
        self.account_values[key] = (value, currency)

    def update_portfolio(self, contract, position, market_price, market_value,
                         average_cost, unrealized_pnl, realized_pnl, account_name):
        self.portfolio.append({
            "symbol": contract.symbol,
            "con_id": contract.con_id,
            "position": position,
            "market_price": market_price,
            "market_value": market_value,
            "average_cost": average_cost,
            "unrealized_pnl": unrealized_pnl,
            "realized_pnl": realized_pnl,
        })
        self.got_portfolio.set()

    def update_account_time(self, timestamp):
        pass

    def account_download_end(self, account):
        self.got_account_download_end.set()

    def position(self, account, contract, pos, avg_cost):
        self.positions.append({
            "account": account,
            "symbol": contract.symbol,
            "con_id": contract.con_id,
            "position": pos,
            "avg_cost": avg_cost,
        })

    def position_end(self):
        self.got_position_end.set()

    def pnl(self, req_id, daily_pnl, unrealized_pnl, realized_pnl):
        self.pnl_data = {
            "daily_pnl": daily_pnl,
            "unrealized_pnl": unrealized_pnl,
            "realized_pnl": realized_pnl,
        }
        self.got_pnl.set()

    def pnl_single(self, req_id, pos, daily_pnl, unrealized_pnl, realized_pnl, value):
        self.pnl_single_data = {
            "pos": pos,
            "daily_pnl": daily_pnl,
            "unrealized_pnl": unrealized_pnl,
            "realized_pnl": realized_pnl,
            "value": value,
        }
        self.got_pnl_single.set()

    def account_summary(self, req_id, account, tag, value, currency):
        self.summary[tag] = (value, currency)

    def account_summary_end(self, req_id):
        self.got_summary_end.set()

    def error(self, req_id, error_code, error_string, advanced_order_reject_json=""):
        if error_code not in (2104, 2106, 2158):
            print(f"  [error] {error_code}: {error_string}")


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
    assert w.connected.wait(timeout=15), "Connection failed"
    print("Connected.")

    # ── Step 1: Account updates streaming ─────────────────────────────────
    print("\n=== Step 1: reqAccountUpdates (subscribe) ===")
    c.req_account_updates(True, "")
    assert w.got_account_download_end.wait(timeout=30), "account_download_end not received"

    assert len(w.account_values) > 0, "Should have account values"
    found = set(w.account_values.keys())
    missing = EXPECTED_ACCOUNT_KEYS - found
    assert len(missing) == 0, f"Missing keys: {missing}"

    nlv = float(w.account_values["NetLiquidation"][0])
    cash = float(w.account_values["TotalCashValue"][0])
    bp = float(w.account_values["BuyingPower"][0])
    assert nlv > 0, f"NetLiquidation should be positive, got {nlv}"
    print(f"  NetLiquidation: {nlv:,.2f}")
    print(f"  TotalCashValue: {cash:,.2f}")
    print(f"  BuyingPower:    {bp:,.2f}")
    print(f"  Total keys received: {len(w.account_values)}")

    # Unsubscribe
    c.req_account_updates(False, "")

    # ── Step 2: Positions (one-shot) ──────────────────────────────────────
    print("\n=== Step 2: reqPositions ===")
    c.req_positions()
    assert w.got_position_end.wait(timeout=15), "position_end not received"
    print(f"  Positions: {len(w.positions)}")
    for p in w.positions:
        print(f"    {p['symbol']} qty={p['position']} avg={p['avg_cost']:.2f}")

    # ── Step 3: Cross-validate positions vs portfolio ─────────────────────
    print("\n=== Step 3: Positions vs Portfolio cross-check ===")
    pos_ids = {p["con_id"] for p in w.positions if p["position"] != 0}
    port_ids = {p["con_id"] for p in w.portfolio if p["position"] != 0}
    if pos_ids:
        match = pos_ids == port_ids
        print(f"  Position conIds:  {pos_ids}")
        print(f"  Portfolio conIds: {port_ids}")
        print(f"  Match: {match}")
        assert match, f"Mismatch: pos={pos_ids} port={port_ids}"
    else:
        print("  No non-zero positions to compare.")

    # ── Step 4: Account-level P&L ─────────────────────────────────────────
    print("\n=== Step 4: reqPnL ===")
    acct = c.get_account_id()
    c.req_pnl(9001, acct)
    got = w.got_pnl.wait(timeout=15)
    c.cancel_pnl(9001)
    if not got:
        print("  No P&L data — may need open positions.")
    else:
        d = w.pnl_data
        print(f"  dailyPnL:      {d['daily_pnl']:.2f}")
        print(f"  unrealizedPnL: {d['unrealized_pnl']:.2f}")
        print(f"  realizedPnL:   {d['realized_pnl']:.2f}")
        assert isinstance(d["daily_pnl"], float)
        assert isinstance(d["unrealized_pnl"], float)
        assert isinstance(d["realized_pnl"], float)

    # ── Step 5: Single-position P&L ───────────────────────────────────────
    print("\n=== Step 5: reqPnLSingle ===")
    if not w.positions:
        print("  No positions — skipping pnlSingle.")
    else:
        con_id = w.positions[0]["con_id"]
        c.req_pnl_single(9002, acct, "", con_id)
        got = w.got_pnl_single.wait(timeout=15)
        c.cancel_pnl_single(9002)
        if not got:
            print(f"  No pnlSingle for conId={con_id}.")
        else:
            d = w.pnl_single_data
            print(f"  conId={con_id}: pos={d['pos']} daily={d['daily_pnl']:.2f} "
                  f"unrealized={d['unrealized_pnl']:.2f} value={d['value']:.2f}")

    # ── Step 6: Account summary ───────────────────────────────────────────
    print("\n=== Step 6: reqAccountSummary ===")
    tags = "NetLiquidation,TotalCashValue,BuyingPower,AvailableFunds,ExcessLiquidity,Cushion,Leverage"
    c.req_account_summary(9003, "All", tags)
    assert w.got_summary_end.wait(timeout=15), "account_summary_end not received"
    c.cancel_account_summary(9003)

    assert len(w.summary) > 0, "Should have summary tags"
    print(f"  Summary tags: {len(w.summary)}")
    for tag, (val, cur) in sorted(w.summary.items()):
        print(f"    {tag}: {val} {cur}")

    # ── Step 7: account_snapshot ──────────────────────────────────────────
    print("\n=== Step 7: account_snapshot() ===")
    # Re-subscribe briefly to populate cache
    w.got_account_download_end.clear()
    c.req_account_updates(True, "")
    w.got_account_download_end.wait(timeout=30)
    c.req_account_updates(False, "")

    snapshot = c.account_snapshot()
    if snapshot is None:
        print("  No snapshot — cache not populated.")
    else:
        print(f"  net_liquidation: {snapshot.get('net_liquidation', 'N/A')}")
        print(f"  buying_power:    {snapshot.get('buying_power', 'N/A')}")
        print(f"  total_cash_value:{snapshot.get('total_cash_value', 'N/A')}")
        assert snapshot.get("net_liquidation", 0) > 0

    c.disconnect()
    t.join(timeout=5)
    print("\n✓ Example complete")


if __name__ == "__main__":
    run_example()
