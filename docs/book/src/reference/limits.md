# Limits

What this client does not do, and what it does differently enough that a
program relying on the other behaviour will be wrong.

Nothing here is a call that returns as though it acted. A call this protocol
cannot carry reports why. The list below is the part a caller has to know
before writing against it.

## Callbacks nothing fires

Six callbacks exist so a program written against the reference client compiles
and runs. No message reaches any of them.

| Callback | Why |
| --- | --- |
| `bond_contract_details` | The venue answers a bond on `contract_details`, like everything else. Read the bond there |
| `receive_fa` | The advisor configuration request reaches the venue; its reply is not parsed |
| `replace_fa_end` | As above, for the replacement |
| `order_bound` | The permanent id an order was given arrives on the order's status and on its fills, so there is no separate message to fire this on |
| `delta_neutral_validation` | Nothing on this client's connections produces it |
| `tick_by_tick_mid_point` | A tick-by-tick stream is asked for by name and the venue names three: all-last, last and bid-ask. There is no name for a midpoint stream to ask under, and a midpoint record arriving anyway is kept as a frame nothing reads |

## Calls whose answer is not read

`request_fa` and `replace_fa` send. The venue's reply lands among the messages
this client records as unread, and reading it needs an advisor account to state
the reply's shape. Allocation on an individual order is a different thing and is
carried.

Every other call in the reference client's surface is served on both languages.
The call-by-call matrix is [generated from the source](./coverage.md).

## Executions and fills are this session's

`fills()`, `executions()` and `reqExecutions()` answer with what **this session**
has seen.

The wrapper this follows asks the venue and is answered with the account's
executions, whoever made them. This protocol carries no such question. The venue
names executions only alongside the orders it replays when a session opens, so a
fill made before this session opened, on an order that has since completed, is
not among them.

An empty answer means this session has seen no fills. It does not mean the
account has none. A program that reconciles against an account's full execution
history needs another source for it.

## A bid of -1 is the venue saying there is none

Some instruments have no bid and no ask. A calculated index is the clearest
case: it is published as a level, not quoted by anyone. The venue states that
by sending `-1` on the bid and the ask, and this client passes it on as sent
rather than turning it into a zero or dropping it.

Told apart by what comes with it:

* **No bid or ask exists.** `-1` on both, and no bid or ask *size* at all,
  while the last, high, low, close and open arrive normally. `VIX` on `CBOE`
  reads this way; `SPX`, which does carry a quote, reads normally.
* **Nothing is flowing.** No ticks at all, and no error — a listing whose
  market is closed, which is what a Tokyo or Sydney listing gives during the
  American session.

So `-1` is a stated absence and silence is an unstated one. Neither is an error,
and neither means the subscription failed.

## Market depth depends on the entitlement

A book is asked for at a named venue, and every level names the venue it came
from. Which venues answer is the account's entitlement, not this client's:

* A venue the account is not entitled to refuses by name, and the refusal
  reaches the caller.
* A book asked for on no particular venue is acknowledged and then produces
  nothing, which is what an account with no aggregate entitlement is answered
  with. It does not error.

Check what came back rather than assuming a subscription that was accepted is a
subscription that will deliver.

## 4,096 instruments at a time

The engine holds a slot for 4,096 distinct contracts concurrently. Registering
one past that is refused with a message naming the limit.

Concurrent, not cumulative: cancelling a market-data subscription frees its slot
and the slot is reused. A long-running process that subscribes and never cancels
will reach it; one that cancels what it is done with will not.

The number is this client's own allocation, not a limit the venue states. One
option chain asked for at once is 282 live subscriptions on a single
underlying, and the venue served all of them.

## A broad lookup takes longer than one contract

A lookup naming a whole class is a different question from one naming a single
contract. `SPY` options across every expiry is 13,580 definitions over nineteen
exchanges, and the venue takes about ten seconds to send the first of them.

The wait measures silence, not the length of an answer: it is reset each time
the venue speaks, so an answer that runs for as long as that one does is not cut
off part-way through. A lookup that does run out says how many definitions
arrived before it did, since a partial answer and no answer are different facts
and only the first says to ask a narrower question — by naming an expiry, or a
single venue.

## Two orders a modify cannot restate

A modify is a full statement of the order, rebuilt from what was placed. Two
kinds cannot be stated that way and the call is refused rather than sent:

| Order | Why |
| --- | --- |
| An adjustable stop | It is an ordinary stop defined by the conversion it carries, and no session has established that a replace keeps it |
| A what-if preview | It is a margin preview, not a resting order, so there is nothing on the book for a replace to act on |

