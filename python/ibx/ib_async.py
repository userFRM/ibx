"""Run an unmodified `ib_async` program without a gateway.

`ib_async` is layered: `IB`, `Wrapper`, `Ticker`, `Trade` and everything above
them are transport-agnostic, and only its `Client`/`Connection` know there is a
socket to a gateway on localhost. This replaces that one layer with this
engine. Everything above it — the events, the async variants, the notebooks —
runs unchanged, from the copy of `ib_async` already installed.

Nothing of theirs is copied or modified: install `ib_async` as usual, and
attach.

    from ib_async import IB, Stock
    import ibx.ib_async

    ib = ibx.ib_async.attach(IB(), username="…", password="…")
    ib.connect()                      # names no host: there is no gateway

    spy = Stock("SPY", "SMART", "USD")
    ib.qualifyContracts(spy)
    print(ib.reqHistoricalData(spy, "", "2 D", "1 hour", "TRADES", useRTH=True))

`IB.connect` takes a host, a port and a client id because it was written for a
gateway. They are accepted and ignored. The credentials are given to `attach`,
or left to `IB_USERNAME` and `IB_PASSWORD`.
"""

import asyncio
import os
import threading
import time

from eventkit import Event

import ibx as _ibx


class IbxClient:
    """What `ib_async.IB` talks to, answered by this engine.

    Holds the same attributes and events `ib_async.Client` does, because `IB`
    reads them directly.
    """

    DISCONNECTED, CONNECTING, CONNECTED = range(3)
    MinClientVersion = 157
    MaxClientVersion = 178

    def __init__(self, wrapper, username="", password="", paper=True):
        self.wrapper = wrapper
        self._username = username or os.environ.get("IB_USERNAME", "")
        self._password = password or os.environ.get("IB_PASSWORD", "")
        self._paper = paper
        self.apiStart = Event("apiStart")
        self.apiEnd = Event("apiEnd")
        self.apiError = Event("apiError")
        self.throttleStart = Event("throttleStart")
        self.throttleEnd = Event("throttleEnd")

        self.host = ""
        self.port = -1
        self.clientId = -1
        self.optCapab = ""
        self.connectOptions = b""
        self.connState = IbxClient.DISCONNECTED
        self._reqIdSeq = 1
        self._accounts: list[str] = []
        self._loop = None
        self._pump = None
        self._stop = threading.Event()
        self._since = time.time()

        # The engine, with ib_async's own wrapper as the callback target: this
        # client already resolves a callback under the reference client's
        # spelling, which is the spelling ib_async's wrapper uses.
        self._client = _ibx.EClient(_LoopBound(wrapper))

    # ── connection ──

    async def connectAsync(self, host, port, clientId, timeout=2.0):
        """Open the session. Host and port name a gateway; there is none."""
        self.host, self.port, self.clientId = host, int(port), int(clientId)
        self.connState = IbxClient.CONNECTING
        self._loop = asyncio.get_running_loop()
        self.wrapper.__dict__.setdefault("clientId", self.clientId)

        # Blocking, so it runs off the loop: everything else here has to stay
        # able to answer while the session opens.
        await self._loop.run_in_executor(
            None,
            lambda: self._client.connect(
                username=self._username,
                password=self._password,
                paper=self._paper,
                client_id=self.clientId,
            ),
        )
        self.connState = IbxClient.CONNECTED

        # What the handshake tells ib_async before it considers the API ready.
        self._accounts = [
            a for a in self._client.get_account_id().split(",") if a
        ]
        self.wrapper.managedAccounts(",".join(self._accounts))
        self.wrapper.nextValidId(self._client.next_order_id())

        self._start_pump()
        self.apiStart.emit()

    def disconnect(self):
        self.connState = IbxClient.DISCONNECTED
        self._stop.set()
        self._client.disconnect()
        self.wrapper.connectionClosed()
        self.apiEnd.emit()

    def _start_pump(self):
        """Drive dispatch, and land every callback on ib_async's own loop.

        ib_async is asyncio end to end: its futures are resolved by wrapper
        callbacks and must be touched from the loop thread.
        """
        self._stop.clear()

        def pass_once():
            """One dispatch, then the boundary ib_async flushes on.

            Their wrapper holds ticker updates until the batch of messages
            ends, and emits their events there. Their own transport calls this
            after each socket read; here it is after each pass of dispatch.
            """
            arrived = getattr(self.wrapper, "tcpDataArrived", None)
            if arrived:
                arrived()
            self._client.poll()
            processed = getattr(self.wrapper, "tcpDataProcessed", None)
            if processed:
                processed()

        def run():
            while not self._stop.is_set():
                self._loop.call_soon_threadsafe(pass_once)
                self._stop.wait(0.01)

        self._pump = threading.Thread(target=run, daemon=True)
        self._pump.start()

    # ── what IB reads directly ──

    def isConnected(self):
        return self.connState == IbxClient.CONNECTED

    def isReady(self):
        return self.isConnected()

    def serverVersion(self):
        return self.MaxClientVersion

    def getAccounts(self):
        return list(self._accounts)

    def getReqId(self):
        self._reqIdSeq += 1
        return self._reqIdSeq

    def updateReqId(self, minReqId):
        self._reqIdSeq = max(self._reqIdSeq, minReqId)

    def connectionStats(self):
        from ib_async.objects import ConnectionStats

        return ConnectionStats(self._since, time.time() - self._since, 0, 0, 0, 0)

    def setConnectOptions(self, options):
        self.connectOptions = options.encode()

    # ── requests: the same shape, all the way down ──

    def reqContractDetails(self, reqId, contract):
        self._client.req_contract_details(reqId, _contract(contract))

    def reqMktData(self, reqId, contract, genericTickList, snapshot,
                   regulatorySnapshot, mktDataOptions):
        self._client.req_mkt_data(
            reqId, _contract(contract), genericTickList, snapshot,
            regulatorySnapshot,
        )

    def cancelMktData(self, reqId):
        self._client.cancel_mkt_data(reqId)

    def reqPositions(self):
        self._client.req_positions()

    def reqHistoricalData(self, reqId, contract, endDateTime, durationStr,
                          barSizeSetting, whatToShow, useRTH, formatDate,
                          keepUpToDate, chartOptions):
        self._client.req_historical_data(
            reqId, _contract(contract), endDateTime, durationStr,
            barSizeSetting, whatToShow, 1 if useRTH else 0, formatDate,
            keepUpToDate, [],
        )

    def reqAccountUpdates(self, subscribe, acctCode):
        self._client.req_account_updates(subscribe, acctCode)

    def reqOpenOrders(self):
        self._client.req_open_orders()

    def reqAutoOpenOrders(self, autoBind):
        self._client.req_auto_open_orders(autoBind)

    def reqCompletedOrders(self, apiOnly):
        self._client.req_completed_orders(apiOnly)

    def reqAccountSummary(self, reqId, groupName, tags):
        self._client.req_account_summary(reqId, groupName, tags)

    def reqMarketDataType(self, marketDataType):
        self._client.req_market_data_type(marketDataType)

    def reqCurrentTime(self):
        self._client.req_current_time()

    def __getattr__(self, name):
        """Every other request, under the name this engine carries it by.

        Both sides follow the reference client's own signatures, so a request
        with no special handling above is forwarded as it stands rather than
        written out again here. One that this engine does not carry says so,
        naming itself, instead of failing as a missing attribute.
        """
        if name.startswith("_"):
            raise AttributeError(name)

        ours = "".join("_" + c.lower() if c.isupper() else c for c in name)
        carried = getattr(self._client, ours, None)
        if carried is None:
            def missing(*args, **kwargs):
                raise NotImplementedError(
                    f"{name}() is not carried by this client"
                )
            return missing

        def forwarding(*args):
            return carried(*(_as_ours(a) for a in args))

        return forwarding


