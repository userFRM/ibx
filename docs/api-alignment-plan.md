# Aligning the client surface with the reference client

A plan for bringing this client's conventions in line with the client it
replaces. Scope is the caller-facing surface — how a request is refused, how a
capability is decided, what is a product and what is an adapter. The wire
protocol and the engine are not in scope; they already follow the reference
client's behaviour and are measured for it.

Each workstream states what is true now, with the evidence, what it should be,
and the gate that keeps it there. A workstream without a gate is not finished:
the two measures this repository already enforces — request reach and order
field reach — exist because a rule with nothing checking it decays.

---

## The conventions being aligned to

Four properties define how the reference client behaves. They are observable in
its API, not inferred.

1. **One API model, several bindings.** `EClient` for requests, `EWrapper` for
   responses, with the same names in every language it ships in. No convenience
   layer is shipped alongside it.
2. **A refusal is data, not control flow.** No method of the request API throws.
   Every problem — a malformed request, a missing entitlement, an unknown
   contract — arrives at `error(reqId, code, message)` under a number. This is
   why programs written against it have an error handler and no exception
   handling around the calls.
3. **Capability is negotiated, not configured.** What a session can do is
   settled by what the server states it supports and what it grants the
   account, not by what the operator declares.
4. **The wire is additive.** Fields are appended and gated; nothing is removed
   or reordered.

---

## Workstream 1 — A refusal is delivered, not thrown

**Priority: first.** It is the largest remaining incompatibility for a ported
program, it is mechanical, and it is countable.

### Now

39 refusals in the Python client raise `PyRuntimeError` or `PyValueError`.
Exactly one path follows the reference convention: a call made without a
connection reports `error(reqId, 504, "Not connected")`
(`src/python/compat/client/mod.rs:459`).

The rest raise, including on the request-shaped methods where the reference
client returns `None` and reports:

| File | Raises | Of which on a request-shaped method |
| --- | ---: | ---: |
| `src/python/compat/client/orders.rs` | 17 | 16 |
| `src/python/compat/client/mod.rs` | 10 | 2 |
| `src/python/compat/client/ask.rs` | 5 | 1 |
| `src/python/compat/client/market_data.rs` | 3 | 3 |
| `src/python/compat/client/stubs.rs` | 2 | 2 |
| `src/python/compat/client/reference.rs` | 1 | 1 |
| `src/python/compat/contract.rs` | 1 | 0 |

A program moved from the reference client has an `error()` handler and no
exception handling around `place_order`, because nothing it was written against
throws there. Those 25 refusals reach neither.

### Target

A request-shaped method — `req_*`, `place_order`, `cancel_*`, `exercise_options`
— never raises. It reports `error(req_id, code, message)` and returns.

Three cases keep raising, and each is correct:

- **Answering calls** (`contract_details`, `historical_data`, `qualify_contract`,
  `option_chains`, …). These are not part of the reference API. They are
  functions with a return value, and a Python function that cannot answer
  raises. `ask.rs:391` — a contract description matching several contracts — is
  the model.
- **Construction and connection.** `connect()` on an open session, a failed
  logon, a constructor given a field that does not exist. There is no request
  id to report against, and the reference client raises here too.
- **Programming errors.** A request id outside the range the wire carries.

### Steps

1. An error table: one Rust enum carrying the reference client's own numbers,
   with the message text beside each. Every refusal in the crate resolves to a
   member of it. Blast radius: one new module, `src/api/error_codes.rs`.
2. `ClientCore` validation functions return that type rather than `String`.
   Blast radius: `src/client_core.rs` (~20 functions), and their callers on
   both clients.
3. The Python request-shaped methods report and return. Blast radius:
   `orders.rs` (16), `market_data.rs` (3), `stubs.rs` (2), `reference.rs` (1),
   `mod.rs` (2).
4. The Rust client keeps returning `Result` — it is a Rust API and a Rust caller
   expects one — but the error carries the same code, so the two clients refuse
   the same request with the same number. Blast radius: `src/api/client/*.rs`
   return types, 41 methods.

### Gate

`scripts/gen_refusal_reach.py`, run in CI beside the other two measures: every
request-shaped method on the Python client, classified as *reports* or *raises*,
with *raises* required to be zero. The existing `test_behaviour_parity.py`
extends to compare the code each client uses for the same refusal.

