# Getting Started

## Install

### Rust

```toml
[dependencies]
ibx = { git = "https://github.com/userFRM/ibx" }
```

### Python

```bash
pip install git+https://github.com/userFRM/ibx
```

> Python wheels are built from the same Rust core via [PyO3](https://pyo3.rs) / [maturin](https://www.maturin.rs/). You need a working Rust toolchain to install from source.

## Credentials

IBX connects directly to IB servers — there is no separate gateway process. Set your account credentials via environment variables:

```bash
export IB_USERNAME="your_username"
export IB_PASSWORD="your_password"
export IB_HOST="cdc1.ibllc.com"   # paper-trading host
```

> **Never** use a live account for testing. Use the paper account for unit and integration tests. The live account is only for read-only validation (login, contract details).

## Hello, world

### Rust

The example this page shows is the one in the repository, included here rather
than copied, so a page that compiles is one the code still compiles too.

```rust
{{#include ../../../examples/hello_tick_data.rs}}
```

Run it with `cargo run --example hello_tick_data`.

### Python

```python
{{#include ../../../examples/hello_tick_data.py}}
```

Run it with `python examples/hello_tick_data.py`.

## Next steps

- [Login (Rust)](./recipes/rust/login.md) · [Login (Python)](./recipes/python/login.md) — minimal connect / `next_valid_id` / disconnect
- [Streaming L2 Market Depth (Rust)](./recipes/rust/streaming-l2.md) — full L2 order book for two tickers
- [Order Lifecycle (Python)](./recipes/python/order-lifecycle.md) — place / modify / cancel / fill on a single order
