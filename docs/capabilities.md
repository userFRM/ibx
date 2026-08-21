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

| | |
| --- | --- |
| Requests | 77. Every one either does what it says or reports why it cannot — none returns success having sent nothing |
| Order fields | 154. 114 are sent; 35 have no field in the protocol to carry them and the call says so rather than dropping them; 5 are what the venue fills on the way back, which an order does not carry out |
| Rust and Python | the same request produces the same call on both, compared against live responses |
| Tests | 2,258 offline, and 166 more that only run against a broker session |

45 of the 46 capabilities are verified against IBKR production servers; the
remaining one, advisor configuration, reaches the server and needs an advisor
account to see what it answers.

Every figure above is measured on each commit, and the build fails if one moves.

---

## Client surfaces

| Surface | Status | Verification |
| --- | :---: | --- |
| `EClient` / `EWrapper` (TWS API shape) | ✅ Supported | `tests/ib_paper_compat`, `tests/python/test_compat_tier1..3.py` |
| `ib_async`, unmodified | ✅ Supported | Their `IB` on this engine via `ibx.ib_async.attach`; their events, async variants and types, with no gateway. All 67 transport calls their library makes are carried, measured against their own source and gated. Their own test suite runs against it, and all three behave as they do against a gateway. Two pass. The third asserts a `RequestError` carrying code 321, which their own wrapper cannot raise: it lists 321 among the codes it treats as warnings, and a warning never ends the request it belongs to. That test fails the same way against any server, this engine or a gateway, and their source carries an open note about the same code. `tests/python/test_ib_async_transport.py`, `tests/ib_async_upstream/conftest.py` |
| `ibx.IB` (ib_async shape) | ✅ Supported | 90/90 methods present; `tests/python/test_ib_facade.py`, `scripts/sdk_sweep.py` |
| `ibx::Client` (Rust) | ✅ Supported | 77/77 callable; 3 return an error naming a local-process facility this client does not have |
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
| 23 order types | ✅ Supported | `whatIf` preview accepted by the server for each; `tests/ib_paper_compat`. A preview states the order's type as one byte, and eleven types have one — the rest are previewed as a limit at the same price, so the margin comes back for a limit. Placing is unaffected; only the preview is |
| Order fields | ✅ Supported | An order has 154 fields. 114 are sent. 35 have no field in this protocol to carry them, and each says so on itself rather than being quietly ignored. 5 more are what the venue fills on the way back, which an order does not carry out. A check on every commit fails if a field starts being dropped |
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

Three properties are measured on every commit, and the build fails if one
stops holding.

| What is guaranteed | Where it stands |
| --- | --- |
| A call never returns success having sent nothing | 77 requests, none silent |
| A field a caller sets is never quietly ignored | 154 order fields, none dropped |
| A field the server sends is never thrown away | What this client has no name for is kept under its tag number — 49 such fields on an equity definition, 46 on a bond |

A fourth is held by a test rather than a measurement: **no wire parser aborts
on malformed input.** Every parser is given each prefix of a well-formed frame,
that frame with a byte replaced at each position, and runs that are not frames
at all (`tests/malformed_input.rs`).

## Protocol constraints

These are properties of the IBKR protocol, not of this implementation. The
official gateway behaves the same way.

