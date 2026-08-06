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

| Capability | Status | Evidence | Workstream |
| --- | --- | --- | --- |
| Session establishment and login | Verified | Live phase | Available |
| Reconnect and subscription rebuild | Verified | Live phase | Available |
| Second factor approval | Implemented | Live logins only. Paper logins do not present the factor | W3 |
| Order submission, 23 order types | Verified | Live phase | Available |
| Order modification | Verified | Live phase | Available |
| Order cancellation, single, by permId, global | Verified | Live phase | Available |
| Bracket and OCA linkage | Verified | Live phase | Available |
| Execution reports and fills | Verified | Live phase | Available |
| Replayed executions on reconnect | Verified | Live phase | Available |
| Trade bust and correction handling | Verified | Synthetic bust and correction reports | Available |
| Order conditions, price, volume, percent, execution | Verified | Live phase | Available |
| Order conditions, standalone time | Blocked | Venue rejects the condition. The vendor client is refused the same condition, so the behaviour matches | None required |
| Combination orders | Implemented | No live phase exists | W2 |
| Market data, top of book | Verified | Live phase | Available |
| Market data, frozen and delayed | Verified | Live measurement | Available |
| Market depth | Verified | Live measurement, 144 updates in 20 seconds | Available |
| Historical bars, ticks, head timestamp, schedules | Verified | Live phase | Available |
| Contract definitions by identifier and symbol | Verified | Live phase | Available |
| Contract definitions by ISIN or CUSIP | Verified | Live lookup by ISIN resolved the contract | Available |
| Option chain discovery | Blocked | Request, reply parsing and delivery are covered by tests. The session answers other requests, including one whose reply is never sent unasked. A chain request is answered in no shape at all, and nothing in the client conditions the request on a permission, so this is a question for whoever provisions the account | W1 |
| Scanners | Verified | Live phase | Available |
| News, providers, articles, historical, bulletins | Verified | Live phase | Available |
| Fundamental data | Verified | Live phase | Available |
| Account values | Verified | Live phase | Available |
| Account summary | Verified | Live session, rows and completion observed | Available |
| Positions and round trip tracking | Implemented | Live phase requires a fill it did not obtain | W2 |
| P&L, per contract | Verified | Live session, valued from the venue's own overnight marks against live quotes | Available |
| P&L, account level subscription | Verified | Live session, reporting a daily figure for a held account rather than falling back to zero | Available |
| Option exercise and lapse | Implemented | No position has been exercised yet | W1 |
| Option analytics, implied volatility and price | Accepted, not served | | W1 |
| Wall Street Horizon event data | Accepted, not served | Requires a separate data subscription | W3 |
| Financial advisor allocation | Absent | | W3 |
| Tick by tick data | Absent | Served by a separate transport whose availability the venue grants; not yet observed as granted to this account | W2 |

## API surface

| Measure | Count |
| --- | --- |
| Rust client calls | 94 |
| Python client calls | 124 |
| Callbacks | 127 |
| Rust calls served | 90 |
| Rust calls accepted, not served | 4 |

### Calls not served

| Call | Surface | Status | Workstream |
| --- | --- | --- | --- |
| `calculate_implied_volatility` | Python | Accepted, not served | W1 |
| `calculate_implied_volatility` | Rust | Absent | W1 |
| `calculate_option_price` | Python | Accepted, not served | W1 |
| `calculate_option_price` | Rust | Absent | W1 |
| `cancel_calculate_implied_volatility` | Rust | Absent | W1 |
| `cancel_calculate_option_price` | Rust, Python | Absent in Rust, accepted and not served in Python | W1 |
| `req_wsh_meta_data` | Rust, Python | Accepted, not served | W3 |
| `req_wsh_event_data` | Rust, Python | Accepted, not served | W3 |
| `cancel_wsh_meta_data` | Rust | Absent | W3 |
| `cancel_wsh_event_data` | Rust | Absent | W3 |
| `request_fa` | Rust, Python | Accepted, not served | W3 |
| `replace_fa` | Rust, Python | Accepted, not served | W3 |

## Asset classes

The contract layer names 24 security types. Coverage is stated per path.

