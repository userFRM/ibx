# Compatibility status

Capability status for the Rust client, the Python bindings, and the engine
underneath both. Status is assigned from a named artifact — a test, a script,
or a recorded server response — not from code inspection.

| Status | Definition |
| :---: | --- |
| ✅ Supported | Implemented; exercised against IBKR production servers; the response is parsed and delivered to the caller |
| 🔬 Implemented | Implemented and unit-tested; the request reaches the server, but the response has not been observed end to end |
| ⛔ Unavailable | Not carried by the protocol; the call returns an error stating the reason |

Verification runs against a paper account on IBKR production servers. Where a
market or data set is not entitled to that account, the entitlement response is
recorded as the result.

---

## Client surfaces

| Surface | Status | Verification |
| --- | :---: | --- |
| `EClient` / `EWrapper` (TWS API shape) | ✅ Supported | `tests/ib_paper_compat`, `tests/python/test_compat_tier1..3.py` |
| `ib_async`, unmodified | ✅ Supported | Their `IB` on this engine via `ibx.ib_async.attach`; their events, async variants and types, with no gateway. Their own test suite runs against it: 2 of 3 pass, and `test_request_error_raised` cannot pass against any server, because 321 is in their `warningCodes` and a warning never ends the request it belongs to. `tests/python/test_ib_async_transport.py`, `tests/ib_async_upstream/conftest.py` |
| `ibx.IB` (ib_async shape) | ✅ Supported | 90/90 methods present; `tests/python/test_ib_facade.py`, `scripts/sdk_sweep.py` |
| `ibx::api::Client` (Rust) | ✅ Supported | 77/77 callable; 3 return an error naming a local-process facility this client does not have |
| Gateway settings | ✅ Supported | 17 settings carried, 7 recorded as having no counterpart; `tests/python/test_gateway_settings.py`, `tests/python/test_settings_parity.py`; session opened under a stated build and time zone |
| Rust/Python equivalence | ✅ Supported | 4 static gates (settings, order fields, surface, error behaviour) plus `scripts/conformance.py --compare`, which compares 10 server responses across both clients |

## Market data

| Capability | Status | Verification |
| --- | :---: | --- |
| Top of book | ✅ Supported | Streaming and snapshot; US equities and FX; `scripts/sdk_sweep.py`, `tests/python/test_live_quotes.py`. Concurrent subscribers on one contract share one wire subscription |
| Market depth (L2) | 🔬 Implemented | Gathered from the venues the server publishes — 203 of them, worldwide — filtered to the security type and the aggregation group of the exchange the contract is listed on. A London book returns EUIBSI, BATEUK, AQXEUK and TRQXUK; a US book returns EDGX, ARCA and NASDAQ. Each level names its venue. The account holds no deep-book entitlement on the US equity venues, which answer the request by refusing it, by name, to the caller; `tests/python/test_live_depth.py`, `tests/python/test_live_session_features.py::test_l2_smart_depth`. A book on one named venue is asked for as a book: a future returns the entitlement response for the account, where the request previously went unanswered |
| Historical bars | ✅ Supported | 9 markets in one session (`src/bin/capture_global.rs`); `keepUpToDate` verified in `tests/python/test_issue_100.py` |
| Historical ticks and schedules | ✅ Supported | `scripts/sdk_sweep.py`; unsupported tick types return an error rather than substituting another series |
| Tick-by-tick quotes | ✅ Supported | FX and US equities, concurrent streams, each record carrying its request id; `tests/python/test_live_python_wrappers.py` |
| Tick-by-tick trades | ✅ Supported | 1,027 trades on one session; `Last` and `AllLast` are distinct streams |
| Trading halt status | ✅ Supported | Tick 437 decoded from status mask and status index; `src/bin/capture_status.rs` |
| Tick attributes | ✅ Supported | Per-trade `unreported` and `pastLimit` observed to vary within one stream |

## Orders

