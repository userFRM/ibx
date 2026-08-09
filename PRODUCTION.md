# The road to production

What "production ready" means here: a program that today runs against IB Gateway
runs against this client instead, gets the same answers, and nothing it is told
is invented. No Java runtime, no gateway process, no tool driving a window.

Everything below is a state, not an intention. A line moves when a check passes,
not when the code is written.

## How a line gets marked done

| Mark | Means |
| --- | --- |
| **wired** | The request reaches IBKR's servers and the reply is read into named fields |
| **kept** | The venue states it and this client stores it, named or under its number |
| **proved** | A live session against real servers showed it working |

`wired` without `proved` is the usual state after an offline change, and is not
finished. Three defects shipped this month passed fifteen hundred offline checks
and were only found against real servers.

---

## 1. Losing nothing the venue sends

The rule: what the venue states is kept. A field with no name is kept under its
number. A field is never dropped because nothing has got round to reading it.

| Item | State |
| --- | --- |
| Every field a contract's definition states | **kept, proved** — 49 unnamed fields on a share, 85 on a foreign share, kept under their numbers |
| Fields repeated in one message (rule bands, identifier groups) | **kept, proved** — the parsed map held one value per tag and lost the rest |
| Fields stated after a contract's identifier block | **kept** — every contract in a multi-contract reply was truncated there |
| A trading halt | **open** — the opcode it was read from was named for a halt on no evidence; the venue states it as a generic tick carrying a status mask (open / regulatory / volatility / short-sale restriction / none). Withdrawn rather than reported wrongly |
| Every field an execution report states | **kept** — what the handler does not name is kept under its number, read from the bytes so repeats survive |
| Every field an order status states | **open** — same, not yet measured |
| Account values outside a whitelist of ~18 | **open** — everything else the venue states is dropped with no trace |
| Tick attributes on a trade or quote | **open** — decoded off the wire, then replaced with defaults presented as the venue's word |
| Option greeks the reference client has no slot for | **kept** — decoded and dropped, matching the terminal's own surface; listed so the choice is visible |

## 2. Saying nothing the venue did not say

The rule: a value handed to a caller came from the venue, or is marked as not
having. This is where the worst defects have been.

| Item | State |
| --- | --- |
| An execution's id and time | **fixed** — were made up from the order number and a process-local counter; the id is what reconciles a fill against a broker statement |
| The smallest order a venue will take | **fixed** — read from a price-precision field; a share claimed a millionth of a share |
| The current time | **fixed** on both surfaces — answered from the local clock, which reports zero skew whatever the truth |
| Which venue a quote's bid, ask and last came from | **fixed** — attributed through a table written in this client; the venue states the list itself |
| A request that returns as though it acted | **fixed and guarded** — held at zero by a generator CI runs |
| Commission and account currency | **open** — hardcoded USD; the contract's currency is parsed and not carried |
| Soft-dollar tiers when the logon states none | **open** — an invented list is served as the account's entitlements. News providers: **fixed**, the logon is the only source and empty means entitled to none |
| A contract's tick size when the venue states none | **open** — 0.01 is invented, wrong for most futures |
| The single letter a venue is known by | **accepted** — the venue names venues in full and states no abbreviation; this is client knowledge, and is recorded as such |

## 3. The wire

| Item | State |
| --- | --- |
| 35 requests that reach the venue | **wired, proved** |
| 26 answered from what the venue pushes on login | **wired, proved** |
| Financial advisor pair, event-data metadata | **open** — should reach the venue and do not; the request has not been established |
| Implied volatility, option price | **refused** — this protocol carries no request taking a caller's price or volatility |
| Tick-by-tick | **diagnosed, ours to fix** — not an entitlement and not the subscription, both of which are right. The decoder expects marker bytes and a per-record timestamp; the wire is bit-packed, field-count delimited, with a frame-level timestamp and prices as a signed delta in units of the contract's own tick |
| Tick-by-tick attributed to the right subscription | **open** — every record is attributed to the first subscription |

## 4. A contract's fields

Settled against real replies for a share, a foreign share, an index, a bond, a
fund, an option and a future.

| Item | State |
| --- | --- |
| Underlying id, symbol, kind | **wired, proved** |
| Last trade time, issue date, economic-value rule | **wired, proved** |
| Settlement method, price and size precision | **wired** |
| Size increment, suggested size increment | **wired** — a rule states two tables under the same tags; reading stopped at the count that opens the second |
| Market rule ids | **open** — assembled per venue, not stated as a list |
| Economic-value multiplier | **open** — no field for it on a definition; it arrives by another path |

## 5. Proving it

Nothing here is finished until a live session shows it. The account permits one
session at a time.

| Item | State |
| --- | --- |
| Contract definitions across seven kinds | **proved** |
| Session, login, reconnect, orders, fills, market data, historical | **proved** — earlier sessions |
| Second factor | **one confirmation, no repeat coverage** |
| Futures options, bond/warrant/fund/commodity market data | **open** — unit tested, never live |
| Everything fixed today | **open** — offline only; needs a session at market hours |

---

## Order of work

1. **Keep everything on the remaining messages.** Executions are done. Order
   status and account values still need what a definition and an execution now
   have: nothing dropped, unnamed fields kept under their numbers. This closes a
   whole class rather than a field, and it is how the definition's own gaps were
   found.
2. **Finish the market-rule block.** The grammar is established; the parser
   stops at the size tables. This settles two published fields.
3. **Stop the remaining invented values.** Currency, tick-size default, the
   entitlement fallbacks, tick attributes.
4. **Fix tick-by-tick attribution**, and make the Python surface refuse what
   the Rust surface refuses rather than accepting a subscription that never
   delivers.
5. **Establish the advisor and event-data requests** from the gateway.
6. **Prove it live**, at market hours, in one session.
