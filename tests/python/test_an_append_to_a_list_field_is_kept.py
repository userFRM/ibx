"""An append to a list field is kept.

The reference client's classes carry plain lists, and its own samples build
every combination contract, every algo order and every conditional order by
appending to one: `contract.comboLegs.append(leg)`,
`order.algoParams.append(TagValue(...))`, `order.conditions.append(cond)`.
Handed back as a fresh list on every read, the append landed on a copy and was
thrown away: the order went to the venue without its legs, its parameters or
its conditions, and nothing said so. A combination that loses its legs is not
a smaller order, it is a different one.

Each test builds the object the way the vendor's sample builds it, then follows
it through `place_order` to what the client reads at send time. A field this
protocol carries comes back on the open order; one it has no field for is
refused by name, which it cannot be if the append was lost.
"""

import math

from ibx import (
    ComboLeg, Contract, EClient, EWrapper, Order, OrderComboLeg, PriceCondition,
    TagValue,
)


class _Recorder(EWrapper):
    def __init__(self):
        super().__init__()
        self.errors = []
        self.placed = {}

    def error(self, req_id, error_time, code, msg, advanced=""):
        self.errors.append((req_id, code, msg))

    def open_order(self, order_id, contract, order, order_state):
        self.placed[order_id] = (contract, order)


SPY_CON_ID = 756733
COMBO_CON_ID = 28812380


def _session():
    recorder = _Recorder()
    client = EClient(recorder)
    client._test_connect("DU0000000")
    # No engine behind this session to hand a contract its slot, so the two
    # contracts placed on are given theirs here.
    client._test_map_con_id(SPY_CON_ID, 0)
    client._test_map_con_id(COMBO_CON_ID, 1)
    return recorder, client


def _read_back(recorder, client, order_id):
    """What the client holds for the order it sent, as a callback hands it over."""
    client.req_open_orders()
    client._test_dispatch_once()
    assert not recorder.errors, recorder.errors
    return recorder.placed[order_id]


def _spy():
    contract = Contract()
    contract.symbol, contract.secType, contract.exchange, contract.currency = "SPY", "STK", "SMART", "USD"
    contract.conId = SPY_CON_ID
    return contract


def _limit(action, quantity, price):
    order = Order()
    order.action = action
    order.orderType = "LMT"
    order.totalQuantity = quantity
    order.lmtPrice = price
    return order


def _stock_combo(second_leg_action="SELL"):
    """`ContractSamples.StockComboContract`, as written."""
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

    leg2 = ComboLeg()
    leg2.conId = 9408
    leg2.ratio = 1
    leg2.action = second_leg_action
    leg2.exchange = "SMART"

    contract.comboLegs = []
    contract.comboLegs.append(leg1)
    contract.comboLegs.append(leg2)
    return contract


def _named_stock_combo():
    """The sample's combination, under an id.

    Without a venue behind the session nothing can name the combination, so
    the id stands in for that answer. The legs are what is under test.
    """
    contract = _stock_combo()
    contract.conId = COMBO_CON_ID
    return contract


def test_a_combination_built_as_the_sample_builds_one_holds_its_legs():
    contract = _stock_combo()
    assert contract.comboLegs is contract.comboLegs, "two reads, one list"
    assert contract.combo_legs is contract.comboLegs, "under either spelling"
    assert [(leg.conId, leg.action) for leg in contract.comboLegs] == [
        (43645865, "BUY"), (9408, "SELL"),
    ]


def test_the_legs_appended_are_the_legs_the_order_goes_out_with():
    recorder, client = _session()
    client.place_order(1001, _named_stock_combo(), _limit("BUY", 1, 10.0))
    # A combination stating no legs is refused before anything is sent.
    assert not recorder.errors, recorder.errors
    held, _ = _read_back(recorder, client, 1001)
    assert held.secType == "BAG"


