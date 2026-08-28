//! Gateway-local fakes and pure no-op stubs.

use crate::error_codes::Refusal;
use crate::python::compat::client::wire_req_id;
use crate::types::ControlCommand;
use pyo3::prelude::*;

use super::EClient;
use super::super::contract::{Contract, NewsProviderPy, SmartComponentPy, SoftDollarTierPy};

#[pymethods]
impl EClient {
    // ── What the venue permits ──

    /// Security type → the order types the venue permits for it, as stated at
    /// logon. Empty until the session is up.
    fn order_permissions(&self) -> PyResult<std::collections::HashMap<String, Vec<String>>> {
        Ok(self.shared_state().map(|s| s.reference.order_permissions()).unwrap_or_default())
    }

    /// The order types permitted for one security type, or `None` when the
    /// type is not permitted at all. A combination is named `COMB`.
    fn permitted_order_types(&self, sec_type: &str) -> PyResult<Option<Vec<String>>> {
        Ok(self.shared_state()
            .ok()
            .and_then(|s| s.reference.permitted_order_types(&sec_type.to_ascii_uppercase())))
    }

    /// Feature tokens the venue enables for this account.
    fn enabled_features(&self) -> PyResult<Vec<String>> {
        Ok(self.shared_state().map(|s| s.reference.enabled_features()).unwrap_or_default())
    }

    /// Which algorithms the venue offers, keyed `PROVIDER/SECTYPE`.
    fn algorithms(&self) -> PyResult<std::collections::HashMap<String, Vec<String>>> {
        Ok(self.shared_state().map(|s| s.reference.algorithms()).unwrap_or_default())
    }

    /// The algorithms offered for one security type, across every provider.
    fn algorithms_for(&self, sec_type: &str) -> PyResult<Vec<String>> {
        Ok(self.shared_state().map(|s| s.reference.algorithms_for(sec_type)).unwrap_or_default())
    }

    // ── Option calculations ──
    //
    // A volatility inverted from a price, and a price implied by a volatility.
    // This protocol carries no request for either: nothing it sends takes a
    // caller-supplied option price or volatility for the venue to work back
    // from. The calls are kept because a caller written against the reference
    // client calls them, and a call that reports why it cannot be served is
    // worth more than a missing attribute; they are not kept because they
    // might start working.

    /// What volatility a price implies for an option, under the model
    /// the venue publishes for that contract. Answered on
    /// `tick_option_computation`.
    ///
    /// `implied_vol_options` is taken and not applied. This protocol's request
    /// carries no free-form option list, so what a caller puts in one cannot be
    /// sent. The reference client's own list is empty on every ordinary call.
    #[pyo3(signature = (req_id, contract, option_price, under_price, implied_vol_options=Vec::new()))]
    fn calculate_implied_volatility(
        &self, py: Python<'_>, req_id: i64, contract: &Contract, option_price: f64,
        under_price: f64, implied_vol_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = implied_vol_options;
        if let Err(why) = self.answer_option_model(req_id, contract, |terms, model| {
            crate::control::option_model::implied_volatility(
                terms, model, option_price, under_price,
            )
        }, |volatility| crate::types::OptionComputation {
            implied_vol: volatility,
            opt_price: option_price,
            und_price: under_price,
            ..crate::types::OptionComputation::solved(req_id)
        }) {
            // The venue states a model for a contract that is watched. Asking
            // about one nobody is watching opens the watch and answers when
            // the model arrives, which is what the caller asked for — rather
            // than refusing the question for having been asked first.
            //
            // Only where that is the trouble. A model already stated and a
            // question it cannot answer is not something waiting will fix,
            // and kept anyway the caller was given neither an answer nor a
            // reason and waited on a model that had already arrived.
            let worth_waiting = why.message == crate::client_core::OPTION_MODEL_UNSTATED;
            if !worth_waiting || !self.watch_for_option_model(
                py, req_id, contract, true, option_price, under_price,
            ) {
                report_reason(self, req_id, &why);
            }
        }
        Ok(())
    }

