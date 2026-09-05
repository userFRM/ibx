"""A charge is never reported before the fill it names.

The engine pushes a fill and then, off a message of its own, the charge that
names it. The charges were taken after the fills, so a pair written while the
dispatcher was inside a caller's callback split: the charge was read in that
pass and its fill in the next. The charge then named an execution nothing had
stored, so it updated nothing -- the fill was filed for a replay with its cost
unknown for ever -- and a caller that files fills first and costs second
dropped it.
"""

import ibx


class Sequence(ibx.EWrapper):
    """Records the order the two callbacks arrive in, and writes mid-pass."""

    def __init__(self):
        super().__init__()
        self.seen = []
        self.client = None
        self.wrote = False

    def execDetails(self, reqId, contract, execution):
        self.seen.append("fill")
        if not self.wrote:
            self.wrote = True
            # The engine writing while the dispatcher is inside a callback,
            # which is the moment the two drains straddle.
            self.client._test_push_fill(0, 78, "BUY", 150.0, 10, 0, 1.25)

    def commissionAndFeesReport(self, report):
        self.seen.append("charge")

    def error(self, *a):
        pass


def test_the_charge_written_mid_pass_arrives_after_its_fill():
    w = Sequence()
    c = ibx.EClient(w)
    w.client = c
    c._test_connect("T")
    c._test_push_fill(0, 77, "BUY", 150.0, 10, 0)
    c._test_dispatch_once()
    c._test_dispatch_once()

    assert w.seen.count("fill") == 2, f"both fills were delivered: {w.seen}"
    assert w.seen.count("charge") == 1, f"the charge was delivered once: {w.seen}"
    assert w.seen.index("charge") > w.seen.index("fill", 1), (
        f"the charge was read before the fill it names: {w.seen}"
    )
