# Streaming Ticks

Subscribe to top-of-book on SPY, collect for five seconds, print the latest bid,
ask and last.

## What this shows

- `req_mkt_data(req_id, contract, generic_tick_list, snapshot, regulatory_snapshot)`.
- Reading prices off `tick_price`, where `tick_type` names which price it is:
  1 bid, 2 ask, 4 last, 9 close.
- `cancel_mkt_data(req_id)` before disconnecting.

## What comes back

`tick_price` and `tick_size` for as long as the subscription runs.

`tick_generic` fires for the halt state the venue states on tick 49: 0 while the
contract is trading, 1 once it has stopped.

With `snapshot=True` you get the first available quote, then
`tick_snapshot_end`, and this client cancels the subscription for you.

`regulatory_snapshot=True` is the venue's own one-shot snapshot. It is a
different request type, and an account without the
entitlement is refused by the venue. `EClient` carries it. The `IB` facade does
not, and says so by name rather than answering with an ordinary subscription.

## Limits

Each number in `generic_tick_list` is asked for, as a subscription of its own
beside the prices. The number you state is the number the venue knows the series
by, so there is nothing to translate. `"292"` additionally subscribes to news for
that contract; an entry that is not a number is reported rather than sent.

These are read and delivered today, each under the number the reference client
publishes it under:

| you ask for | you receive |
|---|---|
| `100` | call and put option volume, on `tick_size` 29 and 30 |
| `101` | call and put open interest, on `tick_size` 27 and 28 |
| `104` | historical volatility, on `tick_generic` 23 |
| `106` | option implied volatility, on `tick_generic` 24 |
| `236` | shortability on `tick_generic` 46, and the borrowable share count on `tick_size` 89 |
| `292` | news, on `tick_news` |
| `105` | average option volume, the two sides added, on `tick_size` 87 |
| `225` | auction volume and imbalance on `tick_size` 34 and 36, auction price on `tick_price` 35 |
| `233` | the trade tape, on `tick_string` 48: `price;size;time;volume;vwap;single` |
| `293` `294` `295` | trade count, trade rate and volume rate, on `tick_generic` 54, 55 and 56 |
| `318` | last regular-session trade, on `tick_price` 57 |
| `411` | real-time historical volatility, on `tick_generic` 58 |
| `460` | bond factor multiplier, on `tick_generic` 60 |
| `499` | borrow fee rate, on `tick_price` 111 |
| `586` | the estimated IPO midpoint on `tick_generic` 101, and what it opened at on 102 |
| `588` | futures open interest, on `tick_size` 86 |

Any other number is still requested and the venue still serves it, but nothing
here decodes that payload yet, so no tick arrives for it. Those are stepped over
rather than guessed at, which is why the prices and the series beside them keep
arriving normally. The rest land series by series.

One subscription per contract. To change the mode on a contract, cancel first.

Delayed and frozen data are requested. Name the mode once with
`req_market_data_type(mode)` and every subscription after it carries it, or
state it per request with `req_mkt_data_ex(..., mode_9887=)`, where the mode is
0 realtime, 1 delayed, 2 frozen, 3 delayed-frozen. Frozen keeps thinly traded
names quoting after hours, when the realtime feed is silent.

## Reading without the callbacks

`c.quote(req_id)` returns a dict of `bid`, `ask`, `last`, `bid_size`,
`ask_size`, `last_size`, `volume`, `high`, `low`, `open`, `close` for a running
subscription, or `None` if that request id is not subscribed. No wrapper state
to keep, no lock to take.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/hello_tick_data.py
```

## Source

```python
{{#include ../../../../../examples/hello_tick_data.py}}
```
