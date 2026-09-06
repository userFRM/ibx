"""A contract's details carry the fields the reference client's carry.

The security-id list came back as pairs where that client holds TagValue
objects, so `.tag` and `.value` raised on every entry. A bond's date went to
the contract's expiry, where that client files it as the details' `maturity`
and leaves the expiry empty. And `maturity`, `minAlgoSize`, `eventContract1`
and the two event descriptions were not there at all: a program reading them
raised before it read.

Run: pytest tests/python/test_a_definition_carries_the_reference_fields.py -v
"""

import ibx
from ibx import UNSET_DOUBLE, ContractDetails, TagValue


class Details(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.details = []

    def contractDetails(self, reqId, contractDetails):
        self.details.append(contractDetails)

    def error(self, *a):
        pass


def _details_for(sec_type, last_trade_date):
    w = Details()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_push_contract_details(1, 4, "T", "", sec_type, last_trade_date)
    c._test_dispatch_once()
    assert w.details, "no details arrived"
    return w.details[0]


def test_the_security_ids_are_tag_values():
    d = ContractDetails()
    assert d.secIdList == []
    d.sec_id_list = [TagValue("ISIN", "US0378331005")]
    entry = d.secIdList[0]
    assert (entry.tag, entry.value) == ("ISIN", "US0378331005")


def test_the_fields_start_unstated():
    d = ContractDetails()
    assert d.maturity == ""
    assert d.minAlgoSize == UNSET_DOUBLE
    assert (d.eventContract1, d.eventContractDescription1, d.eventContractDescription2) == ("", "", "")


def test_a_bond_states_its_maturity_and_no_expiry():
    d = _details_for("BOND", "20300615")
    assert d.maturity == "20300615"
    assert d.contract.lastTradeDateOrContractMonth == ""


def test_any_other_type_states_its_expiry_and_no_maturity():
    d = _details_for("OPT", "20260918")
    assert d.maturity == ""
    assert d.contract.lastTradeDateOrContractMonth == "20260918"
