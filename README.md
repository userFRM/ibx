<p align="center">
  <img src="assets/banner.png" alt="IBX" width="100%">
</p>

<p align="center">
  <strong>A drop-in replacement for IB Gateway. No JVM, no window, no process to keep alive.</strong>
</p>

<p align="center">
  <a href="STATUS.md">Status</a> &bull;
  <a href="#python">Python</a> &bull;
  <a href="#rust">Rust</a> &bull;
  <a href="#api-surface">API surface</a> &bull;
  <a href="#notebooks">Notebooks</a> &bull;
  <a href="#architecture">Architecture</a> &bull;
  <a href="docs/engineering-notes.md">Engineering notes</a>
</p>

---

IBX implements the IBKR client protocol directly. It authenticates, maintains
the market-data, trading, historical and security-definition connections, and
exposes the same API a program would otherwise reach through IB Gateway — with
no gateway process, JVM, or local socket in between.

The API is source-compatible with the TWS API (`EClient` / `EWrapper`) and, in
Python, additionally with [ib_async](https://github.com/ib-api-reloaded/ib_async)
(`IB`). Migrating an existing program changes the connect call:

```diff
- ib.connect("127.0.0.1", 4001, clientId=1)     # requires a running gateway
+ ib.connect(username="...", password="...")    # no external process
```

## Status

[STATUS.md](STATUS.md) holds the capability matrix. Status is assigned from a
named artifact — a test, a script, or a recorded server response. 29 of 30
capabilities are verified against IBKR production servers; the remaining one
requires an advisor account.

| | |
| --- | --- |
| Requests | 76. Every one either does what it says or reports why it cannot — none returns success having sent nothing |
| Order fields | 154. 125 are sent; the other 29 have no field in the protocol to carry them, and the call says so rather than dropping them |
| Rust and Python | the same request produces the same call on both, compared against live responses |
| Tests | 1,968 offline, 157 against production servers |

Every figure above is measured on each commit, and the build fails if one moves.

## Python

### Install

```bash
uv venv .venv --python 3.13
source .venv/bin/activate          # .venv\Scripts\activate on Windows
pip install maturin
maturin develop --features python
```

### TWS API-compatible client (`EClient` / `EWrapper`)

```python
import threading
from ibx import EWrapper, EClient, Contract, Order

class App(EWrapper):
    def __init__(self):
        super().__init__()
        self.ready = threading.Event()

    def next_valid_id(self, order_id):
        self.next_id = order_id
        self.ready.set()

    def tick_price(self, req_id, tick_type, price, attrib):
        print(f"tick {tick_type}: {price}")

    def error(self, req_id, code, message, advanced_order_reject_json=""):
        if code not in (2104, 2106, 2158):
            print(f"error {code}: {message}")

app = App()
client = EClient(app)
client.connect(username="your_user", password="your_pass", paper=True)
threading.Thread(target=client.run, daemon=True).start()
app.ready.wait(timeout=10)

aapl = Contract(symbol="AAPL", secType="STK", exchange="SMART", currency="USD")
client.req_mkt_data(1, aapl, "", False)
client.req_account_summary(2, "All", "NetLiquidation,BuyingPower")
```

Both naming conventions resolve on every type and method: `reqMktData` and
`req_mkt_data`, `secType` and `sec_type`, `conId` and `con_id`.

Both surfaces drive one client and one engine. `ibx.IB` is a facade over
`EClient`; they share a session, and either may be used.

### An existing ib_async program

An unmodified program written against
[ib_async](https://github.com/ib-api-reloaded/ib_async) runs on this engine
with its connect call changed. Nothing of ib_async is copied or modified —
install it as usual, and attach:

```python
from ib_async import IB, Stock
import ibx.ib_async

ib = ibx.ib_async.attach(IB(), username="your_user", password="your_pass")
ib.connect()                      # names no host: there is no gateway

spy = Stock("SPY", "SMART", "USD")
ib.qualifyContracts(spy)
bars = ib.reqHistoricalData(spy, "", "2 D", "1 hour", "TRADES", useRTH=True)

ib.pendingTickersEvent += lambda tickers: print(len(tickers), "updates")
ib.reqMktData(spy)
ib.sleep(5)
ib.disconnect()
```

Their `IB`, their `Wrapper`, their events, their types — this engine
underneath, and no gateway process.

### The same shape, native (`ibx.IB`)

```python
import ibx

ib = ibx.IB()
ib.connect(username="your_user", password="your_pass", paper=True)

spy = ibx.Contract(symbol="SPY", secType="STK", exchange="SMART", currency="USD")

(ticker,) = ib.reqTickers(spy)
print(ticker.bid, ticker.ask)

bars = ib.reqHistoricalData(spy, "", "2 D", "1 hour", "TRADES", useRTH=True)
print(len(bars), "bars")

order = ibx.Order(action="BUY", orderType="LMT", totalQuantity=1, lmtPrice=1.00)
trade = ib.placeOrder(spy, order)
ib.sleep(2)
print(trade.orderStatus.status)
ib.cancelOrder(order)

for value in ib.accountSummary():
    if value.tag == "NetLiquidation":
        print(value.value, value.currency)

ib.disconnect()
```

A contract does not need to be qualified first: a request carrying a contract
rather than a contract id is resolved before transmission.

## Rust

```toml
[dependencies]
ibx = { git = "https://github.com/userFRM/ibx" }
```

Callback API, plus synchronous calls for requests with a single response:

```rust
use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::{Contract, Order};

let client = EClient::connect(&EClientConfig {
    username: "your_user".into(),
    password: "your_pass".into(),
    paper: true,
    ..Default::default()
})?;

let spy = client.qualify_contract(&Contract {
    symbol: "SPY".into(), sec_type: "STK".into(),
    exchange: "SMART".into(), currency: "USD".into(),
    ..Default::default()
})?;

let bars = client.historical_data(&spy, "", "2 D", "1 hour", "TRADES", true)?;
let preview = client.what_if_order(&spy, &Order {
    action: "BUY".into(), order_type: "LMT".into(),
    total_quantity: 1.0, lmt_price: 1.0, ..Default::default()
})?;
println!("{} bars, preview {}", bars.len(), preview.status);
```

A subscribed contract's quote can be read from any thread without waiting on
the callback loop:

```rust
client.req_mkt_data(1, &spy, "", false, false)?;

if let Some(instrument) = client.instrument_of(spy.con_id) {
    let quote = client.shared_state().market.quote(instrument);
    // quote.bid, quote.ask, quote.last — fixed point, 10^-8
}
```

## API surface

| Category | Calls |
| --- | --- |
| **Connection** | `connect`, `disconnect`, `is_connected`, `run`, `get_account_id` |
| **Market data** | `req_mkt_data`, `cancel_mkt_data`, `req_tick_by_tick_data`, `cancel_tick_by_tick_data`, `req_mkt_depth`, `cancel_mkt_depth`, `req_market_data_type` |
| **Orders** | `place_order`, `cancel_order`, `req_global_cancel`, `req_ids`, `req_open_orders`, `req_all_open_orders`, `req_auto_open_orders`, `req_completed_orders`, `req_executions` |
| **Account** | `req_positions`, `cancel_positions`, `req_positions_multi`, `cancel_positions_multi`, `req_account_summary`, `cancel_account_summary`, `req_account_updates`, `req_account_updates_multi`, `cancel_account_updates_multi`, `req_pnl`, `cancel_pnl`, `req_pnl_single`, `cancel_pnl_single`, `req_managed_accts` |
| **Historical** | `req_historical_data`, `cancel_historical_data`, `req_head_time_stamp`, `cancel_head_time_stamp`, `req_historical_ticks`, `req_historical_schedule`, `req_real_time_bars`, `cancel_real_time_bars`, `req_histogram_data`, `cancel_histogram_data` |
| **Reference** | `req_contract_details`, `req_matching_symbols`, `req_sec_def_opt_params`, `req_mkt_depth_exchanges`, `req_market_rule`, `req_smart_components` |
| **Scanner** | `req_scanner_parameters`, `req_scanner_subscription`, `cancel_scanner_subscription` |
| **News** | `req_news_providers`, `req_news_article`, `req_historical_news`, `req_news_bulletins`, `cancel_news_bulletins` |
| **Fundamental** | `req_fundamental_data`, `cancel_fundamental_data` |
| **Options** | `calculate_implied_volatility`, `cancel_calculate_implied_volatility`, `calculate_option_price`, `cancel_calculate_option_price`, `exercise_options` |
| **Other** | `req_current_time`, `req_user_info`, `req_family_codes`, `req_soft_dollar_tiers`, `set_server_log_level`, `req_wsh_meta_data`, `req_wsh_event_data`, `query_display_groups`, `subscribe_to_group_events` |

**Order types.** MKT, LMT, STP, STP LMT, TRAIL, TRAIL LIMIT, MOC, LOC, MTL,
MIT, LIT, MKT PRT, STP PRT, REL, PEG MKT, PEG MID, MIDPRICE, SNAP MKT,
SNAP MID, SNAP PRI, BOX TOP. Algos: VWAP, TWAP, Arrival Price, Close Price,
Dark Ice, PctVol. Conditions: price, volume, percent change, margin, execution
and time. Brackets, one-cancels-all, and combinations with a price per leg.

**Settings.** The gateway's configuration file is replaced by settings on the
client: announced build, time zone, message pacing, execution-report scope, and
others — 17 in total, readable at runtime. Seven gateway settings have no
counterpart and report why (no window geometry, no local listening socket, no
JVM heap). Rust: `EClientConfig.gateway`. Python: `ibx.configure()`.

## Performance

The engine runs on a pinned thread and does not allocate on the hot path:
socket poll → verify → decompress → decode → publish quote → drain outgoing
orders. Ticks are delivered in-process, without a localhost round trip, a JVM,
or a garbage collector.

Measured with `cargo run --release --features dev-tools --bin bench_replay`
and `bench_decode`,
1,000,000 iterations after 100,000 warm-up, no network I/O, on an Intel
i7-10700K with rustc 1.97:

| Path | Median |
| --- | ---: |
| Inbound: verify → decode → state update (5-tick message) | 214 ns |
| Inbound: same, plus seqlock publish and channel send | 252 ns |
| Outbound: build + sign a 16-field limit order | 911 ns |
| Outbound: build + sign a cancel | 687 ns |
| Outbound: build + sign a modify | 939 ns |
| Message type dispatch, body extraction | 4 ns each |

These measure this engine only. `bench/cpp` holds a TWS API harness that times
the same operations through a running gateway; the two are not directly
comparable — one is an in-process call, the other a round trip across a
localhost socket into a JVM — and no ratio between them is published here.

## Notebooks

Adapted from [ib_async's examples](https://ib-api-reloaded.github.io/ib_async/notebooks.html),
running against this engine with no gateway process.

Each subject is written twice: once in the TWS API shape, and once in
[`ib_async`'s own shape](notebooks/ib_async_nogateway/) with that library
unmodified. Only the connect line differs between the second set and the
library's own notebooks, because there is no gateway to name.

| Notebook | What it shows |
| --- | --- |
| [basics](notebooks/basics.ipynb) | Connect, positions, account summary |
| [contract_details](notebooks/contract_details.ipynb) | Contract metadata |
| [bar_data](notebooks/bar_data.ipynb) | Head timestamp, historical bars, a plot |
| [tick_data](notebooks/tick_data.ipynb) | Streaming quotes, tick-by-tick trades and quotes |
| [ordering](notebooks/ordering.ipynb) | Limit orders, cancel, market orders |
| [market_depth](notebooks/market_depth.ipynb) | The book, and the smart book across venues |
| [scanners](notebooks/scanners.ipynb) | Scanner parameters and subscriptions |

The same seven in `ib_async`'s shape: [`notebooks/ib_async_nogateway/`](notebooks/ib_async_nogateway/).

## Architecture

```
    ┌──────────────────────────────────────────────┐
    │           Your code (Rust / Python)          │
    │  process_msgs() → Wrapper callbacks          │
    │  client.quote(id) → lock-free read           │
    │  client.place_order(id,c,o) → control channel│
    └─────────┬──────────────────────┬─────────────┘
              │ events               │ commands
    ┌─────────▼──────────────────────▼─────────────────┐
    │              Engine (pinned thread)              │
    │  ┌────────────────────────────────────────────┐  │
    │  │   Encryption → Auth → Compression → Decode │  │
    │  └────────────┬───────────────┬───────────────┘  │
    └───────────────┼───────────────┼──────────────────┘
               ┌────▼───┐     ┌─────▼────┐
               │ market │     │   auth   │
               │  data  │     │  orders  │
               │  feed  │     │  control │
               └────┬───┘     └────┬─────┘
                    │              │
              ──────▼──────────────▼──────
                     IB servers
```

One pinned core polls the sockets, verifies, decompresses, decodes, updates the
quote table, and drains outgoing orders, without allocating. Quotes are read
through a seqlock from any thread; everything else arrives on the callbacks.
The Python bindings run the same engine and do not hold the GIL while reading
the wire.

### One process, one session

The logon lives in your process. An account takes one logon at a time, so two
programs on one account are two logons, and the venue hands the account to
whichever connected last: the first is told it has lost the session and stops.

Several strategies inside one process share the session and cost nothing
extra: one subscription per contract on the wire, whoever asks for it. Several
*programs* need something holding the session in front of them, which is what a
gateway's local socket does and what this client, having no socket, does not.
See [#2](https://github.com/userFRM/ibx/issues/2).

Run one after another and nothing is needed: the last order id handed out is
remembered, so a later run does not reuse ids the account already holds.

## Requirements

- Rust 2024 edition (1.85+)
- Python 3.11+ for the bindings
- An Interactive Brokers account, paper or live

## License

[AGPL-3.0](LICENSE)

## Disclaimer

Interactive Brokers®, IBKR®, Trader Workstation®, and IB Gateway® are
registered trademarks of Interactive Brokers Group, Inc. This project is **not
affiliated with, endorsed by, or supported by Interactive Brokers**.

IBX is an independent, open-source project provided "as is", without warranty
of any kind.

### How IBX Was Built

IBX was developed through **independent analysis of network traffic** between
the official IB Gateway client and IB servers. No IB software was decompiled,
disassembled, or modified. The protocol implementation was built from scratch
in Rust based solely on observed wire-level behavior.

This approach is consistent with the principle of **interoperability through
protocol analysis** — the same method used by projects like Samba (SMB/CIFS),
open-source Exchange clients, and countless other third-party implementations
of proprietary network protocols.

### Legal Considerations

- **No warranty.** IBX is provided "as is", without warranty of any kind. See [LICENSE](LICENSE) for full terms.
- **Use at your own risk.** Users are solely responsible for ensuring their use of IBX complies with Interactive Brokers' Terms of Service, Customer Agreement, and any applicable laws or regulations. Using IBX may carry risks including but not limited to account restriction or termination by IB.
- **Not financial software.** IBX is an experimental research project. It is not intended as a replacement for officially supported IB software in production trading environments. The authors accept no liability for financial losses, missed trades, account issues, or any other damages arising from the use of this software.
- **Protocol stability.** IBX relies on an undocumented protocol that IB may change at any time without notice. There is no guarantee of continued functionality.

### EU Interoperability

For users and contributors in the European Union: Article 6 of the EU Software
Directive (2009/24/EC) permits reverse engineering for the purpose of achieving
interoperability with independently created software, provided that specific
conditions are met. IBX was developed with this legal framework in mind,
enabling interoperability with IB's trading infrastructure on platforms where
the official Java-based Gateway cannot run (headless Linux, containers,
embedded systems).
