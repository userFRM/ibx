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
    /// What volatility a price implies, under the venue's own model.
    ///
    /// This protocol carries no request for it — the counterpart works it out
    /// in its own process — so it is worked out here, anchored to what the
    /// venue last said its own model made of this contract. Where it has said
    /// nothing, nothing is answered: a number from a rate nobody stated would
    /// be this library's invention.
    pub fn calculate_implied_volatility(
        &self, req_id: i64, contract: &super::Contract,
        option_price: f64, under_price: f64,
    ) {
        match self.solve_option(contract, |terms, model| {
            crate::control::option_model::implied_volatility(
                terms, model, option_price, under_price,
            )
        }) {
            Ok(volatility) => self.shared.market.push_option_computation(
                crate::types::OptionComputation {
                    instrument: req_id.max(0) as u32,
                    implied_vol: volatility,
                    opt_price: option_price,
                    und_price: under_price,
                    ..Default::default()
                },
            ),
            Err(why) => self.report_reason(req_id, &why),
        }
    }

    /// What price a volatility implies, under that same model.
    pub fn calculate_option_price(
        &self, req_id: i64, contract: &super::Contract,
        volatility: f64, under_price: f64,
    ) {
        match self.solve_option(contract, |terms, model| {
            crate::control::option_model::option_price(terms, model, volatility, under_price)
        }) {
            Ok(price) => self.shared.market.push_option_computation(
                crate::types::OptionComputation {
                    instrument: req_id.max(0) as u32,
                    implied_vol: volatility,
                    opt_price: price,
                    und_price: under_price,
                    ..Default::default()
                },
            ),
            Err(why) => self.report_reason(req_id, &why),
        }
    }

    /// The contract's terms and the venue's own model for it, or why neither
    /// question can be answered.
    fn solve_option(
        &self,
        contract: &super::Contract,
        solve: impl Fn(
            crate::control::option_model::OptionTerms,
            crate::control::option_model::VenueModel,
        ) -> Option<f64>,
    ) -> Result<f64, String> {
        let instrument = self
            .instrument_of(contract.con_id)
            .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?;
        let stated = self
            .shared
            .market
            .option_model(instrument)
            .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?;
        let years = years_to_expiry(&contract.last_trade_date_or_contract_month)
            .ok_or_else(|| "the contract states no expiry to measure from".to_string())?;
        let terms = crate::control::option_model::OptionTerms {
            strike: contract.strike,
            years_to_expiry: years,
            is_call: contract.right.eq_ignore_ascii_case("C")
                || contract.right.eq_ignore_ascii_case("CALL"),
        };
        // What the venue did not state is not a number. It writes the largest
        // double where it has nothing to say, which this client passes on
        // as-is because the reference client does — so it has to be read back
        // as silence here rather than taken for a value. Taken for one, a
        // contract with no dividend had the largest double in the world
        // subtracted from its underlying.
        let stated_or_none = |v: f64| (v.is_finite() && v != f64::MAX).then_some(v);
        let model = crate::control::option_model::VenueModel {
            volatility: stated_or_none(stated.implied_vol)
                .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?,
            option_price: stated_or_none(stated.opt_price)
                .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?,
            underlying_price: stated_or_none(stated.und_price)
                .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?,
            // No dividend stated is no dividend, which is what it means.
            present_value_of_dividends: stated_or_none(stated.pv_dividend).unwrap_or(0.0),
        };
        solve(terms, model).ok_or_else(|| {
            "no volatility fits this price under the venue's own model for this contract. An \
             option far enough into the money is worth its intrinsic value and little else, and \
             its price then hardly moves with volatility at all — so there is no one volatility \
             the price implies, and naming one would be picking a number rather than solving \
             for it"
                .to_string()
        })
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

/// Why neither question can be answered without the venue having spoken.
const OPTION_MODEL_UNSTATED: &str =
    "the venue has not stated its own model for this contract on this session. Ask for the \
     option's model first — a market-data subscription on the option carries it — and both \
     questions can then be answered against what it said";

/// Years between now and a stated expiry, as `yyyymmdd`.
fn years_to_expiry(expiry: &str) -> Option<f64> {
    let digits: String = expiry.chars().filter(|c| c.is_ascii_digit()).take(8).collect();
    if digits.len() != 8 {
        return None;
    }
    let year: i64 = digits[0..4].parse().ok()?;
    let month: i64 = digits[4..6].parse().ok()?;
    let day: i64 = digits[6..8].parse().ok()?;
    let expiry_day = days_from_civil(year, month, day);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let today = now / 86_400;
    let days = expiry_day - today;
    (days > 0).then(|| days as f64 / 365.0)
}

/// Days since the epoch for a civil date. Written out rather than pulled in:
/// one date, once, and a dependency for it would be a dependency for good.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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

#[cfg(test)]
mod expiry_tests {
    use super::{days_from_civil, years_to_expiry};

    /// A known date, against a known day count. Written out rather than pulled
    /// in, so it is checked rather than trusted.
    #[test]
    fn a_civil_date_counts_the_days_since_the_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2026, 1, 1), 20454);
    }

    /// An expiry already past is no expiry to measure to.
    #[test]
    fn an_expiry_in_the_past_measures_nothing() {
        assert!(years_to_expiry("19990101").is_none());
        assert!(years_to_expiry("").is_none());
        assert!(years_to_expiry("2026").is_none());
    }

    /// One ahead measures the years between, and a longer one measures more.
    #[test]
    fn an_expiry_ahead_measures_the_years_between() {
        let near = years_to_expiry("20301231").expect("a date ahead");
        let far = years_to_expiry("20351231").expect("a date further ahead");
        assert!(near > 0.0 && far > near, "{near} then {far}");
    }
}
