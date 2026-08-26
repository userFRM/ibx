# Login

The smallest program that opens a session: connect, read the order id this client
would place under, disconnect.

Two flavours are shown. **Paper** for everyday testing, **live** for read-only
checks against the real account.

## What this shows

- Reading credentials from environment variables into `EClientConfig`.
- `paper: true` for the paper account, `paper: false` for the live one.
- `client.account_id`, which names the account the session opened under.
- `req_ids`, which reports the next order id.

## What comes back

`EClient::connect` blocks until the session is up. When it returns, the session
is established and `client.account_id` is populated. There is nothing further to
wait for.

`req_ids` does not ask the venue anything. It calls `next_valid_id` on your
wrapper right there, with one past the highest id the account is working an
order under. The venue names that working set unprompted at every connect, from
every session, not only this one. Nothing is reserved by the call; the
reservation happens when you place.

## Limits

A live login may push a second factor. Approve it on your authenticator; the
`connect` call blocks until the gate clears, so give it a longer wall clock than
a paper login needs.

The live example is read-only: it logs in, prints, and disconnects. Send orders
from the paper account.

## Paper

### Run it

```bash
IB_USERNAME=... IB_PASSWORD=... cargo run --example hello_login
```

### Source

```rust
{{#include ../../../../../examples/hello_login.rs}}
```

## Live

### Run it

```bash
IB_LIVE_USERNAME=... IB_LIVE_PASSWORD=... cargo run --example hello_login_live
```

### Source

```rust
{{#include ../../../../../examples/hello_login_live.rs}}
```
