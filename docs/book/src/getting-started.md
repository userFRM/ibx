# Getting Started

## What you need

* An Interactive Brokers account, paper or live. No IB software, and not the
  `ibapi` package either.
* Rust 1.89 or newer. Both install routes compile the engine.
* Python 3.11 or newer, for the Python surface.

There is no package on crates.io and none on PyPI. Everything below installs
from the repository.

## Install

### Rust

```toml
[dependencies]
ibx = { git = "https://github.com/userFRM/ibx" }
```

### Python

```bash
pip install "git+https://github.com/userFRM/ibx"
```

The build backend is [maturin](https://www.maturin.rs/), which compiles the
Rust core and produces the extension module. `pyproject.toml` already names the
features it needs, so there is nothing to pass.

Working on the client itself, build it in place instead:

```bash
uv venv .venv --python 3.13
source .venv/bin/activate          # .venv\Scripts\activate on Windows
uv pip install maturin
maturin develop
```

`maturin develop` replaces the installed module. A local `pytest` run tests
whatever was built last, so rebuild before running one.

## Feature flags

`default = []`. The Rust client and its blocking calls need nothing turned on.

| Feature | What it adds |
| --- | --- |
| `async` | `AsyncClient`, for driving the client from inside a Tokio runtime. Pulls in Tokio, which a program with its own thread does not need to pay for |
| `python` | The PyO3 bindings |
| `extension-module` | Tells PyO3 not to link libpython. Right for the wheel, wrong for `cargo test`. maturin sets it; you do not |
| `dev-tools` | The binaries under `src/bin` — the benchmarks, and the capture tools this repository is developed with. Most of them read credentials and open a session |

```toml
ibx = { git = "https://github.com/userFRM/ibx", features = ["async"] }
```

`AsyncClient` names the calls a session is usually asked. Everything else the
client can do is reached through `off_reactor`, which runs the call on a thread
that may wait:

```rust,ignore
client.off_reactor(|c| c.req_scanner_parameters()).await??;
```

Reads — positions, fills, orders — are a lock and a copy, so they are taken
inline and need none of this.

The features not in that table exist for this repository's own test suites.
They are off by default and should stay off in anything you install.

## Credentials

The credentials go in the connect call. There is no configuration file and no
process holding a login on your behalf.

```python
ib.connect(username="your_user", password="your_pass", paper=True)
```

```rust
let client = Client::connect(&Config {
    username: "your_user".into(),
    password: "your_pass".into(),
    paper: true,
    ..Default::default()
})?;
```

**Do not name a host.** Left empty, the client knocks on one of the venue's
regional doors and the venue answers by naming the server this account actually
lives on; the session moves there. A host is worth stating only to knock at a
particular region.

**`paper`.** `true` skips the live second-factor approval gate. `false` enters
it on connect, and that call blocks until the factor is approved. An account
whose second factor is an authenticator code has no push to fall back on and
needs a `code_provider`; for an IBKey account, leaving it unset means waiting
for the mobile push.

Use a paper account while you are writing something. A live account is a live
account.

Two places read the environment instead of the call. `ibx.ib_async.attach`
falls back to `IB_USERNAME` and `IB_PASSWORD` when they are not passed, and the
programs under `examples/` read the same two:

```bash
export IB_USERNAME="your_username"
export IB_PASSWORD="your_password"
```

## Hello, world

Both files below are the ones in the repository, included rather than copied,
so this page cannot drift from what actually runs. The Rust one is compiled by
CI along with the rest of `examples/`. Each subscribes to SPY for five seconds
and prints the last quote it saw.

### Rust

```rust
{{#include ../../../examples/hello_tick_data.rs}}
```

```bash
IB_USERNAME=... IB_PASSWORD=... cargo run --example hello_tick_data
```

### Python

```python
{{#include ../../../examples/hello_tick_data.py}}
```

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/hello_tick_data.py
```

That example uses `EClient` / `EWrapper`, which is the shape a TWS API program
already has. Python has a second surface over the same session, `ibx.IB`, shaped
like the widely used asynchronous wrapper — a call sends the question and hands
back the answer, with no callback to register:

```python
import ibx

ib = ibx.IB()
ib.connect(username="your_user", password="your_pass", paper=True)

spy = ibx.Contract(symbol="SPY", secType="STK", exchange="SMART", currency="USD")
(ticker,) = ib.reqTickers(spy)
print(ticker.bid, ticker.ask)

ib.disconnect()
```

The two are one client: `ibx.IB` is a facade over `EClient`, they share a
session, and either may be used. A contract does not have to be qualified
first — a request carrying a contract rather than a contract id is resolved
before it is sent.

## Next steps

* [Login](./recipes/python/login.md) — connect, take the first order id, disconnect
* [Streaming ticks](./recipes/python/tick-data.md) · [L2 depth](./recipes/rust/streaming-l2.md)
* [Order lifecycle](./recipes/python/order-lifecycle.md) — place, modify, cancel, fill
* [An existing ib_async program](./recipes/python/ib_async.md) — one line changed
* [Limits](./reference/limits.md) — read this before you depend on a call
