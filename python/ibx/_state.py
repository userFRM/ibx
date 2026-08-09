"""What the session currently holds, kept current as it runs.

The wrapper this follows hands a caller live objects: ``ib.positions()`` is what
the account holds right now, and ``ib.trades()`` are orders whose status changes
under the caller as fills arrive. That is a different thing from a request that
answers once, and it is the part a program written against that wrapper leans on
most.

It works by keeping a pump running. The pump owns dispatch, and dispatch fires
the callbacks below, which record rather than act. A caller reading a list gets
a snapshot copied under a lock, so the list cannot change while it is being
read — a half-updated list is worse than a slightly old one.
"""

from __future__ import annotations

import threading
from dataclasses import dataclass, field
from typing import Any

from .ibx import EWrapper


@dataclass
class Position:
    account: str = ""
    contract: Any = None
    position: float = 0.0
    avgCost: float = 0.0


@dataclass
class PortfolioItem:
    contract: Any = None
    position: float = 0.0
    marketPrice: float = 0.0
    marketValue: float = 0.0
    averageCost: float = 0.0
    unrealizedPNL: float = 0.0
    realizedPNL: float = 0.0
    account: str = ""


@dataclass
class AccountValue:
    account: str = ""
    tag: str = ""
    value: str = ""
    currency: str = ""


@dataclass
class OrderStatus:
    orderId: int = 0
    status: str = ""
    filled: float = 0.0
    remaining: float = 0.0
    avgFillPrice: float = 0.0
    permId: int = 0
    parentId: int = 0
    lastFillPrice: float = 0.0
    clientId: int = 0
    whyHeld: str = ""
    mktCapPrice: float = 0.0


@dataclass
class Fill:
    contract: Any = None
    execution: Any = None
    commissionReport: Any = None
    time: str = ""


@dataclass
class Trade:
    """An order and what has become of it.

    The status changes under a caller holding this object, which is what makes
    it a trade rather than a receipt.
    """

    contract: Any = None
    order: Any = None
    orderStatus: OrderStatus = field(default_factory=OrderStatus)
    fills: list = field(default_factory=list)
    log: list = field(default_factory=list)

    def isActive(self) -> bool:
        return self.orderStatus.status in _ACTIVE

    def isDone(self) -> bool:
        return not self.isActive()

    def filled(self) -> float:
        return self.orderStatus.filled

    def remaining(self) -> float:
        return self.orderStatus.remaining


#: Statuses under which an order is still working. Taken from what the venue
#: sends rather than inferred: anything not named here has stopped.
_ACTIVE = frozenset({
    "PendingSubmit", "PendingCancel", "PreSubmitted", "Submitted", "ApiPending",
})


