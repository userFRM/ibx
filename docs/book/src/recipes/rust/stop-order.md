# Send a Stop Order

Place a BUY STOP on SPY far above market. The trigger never fires, so the order
rests. Watch it acknowledge, then cancel it.

## What this shows

- `client.next_order_id()`, which reserves one id.
- An `Order` with `order_type: "STP"` and the trigger price in `aux_price`.
  `lmt_price` is unused for a plain stop.
- `tif: "GTC"`, so it survives the close rather than expiring with the day.
- `order_status` callbacks: `PreSubmitted` then `Submitted` while it rests,
  `Cancelled` once the cancel lands.
- `cancel_order(order_id, "")`.

## What comes back

The same `order_status` stream as any other order. A resting stop shows no
fills: `filled` stays 0 and `remaining` stays at the full quantity until the
trigger is touched.

## Limits

A stop is held until its trigger is touched, then goes to the market as a market
order. This example sets the trigger far above market so that never happens. On
a real stop, the fill price is whatever the market gives you at that moment, not
the trigger.

Paper account only.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... cargo run --example hello_stop_order
```

## Source

```rust
{{#include ../../../../../examples/hello_stop_order.rs}}
```