def test_a_leg_appended_is_read_as_it_was_written():
    """The second leg and its contents, not only a count: a leg the client
    refuses is refused by its number and its fault."""
    recorder, client = _session()
    client.place_order(1002, _stock_combo(second_leg_action="HOLD"), _limit("BUY", 1, 10.0))
    assert recorder.errors, "a leg trading no way the venue knows must be refused"
    _, _, why = recorder.errors[-1]
    assert "leg 1" in why and "HOLD" in why, why


def test_an_algo_order_built_as_the_sample_builds_one_carries_its_parameters():
    order = _limit("BUY", 100, 400.0)
    # `AvailableAlgoParams.FillTwapParams`, as written, values included.
    order.algoStrategy = "Twap"
    order.algoParams = []
    order.algoParams.append(TagValue("startTime", "09:00:00 US/Eastern"))
    order.algoParams.append(TagValue("endTime", "16:00:00 US/Eastern"))
    order.algoParams.append(TagValue("allowPastEndTime", int(True)))
    assert order.algoParams is order.algoParams
    assert order.algo_params is order.algoParams

    recorder, client = _session()
    client.place_order(1003, _spy(), order)
    _, sent = _read_back(recorder, client, 1003)
    assert sent.algoStrategy == "Twap"
    assert [(tv.tag, tv.value) for tv in sent.algoParams] == [
        ("startTime", "09:00:00 US/Eastern"),
        ("endTime", "16:00:00 US/Eastern"),
        ("allowPastEndTime", "1"),
    ]


def test_a_tag_value_takes_what_the_samples_hand_it():
    """`TagValue("maxPctVol", 0.1)` and `TagValue("forceCompletion", int(True))`
    are how the samples write them; sixty-odd of the calls in those samples
    pass a variable. The reference client keeps `str()` of whatever it is
    given, at construction, so `.value` reads back the same text that goes
    out, and a number written two ways is one parameter."""
    given = TagValue("maxPctVol", 0.1)
    assert (given.tag, given.value) == ("maxPctVol", "0.1")
    assert TagValue("forceCompletion", int(True)).value == "1"
    assert TagValue("timeBetweenOrders", 5).value == TagValue("timeBetweenOrders", "5").value == "5"
    assert TagValue("k", 1.5).value == "1.5"
    assert TagValue("k", 0.1 + 0.2).value == repr(0.1 + 0.2), "a float keeps its own digits"
    # A bare bool reads back as the reference client's `str()` of it. For the
    # strategies this client models, its parser takes that spelling and the
    # wire gets the flag; the samples themselves write `int(flag)`.
    assert TagValue("k", True).value == "True"


def test_a_bare_bool_on_a_modelled_strategy_is_taken_as_the_flag():
    order = _limit("BUY", 100, 400.0)
    order.algoStrategy = "Twap"
    order.algoParams.append(TagValue("allowPastEndTime", True))

    recorder, client = _session()
    client.place_order(1009, _spy(), order)
    _, sent = _read_back(recorder, client, 1009)
    assert [(tv.tag, tv.value) for tv in sent.algoParams] == [("allowPastEndTime", "True")]


def test_a_parameter_that_cannot_be_read_is_refused_by_its_place_before_anything_is_sent():
    """An element carrying neither name is a refusal naming which one, and
    the order is not half sent around it: nothing is tracked under the id."""
    order = _limit("BUY", 100, 400.0)
    order.algoStrategy = "Twap"
    order.algoParams = [TagValue("startTime", "09:00:00 US/Eastern"), ("endTime", "16:00:00 US/Eastern")]

    recorder, client = _session()
    client.place_order(1010, _spy(), order)
    assert recorder.errors, "a parameter this client cannot read must be refused"
    _, _, why = recorder.errors[-1]
    assert "algo parameter 1" in why and "tag" in why, why
    recorder.errors.clear()
    client.req_open_orders()
    client._test_dispatch_once()
    assert 1010 not in recorder.placed, "refused before anything was sent, so nothing is held"


