# Wire coverage

*Auto-generated from source — do not edit.*

Which messages this client speaks. A claim to replace the vendor's
gateway is a claim about messages rather than about method names, so
this is taken from the dispatch tables and the message builders
themselves, and CI regenerates it and fails if it has drifted.

Test code is excluded: a fixture that builds a message is not this
client sending one.

What this does **not** establish: a type absent here is one this client
neither sends nor handles, which is not the same as one the venue never
sends. That comparison needs the vendor's own inventory.

## Sent

### Message types

| Type | Meaning |
| --- | --- |
| `0` | Heartbeat |
| `1` | Test request |
| `A` | Logon |
| `D` | New order |
| `F` | Order cancel |
| `G` | Order replace |
| `H` | Order mass status request |
| `U` | User message |
| `V` | Market data request |
| `W` | Chart request |
| `Z` | Chart cancel |
| `c` | Security definition request |

### User-message subtypes

A user message carries what it is for on tag 6040.

| Subtype |
| --- |
| `6` |
| `72` |
| `74` |
| `76` |
| `80` |
| `91` |
| `101` |
| `112` |
| `138` |
| `142` |
| `185` |
| `193` |
| `209` |
| `10003` |
| `10004` |
| `10010` |
| `10030` |

## Handled

### During the logon exchange, before the dispatch tables run

| Type | Meaning |
| --- | --- |
| `3` | Session reject |
| `A` | Logon |
| `U` | User message |

### On the trading connection

| Type | Meaning |
| --- | --- |
| `0` | Heartbeat |
| `1` | Test request |
| `3` | Session reject |
| `8` | Execution report |
| `9` | Cancel reject |
| `B` | News |
| `G` | Order replace |
| `U` | User message |
| `W` | Chart request |
| `d` | Security definition |
| `RL` | Account update |
| `UM` | Account update |
| `UP` | Position update |
| `UT` | Account update |

### User-message subtypes on the trading connection

| Subtype |
| --- |
| `75` |
| `77` |
| `81` |
| `102` |
| `107` |
| `139` |
| `143` |
| `152` |
| `186` |
| `210` |

### On the market data connection

| Type | Meaning |
| --- | --- |
| `0` | Heartbeat |
| `1` | Test request |
| `G` | Tick payload, binary |
| `L` | Ticker setup |
| `P` | Tick |
| `Q` | Subscription ack |
| `Y` | Subscription reject |
| `RL` | Account update |
| `UM` | Account update |
| `UP` | Position update |
| `UT` | Account update |

### On the historical connection

| Type | Meaning |
| --- | --- |
| `0` | Heartbeat |
| `1` | Test request |
| `E` | Historical payload |
| `G` | Bar payload |
| `U` | User message |
| `W` | Chart response |

### User-message subtypes on the historical connection

| Subtype |
| --- |
| `10002` |
| `10005` |
| `10012` |
| `10022` |
| `10032` |

