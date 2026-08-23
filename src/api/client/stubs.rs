//! Gateway-local methods that read init data from shared state.
//! Data is populated during connection by Gateway::populate_init_data.
//! Methods that are not yet supported log a warning.

use crate::api::wrapper::Wrapper;
use crate::error_codes::Refusal;

use super::EClient;


impl EClient {
    // ── Smart Components ──

    /// Request smart routing components for a BBO exchange. Matches
    /// `reqSmartComponents` in C++.
    /// Gateway-local — returns component exchanges from init data.
    ///
    /// `bbo_exchange` is taken and not applied. The venue states one table of
    /// routing components at logon, for this session rather than per exchange,
    /// and that whole table is what comes back.
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

    /// The venue's clock, as `reqCurrentTime` reports it.
    ///
    /// Every message the venue sends is stamped with the time it sent it, and
    /// the last one is held. A caller asking for the server's time is asking
    /// how far apart the two clocks are, which this machine's own clock cannot
    /// answer. Where no message has been stamped yet — before the session is
    /// up — there is nothing to report but the local clock.
    pub fn req_current_time(&self, wrapper: &mut impl Wrapper) {
        let stated = self.shared.market.venue_time()
            .as_deref()
            .and_then(crate::protocol::datetime::ib_datetime_to_unix);
        let now = stated.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64
        });
        wrapper.current_time(now);
    }

    // ── FA (Financial Advisor) ──

    /// Ask the venue for a partition of the advisor's own configuration.
    ///
    /// The reference client names the partition by a number — its aliases, its
    /// groups, its allocation profiles — and the venue names it by a word, so
    /// the number is turned into the word it stands for. A number that stands
    /// for nothing is refused rather than sent as an empty partition.
    ///
    /// The request reaches the venue; its answer is not read back yet, so
    /// [`Wrapper::receive_fa`] does not fire. What the venue replies with
    /// lands among the messages this client records as unread. Reading it
    /// needs an advisor account to state the reply's shape, and inventing one
    /// would be a guess about a frame nobody here has seen.
    pub fn request_fa(&self, fa_data_type: i32) -> Result<(), Refusal> {
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
    ///
    /// As with [`request_fa`](Self::request_fa), the replacement reaches the
    /// venue and its answer is not read back, so [`Wrapper::replace_fa_end`]
    /// does not fire.
    pub fn replace_fa(&self, fa_data_type: i32, cxml: &str) -> Result<(), Refusal> {
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

    /// What volatility a price implies, under the venue's model.
    ///
    /// This protocol carries no request for it, so the value is computed
    /// here, anchored to the venue's last stated model output for this
    /// contract. Where the venue has stated no model, nothing is answered rather
    /// than a number derived from an unstated rate.
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
                    implied_vol: volatility,
                    opt_price: option_price,
                    und_price: under_price,
                    ..crate::types::OptionComputation::solved(req_id)
                },
            ),
            // The venue states a model for a contract that is watched. Asking
            // about one nobody is watching opens the watch and answers when
            // the model arrives, which is what the caller asked for — rather
            // than refusing the question for having been asked first.
            Err(why) if self.watch_for_option_model(req_id, contract, true,
                                                    option_price, under_price) => {
                let _ = why;
            }
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
                    implied_vol: volatility,
                    opt_price: price,
                    und_price: under_price,
                    ..crate::types::OptionComputation::solved(req_id)
                },
            ),
            // As above: the watch is opened and the answer follows.
            Err(why) if self.watch_for_option_model(req_id, contract, false,
                                                    volatility, under_price) => {
                let _ = why;
            }
            Err(why) => self.report_reason(req_id, &why),
        }
    }

    /// Answer a kept implied-volatility question, if the venue has stated a
    /// model by now. Answers whether it did.
    pub(crate) fn solve_and_push_volatility(
        &self, req_id: i64, calc: &super::PendingOptionCalc,
    ) -> bool {
        let (opt, und) = (calc.option_price, calc.under_price);
        match self.solve_option(&calc.contract, |terms, model| {
            crate::control::option_model::implied_volatility(terms, model, opt, und)
        }) {
            Ok(volatility) => {
                self.shared.market.push_option_computation(crate::types::OptionComputation {
                    implied_vol: volatility,
                    opt_price: opt,
                    und_price: und,
                    ..crate::types::OptionComputation::solved(req_id)
                });
                true
            }
            Err(_) => false,
        }
    }

    /// Answer a kept option-price question, if the venue has stated a model by
    /// now. Answers whether it did.
    pub(crate) fn solve_and_push_price(
        &self, req_id: i64, calc: &super::PendingOptionCalc,
    ) -> bool {
        let (vol, und) = (calc.option_price, calc.under_price);
        match self.solve_option(&calc.contract, |terms, model| {
            crate::control::option_model::option_price(terms, model, vol, und)
        }) {
            Ok(price) => {
                self.shared.market.push_option_computation(crate::types::OptionComputation {
                    implied_vol: vol,
                    opt_price: price,
                    und_price: und,
                    ..crate::types::OptionComputation::solved(req_id)
                });
                true
            }
            Err(_) => false,
        }
    }

    /// Watch a contract so the venue states a model for it, and keep the
    /// question until it does.
    ///
    /// Answers whether the question was kept, which is only false where the
    /// watch could not be opened at all.
    fn watch_for_option_model(
        &self, req_id: i64, contract: &super::Contract,
        wants_volatility: bool, option_price: f64, under_price: f64,
    ) -> bool {
        // Knowing the contract is not watching it, and only a watched
        // contract has a model stated for it. Already watching, the question
        // is kept as it stands and the model is waited for.
        let watched = self.core.cached_instrument(contract.con_id).is_some_and(|instrument| {
            self.core.instrument_to_req.lock().unwrap().contains_key(&instrument)
        });
        if !watched && self.req_mkt_data(req_id, contract, "", false, false).is_err() {
            return false;
        }
        self.pending_option_calcs.lock().unwrap().insert(req_id, super::PendingOptionCalc {
            contract: contract.clone(),
            wants_volatility,
            option_price,
            under_price,
            answered: false,
        });
        true
    }

    /// The contract's terms and the venue's model for it, or why neither
    /// question can be answered.
    fn solve_option(
        &self,
        contract: &super::Contract,
        solve: impl Fn(
            crate::control::option_model::OptionTerms,
            crate::control::option_model::VenueModel,
        ) -> Option<f64>,
    ) -> Result<f64, Refusal> {
        self.core.solve_option(&self.shared, contract, solve)
    }

    /// Withdraw a question that was waiting on the venue to state a model.
    ///
    /// A question answered from a model already stated started nothing and
    /// stops nothing. One that opened a watch to get an answer withdraws it
    /// here, so a caller that changes its mind is not left watching a
    /// contract it no longer asks about.
    pub fn cancel_calculate_implied_volatility(&self, req_id: i64) {
        self.forget_option_calc(req_id);
    }

    /// As for [`cancel_calculate_implied_volatility`](Self::cancel_calculate_implied_volatility).
    pub fn cancel_calculate_option_price(&self, req_id: i64) {
        self.forget_option_calc(req_id);
    }

    /// Drop a kept question and the watch it opened.
    fn forget_option_calc(&self, req_id: i64) {
        if self.pending_option_calcs.lock().unwrap().remove(&req_id).is_some() {
            let _ = self.cancel_mkt_data(req_id);
        }
    }

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
    pub fn update_display_group(&self, req_id: i64, contract_info: &str) -> Result<(), Refusal> {
        self.core.update_display_group(req_id, contract_info)
            .map_err(Refusal::from)
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
    ///
    /// Taken and not applied. The session holds no log level of its own and this
    /// protocol carries no message asking the venue to change one, so what a
    /// caller states here is written to this client's log and nothing else.
    /// This client's own logging is set where the process sets it, through
    /// `IBX_LOG_LEVEL` or `RUST_LOG`.
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
    pub(crate) fn report_reason(&self, req_id: i64, reason: &Refusal) {
        // A refusal against no request keeps that fact rather than being
        // clamped onto request zero, which a caller may well have asked under.
        let carried = if req_id < 0 {
            crate::bridge::ReferenceState::NO_REQUEST
        } else {
            req_id as u32
        };
        self.shared.reference.push_historical_error(
            carried, reason.code, reason.message.clone(),
        );
    }
}


