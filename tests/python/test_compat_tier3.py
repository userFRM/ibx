"""Compatibility tests for Tier 3 ibapi-compatible API additions.

Tests cover:
- Financial Advisor: request_fa, replace_fa
- Display Groups: query_display_groups, subscribe_to_group_events, etc.
- Market Rules: req_market_rule
- Smart Components: req_smart_components
- Soft Dollar Tiers: req_soft_dollar_tiers
- Family Codes: req_family_codes
- Histogram Data: req_histogram_data, cancel_histogram_data
- Server Log Level: set_server_log_level
- User Info: req_user_info
- WSH: req_wsh_meta_data, req_wsh_event_data
- Completed Orders: req_completed_orders
- EWrapper Tier 3 Callbacks
"""

from ibx import EClient, EWrapper, Contract
from conftest import NotConnectedProbe


# ── Helper fixtures ──

def make_client():
    """Create an unconnected EClient + EWrapper pair."""
    w = NotConnectedProbe()
    c = EClient(w)
    return c, w


def make_contract(**kwargs):
    """Create a Contract with optional overrides."""
    c = Contract()
    for k, v in kwargs.items():
        setattr(c, k, v)
    return c


# ═══════════════════════════════════════════════════════════
# Financial Advisor (stubs — no connection check)
# ═══════════════════════════════════════════════════════════

def test_request_fa_stub():
    c, w = make_client()
    c.request_fa(1)  # should not raise, just logs warning


def test_replace_fa_stub():
    c, w = make_client()
    c.replace_fa(1, 1, "<xml/>")  # should not raise


def test_fa_signatures():
    c, w = make_client()
    assert hasattr(c, "request_fa")
    assert hasattr(c, "replace_fa")


# ═══════════════════════════════════════════════════════════
# Display Groups (stubs — no connection check)
# ═══════════════════════════════════════════════════════════

def test_query_display_groups_stub():
    c, w = make_client()
    c.query_display_groups(1)


def test_subscribe_to_group_events_returns_normally():
    c, w = make_client()
    c.subscribe_to_group_events(1, 1)


def test_unsubscribe_from_group_events_returns_normally():
    c, w = make_client()
    c.unsubscribe_from_group_events(1)


def test_update_display_group_without_subscribing_returns_normally():
    """A request that follows no group cannot put a contract in one.

    The reference client answers such a request on the error callback and
    returns normally, so this must not raise: a caller written against it would
    fall over on a request that merely came in the wrong order.
    """
    c, w = make_client()
    c.update_display_group(1, "265598")


def test_display_group_signatures():
    c, w = make_client()
    assert hasattr(c, "query_display_groups")
    assert hasattr(c, "subscribe_to_group_events")
    assert hasattr(c, "unsubscribe_from_group_events")
    assert hasattr(c, "update_display_group")


# ═══════════════════════════════════════════════════════════
# Market Rules (checks connection for cache lookup)
# ═══════════════════════════════════════════════════════════

def test_req_market_rule_not_connected():
    """Without connection, req_market_rule has no shared state — logs warning."""
    c, w = make_client()
    c.req_market_rule(26)  # no shared state → logs warning, no crash


def test_req_market_rule_signature():
    c, w = make_client()
    assert hasattr(c, "req_market_rule")


# ═══════════════════════════════════════════════════════════
# Smart Components (fires empty callback)
# ═══════════════════════════════════════════════════════════

class SmartComponentsCapture(EWrapper):
    def __init__(self):
        super().__init__()
        self.req_id = None
        self.components = None

    def smart_components(self, req_id, smart_component_map):
        self.req_id = req_id
        self.components = smart_component_map


def test_req_smart_components_fires_callback():
    w = SmartComponentsCapture()
    c = EClient(w)
    c._test_connect()
    c.req_smart_components(1, "a]AMEX")
    assert w.req_id == 1
    assert len(w.components) == 0  # Empty map (gateway-local data not available)


def test_req_smart_components_signature():
    c, w = make_client()
    assert hasattr(c, "req_smart_components")


# ═══════════════════════════════════════════════════════════
# Soft Dollar Tiers (gateway-local, returns empty list)
# ═══════════════════════════════════════════════════════════

class SoftDollarTiersCapture(EWrapper):
    def __init__(self):
        super().__init__()
        self.tiers = None
        self.req_id = None

    def soft_dollar_tiers(self, req_id, tiers):
        self.req_id = req_id
        self.tiers = tiers


