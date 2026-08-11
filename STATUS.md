# Status

A JVM-free Interactive Brokers client. No gateway process, no desktop
application, no tool driving a window.

Legend: **✅ working** · **🔬 built, not yet proved against a live venue** ·
**🚧 in progress** · **⛔ not served, and says so**

---

## Client surfaces

| Surface | Status | Notes |
| --- | :---: | --- |
| `EClient` / `EWrapper` | ✅ | Reference-client shape. Both naming conventions resolve |
| `ibx.IB` | ✅ | 90 / 90 calls of the async wrapper's surface |
| `ibx::api::Client` | ✅ | 77 / 77 callable; 3 report that they belong to a local process this client does not have |
| Gateway settings | ✅ | Stated on the client beside the login, both languages: 17 carried, 7 named as having no counterpart |

## Market data

| Capability | Status | Notes |
| --- | :---: | --- |
| Top of book | ✅ | |
| Depth of book | ✅ | |
| Historical bars, ticks, schedules | ✅ | |
| Tick-by-tick quotes | ✅ | Live on currencies and American listings: several at once, each naming its own request |
| Tick-by-tick trades | ✅ | Live: a thousand trades on a busy listing, priced and sized as the venue printed them |
| Trading halts | ✅ | Live: contracts trading are reported trading |
| Tick attributes | ✅ | Live: reported-away-from-the-exchange varies trade by trade |

## Orders

| Capability | Status | Notes |
| --- | :---: | --- |
| 23 order types | ✅ | |
| Modify, cancel, global cancel | ✅ | |
| Brackets, OCA, combos | ✅ | Combo side convention pending one live capture |
| Conditions | ✅ | Price, volume, percent, execution, time |
| Executions and fills | ✅ | |
| Option exercise / lapse | ✅ | |

## Account

| Capability | Status | Notes |
| --- | :---: | --- |
| Account values, summary | ✅ | Every figure the venue states, in its stated currency |
| Positions, P&L | ✅ | |
| Managed accounts | ✅ | |
| Financial advisor config | 🔬 | Needs an advisor account to verify |

## Reference data

| Capability | Status | Notes |
| --- | :---: | --- |
| Contract definitions | ✅ | Share, foreign share, index, bond, fund, option, future |
| Option chains, symbol search | ✅ | |
| Scanners, news, fundamentals | ✅ | |
| Corporate-events calendar | ✅ | Live: the event types arrive, 179 KB of them, and an events query is answered |
| Implied volatility, option price | ⛔ | Not carried by this protocol at all: the counterpart solves both in its own process from a pricing model it ships |

---

## Design rules

Two invariants, both machine-checked.

**Nothing the venue sends is discarded.** A field with no name is kept under its
tag number. A share's definition carries 49 such fields, a bond 46 — all
reachable. Applies to contract definitions, execution reports and account
values.

**Nothing is reported that the venue did not say.** No defaults standing in for
absent data, no values derived from tables written here.
`scripts/gen_wire_reach.py` runs in CI and fails if any request returns as
though it acted when it did not.

**Nothing a caller sets is dropped without saying so.** A call that reaches the
venue can still leave behind most of what was put in it.
`scripts/gen_order_field_reach.py` counts it: of 154 fields on an order, 127 go
out under a tag and 27 say on the field itself that this protocol does not
carry them. None is read by nothing.

---

## Known limits

| Limit | Detail |
| --- | --- |
| Price precision | Prices are held to a hundred-millionth. The venue holds a price as a count of the contract's own increment, which has no floor — a satoshi sits exactly on ours. Guarded at build time |
| Quantities | Held to a hundred-millionth as well, so the smallest size a venue counts in survives. A day's volume in the busiest listing is four orders of magnitude inside what the field holds |
| Order fields | 27 of 154 are not carried by this protocol. Each says so on the field, with the counterpart's own reason. Counted and checked in CI |

| Advisor and event data | Buildable, not verifiable without an advisor account and a WSH subscription |

## Open questions

Settled only by a live capture, recorded so nobody re-derives them:

- The combo side convention — live evidence and the counterpart's own encoding disagree
- A crypto's tick-by-tick stream is acknowledged, with both its increments stated, and the venue then sends nothing on it. Asked for alone, over minutes, on a contract whose top of book is moving. Not this client's doing, and not yet explained
- The trading-status timestamp's unit, and the fourth number the venue sends with it

---

## Before 1.0

1. Live sessions at market hours until one finds nothing

That step is the constraint. Live sessions have, every time, found defects the
offline suite could not — including a client that died on a live
trade stream, a subscription that delivered the wrong kind of tick, and a
regression that broke every answering call — each while the whole offline suite
stayed green.
