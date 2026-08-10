# Replacing the gateway: state and plan

**Goal.** A program that runs against IB Gateway today runs against this client
instead, gets the same answers, and is told nothing that IBKR did not say. No
Java runtime, no gateway process, no tool driving a window.

**Scope of "1:1".** Four separate claims, kept apart because they fail apart:

| Claim | Meaning | State |
| --- | --- | --- |
| **Compiles** | Every call a program uses exists, under the right name, with the right arguments | Met |
| **Reaches** | The request leaves the machine and the reply is read | 35 reach the venue, 26 answered from what it pushed, 3 missing, 4 unserviceable |
| **True** | Nothing reported was invented, defaulted, or dropped | 11 inventions removed; no message discards what it does not name |
| **Proved** | A live session showed it working | Partial — §5 |

A line moves when a check passes, not when code is written.

---

## 1. What is finished

### 1.1 Nothing the venue sends is discarded

The governing rule. A field with no name is kept under its number; a field is
never dropped because nothing has got round to reading it.

| Message | State | Evidence |
| --- | --- | --- |
| Contract definition | **kept, proved** | 49 unnamed fields on a share, 85 on a foreign share, 46 on a bond — from real replies |
| Execution report | **kept** | ~132 of 173 tags were dropped; all kept under their numbers |
| Account values | **kept** | A whitelist of ~18 keys survived; the rest went with no trace |
| Repeated fields in one message | **kept, proved** | The parsed map held one value per tag — a share's definition carries 51 stated fields where it appeared to carry 37 |
| Fields after a contract's identifier block | **kept** | Every contract in a multi-contract reply was truncated there |
| Tick attributes | **kept** | Decoded off the wire, then replaced with `false` |

### 1.2 Nothing is reported that IBKR did not state

Eleven inventions found and removed. Each was reported with the same confidence
as a real figure.

| What was invented | What it broke |
| --- | --- |
| Execution id and time | No fill could be reconciled against a broker statement — the id was the order number plus a process-local counter |
| Smallest order size | Read from a price-precision field; a share claimed a millionth of a share |
| Current time | The local clock, so skew always measured zero |
| Exchange attribution | A venue list written here; every quote named the wrong exchange |
| News providers | Eight invented entitlements the account may not hold |
| Soft-dollar tiers | An invented list — itself a transcription of the venue's own reply, so it looked right |
| Account currency | Dollars whatever the venue said |
| Commission currency | Dollars, so a fill was costed in a currency it was not charged in |
| Smallest price increment | A penny where the venue stated none — wrong for most futures |
| Tick attributes | `false` for past-limit and unreported, whatever the venue set |
| A request that silently did nothing | Binding externally-entered orders took its flag and returned |

**Guarded.** `scripts/gen_wire_reach.py` runs in CI and fails if any request
returns as though it acted when it did not. That category is held at zero.

### 1.3 Tick-by-tick

Marked blocked for months on the belief that its feed rode an unreachable
service. It did not. **Working end to end against real servers:** a currency
pair at 1.15555 bid, 1.15560 ask, in real sizes, hundreds of quotes a minute,
reaching a caller.

Three assumptions were wrong, none visible from frames written here:

- The continuation bit is **inverted** — a set high bit ends a number.
- A **two-byte bit-length header** bounds the payload.
- **Every record carries its own subscription id and timestamp**, not one per frame.

The first cause was not the frame at all: asking for trades on a currency pair
returns, in plain words, `No historical market data for EUR/CASH@IDEALPRO
AllLast 0`. A currency pair has quotes and no trades.

### 1.4 Client surfaces

| Surface | State |
| --- | --- |
| `EClient` / `EWrapper` (reference client) | Carried; both naming conventions resolve on the client, the wrapper, and every object handed to a callback |
| `ibx.IB` (asynchronous wrapper) | 90 of 90 public calls, checked against a written-out list rather than this client's own bookkeeping |
| `ibx::api::Client` (Rust client) | 72 of 77; 5 have no counterpart here and say why |
| Gateway settings | `ibx.configure()`; 11 carried, 7 named as having no counterpart |

