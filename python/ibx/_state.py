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
