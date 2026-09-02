"""Regression tests for shared locks held across Python callbacks.

The defect class is a Rust mutex held while user code runs. Because the
callback runs with the GIL held, re-entering a path that takes the same
mutex freezes the whole interpreter rather than failing one call, so every
test here would hang rather than fail if its fix regressed. Each is bounded
by a watchdog that runs in C, because a Python-level timer would itself need
the GIL the frozen thread is holding.
"""
import faulthandler
import pytest
from ibx import EClient, EWrapper


class _watchdog:
    """Abort with a traceback instead of hanging the suite if a lock regresses.

    A `threading.Timer` cannot do this job: its callback needs the GIL, which
    is exactly what the frozen main thread is holding, so the timer never runs
    and the suite wedges until something outside kills it. `faulthandler` arms
    a timer in C that does not need the interpreter.
    """

    def __init__(self, seconds=10):
        self.seconds = seconds

    def start(self):
        faulthandler.dump_traceback_later(self.seconds, exit=True)

    def cancel(self):
        faulthandler.cancel_dump_traceback_later()


def test_disconnect_from_the_1100_handler_does_not_deadlock():
    """A handler answering connectivity loss with disconnect() re-enters the
    event lock. Holding it across the callback froze the interpreter."""
    class W(EWrapper):
        def __init__(self):
            super().__init__()
            self.client = None
            self.saw = 0

        def error(self, req_id, error_time, code, msg, advanced=""):
            if code == 1100:
                self.saw += 1
                self.client.disconnect()

    w = W()
    c = EClient(w)
    w.client = c
    c._test_connect("T")
    c._test_push_disconnect_event()

    t = _watchdog()
    t.start()
    c._test_dispatch_once()
    t.cancel()
    assert w.saw == 1


def test_reconnect_from_the_1100_handler_leaves_the_new_session_connected():
    """The handler disconnects and reconnects inside the callback. Storing the
    disconnected flag after the callback clobbered the new session."""
    class W(EWrapper):
        def __init__(self):
            super().__init__()
            self.client = None

        def error(self, req_id, error_time, code, msg, advanced=""):
            if code == 1100:
                self.client.disconnect()
                self.client._test_connect("T2")

    w = W()
    c = EClient(w)
    w.client = c
    c._test_connect("T")
    c._test_push_disconnect_event()

    t = _watchdog()
    t.start()
    c._test_dispatch_once()
    t.cancel()
    assert c.is_connected(), "the reconnected session must not be marked down"


def test_two_disconnects_in_one_batch_fire_one_1100():
    """One network cut can emit several Disconnected events, and a batch is one
    session. Firing per event took a stale second 1100 into the new session."""
    class W(EWrapper):
        def __init__(self):
            super().__init__()
            self.client = None
            self.saw = 0

        def error(self, req_id, error_time, code, msg, advanced=""):
            if code == 1100:
                self.saw += 1
                if self.saw == 1:
                    self.client.disconnect()
                    self.client._test_connect("T2")

    w = W()
    c = EClient(w)
    w.client = c
    c._test_connect("T")
    c._test_push_disconnect_event()
    c._test_push_disconnect_event()

    t = _watchdog()
    t.start()
    c._test_dispatch_once()
    t.cancel()
    assert w.saw == 1, f"one loss is one 1100, saw {w.saw}"
    assert c.is_connected(), "the reconnected session must survive the second event"


def test_a_session_the_caller_ends_is_not_a_session_that_was_lost():
    """1100 is a loss. The reference client answers `disconnect()` with
    `connectionClosed` and says nothing on the error channel, so a program
    that stands down on connectivity loss must not stand down on the session
    it has just closed."""

    class W(EWrapper):
        def __init__(self):
            super().__init__()
            self.codes = []

        def error(self, req_id, error_time, code, msg, advanced=""):
            self.codes.append(code)

    w = W()
    c = EClient(w)
    c._test_connect("T")
    c._test_push_stopped_event()
    c._test_dispatch_once()
    assert 1100 not in w.codes, w.codes


def test_reading_open_orders_from_the_open_order_callback_does_not_deadlock():
    """`open_order` is delivered from inside the order-cache lookup.

    An ibapi wrapper re-requesting its open orders from that callback is
    ordinary, and the order cache is the same lock the delivery walked.
    """
    class W(EWrapper):
        def __init__(self):
            super().__init__()
            self.client = None
            self.saw = 0

        def open_order(self, order_id, contract, order, state):
            self.saw += 1
            # Re-enters the order cache the delivery is walking. Cancelling
            # takes the same lock; re-requesting would recurse instead.
            self.client.cancel_order(order_id)

    w = W()
    c = EClient(w)
    w.client = c
    c._test_connect("T")
    c._test_track_order(11, 0, "SPY", "BUY", 100.0, 1.0, 0)
    c._test_push_order_update(11, 0, "Submitted", 0.0, 100.0)

    t = _watchdog()
    t.start()
    c._test_dispatch_once()
    t.cancel()
    assert w.saw == 1


def test_a_request_before_connect_can_answer_from_its_own_error_handler():
    """A request issued with no session reports on `error` and returns.

    The control channel is read to decide that, and a handler answering it
    with another request reads the same channel.
    """
    class W(EWrapper):
        def __init__(self):
            super().__init__()
            self.client = None
            self.codes = []

        def error(self, req_id, error_time, code, msg, advanced=""):
            self.codes.append(code)
            if len(self.codes) == 1:
                # Re-enters the control channel this call is deciding on.
                self.client.req_pnl(2, "")

    w = W()
    c = EClient(w)
    w.client = c

    t = _watchdog()
    t.start()
    c.req_pnl(1, "")
    t.cancel()
    assert len(w.codes) == 2, "both the call and its handler were told"


def test_an_order_is_reported_under_the_client_that_placed_it():
    """The reference client keys a trade by the client and the order id.

    Reported under client zero when the session connected under another, its
    open-order and status callbacks land on a key nobody is holding: the
    caller's own trade never updates, and a second one appears beside it.
    """
    from ibx import EClient, EWrapper

    class W(EWrapper):
        def __init__(self):
            super().__init__()
            self.opened = []
            self.status = []

        def open_order(self, order_id, contract, order, state):
            self.opened.append(order.clientId)

        def order_status(self, order_id, status, filled, remaining, avg_price,
                         perm_id, parent_id, last_fill_price, client_id,
                         why_held, mkt_cap_price):
            self.status.append(client_id)

    w = W()
    c = EClient(w)
    c._test_connect("T")
    c._test_set_client_id(7)
    c._test_track_order(21, 0, "SPY", "BUY", 100.0, 1.0, 0)
    c._test_push_order_update(21, 0, "Submitted", 0.0, 100.0)
    c._test_dispatch_once()

    assert w.opened == [7], "the order names the client that placed it"
    assert w.status == [7], "and so does its status"

    # The same on the replay a caller asks for, while the order is still open.
    w.status.clear()
    c.req_open_orders()
    c._test_dispatch_once()
    assert w.status == [7], "a replayed status names the client that placed the order"

    # And so does the status a fill produces. Reported under client zero, the
    # fill files under a key nobody holds: the trade shows Submitted under the
    # session's client and Filled under none, so it reads as still working.
    w.status.clear()
    c._test_push_fill(0, 21, "BUY", 100.0, 1, 0, 0.0)
    c._test_dispatch_once()
    assert w.status == [7], f"a fill's status named client {w.status}, not the session's"
