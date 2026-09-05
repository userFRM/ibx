"""A fill's client is the one that placed the order where the report names none.

One pass announced orderStatus under the placing client and filed the same print
under client zero, so a caller replaying its own fills by client id got none of
them.
"""
import ibx


class Filed(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.clients = []

    def execDetails(self, reqId, contract, execution):
        self.clients.append(execution.clientId)

    def error(self, *a):
        pass


def test_a_report_naming_no_client_files_the_fill_under_the_placing_client():
    w = Filed()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_set_client_id(5)
    c._test_track_order(86, 0, "SPY", "BUY", 1, 100.0)
    c._test_push_venue_order(86, "SPY", "BUY", 1, 100.0)
    c._test_push_fill(0, 86, "BUY", 100.0, 1, 0)
    c._test_dispatch_once()
    assert w.clients == [5], f"filed under the client that placed it: {w.clients}"
