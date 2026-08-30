# Engineering notes

Detail behind [the capability matrix](capabilities.md): the per-call matrix, the wire coverage
measurements, what was measured against the protocol, and the reasoning
behind fields this client does not send. Kept for whoever works on the protocol;
the status board is the summary.

Scope: a Rust client that connects to Interactive Brokers directly, with Python bindings. No Java runtime, no desktop application, no local gateway process.

Status is assigned from evidence. `Verified` requires a passing live session phase. No status is assigned from intent.

## Status definitions

| Status | Definition |
| --- | --- |
| Verified | Implemented, unit tested, and passed against a live session |
| Implemented | Implemented and unit tested. No live session has confirmed it |
| Blocked | Implemented. The venue refuses the request and the cause is stated |
| Accepted, not served | Call exists with the expected signature, returns normally, and reports through the error callback that it cannot be served |
| Taken, not applied | Call exists with the expected signature and returns normally, and what a caller states reaches nothing. It says so where a reader will look — its own documentation — rather than on the error callback, because a program written against the reference client calls it as a matter of course and an error on every call is noise, not news |
| Absent | No such call |

## API surface

| Measure | Count |
| --- | --- |
| Canonical calls | 78 |
| Served, Rust | 77 |
| Served, Python | 77 |
| Taken and not applied, Rust | 1 |
| Taken and not applied, Python | 1 |
| Canonical callbacks | 85 |
| Calls where the two surfaces differ | 0 |
| Callbacks where the two surfaces differ | 0 |

Counted from the source by `scripts/gen_api_docs.py` and checked against this
table by `scripts/check_status_counts.py`, both of which CI runs. A figure
here that the source stops supporting fails the build rather than standing. The two surfaces carry the same calls and the same callbacks: a
program written against either finds the same thing, and a call that cannot be
served says so on both rather than being absent from one.

### Calls not served

Every call that exists with the expected signature and reports, through the
error callback, that it cannot be served. Taken from the generated coverage
matrix, which CI checks against the source.

None report through the error callback. One is taken and not applied:
`set_server_log_level`. The session holds no log level of its own and the
protocol carries no message asking the venue to change one, so what a caller
states is written to this client's log and reaches nothing else. It is counted
with the calls that are not served, because a caller who set one and expected
the venue to act on it got neither — and it says so in its own documentation
rather than on the error callback, which a program calling it as a matter of
course would hear on every call.

The two that stood here before it — the advisor configuration request and its
replacement — are not among them: they send to the venue like every other call,
and what does not happen is that their replies are read, which is a callback
nothing reaches and is counted as one on the limits page.

## Asset classes

The contract layer names 24 security types. Coverage is stated per path.

| Class | Definition | Market data | Orders | Status |
| --- | --- | --- | --- | --- |
| Equity | Verified | Verified | Verified | Available |
| Equity option | Verified | Verified | Verified | Available |
| Forex | Verified | Verified | Verified | Available |
| Future | Verified | Verified | Verified | Available |
| Futures option | Verified | Verified | Verified | Available. A lookup must name an expiry: one expiry on the index future returns ten thousand eight hundred and fifty contracts in 11.5s, and every expiry at once does not answer inside the deadline a request is given. An order must state the security type: sent under an id alone the venue answers `Unsupported type` |
| Index | Verified | Verified | Blocked, the venue supports no order on the contract type | None required |
| Bond | Verified | Verified | Verified, quantified in face value | Available |
| Warrant | Verified | Accepted, and answered with nothing | Permitted, sixty-three order types stated. Refused on the exchange and security type one attempt named, which is that pairing and not the type | Named on the venue they list on rather than a routed one: `SWB` returns four thousand four hundred and sixty-nine against a single underlying in 5.5s, where every other venue asked either ran past the deadline or held none. A quote on three of them is accepted and answered with nothing, and no reason is stated, which is the shape an account without the data for that venue is answered with. Silence is not a pass, so this stays short of verified |
| Combination | Verified | Not applicable | Verified | Available |
| Crypto and CFD | Verified | Verified | Verified, crypto requires an immediate-or-cancel or minutes instruction | Available |
| Commodity | Verified | Verified | Verified | Available |
| Bill | Verified | Verified | Verified | Available. A bill is named by its issuer, not by a ticker: `US-T` returns fifty-nine of them in 0.2s, and the first quotes. The venue's symbol search does not surface them, so the issuer is how one is found |
| Fund | Verified | Verified | Blocked, quantified in cash and then refused for residency, which is a property of the account | None required |
| Forward | Not permitted on this account | Not permitted on this account | Blocked: the call takes any contract, and the session states no permission for the type | None required. The venue lists twenty-one security types this account may trade and this is not among them, which settles the row: nothing to reach rather than something unreached |
| Venues outside the United States | Verified | Verified | Verified | Available |

