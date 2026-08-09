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
| Wall Street Horizon event data | Accepted, not served | Has a path to the venue; a separately subscribed data product, and this session holds no subscription | W3 |
| Financial advisor allocation | Accepted, not served | The venue carries the request; exercising it needs an advisor account | W3 |
| Tick by tick data | Blocked | The feed rides a service of its own. This session is sent no list of the services it may reach and a request for that list is refused | W2 |

## API surface

| Measure | Count |
| --- | --- |
| Canonical calls | 77 |
| Served, Rust | 69 |
| Served, Python | 69 |
| Accepted and not served, Rust | 8 |
| Accepted and not served, Python | 8 |
| Canonical callbacks | 81 |
| Calls where the two surfaces differ | 0 |
| Callbacks where the two surfaces differ | 0 |

Counted from the source by `scripts/gen_api_docs.py`, which CI re-runs and
compares. The two surfaces carry the same calls and the same callbacks: a
program written against either finds the same thing, and a call that cannot be
served says so on both rather than being absent from one.

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
| W3.3 | Financial advisor allocation | Allocation groups and methods carried, or the reason recorded | Not wired, and the reason is recorded rather than the surface removed. The venue carries the request, so it is buildable; exercising it needs an advisor account, and this one is not. The calls report exactly that |
| W3.4 | Event data | Wall Street Horizon calls served, or the reason recorded | Not wired, and the reason is recorded rather than the surface removed. The event calendar has a path to the venue; it is a separately subscribed data product and this session has no subscription to exercise it against. The calls report exactly that |
| W3.5 | Compatibility statement | Every call published with its status and the evidence establishing it | Met. The coverage matrix carries, per call, how its status was established: exercised against a live session, exercised by the offline suites, stating why it cannot be served, or exercised by neither. Derived from the suites themselves, so it cannot go quietly out of date |
| W3.6 | Second factor | Approval path covered by an automated live check, or the reason no such check can run recorded | Met against a live session, and not automated, because an approval a person must give is the thing a second factor is for. The paper session used for the rest of the verification is never presented with one, so the path was exercised where it does appear: the venue asked for the device factor, the connect call held, the approval was given on the device, and the gate released and completed the login. Read only — the run asked for the next order id and disconnected. The wire and the gate are covered by fifteen tests besides |

## Wire coverage

A claim to replace the gateway is a claim about messages. This one is checked
rather than asserted: every message the venue sends that nothing here reads is
recorded, and a live test exercises everything a caller can ask for and then
asks what arrived unread.

| Measure | State |
| --- | --- |
| A live session's messages read | Every one. Verified with quotes, depth, holdings, account values, and an order through placing, modifying and cancelling |
| Trading connection | Every type and subtype read, or recorded with why it carries nothing a caller could use |
| Market data connection | Every type read. A venue refusing to show its book now reaches the caller that asked |
| Historical connection | Records anything it does not read |
| The venue's fourth connection | Not needed. It carries a topic-keyed message bus for news search, notifications and window layouts, and no contract or reference data |
| Host redirection, before any session exists | Implemented. The venue can retarget this client to another host and it follows |

The messages this client reads are published in the wire coverage reference,
taken from its own dispatch tables.

## Wires not implemented

The venue's protocol carries more messages than this client sends. Most of
them serve a front end's own windows and have no caller here. These are the
ones that do not, checked against this client's dispatch tables, with what
would settle each.

A wire on this list is one this client neither sends nor reads. Each is named
in the log the first time the venue uses it, so a session that meets one leaves
a record rather than discarding it in silence.

Some inbound subtypes exist to drive a front end's own windows and mean
nothing without one. The table below carries the rest.

**What a live session actually sends.** A session that logs on, subscribes,
asks for holdings and account values, then places, modifies and cancels an
order, receives **nothing this client does not read**. Every subtype below is
one this venue does not use for this account. Each is named in the log the
first time it arrives, so the day one does, it will say so rather than
vanish.

