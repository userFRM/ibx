# What replacing the gateway means, and where this stands

A program that trades Interactive Brokers today runs three things: a client
library, a gateway process holding the session, and — because that process was
built to be driven by hand — a second tool that logs it in and clicks its
dialogs. This client replaces all three. That is four separate parities, and
they are tracked separately because they fail separately.

Counts here are measured, not asserted. Where a number is an estimate it says so.

| Parity | Measure | State |
| --- | --- | --- |
| The wire | 77 canonical calls | 69 served, 8 answer that they cannot be |
| The gateway's settings | 11 with a counterpart, 7 without | all 11 carried, all 7 named |
| The tool that drives the gateway | 51 settings | 12 carried, 33 need no counterpart, 6 open |
| The reference client's shape | `EClient`/`EWrapper` | carried |
| The asynchronous wrapper's shape | 90 methods | 90 carried |
| The Rust client's shape | 77 methods | 56 carried, 21 open |

## 1. The wire

Tracked in [ROADMAP.md](ROADMAP.md). What is left is stated there rather than
repeated: eight calls that report they cannot be served, one capability the
venue refuses this session, and a handful of contract fields still unread.

## 2. The gateway's own settings

A gateway is a process, so it is configured by a file beside it. This client is
a library, so the same settings sit on the client — `ibx.configure()`, read back
with `ibx.settings()`, each one naming what it stands in for.

Settings with no counterpart are named in `ibx.UNAVAILABLE` with the reason. A
port to listen on, and the addresses permitted to reach that port, mean nothing
for something that is the client rather than something clients connect to.

## 3. The tool that drives the gateway

The gateway was built to be operated by a person. The tool that drives it exists
to supply the person: it types the password, answers the second factor, dismisses
the warnings, and restarts the process on a schedule. Most of its settings
describe how to work a window.

**Carried natively.** The login and its second factor are part of this client,
not something typed into it. Trading mode, read-only, and the existing-session
question are settings here.

**Needs no counterpart.** Roughly two thirds — every dismissal of a dialog, the
window's size and position, where to write the settings file, the port the
command server listens on. There is no window and no dialog. These are not gaps;
they are the reason for replacing the process.

The order precautions among them are worth stating precisely rather than
lumping in: they are checks the desktop application applies before it sends,
and a program reaching the venue through this client is not subject to them.
A caller wanting such a check should make it, and it should not be silently
implied by the absence of a dialog.

**Open.** Scheduled restart, scheduled logoff, and cold restart. A gateway had
to be restarted because it was a long-lived process that degraded; this client
holds a session and rebuilds it when it drops, which covers the reason those
settings existed but not every use of them. A caller wanting a session torn down
and rebuilt at a fixed hour has to arrange it. Whether that belongs in a library
is an open question and is recorded as one rather than answered by silence.

## 4. The three client shapes

Three libraries in common use, three shapes. A drop-in replacement means a
program written against any of them runs unchanged, so all three are carried
rather than one being chosen.

### The reference client — carried

`EClient` and `EWrapper`, request under an id and answer on a callback. Both
naming conventions resolve, on the client, the wrapper, and every object handed
to a callback.

### The asynchronous wrapper — 90 of 90

`ibx.IB()`. Its names, its argument names, its defaults, and its habit of
filling a contract in place. Every one of its ninety public calls is carried,
checked against a written-out list rather than against this client's own
bookkeeping, so the check cannot shrink to match what happens to be built.

Carried is not the same as verified. Four of them reach calls this client
answers as not served — implied volatility, option price, and the advisor pair
— and they report that rather than pretending. Everything else is exercised
offline; what a live session has confirmed is in ROADMAP.md, not here.

What that took, in three parts of very different size:

1. ~~**Thin wrappers** over a call that already answers or already sends.~~
   Carried, including the order path: `placeOrder` hands back a record whose
   status moves under the caller rather than a return code.
2. ~~**Calls needing an answering form first**.~~ Carried. `reqTickers`
   subscribes, waits and unsubscribes; `whatIfOrder` marks the order as a
   question so nothing reaches the market; `reqScannerData` runs a subscription
   for one answer and withdraws it.
3. **Live state.** Carried for what the account holds and what its orders are
   doing: `positions()`, `portfolio()`, `accountValues()`, `trades()`,
   `openTrades()`, `orders()`, `openOrders()`, `fills()`, `executions()`,
   `managedAccounts()`. A pump owns dispatch and the callbacks record rather
   than act; a reader gets a snapshot taken under a lock, so a list cannot
   change while it is being read.

   Quotes are carried too. A quote does not arrive as a quote — it arrives as a
   bid, then a size, then a trade — so `reqMktData()` hands back a `Ticker` that
   fills in as the ticks reach it, and `pendingTickers` gives the ones that
   changed since it was last read. A field nobody has sent stays unset rather
   than becoming zero, because a bid of zero and no bid at all are different
   markets and the difference decides whether an order should be sent.

   `reqTickers()` — subscribe, wait for a quote, unsubscribe — is not carried
   yet.

### The Rust client — 56 of 77

`ibx::api::Client`, beside `EClient` rather than instead of it. A call that
answers returns the answer; a call that only sends returns nothing, because
handing back a value meaning "it was sent" tells a caller nothing they did not
already know. What the sending calls produce is recorded, since a caller of this
shape has no callback to hand it to.

Carried: the session, the calls that answer (contract details, qualifying one
contract or a list, bars, positions, the account summary, the option chain, the
earliest data a contract holds, a symbol search, a volume histogram, a
fundamental report), the streams (bars and depth), and the sending calls —
orders, cancels, exercise, the order books, family codes, news providers and
bulletins, market rules, scanner parameters, depth exchanges, market data type,
account updates and the P&L pair.

`place_order` returns the id the venue answers under. A caller with nothing to
correlate on cannot tell which of several answers is theirs, and the same holds
for a scanner subscription and a display group.

Four calls reach requests this client answers as not served — implied
volatility, option price, and the advisor pair. They report that rather than
pretending, which is the honest shape for a request this protocol does not
carry.

What is open is the part of that shape this client has no equivalent for at all:
its builder and configuration, the notice and order-update streams, the server's
own version and clock, and message verification.

Streams are carried as `Subscription<T>`, which a caller loops over. It does two
things a bare loop over a queue would not. It **withdraws**: a dropped
subscription stops asking, because one left running feeds a session nobody reads
and costs the account a line it is not using. And it **ends on the venue's
refusal** rather than blocking on data that is never coming, keeping the refusal
where a caller can read it — a stream that ended because the venue said no and
one that ended because nothing came look identical otherwise.

Bars and depth run on it. The rest of the streams are volume behind that
design rather than design. Until they land, `Client::inner()` reaches the
callback shape for anything not carried, on the same session, so nothing is
unreachable.

## Order of work

1. ~~Live state for what the account holds and what its orders are doing.~~
   Carried.
2. ~~Live quotes.~~ Carried.
3. ~~The thin wrappers.~~ Carried.
4. ~~The answering forms beneath the rest.~~ Carried.
5. The Rust shape. The subscription that decides the rest is built; what is
   left is coverage.
6. The wire's remaining refusals, which need a live session.

## What is not claimed

This is not yet a complete replacement, and the parities above say where.

A program written against the asynchronous wrapper finds every call it uses.
That is not the same as every call having been proved against a live venue, and
ROADMAP.md is where that distinction is kept.

The Rust client's shape is 13 of 77. The wire still has eight calls that answer
they cannot be served, and one capability the venue refuses this session.