    /// What an option is worth at a stated volatility, under the same
    /// model. Answered on `tick_option_computation`.
    ///
    /// `opt_prc_options` is taken and not applied. This protocol's request
    /// carries no free-form option list, so what a caller puts in one cannot be
    /// sent. The reference client's own list is empty on every ordinary call.
    #[pyo3(signature = (req_id, contract, volatility, under_price, opt_prc_options=Vec::new()))]
    fn calculate_option_price(
        &self, py: Python<'_>, req_id: i64, contract: &Contract, volatility: f64,
        under_price: f64, opt_prc_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = opt_prc_options;
        if let Err(why) = self.answer_option_model(req_id, contract, |terms, model| {
            crate::control::option_model::option_price(
                terms, model, volatility, under_price,
            )
        }, |price| crate::types::OptionComputation {
            implied_vol: volatility,
            opt_price: price,
            und_price: under_price,
            ..crate::types::OptionComputation::solved(req_id)
        }) {
            // As above: the watch is opened where the model has not been
            // stated, and the answer follows it. Where it has, and the
            // question still cannot be answered, that is said.
            let worth_waiting = why.message == crate::client_core::OPTION_MODEL_UNSTATED;
            if !worth_waiting || !self.watch_for_option_model(
                py, req_id, contract, false, volatility, under_price,
            ) {
                report_reason(self, req_id, &why);
            }
        }
        Ok(())
    }

    /// Stop waiting on an implied-volatility request.
    ///
    /// A question answered in the call it was asked in leaves nothing to
    /// withdraw. One that opened a watch is holding a subscription the caller
    /// never asked for by name, and this is what releases it.
    fn cancel_calculate_implied_volatility(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        self.forget_option_calc(py, req_id);
        Ok(())
    }

    /// As for [`cancel_calculate_implied_volatility`](Self::cancel_calculate_implied_volatility).
    fn cancel_calculate_option_price(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        self.forget_option_calc(py, req_id);
        Ok(())
    }


    // ── News Bulletins ──

    /// Ask for the notices the venue broadcasts to everyone. Answered on
    /// `update_news_bulletin`.
    ///
    /// `all_msgs` is taken and not applied, and already honoured for everything
    /// it can be. Nothing is sent to the venue: it broadcasts these unasked and
    /// this only decides whether they are delivered, so every bulletin the
    /// session has seen — including those from before this call — is handed
    /// over here. What cannot be had is anything from before the session
    /// existed, because there is no request to ask for it with. The last
    /// [`NEWS_BULLETIN_LIMIT`](crate::bridge::NEWS_BULLETIN_LIMIT)
    /// are kept for a caller who has not asked yet.
    #[pyo3(signature = (all_msgs=true))]
    fn req_news_bulletins(&self, all_msgs: bool) -> PyResult<()> {
        let _ = all_msgs;
        self.core.subscribe_bulletins();
        Ok(())
    }

    /// Stop receiving broadcast notices.
    fn cancel_news_bulletins(&self) -> PyResult<()> {
        self.core.unsubscribe_bulletins();
        Ok(())
    }

    // ── Server Time ──
    //
    // The venue's clock, not this machine's. A caller asks for it to find out
    // how far apart the two are, and answering with the local clock reports
    // zero skew whatever the truth is — the one answer that cannot be wrong
    // and cannot be useful.
    //
    // Every message the venue sends is stamped with the time it sent it, so
    // the answer is the stamp on the last one. Before any message has arrived
    // there is nothing to report but this machine's clock, and that is the
    // only case where it is used.
    /// Ask the venue for its own clock. Answered on `current_time`.
    ///
    /// Before a session exists there is no venue clock to report, so this is
    /// answered the way the reference client answers every request made before
    /// connecting: on `error`, under the number it reports that by. The local
    /// clock is not a substitute, since the caller asks this to measure the
    /// difference between the two.
    fn req_current_time(&self, py: Python<'_>) -> PyResult<()> {
        let Some(_connected) = self.tx_or_report(-1) else { return Ok(()) };
        let from_venue = self
            .shared
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.market.venue_time())
            .and_then(|stamped| crate::protocol::datetime::ib_datetime_to_unix(&stamped));