def _their_type(name):
    """The type of theirs that goes by this name, if there is one."""
    import dataclasses

    import ib_async.contract as contract_types
    import ib_async.objects as objects
    import ib_async.order as order_types

    for module in (contract_types, objects, order_types):
        found = getattr(module, name, None)
        if found is not None and dataclasses.is_dataclass(found):
            return found
    return None


def _as_theirs(value):
    """One of ours, rebuilt as the same-named type of theirs.

    Both sides carry the reference client's own field names, so the conversion
    is driven by their dataclass rather than written out per type or per
    callback: a field they have and we do not keeps its default, and neither
    side needs editing when the other gains one. Anything with no counterpart
    of theirs — a number, a string, a type only we have — is handed over as it
    is.
    """
    import dataclasses

    if isinstance(value, (str, bytes, int, float, bool, type(None))):
        return value
    if isinstance(value, (list, tuple)):
        return type(value)(_as_theirs(v) for v in value)

    theirs = _their_type(type(value).__name__)
    if theirs is None or dataclasses.is_dataclass(value):
        return value

    made = theirs()
    for field in dataclasses.fields(theirs):
        ours = getattr(value, field.name, None)
        if ours is None:
            continue
        try:
            setattr(made, field.name, _as_theirs(ours))
        except (TypeError, ValueError, AttributeError):
            pass
    return made


