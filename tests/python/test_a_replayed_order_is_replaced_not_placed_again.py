"""A working order this client did not place is replaced, not placed again.

An order the venue replays at connect belongs to no book this client keeps.
Classified from that book alone, a call naming its id read as a first
placement: a new order went out under a number the venue is already working,
and the engine's record of the order it named was overwritten on the way.

The same call also settles the contract before it asks for an instrument slot.
Asked first, a replacement naming the wrong contract spent one of the table's
slots on a contract the call then refused, and the table does not grow.

Run: pytest tests/python/test_a_replayed_order_is_replaced_not_placed_again.py -v
"""

from ibx import EWrapper, EClient, Contract, Order

REPLAYED = 4242
SPY_CON_ID = 756733
QQQ_CON_ID = 320227571


class Recorder(EWrapper):
    def __init__(self):
        self.refusals = []

    def error(self, req_id, error_time, code, message, advanced_order_reject_json=""):
        self.refusals.append((req_id, code, message))


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


def connected():
    w = Recorder()
    c = EClient(w)
    c._test_connect()
    c._test_take_commands()
    return w, c


def test_a_replayed_order_is_replaced_rather_than_placed_again():
    w, c = connected()
    c._test_push_venue_order(REPLAYED, "SPY", "BUY", 100.0, 100.0)
    c._test_map_con_id(SPY_CON_ID, 0)

    c.place_order(REPLAYED, spy(), limit(REPLAYED, 101.0))

    sent = c._test_take_commands()
    assert any("Modify" in s for s in sent), sent
    assert not any("SubmitEx" in s for s in sent), sent


def test_a_replace_naming_another_contract_asks_for_no_slot():
    """The refusal comes before the registration, so the table is not spent."""
    w, c = connected()
    c._test_push_venue_order(REPLAYED, "SPY", "BUY", 100.0, 100.0)
    c._test_map_con_id(SPY_CON_ID, 0)
    c._test_track_order(REPLAYED, 0, "SPY", "BUY", 100.0, 100.0, 0)

    elsewhere = spy()
    elsewhere.symbol, elsewhere.con_id = "QQQ", QQQ_CON_ID
    c.place_order(REPLAYED, elsewhere, limit(REPLAYED, 101.0))

    assert w.refusals, "the caller is told the replace names another contract"
    assert "another contract" in w.refusals[-1][2], w.refusals
    assert c._test_take_commands() == [], "and nothing was asked of the engine"