        let seconds = match from_venue {
            Some(secs) => secs,
            None => {
                log::warn!(
                    "current_time: the venue has stamped no message yet, so this \
                     reports the local clock"
                );
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            }
        };
        self.callback(py, "current_time", (seconds,))?;
        Ok(())
    }

    /// Ask the venue for its own clock in milliseconds. Answered on
    /// `current_time_in_millis`.
    ///
    /// The same clock `req_current_time` reports and read the same way. What
    /// differs is the precision kept: the venue sometimes stamps a fraction of
    /// a second, and asking in seconds throws it away. A stamp with no
    /// fraction lands on a whole second, which is the precision the venue
    /// stated rather than a rounding of something finer.
    ///
    /// Answered whether or not a session is up, which is what the request
    /// surface does and so what a caller of either client gets. Before
    /// anything has been stamped there is nothing to report but this machine's
    /// clock, and the log says so rather than leaving a caller waiting on a
    /// callback that is not coming.
    fn req_current_time_in_millis(&self, py: Python<'_>) -> PyResult<()> {
        let from_venue = self
            .shared
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.market.venue_time())
            .and_then(|stamped| crate::protocol::datetime::ib_datetime_to_unix_millis(&stamped));

        let millis = match from_venue {
            Some(ms) => ms,
            None => {
                log::warn!(
                    "current_time_in_millis: the venue has stamped no message yet, so \
                     this reports the local clock"
                );
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64
            }
        };
        self.callback(py, "current_time_in_millis", (millis,))?;
        Ok(())
    }

    // ── FA (Financial Advisor) ──

    /// Ask the venue for a partition of the advisor's own configuration.
    ///
    /// The reference client names the partition by a number: its groups, its
    /// allocation profiles, its aliases. The venue names it by a word, so the
    /// number is turned into the word it stands for. A number that stands for
    /// nothing is refused rather than sent as an empty partition.
    ///
    /// The request reaches the venue; its answer is not read back yet, so
    /// `receive_fa` does not fire. What the venue replies with lands among the
    /// messages this client records as unread. Reading it needs an advisor
    /// account to state the reply's shape, and inventing one would be a guess
    /// about a frame nobody here has seen. Said here because a caller waiting
    /// on a callback that cannot come has nothing else to tell them.
    fn request_fa(&self, py: Python<'_>, fa_data_type: i32) -> PyResult<()> {
        let Some(partition) = advisor_partition(fa_data_type) else {
            return self.report_refusal(py, -1, crate::error_codes::Refusal::validation(
                format!("no advisor configuration is named by {fa_data_type}"),
            ));
        };
        let Some(tx) = self.tx_or_report(-1) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::AdvisorConfig {
            // Asking for it by name.
            command: 5,
            partition: partition.to_string(),
            document: None,
        })
    }

    #[pyo3(signature = (req_id, fa_data_type, cxml))]
    /// Replace a partition of the advisor's configuration with the one given.
    ///
    /// As with `request_fa`, the replacement reaches the venue and its answer
    /// is not read back, so `replace_fa_end` does not fire.
    ///
    /// `req_id` is taken and not applied. The exchange carries no request
    /// number on this wire, and the reference client numbers it only to match
    /// the answer that is not read back here.
    fn replace_fa(&self, py: Python<'_>, req_id: i64, fa_data_type: i32, cxml: &str) -> PyResult<()> {
        let _ = req_id;
        let Some(partition) = advisor_partition(fa_data_type) else {
            return self.report_refusal(py, req_id, crate::error_codes::Refusal::validation(
                format!("no advisor configuration is named by {fa_data_type}"),
            ));
        };
        let Some(tx) = self.tx_or_report(-1) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::AdvisorConfig {
            // Replacing it with what is carried.
            command: 3,
            partition: partition.to_string(),
            document: Some(cxml.to_string()),
        })
    }

    // ── Display Groups ──

    /// Ask which display groups exist. Answered on
    /// `display_group_list`.
    fn query_display_groups(&self, req_id: i64) -> PyResult<()> {
        self.core.query_display_groups(req_id);
        Ok(())
    }

    /// Watch what a display group is showing. Answered on
    /// `display_group_updated`.
    fn subscribe_to_group_events(&self, req_id: i64, group_id: i32) -> PyResult<()> {
        self.core.subscribe_to_group_events(req_id, group_id);
        Ok(())
    }

    /// Stop watching a display group.
    fn unsubscribe_from_group_events(&self, req_id: i64) -> PyResult<()> {
        self.core.unsubscribe_from_group_events(req_id);
        Ok(())
    }

    /// Tell a display group what to show.
    fn update_display_group(&self, req_id: i64, contract_info: &str) -> PyResult<()> {
        // The reference client answers a request it cannot serve on the error
        // callback and returns normally. Raising here would make a caller
        // written against it fall over on a request that merely came in the
        // wrong order.
        if let Err(reason) = self.core.update_display_group(req_id, contract_info) {
            report_reason(self, req_id, &Refusal::validation(reason));
        }
        Ok(())
    }

    // ── Smart Components ──

    /// Ask which venue each bit of a quote's exchange mask refers to.
    /// The venue states the map beside the quote, so a quote has to have been
    /// asked for first. Answered on `smart_components`.
    ///
    /// `bbo_exchange` is taken and not applied. The venue states one table of
    /// routing components at logon, for this session rather than per exchange,
    /// and that whole table is what comes back.
    fn req_smart_components(&self, py: Python<'_>, req_id: i64, bbo_exchange: &str) -> PyResult<()> {
        let _ = bbo_exchange;
        let shared = self.shared_state()?;
        let sc = shared.reference.smart_components();
        let map = pyo3::types::PyDict::new(py);
        for c in sc.iter() {
            let obj = SmartComponentPy {
                bit_number: c.bit_number,
                exchange: c.exchange.clone(),
                exchange_letter: c.exchange_letter.clone(),
            };
            map.set_item(c.bit_number, Py::new(py, obj)?)?;
        }
        self.callback(py, "smart_components", (req_id, map.as_any()))?;
        Ok(())
    }

    // ── News Providers ──

    /// Ask which news providers this account may read. Answered on
    /// `news_providers`.
    fn req_news_providers(&self, py: Python<'_>) -> PyResult<()> {
        let shared = self.shared_state()?;
        let np = shared.reference.news_providers();
        let mut providers: Vec<Py<NewsProviderPy>> = Vec::with_capacity(np.len());
        for p in np.iter() {
            let obj = NewsProviderPy { code: p.code.clone(), name: p.name.clone() };
            providers.push(Py::new(py, obj)?);
        }
        let py_list = pyo3::types::PyList::new(py, providers)?;
        self.callback(py, "news_providers", (py_list.as_any(),))?;
        Ok(())
    }

    // ── Soft Dollar Tiers ──

    /// Ask which soft dollar tiers this account may direct commission
    /// to. Answered on `soft_dollar_tiers`.
    fn req_soft_dollar_tiers(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let shared = self.shared_state()?;
        let tiers = shared.reference.soft_dollar_tiers();
        let mut objs: Vec<Py<SoftDollarTierPy>> = Vec::with_capacity(tiers.len());
        for t in tiers.iter() {
            let obj = SoftDollarTierPy {
                name: t.name.clone(),
                val: t.val.clone(),
                display_name: t.display_name.clone(),
            };
            objs.push(Py::new(py, obj)?);
        }
        let py_list = pyo3::types::PyList::new(py, objs)?;
        self.callback(py, "soft_dollar_tiers", (req_id, py_list.as_any()))?;
        Ok(())
    }

    // ── Family Codes ──

    /// Ask which account families this login belongs to. Answered on
    /// `family_codes`.
    fn req_family_codes(&self, py: Python<'_>) -> PyResult<()> {
        let shared = self.shared_state()?;
        let codes = shared.reference.family_codes();
        let py_list = pyo3::types::PyList::new(py, codes.iter().map(|fc| {
            pyo3::types::PyTuple::new(py, &[
                fc.account_id.as_str().into_pyobject(py).unwrap().into_any(),
                fc.family_code_str.as_str().into_pyobject(py).unwrap().into_any(),
            ]).unwrap()
        }))?;
        self.callback(py, "family_codes", (py_list.as_any(),))?;
        Ok(())
    }

    // ── Server Log Level ──

    /// How much to log about this session, 1 to 5.
    ///
    /// Recorded locally rather than sent: this wire carries no log-level
    /// request. A level outside 1 to 5 is refused rather than reported back as
    /// `warn`, which would tell a caller they had a level that does not
    /// exist.
    #[pyo3(signature = (log_level=2))]
    fn set_server_log_level(&self, py: Python<'_>, log_level: i32) -> PyResult<()> {
        let level = match log_level {
            1 => "error",
            2 => "warn",
            3 => "info",
            4 => "debug",
            5 => "trace",
            _ => return self.report_refusal(py, -1, crate::error_codes::Refusal::validation(
                format!("set_server_log_level: {log_level} is not a log level; it is 1 to 5"),
            )),
        };
        log::info!("set_server_log_level: {level} (level {log_level})");
        Ok(())
    }

    // ── User Info ──

    /// Ask what this login is entitled to. Answered on `user_info`.
    fn req_user_info(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let shared = self.shared_state()?;
        let id = shared.reference.white_branding_id();
        self.callback(py, "user_info", (req_id, id))?;
        Ok(())
    }

    // ── WSH ──

    /// What event types the corporate-events calendar carries. Answered on
    /// `wshMetaData`.
    fn req_wsh_meta_data(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchCalendarMetaData {
            req_id: wire_req_id(req_id)?,
        })
    }

    /// Stop waiting on the event types.
    ///
    /// The query is one message and one answer, so there is nothing at the
    /// venue to withdraw: what is withdrawn is the answer, which would
    /// otherwise reach a caller who has said they are done with it. A cancel
    /// naming no waiting request says so rather than returning as though it
    /// acted.
    fn cancel_wsh_meta_data(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::CancelCalendar {
            req_id: wire_req_id(req_id)?,
        })
    }

    /// Stop waiting on the calendar's events. As above.
    fn cancel_wsh_event_data(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::CancelCalendar {
            req_id: wire_req_id(req_id)?,
        })
    }

    /// The calendar's events. Answered on `wshEventData`.
    ///
    /// `wsh_event_data` is the object the public API takes: a contract id, or
    /// a filter the caller writes, plus the window and what to fill from.
    #[pyo3(signature = (req_id, wsh_event_data=None))]
    fn req_wsh_event_data(&self, py: Python<'_>, req_id: i64, wsh_event_data: Option<Py<PyAny>>) -> PyResult<()> {
        let mut query = crate::types::CalendarQuery::default();
        if let Some(asked) = wsh_event_data.as_ref() {
            let asked = asked.bind(py);
            let text = |name: &str| -> String {
                asked
                    .getattr(name)
                    .ok()
                    .and_then(|v| v.extract::<String>().ok())
                    .unwrap_or_default()
            };
            let flag = |name: &str| -> bool {
                asked.getattr(name).ok().and_then(|v| v.extract::<bool>().ok()).unwrap_or(false)
            };
            let con_id = asked
                .getattr("conId")
                .ok()
                .and_then(|v| v.extract::<i64>().ok())
                .filter(|id| *id > 0);
            query.con_id = con_id;
            query.filter = text("filter");
            query.start_date = text("startDate");
            query.end_date = text("endDate");
            query.fill_watchlist = flag("fillWatchlist");
            query.fill_portfolio = flag("fillPortfolio");
            query.fill_competitors = flag("fillCompetitors");
            query.total_limit = asked
                .getattr("totalLimit")
                .ok()
                .and_then(|v| v.extract::<i64>().ok())
                .filter(|n| *n > 0 && *n < i64::MAX);
        }
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchCalendarEvents {
            req_id: wire_req_id(req_id)?,
            query: Box::new(query),
        })
    }
}

