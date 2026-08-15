# Compatibility status

Capability status for the Rust client, the Python bindings, and the engine
underneath both. Status is assigned from a named artifact — a test, a script,
or a recorded server response — not from code inspection.

| Status | Definition |
| :---: | --- |
| ✅ Supported | Implemented; exercised against IBKR production servers; the response is parsed and delivered to the caller |
| 🔬 Implemented | Implemented and unit-tested; the request reaches the server, but the response has not been observed end to end |
| ⛔ Unavailable | Not carried by the protocol; the call returns an error stating the reason |

Verification runs against a paper account on IBKR production servers, and the order path has additionally been run once end to end on a funded account during regular hours. Where a market or data set is not entitled to that account, the entitlement response is recorded as the result.

---

## Client surfaces

| Surface | Status | Verification |
| --- | :---: | --- |
| `EClient` / `EWrapper` (TWS API shape) | ✅ Supported | `tests/ib_paper_compat`, `tests/python/test_compat_tier1..3.py` |
| `ib_async`, unmodified | ✅ Supported | Their `IB` on this engine via `ibx.ib_async.attach`; their events, async variants and types, with no gateway. All 67 transport calls their library makes are carried, measured against their own source and gated. Their own test suite runs against it: 2 of 3 pass, and `test_request_error_raised` cannot pass against any server, because 321 is in their `warningCodes` and a warning never ends the request it belongs to. `tests/python/test_ib_async_transport.py`, `tests/ib_async_upstream/conftest.py` |
| `ibx.IB` (ib_async shape) | ✅ Supported | 90/90 methods present; `tests/python/test_ib_facade.py`, `scripts/sdk_sweep.py` |
| `ibx::api::Client` (Rust) | ✅ Supported | 77/77 callable; 3 return an error naming a local-process facility this client does not have |
| Gateway settings | ✅ Supported | 17 settings carried, 7 recorded as having no counterpart; `tests/python/test_gateway_settings.py`, `tests/python/test_settings_parity.py`; session opened under a stated build and time zone |
| Rust/Python equivalence | ✅ Supported | 4 static gates (settings, order fields, surface, error behaviour) plus `scripts/conformance.py --compare`, which compares 10 server responses across both clients |

## Market data

| Capability | Status | Verification |
| --- | :---: | --- |
| Top of book | ✅ Supported | Streaming and snapshot; US equities and FX; `scripts/sdk_sweep.py`, `tests/python/test_live_quotes.py`. Concurrent subscribers on one contract share one wire subscription |
| Market depth (L2) | ✅ Supported | A book is asked for once, at the venue named, and every level names it. Ten levels a side on ib_async's own ticker, and 95,985 levels over a 175-minute session on this client's wrapper; `tests/python/test_ib_async_depth.py` holds the delivery without a session. On this account IEX answers — its Level II is fee-waived — and returns 227 levels on one SPY subscription; NASDAQ and CME refuse by name, and the refusal reaches the caller. A book asked for on no particular venue is acknowledged and produces nothing, which is what an account with no aggregate entitlement is answered with. `tests/python/test_live_depth.py`, `src/bin/capture_depth.rs` |
| Historical bars | ✅ Supported | 9 markets in one session (`src/bin/capture_global.rs`); `keepUpToDate` verified in `tests/python/test_historical_and_scanner.py` |
| Historical ticks and schedules | ✅ Supported | `scripts/sdk_sweep.py`; unsupported tick types return an error rather than substituting another series |
| Tick-by-tick quotes | ✅ Supported | FX and US equities, concurrent streams, each record carrying its request id; `tests/python/test_live_python_wrappers.py` |
| Tick-by-tick trades | ✅ Supported | 67,785 trades over a 20-minute session; 327 in the first twenty seconds of one subscription. `Last` and `AllLast` are distinct streams, and a stream is asked for by the venue's id for the contract, which is resolved first when the caller states a description |
| Real-time bars | ✅ Supported | Five-second bars streaming during regular hours, each carrying open, high, low, close and volume, alongside a book on the same session and through ib_async's own `reqRealTimeBars` |
| Trading halt status | ✅ Supported | Tick 437 decoded from status mask and status index; `src/bin/capture_status.rs` |
| Tick attributes | ✅ Supported | Per-trade `unreported` and `pastLimit` observed to vary within one stream |
| Venue map behind the exchange mask | ✅ Supported | Asked for beside the quote and answered at regular trading hours with 18 venues, each with the letter the mask's bits refer to. Outside those hours the server states none, and a venue's letter is empty until it does |

