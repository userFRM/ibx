<div class="ibx-hero">

<img class="ibx-logo" src="./banner.png" alt="IBX" />

# IBX

<p class="ibx-tagline">Direct IB connection engine. No Java Gateway. No middleman. Built in Rust for ultra-low-latency, available as both a Rust crate and a Python wheel.</p>

<p class="ibx-cta">
  <a class="primary" href="./getting-started.html">Get Started →</a>
  <a class="secondary" href="./recipes/rust/login.html">Read the recipes</a>
  <a class="secondary" href="https://github.com/userFRM/ibx">GitHub</a>
</p>

</div>

<div class="ibx-stats">
  <div class="ibx-stat">
    <div class="ibx-stat-value">340 ns</div>
    <div class="ibx-stat-label">Tick read latency</div>
  </div>
  <div class="ibx-stat">
    <div class="ibx-stat-value">~460 ns</div>
    <div class="ibx-stat-label">Order send latency</div>
  </div>
  <div class="ibx-stat">
    <div class="ibx-stat-value">0</div>
    <div class="ibx-stat-label">Local processes to keep alive</div>
  </div>
  <div class="ibx-stat">
    <div class="ibx-stat-value">2</div>
    <div class="ibx-stat-label">Languages, one engine</div>
  </div>
</div>

## Why IBX

<div class="ibx-features">

<div class="ibx-feature">

### No JVM, no gateway

Connect straight to the IB servers. No localhost hop, no garbage-collector pauses, no separate process to babysit.

</div>

<div class="ibx-feature">

### Same API you already know

Drop-in compatible `EClient` / `Wrapper` callback shape. Port existing strategies without rewriting them.

</div>

<div class="ibx-feature">

### Two languages, one core

The Python wheel is built from the same Rust engine via PyO3 — no second implementation, no parity drift.

</div>

<div class="ibx-feature">

### Built for latency

Hot loop pinned to a core, zero-allocation tick parsing, lock-free dispatch. Tick reads in nanoseconds, not milliseconds.

</div>

</div>

## Where to go next

- **New here?** → [Getting Started](./getting-started.html) — install, credentials, hello-world in both languages.
- **Want to see real code?** → Recipes for [Rust](./recipes/rust/login.html) and [Python](./recipes/python/login.html) — end-to-end flows with full source from `examples/`.
- **Looking up a specific call?** → [Rust API](./api/rust.html) · [Python API](./api/python.html).
- **Wondering what's wired up?** → [Endpoint Coverage](./reference/coverage.html).

## Project status

IBX is under active development. The Rust and Python APIs track the official IB API surface; gaps are tracked in the [Endpoint Coverage](./reference/coverage.html) chapter.
