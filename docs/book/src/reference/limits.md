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

## 256 instruments at a time

The engine tracks 256 distinct contracts concurrently. The 257th registration is
refused with a message naming the cap.

Concurrent, not cumulative: cancelling a market-data subscription frees its slot
and the slot is reused. A long-running process that subscribes and never cancels
will reach it; one that cancels what it is done with will not.

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
