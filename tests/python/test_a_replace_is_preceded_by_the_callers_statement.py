"""A replace is preceded by the caller's own statement of the order.

An order the venue named at connect has no placement record here, and one of
the shapes that restate a number from that record could not be replaced at all.
The reference client sends whatever the caller states on a modify; this client
sends that statement ahead of the replace, and the engine keeps it as the record
where it has none.

Run: pytest tests/python/test_a_replace_is_preceded_by_the_callers_statement.py -v
"""

from ibx import EWrapper, EClient, Contract, Order

REPLAYED = 4242
SPY_CON_ID = 756733


def spy():
    c = Contract()
    c.symbol, c.sec_type, c.exchange, c.currency = "SPY", "STK", "SMART", "USD"
    c.con_id = SPY_CON_ID
    return c


def limit(order_id, price):
    o = Order()
    o.order_id = order_id
    o.action, o.total_quantity, o.order_type, o.lmt_price = "BUY", 100.0, "LMT", price
    o.tif, o.transmit = "DAY", True
    return o


def test_the_statement_goes_ahead_of_the_replace():
    c = EClient(EWrapper())
    c._test_connect()
    c._test_take_commands()
    c._test_push_venue_order(REPLAYED, "SPY", "BUY", 100.0, 100.0)
    c._test_map_con_id(SPY_CON_ID, 0)

    c.place_order(REPLAYED, spy(), limit(REPLAYED, 101.0))

    sent = c._test_take_commands()
    kinds = [("Describe" if "Describe" in s else "Modify" if "Modify" in s else s) for s in sent]
    assert kinds == ["Describe", "Modify"], sent


def test_every_replace_of_a_venue_named_order_is_stated_again():
    c = EClient(EWrapper())
    c._test_connect()
    c._test_take_commands()
    c._test_push_venue_order(REPLAYED, "SPY", "BUY", 100.0, 100.0)
    c._test_map_con_id(SPY_CON_ID, 0)

    c.place_order(REPLAYED, spy(), limit(REPLAYED, 101.0))
    c.place_order(REPLAYED, spy(), limit(REPLAYED, 102.0))

    sent = c._test_take_commands()
    kinds = [("Describe" if "Describe" in s else "Modify" if "Modify" in s else s) for s in sent]
    assert kinds == ["Describe", "Modify", "Describe", "Modify"], sent