## Release policy

One release: **1.0.0**. There are no incremental feature releases before it.

1.0.0 ships when every documented client call is served and the behaviour matches the vendor client, so that an existing integration can be repointed at this client without changing its code. Partial coverage is not released.

| Item | Position |
| --- | --- |
| Current code | 0.7.x, development. Tagged for traceability, not offered as a supported release |
| 1.0.0 | First supported release. Requires every documented call served, and the criteria below demonstrated |
| Coverage bar for 1.0.0 | Every call served, or removed with the reason recorded. No call accepted and left unanswered |
| Compatibility from 1.0.0 | Breaking changes require a major version |

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
| Host redirection, before any session exists | Verified. The venue retargets this client to another host and it follows: seventy-eight redirections across live sessions on 2026-08-28, each one reconnecting and going on to log on |

The messages this client reads are published in the wire coverage reference,
taken from its own dispatch tables.

## Wires this client does not send or read

The venue's protocol carries more messages than this client sends. Most of
them serve a front end's own windows and have no caller here. These are the
ones that do not, checked against this client's dispatch tables, with what
would settle each.
A wire on this list is one this client neither sends nor reads. A message
subtype it does not read is named in the log the first time the venue uses it,
once per subtype, so a session that meets one leaves a record rather than
discarding it in silence. A field on a contract definition that nothing reads is
recorded rather than logged: it is on the session, under `unread_wire`, because
one arriving is a fact about the contract and not an event worth a line.
Some inbound subtypes exist to drive a front end's own windows and mean
nothing without one. The table below carries the rest.
**What a live session actually sends.** A session that logs on, subscribes,
asks for holdings and account values, then places, modifies and cancels an
order, receives **nothing this client does not read**. Every subtype below is
one this venue does not use for this account. Each is named in the log the
first time it arrives, so the day one does, it will say so rather than
vanish.

### Which corporate actions this venue has actually stated

The protocol names six kinds and this client reads all six. Five have been
seen; one has not, and the difference is worth writing down so nobody mistakes
"handled" for "observed".

| Kind | Seen | Where |
| --- | --- | --- |
| Cash dividend | Yes, 716 | Every dividend-paying contract asked |
| Split | Yes, 2 | A ten-for-one and a two-for-one, checked against the closes either side |
| Spin-off | Yes, 7 | Six contracts, each stating a value below one |
| Stock dividend | Yes, 9 | Four Brazilian and Indian listings, values 1.03 to 1.3 |
| Rights offer | Yes, 13 | Four closed-end funds that run one as a matter of routine |
| Future rollover | No | Belongs to a future rather than a share, and the continuous
contract it would apply to is refused: asked for by that type, the venue answers
"Unsupported type" |

Thirty-two contracts were asked, across seven countries and both listed shares
and futures, over windows chosen for containing the rarer kinds. Each kind is
read against a value the venue stated rather than a made-up one: the spin-off
is the only one of the six whose value reads as the reciprocal, and a stock
dividend stating the same number the plain way round is what tells that branch
from its neighbour — 1.05 scales an earlier price by 0.9524, where a spin-off
stating 1.05 would scale it by 1.05. A rights offer states values on both sides
of one and moves the scale by neither.

The rollover has no answer behind it and cannot get one through this request:
the contract that would carry it is refused as a type. The arithmetic it would
take is the arithmetic the five seen kinds exercise.

