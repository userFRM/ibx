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

## 2. Capability is negotiated, not configured — done for the one grant that has a setting

The server states its granted features at logon. They are captured
(`src/gateway.rs`, tag 6542) and readable on both clients (`enabled_features`).

`island_for_nasdaq` is the one setting with a matching grant. The counterpart
reads `ISLAND2NASDAQ` off the granted list at logon and holds it in a field of
its own, beside `NOAMOPTCHK` and `FORCENOCBN`. It now takes both here too:
`SharedState::island_for_nasdaq()` is the setting and the grant, settled at
logon rather than scanned for on the path that parses a contract definition.

`server_version()` (`src/api/direct.rs:123`) returns the announced build, so a
program gating a feature on it finds everything available. Deliberate, and a
divergence a caller should be able to read about where they will look for it.

**Left.** The remaining settings against the grant list — none has a matching
token so far. The `server_version()` divergence in its own documentation.

## 3. Nothing about a market is written here — done

Which venues exist, what they offer, and what they are called is the server's
to state. A list written into this source is a list of the markets this client
cannot reach, and it is wrong the day a venue is added.

Three were removed. The map from a quote's exchange mask to a venue came from a
list of seventeen whose order was this client's own; it now comes from the tick
that carries it, `NAME/LETTER` per venue, in the order the mask's bits refer
to. The server answers it at regular trading hours with 18 venues, and states
none outside them, so a venue's letter is empty until it does — which is what
the server has said rather than what this client would have guessed.

The table of eight venue letters in `src/types.rs` is gone.

The venues an aggregate book is gathered from came from eighteen names written
here, then from a global exchange directory read as though its sections were
aggregation groups, and now from the contract's own definition, which names the
venues SMART routes it to.

## 4. Product surface, or adapter — decided: one product, two adapters

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

**Decided: one product, two adapters.** `EClient`/`EWrapper` is the product.
The ib_async shape and the Rust shape are adapters over it — shipped for
migration, thin enough that a defect in either is a defect in the adapter, and
stating what they do not carry. Anything reachable that the reference client
does not have belongs on the engine and is documented as not portable.

What settled it: every defect found against one surface was a defect in the
engine underneath both, and the seam between them was charging rent. The
contract types now convert, so the lookup cache is written once and both
surfaces key it the same way. A surface-shaped fix would have been written
twice.

The rust-ibapi shape is not written. A third adapter earns nothing that the
two do not already carry.

---

Order: 2, then 3.
