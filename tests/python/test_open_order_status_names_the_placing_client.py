"""An open order and its status must name the same client.

The venue names the client an order was placed under, and the order object
handed to `open_order` carries it. The `order_status` beside it read the
client this session happens to be asking under, so the two callbacks for one
order disagreed and a caller sorting orders by client filed somebody else's
order under its own.

Run: pytest tests/python/test_open_order_status_names_the_placing_client.py -v
"""

from ibx import EWrapper, EClient

PLACED_UNDER = 3
ASKING_UNDER = 7
ORDER_ID = 8801


class Recorder(EWrapper):
    def __init__(self):
        self.opened = []
        self.status = []

    def open_order(self, order_id, contract, order, order_state):
        self.opened.append((order_id, order.client_id))

    def order_status(self, order_id, status, filled, remaining, avg_fill_price,
                     perm_id, parent_id, last_fill_price, client_id, why_held,
                     mkt_cap_price):
        self.status.append((order_id, client_id))


def test_the_status_names_the_client_the_order_was_placed_under():
    w = Recorder()
    c = EClient(w)
    c._test_connect()
    c._test_set_client_id(PLACED_UNDER)
    c._test_map_instrument(1, 0)
    c._test_track_order(ORDER_ID, 0, "SPY", "BUY", 1.0, 110.0, 0)

    # Another client asks what is open. The order is still the first one's.
    c._test_set_client_id(ASKING_UNDER)
    c.req_open_orders()
    c._test_dispatch_once()

    assert w.opened == [(ORDER_ID, PLACED_UNDER)], w.opened
    assert w.status == [(ORDER_ID, PLACED_UNDER)], w.status


def test_a_fill_and_a_status_name_the_client_that_placed_the_order():
    """The two dispatch callbacks read the same record as `open_order`.

    They read the client this session happens to be asking under, so a status
    about an order this client did not place named whoever was watching.
    """
    w = Recorder()
    c = EClient(w)
    c._test_connect()
    c._test_set_client_id(PLACED_UNDER)
    c._test_map_instrument(1, 0)
    c._test_track_order(ORDER_ID, 0, "SPY", "BUY", 1.0, 110.0, 0)

    c._test_set_client_id(ASKING_UNDER)
    c._test_push_order_update(ORDER_ID, 0, "Submitted", 0.0, 1.0)
    c._test_dispatch_once()
    c._test_push_fill(0, ORDER_ID, "BUY", 110.0, 1, 0, 0.0)
    c._test_dispatch_once()

    assert w.status, "the order draws a status"
    assert all(client == PLACED_UNDER for _, client in w.status), w.status
