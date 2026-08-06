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
| Order conditions, standalone time | Verified | Live session: a limit carrying a time condition rests and cancels. It was recorded as refused while the venue's reasons were being discarded by this client | Available |
| Combination orders | Verified | Live session: a two-leg vertical previews at the margin the position carries, and the reverse legs are refused as the opposite position | Available |
| Market data, top of book | Verified | Live phase | Available |
| Market data, frozen and delayed | Verified | Live measurement | Available |
| Market depth | Verified | Live measurement, 144 updates in 20 seconds | Available |
| Historical bars, ticks, head timestamp, schedules | Verified | Live phase | Available |
| Contract definitions by identifier and symbol | Verified | Live phase | Available |
| Contract definitions by ISIN or CUSIP | Verified | Live lookup by ISIN resolved the contract | Available |
| Option chain discovery | Verified | Live session: an equity underlying returns every expiration and strike each venue lists | Available |
| Scanners | Verified | Live phase | Available |
| News, providers, articles, historical, bulletins | Verified | Live phase | Available |
| Fundamental data | Verified | Live phase | Available |
| Account values | Verified | Live phase | Available |
| Account summary | Verified | Live session, rows and completion observed | Available |
| Positions and round trip tracking | Verified | Live session: a market order fills and the holding read back moves by the quantity filled | Available |
| P&L, per contract | Verified | Live session, valued from the venue's own overnight marks against live quotes | Available |
| P&L, account level subscription | Verified | Live session, reporting a daily figure for a held account rather than falling back to zero | Available |
| Option exercise and lapse | Verified | Live session: one call exercised, filled at the strike, and the holding it delivered observed. A lapse before the last trading day is refused by the venue | Available |
| Option analytics, implied volatility and greeks | Verified | Live session: the venue's own model arrives on an option subscription. A volatility inverted from a caller's price cannot be served; this protocol carries no request for it | Available |
| Wall Street Horizon event data | Accepted, not served | Requires a separate data subscription | W3 |
| Financial advisor allocation | Absent | | W3 |
| Tick by tick data | Blocked | The feed rides a service of its own. This session is sent no list of the services it may reach and a request for that list is refused | W2 |

## API surface

| Measure | Count |
| --- | --- |
| Canonical calls | 77 |
| Served, Rust | 69 |
| Served, Python | 69 |
| Accepted and not served, Rust | 4 |
| Accepted and not served, Python | 8 |
| Canonical callbacks | 81 |

Counted from the source by `scripts/gen_api_docs.py`, which CI re-runs and
compares. The per-call matrix, and how each call's status was established, is
in the coverage reference.

### Calls not served

Every call that exists with the expected signature and reports, through
the error callback, that it cannot be served. Taken from the generated
coverage matrix, which CI checks against the source.

| Call | Rust | Python |
| --- | :---: | :---: |
| `calculate_implied_volatility` | - | STUB |
| `cancel_calculate_implied_volatility` | - | STUB |
| `calculate_option_price` | - | STUB |
| `cancel_calculate_option_price` | - | STUB |
| `request_fa` | STUB | STUB |
| `replace_fa` | STUB | STUB |
| `req_wsh_meta_data` | STUB | STUB |
| `req_wsh_event_data` | STUB | STUB |

## Asset classes

The contract layer names 24 security types. Coverage is stated per path.