def test_a_condition_appended_as_the_sample_appends_one_holds_the_order():
    # `Program.conditionSamples`: appended to the list a fresh order carries.
    order = _limit("BUY", 100, 400.0)
    condition = PriceCondition()
    condition.conId = 208813720
    condition.exchange = "SMART"
    condition.isMore = False
    condition.triggerMethod = 0
    condition.price = 600.0
    order.conditions.append(condition)
    assert order.conditions is order.conditions

    recorder, client = _session()
    client.place_order(1004, _spy(), order)
    _, sent = _read_back(recorder, client, 1004)
    assert [(c.conId, c.price, c.isMore) for c in sent.conditions] == [
        (208813720, 600.0, False),
    ]


def test_a_leg_price_appended_as_the_sample_appends_one_is_read():
    # `OrderSamples.LimitOrderForComboWithLegPrices`, with a price the client
    # refuses so the refusal names the leg: a price nobody read is not refused.
    order = _limit("BUY", 1, 10.0)
    order.orderComboLegs = []
    for price in [10.0, math.nan]:
        comboLeg = OrderComboLeg()
        comboLeg.price = price
        order.orderComboLegs.append(comboLeg)

    recorder, client = _session()
    client.place_order(1005, _named_stock_combo(), order)
    assert recorder.errors, "a leg priced at no number must be refused"
    assert "order_combo_legs[1]" in recorder.errors[-1][2], recorder.errors


def test_a_routing_parameter_appended_as_the_sample_appends_one_is_refused_by_name():
    # `OrderSamples.ComboLimitOrder(nonGuaranteed=True)`. This protocol has no
    # field for it and the order says so, which it cannot if the append was lost.
    order = _limit("BUY", 1, 10.0)
    order.smartComboRoutingParams = []
    order.smartComboRoutingParams.append(TagValue("NonGuaranteed", "1"))

    recorder, client = _session()
    client.place_order(1006, _named_stock_combo(), order)
    assert recorder.errors, "a routing parameter this protocol cannot carry must be refused"
    assert "smart_combo_routing_params" in recorder.errors[-1][2], recorder.errors


def test_a_miscellaneous_option_appended_is_refused_by_name():
    order = _limit("BUY", 1, 10.0)
    order.orderMiscOptions = []
    order.orderMiscOptions.append(TagValue("cashQtyUsage", "1"))

    recorder, client = _session()
    client.place_order(1007, _spy(), order)
    assert recorder.errors, "an option this protocol cannot carry must be refused"
    assert "order_misc_options" in recorder.errors[-1][2], recorder.errors


def test_a_list_assigned_whole_is_the_list_the_field_holds():
    """Whole-list assignment goes on working, and the list assigned is the one
    held, as on the reference client: a name the caller keeps for it appends
    to the same list."""
    contract = Contract()
    legs = []
    contract.comboLegs = legs
    legs.append(ComboLeg())
    assert contract.comboLegs is legs and len(contract.comboLegs) == 1

    contract.comboLegs = (ComboLeg(), ComboLeg())
    assert isinstance(contract.comboLegs, list) and len(contract.comboLegs) == 2

    # The reference client's own default for four of these fields.
    contract.comboLegs = None
    assert contract.comboLegs == []

    order = Order()
    order.algo_params = [TagValue("a", "b")]
    assert [tv.tag for tv in order.algoParams] == ["a"]


def test_an_order_handed_back_by_a_callback_holds_real_lists_too():
    """An order read back is placed again, so what it hands back is a list."""
    order = _limit("BUY", 100, 400.0)
    order.algoStrategy = "Twap"
    order.algoParams.append(TagValue("startTime", "09:00:00 US/Eastern"))

    recorder, client = _session()
    client.place_order(1008, _spy(), order)
    _, sent = _read_back(recorder, client, 1008)
    sent.algoParams.append(TagValue("endTime", "16:00:00 US/Eastern"))
    assert [tv.tag for tv in sent.algoParams] == ["startTime", "endTime"]