- **35 order fields are not transmitted.** For each, the reference client
  either declares no tag or declares one the server rejects by name
  (`algo_id` → *Invalid value in field # 8016*; `scale_init_fill_qty` →
  *Can not contain field # 6486*). Each field retains the caller's value so an
  order constructed against another client round-trips unchanged.
- **One market-data subscription per contract on the wire.** Multiple callers
  are multiplexed client-side, as the gateway multiplexes across windows.
- **A caller's request id is not what the venue is asked under.** Every
  subscription is asked for under an id this client allocates and is mapped
  back to the caller who wanted it. The venue echoes an id back, so one taken
  from the caller cannot be told apart from one allocated here. The venue
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
| A logon later than this one is another client, and keeps the account | ✅ Supported | A reconnect that finds one reports it and stops; retrying cannot change it. Both times are read from the venue's clock, so two machines' clocks cannot decide it |
| A logon at or before this one is this session's own, still being reaped | ✅ Supported | The reconnect completes over it, which is what an ordinary recovery is |
| The heartbeat is the interval the venue answered with | ✅ Supported | The interval a logon proposes is not what it is held to; the answer is read from the logon response and applied on every reconnect |
| A reconnect follows the venue | ✅ Supported | It uses the hosts this session reached the venue through, on the port the venue named in its redirect, and stops walking hosts when one answers and refuses |
| The first connect knocks on the next door when one does not answer | ✅ Supported | One host per region. A door that answers and refuses ends the walk, so a refused logon is not repeated at every door |
| The last order id is kept between runs | ✅ Supported | An order id belongs to the account, not the process: an id it has already used is refused by name. The last one handed out is remembered per account, kind of session and client id, and the next run counts on from it |
| A session survives losing its connection | ✅ Supported | A dropped connection is rebuilt on the session already open, with no second factor: five forced drops recovered in 2-8s, and an eight hour session rode through its losses unattended |
| A session does not survive its process | ✅ Documented | The venue holds a session for a socket, not for an account: killed without logging out, it was already gone forty seconds later, and a later start is answered with a handshake. The venue stores no session either — it stays connected rather than restarting. So a restart costs a second factor here exactly as it does there, and staying up costs none |
| A session that has ended answers at once | ✅ Supported | Requests made after a terminal loss are refused with 504 immediately, rather than waiting out a timeout each. Every request already answered keeps the venue's answer |

One session, held open for 175 minutes across a market open: 106,053 quotes,
180,433 trades, 95,985 book rows and 4,148 bars, with no unrequested
disconnect and no error other than the venue's answer for a series it does
not hold. `scripts/endurance.py --minutes 175`.

## Architectural differences from a gateway process

| Gateway | This client |
| --- | --- |
| Configuration file and settings window | Settings on the client, read back at runtime. 7 gateway settings have no counterpart: no window geometry, no local listening socket, no JVM heap |
| Local socket for client programs | The client is in-process; there is no socket to connect to, authorise, or keep running |
| Java runtime | None |

## Known limitations

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
  of it would be found rather than invented. The venue reads the same
  three, keeps the same number, and no code path there reads it either.

## Refusals

A request this client will not send is reported through `error(reqId, code,
message)` and the call returns, as the reference client does. The number is the
one that client reports for the same class: 321 for a request that fails
validation, 200 for a contract description that matches nothing, 504 for a call
with no session, and 327 for binding orders entered elsewhere, which that
client refuses for any client but the one they bind to. Construction and
configuration raise, as does a synchronous call with a return value.

## Calls

| Category | Calls |
| --- | --- |
| **Connection** | `connect`, `disconnect`, `is_connected`, `run`, `get_account_id` |
| **Market data** | `req_mkt_data`, `cancel_mkt_data`, `req_tick_by_tick_data`, `cancel_tick_by_tick_data`, `req_mkt_depth`, `cancel_mkt_depth`, `req_market_data_type` |
| **Orders** | `place_order`, `cancel_order`, `req_global_cancel`, `req_ids`, `req_open_orders`, `req_all_open_orders`, `req_auto_open_orders`, `req_completed_orders`, `req_executions` |
| **Account** | `req_positions`, `cancel_positions`, `req_positions_multi`, `cancel_positions_multi`, `req_account_summary`, `cancel_account_summary`, `req_account_updates`, `req_account_updates_multi`, `cancel_account_updates_multi`, `req_pnl`, `cancel_pnl`, `req_pnl_single`, `cancel_pnl_single`, `req_managed_accts` |
| **Historical** | `req_historical_data`, `cancel_historical_data`, `req_head_time_stamp`, `cancel_head_time_stamp`, `req_historical_ticks`, `req_historical_schedule`, `req_real_time_bars`, `cancel_real_time_bars`, `req_histogram_data`, `cancel_histogram_data` |
| **Reference** | `req_contract_details`, `req_matching_symbols`, `req_sec_def_opt_params`, `req_mkt_depth_exchanges`, `req_market_rule`, `req_smart_components` |
| **Scanner** | `req_scanner_parameters`, `req_scanner_subscription`, `cancel_scanner_subscription` |
| **News** | `req_news_providers`, `req_news_article`, `req_historical_news`, `req_news_bulletins`, `cancel_news_bulletins` |
| **Fundamental** | `req_fundamental_data`, `cancel_fundamental_data` |
| **Options** | `calculate_implied_volatility`, `cancel_calculate_implied_volatility`, `calculate_option_price`, `cancel_calculate_option_price`, `exercise_options` |
| **Other** | `req_current_time`, `req_user_info`, `req_family_codes`, `req_soft_dollar_tiers`, `set_server_log_level`, `req_wsh_meta_data`, `req_wsh_event_data`, `query_display_groups`, `subscribe_to_group_events` |

**Order types.** MKT, LMT, STP, STP LMT, TRAIL, TRAIL LIMIT, MOC, LOC, MTL,
MIT, LIT, MKT PRT, STP PRT, REL, PEG MKT, PEG MID, MIDPRICE, SNAP MKT,
SNAP MID, SNAP PRI, BOX TOP. Algos: VWAP, TWAP, Arrival Price, Close Price,
Dark Ice, PctVol. Conditions: price, volume, percent change, margin, execution
and time. Brackets, one-cancels-all, and combinations with a price per leg.

**Settings.** The gateway's configuration file is replaced by settings on the
client: announced build, time zone, message pacing, execution-report scope, and
others — 17 in total, readable at runtime. Seven gateway settings have no
counterpart and report why (no window geometry, no local listening socket, no
JVM heap). Rust: `EClientConfig.gateway`. Python: `ibx.configure()`.
