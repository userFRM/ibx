# Roadmap

Scope: a Rust client that connects to Interactive Brokers directly, with Python bindings. No Java runtime, no desktop application, no local gateway process.

Status is assigned from evidence. `Verified` requires a passing live session phase. No status is assigned from intent.

## Status definitions

| Status | Definition |
| --- | --- |
| Verified | Implemented, unit tested, and passed against a live session |
| Implemented | Implemented and unit tested. No live session has confirmed it |
| Blocked | Implemented. The venue refuses the request and the cause is stated |
| Accepted, not served | Call exists with the expected signature, returns normally, and reports through the error callback that it cannot be served |
| Absent | No such call |

## Capability matrix

| Capability | Status | Evidence | Target |
| --- | --- | --- | --- |
| Session establishment and login | Verified | Live phase | Shipped |
| Reconnect and subscription rebuild | Verified | Live phase | Shipped |
| Second factor approval | Implemented | Live logins only. Paper logins do not present the factor | 1.0 |
| Order submission, 23 order types | Verified | Live phase | Shipped |
| Order modification | Verified | Live phase | Shipped |
| Order cancellation, single, by permId, global | Verified | Live phase | Shipped |
| Bracket and OCA linkage | Verified | Live phase | Shipped |
| Execution reports and fills | Verified | Live phase | Shipped |
| Replayed executions on reconnect | Verified | Live phase | Shipped |
| Trade bust and correction handling | Implemented | No bust occurred during testing | 0.9 |
| Order conditions, price, volume, percent, execution | Verified | Live phase | Shipped |
| Order conditions, standalone time | Blocked | Venue rejects the condition | Unscheduled |
| Combination orders | Implemented | No live phase exists | 0.9 |
| Market data, top of book | Verified | Live phase | Shipped |
| Market data, frozen and delayed | Verified | Live measurement | Shipped |
| Market depth | Implemented | No depth updates observed. Entitlement unconfirmed | 0.9 |
| Historical bars, ticks, head timestamp, schedules | Verified | Live phase | Shipped |
| Contract definitions by identifier and symbol | Verified | Live phase | Shipped |
| Contract definitions by ISIN or CUSIP | Implemented | Identifiers carried on the request. Unconfirmed live | 0.9 |
| Option chain discovery | Absent | Definition query returns one contract by design | 0.8 |
| Scanners | Verified | Live phase | Shipped |
| News, providers, articles, historical, bulletins | Verified | Live phase | Shipped |
| Fundamental data | Verified | Live phase | Shipped |
| Account values | Verified | Live phase | Shipped |
| Account summary | Implemented | Live phase did not observe it | 0.9 |
| Positions and round trip tracking | Implemented | Live phase requires a fill it did not obtain | 0.9 |
| P&L, per contract | Verified | Live phase | Shipped |
| P&L, account level subscription | Implemented | Live phase did not observe it | 0.9 |
| Option exercise and lapse | Accepted, not served | | 0.8 |
| Option analytics, implied volatility and price | Accepted, not served | | 0.8 |
| Wall Street Horizon event data | Accepted, not served | Requires a separate data subscription | 1.0 |
| Financial advisor allocation | Absent | | 1.0 |
| Tick by tick data | Absent | Transport not offered at login | 0.9 |

## API surface

| Measure | Count |
| --- | --- |
| Rust client calls | 92 |
| Python client calls | 123 |
| Callbacks | 125 |
| Rust calls served | 88 |
| Rust calls accepted, not served | 4 |

### Calls not served

| Call | Surface | Status | Target |
| --- | --- | --- | --- |
| `req_sec_def_opt_params` | Python | Accepted, not served | 0.8 |
| `req_sec_def_opt_params` | Rust | Absent | 0.8 |
| `exercise_options` | Python | Accepted, not served | 0.8 |
| `exercise_options` | Rust | Absent | 0.8 |
| `calculate_implied_volatility` | Python | Accepted, not served | 0.8 |
| `calculate_implied_volatility` | Rust | Absent | 0.8 |
| `calculate_option_price` | Python | Accepted, not served | 0.8 |
| `calculate_option_price` | Rust | Absent | 0.8 |
| `cancel_calculate_implied_volatility` | Rust | Absent | 0.8 |
| `cancel_calculate_option_price` | Rust, Python | Absent in Rust, accepted and not served in Python | 0.8 |
| `req_wsh_meta_data` | Rust, Python | Accepted, not served | 1.0 |
| `req_wsh_event_data` | Rust, Python | Accepted, not served | 1.0 |
| `cancel_wsh_meta_data` | Rust | Absent | 1.0 |
| `cancel_wsh_event_data` | Rust | Absent | 1.0 |
| `request_fa` | Rust, Python | Accepted, not served | 1.0 |
| `replace_fa` | Rust, Python | Accepted, not served | 1.0 |