---

## Workstream 2 — One product surface, adapters named as adapters

**Priority: second.** It is a decision, and it gates whether workstreams 1 and 3
apply to one surface or three.

### Now

Three caller-facing surfaces exist or are proposed over one engine:

| Surface | State | Gated by |
| --- | --- | --- |
| `EClient` / `EWrapper` (both languages) | Shipped | 4 parity gates, the compatibility suite |
| `ibx.IB` (ib_async shape) | Shipped | `test_ib_facade.py`, 90/90 method presence |
| rust-ibapi shape | Proposed | — |

`ibx.IB` is gated like a product but documented like a convenience. It also
carries the method names of a library whose programs are asyncio-native: a
caller using `ib.pendingTickersEvent += handler` or `await ib.reqTickersAsync(…)`
fails, because neither the event objects nor the async variants exist here.

### Target

`EClient` / `EWrapper` is the product: versioned, documented, gated, and the
surface every other workstream applies to.

Anything shaped like a third-party library is an **adapter**: shipped for
migration, documented as best-effort, and explicitly not covered by the
compatibility guarantees. An adapter states what it does not carry — for
`ibx.IB` today, the event system and the async variants.

### Steps

1. Say so, in `README.md` and in the module documentation of each surface.
2. Split the parity gates by surface, so a failure names which surface is out
   of line.
3. For `ibx.IB`: either implement the event surface and the async variants, or
   state their absence in the adapter's own documentation. The second is
   defensible; silence is not.
4. Decide on the rust-ibapi shape before writing it, under the same rule.

---

## Workstream 3 — Capability is negotiated

**Priority: third.** It is a correctness matter, and it is small.

### Now

The server states what this account is granted at logon: 299 named features,
captured (`src/gateway.rs`, tag 6542) and readable
(`SharedState::enabled_features`).

Nothing gates on them. The one setting with a corresponding grant —
`island_for_nasdaq`, whose grant is `ISLAND2NASDAQ` — is decided by the
operator's setting alone. The reference client requires both: the grant *and*
the setting.

`server_version()` (`src/api/direct.rs:123`) returns the announced build. A
program that gates a feature on it finds every feature available. That is
deliberate and documented, and it is a divergence: in the reference client that
value is how a program decides what it may send.

### Target

A setting that has a corresponding grant is in force only when both agree. A
program can read the grants. The divergence in `server_version()` is either
closed or documented as an API note where a caller will find it.

### Steps

1. `island_for_nasdaq` consults `ISLAND2NASDAQ` as well as the setting.
2. Enumerate the remaining settings against the grant list; where a grant
   exists, gate on it.
3. Expose the grants on both clients as a first-class call.

### Gate

A test that a setting whose grant is absent does not take effect, and a
generator listing each setting against the grant it depends on, so a new
setting cannot be added without stating which grant it needs, or that it needs
none.

---

## Workstream 4 — A code, not a sentence

Falls out of workstream 1 and is finished with it.

Every refusal carries a number a program can branch on. The error table is the
reference for it, generated into the API documentation, so a caller can look up
what a code means without reading the source.

---

## Workstream 5 — Additive change only

A policy, adopted rather than migrated to.

Nothing already delivered to a caller changes shape. Fields are added; the
meaning of an existing one does not move. Where the server changes what it
sends, the change is absorbed in the engine and the caller-facing shape is
extended, never redefined.

The measures that hold this are already in place: request reach, order-field
reach, and the compatibility suite. The addition is a rule for the changelog —
a caller-visible shape change is a breaking change and is named as one.

---

## Sequence

| Order | Workstream | Why here |
| ---: | --- | --- |
| 1 | Refusals are delivered | Largest incompatibility, mechanical, measurable |
| 2 | One product surface | Decides the scope of everything else |
| 3 | Capability negotiation | Correctness; small once the surface is settled |
| 4 | Error codes documented | Finishes workstream 1 |
| 5 | Additive-change policy | Continuous |

## Not in scope

- The wire protocol and the engine. They follow the reference client's
  behaviour and are measured against it.
- The synchronous answering calls. They are not the reference API's shape and
  are not being made to look like it; they raise, which is correct for what
  they are.
- Naming. This client's method names already resolve under both conventions.
