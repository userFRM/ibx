//! Reference data: contract details, historical data, scanners, news, fundamentals.

use pyo3::prelude::*;

use crate::types::*;
use super::{wire_req_id, EClient};
use super::super::contract::Contract;
use crate::client_core::ClientCore;

#[pymethods]
impl EClient {
    /// Request historical bar data.
    ///
    /// `chart_options` is taken and not applied. This protocol's request
    /// carries no free-form option list, so what a caller puts in one cannot be
    /// sent. The reference client's own list is empty on every ordinary call.
    #[pyo3(signature = (req_id, contract, end_date_time, duration_str, bar_size_setting, what_to_show, use_rth, format_date=1, keep_up_to_date=false, chart_options=Vec::new()))]
    pub(crate) fn req_historical_data(
        &self,
        py: Python<'_>,
        req_id: i64,
        contract: &Contract,
        end_date_time: &str,
        duration_str: &str,
        bar_size_setting: &str,
        what_to_show: &str,
        use_rth: i32,
        format_date: i32,
        keep_up_to_date: bool,
        chart_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        // How this request wants its bar times written, as on the other
        // surface: the venue states one form and the caller may want the other.
        self.core.note_date_format(req_id, format_date);
        let _ = chart_options;
        // Whatever finished under this id before, this is a new request.
        if let Ok(wire) = wire_req_id(req_id) {
            self.core.historical_request_is_new(wire);
        }
        if !what_to_show.eq_ignore_ascii_case("SCHEDULE")
            && let Err(why) = ClientCore::validate_historical_args(
                bar_size_setting, what_to_show, keep_up_to_date,
            )
        {
            return self.report_refusal(py, req_id, why.into());
        }
        if what_to_show.eq_ignore_ascii_case("SCHEDULE") {
            Self::send_control(py, &tx, ControlCommand::FetchHistoricalSchedule {
                req_id: wire_req_id(req_id)?,
                con_id: contract.con_id,
                symbol: contract.symbol.clone(),
                sec_type: contract.sec_type.clone(),
                exchange: contract.exchange.clone(),
                currency: contract.currency.clone(),
                end_date_time: end_date_time.to_string(),
                duration: duration_str.to_string(),
                use_rth: use_rth != 0,
                filters: contract.lookup_filters(),
            })?;
        } else {
            Self::send_control(py, &tx, ControlCommand::FetchHistorical {
                req_id: wire_req_id(req_id)?,
                con_id: contract.con_id,
                symbol: contract.symbol.clone(),
                sec_type: contract.sec_type.clone(),
                exchange: contract.exchange.clone(),
                currency: contract.currency.clone(),
                end_date_time: end_date_time.to_string(),
                duration: duration_str.to_string(),
                bar_size: bar_size_setting.to_string(),
                what_to_show: what_to_show.to_string(),
                use_rth: use_rth != 0,
                keep_up_to_date,
                filters: contract.lookup_filters(),
            })?;
        }
        Ok(())
    }

    /// Cancel historical data.
    fn cancel_historical_data(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        let wire = wire_req_id(req_id)?;
        // A withdrawn stream leaves nothing running under this id.
        self.core.historical_request_is_new(wire);
        Self::send_control(py, &tx, ControlCommand::CancelHistorical { req_id: wire })?;
        Ok(())
    }

