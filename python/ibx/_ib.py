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


class IB:
    """One session, asked questions directly."""

    def __init__(self) -> None:
        self.wrapper = LiveState()
        self.client = EClient(self.wrapper)
        self._pump: threading.Thread | None = None
        self._stop = threading.Event()
        self._subscribed: dict[int, object] = {}
        self._by_contract: dict[int, int] = {}
        self._req_id = 0

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
        """
        del host, port, timeout, account
        self.client.connect(
            client_id=clientId,
            username=username,
            password=password,
            paper=paper,
            readonly=readonly,
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
        return self.wrapper.snapshot_account_values()

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
        req_id = self._by_contract.get(id(contract))
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
        del regulatorySnapshot, mktDataOptions
        req_id = self._next_req_id()
        self._subscribed[req_id] = contract
        self._by_contract[id(contract)] = req_id
        ticker = self.wrapper.bind_ticker(req_id, contract)
        self.client.req_mkt_data(req_id, contract, genericTickList, snapshot, False, [])
        return ticker

    def cancelMktData(self, contract):
        req_id = self._by_contract.pop(id(contract), None)
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
        deadline = time.monotonic() + timeout
        while not self.wrapper.snapshot_account_values() and time.monotonic() < deadline:
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
        del account, modelCode
        self.client.cancel_pnl(self._req_id)

    def cancelPnLSingle(self, account="", modelCode="", conId=0):
        del account, modelCode, conId
        self.client.cancel_pnl_single(self._req_id)

    # -- waiting -----------------------------------------------------------

    def waitOnUpdate(self, timeout=0):
        """Wait for something to arrive.

        A pump is already running, so this is a sleep with a floor rather than
        an event loop turn. It returns True to say the wait completed, matching
        the wrapper this follows.
        """
        time.sleep(timeout if timeout else 0.01)
        return True

    def sleep(self, secs=0.02):
        time.sleep(secs)
        return True

    def setTimeout(self, timeout=60):
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
        del regulatorySnapshot
        tickers = [self.reqMktData(c, snapshot=True) for c in contracts]
        deadline = time.monotonic() + timeout
        # What a snapshot is asked for is the quote, so that is what this
        # waits for. Waiting for any field at all was answered by the previous
        # close, which arrives first, and the quote was cancelled before it
        # came.
        while time.monotonic() < deadline:
            if all(t.hasBidAsk() or t.last is not None for t in tickers):
                break
            time.sleep(0.01)
        for c in contracts:
            self.cancelMktData(c)
        return tickers

    def whatIfOrder(self, contract, order, timeout=5):
        """What the venue says an order would cost, without sending it.

        The order is marked as a question rather than an instruction, so nothing
        reaches the market. Returns what the venue answered about margin and
        commission, or raises if it answered nothing.
        """
        order.whatIf = True
        trade = self.placeOrder(contract, order)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if trade.orderState is not None:
                return trade.orderState
            time.sleep(0.01)
        raise TimeoutError("the venue did not answer what the order would cost")

    def reqScannerData(self, subscription, scannerSubscriptionOptions=None, scannerSubscriptionFilterOptions=None, timeout=5):
        del scannerSubscriptionOptions, scannerSubscriptionFilterOptions
        req_id = self.reqScannerSubscription(subscription)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            rows = self.wrapper.take_scanner(req_id)
            if rows:
                self.cancelScannerSubscription(req_id)
                return [cd for _, cd in rows]
            time.sleep(0.01)
        self.cancelScannerSubscription(req_id)
        return []

    def reqScannerSubscription(self, subscription, scannerSubscriptionOptions=None, scannerSubscriptionFilterOptions=None):
        del scannerSubscriptionOptions, scannerSubscriptionFilterOptions
        req_id = self._next_req_id()
        self.client.req_scanner_subscription(req_id, subscription)
        return req_id

    def cancelScannerSubscription(self, req_id):
        self.client.cancel_scanner_subscription(req_id)

    # -- orders built here rather than asked for ---------------------------

    def bracketOrder(
        self, action, quantity, limitPrice, takeProfitPrice, stopLossPrice, **kwargs
    ):
        """A parent and its two exits, linked so that filling one cancels the other.

        Built here; nothing is sent. The children are held until the parent is
        transmitted, which is what stops a stop-loss reaching the market before
        there is a position for it to protect.
        """
        from .ibx import Order

        reverse = "SELL" if action.upper() == "BUY" else "BUY"
        parent = Order()
        parent.action = action
        parent.orderType = "LMT"
        parent.totalQuantity = quantity
        parent.lmtPrice = limitPrice
        parent.transmit = False

        take_profit = Order()
        take_profit.action = reverse
        take_profit.orderType = "LMT"
        take_profit.totalQuantity = quantity
        take_profit.lmtPrice = takeProfitPrice
        take_profit.transmit = False

        stop_loss = Order()
        stop_loss.action = reverse
        stop_loss.orderType = "STP"
        stop_loss.totalQuantity = quantity
        stop_loss.auxPrice = stopLossPrice
        stop_loss.transmit = True

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
        return self.client.req_wsh_meta_data(self._next_req_id())

    def cancelWshMetaData(self, reqId=0):
        return self.client.cancel_wsh_meta_data(reqId or self._req_id)

    def reqWshEventData(self, data):
        return self.client.req_wsh_event_data(self._next_req_id(), data)

    def cancelWshEventData(self, reqId=0):
        return self.client.cancel_wsh_event_data(reqId or self._req_id)

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

    def reqCompletedOrders(self, apiOnly=False):
        self.client.req_completed_orders(apiOnly)
        return self.trades()

    def exerciseOptions(
        self, contract, exerciseAction, exerciseQuantity, account="", override=False,
    ):
        self.client.exercise_options(
            self._next_req_id(), contract, exerciseAction, exerciseQuantity,
            account, 1 if override else 0,
        )

    # -- streams the caller reads through the live state ------------------

    def reqMktDepth(self, contract, numRows=5, isSmartDepth=False, mktDepthOptions=None):
        del mktDepthOptions
        req_id = self._next_req_id()
        self._subscribed[req_id] = contract
        self._by_contract[id(contract)] = req_id
        self.client.req_mkt_depth(req_id, contract, numRows, isSmartDepth, [])
        return req_id

    def cancelMktDepth(self, contract, isSmartDepth=False):
        req_id = self._by_contract.pop(id(contract), None)
        if req_id is not None:
            self._subscribed.pop(req_id, None)
            self.client.cancel_mkt_depth(req_id, isSmartDepth)

    def reqRealTimeBars(self, contract, barSize=5, whatToShow="TRADES", useRTH=True, realTimeBarsOptions=None):
        del realTimeBarsOptions
        req_id = self._next_req_id()
        self._subscribed[req_id] = contract
        self._by_contract[id(contract)] = req_id
        self.client.req_real_time_bars(req_id, contract, barSize, whatToShow, useRTH, [])
        return req_id

    def cancelRealTimeBars(self, bars):
        req_id = bars if isinstance(bars, int) else self._by_contract.pop(id(bars), None)
        if req_id is not None:
            self.client.cancel_real_time_bars(req_id)

    def reqTickByTickData(self, contract, tickType="Last", numberOfTicks=0, ignoreSize=False):
        req_id = self._next_req_id()
        self._subscribed[req_id] = contract
        self._by_contract[id(contract)] = req_id
        self.client.req_tick_by_tick_data(req_id, contract, tickType, numberOfTicks, ignoreSize)
        return req_id

    def cancelTickByTickData(self, contract, tickType="Last"):
        del tickType
        req_id = self._by_contract.pop(id(contract), None)
        if req_id is not None:
            self.client.cancel_tick_by_tick_data(req_id)

    def reqMarketDataType(self, marketDataType):
        self.client.req_market_data_type(marketDataType)

    # -- account ----------------------------------------------------------

    def reqAccountSummary(self, group="All", tags=""):
        self.client.req_account_summary(self._next_req_id(), group, tags)
        return self.accountValues()

    def reqPnL(self, account="", modelCode=""):
        self.client.req_pnl(self._next_req_id(), account, modelCode)

    def reqPnLSingle(self, account, modelCode, conId):
        self.client.req_pnl_single(self._next_req_id(), account, modelCode, conId)

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

        Returns them, as the client this follows does. It used to send the
        request and return nothing, so a program that assigned the result held
        nothing and could not tell that from an underlying with no options.
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
        del newsArticleOptions
        self.client.req_news_article(self._next_req_id(), providerCode, articleId, [])

    def reqHistoricalNews(
        self, conId, providerCodes, startDateTime, endDateTime,
        totalResults=100, historicalNewsOptions=None,
    ):
        del historicalNewsOptions
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
        del miscOptions
        self.client.req_historical_ticks(
            self._next_req_id(), contract, startDateTime, endDateTime,
            numberOfTicks, whatToShow, 1 if useRth else 0, ignoreSize, [],
        )

    def cancelHistoricalData(self, bars):
        req_id = bars if isinstance(bars, int) else self._by_contract.pop(id(bars), None)
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
        del execFilter
        self.client.req_executions(1)
        return self.fills()

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
        del formatDate, keepUpToDate, chartOptions
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
        del fundamentalDataOptions
        return self.client.fundamental_data(contract, reportType)

    # -- what has not been carried across yet ----------------------------

    def __getattr__(self, name: str):
        """Say plainly that a call is not carried yet.

        The wrapper this follows has ninety methods. A name it has and this does
        not raises where it is called, naming itself — rather than resolving to
        something that quietly returns nothing.
        """
        if name in _NOT_YET:
            def unavailable(*args, **kwargs):
                raise NotImplementedError(
                    f"IB.{name}() is not carried on this client yet; "
                    f"EClient carries the request under the reference client's name"
                )

            return unavailable
        raise AttributeError(f"'IB' object has no attribute '{name}'")


#: Every method the wrapper this follows has, that this facade does not carry
#: yet. Listed rather than inferred so that the count is a fact and not an
#: impression, and so a caller gets a named refusal instead of an AttributeError
#: that reads like a typo.
_NOT_YET = frozenset({
    
    
    
    
    
    
    
    
    
    
    
    
    
    
    
    
    
    
    
    
})