| Class | Definition | Market data | Orders | Workstream |
| --- | --- | --- | --- | --- |
| Equity | Verified | Verified | Verified | Available |
| Equity option | Verified | Verified | Verified | Available |
| Forex | Verified | Verified | Verified | Available |
| Future | Verified | Verified | Verified | Available |
| Futures option | Verified | Implemented | Implemented | W2 |
| Index | Verified | Verified | Blocked, the venue supports no order on the contract type | None required |
| Bond | Verified | Implemented | Verified, quantified in face value | Available |
| Warrant | Verified | Implemented | Blocked, the venue supports no order of this kind on the exchange and security type | None required |
| Combination | Verified | Not applicable | Verified | Available |
| Crypto and CFD | Verified | Verified | Verified, crypto requires an immediate-or-cancel or minutes instruction | Available |
| Commodity | Verified | Implemented | Verified | Available |
| Bill | Verified | Implemented | Verified | Available |
| Fund | Verified | Implemented | Blocked, quantified in cash and then refused for residency, which is a property of the account | None required |
| Forward | Implemented | Implemented | Absent, the session states no permission for the type | None required |
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
| W1.1 | Option chain discovery | Every expiration and strike a venue lists, for a named underlying, delivered through the chain callbacks | Met |
| W1.2 | Option exercise and lapse | Request reaches the venue and the resulting position change is observed | Met. An exercise of one call is filled at the strike and the holding it delivers appears. A lapse is refused before the contract's last trading day, which is the venue's rule and is reported as it states it |
| W1.3 | Option analytics | Implied volatility and option price return values, or the reason they cannot be recorded | Met. This protocol carries no request that takes a caller-supplied option price or volatility for the venue to work back from, so neither can be served. Both calls are kept and report that, because a caller written against the reference client calls them |

### W2 Asset class and instrumentation coverage

| ID | Requirement | Acceptance | Status |
| --- | --- | --- | --- |
| W2.1 | Futures orders | Order accepted by the venue, with a regression test that fails if the ambiguity returns | Met |
| W2.2 | Index, bond and warrant orders | Order accepted for each class against a live session, or the venue's refusal recorded | Met. A bond is accepted and returns margin. The venue refuses an index order for every account: orders are not supported for the contract type. It refuses a warrant order placed through an interface of this kind, on the exchange and security type combination |
| W2.3 | Orders outside the United States | One venue accepted end to end, on an account holding the permission | Met |
| W2.4 | Combination orders | Live phase covering leg construction and acceptance | Met. A two-leg vertical is ordered by its legs alone, with no lookup, and previews at the margin the position carries. Reversing the legs is refused, as it describes the opposite position |
| W2.5 | Market depth | Depth updates observed, or entitlement recorded as the cause | Met |
| W2.6 | Account summary and account level P&L | Both observed end to end in a live phase | Met |
| W2.7 | Positions round trip | Live phase completes a fill and reconciles the resulting position | Met. A market order fills and the holding read back afterwards moves by the quantity filled |
| W2.8 | Contract lookup by ISIN and CUSIP | Lookup confirmed against a live session | Met |
| W2.9 | Trade bust and correction | Handling confirmed against a replayed or synthetic bust | Met |
| W2.10 | Tick by tick data | Available, or the transport requirement recorded | Met by record. The feed rides a service of its own; this session is sent no list of the services it may reach and a request for that list is refused. The call refuses with that reason rather than waiting on a subscription that is acknowledged and never delivers |
| W2.11 | Remaining security types | Crypto, CFD, commodity, fund, forward and bill orders accepted against a live session, or the class recorded as not orderable by this venue | Met. Crypto, commodity and bill are accepted and return margin. The session states no forward permission at all. A fund is refused for residency, which is a property of the account |

### W3 Call contract and behaviour parity

| ID | Requirement | Acceptance | Status |
| --- | --- | --- | --- |
| W3.1 | No silent request | Every call either serves its request or reports through the error callback why it cannot | Met. The display-group calls were the exception: they accepted and did nothing at all, and are served now |
| W3.2 | Pre connection behaviour | A request issued before connection is reported on the error callback with code 504 on the Python surface, matching the reference client, and returns a typed error on the Rust surface | Met |
| W3.3 | Financial advisor allocation | Allocation groups and methods carried, or the surface removed | Open |
| W3.4 | Event data | Wall Street Horizon calls served, or the surface removed | Open |
| W3.5 | Compatibility statement | Every call published with its status and the evidence establishing it | Met. The coverage matrix carries, per call, how its status was established: exercised against a live session, exercised by the offline suites, stating why it cannot be served, or exercised by neither. Derived from the suites themselves, so it cannot go quietly out of date |
| W3.6 | Second factor | Approval path covered by an automated live check, or the reason no such check can run recorded | Blocked. The paper session used for verification is never presented with a second factor, so the approval path cannot be exercised against it. The wire and the gate that waits on it are covered by fifteen tests |

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
