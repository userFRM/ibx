"""The shape of the widely used asynchronous wrapper, over this client.

Method names, argument names and return types follow that wrapper so a program
written against it runs here. Where it returns a list, this returns a list; a
method it spells ``reqHistoricalData`` is spelled that here too.

This is a facade over ``EClient``, not a second client. It holds one session and
answers from it. What it cannot yet answer it says so plainly rather than
returning an empty list, because an empty list is indistinguishable from a
market with nothing in it.
"""

from __future__ import annotations

import threading
import time

from ._state import BracketOrder, HistoricalNews, HistoricalSchedule, LiveState, TradingSession
from .ibx import Contract, EClient


def _refuse_options(named: str, given) -> None:
    """A free-form option list this request cannot carry.

    Every one of these was accepted and dropped on the floor, so a caller who
    tuned a request with one was answered by an untuned request and had no way
    to tell. Empty or absent is what every ordinary call passes, and that is
    taken; anything in it is said out loud.
    """
    if given:
        raise NotImplementedError(
            f"{named}={given!r} is not carried here: this request has no "
            "free-form option list to send it under, so the request would go "
            "out without it and answer something other than what was asked"
        )


def _refuse_regulatory_snapshot(asked: bool) -> None:
    """A regulatory snapshot is a different request, and a chargeable one.

    It was accepted and dropped, so the caller was billed nothing and given an
    ordinary subscription instead of the single NBBO snapshot they asked for.
    Refused by name: a request this cannot make is worth more said out loud
    than answered with something else.
    """
    if asked:
        raise NotImplementedError(
            "regulatorySnapshot is not carried here: it is a separate, "
            "chargeable one-shot request, and answering it with an ordinary "
            "subscription would be a different request than the one asked for. "
            "Use snapshot=True for the free snapshot this does carry"
        )


