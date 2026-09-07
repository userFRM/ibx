"""A snapshot's own withdrawal does not take the subscription its callback made.

A completed snapshot is withdrawn by this client rather than by the caller, and
that withdrawal runs after `tickSnapshotEnd` has been delivered. A handler is
free to withdraw what it has just been told about and ask for something else
under the same number — "the snapshot is in, now stream it" is the obvious
thing to write against this callback — and the withdrawal that followed took
the number alone, so it cancelled the subscription the handler had just made.
The caller was left holding a number that reads as subscribed with nothing
arriving on it.
"""

from ibx import Contract, EClient, EWrapper


class StreamsAfterTheSnapshot(EWrapper):
    def __init__(self):
        super().__init__()
        self.client = None
        self.ended = []

    def tickSnapshotEnd(self, req_id):
        self.ended.append(req_id)
        # The obvious thing to write here: done with the one-shot, now stream
        # the same contract, under the number already in hand.
        self.client.cancelMktData(req_id)
        self.client.reqMktData(
            req_id, Contract(conId=756733, secType="STK", exchange="SMART"),
            "", False, False, [],
        )


def test_the_stream_a_snapshot_callback_asked_for_survives_the_snapshot():
    wrapper = StreamsAfterTheSnapshot()
    client = EClient(wrapper)
    wrapper.client = client
    client._test_connect("DU0000000")
    client._test_set_instrument_count(1)
    client._test_map_con_id(756733, 0)
    # Somebody already holds the contract, so both the snapshot and the stream
    # the callback asks for follow that subscription and need no engine to
    # answer their registration.
    client._test_map_instrument(5, 0)

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
    assert client._test_watching(1) is not None, (
        "the withdrawal that follows a snapshot took the subscription its own "
        "callback had just asked for"
    )