/// Solve an option against the venue's published model.
///
/// The wire carries no request for either calculation, so both are solved
/// locally. The answer is reported on `tick_option_computation`, the same
/// callback tick type 13 arrives on.
impl EClient {
    /// Whether something is already watching the contract, which is what makes
    /// the venue state a model for it.
    fn watching_contract(&self, con_id: i64) -> bool {
        self.core.cached_instrument(con_id).is_some_and(|instrument| {
            self.core.instrument_to_req.lock().unwrap().contains_key(&instrument)
        })
    }

    /// Open a watch on the contract and keep the question until the venue
    /// states a model for it. Answers whether the question is now kept.
    fn watch_for_option_model(
        &self, py: Python<'_>, req_id: i64, contract: &Contract,
        wants_volatility: bool, option_price: f64, under_price: f64,
    ) -> bool {
        let api = contract.to_api();
        // A subscription this client opens rather than the caller. Refusals
        // are reported by the subscribe itself and leave nothing watching,
        // which is what is read back here rather than the call's own result:
        // this surface answers a refusal on the error callback and returns
        // normally, so the result alone does not say whether it took.
        if !self.watching_contract(api.con_id) {
            let opened = self.req_mkt_data(
                py, req_id, contract, "", false, false, Vec::new(),
            );
            if opened.is_err() || !self.watching_contract(api.con_id) {
                return false;
            }
        }
        self.pending_option_calcs.lock().unwrap().insert(
            req_id,
            crate::api::client::PendingOptionCalc {
                contract: api,
                wants_volatility,
                option_price,
                under_price,
                answered: false,
            },
        );
        true
    }

