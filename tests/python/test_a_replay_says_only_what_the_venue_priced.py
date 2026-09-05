"""A replay says nothing about what a fill cost until the venue has.

An execution is stored with its charge deliberately unstated, and every
execution the venue replays at logon is stored that way and never charged.
Reported regardless, the caller was handed a charge saying the fill cost
nothing, in no currency, naming no execution -- the exact statement the
unstated storage exists to avoid.
"""

import ibx


class Costs(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.fills = []
        self.charges = []

    def execDetails(self, reqId, contract, execution):
        self.fills.append(execution.execId)

    def commissionAndFeesReport(self, report):
        self.charges.append(report.execId)

    def error(self, *a):
        pass


class Filter:
    def __init__(self):
        self.clientId, self.acctCode, self.time = 0, "", ""
        self.symbol, self.secType, self.exchange, self.side = "", "", "", ""


def test_only_a_priced_fill_says_what_it_cost():
    w = Costs()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_instrument(90, 0)
    # One fill the venue priced, one it has not.
    c._test_push_fill(0, 1, "BUY", 10.0, 5, 0, 1.25)
    c._test_push_fill(0, 2, "BUY", 10.0, 5, 0)
    c._test_dispatch_once()
    c._test_dispatch_once()
    w.fills.clear()
    w.charges.clear()

    c.reqExecutions(9, Filter())
    c._test_dispatch_once()

    assert len(w.fills) == 2, f"both fills are replayed: {w.fills}"
    assert "" not in w.charges, (
        f"a fill the venue has not priced says nothing about a cost: {w.charges}"
    )