### What a crypto order needs, and what it carries

A crypto is the one contract quoted around the clock, and the only one an order
phase can say anything about on a weekend. Three things a session settled about
it, none of which a share requires.

**A day order is refused by name.** The venue answers one:

```
The crypto buy order must be Minutes or IOC
```

Immediate-or-cancel is taken. So is the life measured in minutes, and named
alone: the venue asks for no number beside it, which is the question that had
never been put to it.

**A price off the tick grid is refused as a price**, `Invalid Price`, rather
than rounded. This client sends prices as the caller gave them, so a caller
pricing a crypto states one the venue's grid carries.

**A quantity is a fraction and stays one.** A crypto is counted in
hundred-millionths where a share is counted in hundredths, so a quantity that
survives every other asset class can still come back a hundred million times
wrong here. A thousandth of a coin was bought and sold back:

```
bought 0.001 at 78807.75
sold 0.001 at 78807.5
```

An immediate-or-cancel order takes what is at the offer and cancels the rest,
so the fill is at most what was asked for — one run filled 0.00018171 of the
thousandth asked for, which is the venue sizing the fill rather than anything
here rounding.

### What a replace states, and what it still cannot

A replace is a full statement of the order, not a difference against the
resting one. It states the order type on tag 40, the prices that type carries,
and the companions the type needs — restated from the record of the order as it
was placed, by the one function the submit uses. Two things a session settled:

**A type with no limit leg states no tag 44.** The replace stated one for every
type, so a market, on-close, market-to-limit or trailing order was replaced
with a price of zero and refused: `Invalid value in field # 44`. The tag is
stated now only where the type's own submit states one.

**A trailing stop restates its trail.** With tag 44 gone the venue read the
replace as the trailing stop it is and asked for the field that defines it:
`Message must contain field # 211`. Stating the type's companions from the
submitted record answers it, and the venue takes the replace:

```
--- Phase 193: what a replace does to a trailing stop ---
  the replace was taken and the order is still working
```

So the refusal in front of a trailing-stop modify is gone, on that answer
rather than on a reading. It is gone only for a trailing stop replaced **as
itself**: a conversion into one has no record to restate a trail from, and is
still refused.

The rest of the refusal stands, and each entry is one session away from the
same treatment: place the order, replace it, read whether the venue takes it
and whether the original is still working.

### Not built

One, and it says what a caller loses by it.

| Wire | What it carries | Why |
| --- | --- | --- |
| `6040` 10031 | Cancelling a news subscription | Not built. A session asked for news, was answered, and withdrew the request under the same id: nothing carrying that id came back. That is consistent with a request already answered having nothing outstanding, and it does not prove it — silence is what the venue did not say. What it does settle is that a caller building this would have the same silence to work with |

`6040` 10021, the cancel that pairs with the corporate-actions request, is not
sent for the same reason: the request is answered once per contract and nothing
stays open behind it. Both withdrawals carry only the id of the request they
withdraw, which is the whole of what this client would have to build.

### Callbacks the reference API defines and this client does not fire

The reference API names a fixed set of messages a client can be handed. The two
below are the ones this client cannot fire at all, which is worth stating plainly
because a program written against that API and pointed at this one would wait
on them forever rather than be told they are not coming.

| Callback | What it carries | Why not |
| --- | --- | --- |
| `rerouteMktDataReq`, `rerouteMktDepthReq` | A request id, a contract id and a venue: the answer to a market data request is "ask again, for this contract, over there" | The redirection is decided from contract data rather than stated on the wire, and this client has not established what decides it. Sending one on a rule of this client's own invention would send a caller to a contract the venue never named |
| `config` | A configuration exchange between a front end and its own process | There is no such process here. This client is the thing a front end would have been talking to |

`currentTimeInMillis` was on this list and is not any more: it reads the same
clock `currentTime` does, at whatever precision the venue stamped.

What it adds, measured rather than assumed, is currently nothing. The stamp
format carries a fraction of a second and this client reads one where the venue
states it, but across the sessions run against this venue the stamps carried no
fraction, so the answer is the other call's thousandfold. The call is served
because the reference API defines it and a caller written against that API
waits on it otherwise, not because it has been seen to carry more precision
here.