| Capability | Status | Verification |
| --- | :---: | --- |
| 23 order types | ✅ Supported | `whatIf` preview accepted by the server for each; `tests/ib_paper_compat` |
| Order fields | ✅ Supported | 154 fields: 125 transmitted, 29 documented as not carried by the protocol, 0 silently dropped; `docs/order-field-reach.md`, regenerated and compared in CI |
| Non-US markets | ✅ Supported | Previews accepted on DE, NL, GB, CH, AU, CA, US equities and FX; JP and HK rejected for lot size, which is the exchange rule and is surfaced to the caller |
| Modify, cancel, global cancel | ✅ Supported | `scripts/sdk_lifecycle.py` (place → modify → cancel), `tests/ib_paper_compat` Phase 9 / 9b |
| Brackets, OCA, combos | ✅ Supported | Per-leg pricing; leg order validated by server rejection of the inverted spread; `src/bin/capture_combo.rs` |
| Conditions | ✅ Supported | All 6 types (price, volume, percent change, margin, execution, time) accepted and held by the server; `tests/ib_paper_compat` Phase 60 |
| Executions and fills | ✅ Supported | Fill reported and position reconciled; execution report retains server fields including unnamed tags; `tests/ib_paper_compat` Phase 97 |
| Option exercise and lapse | ✅ Supported | Both submitted for a resolved option contract; server response 399 *"You have not got the number of options requested to be exercised"* delivered to the caller |

## Account

| Capability | Status | Verification |
| --- | :---: | --- |
| Account values and summary | ✅ Supported | 135 values, each in the currency the server states; subscribed at connect; `tests/python/test_issue_98.py` |
| Positions and P&L | ✅ Supported | Delivered on login and after each fill; `tests/python/test_issue_98.py` |
| Managed accounts | ✅ Supported | `tests/python/test_issue_98.py` |
| Financial advisor configuration | 🔬 Implemented | Request reaches the server on both clients; the response requires an advisor account |

## Reference data

| Capability | Status | Verification |
| --- | :---: | --- |
| Contract details | ✅ Supported | 11 of 12 resolved across 9 countries; the 12th matched 2 contracts and returned an ambiguity error rather than a selection; `src/bin/capture_global.rs` |
| Option chains, symbol search | ✅ Supported | `scripts/sdk_sweep.py` |
| Scanners, fundamentals | ✅ Supported | 697 KB scanner parameter set, fundamental report; `tests/python/test_issue_100.py` |
| News | ✅ Supported | 117 providers parsed. Headline retrieval requires a news subscription; this account holds none, and every provider returns an empty result set |
| Corporate events calendar | ✅ Supported | Event types (179 KB) and event queries answered over the security-definition connection; `src/bin/capture_calendar.rs` |
| Implied volatility, option price | ✅ Supported | Computed in-process; the protocol carries no request for either. Anchored to the server's published model per contract; reproduces the server price to the cent on 2 contracts; `src/bin/capture_option_model.rs` |

---

## Invariants

Three properties are enforced by generators that run in CI and fail the build
on drift.

| Invariant | Measure | Generator |
| --- | --- | --- |
| No request returns as though it acted when it did not | 76 caller-facing requests, 0 silent | `scripts/gen_wire_reach.py` |
| No caller-set order field is discarded without notice | 154 fields, 0 dropped | `scripts/gen_order_field_reach.py` |
| No server field is discarded | Unnamed fields retained under their tag number (49 on an equity definition, 46 on a bond) | `scripts/gen_wire_coverage.py` |

A fourth is enforced by a test rather than a generator: **no wire parser aborts
on malformed input.** Every parser is given each prefix of a well-formed frame,
that frame with a byte replaced at each position, and runs that are not frames
at all (`tests/malformed_input.rs`).

## Protocol constraints

These are properties of the IBKR protocol, not of this implementation. The
official gateway behaves the same way.

