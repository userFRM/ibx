"""What crosses the bridge to `ib_async` crosses whole.

A request names a contract and a callback carries a record. Both are rebuilt on
the way over, and both were being rebuilt short: the request paths copied a
fixed list of fields, so a combination arrived with no legs, and the one
callback renamed on the way through skipped the rebuild entirely, so a fill's
cost arrived as a type their wrapper cannot read.
"""

import ib_async

import ibx
from ibx.ib_async import IbxClient, _LoopBound


def _combo():
    c = ib_async.Contract(secType="BAG", symbol="SPY", exchange="SMART", currency="USD")
    c.comboLegs = [
        ib_async.ComboLeg(conId=1, ratio=1, action="BUY", exchange="SMART"),
        ib_async.ComboLeg(conId=2, ratio=2, action="SELL", exchange="SMART"),
    ]
    return c


class Sent:
    """Stands in for the client, keeping the contract each request was given."""

    def __init__(self):
        self.contracts = []

    def _keep(self, contract):
        self.contracts.append(contract)

    def req_contract_details(self, req_id, contract):
        self._keep(contract)

    def req_mkt_data(self, req_id, contract, *a):
        self._keep(contract)

    def req_historical_data(self, req_id, contract, *a):
        self._keep(contract)


def _client():
    ib = ib_async.IB()
    c = IbxClient(ib.wrapper)
    c._client = Sent()
    return c


def test_a_combination_keeps_its_legs_on_every_request_path():
    c = _client()
    combo = _combo()
    c.reqContractDetails(1, combo)
    c.reqMktData(2, combo, "", False, False, None)
    c.reqHistoricalData(3, combo, "", "1 D", "1 min", "TRADES", True, 1, False, None)

    assert len(c._client.contracts) == 3
    for sent in c._client.contracts:
        legs = sent.comboLegs
        assert [leg.conId for leg in legs] == [1, 2], f"the legs were dropped: {legs}"
        assert [leg.ratio for leg in legs] == [1, 2]


def test_a_contract_named_by_more_than_its_symbol_keeps_the_rest():
    c = _client()
    contract = ib_async.Contract(secType="STK", symbol="SPY", exchange="SMART", currency="USD")
    contract.secIdType = "ISIN"
    contract.secId = "US78462F1030"
    contract.includeExpired = True
    c.reqContractDetails(1, contract)

    sent = c._client.contracts[0]
    assert sent.secIdType == "ISIN" and sent.secId == "US78462F1030"
    assert sent.includeExpired is True


def test_a_fills_cost_arrives_as_the_record_their_wrapper_reads():
    """The callback is renamed on the way over, and was handed the argument
    unrebuilt. Their wrapper reads a field their own record spells its own
    way, so every fill lost what it cost."""
    seen = []

    class Wrapper:
        def commissionReport(self, report):
            seen.append(report)

    bound = _LoopBound(Wrapper())
    ours = ibx.CommissionAndFeesReport()
    ours.execId = "0001.1"
    ours.commissionAndFees = 1.25
    ours.currency = "USD"
    bound.commission_and_fees_report(ours)

    assert seen, "the cost reached nothing"
    assert isinstance(seen[0], ib_async.CommissionReport), type(seen[0])
    assert seen[0].commission == 1.25
    assert seen[0].execId == "0001.1"


def test_the_order_id_file_is_the_one_the_caller_named(tmp_path):
    """Stored and never passed on, a custom path and an opt-out both did
    nothing, and the counter restarted with the account's ids already used."""
    named = tmp_path / "order-ids"
    passed = {}

    class Recording:
        def connect(self, **kwargs):
            passed.update(kwargs)

        def get_account_id(self):
            return "DU1"

        def req_managed_accts(self):
            pass

        def next_order_id(self):
            return 1

        def poll(self):
            pass

    import asyncio

    ib = ib_async.IB()
    c = IbxClient(ib.wrapper, order_id_file=str(named))
    c._client = Recording()
    c._start_pump = lambda: None
    asyncio.run(c.connectAsync("", 0, 1))
    assert passed.get("order_id_file") == str(named), passed


def test_every_account_the_login_holds_crosses_over():
    """The default account read off the client is the first one. Used as the
    whole list, an advisor with several saw one standing for all of them."""
    ib = ib_async.IB()
    c = IbxClient(ib.wrapper)

    class Several:
        def connect(self, **kwargs):
            pass

        def get_account_id(self):
            return "DU1"

        def req_managed_accts(self):
            c._callbacks.managed_accounts("DU1,DU2,DU3")

        def next_order_id(self):
            return 1

    c._client = Several()
    c._start_pump = lambda: None

    import asyncio
    asyncio.run(c.connectAsync("", 0, 1))

    assert ib.managedAccounts() == ["DU1", "DU2", "DU3"], ib.managedAccounts()
    assert c.getAccounts() == ["DU1", "DU2", "DU3"], c.getAccounts()


def test_a_size_with_no_price_behind_it_states_no_price():
    """A zero there is a market quoted at nothing, which is not the same as a
    market nobody has quoted yet."""
    seen = []

    class Wrapper:
        defaultEmptyPrice = -1

        def priceSizeTick(self, reqId, tickType, price, size):
            seen.append((tickType, price, size))

    bound = _LoopBound(Wrapper())
    # A bid size arrives with no bid price ever having been stated.
    bound.tick_size(1, 0, 400.0)
    assert seen == [(1, -1, 400.0)], seen
