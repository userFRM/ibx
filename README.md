<p align="center">
  <img src="docs/book/src/banner.png" alt="IBX" width="100%">
</p>

<p align="center">
  <strong>An Interactive Brokers client with no gateway. No JVM, no window, no process to keep alive.</strong>
</p>

<p align="center">
  <a href="https://github.com/userFRM/ibx/actions"><img src="https://github.com/userFRM/ibx/actions/workflows/tests.yml/badge.svg" alt="Build"></a>
  <img src="https://img.shields.io/badge/rust-1.85+-orange.svg" alt="Rust version">
  <img src="https://img.shields.io/badge/python-3.11+-blue.svg" alt="Python version">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-blue.svg" alt="License"></a>
  <a href="https://userfrm.github.io/ibx/"><img src="https://img.shields.io/badge/docs-book-green.svg" alt="Docs"></a>
</p>

## Introduction

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

### What this removes

* **The gateway process** — nothing to install, launch, log into, or restart
* **The JVM** — no heap to size, no crash on bulk data
* **The localhost socket** — ticks are delivered in-process
* **The window** — runs headless, in a container, over ssh

### Requirements

* An Interactive Brokers account, paper or live
* Rust 1.85+ (2024 edition) for the Rust client
* Python 3.11+ for the bindings

No IB software is required. The `ibapi` package is not needed either.

## Installation

### Python

```bash
uv venv .venv --python 3.13
source .venv/bin/activate          # .venv\Scripts\activate on Windows
pip install maturin
maturin develop --features python
```

### Rust

```toml
[dependencies]
ibx = { git = "https://github.com/userFRM/ibx" }
```

## Quick start

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

ib.disconnect()
```

A contract does not need to be qualified first: a request carrying a contract
rather than a contract id is resolved before transmission.

In Rust:

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

## Running an existing program

### ib_async

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

### TWS API (`EClient` / `EWrapper`)

```python
import threading
from ibx import EWrapper, EClient, Contract

class App(EWrapper):
    def __init__(self):
        super().__init__()
        self.ready = threading.Event()

    def next_valid_id(self, order_id):
        self.next_id = order_id
        self.ready.set()

    def tick_price(self, req_id, tick_type, price, attrib):
        print(f"tick {tick_type}: {price}")

app = App()
client = EClient(app)
client.connect(username="your_user", password="your_pass", paper=True)
threading.Thread(target=client.run, daemon=True).start()
app.ready.wait(timeout=10)

aapl = Contract(symbol="AAPL", secType="STK", exchange="SMART", currency="USD")
client.req_mkt_data(1, aapl, "", False)
```

Both naming conventions resolve on every type and method: `reqMktData` and
`req_mkt_data`, `secType` and `sec_type`, `conId` and `con_id`. Both surfaces
drive one client and one engine — `ibx.IB` is a facade over `EClient`, they
share a session, and either may be used.

## Configuration

The gateway's configuration file is replaced by settings on the client:
announced build, time zone, message pacing, execution-report scope, and others
— 17 in total, readable at runtime. Seven gateway settings have no counterpart
and report why (no window geometry, no local listening socket, no JVM heap).

Rust: `EClientConfig.gateway`. Python: `ibx.configure()`.

## Documentation

* [The book](https://userfrm.github.io/ibx/) — guides, recipes and the generated API reference
* [Capabilities](docs/capabilities.md) — what is supported, and what each claim rests on
* [Engineering notes](docs/engineering-notes.md) — architecture, performance, protocol coverage
* [Notebooks](notebooks/) — the seven ib_async subjects, in the TWS API shape and in [ib_async's own](notebooks/ib_async_nogateway/)
* [Examples](examples/) — runnable single-file programs in Rust and Python

## License

[AGPL-3.0](LICENSE)

## Disclaimer

Interactive Brokers®, IBKR®, Trader Workstation®, and IB Gateway® are
registered trademarks of Interactive Brokers Group, Inc. This project is **not
affiliated with, endorsed by, or supported by Interactive Brokers**.

IBX is an independent, open-source project provided "as is", without warranty
of any kind.

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
