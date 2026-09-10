# Streaming Ticks

Subscribe to top-of-book on SPY, collect for five seconds, print the latest bid,
ask and last.

## What this shows

- `req_mkt_data(req_id, &contract, generic_tick_list, snapshot, regulatory_snapshot)`.
- Reading prices off `tick_price`, where `tick_type` names which price it is:
  1 bid, 2 ask, 4 last, 9 close.
- `cancel_mkt_data(req_id)` before disconnecting.

## What comes back

`tick_price` and `tick_size` for as long as the subscription runs.

`tick_generic` fires for the halt state the venue states on tick 49: 0 while the
contract is trading, 1 once it has stopped.

With `snapshot: true` you get the first available quote, then
`tick_snapshot_end`, and this client cancels the subscription for you. That is
this client ending a subscription, not a separate request.

`regulatory_snapshot: true` is the venue's own one-shot snapshot. It is a
different request type, and an account without the
entitlement is refused by the venue. It also ends on `tick_snapshot_end`.

## Limits

Each number in `generic_tick_list` is asked for, as a subscription of its own
beside the prices. The number you state is the number the venue knows the series
by, so there is nothing to translate. `"292"` additionally subscribes to news for
that contract; an entry that is not a number is reported rather than sent.

Asking for a series and reading it are separate things. The subscription goes
out for every number you name, and the ticks that have a reader here are
published; the rest arrive and are skipped rather than guessed at. So a number
you name is genuinely requested, and what comes back reaches you once its reader
lands.

One subscription per contract. To change the mode on a contract, cancel first.

Delayed and frozen data are requested. Name the mode once with
`req_market_data_type` and every subscription after it carries it, or state it
per request with `req_mkt_data_ex`, whose `mode_9887` is 0 realtime, 1 delayed,
2 frozen, 3 delayed-frozen. Frozen keeps thinly traded names quoting after
hours, when the realtime feed is silent.

## Reading without the callback loop

`quote(req_id)` returns the latest bid, ask, last and sizes for a running
subscription, with no wrapper and no lock. `quote_of(&contract)` does the same
by contract. Both return `None` until the first tick has landed.

Prices and sizes on `Quote` are integers scaled by `PRICE_SCALE` and `QTY_SCALE`
(both 10<sup>8</sup>). Divide when you display; the `tick_price` callback hands
you `f64` already.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... cargo run --example hello_tick_data
```

## Source

```rust
{{#include ../../../../../examples/hello_tick_data.rs}}
```