def test_req_soft_dollar_tiers_fires_callback():
    w = SoftDollarTiersCapture()
    c = EClient(w)
    c._test_connect()
    c.req_soft_dollar_tiers(42)
    assert w.req_id == 42
    assert len(w.tiers) == 0  # Paper accounts return empty


def test_req_soft_dollar_tiers_signature():
    c, w = make_client()
    assert hasattr(c, "req_soft_dollar_tiers")


# ═══════════════════════════════════════════════════════════
# Family Codes (gateway-local, returns account info)
# ═══════════════════════════════════════════════════════════

class FamilyCodesCapture(EWrapper):
    def __init__(self):
        super().__init__()
        self.codes = None

    def family_codes(self, codes):
        self.codes = codes


def test_req_family_codes_fires_callback():
    w = FamilyCodesCapture()
    c = EClient(w)
    c._test_connect()
    c.req_family_codes()
    assert w.codes is not None
    assert isinstance(w.codes, list)
    # Each entry is (accountID, familyCodeStr)


def test_req_family_codes_signature():
    c, w = make_client()
    assert hasattr(c, "req_family_codes")


# ═══════════════════════════════════════════════════════════
# Histogram Data (checks connection)
# ═══════════════════════════════════════════════════════════

def test_req_histogram_data_not_connected():
    c, w = make_client()
    con = make_contract(con_id=265598, symbol="AAPL", sec_type="STK", exchange="SMART")
    c.req_histogram_data(1, con, True, "1 week")
    assert w.not_connected, "the call reports rather than raising"


def test_cancel_histogram_data_not_connected():
    c, w = make_client()
    c.cancel_histogram_data(1)
    assert w.not_connected, "the call reports rather than raising"


def test_histogram_data_signatures():
    c, w = make_client()
    assert hasattr(c, "req_histogram_data")
    assert hasattr(c, "cancel_histogram_data")


# ═══════════════════════════════════════════════════════════
# Historical Schedule (routed via req_historical_data + SCHEDULE)
# ═══════════════════════════════════════════════════════════

def test_req_historical_schedule_not_connected():
    c, w = make_client()
    con = make_contract(con_id=756733, symbol="SPY", sec_type="STK", exchange="SMART")
    c.req_historical_data(1, con, "", "5 D", "1 day", "SCHEDULE", 1)
    assert w.not_connected, "the call reports rather than raising"


def test_req_historical_schedule_signature():
    """SCHEDULE is routed via req_historical_data, not a separate method."""
    c, w = make_client()
    assert hasattr(c, "req_historical_data")


# ═══════════════════════════════════════════════════════════
# Server Log Level (local-only, always succeeds)
# ═══════════════════════════════════════════════════════════

def test_set_server_log_level_all_levels():
    """set_server_log_level should succeed for all valid levels."""
    c, w = make_client()
    for level in [1, 2, 3, 4, 5]:
        c.set_server_log_level(level)


def test_set_server_log_level_default():
    """Default log level (2 = warn)."""
    c, w = make_client()
    c.set_server_log_level()  # uses default


def test_set_server_log_level_signature():
    c, w = make_client()
    assert hasattr(c, "set_server_log_level")


# ═══════════════════════════════════════════════════════════
# User Info (gateway-local, returns empty whiteBrandingId)
# ═══════════════════════════════════════════════════════════

class UserInfoCapture(EWrapper):
    def __init__(self):
        super().__init__()
        self.req_id = None
        self.white_branding_id = None

    def user_info(self, req_id, white_branding_id):
        self.req_id = req_id
        self.white_branding_id = white_branding_id


def test_req_user_info_fires_callback():
    w = UserInfoCapture()
    c = EClient(w)
    c._test_connect()
    c.req_user_info(7)
    assert w.req_id == 7
    assert w.white_branding_id == ""  # Empty on paper


def test_req_user_info_signature():
    c, w = make_client()
    assert hasattr(c, "req_user_info")


# ═══════════════════════════════════════════════════════════
# WSH (stubs)
# ═══════════════════════════════════════════════════════════

def test_req_wsh_meta_data_stub():
    c, w = make_client()
    c.req_wsh_meta_data(1)


def test_req_wsh_event_data_stub():
    c, w = make_client()
    c.req_wsh_event_data(1)