A separate list sits in the generated coverage matrix: callbacks this client
defines and nothing reaches, each because the venue does not send that shape to
this account or because there is no way to ask for it. An advisor's
configuration and its replacement, a bond's own details, a delta-neutral
validation, an order's binding, and a midpoint tick-by-tick stream — that last
because a subscription is asked for by name and the venue names three, none of
them midpoint. Those are counted as stubs there rather than restated here, so
one list is generated and the other is not a copy of it going stale.

### The account decides these

Nothing here is a gap in the code. A wire the venue does not send to this account, or one an entitlement or an advisor account gates, is answered by what the account holds and not by what is written here.

Whether any of them can be asked for was open for as long as this table existed, because "not sent" and "never asked" are not the same. The scanner pair has since been asked properly — a scan subscribed, then suspended and resumed by its own id — and is recorded below as what that returned. Most of the rest are subtypes the venue sends rather than requests a caller makes, so there is nothing to ask: a caller provokes them or does not. Two in the table are requests rather than subtypes the venue sends, and both have since been asked. The venue reads both. One is refused for the instrument, in the venue's own words; the other is refused for a firm this session has no value for. Neither is refused for anything else about the account, and both rows now say what the venue said rather than what this table assumed.

| Wire | What it carries | Why |
| --- | --- | --- |
| `35=R` | Request for quote | Reachable, read, and refused for the instrument. A session sent one and the venue answered on the execution channel, echoing the id and naming each field it wanted in turn — a side, then a quantity — and answered the complete request with "the order type QuoteRequest is invalid for this combination of exchange and security type". So the row's original reading was right and is now the venue's own words rather than an assumption: what refuses it is the instrument, not the account. Nothing is left working, because nothing is accepted |
| `6040` 10006, 10007 | Suspending and resuming a scanner | Asked and unanswered. A session subscribed a scan, suspended it and resumed it by its own id, and nothing carrying that id came back. That is consistent with a control message that never replies and with one this account may not send, and it does not tell those apart. Nor does it establish that the venue read either request: what was observed is that the bytes went out and nothing named them coming back |
| `6040` 146, 151, 208 | Trade-report records, including per-leg fills on a combination | Not sent to this account |
| `6040` 200 | Execution history | Not sent to this account |
| `6040` 109 | An advisor's allocation groups and profiles | Needs an advisor account |
| `6040` 141, 154, 175 | Combination position state and leg definitions | Not sent to this account |
| `6040` 145 | A session-level control message, sibling of the error channel | Not sent to this account |
| `6040` 188 | A newly added or linked account, and what it may do | The one of these with a consequence: a client managing linked accounts that ignores it does not learn of a new one until it reconnects. This session holds a single account and is sent none |
| `6040` 119 | Model allocation figures, per account | Needs an advisor account. It states how an advisor's model is allocated across the accounts under it, and this session holds one account that is not one |
| `6040` 148 | Which order types and algorithms each venue accepts for each security type | Refuses an order before sending it. This client lets the venue refuse, and reads what it permits at logon |
| `6040` 212 | Who decided and who executed, for European transaction reporting | Fills those fields on an order ticket. A caller states them itself, and this client carries all four of them on an order: the decision maker, the algorithm that decided, the executing trader and the algorithm that executed |
| `6040` 211 | A request for the account's own transaction-reporting configuration, which is what would tell a caller the four values rather than leaving them to state them | Reachable. A session sent one and the venue rejected it naming the field it must carry: the firm. Sent empty, it is rejected the same way, so the firm wants a value and not merely a place. This session has none to give it, and inventing one would establish that a wrong firm is refused rather than what the answer looks like |
| `6040` 258 | Which balance panels a front end should show | Nothing to trade on |
| A midpoint peg stated as two offsets | A peg to the midpoint whose offset is given as a whole-tick part and a half-tick part, rather than as one continuous number. A different order type is sent for it, and only when both parts are set and both sit on the destination's tick boundaries. A caller here states one offset, which is the other form and is sent correctly |
| `35=2` | Resending missed messages | Never observed in either direction on any of this client's connections. Implementing it would be work against a wire that never fires |

