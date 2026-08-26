# Streaming L2 Market Depth

Subscribe to Level-2 depth on two contracts at once, keep a bid and ask book per
contract from the update stream, and print both books at the end.

## What this shows

- `req_mkt_depth(req_id, &contract, num_rows, is_smart_depth)`, one request id
  per contract.
- Applying `update_mkt_depth_l2` events to a book keyed by level position.
- `cancel_mkt_depth(req_id)` for each subscription before disconnecting.

## What comes back

`update_mkt_depth_l2` per level change, carrying the level position, the market
maker, and:

| field | values |
|---|---|
| `operation` | 0 insert, 1 update, 2 delete |
| `side` | 0 ask, 1 bid |

Every level from this client names the venue it stands on, which is why the
example keeps `market_maker` alongside price and size.

The example collects for the whole window and prints each book once at the end,
not on every update. `DURATION_SECS` sets that window; the default is 15.

## Limits

A depth subscription the venue will not serve is refused, and the refusal
arrives on the `error` callback. A book that stays empty means the subscription
never started, not that the market is empty.

The example asserts at the end that both books received updates and that neither
is empty, so it panics rather than reporting success on a feed that never
delivered. Run it while the market is open.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... cargo run --example l2_aapl_tsla
```

Collect for longer:

```bash
DURATION_SECS=30 cargo run --example l2_aapl_tsla
```

## Source

```rust
{{#include ../../../../../examples/l2_aapl_tsla.rs}}
```
