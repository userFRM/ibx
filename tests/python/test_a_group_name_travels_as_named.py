"""A one-cancels-all group name travels as the caller names it.

A name that reads as a number was rewritten to the engine's own form on the
way out and read back under it. The venue holds such a name as named, so the
name travels as named, whatever it reads as.

Run: pytest tests/python/test_a_group_name_travels_as_named.py -v
"""

from ibx import EWrapper, EClient, Contract, Order


def spy():
    c = Contract()
    c.symbol, c.sec_type, c.exchange, c.currency = "SPY", "STK", "SMART", "USD"
    c.con_id = 756733
    return c


def limit(order_id, group):
    o = Order()
    o.order_id = order_id
    o.action, o.total_quantity, o.order_type, o.lmt_price = "BUY", 100.0, "LMT", 100.0
    o.tif, o.transmit, o.oca_group, o.oca_type = "DAY", True, group, 1
    return o


def test_a_numeric_group_name_travels_as_named():
    c = EClient(EWrapper())
    c._test_connect()
    c._test_map_con_id(756733, 0)
    c._test_take_commands()

    c.place_order(7, spy(), limit(7, "1234"))

    sent = c._test_take_commands()
    assert any("SubmitEx" in s and 'oca_group_str: "1234"' in s for s in sent), sent