## Orders

| Capability | Status | Verification |
| --- | :---: | --- |
| 23 order types | ✅ Supported | `whatIf` preview accepted by the server for each; `tests/ib_paper_compat` |
| Order fields | ✅ Supported | 154 fields: 125 transmitted, 29 documented as not carried by the protocol, 0 silently dropped; `docs/order-field-reach.md`, regenerated and compared in CI |
| Non-US markets | ✅ Supported | Previews accepted on DE, NL, GB, CH, AU, CA, US equities and FX; JP and HK rejected for lot size, which is the exchange rule and is surfaced to the caller |
| Modify, cancel, global cancel | ✅ Supported | `scripts/sdk_lifecycle.py` (place → modify → cancel), `tests/ib_paper_compat` Phase 9 / 9b |
| Brackets, OCA, combos | ✅ Supported | Per-leg pricing; leg order validated by server rejection of the inverted spread; `src/bin/capture_combo.rs` |
| Conditions | ✅ Supported | All 6 types (price, volume, percent change, margin, execution, time) accepted and held by the server; `tests/ib_paper_compat` Phase 60 |
| Order acceptance | ✅ Supported | Every change to an order answers with the order as this client sent it and the status it is now in, which is the pair the reference client answers with. 45 orders placed, modified and withdrawn over a 15-cycle session, every one reaching Cancelled, with no error |
| Executions and fills | ✅ Supported | Fill reported and position reconciled; execution report retains server fields including unnamed tags; `tests/ib_paper_compat` Phase 97 |
| A round trip on a funded account | ✅ Supported | One same-day option bought and sold on a live account during regular hours: limit in, filled, limit out, position and account values reconciled, and nothing left open. The order id an account has already used is refused by name, so ids are counted from what the account last used rather than from one |
| Option exercise and lapse | ✅ Supported | Both submitted for a resolved option contract; server response 399 *"You have not got the number of options requested to be exercised"* delivered to the caller |

## Account

| Capability | Status | Verification |
| --- | :---: | --- |
| Account values and summary | ✅ Supported | 135 values, each in the currency the server states; subscribed at connect; `tests/python/test_account_updates_and_pnl.py` |
| Positions and P&L | ✅ Supported | Delivered on login and after each fill; `tests/python/test_account_updates_and_pnl.py` |
| Managed accounts | ✅ Supported | `tests/python/test_account_updates_and_pnl.py` |
| Financial advisor configuration | 🔬 Implemented | Request reaches the server on both clients; the response requires an advisor account |

## Reference data

| Capability | Status | Verification |
| --- | :---: | --- |
| Contract details | ✅ Supported | 11 of 12 resolved across 9 countries; the 12th matched 2 contracts and returned an ambiguity error rather than a selection; `src/bin/capture_global.rs` |
| Option chains, symbol search | ✅ Supported | `scripts/sdk_sweep.py` |
| Scanners, fundamentals | ✅ Supported | 697 KB scanner parameter set, fundamental report; `tests/python/test_historical_and_scanner.py` |
| News | ✅ Supported | 117 providers parsed. Headline retrieval requires a news subscription; this account holds none, and every provider returns an empty result set |
| Exchange directory | ✅ Supported | 203 exchanges, in the two sections the venue states them in: shares and derivatives. What each carries and which group each aggregates into are not stated by the venue and are not stated here |
| Corporate events calendar | ✅ Supported | The calendar states what it carries — 43 event types with their field schemas, 179 KB — over the security-definition connection, and answers an event query with a well-formed result. Events themselves require a Wall Street Horizon subscription; this account holds none, and every query, by contract and by filter, is answered with an empty set. A query can be withdrawn: it is one message and one answer, so what is withdrawn is the answer. `src/bin/capture_calendar.rs` |
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
- **A caller's request id is not what the venue is asked under.** Every
  subscription is asked for under an id this client allocates and is mapped
  back to the caller who wanted it. The venue echoes an id back, so one taken
  from the caller cannot be told apart from one allocated here. The counterpart
  allocates the same way, from one upward, and keys its subscriptions on it.
