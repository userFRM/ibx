# An existing ib_async program

`ib_async` is layered. `IB`, `Wrapper`, `Ticker`, `Trade` and everything above
them are transport-agnostic; only its `Client` and `Connection` know there is a
socket to a gateway on localhost. Replacing that one layer runs the rest of the
library unchanged.

Nothing of `ib_async` is copied or modified. Install it as usual, and attach.

```python
from ib_async import IB, Stock
import ibx.ib_async

ib = ibx.ib_async.attach(IB(), username="your_user", password="your_pass")
ib.connect()                      # names no host: there is no gateway

spy = Stock("SPY", "SMART", "USD")
ib.qualifyContracts(spy)
bars = ib.reqHistoricalData(spy, "", "2 D", "1 hour", "TRADES", useRTH=True)

ib.pendingTickersEvent += lambda tickers: print(len(tickers), "updates")
ib.reqMktData(spy)
ib.sleep(5)
ib.disconnect()
```

Their `IB`, their `Wrapper`, their events, their types — this engine
underneath, and no gateway process.

## What changes

One line, and it is not the connect call:

```diff
+ ib = ibx.ib_async.attach(ib, username="...", password="...")
- ib.connect("127.0.0.1", 4001, clientId=1)     # requires a running gateway
+ ib.connect()                                  # no external process
```

`IB.connect` stays theirs, with their signature. It takes a host, a port and a
client id because it was written for a gateway: the host and port are recorded
and never used, and the client id is carried into the login. Credentials never
go through it. They go to `attach`, or are left to `IB_USERNAME` and
`IB_PASSWORD`.

## What is carried

Their `IB` calls 67 methods on the transport layer, counted from their own
source at 2.1.0. All of them are carried here. A test reads that list out of
their source on every run rather than checking a list kept by hand, so a name
they add is a failure here rather than a program that stops.

Their types are theirs. A callback carrying an object hands over one of this
engine's, and their wrapper reads one of theirs, so every argument is rebuilt
on the way through — by its own type name, from their dataclass, which is why a
field they have and this engine does not keeps its default and neither side
needs editing when the other gains one.

Two details are worth knowing:

- A bar's date is handed over in the spelling their own parser reads. Their
  `parseIBDatetime` decides the shape from the string — eight digits is a day,
  a date and a time and a zone separated by single spaces is an aware moment —
  and their frame conversion, which is the first example in their
  documentation, refuses a naive one.
- `disconnect()` does not call their `connectionClosed`. Their wrapper treats
  that as a session that went away underneath them: it fails every request
  still waiting and raises on their global error event. That is right for a
  socket that dropped and wrong for a caller who asked to stop.

## Their own test suite

The strongest available statement about whether their library runs here is
their own tests. They are not vendored; point a run at a checkout of theirs:

```bash
git clone https://github.com/ib-api-reloaded/ib_async /tmp/ib_async
cp tests/ib_async_upstream/conftest.py /tmp/ib_async/tests/
IB_USERNAME=… IB_PASSWORD=… pytest /tmp/ib_async/tests \
    -o asyncio_mode=auto \
    -o asyncio_default_fixture_loop_scope=session \
    -o asyncio_default_test_loop_scope=session
```

Both loop scopes are needed: their session-scoped connection fixture and their
tests must share one event loop, or callbacks land on a loop that is not
running while the test waits on them.

At 2.1.0 their suite is three tests. One of them,
`test_request_error_raised`, cannot pass against any server. Its last line
asserts a `RequestError` carrying 321, and 321 is in their own `warningCodes`
frozenset, where a warning never ends the request it belongs to. So the error
they are waiting for is never raised.

## What it does not carry

`FlexReport` reads a report over the web. It is a class of its own, not a method
on their `IB`, and it never touches a session. Everything on their `IB` is
routed.
