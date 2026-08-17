# Changelog

What changed, and what it means for a program written against this client.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- **`Client`**, a session that keeps what it is told. A position, an order, a
  fill and a quote are read rather than asked for: the account, its holdings
  and anything already working are subscribed to as the session opens, so they
  are there the moment `connect` returns. `EClient` remains the reference
  client's own shape, for a program being moved onto this one.
- **`AsyncClient`**, the same session for a program already running a runtime.
  A question goes onto a thread that may wait; reading what the session holds
  is not awaited, because it is a lock and a copy. Every question has the same
  name and the same answer on both surfaces, and a test fails if either grows a
  name the other does not have.
- **`PlacedOrder`**, returned by `place`. It knows its own number, so that is
  not bookkeeping a caller keeps, and it answers about itself as of the moment
  it is asked: `status`, `is_done`, `fills`, `wait_done`, `cancel`.
- **Streams for what happens as it happens** — `ticks` on a contract,
  `order_events`, `live_bar_stream` for five-second bars, `news_stream` for
  headlines. Both are iterators, read the way
  anything else in Rust is read. `ticks` subscribes and hands back the stream
  in one, carries only that contract's, and withdraws the subscription when the
  stream is dropped — the same for bars. Bars and headlines are kept as well as
  streamed, so a caller who subscribed and then looked rather than iterating
  still finds what arrived.
- **What the session holds, by name**: `holdings` priced rather than at cost,
  `pnl`, `bulletins`, `live_bars`, `news`, and `quotes` for one price each
  across several contracts at once.
- **Questions that were assemblies**: `scan`, `schedule`, `calendar_schema` and
  `calendar_events`. Each is one call, and each withdraws what it started — a
  scan left running keeps answering into a session nobody is reading.
- **`place_bracket`**, an entry and the two exits that close it, sent as the one
  instruction the engine has for it. The venue links them, so neither child
  reaches the market before the parent has a position to work against. Refused
  before it is sent if an exit sits the wrong side of the entry.
- Shorthand for what a caller actually names: `Contract::{stock, call, put,
  future, forex, index, by_id}` and `Order::{market, limit, stop, stop_limit,
  trailing_stop}`. They state what the kind of instrument requires and leave
  what identifies one listing from another — a currency on a future, a
  multiplier on an option — for the venue to answer with.
- An `async` feature, off by default, so a program driving this from a thread
  of its own pays for no runtime.

### Fixed

- **A Python caller's order reached the venue missing most of what was set on
  it.** The conversion into the engine ended in a struct-update fallback and a
  hundred and ten of the hundred and fifty-four fields fell into it — a
  volatility, a hedge, a short-sale slot, a settling firm, a clearing account, a
  scale table, a soft-dollar tier — with no error and nothing in the log. Both
  conversions state every field now and neither has a fallback, so a field added
  to either side fails to compile rather than being defaulted.
- **A delayed order traded immediately.** `good_after_time` is carried on tag
  168, as a timestamp in UTC joined by a dash — not the space-joined form the
  rest of this client writes, which is why an earlier attempt at it never read
  back.
- **Midpoint bars have never worked.** The venue knows the series as `MidPoint`
  and this asked for `Midpoint`, so it answered "no historical market data",
  which reads as a series that does not exist rather than a name it does not
  know. A currency pair has no trades at all, so the midpoint is the only bar it
  has, and `bars` asks for it there.
- **Orders that could not be placed as asked are refused rather than altered.**
  An unrecognised time in force became a DAY order that died at the close; an
  unreadable expiry was dropped so the order stood with none; a hedge struck at
  an unreadable number went out hedged against nothing; a combination leg with
  an unrecognised side was sent as a buy; a negative quantity was clamped to
  none; a value above what its field holds arrived as something else; half a
  soft-dollar arrangement sent neither half.
- **Two questions asked at once consumed each other's answers.** A question
  drives the message pump, and the pump hands what it drains to whichever
  collector is running — so the second waited out its timeout for a reply that
  had already arrived. Questions take a turn now, held across the sending as
  well as the waiting.
- A Python object this client cannot read is refused rather than emptied: an
  unreadable leg price would have gone out as no price at all, and an unknown
  order condition was dropped so the order went live at once.

### Changed

- `Client`, `AsyncClient` and `Config` replace `IB`, `AsyncIB`, `EClientConfig`
  on the friendly surface, and the alias of `Client` for `EClient` is gone.
  Four names, each meaning one thing. Python's session is `Client` too, with
  `IB` kept as an alias for a program that looks for that name.
- **One client, not two.** The session dereferences to the client it is built
  on, so every request the protocol carries is reachable on it — nothing to
  import and nothing to choose between. Twelve names exist on both and the
  session's wins, because in each case it is the better answer; a test names
  those twelve so a thirteenth cannot appear unnoticed.
- What an order carries is stated as three numbers rather than two: 154 fields,
  of which 114 are sent, 35 have no field in the protocol to carry them and say
  so on themselves, and 5 are what the venue fills on the way back.

### Removed

- The event channel from the recommended path. A queue has one reader, it is
  told what it drains and nothing else, and a bounded one drops what arrives
  while it is full. `connect_with_events` remains for a program that would
  rather own a queue, with what it discarded readable.
- A dependency with no caller anywhere in the library, the tests, the examples
  or the build script.

### Known

- A holding in a future arrives carrying the venue's id and nothing else, and
  the definition service will not answer a query keyed on that id for a future
  though it answers one for a share. The id and the quantity are what is known.
- A preview states the order's type as one byte and eleven types have one; the
  rest are previewed as a limit at the same price, so the margin comes back for
  a limit rather than for the order asked about.
- An order read back from the venue and placed again from Python goes out
  without its conditions and without a price per leg: the engine holds what
  they meant, not the objects they were read from.