- **29 order fields are not transmitted.** For each, the reference client
  either declares no tag or declares one the server rejects by name
  (`algo_id` → *Invalid value in field # 8016*; `scale_init_fill_qty` →
  *Can not contain field # 6486*). Each field retains the caller's value so an
  order constructed against another client round-trips unchanged.
- **One market-data subscription per contract on the wire.** Multiple callers
  are multiplexed client-side, as the gateway multiplexes across windows.
- **`keepUpToDate` queries are closed on first response.** Continuation is
  provided by folding the 5-second bar stream into the requested bar size.
- **Historical execution reports require a window within 7 days.** A request
  without one is rejected in full.

## Architectural differences from a gateway process

| Gateway | This client |
| --- | --- |
| Configuration file and settings window | Settings on the client, read back at runtime. 7 gateway settings have no counterpart: no window geometry, no local listening socket, no JVM heap |
| Local socket for client programs | The client is in-process; there is no socket to connect to, authorise, or keep running |
| Java runtime | None |

## Unresolved

- The option-exercise interest rate series (`OptExInterestRate`) is accepted as
  a tick query against an option contract and rejected by name against the
  underlying. Every window tested returns an empty result set. Until it
  resolves, the model's carry term is fitted rather than read, and absorbs
  model differences: two contracts on one underlying and expiry fitted 4.9% and
  20.1%.
- A crypto's book and its tick-by-tick stream are acknowledged and produce
  nothing, with no response of any kind. The client this replaces sends the
  same request for a crypto as for a share: the only field that differs is the
  security type, and the same request against a future is answered with the
  entitlement, so the shape reaches the server and is understood. The silence
  is the venue's, not the request's.
- Crypto tick-by-tick subscriptions are acknowledged with both increments and
  produce no records. In the same session and on the same contract, top of book
  streams continuously and a historical tick request is answered; equities and
  FX stream throughout. The server holds crypto tick data and does not stream
  it.
- Tick 437 carries four 32-bit integers; the unit of the second (timestamp) and
  the meaning of the fourth are not established. Neither is used.

## Test inventory

| Suite | Count | Requires credentials |
| --- | ---: | :---: |
| Rust unit and integration | 1,367 | No |
| Python | 374 | No |
| Python, live | 489 | Yes |
| Paper compatibility suite (137 phases) | 17 tests | Yes |

Run order and environment are documented in
[docs/engineering-notes.md](docs/engineering-notes.md).

## Refusals

A request this client will not send is reported through `error(reqId, code,
message)` and the call returns, as the reference client does. The number is the
one that client reports for the same class: 321 for a request that fails
validation, 200 for a contract description that matches nothing, 504 for a call
with no session. Construction and configuration raise, as does a synchronous
call with a return value.

## Planned work

[docs/api-alignment-plan.md](docs/api-alignment-plan.md) lists two open items:
settings decided by configuration where the server states a grant for them, and
whether the surfaces over this engine are three products or one product and two
adapters.

## Release criteria

Live sessions are run at market hours until a session produces no new defect.
Every session to date has produced at least one that the offline suites did not
detect, including a crash on a live trade stream, a subscription delivering the
wrong tick type, and a regression affecting every synchronous call.

The most recent session produced eleven, listed here as the caller-visible
symptom:

| Symptom | Cause |
| --- | --- |
| `reqHistoricalData` returned no bars for an unqualified contract | Request sent with contract id 0; no lookup performed |
| `accountSummary()` returned an empty list | Account not subscribed until an explicit request |
| Account values all zero for a non-USD account | Values sourced from a local copy rather than server statements |
| Session opened with no execution history | Server rejects the request without a time window |
| `reqNewsProviders()` returned one provider containing all others | Field separators inverted |
| Rust `option_chain()` empty while Python returned 3 | Dispatch drain withheld results from its own synchronous call |
| `reqTickByTickData(contract, "Last")` returned the other stream, then nothing | Two distinct streams conflated |
| `reqTickers()` returned previous close, no bid/ask | Snapshot cancelled on first tick of any type |
| Contract with no bid quoted at −1 | Server's absent-value sentinel treated as a price |
| `reqHistoricalSchedule()` returned `None` | No return path implemented |
| Order modify left the order inactive; cancel found nothing | Replace omitted the destination field; server rejected as a mismatch |