def test_req_wsh_event_data_with_arg():
    c, w = make_client()
    c.req_wsh_event_data(1, None)  # optional arg


def test_wsh_signatures():
    c, w = make_client()
    assert hasattr(c, "req_wsh_meta_data")
    assert hasattr(c, "req_wsh_event_data")


# ═══════════════════════════════════════════════════════════
# Completed Orders (session archive)
# ═══════════════════════════════════════════════════════════

def test_req_completed_orders_no_shared_state():
    """Without connection, req_completed_orders should not crash."""
    c, w = make_client()
    c.req_completed_orders(True)


def test_req_completed_orders_default_arg():
    c, w = make_client()
    c.req_completed_orders()  # api_only defaults to False


def test_req_completed_orders_signature():
    c, w = make_client()
    assert hasattr(c, "req_completed_orders")


# ═══════════════════════════════════════════════════════════
# EWrapper Tier 3 Callbacks (no-op defaults)
# ═══════════════════════════════════════════════════════════

def test_wrapper_display_group_list():
    w = EWrapper()
    w.display_group_list(1, "1|2|3")


def test_wrapper_display_group_updated():
    w = EWrapper()
    w.display_group_updated(1, "265598")


def test_wrapper_market_rule():
    w = EWrapper()
    w.market_rule(26, None)


def test_wrapper_smart_components():
    w = EWrapper()
    w.smart_components(1, None)


def test_wrapper_soft_dollar_tiers():
    w = EWrapper()
    w.soft_dollar_tiers(1, None)


def test_wrapper_family_codes():
    w = EWrapper()
    w.family_codes(None)


def test_wrapper_histogram_data():
    w = EWrapper()
    w.histogram_data(1, None)


def test_wrapper_user_info():
    w = EWrapper()
    w.user_info(1, "")


def test_wrapper_wsh_meta_data():
    w = EWrapper()
    w.wsh_meta_data(1, "{}")


def test_wrapper_wsh_event_data():
    w = EWrapper()
    w.wsh_event_data(1, "{}")


def test_wrapper_completed_order():
    w = EWrapper()
    w.completed_order(None, None, None)


def test_wrapper_completed_orders_end():
    w = EWrapper()
    w.completed_orders_end()


def test_wrapper_order_bound():
    w = EWrapper()
    w.order_bound(1, 0, 1)


def test_wrapper_tick_req_params():
    w = EWrapper()
    w.tick_req_params(1, 0.01, "SMART", 1)


def test_wrapper_bond_contract_details():
    w = EWrapper()
    w.bond_contract_details(1, None)


def test_wrapper_delta_neutral_validation():
    w = EWrapper()
    w.delta_neutral_validation(1, None)


def test_wrapper_historical_schedule():
    w = EWrapper()
    w.historical_schedule(1, "20230101", "20230201", "US/Eastern", None)


# ═══════════════════════════════════════════════════════════
# Positions Multi (one-shot from SharedState)
# ═══════════════════════════════════════════════════════════

def test_wrapper_position_multi():
    w = EWrapper()
    w.position_multi(1, "DU12345", "", None, 100.0, 50.5)


def test_wrapper_position_multi_end():
    w = EWrapper()
    w.position_multi_end(1)


# ═══════════════════════════════════════════════════════════
# Account Updates Multi (one-shot from SharedState)
# ═══════════════════════════════════════════════════════════

def test_wrapper_account_update_multi():
    w = EWrapper()
    w.account_update_multi(1, "DU12345", "", "NetLiquidation", "100000.00", "USD")


def test_wrapper_account_update_multi_end():
    w = EWrapper()
    w.account_update_multi_end(1)


def test_a_request_id_answers_more_than_once():
    """A request id is a caller's label, not a one-shot token.

    Bars answering a fresh request were delivered as though they continued the
    last one — as updates, with no completion — because the id had been marked
    finished by the request before and nothing unmarked it. A program looping
    over contracts under one id was answered once and never again.
    """
    class Probe(EWrapper):
        def __init__(self):
            super().__init__()
            self.bars = 0
            self.updates = 0
            self.ends = 0

        def historical_data(self, req_id, bar):
            self.bars += 1

        def historical_data_update(self, req_id, bar):
            self.updates += 1

        def historical_data_end(self, req_id, start, end):
            self.ends += 1

    w = Probe()
    c = EClient(w)
    c._test_connect("DU000000", False)

    spy = Contract()
    spy.con_id, spy.symbol, spy.sec_type = 756733, "SPY", "STK"
    spy.exchange, spy.currency = "SMART", "USD"

    for _ in range(2):
        c.req_historical_data(500, spy, "", "1 D", "1 hour", "TRADES", 1, 1, False, [])
        c._test_push_historical_data(500, [("20260812 10:00:00", 1.0, 2.0, 0.5, 1.5, 100)], True)
        c._test_dispatch_once()

    assert w.bars == 2, f"both requests answered with bars, got {w.bars}"
    assert w.ends == 2, f"and both said they had finished, got {w.ends}"
    assert w.updates == 0, "and neither was delivered as a continuation of the other"


