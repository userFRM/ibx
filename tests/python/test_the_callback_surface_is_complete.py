"""Every callback this client can fire is one a subclass can override.

The list is written out rather than read off the class, so it cannot shrink to
match what happens to be there. A callback that loses its name, or is spelled
one way in dispatch and another on the base class, reaches the caller's code
never and says nothing about it — which is the failure this whole surface is
built to avoid.

81 callbacks, the count the reference table publishes.
"""

from ibx import EWrapper

CALLBACKS = [
    "connect_ack", "connection_closed", "next_valid_id", "managed_accounts",
    "error", "current_time", "tick_price", "tick_size", "tick_string",
    "tick_generic", "tick_snapshot_end", "market_data_type",
    "tick_req_params", "order_status", "open_order", "open_order_end",
    "order_bound", "exec_details", "exec_details_end",
    "commission_and_fees_report", "update_account_value",
    "update_portfolio", "update_account_time", "account_download_end",
    "account_summary", "account_summary_end", "position", "position_end",
    "pnl", "pnl_single", "position_multi", "position_multi_end",
    "account_update_multi", "account_update_multi_end", "contract_details",
    "contract_details_end", "bond_contract_details", "symbol_samples",
    "historical_data", "historical_data_end", "historical_data_update",
    "head_timestamp", "historical_ticks", "historical_ticks_bid_ask",
    "historical_ticks_last", "histogram_data", "historical_schedule",
    "update_mkt_depth", "update_mkt_depth_l2", "mkt_depth_exchanges",
    "tick_by_tick_all_last", "tick_by_tick_bid_ask",
    "tick_by_tick_mid_point", "scanner_data", "scanner_data_end",
    "scanner_parameters", "news_providers", "news_article",
    "historical_news", "historical_news_end", "tick_news",
    "update_news_bulletin", "real_time_bar", "fundamental_data",
    "market_rule", "completed_order", "completed_orders_end",
    "tick_option_computation", "security_definition_option_parameter",
    "security_definition_option_parameter_end", "smart_components",
    "soft_dollar_tiers", "family_codes", "user_info", "receive_fa",
    "replace_fa_end", "display_group_list", "display_group_updated",
    "delta_neutral_validation", "wsh_meta_data", "wsh_event_data",
]


def test_the_count_is_the_published_one():
    assert len(CALLBACKS) == 81


def test_every_callback_is_there_to_override():
    missing = [name for name in CALLBACKS if not hasattr(EWrapper, name)]
    assert not missing, f"a subclass cannot override: {missing}"


def test_every_one_of_them_is_callable():
    """A name resolving to something that is not a method is not a callback."""
    unusable = [name for name in CALLBACKS if not callable(getattr(EWrapper, name, None))]
    assert not unusable, f"named and not callable: {unusable}"
