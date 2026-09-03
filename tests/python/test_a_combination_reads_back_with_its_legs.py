"""A combination placed with its legs is read back with them.

`ContractSamples.StockComboContract` is a BAG carrying two legs and nothing
else that names it. Placed, the legs reached the venue; read back on
`openOrder`, the contract arrived a BAG holding no legs at all. That is not a
partial answer, it is a different contract — the one a program would then
cancel against, place again, or log.

Every callback and request that hands back a contract this client placed is
tried here from one session: the open-order snapshot, the live status update,
a what-if preview, a fill and its replay, and the completed order. Every field
the reference leg carries is set on the first leg, so a field dropped on the
way back reads as a dropped field and not as a default.

What comes back is this client's own record of the legs, made when the order
was placed. The engine reads no leg off a report, so a combination the venue
replays from another session comes back with none.
"""

from ibx import ComboLeg, Contract, EClient, EWrapper, Order

COMBO_CON_ID = 28812380
COMBO_INSTRUMENT = 1
ORDER_ID = 1001

#: (conId, ratio, action, exchange, openClose, shortSaleSlot,
#:  designatedLocation, exemptCode), as the sample below states them.
LEGS = [
    (43645865, 1, "BUY", "SMART", 1, 2, "ABC", 7),
    (9408, 1, "SELL", "SMART", 0, 0, "", -1),
]


class _Recorder(EWrapper):
    def __init__(self):
        super().__init__()
        self.errors = []
        self.opened = {}
        self.executed = []
        self.completed = []

    def error(self, req_id, error_time, code, msg, advanced=""):
        self.errors.append((req_id, code, msg))

    def open_order(self, order_id, contract, order, order_state):
        self.opened[order_id] = contract

    def exec_details(self, req_id, contract, execution):
        self.executed.append(contract)

    def completed_order(self, contract, order, order_state):
        self.completed.append(contract)


def _stock_combo():
    """`ContractSamples.StockComboContract`, as written, under an id.

    No venue behind the session names the combination, so the id stands in
    for that answer. The legs are what is under test.
    """
    contract = Contract()
    contract.symbol = "IBKR,MCD"
    contract.secType = "BAG"
    contract.currency = "USD"
    contract.exchange = "SMART"

    leg1 = ComboLeg()
    leg1.conId = 43645865
    leg1.ratio = 1
    leg1.action = "BUY"
    leg1.exchange = "SMART"
    leg1.openClose = 1
    leg1.shortSaleSlot = 2
    leg1.designatedLocation = "ABC"
    leg1.exemptCode = 7

    leg2 = ComboLeg()
    leg2.conId = 9408
    leg2.ratio = 1
    leg2.action = "SELL"
    leg2.exchange = "SMART"

    contract.comboLegs = []
    contract.comboLegs.append(leg1)
    contract.comboLegs.append(leg2)
    contract.conId = COMBO_CON_ID
    return contract


def _placed():
    recorder = _Recorder()
    client = EClient(recorder)
    client._test_connect("DU0000000")
    client._test_map_con_id(COMBO_CON_ID, COMBO_INSTRUMENT)
    order = Order()
    order.action, order.orderType, order.totalQuantity, order.lmtPrice = "BUY", "LMT", 1, 10.0
    client.place_order(ORDER_ID, _stock_combo(), order)
    assert not recorder.errors, recorder.errors
    return recorder, client


def _legs_of(contract):
    assert contract.secType == "BAG"
    assert all(isinstance(leg, ComboLeg) for leg in contract.comboLegs), contract.comboLegs
    return [
        (leg.conId, leg.ratio, leg.action, leg.exchange, leg.openClose,
         leg.shortSaleSlot, leg.designatedLocation, leg.exemptCode)
        for leg in contract.comboLegs
    ]


def test_the_open_order_snapshot_carries_the_legs():
    recorder, client = _placed()
    client.req_open_orders()
    client._test_dispatch_once()
    assert _legs_of(recorder.opened[ORDER_ID]) == LEGS


def test_a_status_update_carries_the_legs():
    recorder, client = _placed()
    client._test_push_order_update(ORDER_ID, COMBO_INSTRUMENT, "Submitted", 0.0, 1.0)
    client._test_dispatch_once()
    assert _legs_of(recorder.opened[ORDER_ID]) == LEGS


def test_a_what_if_preview_carries_the_legs():
    recorder, client = _placed()
    client._test_push_what_if(ORDER_ID, COMBO_INSTRUMENT, 0, 0, 0, 0, 0, 0, 0)
    client._test_dispatch_once()
    assert _legs_of(recorder.opened[ORDER_ID]) == LEGS


def test_a_fill_and_its_replay_carry_the_legs():
    recorder, client = _placed()
    client._test_push_fill(COMBO_INSTRUMENT, ORDER_ID, "BUY", 10.0, 1, 0)
    client._test_dispatch_once()
    assert [_legs_of(c) for c in recorder.executed] == [LEGS]

    recorder.executed.clear()
    client.req_executions(7)
    client._test_dispatch_once()
    assert [_legs_of(c) for c in recorder.executed] == [LEGS]


def test_a_completed_order_carries_the_legs():
    recorder, client = _placed()
    client._test_push_completed_order(
        ORDER_ID, COMBO_INSTRUMENT, "Cancelled", 0, "IBKR,MCD", "BUY", 1.0, 10.0,
        "Cancelled", "20260903 10:00:00", "USD", "", 0.0,
    )
    client.req_completed_orders()
    client._test_dispatch_once()
    assert [_legs_of(c) for c in recorder.completed] == [LEGS]


def test_a_leg_read_back_is_the_class_a_caller_builds_one_with():
    """The same class either way, so `isinstance` holds and a leg read back
    can be appended to the next combination as it is."""
    recorder, client = _placed()
    client.req_open_orders()
    client._test_dispatch_once()
    leg = recorder.opened[ORDER_ID].comboLegs[0]
    assert type(leg) is ComboLeg
    assert (leg.con_id, leg.open_close, leg.short_sale_slot) == (43645865, 1, 2), \
        "under this client's spelling as well as the reference client's"
    assert (ComboLeg.SAME, ComboLeg.OPEN, ComboLeg.CLOSE, ComboLeg.UNKNOWN) == (0, 1, 2, 3)
