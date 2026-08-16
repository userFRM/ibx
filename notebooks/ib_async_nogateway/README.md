# ib_async, without a gateway

The notebooks one level up use the TWS API shape: an `EClient`, and a wrapper
of callbacks. These are the same seven subjects written against
[`ib_async`](https://github.com/ib-api-reloaded/ib_async) — its `IB`, its
contracts, its events, its `util.df` — running on this engine instead of on a
gateway.

`ib_async` is not copied or modified here. Install it as usual. It is layered:
everything above `Client`/`Connection` is transport-agnostic, and only that
layer knows there is a socket to a local process. `ibx.ib_async.attach`
replaces that layer, so one line differs from what the library's own notebooks
do:

```python
ib = ibx.ib_async.attach(IB(), username="...", password="...", paper=True)
ib.connect()          # names no host: there is no gateway to name
```

`IB.connect` still takes a host, a port and a client id, because it was written
for a gateway. They are accepted and ignored.

## Running them

```bash
uv venv .venv --python 3.13 && source .venv/bin/activate
pip install maturin ib_async jupyter python-dotenv
maturin develop --release --features python,extension-module
jupyter lab notebooks/ib_async_nogateway
```

Credentials come from `.env` at the repository root, as `IB_USERNAME` and
`IB_PASSWORD`. Every notebook opens a paper session.

## One thing to keep

`ib.sleep()`, never `time.sleep()`. The library runs its event loop on the
calling thread, so a plain sleep stops it: quotes stop arriving, and every
stream reads as dead when it is only unattended.
