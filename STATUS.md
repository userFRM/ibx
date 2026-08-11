# Status

A JVM-free Interactive Brokers client. No gateway process, no desktop
application, no tool driving a window.

Every row says what proved it, because "working" on its own is a claim and not
an answer.

| Stage | What it means |
| :---: | --- |
| ✅ **live** | Exercised against the venue and the answer checked. The evidence is named |
| 🔬 **wire** | The request reaches the venue and it answers, but nothing has read the answer end to end |
| 🛠 **offline** | Built and tested here. Nothing has put it in front of the venue |
| ⛔ **refused** | Not served, and it says so where the caller can see it |

Anything marked **live** was proved on a paper account against the real
servers. Where a market's data is not entitled to this account, that is said
rather than counted as a failure of the client: the request left, the venue
answered, and what it answered was about the subscription.

---

## Client surfaces

| Surface | Stage | Proved by |
| --- | :---: | --- |
| `EClient` / `EWrapper` | ✅ live | Every live session in this file runs through it. Both naming conventions resolve |
| `ibx.IB` | ✅ live | 90 / 90 calls of the async wrapper's surface; the Python suite exercises it |
| `ibx::api::Client` | ✅ live | 77 / 77 callable; 3 say they belong to a local process this client does not have |
| Gateway settings | ✅ live | 17 carried, 7 named as having no counterpart. A session opened against the venue under a stated build and time zone, resolved SPY, and read the settings back |
| Both clients agree | ✅ live | Four gates compare them offline — settings, order fields, surfaces, and what each does when a call cannot be served — and a fifth asks the venue the same ten questions from each and compares the answers. It found the Rust client's option chains coming back empty while the Python client's were answered |

## Market data

| Capability | Stage | Proved by |
| --- | :---: | --- |
| Top of book | ✅ live | American shares and currencies quote continuously. European and Canadian venues answer the subscription with what this account is entitled to, and that answer reaches the caller |
| Depth of book | ✅ live | Forty-nine book updates in twenty seconds on one American listing. The venues that refuse depth to this account say so by name, and each refusal reaches the caller |
| Historical bars | ✅ live | Bars returned for Dutch, British, Hong Kong, Australian and American listings and an American index, in one sweep. A contract named rather than identified is looked up first, so a request carrying what a caller wrote down is answered like one carrying the venue's own id |
| Historical ticks, schedules | ✅ live | Ticks and trading schedules answered; a series this client does not know is refused rather than turned into trades |
| Tick-by-tick quotes | ✅ live | Currencies and American listings, several streams at once, each record naming its own request |
| Tick-by-tick trades | ✅ live | A thousand and twenty-seven trades on a busy listing, priced to the cent and sized in the shares the venue printed |
| Trading halts | ✅ live | Contracts that are trading are reported trading, read from the mask and the named status together |
| Tick attributes | ✅ live | Reported-away-from-the-exchange varies trade by trade, as it does for an off-exchange facility |

## Orders

| Capability | Stage | Proved by |
| --- | :---: | --- |
| 23 order types | ✅ live | Previewed against the venue, which prices them and places nothing |
| Every field of an order | ✅ live | 127 of 154 go out under a tag; 27 say on the field that this protocol does not carry them; none is dropped in silence. Each field wired this session was previewed, and two the venue refused were taken back out |
| Orders across markets | ✅ live | Previews accepted on German, Dutch, British, Swiss, Australian, Canadian and American listings and on a currency pair. Japan and Hong Kong refused them for a lot size this client had not rounded to — the venue's own rule, reached and reported |
| Modify, cancel, global cancel | ✅ live | An order placed on paper far under the market was acknowledged, changed, and withdrawn in one session |
| Brackets, OCA, combos | ✅ live | A combination states its legs and, where the caller priced them separately, each leg's own price. The side convention is settled by the venue: a call spread with the nearer strike bought is priced, and the same two legs the other way round is refused as one that can only lose |
| Conditions | ✅ live | All six kinds — price, volume, percent change, margin, execution and time — placed on a real order through the Python client and held by the venue until their condition. An execution condition missing any of its three parts is refused here, because the venue answers that one by holding the order inactive and naming a tag |
| Executions and fills | ✅ live | A fill reported and the position reconciled against it. The report keeps every field the venue states, named or not. The session asks for them within the window the venue answers, which it requires |
| Option exercise / lapse | 🛠 offline | Encoded and tested here; exercising a real option is not something to do on a whim |

## Account

| Capability | Stage | Proved by |
| --- | :---: | --- |
| Account values, summary | ✅ live | Twenty-nine figures, in their stated currencies, subscribed as the session opens so a program that reads them straight afterwards finds them there |
| Positions, P&L | ✅ live | Positions and daily figures arrive on login and after each fill |
| Managed accounts | ✅ live | Every account this login holds is named |
| Financial advisor config | 🔬 wire | Both clients ask the venue. Seeing the answer needs an advisor account, which this login is not |

## Reference data

| Capability | Stage | Proved by |
| --- | :---: | --- |
| Contract definitions | ✅ live | Eleven of twelve resolved in one sweep across nine countries — German, Dutch, British, Swiss, Japanese, Hong Kong, Australian, Canadian and American, plus a currency pair and an index. The twelfth was a future matching two contracts, which is refused by name rather than guessed at |
| Option chains, symbol search | ✅ live | Three chain replies for one underlying, and fifty-six matches for a three-letter search |
| Scanners, news, fundamentals | ✅ live | Six hundred and ninety-seven thousand bytes of scanner parameters, a fundamental document, and the hundred and seventeen news providers the venue lists — each under its own code. Headlines need a subscription this login holds for none of them: every provider, asked for alone, is answered and answered empty |
| Corporate-events calendar | ✅ live | Live: the event types arrive, 179 KB of them, and an events query is answered |
| Implied volatility, option price | ✅ live | Answered here, not asked for: this protocol carries no request and the counterpart computes both in its own process. Anchored to the venue's published model for the contract, so the price it reproduces is the venue's own — live, to the cent, on two contracts |

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

- The one number an option model needs that no tick states is fitted, not read. The venue does serve its own — a series named `OptExInterestRate`, which it recognises and refuses only the query shape for, answering `QueryType BarData(BarDataIntraday) is not supported for tick type`. Asked as a tick series instead it is accepted, and answers so far are empty for the contracts tried. Until then the fitted number absorbs whatever the venue's model does that this one does not, which is visible: two contracts on one underlying and one expiry wanted five per cent and twenty
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
