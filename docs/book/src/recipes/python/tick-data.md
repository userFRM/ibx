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

`generic_tick_list` is not transmitted. The one exception is `"292"`, which
additionally subscribes to news for that contract. Any other entry is warned
about rather than quietly dropped: the venue asks for those series under
numbers of its own, and this client does not carry the mapping. So a request for
RTVolume and friends will not answer, and you will be told so instead of waiting
on a stream that is never coming.

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
