//! Gateway-local methods that read init data from shared state.
//! Data is populated during connection by Gateway::populate_init_data().
//! Methods that are not yet supported log a warning.

use crate::api::wrapper::Wrapper;

use super::EClient;

/// Why neither option calculation can be served here.
const OPTION_CALCULATION_UNSERVED: &str =
    "this protocol carries no request that takes a caller-supplied option price or \
     volatility for the venue to work back from, so neither an implied volatility nor \
     an option price can be asked of it";

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
    /// Ask the venue for a partition of the advisor's own configuration.
    ///
    /// The reference client names the partition by a number — its aliases, its
    /// groups, its allocation profiles — and the venue names it by a word, so
    /// the number is turned into the word it stands for. A number that stands
    /// for nothing is refused rather than sent as an empty partition.
    pub fn request_fa(&self, fa_data_type: i32) -> Result<(), String> {
        let partition = advisor_partition(fa_data_type)
            .ok_or_else(|| format!("no advisor configuration is named by {fa_data_type}"))?;
        self.send(crate::types::ControlCommand::AdvisorConfig {
            // Asking for it by name.
            command: 5,
            partition: partition.to_string(),
            document: None,
        })
    }

    /// Replace a partition of the advisor's configuration with the one given.
    pub fn replace_fa(&self, fa_data_type: i32, cxml: &str) -> Result<(), String> {
        let partition = advisor_partition(fa_data_type)
            .ok_or_else(|| format!("no advisor configuration is named by {fa_data_type}"))?;
        self.send(crate::types::ControlCommand::AdvisorConfig {
            // Replacing it with what is carried.
            command: 3,
            partition: partition.to_string(),
            document: Some(cxml.to_string()),
        })
    }

    // ── Option calculations ──
    //
    // A volatility inverted from a price, and a price implied by a volatility.
    // This protocol carries no request for either: nothing it sends takes a
    // caller-supplied option price or volatility for the venue to work back
    // from. They exist so a caller written against the reference client finds
    // the call and is told why it cannot be served, rather than finding
    // nothing at all.

    /// Not served. Reports why on the error callback.
    pub fn calculate_implied_volatility(
        &self, req_id: i64, _contract: &super::Contract,
        _option_price: f64, _under_price: f64,
    ) {
        self.report_reason(req_id, OPTION_CALCULATION_UNSERVED);
    }

    /// Not served. Reports why on the error callback.
    pub fn calculate_option_price(
        &self, req_id: i64, _contract: &super::Contract,
        _volatility: f64, _under_price: f64,
    ) {
        self.report_reason(req_id, OPTION_CALCULATION_UNSERVED);
    }

    /// Nothing was started, so there is nothing to stop.
    pub fn cancel_calculate_implied_volatility(&self, _req_id: i64) {}

    /// Nothing was started, so there is nothing to stop.
    pub fn cancel_calculate_option_price(&self, _req_id: i64) {}

    // ── Display Groups ──

    /// Query display groups. Not yet implemented.
    /// The display groups on offer. Answered on `display_group_list`.
    ///
    /// A display group is a way for several callers on one session to agree on
    /// a contract. The venue knows nothing about them, and never did: the
    /// vendor's own client keeps them in its own state and serves them to its
    /// callers from there, which is exactly what this does.
    pub fn query_display_groups(&self, req_id: i64) {
        self.core.query_display_groups(req_id);
    }

    /// Follow a display group. Answered on `display_group_updated`, at once
    /// with what the group holds and again whenever it changes.
    pub fn subscribe_to_group_events(&self, req_id: i64, group_id: i32) {
        self.core.subscribe_to_group_events(req_id, group_id);
    }

    /// Stop following a display group.
    pub fn unsubscribe_from_group_events(&self, req_id: i64) {
        self.core.unsubscribe_from_group_events(req_id);
    }

    /// Put a contract in the group this request follows, stated as
    /// `conId@exchange`, or `none` to empty it. Every follower of that group is
    /// told, including this one.
    pub fn update_display_group(&self, req_id: i64, contract_info: &str) -> Result<(), String> {
        self.core.update_display_group(req_id, contract_info)
    }

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

    /// A request this client cannot serve is answered, not ignored. A caller
    /// waiting on a callback that will never come cannot tell that apart from
    /// a slow gateway, so it is told on the channel a venue uses to say it
    /// will not act on a request.
    fn report_reason(&self, req_id: i64, reason: &str) {
        self.shared.reference.push_historical_error(req_id.max(0) as u32, 321, reason.to_string());
    }
}

/// The word the venue names an advisor's configuration partition by, from the
/// number the reference client names it by.
fn advisor_partition(fa_data_type: i32) -> Option<&'static str> {
    match fa_data_type {
        1 => Some("Aliases"),
        2 => Some("Group"),
        3 => Some("Profile"),
        _ => None,
    }
}

#[cfg(test)]
mod advisor_partition_tests {
    use super::advisor_partition;

    /// The reference client names a partition by a number and the venue names
    /// it by a word. Both clients here send the word, and a number that
    /// stands for nothing is refused rather than sent as an empty partition.
    #[test]
    fn each_number_names_the_partition_the_venue_knows() {
        assert_eq!(advisor_partition(1), Some("Aliases"));
        assert_eq!(advisor_partition(2), Some("Group"));
        assert_eq!(advisor_partition(3), Some("Profile"));
    }

    #[test]
    fn a_number_that_names_nothing_is_refused() {
        for unknown in [0, 4, -1, i32::MAX] {
            assert_eq!(advisor_partition(unknown), None, "{unknown} was taken");
        }
    }
}
