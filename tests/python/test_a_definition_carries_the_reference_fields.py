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
from ibx import UNSET_DOUBLE, ContractDetails, IneligibilityReason, TagValue


class Details(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.details = []
        self.on = []

    def contractDetails(self, reqId, contractDetails):
        self.details.append(contractDetails)
        self.on.append("contractDetails")

    def bondContractDetails(self, reqId, contractDetails):
        self.details.append(contractDetails)
        self.on.append("bondContractDetails")

    def error(self, *a):
        pass


def _heard(sec_type, last_trade_date):
    w = Details()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_push_contract_details(1, 4, "T", "", sec_type, last_trade_date)
    c._test_dispatch_once()
    assert w.details, "no details arrived"
    return w


def _details_for(sec_type, last_trade_date):
    return _heard(sec_type, last_trade_date).details[0]


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
    # The reference client leaves the size and precision figures unset too.
    assert d.minSize == UNSET_DOUBLE
    assert d.sizeIncrement == UNSET_DOUBLE
    assert d.suggestedSizeIncrement == UNSET_DOUBLE
    assert d.lastPricePrecision == UNSET_DOUBLE
    assert d.lastSizePrecision == UNSET_DOUBLE
    assert (d.eventContract1, d.eventContractDescription1, d.eventContractDescription2) == ("", "", "")


def test_a_bond_states_its_maturity_and_no_expiry():
    d = _details_for("BOND", "20300615")
    assert d.maturity == "20300615"
    assert d.contract.lastTradeDateOrContractMonth == ""


def test_any_other_type_states_its_expiry_and_no_maturity():
    d = _details_for("OPT", "20260918")
    assert d.maturity == ""
    assert d.contract.lastTradeDateOrContractMonth == "20260918"


def test_the_security_id_list_shares_so_an_append_reaches_the_field():
    """The reference client's decoder builds secIdList by appending to it; the
    field is the shared list, so an append is kept rather than lost on a copy."""
    d = ContractDetails()
    d.secIdList.append(TagValue("ISIN", "US0378331005"))
    d.secIdList.append(TagValue("CUSIP", "037833100"))
    assert [(t.tag, t.value) for t in d.secIdList] == [
        ("ISIN", "US0378331005"), ("CUSIP", "037833100"),
    ]


def test_the_derivative_sec_types_share_so_an_append_reaches_the_field():
    from ibx import ContractDescription
    cd = ContractDescription(265598, "SPY", "STK", "USD", "SMART", [])
    cd.derivativeSecTypes.append("OPT")
    cd.derivativeSecTypes.append("WAR")
    assert list(cd.derivativeSecTypes) == ["OPT", "WAR"]


def test_a_dealable_contract_names_no_reason_it_cannot_be():
    """Nothing rather than an empty list, as the reference client holds it: a
    program that tests the field rather than its length reads the same thing."""
    assert ContractDetails().ineligibilityReasonList is None


def test_a_reason_reads_under_the_reference_names():
    r = IneligibilityReason("8", "not offered to this account")
    assert (r.id_, r.description) == ("8", "not offered to this account")
    # The reference client puts str() round both, so a reason nobody filled in
    # holds the word for nothing rather than an empty string.
    assert (IneligibilityReason().id_, IneligibilityReason().description) == ("None", "None")


def test_a_reason_is_importable_where_the_reference_client_keeps_it():
    """That client keeps it in a module of its own, and a program written
    against it imports the module rather than the flat name."""
    from ibx.ineligibility_reason import IneligibilityReason as Reference

    assert Reference is IneligibilityReason


def test_fixed_income_answers_on_the_callback_written_for_it():
    """A bond, a bill and the type the venue spells `FIXED` share one callback,
    and the venue answers them with a different set of fields from every other
    type's. Answered on the ordinary one, a program written to wait for a bond
    waited through its own answer."""
    for sec_type in ("BOND", "BILL", "FIXED"):
        assert _heard(sec_type, "20300615").on == ["bondContractDetails"], sec_type
    assert _heard("STK", "").on == ["contractDetails"]


def test_a_bill_and_a_fixed_state_their_maturity_like_a_bond():
    """The venue answers all three the same way, so all three file the date
    where the reference client's bond decoder files it."""
    for sec_type in ("BILL", "FIXED"):
        d = _details_for(sec_type, "20300615")
        assert d.maturity == "20300615", sec_type
        assert d.contract.lastTradeDateOrContractMonth == "", sec_type
