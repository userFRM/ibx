# Send a Limit Order

Place a BUY LMT on SPY far below market, watch it acknowledge, then cancel it.
Connect, take an id, place, read status, cancel, disconnect.

## What this shows

- Taking the order id off `next_valid_id`, which `connect()` fires before it
  returns. `c.next_order_id()` reserves one directly if you would rather ask.
- An `Order` with `order_type = "LMT"` and `lmt_price` set. `outside_rth = True`
  is what lets it be acknowledged outside regular hours.
- `order_status` callbacks. `PreSubmitted` then `Submitted` while it rests,
  `Cancelled` once the cancel lands.
- `cancel_order(order_id, manual_order_cancel_time)`. Pass `""` for the second
  argument unless you are stating a manual cancel time.

## What comes back

`order_status` on every change and again on each fill. `filled` and `remaining`
are quantities, `avg_fill_price` is the average of what has filled so far. An
order that rests will sit at `Submitted` until you cancel it.

The `perm_id` on that callback is derived by this client from the identifier the
venue gives the order. It is stable for the life of the order, including across
a modify. It is not a number the venue states on the wire.

## Limits

Order ids count from one past the highest id the account is working an order
under, which the venue names at every connect, from every session. So one taken
here does not collide with an order placed elsewhere.

`tif` is checked against what the venue carries: `DAY`, `GTC`, `IOC`, `FOK`,
`OPG`, `GTD`, `GTX`, `DTC`, `AUC`, spelled exactly. An unrecognised value is
refused here rather than sent as `DAY` and quietly expiring at the close.

Paper account only. The price is set far below market so it will not fill.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/hello_limit_order.py
```

## Source

```python
{{#include ../../../../../examples/hello_limit_order.py}}
```