- **`keepUpToDate` queries are closed on first response.** Continuation is
  provided by folding the 5-second bar stream into the requested bar size.
- **Historical execution reports require a window within 7 days.** A request
  without one is rejected in full.

## Sessions

An account takes one logon at a time. The venue states this at connect, names
the session already holding the account, and states when that session logged
in.

| Behaviour | Status | Verification |
| --- | :---: | --- |
| The session holding the account is reported to the caller | ✅ Supported | `competing_session()` returns the address, the logon time, and whether this session may trade. A read-only flag from the venue is carried as stated |
| A logon later than this one is another client, and keeps the account | ✅ Supported | A reconnect that finds one reports it and stops; retrying cannot change it. Both times are read from the venue's own clock, so two machines' clocks cannot decide it |
| A logon at or before this one is this session's own, still being reaped | ✅ Supported | The reconnect completes over it, which is what an ordinary recovery is |
| The heartbeat is the interval the venue answered with | ✅ Supported | The interval a logon proposes is not what it is held to; the answer is read from the logon response and applied on every reconnect |
| A reconnect follows the venue | ✅ Supported | It uses the hosts this session reached the venue through, on the port the venue named in its redirect, and stops walking hosts when one answers and refuses |
| The first connect knocks on the next door when one does not answer | ✅ Supported | One host per region. A door that answers and refuses ends the walk, so a refused logon is not repeated at every door |
| The last order id is kept between runs | ✅ Supported | An order id belongs to the account, not the process: an id it has already used is refused by name. The last one handed out is remembered per account, kind of session and client id, and the next run counts on from it |
| A session survives losing its connection | ✅ Supported | A dropped connection is rebuilt on the session already open, with no second factor: five forced drops recovered in 2-8s, and an eight hour session rode through its losses unattended |
| A session does not survive its process | ✅ Documented | The venue holds a session for a socket, not for an account: killed without logging out, it was already gone forty seconds later, and a later start is answered with a handshake. The counterpart stores no session either — it stays connected rather than restarting. So a restart costs a second factor here exactly as it does there, and staying up costs none |
| A session that has ended answers at once | ✅ Supported | Requests made after a terminal loss are refused with 504 immediately, rather than waiting out a timeout each. Every request already answered keeps the venue's own answer |

One session, held open for 175 minutes across a market open: 106,053 quotes,
180,433 trades, 95,985 book rows and 4,148 bars, with no unrequested
disconnect and no error other than the venue's own answer for a series it does
not hold. `scripts/endurance.py --minutes 175`.

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
- Tick 437 carries four 32-bit integers. Three are read — a status mask, a
  number, and an index naming one status — and the fourth is left alone. The
  number is kept exactly as stated: its unit is not established, and no reading
  of it would be found rather than invented. The counterpart reads the same
  three, keeps the same number, and no code path there reads it either.

## Test inventory

| Suite | Count | Requires credentials |
| --- | ---: | :---: |
| Rust unit and integration | 1,574 | No |
| Python | 381 | No |
| Python, live | 131 | Yes |
| Paper compatibility suite (136 phases) | 26 tests | Yes |

Counted rather than stated: `scripts/check_status_counts.py` names every test
in each suite and fails the gate when this table disagrees with it, so a
figure here cannot go quietly out of date as the suites grow.

Run order and environment are documented in
[docs/engineering-notes.md](docs/engineering-notes.md).

## Refusals

A request this client will not send is reported through `error(reqId, code,
message)` and the call returns, as the reference client does. The number is the
one that client reports for the same class: 321 for a request that fails
validation, 200 for a contract description that matches nothing, 504 for a call
with no session, and 327 for binding orders entered elsewhere, which that
client refuses for any client but the one they bind to. Construction and
configuration raise, as does a synchronous call with a return value.

## Planned work

[docs/api-alignment-plan.md](docs/api-alignment-plan.md) lists one open item: settings decided by configuration where the server states a grant for them.

The surface question is settled. `EClient`/`EWrapper` is the product; the ib_async shape and the Rust shape are adapters over it, and the rust-ibapi shape is not written.