### Order options this client does not carry

The reference API lets nine of its requests carry a list of free-form
key-and-value options — market data, an order, market depth, historical data, a
scanner subscription, real-time bars, a news article, historical news and
historical ticks. The list is not free-form in practice: exactly one key is
accepted, `manual`, and its value is `0` or `1`.

An order stating any option is refused here rather than sent without it. The
one key is a statement about how the order came to be placed, and this client
has not established what carrying it changes on the wire — so it is refused
where a caller can see it, rather than accepted and dropped where they cannot.

### Read, or deliberately read past

These describe working behaviour and are here so the list of subtypes is complete. A wire read past carries something that reaches this client first by another route, and reading both would count it twice.

| Wire | What it carries | Why |
| --- | --- | --- |
| `6040` 110 | A live order's price and state, keyed by the order's own id | Arrives by another route: order state is on the execution reports this client reads. Not sent to this account either, and a full order lifecycle produced none |
| `6040` 7 | The price increments a contract trades in, pushed | Arrives by another route: the same rules are attached to a contract's details, which is where this client reads them. Not sent to this account either |
| Out-of-band `AP`, `DO`, `DP` | Holdings the broker does not hold itself: held away, shown but not held, and one set it reports apart without saying why | Read. They carry the same fields in the same tags as the account's own holdings, and are kept apart from them |
| `6040` 192, 278 | The venue's error channel | Read. Both numbers are one channel: which one it arrives under depends on a capability the session negotiated, not on the error |
| `6040` 60 | A trade record | Sent, and read past. What it reports arrives first on the execution reports this client already reads, and a fill counted from both would be counted twice |
| `6040` 18 | The venue's clock, for drift against the local one | Sent, and read past. Every message the venue sends carries the time it sent it, and that is what this client keeps and answers a caller with, so a message stating the same clock again adds nothing |

## Excluded surface

| Surface | Reason |
| --- | --- |
| Display groups and screen linkage | Client application state, not venue state |
| Order staging without transmission | Client application state. A venue side hold until activation is carried |
| Financial advisor profile screens | Client application state. Allocation on an order is in scope, see W3.3 |

## Verification

| Method | What it establishes |
| --- | --- |
| Unit tests, counted in the inventory below | The call encodes and decodes as specified |
| Live session phases against a paper account | The venue accepts the request and returns what is expected |
| A session held open under load | What only time finds: a connection that stops answering, a stream that stops arriving, a request id that answers once |
| Malformed input to every wire parser | No frame, however cut or corrupted, takes the process down |
| Continuous integration on every push | Tests, lint, documentation, and builds for Linux, macOS and Windows |

Test count is not coverage. It states what is checked, not what fraction of venue behaviour is reached.

### Keeping what the venue actually sent

A reading checked only against frames this client made up says nothing about
the ones that arrive. With `IBX_CAPTURE_WIRE` set, the connections keep what
they carried, and the capture binaries ask for the traffic worth keeping:

| Binary | What it asks for |
| --- | --- |
| `capture_depth` | A book, aggregate and on one named venue, on the same contract |
| `capture_ticks` | Every trade and quote change on a contract |
| `capture_status` | The venues that state a trading status |
| `capture_global` | Contracts and bars across nine markets |

They are development tools rather than part of the library, so they are built
only when asked for: `cargo run --features dev-tools --bin capture_depth`.

The book capture is what settled where a section names its subscription: the
tag sits three bytes into the frame, after two bytes and a marker. Read a byte
earlier — which is what this client did — a section matched no subscription and
delivered a fraction of the levels it carried. The historical query is kept as
it goes out for the same reason: a request the venue never received is
otherwise indistinguishable from one it answered into a callback nobody was
listening on.

### Measured against the published schema

Every message type here was checked field by field against a complete schema
of the protocol: 198 messages and 941 fields, each with its name and number.

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
bond and a fund are; without them a caller asking about either receives a symbol.

