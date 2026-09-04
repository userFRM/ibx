"""A snapshot this client withdraws itself is not reported against the caller.

A snapshot ends on `tickSnapshotEnd`, and the ordinary program written around
one disconnects right there: it asked for a price and has it. The withdrawal
of the finished subscription is this client's own doing, made on the same pass
after the callback returns — and made through the public cancel, it reported
504 "Not connected" under the request's own id, for a request that had just
completed, to a caller who had just closed the session on purpose.
"""

from ibx import Contract, EClient, EWrapper


class TakesOneAndLeaves(EWrapper):
    def __init__(self):
        super().__init__()
        self.errors = []
        self.ended = []
        self.client = None

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.errors.append((req_id, code, msg))

    def tickSnapshotEnd(self, req_id):
        self.ended.append(req_id)
        self.client.disconnect()


def test_a_handler_that_disconnects_on_snapshot_end_hears_no_504():
    wrapper = TakesOneAndLeaves()
    client = EClient(wrapper)
    wrapper.client = client
    client._test_connect("DU0000000")
    client._test_set_instrument_count(1)
    client._test_map_con_id(756733, 0)
    # Somebody already holds the contract, so the snapshot follows that
    # subscription and needs no engine to answer the registration.
    client._test_map_instrument(5, 0)
    # Stated in full: a contract carrying an id and nothing else is one this
    # client asks the venue to name, and the naming is not what is under test.
    client.reqMktData(
        1, Contract(conId=756733, secType="STK", exchange="SMART"), "", True, False, [],
    )
    # Every kind a snapshot is made of: bid, ask, last, open and close.
    client._test_push_quote(
        0, bid=10.0, ask=10.5, last=10.2, bid_size=1, ask_size=1, last_size=1,
        volume=9, open=10.0, high=11.0, low=9.0, close=9.9,
    )

    client._test_dispatch_once()

    assert wrapper.ended == [1], wrapper.ended
    assert not client.isConnected()
    assert wrapper.errors == [], f"the finished snapshot was reported against: {wrapper.errors}"
