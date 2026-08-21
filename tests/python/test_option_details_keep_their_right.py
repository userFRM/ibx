"""An option's details name a call or a put, in the venue's letters.

The right is what tells one option from the other on the same strike, and the
details are read to be used: the contract inside them goes back out on the next
request. Left off, that request names whichever of the two the venue picks; sent
back as the word this crate spells it with, it names neither.
"""

import ibx


class Details(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.details = []

    def contractDetails(self, reqId, contractDetails):
        self.details.append(contractDetails)

    def error(self, *a):
        pass


def _details_for(right):
    w = Details()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_push_contract_details(1, 756733, "SPY", right)
    c._test_dispatch_once()
    assert w.details, "no details arrived"
    return w.details[0]


def test_the_contract_in_the_details_carries_the_right():
    assert _details_for("C").contract.right == "C"
    assert _details_for("P").contract.right == "P"


def test_the_right_beside_the_contract_is_the_wire_letter():
    assert _details_for("C").right == "C"
    assert _details_for("P").right == "P"


def test_a_contract_with_no_right_states_none():
    d = _details_for("")
    assert d.right == "" and d.contract.right == ""