Sixteen remain. Six are precision and sizing figures computed locally
rather than reads, so they are not a tag to copy. One, a contract's CUSIP, has
no field of its own at all: it is taken from the list of identifiers, chosen by
which kind each one is. The rest are named by the protocol and not yet tied
to the field they fill, and are left out rather than guessed at.

An order and an execution are all but complete against that schema. A contract's
details are a quarter of it.

### How much of an order this client writes

Measured rather than asserted, by taking every field the protocol defines
writes on a new order and comparing it against every field this one writes.

| Measure | State |
| --- | --- |
| Fields the protocol defines on a new order | 79 |
| Of those, written here | 51 |
| Of those, not written here | 28, each one named or numbered below |
| Fields this client writes in total | 163 |

The twenty-eight are: a request for quote's own id, the server and client-session
ids, the basket an order belongs to, deactivation at the close, the id of an
order container, an acknowledgement flag, the manual order type, a hedge child's
parent-price flag, a model flag, and sixteen more that carry a number and no
name.

Five of them now have names, and the names settle what they are: an order
container's id, an acknowledgement flag, a hedge child's parent-price flag, a
manual order type, and a model flag. None is a field this API offers a caller —
they are working state of the sending process, written because it keeps its
blotter and its tickets on the same wire it sends orders on. That reframes the
rest of the twenty-eight rather than closing them: the measure counts every
field the protocol defines, including the ones that exist only because a
also a terminal.

A tag number does not carry one meaning across messages, and assuming it does
is how a plausible wrong reading gets shipped. The economic-value rule is stated
on a definition under the same number the execution report uses for it, and it
was read on the report and dropped on the definition; it is read on both now.
The multiplier beside it uses a number this client already reads as the
multiplier on the execution report — and on a definition that number is a
different field entirely. It stays unread until it is established rather than
inferred from the pair.

The same check run across every named attribute the protocol carries found the
unwritten ones read the same way — blotter tickets,
cross and stock-loan bookkeeping, cost reports, model rebalancing. The one
cluster among them that is a published feature is the scale order, and what is
unwritten there is a scale reporting its own progress back to the sender,
not the fields that place one. Those this client writes in full.

Writing more fields is not the same as writing the right ones: much of what
this client states rides fields composed elsewhere. The number that matters is
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
| The open/close of a delta-neutral leg | The protocol defines no field for it, so neither does this |
| Shareholder, auction strategy, basis points and their type, accrued interest on a bond, the percentage-constraint override, the auto-price override for a hedge | The protocol defines none of them. Sending a field it does not send is a guess about what the venue reads |
| The scale table, the position and fill quantity a scale starts from | Named nowhere that could be established. Two classes look like the last two by name alone, which is not evidence |
| Order origin, opt-out of smart routing, the parent's permanent id, the combo routing parameters, the miscellaneous options | Sought and not established. Each has a candidate that was rejected on inspection rather than accepted on resemblance |

A field written on a guessed tag is worse than a field not written: the first
puts something on a live order that the venue may read as something else, and
the second is visible. So these stay unsent until the tag is established.

### What the venue refuses this account

Eleven order types and times in force are refused, and the refusal is the
venue's, not this client's. Each was offered on more than one destination —
the smart route, IBKRATS and a named exchange — and on more than one security
type, a share and a future, and refused alike every time, in the venue's
words: *"invalid for this combination of exchange and security type"*.

| refused | offered on |
| --- | --- |
| Fill or kill, auction | share and future, three destinations |
| Market with protection, stop with protection | share and future |
| Mid-price, pegged to market, pegged to midpoint | share, three destinations |
| Short sale | share |
| Iceberg | share, every displayed quantity tried |

What this client writes for each is checked on the bytes, and matches what the
protocol defines, field for field: the order type character, the
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
way the protocol defines, the venue names it correctly. A refusal naming a type
the caller did not request is a fault in the request, not an answer about the
account — the pegged family works on this account, and two of its members are
accepted.

### What a skipped phase is allowed to mean

A live phase may skip, because a venue is not always in a state to answer. It may not skip on silence alone, which is the same reason a test count that cannot be read is not a passing one: a client that asks wrongly and a venue that is closed look identical from the outside, and the closed venue is the reading that keeps a suite green.

