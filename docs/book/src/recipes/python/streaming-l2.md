# Streaming L2 Market Depth

Subscribe to Level-2 depth on two contracts at once, keep a bid and ask book per
contract from the update stream, and print top-of-book at the end.

## What this shows

- `req_mkt_depth(req_id, contract, num_rows=5)`, one request id per contract.
- Applying `update_mkt_depth_l2` events to a book keyed by level position.
- `cancel_mkt_depth(req_id)` for each subscription before disconnecting.

## What comes back

`update_mkt_depth_l2(req_id, position, market_maker, operation, side, price,
size, is_smart_depth)` on every level change:

| field | values |
|---|---|
| `operation` | 0 insert, 1 update, 2 delete |
| `side` | 0 ask, 1 bid |

Every level from this client names the venue it stands on, which is why the
example keeps `market_maker` alongside price and size.

The example collects for the whole window and prints once at the end, not on
every update. `DURATION_SECS` sets that window; the default is 15.

## Limits

A depth subscription the venue will not serve is refused, and the refusal
arrives on the `error` callback. The example prints an em dash for a side with
no levels rather than failing, so a book that never started shows up as zero
updates rather than as an error.

A contract whose security type and exchange are both left off is sent as it
stands, and an unnamed exchange is read as the smart destination. Naming a
security type that does not match the contract asks for one book as another's,
which the venue refuses.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/hello_l2.py
```

Collect for longer:

```bash
DURATION_SECS=30 python examples/hello_l2.py
```

## Source

```python
{{#include ../../../../../examples/hello_l2.py}}
```
