# What the session states, and the API does not forward

This client speaks the protocol a gateway speaks. The gateway receives more
than it forwards: some of what the venue states at logon and on a contract has
no message in the API a gateway offers its own clients, so a program written
against that API has no way to ask for it.

Everything below is stated by the venue on an ordinary session. The figures are
from one paper session and will differ with the account; what does not differ is
that none of it is reachable through `ib_async` or the TWS API.

The examples are Python. Every one of these is on the Rust `EClient` under the
same name — see the [table at the end](#the-same-calls-in-rust).

## What the account may do

The venue states its grants at logon — one token per capability.

```python
grants = client.enabled_features()          # 302 on the session this was written from
"ISLAND2NASDAQ" in grants                   # True
```

They decide behaviour. `island_for_nasdaq` — whether a US stock trading on
Nasdaq is named by the older spelling — takes this grant as well as the
setting, which is how it is decided: the same token is read off
the same list at logon and holds it beside the setting.

## Which orders the venue will take

Stated at logon, per security type, before a single order is sent.

```python
perms = client.order_permissions()           # 21 security types
len(perms["STK"])                            # 92 order types and attributes
client.permitted_order_types("FUT")          # or None, if the type is not permitted at all
```

Through the API a program discovers this by being refused. A security type
absent from this map is one the venue answers with an Inactive order and no
text at all, which is why this client refuses it first and says why.

## Which algorithms this account may use

Keyed by provider and security type, also stated at logon.

```python
client.algorithms()                          # 13 keys: 'IBALGO/STK', 'FOXRIVER/STK', …
client.algorithms_for("STK")                 # ['FOXRIVER-AE', 'IBALGO-AE', 'JONES-AE', …]
```

An algorithm absent here is one an order naming it is refused for.

## What the session itself is

```python
client.get_account_id()                      # the account this session acts for
client.next_order_id()                       # the first id it may use
client.ccp_session_id()                      # what a web endpoint expects as a header
client.misc_url("region_dam")                # hosts the venue pushed at logon
```

## How far away the venue is

```python
client.req_ping()
client.last_rtt_ms()                         # 10.96 on the session this was written from
```

The API has no notion of this. A program that wants to know whether it is
close to the venue has to time a request that does something else.

## Why this is not in the API

The API is a protocol between a gateway and a program on the same machine. The
gateway holds this to decide what to enable, what to grey out, and what to
refuse before sending; none of it was ever framed as a message to forward. A
client that speaks the venue's protocol receives it directly, so it is here.

Two consequences worth stating plainly:

- **None of this is derived.** Every figure above is stated by the venue,
  read off the session. Nothing here computes what the venue did not say.
- **It is not portable.** A program using these calls will not run against a
  gateway, because a gateway has no message to carry them. They are the part of
  this client that is not a drop-in, and they are named again under
  [Limits](./limits.md) for that reason.

## The same calls in Rust

| Python | Rust |
| --- | --- |
| `client.enabled_features()` | `client.enabled_features()` |
| `client.order_permissions()` | `client.order_permissions()` |
| `client.permitted_order_types(sec_type)` | `client.permitted_order_types(sec_type)` |
| `client.algorithms()` | `client.algorithms()` |
| `client.algorithms_for(sec_type)` | `client.algorithms_for(sec_type)` |
| `client.get_account_id()` | `client.account_id` (a field) |
| `client.next_order_id()` | `client.next_order_id()` |
| `client.ccp_session_id()` | `client.ccp_session_id()` |
| `client.misc_url(key)` | `client.misc_url(key)` |
| `client.req_ping()` | `client.req_ping()` |
| `client.last_rtt_ms()` | `client.shared_state().last_ccp_rtt()`, a `Duration` |