class Client:
    """One session, asked questions directly."""

    def __init__(self) -> None:
        self.wrapper = LiveState()
        self.client = EClient(self.wrapper)
        self._pump: threading.Thread | None = None
        self._stop = threading.Event()
        self._subscribed: dict[int, object] = {}
        # Keyed by the kind of stream as well as the contract. One contract can
        # carry a quote, a book, bars and a tick stream at once, and one slot
        # per contract meant the newest request overwrote the rest: cancelling
        # any of them sent the wrong kind of cancel under another request's id,
        # withdrew a subscription the caller still wanted, and left the one
        # they asked to stop running at the venue.
        self._by_contract: dict[tuple[str, int], int] = {}
        # Which request id each P&L subscription was made under, so a cancel
        # names the one it was asked about.
        self._pnl_reqs: dict[tuple[str, str], int] = {}
        self._pnl_single_reqs: dict[tuple[str, str, int], int] = {}
        # Which id the calendar was asked for under, so a cancel names it.
        self._wsh_meta = 0
        self._wsh_event = 0
        self._req_id = 0
        # What a wait with no timeout of its own waits for. `setTimeout` names
        # another.
        self._timeout = 60

    def _next_req_id(self) -> int:
        self._req_id += 1
        return self._req_id

    # -- session ---------------------------------------------------------

    def connect(
        self,
        host="",
        port=0,
        clientId=1,
        timeout=4,
        readonly=False,
        account="",
        username="",
        password="",
        paper=True,
        settings=None,
    ):
        """Open the session.

        The host and port name a local process in the wrapper this follows.
        There is no local process here — this client logs in itself — so they
        are accepted and ignored, and the credentials are given here instead.
        A program that already holds a session needs no edit; one that relied
        on a running gateway to have logged in supplies the login here.

        ``readonly`` is carried through: a read-only session refuses to send
        anything that changes a position, and says so rather than appearing to
        have sent it.

        ``settings`` is what this session runs under, by the names
        :func:`ibx.configure` uses — ``{"timezone": "Europe/Zurich"}``. Stated
        here they belong to this session; stated through ``configure`` they
        belong to the process, and are what a session that states none falls
        back to.
        """
        del host, port, timeout, account
        self.client.connect(
            client_id=clientId,
            username=username,
            password=password,
            paper=paper,
            readonly=readonly,
            settings={k: str(v) for k, v in (settings or {}).items()},
        )
        self._start_pump()
        # The wrapper this follows subscribes to the account as it connects,
        # and a program reads `accountValues()` straight afterwards. Without
        # this the account is silent until something asks, and an empty list
        # reads as an account holding nothing.
        self.client.req_account_updates(True, "")
        return self

    def _start_pump(self) -> None:
        """Keep dispatch running so the live state stays current.

        A daemon thread, so it never holds a program open. It owns dispatch;
        the calls that answer take their replies by request id and are left
        alone by it, so the two run together rather than competing.
        """
        if self._pump is not None and self._pump.is_alive():
            return
        self._stop.clear()

        def pump():
            try:
                # Loops until the session closes, releasing the interpreter
                # lock while it waits, so it costs a thread and not a core.
                self.client.run()
            except Exception:
                # A session that has gone ends the pump. connect() starts a new
                # one; nothing here tries to reconnect behind the caller's back.
                return

        self._pump = threading.Thread(target=pump, name="ibx-pump", daemon=True)
        self._pump.start()

    def disconnect(self) -> None:
        self._stop.set()
        self.client.disconnect()
        if self._pump is not None:
            self._pump.join(timeout=2.0)
            self._pump = None

    # -- what the session currently holds --------------------------------

    def positions(self):
        return self.wrapper.snapshot_positions()

    def portfolio(self):
        return self.wrapper.snapshot_portfolio()

    def accountValues(self):
        """Every figure the venue states about the account.

        The same figures :meth:`accountSummary` hands back, and waited for the
        same way. The subscription is made as the session opens and the answer
        lands a moment later, so a program that connects and reads in the same
        breath — which is how one is written — was handed an empty list and
        nothing to say whether the account held nothing or had not yet spoken.
        """
        return self._account_figures()

    def trades(self):
        return self.wrapper.snapshot_trades()

    def openTrades(self):
        return [t for t in self.wrapper.snapshot_trades() if t.isActive()]

    def orders(self):
        return [t.order for t in self.wrapper.snapshot_trades() if t.order is not None]

    def openOrders(self):
        return [t.order for t in self.openTrades() if t.order is not None]

    def fills(self):
        return self.wrapper.snapshot_fills()

    def executions(self):
        return [f.execution for f in self.wrapper.snapshot_fills()]

    def managedAccounts(self):
        return self.wrapper.snapshot_accounts()

    def tickers(self):
        return self.wrapper.snapshot_tickers()

    def ticker(self, contract):
        """The quote for a contract already subscribed to, or nothing."""
        req_id = self._by_contract.get(("quote", id(contract)))
        if req_id is None:
            for rid, c in self._subscribed.items():
                if getattr(c, "conId", None) and getattr(c, "conId", None) == getattr(contract, "conId", None):
                    req_id = rid
                    break
        return self.wrapper.ticker_for(req_id) if req_id is not None else None

    @property
    def pendingTickers(self):
        return self.wrapper.take_pending()

    def reqMktData(
        self, contract, genericTickList="", snapshot=False,
        regulatorySnapshot=False, mktDataOptions=None,
    ):
        """Subscribe to a contract's quote and hand back the object it fills.

        The object is returned before anything has arrived, and fills in as
        ticks reach it. Its fields are ``None`` until the venue sends them, so a
        caller can tell "no bid yet" from "a bid of zero".
        """
        _refuse_options("mktDataOptions", mktDataOptions)
        _refuse_regulatory_snapshot(regulatorySnapshot)
        # One stream per contract object, and the same one back. The registry
        # holds a single request against the object, so opening a second left
        # the first running with nothing holding its id: the cancel names the
        # object and withdraws whichever was registered last.
        #
        # A snapshot is not registered and never answers a later ask. It ends
        # on its own, so the entry would name a request that is already over —
        # and it would take a running stream's place, as `reqTickers` is
        # careful not to.
        if not snapshot:
            already = self._by_contract.get(("quote", id(contract)))
            if already is not None:
                return self.wrapper.ticker_for(already)
        req_id, ticker = self._start_quote(contract, genericTickList, snapshot)
        # Under its own name, so it neither takes the stream's place nor
        # answers a later ask for one — and a snapshot on a contract that
        # never quotes can still be withdrawn by naming the contract, which
        # is the only handle a caller who did not keep the id has.
        self._by_contract[("snapshot" if snapshot else "quote", id(contract))] = req_id
        return ticker

    def _start_quote(self, contract, genericTickList="", snapshot=False):
        """Subscribe and hand back the request id along with the quote.

        The id is what a cancel names. A caller who owns it can withdraw its
        own subscription without going through the per-contract registry,
        which holds one quote per contract object and would otherwise lose
        whichever came first.
        """
        req_id = self._next_req_id()
        self._subscribed[req_id] = contract
        ticker = self.wrapper.bind_ticker(req_id, contract)
        self.client.req_mkt_data(req_id, contract, genericTickList, snapshot, False, [])
        return req_id, ticker

    def cancelMktData(self, contract):
        # The stream first: a snapshot beside it ends on its own, and a caller
        # cancelling a contract it is streaming means the stream.
        req_id = self._by_contract.pop(("quote", id(contract)), None)
        if req_id is None:
            req_id = self._by_contract.pop(("snapshot", id(contract)), None)
        if req_id is None:
            return
        self._subscribed.pop(req_id, None)
        self.client.cancel_mkt_data(req_id)



    # -- the rest of the live state ---------------------------------------

    def accountSummary(self, account="", timeout=5):
        """What the venue says the account is worth.

        The subscription is made as the session opens, so this normally has
        its answer already. On the first call it may not have arrived yet;
        waiting for it is what tells an account with nothing in it from one
        that has not spoken.
        """
        del account
        return self._account_figures(timeout)

    def _account_figures(self, timeout=5):
        """The account's figures, once the venue has stated all of them.

        Waits for the venue to say it has stated every figure, not for the
        first of them. Stopping at the first handed back one field of an
        account and read the same as a whole one, so a caller sizing a
        position off net liquidation sized it off whatever had landed.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and not self.wrapper.account_download_finished():
            time.sleep(0.05)
        return self.wrapper.snapshot_account_values()

    def pnl(self, account="", modelCode=""):
        del account, modelCode
        return self.wrapper.snapshot_pnl()

    def pnlSingle(self, account="", modelCode="", conId=0):
        del account, modelCode, conId
        return self.wrapper.snapshot_pnl_single()

    def newsBulletins(self):
        return self.wrapper.snapshot_bulletins()

    def newsTicks(self):
        return self.wrapper.snapshot_news_ticks()

    def realtimeBars(self):
        return self.wrapper.snapshot_bars()

    def cancelPnL(self, account="", modelCode=""):
        """Withdraw the P&L subscription this account and model asked for.

        The id the request was made under is what the venue and the client
        underneath both match a cancel against. This used to send the newest
        request id of any kind, so any request in between made the cancel name
        something else: the P&L stream carried on and nothing said so.
        """
        req_id = self._pnl_reqs.pop((account, modelCode), None)
        if req_id is not None:
            self.client.cancel_pnl(req_id)

    def cancelPnLSingle(self, account="", modelCode="", conId=0):
        """As above, for one position's P&L."""
        req_id = self._pnl_single_reqs.pop((account, modelCode, conId), None)
        if req_id is not None:
            self.client.cancel_pnl_single(req_id)

    # -- waiting -----------------------------------------------------------

    def waitOnUpdate(self, timeout=0):
        """Wait for something to arrive, and say whether anything did.

        A pump is already running, so this watches what it records rather than
        turning an event loop. False means the wait ran out with the session
        silent, which is what a program checking that its feed is alive is
        asking. Returning True regardless made that check one that could not
        fail, so a dead feed read exactly like a busy one.

        With no timeout, the one set by ``setTimeout`` is used.
        """
        seen = self.wrapper.updates()
        deadline = time.monotonic() + (timeout or self._timeout)
        while time.monotonic() < deadline:
            if self.wrapper.updates() != seen:
                return True
            time.sleep(0.005)
        return False

    def sleep(self, secs=0.02):
        time.sleep(secs)
        return True

    def setTimeout(self, timeout=60):
        """How long a wait with no timeout of its own waits for."""
        self._timeout = timeout

    def loopUntil(self, condition=None, timeout=0):
        """Run until a condition holds, or until the time is up.

        Yields as the wrapper this follows does, so ``for _ in ib.loopUntil(...)``
        reads the same.
        """
        deadline = time.monotonic() + timeout if timeout else None
        while True:
            if condition is not None and condition():
                return
            if deadline is not None and time.monotonic() > deadline:
                return
            yield self
            time.sleep(0.01)

    # -- questions that answer ---------------------------------------------

    def reqTickers(self, *contracts, regulatorySnapshot=False, timeout=5):
        """Subscribe, wait for each quote to arrive, then unsubscribe.

        A snapshot rather than a stream. A contract whose quote never arrives is
        returned with its fields unset rather than dropped, so the result lines
        up with what was asked for.
        """
        _refuse_regulatory_snapshot(regulatorySnapshot)
        # Its own ids, kept here rather than in the per-contract registry. A
        # snapshot on a contract already streaming would otherwise overwrite
        # the stream's id there, and cancelling the snapshot would drop the
        # entry and leave the stream running with nothing to name it by.
        started = [self._start_quote(c, snapshot=True) for c in contracts]
        tickers = [t for _, t in started]
        deadline = time.monotonic() + timeout
        # A snapshot is asked for the quote, so that is what this waits for.
        # Waiting for any field at all is satisfied by the previous close,
        # which arrives first, and cancels before the quote lands.
        while time.monotonic() < deadline:
            if all(t.hasBidAsk() or t.last is not None for t in tickers):
                break
            time.sleep(0.01)
        for req_id, _ in started:
            self._subscribed.pop(req_id, None)
            self.client.cancel_mkt_data(req_id)
        return tickers

    def whatIfOrder(self, contract, order, timeout=5):
        """What the venue says an order would cost, without sending it.

        The order is marked as a question rather than an instruction, so nothing
        reaches the market. Returns what the venue answered about margin and
        commission, or raises if it answered nothing.

        The caller's order is handed back as they wrote it. The mark used to
        stay on it, so the next time they placed that same order it went out as
        another question and nothing reached the market. The record the preview
        left behind is dropped for the same reason: a question is not an order,
        and the session reported one working that nobody had sent.
        """
        stated = getattr(order, "whatIf", False), getattr(order, "orderId", 0)
        order_id = stated[1] or self.client.next_order_id()
        order.whatIf = True
        order.orderId = order_id
        try:
            trade = self.placeOrder(contract, order)
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if trade.orderState is not None:
                    return trade.orderState
                time.sleep(0.01)
            raise TimeoutError("the venue did not answer what the order would cost")
        finally:
            order.whatIf, order.orderId = stated
            self.wrapper.forget_trade(order_id)

    def reqScannerData(self, subscription, scannerSubscriptionOptions=None, scannerSubscriptionFilterOptions=None, timeout=5):
        """Run a scan and hand back its rows, in the order the venue ranked them.

        Waits for the venue to say it has named every row. Stopping at the
        first one it had named was a scan of one out of fifty, with nothing to
        say the rest were still coming.
        """
        req_id = self.reqScannerSubscription(
            subscription, scannerSubscriptionOptions, scannerSubscriptionFilterOptions,
        )
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and not self.wrapper.scanner_finished(req_id):
            time.sleep(0.01)
        rows = self.wrapper.take_scanner(req_id)
        self.cancelScannerSubscription(req_id)
        self.wrapper.forget_scanner(req_id)
        return rows

    def reqScannerSubscription(self, subscription, scannerSubscriptionOptions=None, scannerSubscriptionFilterOptions=None):
        """Start a scan. The filters go with it; dropped, the scan that runs is
        a broader one than the caller described."""
        req_id = self._next_req_id()
        self.client.req_scanner_subscription(
            req_id, subscription,
            scannerSubscriptionOptions or [],
            scannerSubscriptionFilterOptions or [],
        )
        return req_id

    def cancelScannerSubscription(self, req_id):
        self.client.cancel_scanner_subscription(req_id)

    # -- orders built here rather than asked for ---------------------------

    def bracketOrder(
        self, action, quantity, limitPrice, takeProfitPrice, stopLossPrice, **kwargs
    ):
        """A parent and its two exits, each numbered and each naming its parent.

        Built here; nothing is sent. Each order is given its own id and both
        exits name the parent's, which is what holds them at the venue until
        the parent fills and what makes filling one withdraw the other. Left
        unnumbered they are three unrelated orders, and placing them sends a
        stop-loss to the market with no position behind it.

        The wrapper this follows holds the first two back with transmit=False
        as well. There is no staging here — an order reaches the market when it
        is placed — so an order carrying it is refused outright, and the parent
        id is what does the linking.
        """
        from .ibx import Order

        reverse = "SELL" if action.upper() == "BUY" else "BUY"
        parent = Order()
        parent.orderId = self.client.next_order_id()
        parent.action = action
        parent.orderType = "LMT"
        parent.totalQuantity = quantity
        parent.lmtPrice = limitPrice

        take_profit = Order()
        take_profit.orderId = self.client.next_order_id()
        take_profit.parentId = parent.orderId
        take_profit.action = reverse
        take_profit.orderType = "LMT"
        take_profit.totalQuantity = quantity
        take_profit.lmtPrice = takeProfitPrice

        stop_loss = Order()
        stop_loss.orderId = self.client.next_order_id()
        stop_loss.parentId = parent.orderId
        stop_loss.action = reverse
        stop_loss.orderType = "STP"
        stop_loss.totalQuantity = quantity
        stop_loss.auxPrice = stopLossPrice

        for o in (parent, take_profit, stop_loss):
            for k, v in kwargs.items():
                setattr(o, k, v)
        return BracketOrder(parent, take_profit, stop_loss)

    def oneCancelsAll(self, orders, ocaGroup, ocaType):
        """Link orders so that a fill on one withdraws the rest."""
        for o in orders:
            o.ocaGroup = ocaGroup
            o.ocaType = ocaType
        return orders

    # -- carried, and answered by the venue as not served ------------------

    def calculateImpliedVolatility(self, contract, optionPrice, underPrice, **kwargs):
        del kwargs
        return self.client.calculate_implied_volatility(
            self._next_req_id(), contract, optionPrice, underPrice, []
        )

    def calculateOptionPrice(self, contract, volatility, underPrice, **kwargs):
        del kwargs
        return self.client.calculate_option_price(
            self._next_req_id(), contract, volatility, underPrice, []
        )

    def requestFA(self, faDataType):
        return self.client.request_fa(faDataType)

    def replaceFA(self, faDataType, xml):
        return self.client.replace_fa(self._next_req_id(), faDataType, xml)

    def reqWshMetaData(self):
        self._wsh_meta = self._next_req_id()
        return self.client.req_wsh_meta_data(self._wsh_meta)

    def cancelWshMetaData(self, reqId=0):
        """Withdraw the calendar request this session made.

        Falling back to the newest request id of any kind, any request in
        between made the cancel name something else: the calendar carried on
        and an unrelated subscription was withdrawn in its place.
        """
        return self.client.cancel_wsh_meta_data(reqId or self._wsh_meta)

    def reqWshEventData(self, data):
        self._wsh_event = self._next_req_id()
        return self.client.req_wsh_event_data(self._wsh_event, data)

    def cancelWshEventData(self, reqId=0):
        """As above, for the events themselves."""
        return self.client.cancel_wsh_event_data(reqId or self._wsh_event)

    def getWshMetaData(self):
        return self.reqWshMetaData()

    def getWshEventData(self, data):
        return self.reqWshEventData(data)

    # -- orders ----------------------------------------------------------

    def placeOrder(self, contract, order):
        """Send an order and hand back the record of it.

        The record is returned before the venue has said anything, and its
        status moves under the caller as the venue answers. That is what makes
        it worth holding on to rather than reading a return code.

        A contract with no ``conId`` is resolved before the order is sent. That
        resolution is a request and an answer, so this call blocks until it
        completes. Call ``qualifyContracts`` once and place against the result,
        as the reference client's examples do, and this call does not block.
        """
        order_id = getattr(order, "orderId", 0) or self.client.next_order_id()
        try:
            order.orderId = order_id
        except AttributeError:
            pass
        trade = self.wrapper.register_order(order_id, contract, order)
        self.client.place_order(order_id, contract, order)
        return trade

    def cancelOrder(self, order, manualCancelOrderTime=""):
        order_id = getattr(order, "orderId", order)
        self.client.cancel_order(order_id, manualCancelOrderTime)
        return self.wrapper.trade_for(order_id)

    def reqGlobalCancel(self):
        self.client.req_global_cancel()

    def reqAutoOpenOrders(self, autoBind=True):
        self.client.req_auto_open_orders(autoBind)

    def reqCompletedOrders(self, apiOnly=False, timeout=5):
        """The orders the venue has finished with.

        Waits for the venue to say it has named them all, and hands back what
        it named. This used to return the session's ordinary trades, so an
        answer that arrived and an answer that never came looked the same.
        """
        self.wrapper.forget_completed()
        self.client.req_completed_orders(apiOnly)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and not self.wrapper.completed_orders_finished():
            time.sleep(0.01)
        return self.wrapper.snapshot_completed()

    def exerciseOptions(
        self, contract, exerciseAction, exerciseQuantity, account="", override=False,
    ):
        self.client.exercise_options(
            self._next_req_id(), contract, exerciseAction, exerciseQuantity,
            account, 1 if override else 0,
        )

    # -- streams the caller reads through the live state ------------------

    def reqMktDepth(self, contract, numRows=5, isSmartDepth=False, mktDepthOptions=None):
        _refuse_options("mktDepthOptions", mktDepthOptions)
        req_id = self._next_req_id()
        self._subscribed[req_id] = contract
        self._by_contract[("depth", id(contract))] = req_id
        self.client.req_mkt_depth(req_id, contract, numRows, isSmartDepth, [])
        return req_id

    def cancelMktDepth(self, contract, isSmartDepth=False):
        req_id = self._by_contract.pop(("depth", id(contract)), None)
        if req_id is not None:
            self._subscribed.pop(req_id, None)
            self.client.cancel_mkt_depth(req_id, isSmartDepth)

    def reqRealTimeBars(self, contract, barSize=5, whatToShow="TRADES", useRTH=True, realTimeBarsOptions=None):
        _refuse_options("realTimeBarsOptions", realTimeBarsOptions)
        req_id = self._next_req_id()
        self._subscribed[req_id] = contract
        self._by_contract[("bars", id(contract))] = req_id
        self.client.req_real_time_bars(req_id, contract, barSize, whatToShow, useRTH, [])
        return req_id

    def cancelRealTimeBars(self, bars):
        req_id = bars if isinstance(bars, int) else self._by_contract.pop(("bars", id(bars)), None)
        if req_id is not None:
            self.client.cancel_real_time_bars(req_id)

    def reqTickByTickData(self, contract, tickType="Last", numberOfTicks=0, ignoreSize=False):
        req_id = self._next_req_id()
        self._subscribed[req_id] = contract
        self._by_contract[("ticks", id(contract))] = req_id
        self.client.req_tick_by_tick_data(req_id, contract, tickType, numberOfTicks, ignoreSize)
        return req_id

    def cancelTickByTickData(self, contract, tickType="Last"):
        del tickType
        req_id = self._by_contract.pop(("ticks", id(contract)), None)
        if req_id is not None:
            self.client.cancel_tick_by_tick_data(req_id)

    def reqMarketDataType(self, marketDataType):
        self.client.req_market_data_type(marketDataType)

    # -- account ----------------------------------------------------------

    def reqAccountSummary(self, group="All", tags="", timeout=5):
        """What the venue says the account is worth, tag by tag.

        The request is answered on its own callback, which this records. It
        used to hand back the running account values instead — a different set,
        arriving from a different subscription — while the rows actually asked
        for reached a callback nothing implemented and were dropped.
        """
        req_id = self._next_req_id()
        self.wrapper.forget_account_summary(req_id)
        self.client.req_account_summary(req_id, group, tags)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and not self.wrapper.account_summary_finished(req_id):
            time.sleep(0.01)
        return self.wrapper.take_account_summary(req_id)

    def reqPnL(self, account="", modelCode=""):
        req_id = self._next_req_id()
        self._pnl_reqs[(account, modelCode)] = req_id
        self.client.req_pnl(req_id, account, modelCode)
        return req_id

    def reqPnLSingle(self, account, modelCode, conId):
        req_id = self._next_req_id()
        self._pnl_single_reqs[(account, modelCode, conId)] = req_id
        self.client.req_pnl_single(req_id, account, modelCode, conId)
        return req_id

    def reqAccountUpdatesMulti(self, account="", modelCode="", ledgerAndNLV=False):
        self.client.req_account_updates_multi(self._next_req_id(), account, modelCode, ledgerAndNLV)

    def reqUserInfo(self):
        self.client.req_user_info(self._next_req_id())

    # -- reference and news -----------------------------------------------

    def reqCurrentTime(self):
        self.client.req_current_time()

    def reqMarketRule(self, marketRuleId):
        self.client.req_market_rule(marketRuleId)

    def reqSecDefOptParams(self, underlyingSymbol, futFopExchange, underlyingSecType, underlyingConId):
        """The option chains an underlying has, one per venue.

        Returns them, as the client this follows does. Sending the request and
        returning nothing leaves a program that assigns the result holding
        nothing, indistinguishable from an underlying with no options.
        """
        return self.client.option_chains(
            underlyingSymbol, futFopExchange, underlyingSecType, underlyingConId,
        )

    def reqSmartComponents(self, bboExchange):
        self.client.req_smart_components(self._next_req_id(), bboExchange)

    def reqMktDepthExchanges(self):
        self.client.req_mkt_depth_exchanges()

    def reqNewsProviders(self):
        self.client.req_news_providers()

    def reqNewsArticle(self, providerCode, articleId, newsArticleOptions=None):
        _refuse_options("newsArticleOptions", newsArticleOptions)
        self.client.req_news_article(self._next_req_id(), providerCode, articleId, [])

    def reqHistoricalNews(
        self, conId, providerCodes, startDateTime, endDateTime,
        totalResults=100, historicalNewsOptions=None,
    ):
        _refuse_options("historicalNewsOptions", historicalNewsOptions)
        return [
            HistoricalNews(*row)
            for row in self.client.news_headlines(
                conId, providerCodes, startDateTime, endDateTime, totalResults,
            )
        ]

    def reqNewsBulletins(self, allMessages=True):
        self.client.req_news_bulletins(allMessages)

    def cancelNewsBulletins(self):
        self.client.cancel_news_bulletins()

    def reqScannerParameters(self):
        self.client.req_scanner_parameters()

    def reqHistoricalSchedule(self, contract, endDateTime="", durationStr="1 M", useRTH=True):
        """When a contract trades over a stretch of days.

        Each session is its opening, its close, and the day it belongs to, in
        the time zone the venue states them in.
        """
        timezone, sessions = self.client.trading_schedule(
            contract, endDateTime, durationStr, useRTH
        )
        return HistoricalSchedule(
            timeZone=timezone,
            sessions=[TradingSession(*row) for row in sessions],
        )

    def reqHistoricalTicks(
        self, contract, startDateTime="", endDateTime="", numberOfTicks=1000,
        whatToShow="TRADES", useRth=True, ignoreSize=False, miscOptions=None,
    ):
        _refuse_options("miscOptions", miscOptions)
        self.client.req_historical_ticks(
            self._next_req_id(), contract, startDateTime, endDateTime,
            numberOfTicks, whatToShow, 1 if useRth else 0, ignoreSize, [],
        )

    def cancelHistoricalData(self, bars):
        req_id = bars if isinstance(bars, int) else self._by_contract.pop(("history", id(bars)), None)
        if req_id is not None:
            self.client.cancel_historical_data(req_id)

    # -- asking the venue to start sending it ----------------------------

    def reqPositions(self):
        self.client.req_positions()
        return self.positions()

    def reqAccountUpdates(self, account=""):
        self.client.req_account_updates(True, account)
        return self.accountValues()

    def reqOpenOrders(self):
        self.client.req_open_orders()
        return self.openTrades()

    def reqAllOpenOrders(self):
        self.client.req_all_open_orders()
        return self.trades()

    def reqExecutions(self, execFilter=None):
        """The fills the filter names, and only those.

        The filter goes to the client, which decides what the request replays.
        Dropped, the answer was every fill the session had seen: another
        client's, and ones before the cutoff the caller asked from."""
        before = len(self.wrapper.snapshot_fills())
        self.client.req_executions(self._next_req_id(), execFilter)
        return self.wrapper.snapshot_fills()[before:]

    def reqManagedAccts(self):
        self.client.req_managed_accts()
        return self.managedAccounts()

    def isConnected(self) -> bool:
        return self.client.isConnected()

    # -- reference data --------------------------------------------------

    def reqContractDetails(self, contract: Contract):
        return self.client.contract_details(contract)

    def qualifyContracts(self, *contracts: Contract):
        """Fill in what the venue knows about each contract, above all its id.

        Returns the contracts it could resolve, and updates each argument in
        place, which is what the wrapper this follows does.
        """
        resolved = []
        for c in contracts:
            filled = self.client.qualify_contract(c)
            for name in (
                "conId", "symbol", "secType", "lastTradeDateOrContractMonth",
                "strike", "right", "multiplier", "exchange", "primaryExchange",
                "currency", "localSymbol", "tradingClass",
            ):
                try:
                    setattr(c, name, getattr(filled, name))
                except AttributeError:
                    pass
            resolved.append(c)
        return resolved

    def reqMatchingSymbols(self, pattern: str):
        return self.client.matching_symbols(pattern)

    # -- historical data -------------------------------------------------

    def reqHistoricalData(
        self,
        contract: Contract,
        endDateTime="",
        durationStr="1 D",
        barSizeSetting="1 min",
        whatToShow="TRADES",
        useRTH=True,
        formatDate=1,
        keepUpToDate=False,
        chartOptions=None,
    ):
        _refuse_options("chartOptions", chartOptions)
        # Refused rather than dropped. This answers once with the bars the
        # venue has, so a caller asking for a series that keeps updating would
        # be handed a snapshot that never changes; and it hands back the
        # moments as the venue spelled them, so one asking for seconds since
        # the epoch would be reading dates. Both are carried by the client
        # underneath, where the answer arrives on a callback.
        if keepUpToDate:
            raise NotImplementedError(
                "keepUpToDate is not carried here: this answers once with the "
                "bars the venue has. Ask the client for a series that keeps "
                "updating: ib.client.req_historical_data(...), whose bars "
                "arrive on historicalDataUpdate"
            )
        if formatDate != 1:
            raise NotImplementedError(
                f"formatDate={formatDate} is not carried here: the bars come "
                "back stating the moment as the venue spells it. Ask the "
                "client for another shape: ib.client.req_historical_data(...)"
            )
        return self.client.historical_data(
            contract,
            endDateTime,
            durationStr,
            barSizeSetting,
            whatToShow,
            1 if useRTH else 0,
        )

    def reqHeadTimeStamp(self, contract: Contract, whatToShow="TRADES", useRTH=True, formatDate=1):
        del formatDate
        return self.client.head_timestamp(contract, whatToShow, 1 if useRTH else 0)

    def reqHistogramData(self, contract: Contract, useRTH=True, period="3 days"):
        return self.client.histogram_data(contract, useRTH, period)

    def reqFundamentalData(self, contract: Contract, reportType: str, fundamentalDataOptions=None):
        _refuse_options("fundamentalDataOptions", fundamentalDataOptions)
        return self.client.fundamental_data(contract, reportType)

    # -- what has not been carried across yet ----------------------------

    def __getattr__(self, name: str):
        """Everything this does not name itself is the client's own.

        A session is the reference client with what it has been told kept
        beside it, so every request that client carries is a request this one
        carries. Reached this way it has the client's own shape — a request id
        to state, an answer arriving on a callback — because that is what it
        is; the calls named on this class are the ones with a shape of their
        own worth having.

        Python looks here only after failing to find the name on this class, so
        nothing defined above can be hidden by it.

        A name opening on an underscore is not carried across. The client keeps
        its own workings under those names and a session is not the place to
        reach them; forwarding them published the whole test-injection surface
        as though it were this class's own API.
        """
        if name.startswith("_"):
            raise AttributeError(name)
        try:
            return getattr(self.client, name)
        except AttributeError:
            raise AttributeError(
                f"neither this session nor the client underneath has {name!r}"
            ) from None


#: The session under the name the widely used asynchronous wrapper gives it, so
#: a program written against that one — and a test written against this one —
#: finds what it is looking for. The same class either way.
IB = Client
