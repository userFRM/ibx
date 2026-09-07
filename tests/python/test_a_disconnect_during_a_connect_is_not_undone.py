"""A disconnect that lands while connect is still logging in is not undone.

`connect` claims the connected flag, then releases the GIL for the whole logon
— which is what this client's own documentation asks a caller to do, run it on
a worker thread and time it out yourself. A `disconnect()` arriving in that
window finds nothing installed to stop, clears what is already clear, and says
the session closed. The connect behind it then installed the session it had
just opened and said it was connected, so the caller was left holding a client
it had been told had closed, in front of a logged-in venue session.

Driven here through the counter rather than a real logon: what the fix turns on
is that a disconnect during the window is COUNTED, and that connect reads the
count either side of it.
"""

import ibx


def test_a_disconnect_is_counted_so_a_connect_can_see_it():
    w = ibx.EWrapper()
    c = ibx.EClient(w)
    c._test_connect("T")
    before = c._test_disconnects()
    c.disconnect()
    assert c._test_disconnects() == before + 1, (
        "a disconnect a connect could race has to be visible to it"
    )


def test_a_disconnect_with_no_session_still_counts():
    """The window's whole point: there is nothing installed to stop yet."""
    w = ibx.EWrapper()
    c = ibx.EClient(w)
    before = c._test_disconnects()
    c.disconnect()
    assert c._test_disconnects() == before + 1, (
        "the disconnect that finds nothing to stop is the one that races a connect"
    )


def test_repeated_disconnects_each_count():
    w = ibx.EWrapper()
    c = ibx.EClient(w)
    c._test_connect("T")
    before = c._test_disconnects()
    c.disconnect()
    c.disconnect()
    assert c._test_disconnects() == before + 2