class _LoopBound:
    """ib_async's wrapper, reached under the names this engine calls.

    A callback carrying an object hands over one of ours; their wrapper reads
    one of theirs. Every argument is rebuilt on the way through, by its own
    type name — so a callback nobody thought to list is carried too.
    """

    #: The size that goes with each price, by tick type: bid, ask, last, and
    #: the same three delayed.
    _SIZE_OF = {1: 0, 2: 3, 4: 5, 66: 69, 67: 70, 68: 71}
    _PRICE_OF = {size: price for price, size in _SIZE_OF.items()}

    def __init__(self, wrapper):
        self._wrapper = wrapper
        self._quotes: dict[int, dict[int, float]] = {}

    def tick_price(self, req_id, tick_type, price, attrib=None):
        """A price, delivered with the size that belongs to it.

        This engine states a price and a size as the reference client does —
        two ticks. ib_async holds a quote as one thing and blanks a side whose
        size is zero, so the two are paired here rather than each arrival
        wiping the other half.
        """
        held = self._quotes.setdefault(req_id, {})
        held[tick_type] = price
        size = held.get(self._SIZE_OF.get(tick_type, -1), 0.0)
        self._wrapper.priceSizeTick(req_id, tick_type, price, size)

    def tick_size(self, req_id, tick_type, size):
        held = self._quotes.setdefault(req_id, {})
        held[tick_type] = size
        price_type = self._PRICE_OF.get(tick_type)
        if price_type is None:
            self._wrapper.tickSize(req_id, tick_type, size)
            return
        self._wrapper.priceSizeTick(
            req_id, price_type, held.get(price_type, 0.0), size
        )

    def __getattr__(self, name):
        # Under either spelling: this engine calls a callback by the name it
        # holds it under, and their wrapper declares the reference client's.
        method = getattr(self._wrapper, name, None)
        if method is None:
            words = name.split("_")
            camel = words[0] + "".join(w.title() for w in words[1:])
            method = getattr(self._wrapper, camel, None)
        if method is None:
            raise AttributeError(name)

        def carrying(*args):
            return method(*(_as_theirs(a) for a in args))

        return carrying


def _as_ours(value):
    """One of theirs, rebuilt as this engine's own type of the same name."""
    import dataclasses

    if not dataclasses.is_dataclass(value):
        return value
    ours = getattr(_ibx, type(value).__name__, None)
    if ours is None:
        return value
    made = ours()
    for field in dataclasses.fields(value):
        held = getattr(value, field.name, None)
        if held is None:
            continue
        try:
            setattr(made, field.name, _as_ours(held))
        except (AttributeError, TypeError, ValueError):
            pass
    return made


def _contract(contract):
    """An ib_async contract, as this engine's own."""
    made = _ibx.Contract()
    for field in ("conId", "symbol", "secType", "exchange", "currency",
                  "lastTradeDateOrContractMonth", "strike", "right",
                  "multiplier", "localSymbol", "primaryExchange",
                  "tradingClass"):
        value = getattr(contract, field, None)
        if value:
            setattr(made, field, value)
    return made


def attach(ib, username="", password="", paper=True):
    """Point an `ib_async.IB` at this engine, and hand it back.

    The credentials are this session's; left out, `IB_USERNAME` and
    `IB_PASSWORD` are used.
    """
    ib.client = IbxClient(ib.wrapper, username, password, paper)
    ib.wrapper.client = ib.client
    return ib
