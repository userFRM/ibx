# Request Account PnL

Subscribe to the account-level PnL stream, take the first update, cancel and
disconnect.

## What this shows

- Reading the account name off `client.account_id`, which `connect` populates.
- `req_pnl(req_id, account, model_code)`.
- Reading `daily_pnl`, `unrealized_pnl` and `realized_pnl` off the `pnl`
  callback.
- `cancel_pnl(req_id)` before disconnecting.

`account` empty means the account this session opened under.

## What comes back

The venue states what each holding was worth at midnight and what it has
realised since. The three figures on the `pnl` callback are worked out from
those against the prices this session is being told, so they move as the
session's quotes move.

## Limits

`model_code` is taken and not applied. There is no model portfolio to name here,
so pass `""`.

`cancel_pnl` stops the reporting on this side. Nothing on this wire withdraws
the subscription at the venue, so treat it as "stop telling me", not "stop
sending".

An account with no position reports zeros. Pair this with the limit-order recipe
if you want to watch the numbers move.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... cargo run --example hello_pnl
```

## Source

```rust
{{#include ../../../../../examples/hello_pnl.rs}}
```
