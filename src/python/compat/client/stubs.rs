//! Gateway-local fakes and pure no-op stubs.

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
    #[pyo3(signature = (req_id, contract, option_price, under_price, implied_vol_options=Vec::new()))]
    fn calculate_implied_volatility(
        &self, req_id: i64, contract: &Contract, option_price: f64,
        under_price: f64, implied_vol_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = (req_id, contract, option_price, under_price, implied_vol_options);
        report_reason(self, req_id, MODELLED_IN_PROCESS);
        Ok(())
    }

    /// What an option is worth at a stated volatility, under the same
    /// model. Answered on `tick_option_computation`.
    #[pyo3(signature = (req_id, contract, volatility, under_price, opt_prc_options=Vec::new()))]
    fn calculate_option_price(
        &self, req_id: i64, contract: &Contract, volatility: f64,
        under_price: f64, opt_prc_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = (req_id, contract, volatility, under_price, opt_prc_options);
        report_reason(self, req_id, MODELLED_IN_PROCESS);
        Ok(())
    }

    // nothing to withdraw: the request it would withdraw is refused, so none
    // is ever outstanding.
    /// Stop waiting on an implied-volatility request.
    fn cancel_calculate_implied_volatility(&self, req_id: i64) -> PyResult<()> {
        let _ = req_id;
        Ok(())
    }

    // nothing to withdraw: as above.
    /// Stop waiting on an option-price request.
    fn cancel_calculate_option_price(&self, req_id: i64) -> PyResult<()> {
        let _ = req_id;
        Ok(())
    }


    // ── News Bulletins ──

    /// Ask for the notices the venue broadcasts to everyone. Answered on
    /// `update_news_bulletin`.
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
    fn req_current_time(&self, py: Python<'_>) -> PyResult<()> {
        let from_venue = self
            .shared
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.market.venue_time())
            .and_then(|stamped| crate::config::ib_datetime_to_unix(&stamped));

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

    // ── FA (Financial Advisor) ──

    /// Ask the venue for a partition of the advisor's own configuration.
    ///
    /// The reference client names the partition by a number: its groups, its
    /// allocation profiles, its aliases. The venue names it by a word, so the
    /// number is turned into the word it stands for. A number that stands for
    /// nothing is refused rather than sent as an empty partition.
    fn request_fa(&self, py: Python<'_>, fa_data_type: i32) -> PyResult<()> {
        let Some(partition) = advisor_partition(fa_data_type) else {
            return self.report_refusal(py, -1, crate::api::error_codes::Refusal::validation(
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
    fn replace_fa(&self, py: Python<'_>, req_id: i64, fa_data_type: i32, cxml: &str) -> PyResult<()> {
        let _ = req_id;
        let Some(partition) = advisor_partition(fa_data_type) else {
            return self.report_refusal(py, req_id, crate::api::error_codes::Refusal::validation(
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
            report_reason(self, req_id, &reason);
        }
        Ok(())
    }

    // ── Smart Components ──

    /// Ask which venue each bit of a quote's exchange mask refers to.
    /// The venue states the map beside the quote, so a quote has to have been
    /// asked for first. Answered on `smart_components`.
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

    /// How much the venue should log about this session, 1 to 5.
    #[pyo3(signature = (log_level=2))]
    fn set_server_log_level(&self, log_level: i32) -> PyResult<()> {
        let level = match log_level {
            1 => "error",
            2 => "warn",
            3 => "info",
            4 => "debug",
            5 => "trace",
            _ => "warn",
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
        let mut query = crate::control::calendar::CalendarQuery::default();
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

/// Why solving an option for its volatility, or for its price, is not served.
///
/// Not for want of finding it on the wire: there is nothing on the wire to
/// find. The counterpart solves both in its own process, with a pricing model
/// it carries, seeded by the caller's number and the market data it already
/// holds. No request leaves the machine and no answer comes back, so serving
/// these means shipping a pricing model and the curves it needs, which is a
/// different undertaking from carrying a message.
///
/// What the venue will model, it models on its own terms, and this client
/// already asks for it: the option model arrives as its own tick.
const MODELLED_IN_PROCESS: &str = "solving an option for its volatility or its price is not a \
     request this protocol carries: the counterpart computes both in its own process from a \
     pricing model it ships. The venue's own model is available as a market-data subscription \
     instead";


/// Answer a request this client cannot serve the way the reference client
/// does: on the error callback, returning normally.
pub(crate) fn report_unserviceable(client: &EClient, req_id: i64, reason: &str) {
    report_reason(client, req_id, reason);
}

/// Answer a request this client cannot serve the way the reference client
/// does: on the error callback, returning normally.
fn report_reason(client: &EClient, req_id: i64, reason: &str) {
    if let Ok(shared) = client.shared_state() {
        shared.reference.push_historical_error(req_id.max(0) as u32, 321, reason.to_string());
    }
}

/// The word the venue names a partition of an advisor's configuration by.
///
/// The reference client names it by a number. The two vocabularies are not the
/// same, and sending the number would ask for a partition that does not exist.
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

    /// The reference client names a partition of an advisor's configuration by
    /// a number; the venue names it by a word. Sending the number would ask for
    /// a partition that does not exist.
    #[test]
    fn a_number_is_turned_into_the_word_the_venue_uses() {
        assert_eq!(advisor_partition(1), Some("Aliases"));
        assert_eq!(advisor_partition(2), Some("Group"));
        assert_eq!(advisor_partition(3), Some("Profile"));
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
