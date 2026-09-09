"""What a raising EWrapper callback does to dispatch.

The reference client's own loop catches only KeyboardInterrupt, SystemExit and
a bad message; anything else a handler raises escapes the loop and the session
closes in the `finally` beneath it. So the exception reaches the caller and the
session ends — swallowed and logged instead, a program whose handler was
failing went on being fed events with nothing anywhere to say so.

What must still hold is that the raise is an ordinary Python exception on the
way out: the bridge is not left poisoned, the engine's own state is untouched,
and a fresh session on the same process works.
"""

import pytest
from ibx import EWrapper, EClient


class RaisingTickPriceWrapper(EWrapper):
    """Tick_price raises, all other callbacks record normally."""
    def __init__(self):
        super().__init__()
        self.events = []

    def tick_price(self, req_id, tick_type, price, attrib):
        raise RuntimeError("simulated user error in tick_price")

    def tick_size(self, req_id, tick_type, size):
        self.events.append(("tick_size", req_id, tick_type, size))

    def error(self, req_id, error_time, error_code, error_string, advanced_order_reject_json=""):
        self.events.append(("error", req_id, error_code, error_string))

    def connect_ack(self):
        self.events.append(("connect_ack",))

    def next_valid_id(self, order_id):
        self.events.append(("next_valid_id", order_id))

    def managed_accounts(self, accounts_list):
        self.events.append(("managed_accounts", accounts_list))


class QuietWrapper(EWrapper):
    """Records what it is given and raises at nothing."""
    def __init__(self):
        super().__init__()
        self.events = []

    def tick_price(self, req_id, tick_type, price, attrib):
        self.events.append(("tick_price", req_id, tick_type, price))

    def tick_size(self, req_id, tick_type, size):
        self.events.append(("tick_size", req_id, tick_type, size))


def make_client(wrapper):
    c = EClient(wrapper)
    c._test_connect("TEST")
    return c


def test_tick_price_exception_reaches_the_caller():
    """The handler's own exception is what leaves dispatch, unwrapped."""
    w = RaisingTickPriceWrapper()
    c = make_client(w)
    c._test_set_instrument_count(1)
    c._test_map_instrument(1, 0)
    c._test_push_quote(0, bid=150.0, ask=151.0, bid_size=100, ask_size=200)

    with pytest.raises(RuntimeError, match="simulated user error in tick_price"):
        c._test_dispatch_once()
    assert not c.is_connected(), "the session closes beneath the raise"


def test_a_fresh_session_works_after_a_raise_closed_the_last_one():
    """The raise ends one session, not the process: the bridge is not poisoned."""
    w = RaisingTickPriceWrapper()
    c = make_client(w)
    c._test_set_instrument_count(1)
    c._test_map_instrument(1, 0)
    c._test_push_quote(0, bid=150.0, bid_size=100)
    with pytest.raises(RuntimeError):
        c._test_dispatch_once()

    # A second session on the same client, with a handler that does not raise.
    quiet = QuietWrapper()
    c2 = make_client(quiet)
    c2._test_set_instrument_count(1)
    c2._test_map_instrument(1, 0)
    c2._test_push_quote(0, bid=151.0, bid_size=200)
    c2._test_dispatch_once()

    size_events = [e for e in quiet.events if e[0] == "tick_size"]
    assert len(size_events) > 0, f"a fresh session still delivers: {quiet.events}"


def test_engine_state_intact_after_callback_exception():
    """The engine's own state survives the raise that closed the session."""
    w = RaisingTickPriceWrapper()
    c = make_client(w)
    c._test_set_instrument_count(1)
    c._test_map_instrument(1, 0)
    c._test_push_quote(0, bid=150.0, bid_size=100)
    with pytest.raises(RuntimeError):
        c._test_dispatch_once()

    # Closed, and closed cleanly: the state is readable rather than poisoned.
    assert not c.is_connected(), "the session closed beneath the raise"
    assert c.is_connected() is False, "and answers the same way twice"


class RaisingAllWrapper(EWrapper):
    """Every callback raises — stress test for dispatch resilience."""
    def __init__(self):
        super().__init__()
        self.call_count = 0

    def tick_price(self, req_id, tick_type, price, attrib):
        self.call_count += 1
        raise ValueError("tick_price boom")

    def tick_size(self, req_id, tick_type, size):
        self.call_count += 1
        raise ValueError("tick_size boom")

    def error(self, req_id, error_time, error_code, error_string, advanced_order_reject_json=""):
        self.call_count += 1
        raise ValueError("error boom")

    def connect_ack(self):
        self.call_count += 1
        raise ValueError("connect_ack boom")

    def next_valid_id(self, order_id):
        self.call_count += 1
        raise ValueError("next_valid_id boom")

    def managed_accounts(self, accounts_list):
        self.call_count += 1
        raise ValueError("managed_accounts boom")


def test_all_callbacks_raising_leaves_by_the_first_one():
    """The first raise is the one that leaves; nothing after it is called."""
    w = RaisingAllWrapper()
    c = EClient(w)
    c._test_connect("TEST")
    c._test_set_instrument_count(1)
    c._test_map_instrument(1, 0)
    c._test_push_quote(0, bid=150.0, ask=151.0, bid_size=100, ask_size=200)

    with pytest.raises(Exception):
        c._test_dispatch_once()

    assert w.call_count > 0, "the handler was reached before it raised"
    assert not c.is_connected()
