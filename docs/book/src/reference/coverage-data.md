# API Coverage Matrix (v0.7.1)

*Auto-generated from source — do not edit.*

Canonical IB API methods vs ibx implementation status.

- **Y** = Implemented
- **STUB** = Accepts call but not wired to server (logs warning or no-op)
- **-** = Not present

The evidence column says how each status was established, and is
derived rather than asserted: a call is credited to the live session
only when the suite that runs against a real account names it, and to
the offline suites only when a test names it.

## Summary

| | IB API | Rust | Python |
|---|:---:|:---:|:---:|
| **EClient methods** | 77 | 65 impl, 8 stub | 65 impl, 12 stub |
| **EWrapper callbacks** | 81 | 68 impl, 1 stub | 72 impl, 9 stub |

## EClient Methods

| Category | IB API Method | C++ Name | Rust | Python | Evidence |
|----------|---------------|----------|:----:|:------:|----------|
| Connection | `connect` | `eConnect` | Y | Y | Live session |
|  | `disconnect` | `eDisconnect` | Y | Y | Live session |
|  | `is_connected` | `isConnected` | Y | Y | Live session |
|  | `set_server_log_level` | `setServerLogLevel` | Y | Y | Not exercised |
|  | `req_current_time` | `reqCurrentTime` | Y | Y | Live session |
| Market Data | `req_mkt_data` | `reqMktData` | Y | Y | Live session |
|  | `cancel_mkt_data` | `cancelMktData` | Y | Y | Live session |
|  | `req_market_data_type` | `reqMarketDataType` | Y | Y | Not exercised |
|  | `req_tick_by_tick_data` | `reqTickByTickData` | Y | Y | Offline suites |
|  | `cancel_tick_by_tick_data` | `cancelTickByTickData` | Y | Y | Offline suites |
|  | `req_mkt_depth` | `reqMktDepth` | Y | Y | Offline suites |
|  | `cancel_mkt_depth` | `cancelMktDepth` | Y | Y | Offline suites |
|  | `req_mkt_depth_exchanges` | `reqMktDepthExchanges` | Y | Y | Not exercised |
|  | `req_smart_components` | `reqSmartComponents` | Y | Y | Live session |
|  | `req_real_time_bars` | `reqRealTimeBars` | Y | Y | Offline suites |
|  | `cancel_real_time_bars` | `cancelRealTimeBars` | Y | Y | Offline suites |
| Historical Data | `req_historical_data` | `reqHistoricalData` | Y | Y | Live session |
|  | `cancel_historical_data` | `cancelHistoricalData` | Y | Y | Live session |
|  | `req_head_time_stamp` | `reqHeadTimeStamp` | Y | Y | Live session |
|  | `cancel_head_time_stamp` | `cancelHeadTimestamp` | Y | Y | Offline suites |
|  | `req_historical_ticks` | `reqHistoricalTicks` | Y | Y | Live session |
|  | `req_histogram_data` | `reqHistogramData` | Y | Y | Live session |
|  | `cancel_histogram_data` | `cancelHistogramData` | Y | Y | Live session |
|  | `req_historical_schedule` | `reqHistoricalSchedule` | Y | Y | Live session |
| Orders | `place_order` | `placeOrder` | Y | Y | Live session |
|  | `cancel_order` | `cancelOrder` | Y | Y | Live session |
|  | `req_open_orders` | `reqOpenOrders` | Y | Y | Offline suites |
|  | `req_all_open_orders` | `reqAllOpenOrders` | Y | Y | Live session |
|  | `req_auto_open_orders` | `reqAutoOpenOrders` | Y | Y | Not exercised |
|  | `req_ids` | `reqIds` | Y | Y | Live session |
|  | `req_global_cancel` | `reqGlobalCancel` | Y | Y | Live session |
|  | `req_completed_orders` | `reqCompletedOrders` | Y | Y | Live session |
| Executions | `req_executions` | `reqExecutions` | Y | Y | Offline suites |
| Account | `req_account_updates` | `reqAccountUpdates` | Y | Y | Live session |
|  | `req_account_summary` | `reqAccountSummary` | Y | Y | Live session |
|  | `cancel_account_summary` | `cancelAccountSummary` | Y | Y | Live session |
|  | `req_positions` | `reqPositions` | Y | Y | Live session |
|  | `cancel_positions` | `cancelPositions` | Y | Y | Not exercised |
|  | `req_pnl` | `reqPnL` | Y | Y | Live session |
|  | `cancel_pnl` | `cancelPnL` | Y | Y | Live session |
|  | `req_pnl_single` | `reqPnLSingle` | Y | Y | Live session |
|  | `cancel_pnl_single` | `cancelPnLSingle` | Y | Y | Live session |
|  | `req_managed_accts` | `reqManagedAccts` | Y | Y | Not exercised |
|  | `req_account_updates_multi` | `reqAccountUpdatesMulti` | Y | Y | Not exercised |
|  | `cancel_account_updates_multi` | `cancelAccountUpdatesMulti` | Y | Y | Not exercised |
|  | `req_positions_multi` | `reqPositionsMulti` | Y | Y | Not exercised |
|  | `cancel_positions_multi` | `cancelPositionsMulti` | Y | Y | Not exercised |
| Contract | `req_contract_details` | `reqContractDetails` | Y | Y | Live session |
|  | `req_matching_symbols` | `reqMatchingSymbols` | Y | Y | Live session |
|  | `req_market_rule` | `reqMarketRule` | Y | Y | Not exercised |
| Scanner | `req_scanner_parameters` | `reqScannerParameters` | Y | Y | Live session |
|  | `req_scanner_subscription` | `reqScannerSubscription` | Y | Y | Live session |
|  | `cancel_scanner_subscription` | `cancelScannerSubscription` | Y | Y | Live session |
| News | `req_news_providers` | `reqNewsProviders` | Y | Y | Live session |
|  | `req_news_article` | `reqNewsArticle` | Y | Y | Live session |
|  | `req_historical_news` | `reqHistoricalNews` | Y | Y | Live session |
|  | `req_news_bulletins` | `reqNewsBulletins` | Y | Y | Live session |
|  | `cancel_news_bulletins` | `cancelNewsBulletins` | Y | Y | Live session |
| Fundamental | `req_fundamental_data` | `reqFundamentalData` | Y | Y | Live session |
|  | `cancel_fundamental_data` | `cancelFundamentalData` | Y | Y | Live session |
| Options | `calculate_implied_volatility` | `calculateImpliedVolatility` | - | STUB | States why it cannot be served |
|  | `cancel_calculate_implied_volatility` | `cancelCalculateImpliedVolatility` | - | STUB | States why it cannot be served |
|  | `calculate_option_price` | `calculateOptionPrice` | - | STUB | States why it cannot be served |
|  | `cancel_calculate_option_price` | `cancelCalculateOptionPrice` | - | STUB | States why it cannot be served |
|  | `exercise_options` | `exerciseOptions` | Y | Y | Offline suites |
|  | `req_sec_def_opt_params` | `reqSecDefOptParams` | Y | Y | Offline suites |
| Reference | `req_soft_dollar_tiers` | `reqSoftDollarTiers` | Y | Y | Live session |
|  | `req_family_codes` | `reqFamilyCodes` | Y | Y | Live session |
|  | `req_user_info` | `reqUserInfo` | Y | Y | Live session |
| Financial Advisor | `request_fa` | `requestFA` | STUB | STUB | States why it cannot be served |
|  | `replace_fa` | `replaceFA` | STUB | STUB | States why it cannot be served |
| Display Groups | `query_display_groups` | `queryDisplayGroups` | STUB | STUB | States why it cannot be served |
|  | `subscribe_to_group_events` | `subscribeToGroupEvents` | STUB | STUB | States why it cannot be served |
|  | `unsubscribe_from_group_events` | `unsubscribeFromGroupEvents` | STUB | STUB | States why it cannot be served |
|  | `update_display_group` | `updateDisplayGroup` | STUB | STUB | States why it cannot be served |
| WSH | `req_wsh_meta_data` | `reqWshMetaData` | STUB | STUB | States why it cannot be served |
|  | `req_wsh_event_data` | `reqWshEventData` | STUB | STUB | States why it cannot be served |

