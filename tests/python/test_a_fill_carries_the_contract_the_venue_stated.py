"""A fill carries the contract the venue stated, not the one the caller typed.

Placed by symbol, the order's own record holds no contract id; the venue's
report names one. Handed the typed contract, a program keying fills to
positions by contract id matched nothing on this surface and everything on
the other, and the reference client names the id on every fill.
"""

import ibx


class Fills(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.contracts = []

    def execDetails(self, reqId, contract, execution):
        self.contracts.append(contract.conId)

    def error(self, *a):
        pass


def test_a_fill_carries_the_contract_the_venue_stated():
    w = Fills()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_track_order(86, 0, "SPY", "BUY", 1, 100.0)
    c._test_push_venue_order(86, "SPY", "BUY", 1, 100.0, con_id=756733)
    c._test_push_fill(0, 86, "BUY", 100.0, 1, 0)
    c._test_dispatch_once()
    assert w.contracts == [756733], f"the venue's contract, id and all: {w.contracts}"
