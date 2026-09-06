"""A fill states how an option came to be exercised, or that it was not.

The reference client's `Execution` carries `optExerciseOrLapseType`, -1 where
the report states none. It was not here, so a program reading it off every
fill raised AttributeError.

Run: pytest tests/python/test_a_fill_states_no_exercise_type.py -v
"""

from ibx import Contract, EClient, EWrapper, Execution, Order


class Recorder(EWrapper):
    def __init__(self):
        super().__init__()
        self.fills = []
        self.errors = []

    def error(self, req_id, error_time, code, msg, advanced=""):
        self.errors.append((req_id, code, msg))

    def execDetails(self, req_id, contract, execution):
        self.fills.append(execution)


def test_a_fresh_execution_states_none():
    assert Execution().optExerciseOrLapseType == -1


def test_a_fill_states_none():
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
    client._test_push_fill(1, 1, "BUY", 10.0, 1, 0)
    client._test_dispatch_once()
    assert [fill.optExerciseOrLapseType for fill in recorder.fills] == [-1]
