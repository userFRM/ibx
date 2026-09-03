"""A write to the soft-dollar tier is kept.

The reference client's `Order` carries one `SoftDollarTier` object, and a
program directs commission by writing to it: `order.softDollarTier.name = ...`
and `order.softDollarTier.val = ...`, or by building one as that client builds
one, `SoftDollarTier(name, val, displayName)`, from what `reqSoftDollarTiers`
handed back. Built fresh on every read from three strings on the order, the
tier took each write on a temporary and threw it away: the order went to the
venue directing its commission nowhere, and nothing said so. And the class
took no arguments, so the constructor that client's programs use was a
`TypeError`.

Each test writes the tier the way such a program writes it, then follows it
through `place_order` to what the client reads at send time.
"""

from types import SimpleNamespace

import pytest

from ibx import Contract, EClient, EWrapper, Order, SoftDollarTier


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


def _session():
    recorder = _Recorder()
    client = EClient(recorder)
    client._test_connect("DU0000000")
    client._test_map_con_id(SPY_CON_ID, 0)
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


def _limit():
    order = Order()
    order.action = "BUY"
    order.orderType = "LMT"
    order.totalQuantity = 100
    order.lmtPrice = 400.0
    return order


def test_a_write_to_a_field_of_the_tier_is_the_tier_the_order_goes_out_with():
    order = _limit()
    order.softDollarTier.name = "Tier A"
    order.softDollarTier.val = "45.5"
    assert order.softDollarTier is order.softDollarTier, "two reads, one object"
    assert order.soft_dollar_tier is order.softDollarTier, "under either spelling"
    assert (order.softDollarTier.name, order.softDollarTier.val) == ("Tier A", "45.5")

    recorder, client = _session()
    client.place_order(1001, _spy(), order)
    _, sent = _read_back(recorder, client, 1001)
    assert (sent.softDollarTier.name, sent.softDollarTier.val) == ("Tier A", "45.5")


def test_a_tier_is_built_the_way_the_reference_client_builds_one():
    tier = SoftDollarTier("Tier A", "45.5", "Tier A (45.5)")
    assert (tier.name, tier.val, tier.displayName) == ("Tier A", "45.5", "Tier A (45.5)")
    named = SoftDollarTier(name="Tier B", val="12", displayName="Tier B (12)")
    assert (named.name, named.val, named.displayName) == ("Tier B", "12", "Tier B (12)")
    empty = SoftDollarTier()
    assert (empty.name, empty.val, empty.displayName) == ("", "", "")
    with pytest.raises(TypeError):
        SoftDollarTier("Tier A", "45.5", "Tier A (45.5)", "a fourth the reference does not take")


def test_a_tier_assigned_whole_is_the_tier_the_order_holds():
    """Whole-object assignment goes on working, and the object assigned is the
    one held, as on the reference client: a name the caller keeps for it goes
    on writing to the order's tier."""
    order = _limit()
    tier = SoftDollarTier("Tier A", "45.5")
    order.softDollarTier = tier
    assert order.softDollarTier is tier
    tier.val = "46"
    assert order.softDollarTier.val == "46"

    recorder, client = _session()
    client.place_order(1002, _spy(), order)
    _, sent = _read_back(recorder, client, 1002)
    assert (sent.softDollarTier.name, sent.softDollarTier.val) == ("Tier A", "46")


def test_another_clients_tier_is_taken_by_the_names_the_reference_gives_them():
    """An object of some other client's type carrying the three names is read
    by them; one carrying none is refused by the name it lacks, not taken as
    an empty tier."""
    order = _limit()
    order.softDollarTier = SimpleNamespace(name="Tier A", val="45.5", displayName="Tier A (45.5)")
    assert isinstance(order.softDollarTier, SoftDollarTier)
    assert (order.softDollarTier.name, order.softDollarTier.val, order.softDollarTier.displayName) == (
        "Tier A", "45.5", "Tier A (45.5)",
    )
    with pytest.raises(AttributeError, match="name"):
        order.softDollarTier = "Tier A"