| Wire | What it carries | Why it is not implemented |
| --- | --- | --- |
| Out-of-band `AP`, `DO`, `DP` | Holdings the broker does not hold itself: held away, shown but not held, and one set it reports apart without saying why | Read. They carry the same fields in the same tags as the account's own holdings, and are kept apart from them |


| `6040` 192, 278 | The venue's own error channel | Read. Both numbers are one channel: which one it arrives under depends on a capability the session negotiated, not on the error |
| `35=R` | Request for quote | Instruments that accept an RFQ, which this account does not hold |
| `6040` 10006, 10007 | Suspending and resuming a scanner | Needs a scanner entitlement |
| `6040` 10020, 10021 | Contract adjustments, for splits and dividends | Not yet built. Historical prices are unadjusted without it |
| `6040` 10031 | Cancelling a news subscription | Not yet built. Historical news is answered once, so there is nothing outstanding for a caller to withdraw |
| `6040` 110 | A live order's price and state, keyed by the order's own id | Not sent to this account: a full order lifecycle — placed, modified, cancelled — produced none. Order state arrives on execution reports here |
| `6040` 7 | The price increments a contract trades in, pushed | Not sent to this account. The same rules arrive attached to a contract's details, which is where this client reads them |
| `6040` 60, 146, 151, 208 | Execution and trade-report records, including per-leg fills on a combination | Not sent to this account. Fills arrive on execution reports here |
| `6040` 200 | Execution history | Not sent to this account |
| `6040` 109 | An advisor's allocation groups and profiles | Needs an advisor account |
| `6040` 141, 154, 175 | Combination position state and leg definitions | Not sent to this account |
| `6040` 145 | A session-level control message, sibling of the error channel | Not sent to this account |
| `6040` 18 | The venue's own clock, for drift against ours | Not sent to this account |
| `6040` 188 | A newly added or linked account, and what it may do | The one of these with a consequence: a client managing linked accounts that ignores it does not learn of a new one until it reconnects. This session holds a single account and is sent none |
| `6040` 119 | Model allocation figures, per account | Answers a request for them, which this client does not send |
| `6040` 148 | Which order types and algorithms each venue accepts for each security type | Refuses an order before sending it. This client lets the venue refuse, and reads what it permits at logon |
| `6040` 212 | Who decided and who executed, for European transaction reporting | Fills those fields on an order ticket. A caller states them itself |
| `6040` 258 | Which balance panels a front end should show | Nothing to trade on |
| A midpoint peg stated as two offsets | A peg to the midpoint whose offset is given as a whole-tick part and a half-tick part, rather than as one continuous number. The counterpart sends a different order type for it, and only when both parts are set and both sit on the destination's tick boundaries. A caller here states one offset, which is the other form and is sent correctly |
| `35=2` | Resending missed messages | Never observed in either direction on any of this client's connections. Implementing it would be work against a wire that never fires |

## Excluded surface

| Surface | Reason |
| --- | --- |
| Display groups and screen linkage | Client application state, not venue state |
| Order staging without transmission | Client application state. A venue side hold until activation is carried |
| Financial advisor profile screens | Client application state. Allocation on an order is in scope, see W3.3 |

## Verification

| Method | What it establishes |
| --- | --- |
| Unit tests, 1195 engine and client, 310 Python bindings | The call encodes and decodes as specified |
| Live session phases against a paper account | The venue accepts the request and returns what is expected |
| Continuous integration on every push | Tests, lint, documentation, and builds for Linux, macOS and Windows |

Test count is not coverage. It states what is checked, not what fraction of venue behaviour is reached.

### Measured against the counterpart's own published schema

The counterpart carries a complete, named description of every message it
exchanges — 198 messages and 941 fields, with the real name and number of each.
Every type here was compared against it.

| Type | Fields it names | Carried here |
| --- | --- | --- |
| An order | 150 | 149. The one missing states a hedge's largest size, and the field it rides is not established |
| An execution | 19 published | all of them |
| A contract | 21 | all |
| A combination leg | 9 | 7 |
| A contract's details | 64 | 48 |

