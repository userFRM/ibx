"""A replace carries the caller's own statement of the order.

An order the venue named at connect has no placement record here, and one of
the shapes that restate a number from that record could not be replaced at all.
The reference client sends whatever the caller states on a modify, and merges it
onto the order it is already working. This client carries that statement on the
replace itself, so the two cannot arrive apart: sent separately, two replaces of
one order interleaved and each went out under the other's terms.

Run: pytest tests/python/test_a_replace_carries_the_callers_statement.py -v
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


def venue_named_order():
    c = EClient(EWrapper())
    c._test_connect()
    c._test_take_commands()
    c._test_push_venue_order(REPLAYED, "SPY", "BUY", 100.0, 100.0)
    c._test_map_con_id(SPY_CON_ID, 0)
    return c


def test_the_statement_travels_on_the_replace():
    c = venue_named_order()

    c.place_order(REPLAYED, spy(), limit(REPLAYED, 101.0))

    sent = c._test_take_commands()
    assert len(sent) == 1, sent
    assert "Modify" in sent[0], sent
    assert "spec: Some(" in sent[0], "the replace carries the statement: %s" % sent


def test_every_replace_of_a_venue_named_order_states_it_again():
    c = venue_named_order()

    c.place_order(REPLAYED, spy(), limit(REPLAYED, 101.0))
    c.place_order(REPLAYED, spy(), limit(REPLAYED, 102.0))

    sent = c._test_take_commands()
    assert len(sent) == 2, sent
    assert all("Modify" in s and "spec: Some(" in s for s in sent), sent
