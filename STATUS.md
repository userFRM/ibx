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
| `ibx::api::Client` | ✅ | 72 / 77; 5 have no counterpart here |
| `ibx.configure()` | ✅ | Gateway settings; 11 carried, 7 named as N/A |

## Market data

| Capability | Status | Notes |
| --- | :---: | --- |
| Top of book | ✅ | |
| Depth of book | ✅ | |
| Historical bars, ticks, schedules | ✅ | |
| Tick-by-tick | ✅ | Trades, quotes, midpoint |
| Trading halts | 🔬 | Decoder done; needs a generic-tick subscription |
| Tick attributes | 🔬 | Past-limit, unreported, past-low/high |

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
| Wall Street Horizon events | 🔬 | Needs a separate subscription to verify |
| Implied volatility, option price | ⛔ | This protocol carries no request for either |

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

---

## Known limits

| Limit | Detail |
| --- | --- |
| Price precision | Prices are held to a hundred-millionth. The venue holds a price as a count of the contract's own increment, which has no floor — a satoshi sits exactly on ours. Guarded at build time |

| Advisor and event data | Buildable, not verifiable without an advisor account and a WSH subscription |

## Open questions

Settled only by a live capture, recorded so nobody re-derives them:

- The combo side convention — live evidence and the counterpart's own encoding disagree
- The trading-status timestamp's unit
- Whether a size carries an implied decimal for fractional instruments

---

## Before 1.0

1. Subscribe generic ticks — turns halts and tick attributes from 🔬 to ✅
2. Name `evMultiplier` and `marketRuleIds`, currently reachable by tag number
3. Live sessions at market hours until one finds nothing

Step 3 is the constraint. Live sessions have, every time, found defects the
offline suite could not — including a regression that broke every answering
call while 1,391 tests stayed green.
