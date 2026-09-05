"""A replace of a trailing stop limit names its limit offset and its trail.

The replace reads each number from the field the submit reads it from: the
limit offset from lmtPriceOffset and the trail from auxPrice. Read from
lmtPrice, the offset a caller set never reached the request, and the venue
went on working the order as it was placed.

Run: pytest tests/python/test_a_replace_of_a_trailing_stop_limit_names_its_offset_and_trail.py -v
"""

from ibx import EWrapper, EClient, Contract, Order

SPY_CON_ID = 756733


def spy():
    c = Contract()
    c.symbol, c.sec_type, c.exchange, c.currency = "SPY", "STK", "SMART", "USD"
    c.con_id = SPY_CON_ID
    return c


def trail_limit(order_id, trail, offset):
    o = Order()
    o.order_id = order_id
    o.action, o.total_quantity, o.order_type = "SELL", 1.0, "TRAIL LIMIT"
    o.aux_price, o.lmt_price_offset = trail, offset
    o.tif, o.transmit = "DAY", True
    return o


def test_the_replace_names_the_offset_as_its_price_and_the_trail_as_its_trigger():
    c = EClient(EWrapper())
    c._test_connect()
    c._test_take_commands()
    c._test_map_con_id(SPY_CON_ID, 0)

    c.place_order(7, spy(), trail_limit(7, 1.0, 0.1))
    placed = c._test_take_commands()
    assert any("SubmitEx" in s for s in placed), placed

    c.place_order(7, spy(), trail_limit(7, 2.0, 0.2))
    sent = c._test_take_commands()
    modify = next((s for s in sent if "Modify" in s), None)
    assert modify, sent
    assert "price: 20000000" in modify, modify
    assert "stop_price: 200000000" in modify, modify