    /// Request head timestamp.
    #[pyo3(signature = (req_id, contract, what_to_show, use_rth, format_date=1))]
    pub(crate) fn req_head_time_stamp(
        &self,
        py: Python<'_>,
        req_id: i64,
        contract: &Contract,
        what_to_show: &str,
        use_rth: i32,
        format_date: i32,
    ) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchHeadTimestamp {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id,
            symbol: contract.symbol.clone(),
            sec_type: contract.sec_type.clone(),
            exchange: contract.exchange.clone(),
            currency: contract.currency.clone(),
            what_to_show: what_to_show.to_string(),
            use_rth: use_rth != 0,
            filters: contract.lookup_filters(),
        })?;
        self.core.note_date_format(req_id, format_date);
        Ok(())
    }

    /// Cancel head timestamp request.
    fn cancel_head_time_stamp(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::CancelHeadTimestamp { req_id: wire_req_id(req_id)? })?;
        Ok(())
    }

    /// Request contract details.
    pub(crate) fn req_contract_details(&self, py: Python<'_>, req_id: i64, contract: &Contract) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchContractDetails {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id,
            symbol: contract.symbol.clone(),
            sec_type: contract.sec_type.clone(),
            exchange: contract.exchange.clone(),
            currency: contract.currency.clone(),
            filters: contract.lookup_filters(),
        })?;
        Ok(())
    }

    /// Request available exchanges for market depth.
    fn req_mkt_depth_exchanges(&self, py: Python<'_>) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(-1) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchMktDepthExchanges)?;
        Ok(())
    }

    /// Search for matching symbols.
    pub(crate) fn req_matching_symbols(&self, py: Python<'_>, req_id: i64, pattern: &str) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchMatchingSymbols {
            req_id: wire_req_id(req_id)?,
            pattern: pattern.to_string(),
        })?;
        Ok(())
    }

    /// Request option chain parameters.
    #[pyo3(signature = (req_id, underlying_symbol, fut_fop_exchange="", underlying_sec_type="STK", underlying_con_id=0))]
    pub(crate) fn req_sec_def_opt_params(
        &self,
        py: Python<'_>,
        req_id: i64,
        underlying_symbol: &str,
        fut_fop_exchange: &str,
        underlying_sec_type: &str,
        underlying_con_id: i64,
    ) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchOptionParams {
            req_id: wire_req_id(req_id)?,
            symbol: underlying_symbol.to_string(),
            fut_fop_exchange: fut_fop_exchange.to_string(),
            underlying_sec_type: underlying_sec_type.to_string(),
            underlying_con_id,
        })?;
        Ok(())
    }

    /// Request scanner subscription.
    ///
    /// `scanner_subscription_options` is taken and not applied. This protocol's
    /// request carries no free-form option list, so what a caller puts in one
    /// cannot be sent. The reference client's own list is empty on every
    /// ordinary call.
    #[pyo3(signature = (req_id, subscription, scanner_subscription_options=Vec::new(), scanner_subscription_filter_options=Vec::new()))]
    fn req_scanner_subscription(
        &self,
        req_id: i64,
        subscription: Py<PyAny>,
        scanner_subscription_options: Vec<Py<PyAny>>,
        scanner_subscription_filter_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = scanner_subscription_options;
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Python::attach(|py| {
            let instrument = subscription.getattr(py, "instrument")
                .and_then(|v| v.extract::<String>(py)).unwrap_or_else(|_| "STK".to_string());
            let location_code = subscription.getattr(py, "locationCode")
                .and_then(|v| v.extract::<String>(py)).unwrap_or_else(|_| "STK.US.MAJOR".to_string());
            let scan_code = subscription.getattr(py, "scanCode")
                .and_then(|v| v.extract::<String>(py)).unwrap_or_else(|_| "TOP_PERC_GAIN".to_string());
            let max_items = subscription.getattr(py, "numberOfRows")
                .and_then(|v| v.extract::<u32>(py)).unwrap_or(50);
            let filters = scanner_filters(py, &subscription, &scanner_subscription_filter_options);
            Self::send_control(py, &tx, ControlCommand::SubscribeScanner {
                req_id: wire_req_id(req_id)?, instrument, location_code, scan_code, max_items, filters,
            })
        })
    }

    /// Cancel scanner subscription.
    fn cancel_scanner_subscription(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::CancelScanner { req_id: wire_req_id(req_id)? })?;
        Ok(())
    }

    /// Request scanner parameters XML.
    fn req_scanner_parameters(&self, py: Python<'_>) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(-1) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchScannerParams)?;
        Ok(())
    }

    /// Request a news article.
    ///
    /// `news_article_options` is taken and not applied. This protocol's request
    /// carries no free-form option list, so what a caller puts in one cannot be
    /// sent. The reference client's own list is empty on every ordinary call.
    #[pyo3(signature = (req_id, provider_code, article_id, news_article_options=Vec::new()))]
    fn req_news_article(
        &self,
        py: Python<'_>,
        req_id: i64,
        provider_code: &str,
        article_id: &str,
        news_article_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = news_article_options;
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchNewsArticle {
            req_id: wire_req_id(req_id)?,
            provider_code: provider_code.to_string(),
            article_id: article_id.to_string(),
        })?;
        Ok(())
    }

    /// Request historical news.
    ///
    /// `historical_news_options` is taken and not applied. This protocol's
    /// request carries no free-form option list, so what a caller puts in one
    /// cannot be sent. The reference client's own list is empty on every
    /// ordinary call.
    #[pyo3(signature = (req_id, con_id, provider_codes, start_date_time, end_date_time, total_results, historical_news_options=Vec::new()))]
    pub(crate) fn req_historical_news(
        &self,
        py: Python<'_>,
        req_id: i64,
        con_id: i64,
        provider_codes: &str,
        start_date_time: &str,
        end_date_time: &str,
        total_results: i32,
        historical_news_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = historical_news_options;
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchHistoricalNews {
            req_id: wire_req_id(req_id)?,
            con_id: con_id as u32,
            provider_codes: provider_codes.to_string(),
            start_time: start_date_time.to_string(),
            end_time: end_date_time.to_string(),
            max_results: total_results as u32,
        })?;
        Ok(())
    }

    /// Request fundamental data.
    ///
    /// `fundamental_data_options` is taken and not applied. This protocol's
    /// request carries no free-form option list, so what a caller puts in one
    /// cannot be sent. The reference client's own list is empty on every
    /// ordinary call.
    #[pyo3(signature = (req_id, contract, report_type, fundamental_data_options=Vec::new()))]
    pub(crate) fn req_fundamental_data(
        &self,
        py: Python<'_>,
        req_id: i64,
        contract: &Contract,
        report_type: &str,
        fundamental_data_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = fundamental_data_options;
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchFundamentalData {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id as u32,
            report_type: report_type.to_string(),
        })?;
        Ok(())
    }

    /// Cancel fundamental data.
    fn cancel_fundamental_data(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::CancelFundamentalData { req_id: wire_req_id(req_id)? })?;
        Ok(())
    }

    /// Request historical tick data.
    ///
    /// `ignore_size` and `misc_options` are taken and not applied. The request
    /// has no field for suppressing size-only changes, and none for a free-form
    /// option list.
    #[pyo3(signature = (req_id, contract, start_date_time="", end_date_time="", number_of_ticks=1000, what_to_show="TRADES", use_rth=1, ignore_size=false, misc_options=Vec::new()))]
    fn req_historical_ticks(
        &self,
        py: Python<'_>,
        req_id: i64,
        contract: &Contract,
        start_date_time: &str,
        end_date_time: &str,
        number_of_ticks: i32,
        what_to_show: &str,
        use_rth: i32,
        ignore_size: bool,
        misc_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        let _ = (ignore_size, misc_options);
        Self::send_control(py, &tx, ControlCommand::FetchHistoricalTicks {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id,
            symbol: contract.symbol.clone(),
            sec_type: contract.sec_type.clone(),
            exchange: contract.exchange.clone(),
            currency: contract.currency.clone(),
            start_date_time: start_date_time.to_string(),
            end_date_time: end_date_time.to_string(),
            number_of_ticks: number_of_ticks as u32,
            what_to_show: what_to_show.to_string(),
            use_rth: use_rth != 0,
            filters: contract.lookup_filters(),
        })?;
        Ok(())
    }

    /// Request market rule details.
    fn req_market_rule(&self, py: Python<'_>, market_rule_id: i32) -> PyResult<()> {
        // Released before the callback below — see the note in
        // req_completed_orders.
        let shared = self.shared.lock().unwrap().clone();
        if let Some(shared) = shared
            && let Some(rule) = shared.reference.market_rule(market_rule_id) {
                // Objects with names on them, as the reference client hands
                // them over and as the Rust surface here already did. A pair of
                // numbers left a program reading `lowEdge` holding a tuple.
                let steps: Vec<Py<pyo3::PyAny>> = rule.price_increments.iter()
                    .map(|pi| Py::new(py, super::super::contract::PriceIncrementPy {
                        low_edge: pi.low_edge,
                        increment: pi.increment,
                    }).map(|o| o.into_any()))
                    .collect::<PyResult<_>>()?;
                let list = pyo3::types::PyList::new(py, steps)?;
                self.callback(py, "market_rule", (market_rule_id as i64, list.as_any()))?;
                return Ok(());
            }
        // Answered, not logged. A caller waiting on a callback that will never
        // come cannot tell that apart from a slow venue, and the other client
        // here has answered this all along.
        self.callback(
            py,
            "error",
            (
                market_rule_id as i64,
                321i64,
                format!(
                    "market rule {market_rule_id} has not been seen on this session. Rules \
                     arrive with the details of a contract that uses them, so ask for such a \
                     contract first"
                ),
                "",
            ),
        )?;
        Ok(())
    }

    /// Request histogram data.
    #[pyo3(signature = (req_id, contract, use_rth, time_period))]
    pub(crate) fn req_histogram_data(&self, py: Python<'_>, req_id: i64, contract: &Contract, use_rth: bool, time_period: &str) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchHistogramData {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id as u32,
            use_rth,
            period: time_period.to_string(),
        })?;
        Ok(())
    }

    /// Cancel histogram data.
    fn cancel_histogram_data(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::CancelHistogramData { req_id: wire_req_id(req_id)? })?;
        Ok(())
    }

    /// Request historical trading schedule.
    #[pyo3(signature = (req_id, contract, end_date_time="", duration_str="1 M", use_rth=true))]
    pub(crate) fn req_historical_schedule(
        &self, py: Python<'_>, req_id: i64, contract: &Contract,
        end_date_time: &str, duration_str: &str, use_rth: bool,
    ) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::FetchHistoricalSchedule {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id,
            symbol: contract.symbol.clone(),
            sec_type: contract.sec_type.clone(),
            exchange: contract.exchange.clone(),
            currency: contract.currency.clone(),
            end_date_time: end_date_time.into(),
            duration: duration_str.into(),
            use_rth,
            filters: contract.lookup_filters(),
        })?;
        Ok(())
    }
}