Everything else is replaced as itself, including the ones that carry more than
a type and a price: hidden, all-or-none, iceberg, discretionary, sweep-to-fill,
an OCA group, a good-till date, a bracket child, an algo, a conditional order,
and the trailing, pegged, midpoint and limit-if-touched types.

A relative order is refused a modify as well. It answers both ways: sometimes
the venue takes the replace and the order goes on working, and sometimes
neither the replace nor a withdrawal after it draws any answer at all. A modify
that strands the order some of the time is worse than one that is refused.

## What a crypto order needs

A crypto is quoted around the clock and priced and sized differently from a
share.

| | |
| --- | --- |
| Time in force | Immediate-or-cancel, or the one measured in minutes. A day order is refused: *"The crypto buy order must be Minutes or IOC"* |
| Price | On the venue's grid. One that is not is refused as a price, *"Invalid Price"*, rather than rounded — this client sends prices as they were given |
| Quantity | A fraction, counted in hundred-millionths. A thousandth of a coin is an ordinary size |

## Order fields the protocol has nowhere to put

An order carries 154 fields. 114 go out under a tag. 35 have no field in this
protocol to carry them, and each says so on itself rather than being quietly
dropped. 5 more are what the venue fills in on the way back, which an order
being placed does not carry out.

None is silently dropped, and that is checked rather than claimed:
`python scripts/gen_order_field_reach.py` recounts all four figures from the
order builders and exits non-zero if any field becomes settable and unread.

## What a margin preview actually previews

A preview states the order's type as a single byte, and not every order type has
one. Those that do not are previewed as a limit at the same price, so the margin
that comes back is a limit's.

Placing is unaffected. Only the preview is. Which types have their own byte is
recorded in
[capabilities.md](https://github.com/userFRM/ibx/blob/main/docs/capabilities.md),
against the session it was measured on.

## Implied volatility and option price

The venue computes its option model and publishes it per option, on a
subscription of its own. Volatility and greeks read off a quote are the venue's
own numbers.

What this protocol carries no request for is the inversion: an option price or a
volatility the caller supplies, for the venue to work back from. That one is
solved here, against the venue's published model for the same contract.

Where the venue has published no model, nothing is answered — rather than a
number derived from a rate nobody stated. Asked before the model has arrived,
the question waits on the subscription that asking opens.

## Things an entitlement decides, not this client

* **News headlines** need a news subscription. Without one, the providers list
  is returned and every query comes back empty.
* **Corporate events content** needs a Wall Street Horizon subscription. The
  calendar's schema and event types are delivered either way; the events
  themselves come back empty without it.

## A reconnect already under way outlives the call that stops it

`disconnect` stops the engine and returns. An attempt to reopen a connection
that was already in flight when it did is not stopped: it runs to its own end
on a thread of its own.

Nothing it opens is used. The engine refuses to install a connection that
arrives after the stop — installed, it would be a freshly authenticated session
at the venue opened after the caller was told the engine had stopped — and the
socket closes when the engine goes.

What a caller can see is a second-factor prompt arriving on their phone after
`disconnect` returned, for a login nothing goes on to use. Stopping the attempt
itself means interrupting a handshake in the middle of a blocking read, which
this client does not do.

## What authenticates a farm connection

A market-data or trading connection is opened with a request carrying the
session's own token, and the venue may answer it in one of two ways: by asking
this session to authenticate in full, or by acknowledging the logon against the
token it was already given. Both are the venue accepting a credential.

Where it asks in full, the answer it sends back is checked: the venue states a
proof of the session key that only a party holding the account's verifier can
compute, and a logon whose proof does not match is refused. The group that
exchange runs in is this venue's own and no other, because a peer names it
before it has proved anything.

What none of that establishes is that the party holding the channel keys is the
party that answered the logon. The logon runs beside the channel rather than
inside it, so a peer that relays it to the venue in real time collects a proof
it did not compute. Binding the two needs the venue to state something over
both, and nothing on this wire does.

## Not portable

The session data on [what the API does not forward](./beyond-the-api.md) —
the account's grants, the order types the venue will take, its algorithms, the
round-trip time — has no message in the API a gateway offers. A program using
those calls runs here and does not run against a gateway. They are the part of
this client that is not a drop-in.

## The protocol is not published

This client speaks a protocol the venue does not document and can change
without notice. That is the standing risk of not running the vendor's own
process, and no amount of testing removes it. What the repository does about it
is regenerate the coverage matrices from the source on every commit and fail the
build when a claim stops matching the code.