class LiveState(EWrapper):
    """Records what arrives instead of acting on it.

    Every accessor hands back a copy. A caller iterating a list while the pump
    appends to it would otherwise see the list change under the iteration.
    """

    def __init__(self) -> None:
        super().__init__()
        self._lock = threading.RLock()
        self._positions: dict[tuple[str, int], Position] = {}
        self._portfolio: dict[int, PortfolioItem] = {}
        self._account_values: dict[tuple[str, str, str], AccountValue] = {}
        self._trades: dict[int, Trade] = {}
        self._fills: list[Fill] = []
        self._accounts: list[str] = []
        self._tickers: dict[int, Ticker] = {}
        self._pending: set[int] = set()
        self._fields = None
        self.errors: list[tuple[int, int, str, str]] = []

    # -- what the venue tells us -----------------------------------------

    def error(self, reqId, errorCode, errorString, advancedOrderRejectJson=""):
        with self._lock:
            self.errors.append((reqId, errorCode, errorString, advancedOrderRejectJson))

    def managedAccounts(self, accountsList):
        with self._lock:
            self._accounts = [a for a in accountsList.split(",") if a]

    def position(self, account, contract, position, avgCost):
        with self._lock:
            key = (account, getattr(contract, "conId", 0))
            if position == 0:
                self._positions.pop(key, None)
            else:
                self._positions[key] = Position(account, contract, position, avgCost)

    def updatePortfolio(
        self, contract, position, marketPrice, marketValue,
        averageCost, unrealizedPNL, realizedPNL, accountName,
    ):
        with self._lock:
            self._portfolio[getattr(contract, "conId", 0)] = PortfolioItem(
                contract, position, marketPrice, marketValue,
                averageCost, unrealizedPNL, realizedPNL, accountName,
            )

    def updateAccountValue(self, key, val, currency, accountName):
        with self._lock:
            self._account_values[(accountName, key, currency)] = AccountValue(
                accountName, key, val, currency
            )

    def openOrder(self, orderId, contract, order, orderState):
        with self._lock:
            trade = self._trades.setdefault(orderId, Trade())
            trade.contract = contract
            trade.order = order
            trade.orderStatus.orderId = orderId

    def orderStatus(
        self, orderId, status, filled, remaining, avgFillPrice, permId,
        parentId, lastFillPrice, clientId, whyHeld, mktCapPrice=0.0,
    ):
        with self._lock:
            trade = self._trades.setdefault(orderId, Trade())
            before = trade.orderStatus.status
            trade.orderStatus = OrderStatus(
                orderId, status, filled, remaining, avgFillPrice, permId,
                parentId, lastFillPrice, clientId, whyHeld, mktCapPrice,
            )
            if status != before:
                trade.log.append(status)

    def execDetails(self, reqId, contract, execution):
        with self._lock:
            fill = Fill(contract, execution, None, getattr(execution, "time", ""))
            self._fills.append(fill)
            order_id = getattr(execution, "orderId", None)
            if order_id is not None and order_id in self._trades:
                self._trades[order_id].fills.append(fill)

    def commissionAndFeesReport(self, report):
        """Attach the cost to the fill it belongs to.

        It arrives separately from the fill and names it by execution id. A fill
        whose cost never arrived keeps ``None`` rather than a zero, because a
        zero commission and an unknown one are different facts.
        """
        exec_id = getattr(report, "execId", None)
        if exec_id is None:
            return
        with self._lock:
            for fill in reversed(self._fills):
                if getattr(fill.execution, "execId", None) == exec_id:
                    fill.commissionReport = report
                    return

    # -- quotes ----------------------------------------------------------

    def _ticker(self, req_id: int) -> Ticker:
        t = self._tickers.get(req_id)
        if t is None:
            t = Ticker()
            self._tickers[req_id] = t
        return t

    def _apply(self, req_id, tick_type, value):
        if self._fields is None:
            self._fields = _tick_fields()
        pair = self._fields.get(tick_type)
        if pair is None:
            # A tick this does not carry is not an error and is not recorded as
            # one. It reaches a caller through the callback either way.
            return
        name, prev_name = pair
        with self._lock:
            t = self._ticker(req_id)
            if prev_name is not None:
                setattr(t, prev_name, getattr(t, name))
            setattr(t, name, value)
            self._pending.add(req_id)

    def tickPrice(self, reqId, tickType, price, attrib=None):
        self._apply(reqId, tickType, price)

    def tickSize(self, reqId, tickType, size):
        self._apply(reqId, tickType, size)

    def tickString(self, reqId, tickType, value):
        from .ibx import TickTypeEnum as T

        if tickType == T.LAST_TIMESTAMP:
            with self._lock:
                self._ticker(reqId).time = value
                self._pending.add(reqId)

    def bind_ticker(self, req_id: int, contract) -> Ticker:
        """Name the contract a request's quote belongs to."""
        with self._lock:
            t = self._ticker(req_id)
            t.contract = contract
            return t

    def snapshot_tickers(self) -> list[Ticker]:
        with self._lock:
            return list(self._tickers.values())

    def ticker_for(self, req_id: int) -> Ticker | None:
        with self._lock:
            return self._tickers.get(req_id)

    def take_pending(self) -> list[Ticker]:
        """The quotes that changed since this was last read, and clear the mark.

        Reading is what clears it, so two readers do not each see half the
        changes. A caller wanting every change should be the only one reading.
        """
        with self._lock:
            out = [self._tickers[r] for r in self._pending if r in self._tickers]
            self._pending.clear()
            return out

    # -- what a caller reads ---------------------------------------------

    def snapshot_positions(self) -> list[Position]:
        with self._lock:
            return list(self._positions.values())

    def snapshot_portfolio(self) -> list[PortfolioItem]:
        with self._lock:
            return list(self._portfolio.values())

    def snapshot_account_values(self) -> list[AccountValue]:
        with self._lock:
            return list(self._account_values.values())

    def snapshot_trades(self) -> list[Trade]:
        with self._lock:
            return list(self._trades.values())

    def snapshot_fills(self) -> list[Fill]:
        with self._lock:
            return list(self._fills)

    def snapshot_accounts(self) -> list[str]:
        with self._lock:
            return list(self._accounts)


@dataclass
class Ticker:
    """A contract's current quote, accumulated from the ticks that make it up.

    A quote does not arrive as a quote. It arrives as a bid, then a size, then a
    trade, each on its own, and a caller wanting "the current market" has to
    hold them together. This does that.

    A field nobody has sent stays ``None`` rather than becoming zero. A bid of
    zero and no bid at all are different markets, and the difference decides
    whether an order should be sent.
    """

    contract: Any = None
    time: str = ""
    bid: float | None = None
    bidSize: float | None = None
    ask: float | None = None
    askSize: float | None = None
    last: float | None = None
    lastSize: float | None = None
    prevBid: float | None = None
    prevAsk: float | None = None
    prevLast: float | None = None
    volume: float | None = None
    open: float | None = None
    high: float | None = None
    low: float | None = None
    close: float | None = None
    halted: float | None = None

    def hasBidAsk(self) -> bool:
        return self.bid is not None and self.ask is not None

    def midpoint(self) -> float | None:
        """Half way between the two sides, or nothing if there are not two."""
        if not self.hasBidAsk():
            return None
        return (self.bid + self.ask) / 2

    def marketPrice(self) -> float | None:
        """The price to value a holding at.

        The last trade when it sits inside the current spread, the midpoint when
        it does not. A last trade outside the spread is stale — the market moved
        away from it — and valuing at it is how a position is marked to a price
        nobody would deal at.
        """
        mid = self.midpoint()
        if self.last is not None and (mid is None or self.bid <= self.last <= self.ask):
            return self.last
        if mid is not None:
            return mid
        return self.close


#: Which tick number sets which field, and whether the previous value is kept.
#: Taken from the enum this client publishes rather than written out again, so
#: the two cannot drift apart.
def _tick_fields():
    from .ibx import TickTypeEnum as T

    return {
        T.BID: ("bid", "prevBid"),
        T.ASK: ("ask", "prevAsk"),
        T.LAST: ("last", "prevLast"),
        T.BID_SIZE: ("bidSize", None),
        T.ASK_SIZE: ("askSize", None),
        T.LAST_SIZE: ("lastSize", None),
        T.VOLUME: ("volume", None),
        T.OPEN: ("open", None),
        T.HIGH: ("high", None),
        T.LOW: ("low", None),
        T.CLOSE: ("close", None),
        T.HALTED: ("halted", None),
    }