---

## 2. What remains

### 2.1 Data still dropped — none

Order status was listed here and is already covered: an order's status is
produced from the execution report, whose fields are all kept, so there is no
second message to change. The only other producer of a status is this client
itself, marking orders uncertain when a transport goes away — nothing from the
venue to keep.

Every message the venue sends is now read without discarding what it does not
name.

### 2.2 Requests that do not reach IBKR — 3

The wires are established from the counterpart, tag by tag. None is guessed.

| Item | Wire |
| --- | --- |
| `requestFA` | MsgType `U`, 6040=116, command in 6905, partition in 6158, XML in 6118 |
| `replaceFA` | Same, command 3 |
| `reqWshMetaData` / event data | MsgType `U`, 6040=155, name in 6556, type in 8081, JSON in 8082 |

~½ session to build. **Neither can be verified on this account** — the advisor
pair needs an advisor account, event data a separate subscription.

### 2.3 Fields without names — 3

All reachable today under their tag numbers; naming moves them from a number to
a word.

- `evMultiplier` — no field for it on a definition; it arrives by another path
- `marketRuleIds` — assembled per venue, not stated as a list

### 2.4 Unserviceable — 4, correctly

Implied volatility and option price take a caller-supplied price or volatility
for the venue to work back from, and this protocol carries no such request. They
report that rather than pretending. The advisor pair reports the same until §2.2
lands.

---

## 3. Known divergences, accepted and recorded

| Divergence | Why |
| --- | --- |
| A price is held against one fixed scale of a hundred-millionth | The venue holds a price as a count of the contract's own increment, which has no floor. Ours has one, and a satoshi sits exactly on it. Guarded at build time. Closing it touches every price in the client |
| The single letter a venue is known by | The venue names venues in full and states no abbreviation. Client knowledge, not derivable from the name — NASDAQ is `Q`, not `N` |

---

## 4. What could not be established offline

Recorded so nobody re-derives them.

- **The combo side convention.** This client writes 6082 as the side on live
  evidence — sent the other way, a long call spread was refused as
  "Guaranteed-to-Lose". A static reading of the counterpart disagrees and is
  incoherent with itself. One live capture of a two-leg spread settles it.
- **The trading-status timestamp's unit.** Nothing in the counterpart reads it.
- **Whether a size carries an implied decimal** for fractional instruments.

---

## 5. Proving it

**This is the binding constraint, not the code.**

Live sessions have found, every time, defects no offline test could:

| Found live | Would offline tests have caught it? |
| --- | --- |
| Every Rust answering call broken by a queue change | No — 1391 tests passed while a contract lookup could not complete |
| Tick-by-tick's real cause, in the venue's own words | No |
| Frames attributed to the first subscription | No — invisible with one subscription |
| The soft-dollar format, in a log line | No |
| Sizes handed over unscaled | No |

| Item | State |
| --- | --- |
| Session, login, reconnect, orders, fills, market data, historical | Proved |
| Contract definitions across seven kinds | Proved |
| Tick-by-tick | Proved |
| Second factor | One confirmation, no repeat coverage |
| Futures options; bond, warrant, fund, commodity market data | Unit tested, never live |
| Everything else fixed today | Offline only |

---

## 6. Plan

| # | Work | Size |
| --- | --- | --- |
| 1 | Build the advisor and event-data wires | ½ session |
| 2 | Name the remaining fields | ½ session |
| 3 | **Live session at market hours** — prove everything since the last one | 1 session, then fixes |
| 4 | Repeat 3 until a session finds nothing | 2–4 sessions |

Steps 1–2 are days. Step 4 decides the date and cannot be compressed: today's
rate was roughly four real defects per live round.

**Not reachable on this account, ever:** the advisor pair and event data can be
built correctly but never verified here.