## EWrapper Callbacks

| Category | Callback | Rust | Python |
|----------|----------|:----:|:------:|
| Connection | `connect_ack` | Y | Y |
|  | `connection_closed` | Y | Y |
|  | `next_valid_id` | Y | Y |
|  | `managed_accounts` | Y | Y |
|  | `error` | Y | Y |
|  | `current_time` | Y | Y |
| Market Data | `tick_price` | Y | Y |
|  | `tick_size` | Y | Y |
|  | `tick_string` | Y | Y |
|  | `tick_generic` | Y | Y |
|  | `tick_snapshot_end` | Y | Y |
|  | `market_data_type` | Y | Y |
|  | `tick_req_params` | Y | Y |
| Orders | `order_status` | Y | Y |
|  | `open_order` | Y | Y |
|  | `open_order_end` | Y | Y |
|  | `order_bound` | - | STUB |
| Executions | `exec_details` | Y | Y |
|  | `exec_details_end` | Y | Y |
|  | `commission_and_fees_report` | Y | Y |
| Account | `update_account_value` | Y | Y |
|  | `update_portfolio` | Y | Y |
|  | `update_account_time` | Y | Y |
|  | `account_download_end` | Y | Y |
|  | `account_summary` | Y | Y |
|  | `account_summary_end` | Y | Y |
|  | `position` | Y | Y |
|  | `position_end` | Y | Y |
|  | `pnl` | Y | Y |
|  | `pnl_single` | Y | Y |
|  | `position_multi` | - | Y |
|  | `position_multi_end` | - | Y |
|  | `account_update_multi` | - | Y |
|  | `account_update_multi_end` | - | Y |
| Contract | `contract_details` | Y | Y |
|  | `contract_details_end` | Y | Y |
|  | `bond_contract_details` | - | STUB |
|  | `symbol_samples` | Y | Y |
| Historical Data | `historical_data` | Y | Y |
|  | `historical_data_end` | Y | Y |
|  | `historical_data_update` | Y | Y |
|  | `head_timestamp` | Y | Y |
|  | `historical_ticks` | Y | Y |
|  | `historical_ticks_bid_ask` | Y | Y |
|  | `historical_ticks_last` | Y | Y |
|  | `histogram_data` | Y | Y |
|  | `historical_schedule` | Y | Y |
| Market Depth | `update_mkt_depth` | Y | Y |
|  | `update_mkt_depth_l2` | Y | Y |
|  | `mkt_depth_exchanges` | Y | Y |
| Tick-by-Tick | `tick_by_tick_all_last` | Y | Y |
|  | `tick_by_tick_bid_ask` | Y | Y |
|  | `tick_by_tick_mid_point` | Y | Y |
| Scanner | `scanner_data` | Y | Y |
|  | `scanner_data_end` | Y | Y |
|  | `scanner_parameters` | Y | Y |
| News | `news_providers` | Y | Y |
|  | `news_article` | Y | Y |
|  | `historical_news` | Y | Y |
|  | `historical_news_end` | Y | Y |
|  | `tick_news` | Y | Y |
|  | `update_news_bulletin` | Y | Y |
| Real-Time Bars | `real_time_bar` | Y | Y |
| Fundamental | `fundamental_data` | Y | Y |
| Market Rules | `market_rule` | Y | Y |
| Completed Orders | `completed_order` | Y | Y |
|  | `completed_orders_end` | Y | Y |
| Options | `tick_option_computation` | Y | Y |
|  | `security_definition_option_parameter` | Y | Y |
|  | `security_definition_option_parameter_end` | Y | Y |
| Reference | `smart_components` | Y | Y |
|  | `soft_dollar_tiers` | Y | Y |
|  | `family_codes` | Y | Y |
|  | `user_info` | Y | Y |
| FA | `receive_fa` | - | STUB |
|  | `replace_fa_end` | - | STUB |
| Display Groups | `display_group_list` | - | STUB |
|  | `display_group_updated` | - | STUB |
| Other | `delta_neutral_validation` | STUB | STUB |
| WSH | `wsh_meta_data` | - | STUB |
|  | `wsh_event_data` | - | STUB |
