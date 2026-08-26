<div class="ibx-hero">

<img class="ibx-logo" src="./banner.png" alt="IBX" />

# IBX

<p class="ibx-tagline">An Interactive Brokers client that speaks the venue's protocol itself. Nothing to install alongside it, no process to keep alive. One engine, reachable from Rust and from Python.</p>

<p class="ibx-cta">
  <a class="primary" href="./getting-started.html">Get started</a>
  <a class="secondary" href="./recipes/python/login.html">Recipes</a>
  <a class="secondary" href="./reference/limits.html">What it does not do</a>
  <a class="secondary" href="https://github.com/userFRM/ibx">GitHub</a>
</p>

</div>

## What it replaces

A program written against the TWS API talks to IB Gateway or Trader Workstation
over a socket on localhost, and that process talks to the venue. IBX is the
second half of that arrangement. It logs in, holds the trading, market-data,
historical and security-definition connections open, and gives your program the
same calls and the same callbacks.

Migrating an existing program is the connect call:

```diff
- ib.connect("127.0.0.1", 4001, clientId=1)     # needs a gateway running
+ ib.connect(username="...", password="...")    # no external process
```

`host` and `port` are still accepted and are ignored. There is no local process
to point them at.

What goes with the gateway:

* **The process.** Nothing to install, launch, log into on a schedule, or restart.
* **The JVM.** No heap to size.
* **The localhost socket.** Ticks are delivered in-process.
* **The window.** It runs headless, in a container, over ssh.

<div class="ibx-features">

<div class="ibx-feature">

### The shape your program already has

`EClient` / `EWrapper`, with the same method names and the same callbacks. In
Python there is also `ibx.IB`, shaped like the widely used asynchronous wrapper,
and `ibx.ib_async.attach`, which points an unmodified
[ib_async](https://github.com/ib-api-reloaded/ib_async) program at this engine.

</div>

<div class="ibx-feature">

### One engine, two languages

The Python module is the same Rust core through [PyO3](https://pyo3.rs). Not a
second implementation, so there is no parity to drift. A gate on every commit
fails if either surface grows a call or a callback the other does not have.

</div>

<div class="ibx-feature">

### Nothing between you and the venue

The engine runs on a thread of its own and can be pinned to a core. Quotes are
published through a seqlock, so the writer never blocks and a reader takes a
whole quote without taking a lock. A blocking call holds the thread that made
it and nothing else.

</div>

<div class="ibx-feature">

### It says what it cannot do

A call that this protocol cannot carry reports why instead of returning as
though it acted. Which ones those are is
[written down](./reference/limits.html), over a coverage matrix regenerated
from the source on every commit.

</div>

</div>

## Where to go next

* **Installing it** — [Getting Started](./getting-started.md): the two install
  routes, the feature flags, credentials, and a program that connects.
* **Real code** — recipes in [Rust](./recipes/rust/login.md) and
  [Python](./recipes/python/login.md), each one a runnable file from
  `examples/`, included rather than copied.
* **Looking a call up** — [Rust API](./api/rust.md) · [Python API](./api/python.md).
* **Before you depend on it** — [Limits](./reference/limits.md), then
  [Endpoint Coverage](./reference/coverage.md) for the call-by-call matrix.
* **What a gateway never forwards** — [beyond the API](./reference/beyond-the-api.md):
  the account's grants, the order types it will take, its algorithms, and the
  round-trip time to the venue.

## Status

Under active development. Every capability claim in this repository is
assigned from a named artifact — a test, a script, or a
recorded server response — never from reading the code. The matrix is in
[capabilities.md](https://github.com/userFRM/ibx/blob/main/docs/capabilities.md),
and the counts in it are recomputed on every commit; the build fails if one
moves.

There is no published package yet. Both install routes build from the
repository. See [Getting Started](./getting-started.md).
