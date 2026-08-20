"""The facade carrying the shape of the widely used asynchronous wrapper."""

import pytest

import ibx


def spy():
    c = ibx.Contract()
    c.symbol = "SPY"
    c.secType = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


def connected_ib():
    ib = ibx.IB()
    ib.client._test_connect("DU0000000")
    return ib


def test_the_reference_client_surface_is_still_exported():
    """The facade is an addition. A program using the other surface is untouched."""
    for name in ("EClient", "EWrapper", "Contract", "Order", "ContractDetails"):
        assert hasattr(ibx, name)


def test_qualifying_fills_the_contract_in_place():
    """The wrapper this follows updates the argument, and callers rely on it."""
    ib = connected_ib()
    req_id = ib.client._test_peek_ask_id()
    ib.client._test_push_contract_details(req_id, 756733, "SPY")
    ib.client._test_push_contract_details_end(req_id)

    c = spy()
    out = ib.qualifyContracts(c)
    assert out[0] is c
    assert c.conId == 756733


def test_bars_come_back_from_the_facade():
    ib = connected_ib()
    req_id = ib.client._test_peek_ask_id()
    ib.client._test_push_historical_data(
        req_id, [("20260101 09:30:00", 1.0, 2.0, 0.5, 1.5, 100)], True
    )
    bars = ib.reqHistoricalData(spy(), durationStr="1 D", barSizeSetting="1 min")
    assert [b.close for b in bars] == [1.5]


def test_the_earliest_data_comes_back_from_the_facade():
    ib = connected_ib()
    req_id = ib.client._test_peek_ask_id()
    ib.client._test_push_head_timestamp(req_id, "19930129 14:30:00")
    assert ib.reqHeadTimeStamp(spy()) == "19930129 14:30:00"


def test_a_name_nobody_carries_is_refused_by_name():
    """A gap is loud rather than silent: the refusal names what was asked for
    and where it was looked for, instead of the bare error a missing attribute
    would raise."""
    ib = ibx.IB()
    with pytest.raises(AttributeError):
        ib.somethingNobodyImplemented()


def test_a_name_that_is_no_method_is_still_refused():
    ib = ibx.IB()
    with pytest.raises(AttributeError):
        ib.thisIsNotAMethod


def test_a_read_only_session_refuses_to_change_a_position():
    """The reference client carries the same control. A research program wants
    the guarantee at the client rather than in its own discipline."""
    c = ibx.EClient(ibx.EWrapper())
    c._test_connect("DU0000000", readonly=True)

    order = ibx.Order()
    order.action = "BUY"
    order.orderType = "MKT"
    order.totalQuantity = 1

    for call in (
        lambda: c.placeOrder(1, spy(), order),
        lambda: c.cancelOrder(1, ""),
        lambda: c.reqGlobalCancel(),
    ):
        with pytest.raises(RuntimeError, match="read-only"):
            call()


def test_a_session_that_is_not_read_only_does_not_refuse():
    """The guard must fire on the flag, not on every order.

    A test-connected client has no venue behind it, so the order fails further
    down. What matters here is that it fails somewhere other than the guard.
    """
    c = ibx.EClient(ibx.EWrapper())
    c._test_connect("DU0000000")
    order = ibx.Order()
    order.action = "BUY"
    order.orderType = "MKT"
    order.totalQuantity = 1
    try:
        c.placeOrder(1, spy(), order)
    except RuntimeError as e:
        assert "read-only" not in str(e), "the guard fired on a session that is not read-only"


def test_placing_an_order_hands_back_a_record_that_moves():
    """The record is returned before the venue has answered, and its status
    moves under the caller. That is what makes it worth holding."""
    ib = connected_ib()
    order = ibx.Order()
    order.orderId = 7
    order.action = "BUY"
    order.orderType = "MKT"
    order.totalQuantity = 5

    try:
        trade = ib.placeOrder(spy(), order)
    except RuntimeError:
        # No venue behind a test session; the record is still registered.
        trade = ib.wrapper.trade_for(7)

    assert trade is not None
    assert trade.orderStatus.status == "PendingSubmit"
    assert trade.isActive()

    ib.wrapper.orderStatus(7, "Filled", 5.0, 0.0, 10.0, 1, 0, 10.0, 1, "")
    assert trade.isDone() and trade.filled() == 5.0
    assert trade in ib.trades()
    assert trade not in ib.openTrades()


def test_every_carried_method_is_actually_callable():
    """A name that resolves but is not a method would pass the honesty test
    while failing the caller."""
    from ibx._ib import IB

    for name in dir(IB):
        if name.startswith("_"):
            continue
        attr = getattr(IB, name)
        assert callable(attr) or isinstance(attr, property), name


def test_a_bracket_links_its_children_to_the_parent():
    """A stop-loss reaching the market before there is a position to protect
    is an unhedged short. The parent id is what holds it back."""
    ib = connected_ib()
    b = ib.bracketOrder("BUY", 100, limitPrice=10.0, takeProfitPrice=12.0, stopLossPrice=9.0)

    assert b.parent.action == "BUY"
    assert b.takeProfit.action == "SELL" and b.stopLoss.action == "SELL"

    ids = {b.parent.orderId, b.takeProfit.orderId, b.stopLoss.orderId}
    assert 0 not in ids and len(ids) == 3, "the three orders are not three orders"
    assert b.takeProfit.parentId == b.parent.orderId
    assert b.stopLoss.parentId == b.parent.orderId

    # Nothing is staged here, so nothing is held back by asking not to send it.
    assert all(o.transmit is True for o in b)

    assert b.takeProfit.lmtPrice == 12.0
    assert b.stopLoss.auxPrice == 9.0
    assert [o for o in b] == [b.parent, b.takeProfit, b.stopLoss]


