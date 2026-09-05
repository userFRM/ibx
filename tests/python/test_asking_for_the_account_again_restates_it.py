"""Asking for the account again restates it.

The reference client answers a second reqAccountUpdates(True) with every figure
and accountDownloadEnd again. Here the second ask was answered with nothing at
all, and a caller blocking on the end waited for ever.
"""
import ibx


class Account(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.values = 0
        self.ends = 0

    def updateAccountValue(self, key, val, currency, accountName):
        self.values += 1

    def accountDownloadEnd(self, accountName):
        self.ends += 1

    def error(self, *a):
        pass


def test_asking_again_restates_every_figure_and_the_end():
    w = Account()
    c = ibx.EClient(w)
    c._test_connect("T")
    c.reqAccountUpdates(True, "")
    c._test_set_account(100000.0, 200000.0, 0.0, 0.0, 0.0)
    c._test_finish_account_download()
    c._test_dispatch_once()
    assert w.ends == 1 and w.values > 0, (w.values, w.ends)
    stated = w.values

    c.reqAccountUpdates(True, "")
    c._test_dispatch_once()
    assert w.ends == 2, f"the end is said again: {w.ends}"
    assert w.values == 2 * stated, f"and every figure again: {w.values} after {stated}"