/// `ScannerSubscription` attribute -> the scanner filter code it selects. Everything a
/// caller sets beyond instrument / location / scan code is a filter, and a filter the
/// subscription drops is a different result set.
const SCANNER_FILTERS: &[(&str, &str)] = &[
    ("abovePrice", "priceAbove"),
    ("belowPrice", "priceBelow"),
    ("aboveVolume", "volumeAbove"),
    ("marketCapAbove", "marketCapAbove1e6"),
    ("marketCapBelow", "marketCapBelow1e6"),
    ("moodyRatingAbove", "moodyRatingAbove"),
    ("moodyRatingBelow", "moodyRatingBelow"),
    ("spRatingAbove", "spRatingAbove"),
    ("spRatingBelow", "spRatingBelow"),
    ("maturityDateAbove", "maturityDateAbove"),
    ("maturityDateBelow", "maturityDateBelow"),
    ("couponRateAbove", "couponRateAbove"),
    ("couponRateBelow", "couponRateBelow"),
    ("averageOptionVolumeAbove", "avgOptVolumeAbove"),
];

/// `stockTypeFilter` name -> its filter value. Anything else, `ALL` included, is no filter.
fn stk_types_code(name: &str) -> &'static str {
    match name.to_ascii_uppercase().as_str() {
        "STOCK" => "exc:ETF",
        "ETF" => "inc:ETF",
        "CORP" => "inc:CORP",
        "ADR" => "inc:ADR",
        "REIT" => "inc:REIT",
        "CEF" => "inc:CEF",
        _ => "",
    }
}

