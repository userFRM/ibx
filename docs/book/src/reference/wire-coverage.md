# Wire coverage

*Auto-generated from source — do not edit.*

Which messages this client speaks. A claim to replace the vendor's
gateway is a claim about messages rather than about method names, so
this is taken from the dispatch tables and the message builders
themselves and CI checks it against them.

A message type absent here is one this client neither sends nor
handles. That is not the same as one the venue never sends.

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
| `U` | User message |
| `V` | Market data request |
| `W` | Chart request |
| `Z` | Chart cancel |
| `c` | Security definition request |
| `d` | Security definition |

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
| `139` |
| `142` |
| `185` |
| `186` |
| `193` |
| `209` |
| `10003` |
| `10004` |
| `10010` |
| `10030` |

## Handled

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

### User-message subtypes handled

| Subtype |
| --- |
| `75` |
| `77` |
| `102` |
| `107` |
| `139` |
| `143` |
| `152` |
| `186` |

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
| `UM` | not named here |
| `UP` | Position update |
| `UT` | not named here |