| Class | Definition | Market data | Orders | Workstream |
| --- | --- | --- | --- | --- |
| Equity | Verified | Verified | Verified | Available |
| Equity option | Verified | Verified | Verified | Available |
| Forex | Verified | Verified | Verified | Available |
| Future | Verified | Verified | Verified | Available |
| Futures option | Verified | Implemented | Implemented | W2 |
| Index | Verified | Verified | Implemented | W2 |
| Bond | Implemented | Implemented | Implemented | W2 |
| Warrant | Implemented | Implemented | Implemented | W2 |
| Combination | Verified | Not applicable | Implemented | W2 |
| Crypto and CFD | Verified | Verified | Verified, crypto requires an immediate-or-cancel or minutes instruction | Available |
| Commodity, fund, forward, bill | Implemented | Implemented | Absent | W2 |
| Venues outside the United States | Verified | Verified | Verified | Available |

## Release policy

One release: **1.0.0**. There are no incremental feature releases before it.

1.0.0 ships when every documented client call is served and the behaviour matches the vendor client, so that an existing integration can be repointed at this client without changing its code. Partial coverage is not released.

| Item | Position |
| --- | --- |
| Current code | 0.7.x, development. Tagged for traceability, not offered as a supported release |
| 1.0.0 | First supported release. Requires every criterion in the workstreams below |
| Coverage bar for 1.0.0 | Every call served, or removed with the reason recorded. No call accepted and left unanswered |
| Compatibility from 1.0.0 | Breaking changes require a major version |

## Workstreams

Every workstream gates 1.0.0. Exit criteria, not dates. A workstream closes when every criterion is demonstrated.

### W1 Option surface

| ID | Requirement | Acceptance | Status |
| --- | --- | --- | --- |
| W1.1 | Option chain discovery | Every expiration and strike a venue lists, for a named underlying, delivered through the chain callbacks | Open, blocked on the venue answering the request |
| W1.2 | Option exercise and lapse | Request reaches the venue and the resulting position change is observed | Open |
| W1.3 | Option analytics | Implied volatility and option price return values, or the calls are removed with the reason recorded | Open |

### W2 Asset class and instrumentation coverage

| ID | Requirement | Acceptance | Status |
| --- | --- | --- | --- |
| W2.1 | Futures orders | Order accepted by the venue, with a regression test that fails if the ambiguity returns | Met |
| W2.2 | Index, bond and warrant orders | Order accepted for each class against a live session | Open |
| W2.3 | Orders outside the United States | One venue accepted end to end, on an account holding the permission | Met |
| W2.4 | Combination orders | Live phase covering leg construction and acceptance | Open |
| W2.5 | Market depth | Depth updates observed, or entitlement recorded as the cause | Met |
| W2.6 | Account summary and account level P&L | Both observed end to end in a live phase | Met |
| W2.7 | Positions round trip | Live phase completes a fill and reconciles the resulting position | Open |
| W2.8 | Contract lookup by ISIN and CUSIP | Lookup confirmed against a live session | Met |
| W2.9 | Trade bust and correction | Handling confirmed against a replayed or synthetic bust | Met |
| W2.10 | Tick by tick data | Available, or the transport requirement recorded | Open |
| W2.11 | Remaining security types | Crypto, CFD, commodity, fund, forward and bill orders accepted against a live session, or the class recorded as not orderable by this venue | Met for crypto. Commodity, fund, forward and bill open |

### W3 Call contract and behaviour parity

| ID | Requirement | Acceptance | Status |
| --- | --- | --- | --- |
| W3.1 | No silent request | Every call either serves its request or reports through the error callback why it cannot | Met |
| W3.2 | Pre connection behaviour | A request issued before connection is reported on the error callback with code 504 on the Python surface, matching the reference client, and returns a typed error on the Rust surface | Met |
| W3.3 | Financial advisor allocation | Allocation groups and methods carried, or the surface removed | Open |
| W3.4 | Event data | Wall Street Horizon calls served, or the surface removed | Open |
| W3.5 | Compatibility statement | Every call published with its status and the evidence establishing it | Open |
| W3.6 | Second factor | Approval path covered by an automated live check | Open |

## Excluded surface

| Surface | Reason |
| --- | --- |
| Display groups and screen linkage | Client application state, not venue state |
| Order staging without transmission | Client application state. A venue side hold until activation is carried |
| Financial advisor profile screens | Client application state. Allocation on an order is in scope, see W3.3 |

## Verification

| Method | What it establishes |
| --- | --- |
| Unit tests, 1126 engine and client, 308 Python bindings | The call encodes and decodes as specified |
| Live session phases against a paper account | The venue accepts the request and returns what is expected |
| Continuous integration on every push | Tests, lint, documentation, and builds for Linux, macOS and Windows |

Test count is not coverage. It states what is checked, not what fraction of venue behaviour is reached.