So a phase skips only against something stated. Fills and quotes are gated on a clock that knows Eastern time, daylight saving, the holidays and the early closes, and the skip names the session it saw. A rejected order skips only when the venue's words say the market or the account refused it, and fails on any other reason, printed. A historical request skips only when the venue answered with a code and a message, pacing or an entitlement, and fails on silence.

Nothing skips for contract data or account state. The venue answers for a contract's definition, its details, its trading schedule and a symbol search when every market is shut, and a session that is logged in reports its account values, its profit and loss, the position that follows a fill, and the loss of its own connection at any hour. An absence there is this client's.

---

## Test inventory

| Suite | Count | Requires credentials |
| --- | ---: | :---: |
| Rust unit and integration | 1,786 | No |
| Rust, live | 9 | Yes |
| Python | 471 | No |
| Python, live | 135 | Yes |
| Paper compatibility suite (146 phases) | 39 tests | Yes |

Counted rather than stated: `scripts/check_status_counts.py` names every test
in each suite and fails the gate when this table disagrees with it, so a figure
here cannot go quietly out of date as the suites grow.


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
suites that need a session, and `scripts/endurance.py`, an hour into the New
York session: the phases that need a market — a resting order, a book, a trade
stream — skip outside one. It is dormant until `IB_USERNAME` and `IB_PASSWORD` are set as
repository secrets: without them each job reports that it was not run, rather
than passing on a session it never opened.


## Architecture

```
    ┌──────────────────────────────────────────────┐
    │           Your code (Rust / Python)          │
    │  process_msgs() → Wrapper callbacks          │
    │  client.quote(id) → lock-free read           │
    │  client.place_order(id,c,o) → control channel│
    └─────────┬──────────────────────┬─────────────┘
              │ events               │ commands
    ┌─────────▼──────────────────────▼─────────────────┐
    │              Engine (pinned thread)              │
    │  ┌────────────────────────────────────────────┐  │
    │  │   Encryption → Auth → Compression → Decode │  │
    │  └────────────┬───────────────┬───────────────┘  │
    └───────────────┼───────────────┼──────────────────┘
               ┌────▼───┐     ┌─────▼────┐
               │ market │     │   auth   │
               │  data  │     │  orders  │
               │  feed  │     │  control │
               └────┬───┘     └────┬─────┘
                    │              │
              ──────▼──────────────▼──────
                     IB servers
```

One pinned core polls the sockets, verifies, decompresses, decodes, updates the
quote table, and drains outgoing orders, without allocating. Quotes are read
through a seqlock from any thread; everything else arrives on the callbacks.
The Python bindings run the same engine and do not hold the GIL while reading
the wire.

### One process, one session

The logon lives in your process. An account takes one logon at a time, so two
programs on one account are two logons, and the venue hands the account to
whichever connected last: the first is told it has lost the session and stops.

