# Aligning the client surface with the reference client

Four items. The first is done. Two are defects with known fixes and no decision
attached. The last needs a decision before any code is written.

---

## 1. A refusal is delivered, not thrown — done

Every request-shaped method on the Python client reports a refusal through
`error(reqId, code, message)` and returns. The number is the one the reference
client reports for the same class: 321 for a request that fails validation, 200
for a contract description that matches nothing, 504 for a call with no
session. The connection is checked first, as it is there.

`src/api/error_codes.rs` holds the type and the numbers. The Rust client
returns it, so both surfaces refuse with the same number and the same text.

Construction and configuration still raise: a read-only client asked to trade
is a programming error, not a request outcome. So do the synchronous answering
calls — `contract_details`, `qualify_contract` — which have a return value and
no shape in the reference API to match.

## 2. Capability is negotiated, not configured

The server states 299 granted features at logon. They are captured
(`src/gateway.rs`, tag 6542) and readable on both clients (`enabled_features`).
Nothing gates on them.

`island_for_nasdaq` is the one setting with a matching grant. The counterpart
reads `ISLAND2NASDAQ` off the granted list at logon and holds it in a field of
its own, beside `NOAMOPTCHK` and `FORCENOCBN`, which is what a setting alone
does not decide. Here it is decided by the setting alone.

`server_version()` (`src/api/direct.rs:123`) returns the announced build, so a
program gating a feature on it finds everything available. Deliberate, and a
divergence a caller should be able to read about where they will look for it.

**Fix.** Gate `island_for_nasdaq` on the grant. Check the remaining settings
against the grant list. Note the `server_version()` divergence in its own
documentation.

**Gate.** A test that a setting whose grant is absent does not take effect.

## 3. Nothing about a market is written here

Which venues exist, what they offer, and what they are called is the server's
to state. A list written into this source is a list of the markets this client
cannot reach, and it is wrong the day a venue is added.

Two are done. The venues a book is gathered from came from eighteen names
written here and now come from the two hundred and three the server publishes,
which is what made a London book reachable. The map from a quote's exchange
mask to a venue came from a list of seventeen whose order was this client's
own, and now comes from the contract's own definition.

The third is the table of eight venue letters that used to sit in
`src/types.rs`. It is gone. The counterpart carries no such table either: it
reads the map off the wire, as `NAME/LETTER` per venue, in the order the
exchange mask's bits refer to, and the tick that carries it is asked for
alongside the quote. This client asks for it now. What is not yet settled is
that the server has not answered it in the sessions tried, so a venue's letter
is empty until it does — which is what the server has said, rather than what
this client would have guessed.

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

Order: 2, then 3. Item 4 before either, if the answer is "three products".
