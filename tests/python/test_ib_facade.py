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


def test_nothing_is_left_uncarried_and_the_refusal_still_works_if_it_were():
    """The list is empty now. The mechanism stays, because it is what keeps a
    future gap loud instead of silent."""
    from ibx._ib import IB, _NOT_YET

    assert _NOT_YET == frozenset()

    ib = ibx.IB()
    with pytest.raises(AttributeError):
        ib.somethingNobodyImplemented()


def test_a_name_that_is_no_method_is_still_refused():
    ib = ibx.IB()
    with pytest.raises(AttributeError):
        ib.thisIsNotAMethod


def test_the_unfinished_list_is_honest():
    """Nothing may be listed as unfinished while it is in fact carried."""
    from ibx._ib import IB, _NOT_YET

    carried = {n for n in dir(IB) if not n.startswith("_")}
    assert not (carried & _NOT_YET), "a method is both carried and listed as missing"


def test_a_read_only_session_refuses_to_change_a_position():
    """The counterpart carries the same control. A research program wants the
    guarantee at the client rather than in its own discipline."""
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


def test_a_bracket_holds_its_children_until_the_parent_is_transmitted():
    """A stop-loss reaching the market before there is a position to protect
    is an unhedged short."""
    ib = ibx.IB()
    b = ib.bracketOrder("BUY", 100, limitPrice=10.0, takeProfitPrice=12.0, stopLossPrice=9.0)

    assert b.parent.action == "BUY"
    assert b.takeProfit.action == "SELL" and b.stopLoss.action == "SELL"
    assert b.parent.transmit is False
    assert b.takeProfit.transmit is False
    assert b.stopLoss.transmit is True  # the last one releases the set

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


def test_a_what_if_never_reaches_the_market():
    """It is a question, not an instruction."""
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
    assert order.whatIf is True