The one capability a gateway has and this does not is [#2](https://github.com/userFRM/ibx/issues/2): several programs sharing one logon. A gateway rents its single session out over a local socket, and this client, having no socket, cannot. One process holds one session.

## Release criteria

Live sessions are run at market hours until a session produces no new defect,
and a session is held open under load until it produces none.

A 20-minute session cycling subscriptions across five contracts — quotes,
books, trade streams and bars, subscribed and withdrawn every minute — ran 18
cycles with every stream growing throughout: 3,391 price ticks, 67,785 trades,
16 bars a cycle, and one error, which is the venue stating that a currency
pair has no trades to report. A second session placed, moved and withdrew
three orders a cycle for fifteen cycles: 45 orders, every one reaching
Cancelled, 270 acceptances and 270 status changes, no error. Earlier runs of the same session are what found
four of the defects below; none of them was reachable by any offline suite,
and two were invisible to the live suites as well, because those skipped when
no data arrived.

The most recent session produced fifteen, listed here as the caller-visible
symptom:

| Symptom | Cause |
| --- | --- |
| `place_order` on a contract carrying no id did nothing, and reported nothing | Registered under contract id 0 and sent; the venue has nothing to match and answers nothing |
| An order stating a symbol and no exchange was filled on a venue the caller never named | No destination required; the definition lookup answered with whichever listing came first |
| A refused request raised where the reference client reports it | Refusals raised on 25 request-shaped methods; a program written against that client has no exception handling there |
| A bar whose low is below zero took the process down | 31-bit sign extension performed in an i32, which the intermediate does not fit |
| `util.df(bars)` refused the bars an ib_async program asked for | The date was handed over in a spelling their parser reads as naive, which their frame conversion rejects |
| A SMART book's levels named no venue | Gathered by reading the session's exchange directory as though its sections were aggregation groups |
| `disconnect()` reported connectivity lost | A session the caller ended and a session that went away were the same event |
| A second book on a venue already streaming returned nothing, and said nothing | The venue answers it with the tag it is already using, and levels were delivered to the first request holding that tag |
| A caller's book was attributed to a venue they never asked for | This client's own subscription ids were numbered from the same range a caller states |
| `accountSummary()` was answered with nothing before the account was fully stated | Account data counted as received only once the typed copy was built |
| Every stream on a session went silent after a minute of subscribing and withdrawing | A book was gathered by asking each venue the contract is routed to; four contracts cycled put seventy subscribes and as many withdrawals on the connection a minute, and the venue stopped answering it |
| A book on a named venue delivered a fraction of its levels | The section tag was read a byte early, so a section named no subscription and its levels waited for a sentinel further in |
| Bars stopped arriving after the seventh minute, with every other stream healthy | A request id was marked finished and never unmarked, so bars answering a later request under it were delivered as a continuation of the first |
| An order was placed and nothing said the venue had taken it | Only the status was answered; the reference client answers an order's every change with the order it holds as well, and its own method for it sends the pair |
| A trade stream delivered nothing, and a request for one was handed the contract's quotes | The stream went out with contract id 0 and was refused against a query nobody was told about; and it was held in the quote tables, which it also emptied when withdrawn |

Two suites were added rather than a symptom fixed: every wire parser is now
given malformed input (`tests/malformed_input.rs`), which is what found the bar
decoder; and ib_async's own test suite is run against this engine
(`tests/ib_async_upstream/conftest.py`).

The live suites now skip on evidence rather than on absence. A quote on a
contract is what establishes that it is trading, and a position is what
establishes there is a running profit to report; reference data — providers, a
symbol search, an account summary, the scanner parameter set — is stated at any
hour and is checked rather than skipped. Two of the defects above were invisible
while those tests skipped whenever nothing arrived.

A paper account is the same session. `paper` decides one step of the logon — a
token conversion and which slot its hash occupies — and after the handshake the
market-data, trading, historical and security-definition connections are the
same code sending the same messages to the same servers. Every defect above was
found on one. What differs is that fills are simulated, so what a paper session
does not establish is a fill against real liquidity.

`.github/workflows/session.yml` runs the paper compatibility suite, the Python
suites that need a session, and `scripts/endurance.py`, after the New York
close. It is dormant until `IB_USERNAME` and `IB_PASSWORD` are set as
repository secrets: without them each job reports that it was not run, rather
than passing on a session it never opened.
