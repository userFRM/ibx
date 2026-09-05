"""The account snapshot answers only once the download has finished.

After a drop the struct still holds the pre-drop figures, and the first frame
of the new connection restates one of them. Gated on whether anything had been
heard, the snapshot handed back all nineteen fields with most of them from
before the drop -- the buying power a caller sizes an order on among them.
"""
import ibx


def test_the_snapshot_waits_for_the_download():
    c = ibx.EClient(ibx.EWrapper())
    c._test_connect("T")
    c._test_set_account(100000.0, 200000.0, 0.0, 0.0, 0.0)
    assert c.accountSnapshot() is None, "figures the download has not finished stating are not the account"
    c._test_finish_account_download()
    snapshot = c.accountSnapshot()
    assert snapshot is not None and snapshot["buying_power"] == 200000.0
