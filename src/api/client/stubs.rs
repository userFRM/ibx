//! Gateway-local methods that read init data from shared state.
//! Data is populated during connection by Gateway::populate_init_data().
//! Methods that are not yet supported log a warning.

use crate::api::wrapper::Wrapper;

use super::EClient;

impl EClient {
    // ── Smart Components ──

    /// Request smart routing components for a BBO exchange. Matches `reqSmartComponents` in C++.
    /// Gateway-local — returns component exchanges from init data.
    pub fn req_smart_components(&self, req_id: i64, _bbo_exchange: &str, wrapper: &mut impl Wrapper) {
        let components = self.shared.reference.smart_components();
        wrapper.smart_components(req_id, &components);
    }

    // ── News Providers ──

    /// Request available news providers. Matches `reqNewsProviders` in C++.
    /// Gateway-local — returns provider list from init data.
    pub fn req_news_providers(&self, wrapper: &mut impl Wrapper) {
        let providers = self.shared.reference.news_providers();
        wrapper.news_providers(&providers);
    }

    // ── Server Time ──

    /// Request current server time. Matches `reqCurrentTime` in C++.
    /// Returns local system time (no server round-trip).
    pub fn req_current_time(&self, wrapper: &mut impl Wrapper) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        wrapper.current_time(now);
    }

    // ── FA (Financial Advisor) ──

    /// Request FA data. Not yet implemented.
    pub fn request_fa(&self, _fa_data_type: i32) {
        self.report_unserviceable(-1, "request_fa");
    }

    /// Replace FA data. Not yet implemented.
    pub fn replace_fa(&self, req_id: i64, _fa_data_type: i32, _cxml: &str) {
        self.report_unserviceable(req_id, "replace_fa");
    }

    // ── Display Groups ──

    /// Query display groups. Not yet implemented.
    pub fn query_display_groups(&self, _req_id: i64) {}

    /// Subscribe to display group events. Not yet implemented.
    pub fn subscribe_to_group_events(&self, _req_id: i64, _group_id: i32) {}

    /// Unsubscribe from display group events. Not yet implemented.
    pub fn unsubscribe_from_group_events(&self, _req_id: i64) {}

    /// Update display group. Not yet implemented.
    pub fn update_display_group(&self, _req_id: i64, _contract_info: &str) {}

    // ── Soft Dollar Tiers ──

    /// Request soft dollar tiers. Matches `reqSoftDollarTiers` in C++.
    /// Gateway-local — returns tiers parsed from CCP logon tag 6560.
    pub fn req_soft_dollar_tiers(&self, req_id: i64, wrapper: &mut impl Wrapper) {
        let tiers = self.shared.reference.soft_dollar_tiers();
        wrapper.soft_dollar_tiers(req_id, &tiers);
    }

    // ── Family Codes ──

    /// Request family codes. Matches `reqFamilyCodes` in C++.
    /// Gateway-local — returns codes parsed from CCP logon tag 6823.
    pub fn req_family_codes(&self, wrapper: &mut impl Wrapper) {
        let codes = self.shared.reference.family_codes();
        wrapper.family_codes(&codes);
    }

    // ── Server Log Level ──

    /// Set server log level. Matches `setServerLogLevel` in C++.
    pub fn set_server_log_level(&self, log_level: i32) {
        let level = match log_level {
            1 => "error",
            2 => "warn",
            3 => "info",
            4 => "debug",
            5 => "trace",
            _ => "warn",
        };
        log::info!("set_server_log_level: {level} (level {log_level})");
    }

    // ── User Info ──

    /// Request user info. Matches `reqUserInfo` in C++.
    /// Gateway-local — returns whiteBrandingId from CCP logon.
    pub fn req_user_info(&self, req_id: i64, wrapper: &mut impl Wrapper) {
        let id = self.shared.reference.white_branding_id();
        wrapper.user_info(req_id, &id);
    }

    // ── WSH ──

    /// Request WSH metadata. Not yet implemented.
    pub fn req_wsh_meta_data(&self, req_id: i64) {
        self.report_unserviceable(req_id, "req_wsh_meta_data");
    }

    /// Request WSH event data. Not yet implemented.
    pub fn req_wsh_event_data(&self, req_id: i64) {
        self.report_unserviceable(req_id, "req_wsh_event_data");
    }

    /// A request this client cannot serve is answered, not ignored. A caller
    /// waiting on a callback that will never come cannot tell that apart from
    /// a slow gateway, so it is told on the channel a venue uses to say it
    /// will not act on a request.
    fn report_unserviceable(&self, req_id: i64, call: &str) {
        self.shared.reference.push_historical_error(
            req_id.max(0) as u32,
            321,
            format!("{call} is not served by this client"),
        );
    }
}