    /// Drop a kept question and the watch it opened.
    fn forget_option_calc(&self, py: Python<'_>, req_id: i64) {
        if self.pending_option_calcs.lock().unwrap().remove(&req_id).is_some() {
            let _ = self.cancel_mkt_data(py, req_id);
        }
    }

    /// Answer the questions that were waiting on the venue to state a model,
    /// and forget them.
    ///
    /// One the venue still cannot answer is kept: the watch is open, so the
    /// model may yet arrive. It is dropped when the caller withdraws it.
    pub(crate) fn answer_kept_option_calcs(&self) {
        let kept: Vec<(i64, crate::api::client::PendingOptionCalc)> = self
            .pending_option_calcs.lock().unwrap()
            .iter().map(|(k, v)| (*k, v.clone())).collect();
        for (req_id, calc) in kept {
            if calc.answered {
                continue;
            }
            if self.solve_and_push_kept(req_id, &calc) {
                // Marked, not dropped, so the caller's withdrawal still has a
                // question to find and can still take down the watch this
                // client opened to obtain the model. Nothing is sent here.
                if let Some(kept) = self.pending_option_calcs.lock().unwrap().get_mut(&req_id) {
                    kept.answered = true;
                }
            }
        }
    }

    /// Answer one kept question, if the venue has stated a model by now.
    /// Answers whether it did.
    fn solve_and_push_kept(
        &self, req_id: i64, calc: &crate::api::client::PendingOptionCalc,
    ) -> bool {
        let Ok(shared) = self.shared_state() else { return false };
        let (given, und) = (calc.option_price, calc.under_price);
        let wants_volatility = calc.wants_volatility;
        let solved = self.core.solve_option(&shared, &calc.contract, |terms, model| {
            if wants_volatility {
                crate::control::option_model::implied_volatility(terms, model, given, und)
            } else {
                crate::control::option_model::option_price(terms, model, given, und)
            }
        });
        match solved {
            Ok(answer) => {
                // The caller supplied one of the pair and asked for the other,
                // so the answer takes the side they left open.
                let (implied_vol, opt_price) =
                    if wants_volatility { (answer, given) } else { (given, answer) };
                shared.market.push_option_computation(crate::types::OptionComputation {
                    implied_vol,
                    opt_price,
                    und_price: und,
                    ..crate::types::OptionComputation::solved(req_id)
                });
                true
            }
            // As on the other surface: only the refusal saying the venue has
            // not stated its model resolves by waiting. The rest never do, and
            // read as "not yet" they keep the question for the life of the
            // session with nothing ever said about it.
            Err(why) if why.message == crate::client_core::OPTION_MODEL_UNSTATED => false,
            Err(why) => {
                report_reason(self, req_id, &why);
                true
            }
        }
    }