Several strategies inside one process share the session and cost nothing
extra: one subscription per contract on the wire, whoever asks for it. Several
*programs* need something holding the session in front of them, which is what a
gateway's local socket does and what this client, having no socket, does not.
See [#2](https://github.com/userFRM/ibx/issues/2).

Run one after another and nothing is needed: the last order id handed out is
remembered, so a later run does not reuse ids the account already holds.

## Performance

The engine runs on a pinned thread and does not allocate on the hot path:
socket poll → verify → decompress → decode → publish quote → drain outgoing
orders. Ticks are delivered in-process, without a localhost round trip, a JVM,
or a garbage collector.

Measured with `cargo run --release --features dev-tools --bin bench_replay`
and `bench_decode`,
1,000,000 iterations after 100,000 warm-up, no network I/O, on an Intel
i7-10700K with rustc 1.97:

| Path | Mean |
| --- | ---: |
| Inbound: verify → decode → state update (5-tick message) | 214 ns |
| Inbound: same, plus seqlock publish and channel send | 252 ns |
| Outbound: build + sign a 16-field limit order | 911 ns |
| Outbound: build + sign a cancel | 687 ns |
| Outbound: build + sign a modify | 939 ns |
| Message type dispatch, body extraction | 4 ns each |

These measure this engine only. No comparison against a gateway round trip is
published here: one is an in-process call and the other crosses a localhost
socket, so a ratio between them would report the socket rather than the code.

## What this client sends that the gateway does not, and the other way round

Read tag by tag against the protocol's own requirements for a new order, a
replace, a cancel, the logon and the P&L request. Ninety-six tags were
compared; sixty-seven match outright. What follows
is every difference, and why it stands.

| Tag | This client | The protocol | Standing because |
|---|---|---|---|
| 60 | sent on every order | no writer found | The venue accepts it and every order this client has placed carried it. |
| 204 | sent as `0` | no writer found | As above. |
| 200 / 541 | 541 for a maturity of eight characters, 200 for six | the new-order encoder writes 200 for options and futures alike and never writes 541 | An option order carrying 541 was placed and accepted against a live session, and the order also names the contract by id on 6008, which identifies it exactly. |
| 6122 | sent as `c` on a replace | an option account attribute the replace encoder does not write | The replace the venue accepts is the one that carries it. |
| 6398, 6399 | not sent | the logon writes both unconditionally | What they carry has not been resolved, and the logon succeeds without them. |
| 6156, 6958 | not sent | the new-order encoder writes both unconditionally, as booleans | Both describe the sending processrpart's own handling of the order rather than anything the caller states: 6156 is whether it belongs to a basket, and neither appears in the attribute registry that backs the caller-facing fields. There is nothing here to state, and stating a zero would invent one. |
| 9801 | was `1`, now `Y` | its block-order attribute writes the character `Y`, and writes nothing when the flag is off | Corrected. The generic boolean writer states a one; this attribute overrides it. |
| 9816 | the caller's value as it stands | the new-order encoder multiplies its own value by a hundred | The wire carries the percentage. This client holds the percentage, so it passes through; a doc comment here called it a decimal, which would have made every volatility order a hundredth of what was asked. |
| 100, 6210 | sent on a replace | the replace encoder states no destination | The replace is accepted as it stands. |

Each of these is a difference this client can defend, not one it was unaware of.
Where a protocol reading and a live session disagree, the live session decides: a
value the venue accepts is not proof it was read as intended, but a value the
venue refuses is proof it was not.

## A future states its maturity on the tag that carries it

A month stated as a month is the contract month and rides tag 200; a full date
is a date and rides tag 541. Futures were the one kind that took the first six
characters of whatever it was given and sent that as the month.

That is right only when a contract stops trading in the month it is named for.
`CLZ6` is the December contract and stops trading on the twentieth of November,
so the month sent named a contract that does not exist, and the venue refused the
order with "contract does not match supplied contract parameters". Energy futures
expire in the month before the one they are named for as a rule, so this was an
asset class rather than a contract.

Against a live session, before and after:

| Contract | Last trade date | Local symbol | Before | After |
|---|---|---|---|---|
| ES on CME | 20261218 | ESZ6 | margin 28,017.36 | margin 27,999.30 |
| GC on COMEX | 20261229 | GCZ6 | margin 34,244.70 | margin 34,288.85 |
| CL on NYMEX | 20261120 | CLZ6 | refused | margin 12,643.29 |

Every kind now follows the rule the rest already followed.

## A subscription states the contract, because the id alone is not answered

The rule this client holds to is that a value carried by the wire is not
hardcoded here. Market data is where that rule meets something firmer: the venue
will not answer a subscription that names only a contract id.

The same contract, at the same moment, asked both ways:

| Named by | Answer |
|---|---|
| id, security type, exchange and currency | quoted |
| id alone | silence |

So a security type and an exchange must be stated. What is stated should be the
contract's own, and the last resort in the subscribe path describes a US stock,
which means an undescribed contract of any other kind is asked about as one.
That fallback stands because removing it stops market data outright; what
removes the guess is the description reaching the subscribe, not the fallback
going away.

Removed from the historical and histogram paths, where nothing needs them: an
empty exchange became `BEST` and an empty security type became `CS`. The
contract id names the contract there, and those queries carry it.
