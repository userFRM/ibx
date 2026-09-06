"""An order state's outside-hours figures are unset until the venue states them.

The nine outside-hours margin and equity figures started at nought, on the
class and on the state a preview answers with, so a program reading a preview
was told the order changes nothing outside regular hours when the venue had
said nothing about it. The commission bounds already read as unset in that
case; these do now too.

Run: pytest tests/python/test_an_order_state_leaves_the_outside_hours_figures_unstated.py -v
"""

from ibx import UNSET_DOUBLE, Contract, EClient, EWrapper, Order, OrderState

FIGURES = [
    "initMarginBeforeOutsideRth", "maintMarginBeforeOutsideRth", "equityWithLoanBeforeOutsideRth",
    "initMarginChangeOutsideRth", "maintMarginChangeOutsideRth", "equityWithLoanChangeOutsideRth",
    "initMarginAfterOutsideRth", "maintMarginAfterOutsideRth", "equityWithLoanAfterOutsideRth",
]


class Recorder(EWrapper):
    def __init__(self):
        super().__init__()
        self.states = []
        self.errors = []

    def error(self, req_id, error_time, code, msg, advanced=""):
        self.errors.append((req_id, code, msg))

    def openOrder(self, order_id, contract, order, order_state):
        self.states.append(order_state)


def test_a_fresh_state_leaves_them_unset():
    state = OrderState()
    assert [getattr(state, name) for name in FIGURES] == [UNSET_DOUBLE] * 9


def test_a_preview_that_states_none_leaves_them_unset():
    recorder = Recorder()
    client = EClient(recorder)
    client._test_connect("DU0000000")
    client._test_map_con_id(265598, 1)
    contract = Contract()
    contract.conId, contract.symbol, contract.secType, contract.exchange, contract.currency = 265598, "AAPL", "STK", "SMART", "USD"
    order = Order()
    order.action, order.orderType, order.totalQuantity, order.lmtPrice = "BUY", "LMT", 1, 10.0
    client.placeOrder(1, contract, order)
    assert not recorder.errors, recorder.errors
    client._test_push_what_if(1, 1, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 7.0)
    client._test_dispatch_once()
    assert recorder.states, "no preview arrived"
    assert [getattr(recorder.states[0], name) for name in FIGURES] == [UNSET_DOUBLE] * 9