    fn answer_option_model(
        &self,
        req_id: i64,
        contract: &Contract,
        solve: impl Fn(
            crate::control::option_model::OptionTerms,
            crate::control::option_model::VenueModel,
        ) -> Option<f64>,
        into_computation: impl Fn(f64) -> crate::types::OptionComputation,
    ) -> Result<(), Refusal> {
        let _ = req_id;
        // The refusal is carried whole. Flattened to its text the code went
        // with it, and every one of them reached a caller as the same number —
        // which is the one thing a caller written against the reference client
        // branches on.
        let shared = self.shared_state()
            .map_err(|_| Refusal::not_connected("not connected"))?;
        let answer = self.core.solve_option(&shared, &contract.to_api(), solve)?;
        shared.market.push_option_computation(into_computation(answer));
        Ok(())
    }
}

/// Answer a request this client cannot serve the way the reference client
/// does: on the error callback, returning normally.
///
/// Takes the code for the specific refusal rather than the general one.
pub(crate) fn report_unserviceable_with(
    client: &EClient, req_id: i64, code: i32, reason: &str,
) {
    if let Ok(shared) = client.shared_state() {
        shared.reference.push_historical_error(carried_under(req_id), code, reason.to_string());
    }
}

/// Answer a request this client cannot serve the way the reference client
/// does: on the error callback, returning normally.
///
/// The refusal's own code is carried rather than one number for all of them,
/// and a refusal belonging to no request keeps that rather than being clamped
/// onto request zero, which a caller may well have asked under.
fn report_reason(client: &EClient, req_id: i64, reason: &Refusal) {
    if let Ok(shared) = client.shared_state() {
        shared.reference.push_historical_error(
            carried_under(req_id), reason.code, reason.message.clone(),
        );
    }
}

