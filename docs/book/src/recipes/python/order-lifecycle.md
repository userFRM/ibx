# Order Lifecycle

One order walked through every state it can reach on SPY: place, modify, fill,
cancel, and the executions query afterwards. Use this when you are changing
order handling and want each transition checked, rather than as a template to
copy into a strategy.

It prints a pass/fail line per step and exits non-zero if any step failed.

## What this shows

- Taking the first order id off `next_valid_id` and counting up from it. The
  example keeps its own counter so it can hold two ids at once;
  `c.next_order_id()` reserves one directly.
- Placing a limit order far from market and reading `Submitted` off
  `order_status`.
- `req_open_orders()`, and finding the order in the `open_order` callbacks.
- Placing again under the *same* order id at a new price, which is a modify, and
  checking the permId did not move.
- Walking the price to market to trigger a fill, then `req_executions(req_id)`.
- Placing a second order, cancelling it, and reading the `Cancelled` terminal
  status.
- Flattening whatever filled before disconnecting.

## What comes back

`req_open_orders()` waits up to three seconds for the replay the venue sends
unprompted after a connect, then fires `open_order` and `order_status` for each
working order. It waits because answering before that replay lands reports none
of them, and a caller who reads "nothing working" places the same order twice.
`req_all_open_orders()` is the same call.

`req_executions(req_id)` replays **this session's own record** of fills, then
fires `exec_details_end`. The account's history is not in it. The venue names
executions alongside the orders it replays when a session opens, and an
execution whose order has since completed is not replayed. So an empty answer
means this session has seen no fills, not that the account has none. The fill in
step 6 happens inside this session, which is why the query finds it.

An `exec_filter` is read by attribute: `symbol`, `secType`, `exchange`, `side`.
A `side` of `BUY` or `SELL` is translated to the venue's own word before
matching, so a filter written the way you write an order action works.

## permId

The `perm_id` on `order_status` is derived by this client from the identifier
the venue gives the order. It is stable for the life of the order, which is what
step 5 checks: a modify keeps the same order, so it keeps the same permId.

It is not a number the venue states on the wire. Do not expect it to match an
identifier from another system, and do not persist it as one.

## Limits

Paper account only. Step 6 deliberately crosses the spread, so it moves real
paper position; the script flattens it at the end.

Steps 6 and 7 need an open market. With the market shut, price discovery falls
back to `SPY_PRICE` (default 630.0), nothing fills, and both steps report
failure rather than passing on a market that never traded.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/order_lifecycle.py
```

## Source

```python
{{#include ../../../../../examples/order_lifecycle.py}}
```