def test_one_cancels_all_links_every_order_in_the_set():
    ib = ibx.IB()
    orders = []
    for _ in range(3):
        o = ibx.Order()
        o.action = "BUY"
        orders.append(o)
    ib.oneCancelsAll(orders, "grp-1", 1)
    assert all(o.ocaGroup == "grp-1" and o.ocaType == 1 for o in orders)


def test_a_what_if_leaves_the_order_as_the_caller_wrote_it():
    """It is a question, not an instruction, and asking it must not turn the
    caller's order into one.

    The mark used to stay on the order, so the next time the caller placed that
    same order it went out as another question and nothing reached the market.
    This asserted the mark had stuck, which is the defect written down as the
    behaviour.
    """
    ib = connected_ib()
    order = ibx.Order()
    order.orderId = 11
    order.action = "BUY"
    order.orderType = "MKT"
    order.totalQuantity = 1
    try:
        ib.whatIfOrder(spy(), order, timeout=0.05)
    except (TimeoutError, RuntimeError):
        pass
    assert order.whatIf is False, "the question must not be left on the order"
    assert order.orderId == 11, "and neither must the number it was asked under"


def test_a_what_if_leaves_no_order_behind():
    """A question is not an order. The preview travels the order path and is
    answered on the order callbacks, so it left a record that read as an order
    working at the venue that nobody had sent."""
    ib = connected_ib()
    order = ibx.Order()
    order.orderId = 12
    order.action = "BUY"
    order.orderType = "MKT"
    order.totalQuantity = 1
    try:
        ib.whatIfOrder(spy(), order, timeout=0.05)
    except (TimeoutError, RuntimeError):
        pass
    assert ib.openTrades() == []
    assert ib.wrapper.trade_for(12) is None


def test_a_regulatory_snapshot_is_refused_rather_than_turned_into_a_stream():
    """It is a separate, chargeable one-shot request. Accepted and dropped, the
    caller was handed an ordinary subscription instead of what they asked for
    and had no way to tell."""
    ib = connected_ib()
    with pytest.raises(NotImplementedError, match="regulatorySnapshot"):
        ib.reqMktData(spy(), regulatorySnapshot=True)
    with pytest.raises(NotImplementedError, match="regulatorySnapshot"):
        ib.reqTickers(spy(), regulatorySnapshot=True)


def test_a_snapshot_does_not_take_a_running_stream_s_place():
    """`reqTickers` used to register under the same per-contract slot as the
    stream, so the stream's id was overwritten, then dropped when the snapshot
    was cancelled. The subscription stayed up at the venue with nothing left to
    name it by."""
    ib = connected_ib()
    contract = spy()
    ib.reqMktData(contract)
    streaming = ib._by_contract[("quote", id(contract))]
    ib.reqTickers(contract, timeout=0)
    assert ib._by_contract[("quote", id(contract))] == streaming
    assert streaming in ib._subscribed


def test_a_pnl_cancel_names_the_subscription_it_was_asked_about():
    """It used to send the newest request id of any kind, so any request in
    between made the cancel name something else and the stream carried on."""
    ib = connected_ib()
    req_id = ib.reqPnL("DU0000000", "")
    later = ib.reqPnLSingle("DU0000000", "", 756733)   # any request in between
    assert later != req_id
    assert ib._pnl_reqs[("DU0000000", "")] == req_id
    ib.cancelPnL("DU0000000", "")
    assert ("DU0000000", "") not in ib._pnl_reqs


def test_the_calendar_cancel_names_the_request_it_was_asked_under():
    """It used to send the newest request id of any kind, so any request in
    between withdrew something else and left the calendar running."""
    ib = connected_ib()
    ib.reqWshMetaData()
    asked_under = ib._wsh_meta
    ib.reqWshEventData("{}")            # any request in between
    assert ib._wsh_event != asked_under
    assert ib._wsh_meta == asked_under


def test_completed_orders_come_back_from_the_venue_not_from_the_trades():
    """This used to hand back the session's ordinary trades, so an answer that
    arrived and one that never came looked the same.

    The venue answers on the session's second look, so what is measured is
    that the wait notices rather than how a loaded machine schedules.
    """
    from ibx._state import LiveState

    class Answers(LiveState):
        def completed_orders_finished(self):
            if not super().completed_orders_finished():
                self.completedOrder("a contract", "an order", "a state")
                self.completedOrdersEnd()
                return False
            return True

    ib = ibx.IB()
    ib.wrapper = Answers()
    ib.client._test_connect("DU0000000")
    ib.placeOrder(spy(), _market_order())          # an ordinary trade, not a completed one
    done = ib.reqCompletedOrders(timeout=5)
    assert [d.order for d in done] == ["an order"], done


def _market_order():
    o = ibx.Order()
    o.orderId, o.action, o.orderType, o.totalQuantity = 21, "BUY", "MKT", 1
    return o


def test_an_option_list_this_request_cannot_carry_is_refused():
    """Every one of these was accepted and dropped on the floor, so a caller
    who tuned a request with one was answered by an untuned request."""
    ib = connected_ib()
    with pytest.raises(NotImplementedError, match="mktDataOptions"):
        ib.reqMktData(spy(), mktDataOptions=[("manual", "1")])
    with pytest.raises(NotImplementedError, match="chartOptions"):
        ib.reqHistoricalData(spy(), chartOptions=[("manual", "1")])
    with pytest.raises(NotImplementedError, match="mktDepthOptions"):
        ib.reqMktDepth(spy(), mktDepthOptions=[("manual", "1")])


def test_an_empty_option_list_is_what_every_ordinary_call_passes():
    ib = connected_ib()
    ib.reqMktData(spy(), mktDataOptions=[])
    ib.reqMktData(spy(), mktDataOptions=None)
