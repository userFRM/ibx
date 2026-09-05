"""A fill carries the account the report names, as the other surface reads it.

Labelled with the session's account instead, every fill in an allocated session
read as the master account's, and a request filtered by account answered the two
surfaces differently for one filter.
"""
import ibx


class Filed(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.accounts = []

    def execDetails(self, reqId, contract, execution):
        self.accounts.append(execution.acctNumber)

    def error(self, *a):
        pass


def test_the_account_is_the_report_s():
    w = Filed()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_push_venue_order(86, "SPY", "BUY", 1, 100.0, acct_number="DU999")
    c._test_push_fill(0, 86, "BUY", 100.0, 1, 0)
    c._test_dispatch_once()
    assert w.accounts == ["DU999"], f"the report's account, not the session's: {w.accounts}"