A bond now states its terms — what it pays, how, when it can be called or put,
whether it converts, what it is rated — and a fund states what it charges, what
it is closed to and where it may be sold. Those two blocks are most of what a
bond and a fund are, and a caller asking about either used to receive a symbol.

Sixteen remain. Six are precision and sizing figures the counterpart computes
rather than reads, so they are not a tag to copy. One, a contract's CUSIP, has
no field of its own at all: it is taken from the list of identifiers, chosen by
which kind each one is. The rest are named in the counterpart and not yet tied
to the field they fill, and are left out rather than guessed at.

An order and an execution are all but complete against that schema. A contract's
details are a quarter of it.

### Whether the reference this was measured against is current

It is. A build four releases newer was fetched and compared: the encoder that
builds an order gained two fields, one of which this client already writes, and
lost none. The parser that reads an execution report is identical — the same
number of fields, none added, none removed.

So the protocol did not move, and a measurement taken against the older build
stands for the newer one.

### How much of an order this client writes

Measured rather than asserted, by taking every field the counterpart's encoder
writes on a new order and comparing it against every field this one writes.

| Measure | State |
| --- | --- |
| Fields the counterpart's encoder writes on a new order | 79 |
| Of those, written here | 51 |
| Of those, not written here | 28, each one named or numbered below |
| Fields this client writes in total | 163 |

The twenty-eight are: a request for quote's own id, the server and client-session
ids, the basket an order belongs to, deactivation at the close, the id of an
order container, an acknowledgement flag, the manual order type, a hedge child's
parent-price flag, a model flag, and sixteen more that are numbered in the
counterpart and named nowhere.

Five of them now have names, and the names settle what they are: an order
container's id, an acknowledgement flag, a hedge child's parent-price flag, a
manual order type, and a model flag. None is a field this API offers a caller —
they are the counterpart's own working state, written because it keeps its
blotter and its tickets on the same wire it sends orders on. That reframes the
rest of the twenty-eight rather than closing them: the measure counts every
field the counterpart writes, including the ones that exist only because it is
also a terminal.

A tag number does not carry one meaning across messages, and assuming it does
is how a plausible wrong reading gets shipped. The economic-value rule is stated
on a definition under the same number the execution report uses for it, and it
was read on the report and dropped on the definition; it is read on both now.
The multiplier beside it uses a number this client already reads as the
multiplier on the execution report — and on a definition that number is a
different field entirely. It stays unread until it is established rather than
inferred from the pair.

The same check run across every named attribute the counterpart carries found
162 this client does not write, and they read the same way — blotter tickets,
cross and stock-loan bookkeeping, cost reports, model rebalancing. The one
cluster among them that is a published feature is the scale order, and what is
unwritten there is the counterpart reporting a scale's progress back to itself,
not the fields that place one. Those this client writes in full.

Writing more than the counterpart is not the same as writing what it writes:
this client states 163 fields against its 79 because much of what it states
rides fields the counterpart writes from elsewhere. The number that matters is
the fifty-one, and the twenty-eight beside it.

### Data this client states without the venue having stated it

| What | Why it is written here | What depends on it |
| --- | --- | --- |
| The single letter each venue is known by on a quote | The venue names venues in full and states no abbreviation | Which venue a quote's exchange letter refers to |

The venues SMART routes to, and the order whose positions a quote's exchange
bitmask refers to, were on this list until the venue was asked: it states them
on the contract's own definition, and the order it states bore no resemblance to
the one being used. Read from the definition now.

### Fields of a contract still unread

Asked the venue about a share, a share listed outside the United States, an
index, a bond, a fund, an option and a future, and kept every field each reply
carried. What the reference client publishes and this does not:

| Field | State |
| --- | --- |
| `evMultiplier` | Unsettled. The number carrying it on an execution report is a different field on a definition, so the pair is not evidence |
| `sizeIncrement`, `suggestedSizeIncrement` | A rule states an increment per price band and again per size band under the same numbers, and the size bands are not read |
| `marketRuleIds` | The per-venue list is not stated as a list; whether it can be assembled from the rules is not settled |

Everything else the reference client publishes is carried. The rest of what a
definition states — fifty-one stated fields on a share, forty-six on a bond — is
kept under its tag number whether or not it has a name here, so a field without
a name is still reachable.

### Order fields this client deliberately does not send

Every field on the order this API takes was checked against what reaches the
wire. Most of what was reaching nothing is carried now. These are the ones that
are not, and are not gaps:

| field | why |
| --- | --- |
| The open/close of a delta-neutral leg | The counterpart's handler for it is empty: it writes nothing, so neither does this |
| Shareholder, auction strategy, basis points and their type, accrued interest on a bond, the percentage-constraint override, the auto-price override for a hedge | Nothing in the counterpart writes any of them. Sending a field it does not send is a guess about what the venue reads |
| The scale table, the position and fill quantity a scale starts from | Named nowhere that could be established. Two classes look like the last two by name alone, which is not evidence |
| Order origin, opt-out of smart routing, the parent's permanent id, the combo routing parameters, the miscellaneous options | Sought and not established. Each has a candidate that was rejected on inspection rather than accepted on resemblance |

A field written on a guessed tag is worse than a field not written: the first
puts something on a live order that the venue may read as something else, and
the second is visible. So these stay unsent until the tag is established.

### What the venue refuses this account

Eleven order types and times in force are refused, and the refusal is the
venue's, not this client's. Each was offered on more than one destination —
the smart route, IBKRATS and a named exchange — and on more than one security
type, a share and a future, and refused alike every time, in the venue's own
words: *"invalid for this combination of exchange and security type"*.

| refused | offered on |
| --- | --- |
| Fill or kill, auction | share and future, three destinations |
| Market with protection, stop with protection | share and future |
| Mid-price, pegged to market, pegged to midpoint | share, three destinations |
| Short sale | share |
| Iceberg | share, every displayed quantity tried |

What this client writes for each is checked on the bytes, and matches what the
counterpart client writes, field for field: the order type character, the
execution instruction beside it, the time in force, the displayed quantity, the
side and the locate flag. So a caller sending one of these receives the venue's
refusal because the venue refuses it, not because it was asked wrongly.

Some of these are the simulated account rather than the venue. The broker
documents that a paper account does not support auction orders, pegged to
market, requests for quote or VWAP, and that its fills are simulated from the
top of the book. Three of the refusals above are named in that list, so they
are what a paper account answers and say nothing about what a funded one would.
Verifying them needs an account that is not simulated.

That distinction had to be earned. Two of these were recorded here as refusals
and were not: an order asking to peg went out under a type the venue uses for
something else, so it was refused under a name nobody had asked for. Asked the
way the counterpart asks, the venue names it correctly. A refusal naming a type
the caller did not request is a fault in the request, not an answer about the
account — the pegged family works on this account, and two of its members are
accepted.

### What a skipped phase is allowed to mean

A live phase may skip, because a venue is not always in a state to answer. It may not skip on silence alone, which is the same reason a test count that cannot be read is not a passing one: a client that asks wrongly and a venue that is closed look identical from the outside, and the closed venue is the reading that keeps a suite green.

So a phase skips only against something stated. Fills and quotes are gated on a clock that knows Eastern time, daylight saving, the holidays and the early closes, and the skip names the session it saw. A rejected order skips only when the venue's own words say the market or the account refused it, and fails on any other reason, printed. A historical request skips only when the venue answered with a code and a message, pacing or an entitlement, and fails on silence.

Nothing skips for contract data or account state. The venue answers for a contract's definition, its details, its trading schedule and a symbol search when every market is shut, and a session that is logged in reports its account values, its profit and loss, the position that follows a fill, and the loss of its own connection at any hour. An absence there is this client's.