/// The request a refusal is reported against, or the mark for none.
fn carried_under(req_id: i64) -> u32 {
    if req_id < 0 {
        crate::bridge::ReferenceState::NO_REQUEST
    } else {
        req_id as u32
    }
}

/// The word the venue names a partition of an advisor's configuration by.
///
/// The reference client names it by a number. The two vocabularies are not the
/// same, and sending the number would ask for a partition that does not exist.
///
/// The numbers are the reference client's, and they run groups, profiles,
/// aliases — which is what this surface's own reference states and what the
/// Rust surface sends. Rotated by one here, a caller that asked for its groups
/// was given its aliases.
fn advisor_partition(fa_data_type: i32) -> Option<&'static str> {
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

    /// The reference client names a partition of an advisor's configuration by
    /// a number; the venue names it by a word. Sending the number would ask for
    /// a partition that does not exist.
    #[test]
    fn a_number_is_turned_into_the_word_the_venue_uses() {
        // The order is the reference client's: groups, profiles, aliases.
        assert_eq!(advisor_partition(1), Some("Group"));
        assert_eq!(advisor_partition(2), Some("Profile"));
        assert_eq!(advisor_partition(3), Some("Aliases"));
    }

    /// A number standing for nothing is refused rather than sent as an empty
    /// partition, which the venue would answer for something else or not at all.
    #[test]
    fn a_number_standing_for_nothing_names_nothing() {
        for unknown in [0, 4, -1, 99] {
            assert_eq!(advisor_partition(unknown), None, "{unknown}");
        }
    }
}