## Asset classes

The contract layer names 24 security types. Coverage is stated per path.

| Class | Definition | Market data | Orders | Target |
| --- | --- | --- | --- | --- |
| Equity | Verified | Verified | Verified | Shipped |
| Equity option | Verified | Verified | Verified | Shipped |
| Forex | Verified | Verified | Verified | Shipped |
| Future | Verified | Verified | Blocked, venue reports an ambiguous contract | 0.9 |
| Futures option | Verified | Implemented | Implemented | 0.9 |
| Index | Verified | Verified | Absent | 0.9 |
| Bond | Implemented | Implemented | Absent | 0.9 |
| Warrant | Implemented | Implemented | Absent | 0.9 |
| Combination | Verified | Not applicable | Implemented | 0.9 |
| Crypto, CFD, commodity, fund, forward, bill | Implemented | Implemented | Absent | Unscheduled |
| Venues outside the United States | Verified | Verified | Blocked, orders return inactive with no stated reason | 0.9 |

## Releases

Current release: 0.7.1. Milestones are release versions under semantic versioning.

| Release | Meaning | API compatibility |
| --- | --- | --- |
| 0.7.x | Current. The paths in the capability matrix marked Verified are usable | Breaking changes may occur in a minor release |
| 0.8 | Option surface complete | Breaking changes may occur |
| 0.9 | Asset class and instrumentation coverage complete | Breaking changes may occur |
| 1.0 | Every call has defined behaviour and published status | Breaking changes require a major release |

A capability marked Shipped in the Target column is available in 0.7.x.

## Milestones

Exit criteria, not dates. A milestone closes when every criterion is met and demonstrated.

### 0.8 Option surface

| ID | Requirement | Acceptance |
| --- | --- | --- |
| 0.8.1 | Option chain discovery | Every expiration and strike a venue lists, for a named underlying, delivered through the chain callbacks |
| 0.8.2 | Option exercise and lapse | Request reaches the venue and the resulting position change is observed |
| 0.8.3 | Option analytics | Implied volatility and option price return values, or the calls are removed with the reason recorded |

### 0.9 Asset class and instrumentation completeness

| ID | Requirement | Acceptance |
| --- | --- | --- |
| 0.9.1 | Futures orders | Order accepted by the venue, with a regression test that fails if the ambiguity returns |
| 0.9.2 | Index, bond and warrant orders | Order accepted for each class against a live session |
| 0.9.3 | Orders outside the United States | One venue accepted end to end, on an account holding the permission |
| 0.9.4 | Combination orders | Live phase covering leg construction and acceptance |
| 0.9.5 | Market depth | Depth updates observed, or entitlement recorded as the cause |
| 0.9.6 | Account summary and account level P&L | Both observed end to end in a live phase |
| 0.9.7 | Positions round trip | Live phase completes a fill and reconciles the resulting position |
| 0.9.8 | Contract lookup by ISIN and CUSIP | Lookup confirmed against a live session |
| 0.9.9 | Trade bust and correction | Handling confirmed against a replayed or synthetic bust |
| 0.9.10 | Tick by tick data | Available, or the transport requirement recorded |

### 1.0 Contract stability

| ID | Requirement | Acceptance |
| --- | --- | --- |
| 1.0.1 | No silent request | Every call either serves its request or reports through the error callback why it cannot |
| 1.0.2 | Pre connection behaviour | Behaviour of a call issued before connection is defined, documented, and identical across the Rust and Python surfaces |
| 1.0.3 | Financial advisor allocation | Allocation groups and methods carried, or the surface removed |
| 1.0.4 | Event data | Wall Street Horizon calls served, or the surface removed |
| 1.0.5 | Compatibility statement | Every call published with its status and the evidence establishing it |
| 1.0.6 | Second factor | Approval path covered by an automated live check |

## Excluded surface

| Surface | Reason |
| --- | --- |
| Display groups and screen linkage | Client application state, not venue state |
| Order staging without transmission | Client application state. A venue side hold until activation is carried |
| Financial advisor profile screens | Client application state. Allocation on an order is in scope, see 1.0.3 |

## Verification

| Method | What it establishes |
| --- | --- |
| Unit tests, 1126 engine and client, 308 Python bindings | The call encodes and decodes as specified |
| Live session phases against a paper account | The venue accepts the request and returns what is expected |
| Continuous integration on every push | Tests, lint, documentation, and builds for Linux, macOS and Windows |

Test count is not coverage. It states what is checked, not what fraction of venue behaviour is reached.