def test_a_trade_stream_is_not_a_quote_subscription():
    """A stream is held in its own table.

    Held in the quote tables, a request for trades took the contract's quote
    slot and was handed the contract's quotes; and withdrawing it removed that
    slot, so it took the quotes away from whoever was watching them.
    """

    class Quotes(EWrapper):
        def __init__(self):
            super().__init__()
            self.prices = 0

        def tick_price(self, req_id, tick_type, price, attrib=None):
            self.prices += 1

    w = Quotes()
    c = EClient(w)
    c._test_connect("DU000000", False)
    c._test_map_con_id(756733, 3)
    c._test_map_instrument(900, 3)

    # Someone is quoting the contract.
    c._test_push_quote(3, 100.0, 101.0, 100.5, 1, 1, 1, 10, 100.0, 101.0, 99.0, 100.0)
    c._test_dispatch_once()
    quoted_before = w.prices
    assert quoted_before > 0, "the quote reaches the caller watching it"

    # A trade stream is withdrawn on the same contract.
    c.cancel_tick_by_tick_data(901)

    c._test_push_quote(3, 101.0, 102.0, 101.5, 1, 1, 1, 11, 100.0, 102.0, 99.0, 100.0)
    c._test_dispatch_once()
    assert w.prices > quoted_before, (
        "and still does once a stream on the same contract is withdrawn"
    )


def test_a_session_held_open_notices_what_stopped():
    """The check that watches a held-open session has to be able to fail.

    Bars stopping after the seventh minute, with every other stream healthy,
    is what a session found and no offline gate could. A run that printed its
    counters and passed regardless would have found it too, and said nothing.
    """
    import importlib.util
    import pathlib

    here = pathlib.Path(__file__).resolve().parents[2] / "scripts" / "endurance.py"
    spec = importlib.util.spec_from_file_location("endurance", here)
    endurance = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(endurance)

    growing = {"quotes": 100, "bars": 10}
    assert endurance.what_stopped(growing, {"quotes": 200, "bars": 20}, 3) == []

    stalled = endurance.what_stopped(growing, {"quotes": 200, "bars": 10}, 8)
    assert stalled == ["bars stopped arriving after cycle 7"], stalled

    both = endurance.what_stopped(growing, {"quotes": 100, "bars": 10}, 2)
    assert len(both) == 2, both

    # A book and a trade stream are quiet on a contract nobody is trading, so
    # they are not held to every cycle — but a whole run without one is a
    # failure, and the script's own prose said so before the code did.
    assert endurance.REQUIRED_AT_LEAST_ONCE == ("book", "trades")
    ran = {"quotes": 200, "bars": 20, "book": 5, "trades": 3}
    assert [k for k in endurance.REQUIRED_AT_LEAST_ONCE if not ran.get(k)] == []
    silent = {"quotes": 200, "bars": 20, "book": 0}
    assert [k for k in endurance.REQUIRED_AT_LEAST_ONCE if not silent.get(k)] == [
        "book", "trades",
    ]


def test_every_call_and_callback_says_what_it_does():
    """The surface a program touches is documented, all of it.

    Measured rather than reviewed: a callback with no documentation is one a
    caller has to discover by watching what arrives, and the whole callback
    surface was in that state.
    """
    import ibx

    for named, cls in (("EClient", ibx.EClient), ("EWrapper", ibx.EWrapper)):
        public = [n for n in dir(cls) if not n.startswith("_")]
        assert len(public) > 50, f"{named} should carry a substantial surface"
        silent = [n for n in public if not (getattr(cls, n).__doc__ or "").strip()]
        assert not silent, f"{named} says nothing about: {sorted(silent)}"
