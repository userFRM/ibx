# Aligning the client surface with the reference client

Three items. Two are defects with known fixes and no decision attached. The
third needs a decision before any code is written.

---

## 1. A refusal is delivered, not thrown

39 refusals in the Python client raise. **25 of them are on request-shaped
methods** — `place_order`, `req_*`, `cancel_*`, `exercise_options` — where the
reference client returns and reports `error(reqId, code, message)`. One path
does it correctly: a call with no connection reports 504
(`src/python/compat/client/mod.rs:459`).

A program moved from the reference client has an `error()` handler and no
exception handling around `place_order`, because nothing it was written against
throws there. Those 25 refusals reach neither.

| File | Raises | On request-shaped methods |
| --- | ---: | ---: |
| `client/orders.rs` | 17 | 16 |
| `client/mod.rs` | 10 | 2 |
| `client/ask.rs` | 5 | 1 |
| `client/market_data.rs` | 3 | 3 |
| `client/stubs.rs` | 2 | 2 |
| `client/reference.rs` | 1 | 1 |
| `contract.rs` | 1 | 0 |

**Fix.** One enum in `src/api/error_codes.rs` carrying the reference client's
numbers and message text. The shared validators in `src/client_core.rs` return
it. The Python request-shaped methods report and return; the Rust client keeps
`Result<(), String>` and renders the same error through `Display`, so both
refuse with the same number without touching 41 signatures.

**Keeps raising, correctly:** the synchronous answering calls
(`contract_details`, `qualify_contract` — functions with a return value, not the
reference API's shape); connect and construction (no request id to report
against); a request id outside the range the wire carries.

**Gate.** `scripts/gen_refusal_reach.py` in CI: every request-shaped method
classified *reports* or *raises*, raises required to be zero. Codes are
generated into the API reference from the same enum.

---

## 2. Capability is negotiated, not configured

The server states 299 granted features at logon. They are captured
(`src/gateway.rs`, tag 6542) and readable (`SharedState::enabled_features`).
Nothing gates on them.

`island_for_nasdaq` is the one setting with a matching grant
(`ISLAND2NASDAQ`), and it is decided by the setting alone. The reference client
requires the grant *and* the setting.

`server_version()` (`src/api/direct.rs:123`) returns the announced build, so a
program gating a feature on it finds everything available. Deliberate, and a
divergence a caller should be able to read about where they will look for it.

**Fix.** Gate `island_for_nasdaq` on the grant. Check the remaining settings
against the grant list. Expose the grants as a call on both clients. Note the
`server_version()` divergence in its own documentation.

**Gate.** A test that a setting whose grant is absent does not take effect.

---

## 3. Nothing about a market is written here

Which venues exist, what they offer, and what they are called is the server's
to state. A list written into this source is a list of the markets this client
cannot reach, and it is wrong the day a venue is added.

Two are done. The venues a book is gathered from came from eighteen names
written here and now come from the two hundred and three the server publishes,
which is what made a London book reachable. The map from a quote's exchange
mask to a venue came from a list of seventeen whose order was this client's
own, and now comes from the contract's own definition.

One is left. `exchange_letter` in `src/types.rs` maps eight venues to the
single character the reference client reports, and returns nothing for every
other venue — including most US ones and all others. The server states these,
in its answer to a request for the components of a smart quote; this client
answers that request from its own table instead of sending it. Locating that
request settles it, and deletes the table.

## 4. Product surface, or adapter — needs a decision

Three caller-facing surfaces over one engine:

| Surface | State |
| --- | --- |
| `EClient` / `EWrapper` | Shipped, gated by 4 parity checks and the compatibility suite |
| `ibx.IB` (ib_async shape) | Shipped, gated on 90/90 method presence |
| rust-ibapi shape | Proposed |

`ibx.IB` is gated like a product and documented like a convenience. It carries
the method names of an asyncio-native library while having neither its event
objects nor its async variants, so `ib.pendingTickersEvent += handler` and
`await ib.reqTickersAsync(...)` fail.

The reference client ships one API surface and no convenience layer.

**The decision.** Either each surface is a product — parity-gated, versioned,
documented equally, and for `ibx.IB` that means building the event system and
the async variants — or `EClient`/`EWrapper` is the product and the rest are
adapters: shipped for migration, best-effort, and stating what they do not
carry. This decides whether items 1 and 2 apply to one surface or three, and
whether the rust-ibapi shape gets written at all.

---

Order: 1, then 2, then 3. Item 4 before any of them, if the answer is "three products".
