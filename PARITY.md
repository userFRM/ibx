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
| The asynchronous wrapper's shape | 90 methods | 26 carried, 65 open |
| The Rust client's shape | 77 methods | not started |

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

### The asynchronous wrapper — 7 of 90

`ibx.IB()`. Its names, its argument names, its defaults, and its habit of
filling a contract in place. The rest raise and name themselves; a
test holds that list honest, so it cannot drift into claiming more than is
there.

Three kinds of work sit behind the 83, and they are not the same size:

1. **Thin wrappers** over a call that already answers or already sends. Roughly
   thirty. Mechanical.
2. **Calls needing an answering form first** — the request exists and returns
   through a callback; the answering version has to be built beneath it.
   Roughly twenty.
3. **Live state.** Carried for what the account holds and what its orders are
   doing: `positions()`, `portfolio()`, `accountValues()`, `trades()`,
   `openTrades()`, `orders()`, `openOrders()`, `fills()`, `executions()`,
   `managedAccounts()`. A pump owns dispatch and the callbacks record rather
   than act; a reader gets a snapshot taken under a lock, so a list cannot
   change while it is being read.

   Quotes are not carried yet — `tickers()`, `pendingTickers`, `reqTickers()`.
   A `Ticker` accumulates several kinds of tick into one object and says which
   of them are stale, which is more than recording what arrives.

### The Rust client — not started

77 methods, a different shape again: typed returns and subscription iterators
rather than callbacks. The answering layer this client already has is the same
idea, so the work is naming and coverage rather than design.

## Order of work

1. ~~Live state for what the account holds and what its orders are doing.~~
   Carried.
2. Live quotes, which are the remaining half of live state.
3. The thin wrappers, which are volume rather than difficulty.
3. The answering forms beneath the rest.
4. The Rust shape, once the answering layer is complete enough to name.
5. The wire's remaining refusals, which need a live session.

## What is not claimed

This is not yet a complete replacement, and the parities above say where. The
gap that matters most is live state: a program written for the asynchronous
wrapper that reads `ib.positions()` in a loop does not run here yet.
