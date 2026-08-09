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


def test_a_call_not_carried_yet_names_itself():
    """Not an AttributeError that reads like a typo, and not a quiet empty list."""
    ib = ibx.IB()
    with pytest.raises(NotImplementedError, match="placeOrder"):
        ib.placeOrder()


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
