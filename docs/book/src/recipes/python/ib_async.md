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

The connect call, and nothing else:

```diff
- ib.connect("127.0.0.1", 4001, clientId=1)     # requires a running gateway
+ ib.connect(username="...", password="...")    # no external process
```

`IB.connect` takes a host, a port and a client id because it was written for a
gateway. They are accepted and ignored. Credentials go to `attach`, or are left
to `IB_USERNAME` and `IB_PASSWORD`.

## What is carried

Their `IB` calls 67 methods on the transport layer. All 67 are carried here,
and a test measures that against their own source rather than against a list
kept by hand, so a name they add is a failure here rather than a program that
stops.

Their types are theirs. A callback carrying an object hands over one of this
engine's, and their wrapper reads one of theirs, so every argument is rebuilt
on the way through — by its own type name, from their dataclass, which is why a
field they have and this engine does not keeps its default and neither side
needs editing when the other gains one.

Two details are worth knowing, because both were defects until they were not:

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

Two of their three tests pass. The third, `test_request_error_raised`, cannot
pass against any server: it requires a `RequestError` carrying 321, and 321 is
in their own `warningCodes`, where a warning never ends the request it belongs
to.

## What it does not carry

`ib_async` reads a Flex report over the web, which has nothing to do with a
session and is unaffected. Everything else in their `IB` is routed.
