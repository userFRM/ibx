"""A single-position profit request naming another account is told what the
account-level one is told: the figures are this session's account's."""
import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.seen.append((req_id, code, msg))


def test_req_pnl_single_says_whose_figures_it_answers_with():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    c.reqPnLSingle(7, "DU999", "", 265598)
    assert [(r, code) for r, code, _ in w.seen] == [(7, 321)], w.seen
    assert "DU999" in w.seen[0][2]
