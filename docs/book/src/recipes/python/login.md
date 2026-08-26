# Login

The smallest program that opens a session: connect, read the order id, disconnect.

Two flavours are shown. **Paper** for everyday testing, **live** for read-only
checks against the real account.

## What this shows

- Reading credentials from environment variables.
- `EClient.connect(username=, password=, host=, paper=)`.
- Starting `EClient.run` on a daemon thread.
- Reading the order id off the `next_valid_id` callback.

## What comes back

`connect()` blocks until the session is up, and fires three callbacks on your
wrapper before it returns, on the calling thread: `connect_ack`,
`managed_accounts`, then `next_valid_id`.

`next_valid_id` is fired last on purpose. The venue names what the account has
already used just after the connection is made, and an id announced before that
lands is one a fill spent long ago. A program that trusted it would have its
first order refused as a duplicate.

So when `connect()` returns, the session is established and the id has already
been delivered. The `wait()` in the example is belt and braces, not a gate.

The daemon `run()` thread is what carries every callback after connect: quotes,
order status, bars, errors. Start it, or nothing further arrives.

## Limits

A live login may push a second factor. Approve it on your authenticator;
`connect()` blocks until the gate clears, so give it a longer wall clock than a
paper login needs.

The live example is read-only: it logs in, prints, and disconnects. Send orders
from the paper account.

## Paper

### Run it

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/hello_login.py
```

### Source

```python
{{#include ../../../../../examples/hello_login.py}}
```

## Live

### Run it

```bash
IB_LIVE_USERNAME=... IB_LIVE_PASSWORD=... python examples/hello_login_live.py
```

### Source

```python
{{#include ../../../../../examples/hello_login_live.py}}
```
