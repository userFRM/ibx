"""A program driving its own loop is told the session closed after `disconnect()`.

`poll()` returned before the notice once the session was taken away, so an
asyncio program waiting on `connectionClosed` to know it may exit waited for
ever. A client that never connected is told nothing.
"""

import ibx


class Closed(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.closed = 0

    def connectionClosed(self):
        self.closed += 1


def test_poll_says_the_session_closed_after_disconnect():
    w = Closed()
    c = ibx.EClient(w)
    c._test_connect("T")
    c.disconnect()
    c.poll()
    c.poll()
    assert w.closed == 1, f"said once, on the pass after: {w.closed}"


def test_a_client_that_never_connected_is_not_told_a_session_closed():
    w = Closed()
    c = ibx.EClient(w)
    c.disconnect()
    c.poll()
    assert w.closed == 0
