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
  <a href="#what-it-carries">What it carries</a> &bull;
  <a href="#notebooks">Notebooks</a> &bull;
  <a href="#how-it-is-built">Architecture</a> &bull;
  <a href="docs/engineering-notes.md">Engineering notes</a>
</p>

---

A program that trades through Interactive Brokers runs a gateway beside it: a
Java process that logs in, holds the connection, and offers a socket on
localhost. The program talks to that socket. Everything it can do, it does
through a process that has to be started, watched, restarted, and given a
screen it does not use.

IBX is that gateway, as a library. It logs in itself, holds the connections
itself, and hands your program the same API the gateway's socket does — in
Rust, and in Python through the same `EClient`/`EWrapper` and `IB` shapes a
program written against
[ibapi](https://github.com/InteractiveBrokers/tws-api) or
[ib_async](https://github.com/ib-api-reloaded/ib_async) already uses.

```diff
- ib.connect("127.0.0.1", 4001, clientId=1)     # and a gateway running somewhere
+ ib.connect(username="...", password="...")    # and nothing else
```

Everything else in the program stays as it is.

## Status

[STATUS.md](STATUS.md) is the board, and it is written from evidence: a
capability counts as working when a live session has shown it and the answer
has been read, and the row says what proved it. Anything not yet shown says so.

Today that is 29 of 30 rows proved against real servers, and the one that is
not needs an advisor account this login is not. Alongside it:

| Measure | Where it stands | Kept honest by |
| --- | --- | --- |
| Caller-facing requests | 76, none of which returns as though it acted when it did not | [`scripts/gen_wire_reach.py`](scripts/gen_wire_reach.py), in CI |
| Fields of an order | 154: 125 reach the venue, 29 say on the field that this protocol carries them nowhere, none is dropped in silence | [`scripts/gen_order_field_reach.py`](scripts/gen_order_field_reach.py), in CI |
| The two clients | Same settings, same order fields, same surface, same refusals — and the same answers from the venue | four offline gates, plus [`scripts/conformance.py`](scripts/conformance.py) against real servers |
| Tests | 1,367 offline, 489 against real servers | `cargo test`, `pytest tests/python` |

## Python

### Install

```bash
uv venv .venv --python 3.13
source .venv/bin/activate          # .venv\Scripts\activate on Windows
pip install maturin
maturin develop --features python
```

### The ib_async shape

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

The contract needs no lookup first: a request that names a contract is given
its id on the way out, the way a gateway does it.

### The ibapi shape

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

Both naming conventions resolve: `reqMktData` and `req_mkt_data`, `secType`
and `sec_type`, `conId` and `con_id`.

## Rust

```toml
[dependencies]
ibx = { git = "https://github.com/userFRM/ibx" }
```

The same callbacks, and calls that answer where a question has one answer:

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

A contract being watched can be read at any moment from any thread, without
waiting on the callback loop:

```rust
client.req_mkt_data(1, &spy, "", false, false)?;

if let Some(instrument) = client.instrument_of(spy.con_id) {
    let quote = client.shared_state().market.quote(instrument);
    // quote.bid, quote.ask, quote.last — fixed point, 10^-8
}
```

## What it carries

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

**Settings.** What a gateway reads from the file beside it is stated on the
client and read back — the build it announces, the time zone it states times
in, how many messages it paces, which executions a session opens with. Seven
of the gateway's own settings have no counterpart here and say why: there is
no window to size, no local socket to listen on, no runtime to give memory to.

## Speed

The gateway is a process on the other side of a socket. IBX is a library in
your program: a tick is decoded and handed to your code without a hop through
localhost, a JVM, or a garbage collector.

Measured in isolation, without network I/O, over a million iterations — the
harness is in [`bench/`](bench) and the counterpart it is measured against is
the official client:

| | Official client | IBX | |
| --- | --- | --- | --- |
| Reading a tick, wire to strategy | 2 ms | 340 ns | |
| Sending a limit order | 83 µs | 459 ns | |
| Cancelling one | 125 µs | 386 ns | |
| Changing one | 86 µs | 470 ns | |

## Notebooks

Adapted from [ib_async's own examples](https://ib-api-reloaded.github.io/ib_async/notebooks.html),
running against this engine with no gateway underneath.

| Notebook | What it shows |
| --- | --- |
| [basics](notebooks/basics.ipynb) | Connect, positions, account summary |
| [contract_details](notebooks/contract_details.ipynb) | Contract metadata |
| [bar_data](notebooks/bar_data.ipynb) | Head timestamp, historical bars, a plot |
| [tick_data](notebooks/tick_data.ipynb) | Streaming quotes, tick-by-tick trades and quotes |
| [ordering](notebooks/ordering.ipynb) | Limit orders, cancel, market orders |
| [market_depth](notebooks/market_depth.ipynb) | The book, and the smart book across venues |
| [scanners](notebooks/scanners.ipynb) | Scanner parameters and subscriptions |

## How it is built

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
quote table, and drains outgoing orders — without allocating. Your code reads
quotes without waiting for it, and receives everything else on the callbacks it
already has. The Python bindings are the same engine: nothing holds the GIL
while the wire is being read.

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