/// The word the venue names an advisor's configuration partition by, from the
/// number the reference client names it by.
fn advisor_partition(fa_data_type: i32) -> Option<&'static str> {
    // The order the venue reads them in. Rotated by one here, every
    // advisor request asked for a different partition than the caller named:
    // a request for groups returned aliases, and one for aliases returned
    // nothing the caller could use.
    match fa_data_type {
        1 => Some("Group"),
        2 => Some("Profile"),
        3 => Some("Aliases"),
        _ => None,
    }
}




#[cfg(test)]
mod advisor_partition_tests {
    use super::advisor_partition;

    /// The reference client names a partition by a number and the venue names
    /// it by a word. Both clients here send the word, and a number that
    /// stands for nothing is refused rather than sent as an empty partition.
    ///
    /// The order the venue reads: one names the group, two the
    /// profile, three the aliases. Rotated by one, every advisor request asked
    /// for a partition the caller had not named, and this test agreed with it.
    #[test]
    fn each_number_names_the_partition_the_venue_knows() {
        assert_eq!(advisor_partition(1), Some("Group"));
        assert_eq!(advisor_partition(2), Some("Profile"));
        assert_eq!(advisor_partition(3), Some("Aliases"));
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
    use crate::client_core::{days_from_civil, years_to_expiry};

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
