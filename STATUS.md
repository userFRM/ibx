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
| Top of book | ✅ live | American shares and currencies quote continuously, whether asked for as a stream or as a snapshot, and by as many callers at once as ask: ten requests on one contract were each answered with the same quotes, and the one holding the subscription leaving handed it to the next rather than taking it away. European and Canadian venues answer the subscription with what this account is entitled to, and that answer reaches the caller |
| Depth of book | ✅ live | A smart book on an American listing, every level naming the venue it stands on — eleven of them in twelve seconds across PSX, MEMX, AMEX, IEX and the rest. A level whose section names no subscription is not delivered at all, where before every one of them arrived under request zero. The venues that refuse depth to this account say so by name, and each refusal reaches the caller |
| Historical bars | ✅ live | Bars returned for Dutch, British, Hong Kong, Australian and American listings and an American index, in one sweep. Bars kept up to date arrive and go on arriving: the venue closes a query as soon as it answers it, so the bar still forming is folded from the five-second bars it does keep sending. A contract named rather than identified is looked up first, so a request carrying what a caller wrote down is answered like one carrying the venue's own id |
| Historical ticks, schedules | ✅ live | Ticks and trading schedules answered; a series this client does not know is refused rather than turned into trades |
| Tick-by-tick quotes | ✅ live | Currencies and American listings, several streams at once, each record naming its own request |
| Tick-by-tick trades | ✅ live | A thousand and twenty-seven trades on a busy listing, priced to the cent and sized in the shares the venue printed. The exchange's own trades and every trade including those never reported to the tape are two streams: the venue serves the wider one and marks each print, so the narrower one is that stream without them. Both were counted on a future, which has no off-exchange tape |
| Trading halts | ✅ live | Contracts that are trading are reported trading, read from the mask and the named status together |
| Tick attributes | ✅ live | Reported-away-from-the-exchange varies trade by trade, as it does for an off-exchange facility |

## Orders

| Capability | Stage | Proved by |
| --- | :---: | --- |
| 23 order types | ✅ live | Previewed against the venue, which prices them and places nothing |
| Every field of an order | ✅ live | 125 of 154 go out under a tag; 29 say on the field that this protocol does not carry them; none is dropped in silence. Each field wired this session was previewed, and two the venue refused were taken back out |
| Orders across markets | ✅ live | Previews accepted on German, Dutch, British, Swiss, Australian, Canadian and American listings and on a currency pair. Japan and Hong Kong refused them for a lot size this client had not rounded to — the venue's own rule, reached and reported |
| Modify, cancel, global cancel | ✅ live | An order placed on paper far under the market was acknowledged, changed, and withdrawn in one session, through both clients. A change states where the resting order is working; left off, the venue refused it as a mismatch and the order sat inactive while the caller's own cancel found nothing |
| Brackets, OCA, combos | ✅ live | A combination states its legs and, where the caller priced them separately, each leg's own price. The side convention is settled by the venue: a call spread with the nearer strike bought is priced, and the same two legs the other way round is refused as one that can only lose |
| Conditions | ✅ live | All six kinds — price, volume, percent change, margin, execution and time — placed on a real order through the Python client and held by the venue until their condition. An execution condition missing any of its three parts is refused here, because the venue answers that one by holding the order inactive and naming a tag |
| Executions and fills | ✅ live | A fill reported and the position reconciled against it. The report keeps every field the venue states, named or not. The session asks for them within the window the venue answers, which it requires |
| Option exercise / lapse | ✅ live | Both sent for a real option contract this account does not hold, and the venue answers each by name: you have not got the number of options requested to be exercised. That answer reaches the caller |

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
`scripts/gen_order_field_reach.py` counts it: of 154 fields on an order, 125 go
out under a tag and 29 say on the field itself that this protocol does not
carry them. None is read by nothing.

---

## Where this differs from a gateway

Nothing a caller can reach. The differences are what a gateway is, not what it
does for a program:

| A gateway has | Here |
| --- | --- |
| A window, and a file beside it | Settings stated on the client, and read back. Seven of the gateway's own have no counterpart because there is nothing to configure: no window to size, no local socket to listen on, no runtime to give memory to |
| A local socket a program connects to | This client *is* the program's client. Nothing to connect to, nothing to trust, nothing to leave running |
| A Java runtime | None |

On the two things that look like limits and are not:

- **The 29 order fields that do not reach the venue.** The counterpart does not
  send them either — for each, its own field declares no tag, or declares one
  the venue refuses by name. A program that sets them through a gateway is not
  sending them either; the difference is that here it is written down, counted,
  and checked on every build. Each field keeps what a caller set, so an order
  built against another client reads back unchanged.
- **Prices and quantities held to a hundred-millionth.** A gateway hands a
  caller a double. This holds a fixed point fine enough for a satoshi and for
  four orders of magnitude more volume than the busiest listing trades in a
  day, and refuses at build time to be made coarser.

What is not yet proved here, rather than not built: an advisor's allocations
and the corporate-events subscription. Both reach the venue and are answered;
seeing what they answer needs an advisor account and a subscription this login
does not have.

## Open questions

Settled only by a live capture, recorded so nobody re-derives them:

- The one number an option model needs that no tick states is fitted, not read. The venue does serve its own — a series named `OptExInterestRate`, which it recognises and refuses only the query shape for, answering `QueryType BarData(BarDataIntraday) is not supported for tick type`. Asked as a tick series it is accepted against an option contract and refused by name against the underlying share, which says where the series lives; every window tried against the option — a day, a week, ten weeks, with and without regular hours — is answered with an empty batch. Until then the fitted number absorbs whatever the venue's model does that this one does not, which is visible: two contracts on one underlying and one expiry wanted five per cent and twenty
- A crypto's tick-by-tick stream is acknowledged, with both its increments stated, and the venue then sends nothing on it. In the same session and on the same contract, the top of book quotes continuously and a historical tick request is answered, while an American share and a currency pair stream ticks throughout. So the venue holds crypto ticks and does not stream them
- The trading-status timestamp's unit, and the fourth number the venue sends with it

---

## Before 1.0

1. Live sessions at market hours until one finds nothing

That step is the constraint. Live sessions have, every time, found defects the
offline suite could not — including a client that died on a live
trade stream, a subscription that delivered the wrong kind of tick, and a
regression that broke every answering call — each while the whole offline suite
stayed green.

The last session found ten more. Each is what a program would have seen:

- `reqHistoricalData(Contract("SPY"))` returned no bars at all, unless the
  contract had been looked up first
- `accountSummary()` returned an empty list until something else asked the
  venue for the account
- a session opened with no execution history, because the venue rejected the
  request that asks for it
- `reqNewsProviders()` returned one provider whose name was every other
  provider
- the Rust client's `option_chain()` returned nothing while the Python
  client's returned three
- `reqTickByTickData(contract, "Last")` was answered with the other trade
  stream, then with silence
- `reqTickers()` returned a previous close and no bid or ask
- a contract with no bid quoted at minus one, and a caller waiting for a
  quote took that for one
- `reqHistoricalSchedule()` returned `None`
- changing an order's price left it inactive, and cancelling it then found
  nothing to cancel