/// One filter value, or `None` when the attribute is missing or left at its unset default.
fn scanner_filter_value(py: Python<'_>, sub: &Py<PyAny>, attr: &str) -> Option<String> {
    let value = sub.getattr(py, attr).ok()?;
    if let Ok(n) = value.extract::<f64>(py) {
        // An unset numeric filter arrives as `sys.float_info.max` or `2**31 - 1`, and
        // sending either as a bound would empty the scan.
        if n == f64::MAX || n == f64::from(i32::MAX) {
            return None;
        }
        return Some(n.to_string());
    }
    let text = value.extract::<String>(py).ok()?;
    (!text.is_empty()).then_some(text)
}

/// Collect the subscription's filters, then the caller's explicit filter tags, which win
/// over the named attribute selecting the same code.
fn scanner_filters(py: Python<'_>, sub: &Py<PyAny>, filter_options: &[Py<PyAny>]) -> Vec<(String, String)> {
    let mut filters: Vec<(String, String)> = SCANNER_FILTERS.iter()
        .filter_map(|(attr, code)| Some(((*code).to_string(), scanner_filter_value(py, sub, attr)?)))
        .collect();

    if sub.getattr(py, "excludeConvertible").and_then(|v| v.extract::<bool>(py)).unwrap_or(false) {
        filters.push(("excludeConvertible".to_string(), "true".to_string()));
    }
    let stk_types = sub.getattr(py, "stockTypeFilter")
        .and_then(|v| v.extract::<String>(py)).unwrap_or_default();
    let stk_types = stk_types_code(&stk_types);
    if !stk_types.is_empty() {
        filters.push(("stkTypes".to_string(), stk_types.to_string()));
    }

    for option in filter_options {
        let (Ok(tag), Ok(value)) = (
            option.getattr(py, "tag").and_then(|v| v.extract::<String>(py)),
            option.getattr(py, "value").and_then(|v| v.extract::<String>(py)),
        ) else { continue };
        if tag.is_empty() {
            continue;
        }
        filters.retain(|(code, _)| *code != tag);
        filters.push((tag, value));
    }
    filters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespace(py: Python<'_>, fields: &str) -> Py<PyAny> {
        py.eval(&std::ffi::CString::new(format!("__import__('types').SimpleNamespace({fields})")).unwrap(), None, None)
            .unwrap().unbind()
    }

    #[test]
    fn scanner_subscription_attributes_become_filters() {
        Python::initialize();
        Python::attach(|py| {
            let sub = namespace(py, "abovePrice=10.0, belowPrice=1.7976931348623157e+308, \
                aboveVolume=2147483647, marketCapAbove=1.7976931348623157e+308, \
                moodyRatingAbove='', spRatingAbove='A', averageOptionVolumeAbove=500, \
                excludeConvertible=True, stockTypeFilter='etf'");
            assert_eq!(scanner_filters(py, &sub, &[]), vec![
                ("priceAbove".to_string(), "10".to_string()),
                ("spRatingAbove".to_string(), "A".to_string()),
                ("avgOptVolumeAbove".to_string(), "500".to_string()),
                ("excludeConvertible".to_string(), "true".to_string()),
                ("stkTypes".to_string(), "inc:ETF".to_string()),
            ]);
        });
    }

    #[test]
    fn explicit_filter_tags_replace_the_attribute_for_the_same_code() {
        Python::initialize();
        Python::attach(|py| {
            let sub = namespace(py, "abovePrice=10.0, excludeConvertible=False, stockTypeFilter='ALL'");
            let options = [
                namespace(py, "tag='priceAbove', value='20'"),
                namespace(py, "tag='usdMarketCapAbove', value='10000'"),
            ];
            assert_eq!(scanner_filters(py, &sub, &options), vec![
                ("priceAbove".to_string(), "20".to_string()),
                ("usdMarketCapAbove".to_string(), "10000".to_string()),
            ]);
        });
    }
}
