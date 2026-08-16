//! Reference data: contract details, historical data, scanners, news, fundamentals.

use crate::types::*;
use crate::error_codes::Refusal;

use super::{wire_req_id, Contract, EClient, TagValue};
use crate::client_core::ClientCore;

/// What this client reports when a market rule has not been seen.
const MARKET_RULE_NOT_KNOWN: i64 = 321;

impl EClient {
    // ── Historical Data ──

    /// Request historical data. Matches `reqHistoricalData` in C++.
    pub fn req_historical_data(
        &self, req_id: i64, contract: &Contract,
        end_date_time: &str, duration: &str, bar_size: &str,
        what_to_show: &str, use_rth: bool, format_date: i32, keep_up_to_date: bool,
    ) -> Result<(), Refusal> {
        ClientCore::validate_historical_args(bar_size, what_to_show, keep_up_to_date)?;
        // How this request wants its bar times written. The venue states one
        // form; the counterpart writes whichever the caller asked for.
        self.core.note_date_format(req_id, format_date);
        self.send(ControlCommand::FetchHistorical {
            contract: contract.into(),
            req_id: wire_req_id(req_id)?,
            end_date_time: end_date_time.into(),
            duration: duration.into(),
            bar_size: bar_size.into(),
            filters: contract.lookup_filters(),
            what_to_show: what_to_show.into(),
            use_rth,
            keep_up_to_date,
        })
    }

    /// Cancel historical data. Matches `cancelHistoricalData` in C++.
    pub fn cancel_historical_data(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelHistorical { req_id: wire_req_id(req_id)? })
    }

    /// Request head timestamp. Matches `reqHeadTimeStamp` in C++.
    pub fn req_head_time_stamp(
        &self, req_id: i64, contract: &Contract, what_to_show: &str, use_rth: bool,
        format_date: i32,
    ) -> Result<(), Refusal> {
        self.core.note_date_format(req_id, format_date);
        self.send(ControlCommand::FetchHeadTimestamp {
            contract: contract.into(),
            req_id: wire_req_id(req_id)?,
            filters: contract.lookup_filters(),
            what_to_show: what_to_show.into(),
            use_rth,
        })
    }

    // ── Contract Details ──

    /// Request contract details. Matches `reqContractDetails` in C++.
    pub fn req_contract_details(&self, req_id: i64, contract: &Contract) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchContractDetails {
            contract: contract.into(),
            req_id: wire_req_id(req_id)?,
            filters: contract.lookup_filters(),
        })
    }

    /// Request available exchanges for market depth.
    pub fn req_mkt_depth_exchanges(&self) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchMktDepthExchanges)
    }

    /// Request matching symbols. Matches `reqMatchingSymbols` in C++.
    pub fn req_matching_symbols(&self, req_id: i64, pattern: &str) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchMatchingSymbols {
            req_id: wire_req_id(req_id)?,
            pattern: pattern.into(),
        })
    }

    /// Ask what event types the corporate-events calendar carries.
    ///
    /// Has to be asked before events can be: the counterpart holds the answer
    /// and will not build an event request without it.
    pub fn req_wsh_meta_data(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchCalendarMetaData { req_id: wire_req_id(req_id)? })
    }

    /// Stop waiting on the event types.
    ///
    /// The query is one message and one answer, so there is nothing at the
    /// venue to withdraw: what is withdrawn is the answer, which would
    /// otherwise reach a caller who has said they are done with it. A cancel
    /// naming no waiting request says so rather than returning as though it
    /// acted.
    pub fn cancel_wsh_meta_data(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelCalendar { req_id: wire_req_id(req_id)? })
    }

    /// Stop waiting on the calendar's events. As above.
    pub fn cancel_wsh_event_data(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelCalendar { req_id: wire_req_id(req_id)? })
    }

    /// Ask the corporate-events calendar for events.
    ///
    /// A caller either names a contract or writes its own filter. The filter
    /// goes to the venue as written: the venue validates it, and rewriting it
    /// here would change what was asked.
    pub fn req_wsh_event_data(
        &self,
        req_id: i64,
        query: crate::control::calendar::CalendarQuery,
    ) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchCalendarEvents {
            req_id: wire_req_id(req_id)?,
            query: Box::new(query),
        })
    }

    /// Request option chain parameters. Matches `reqSecDefOptParams` in C++.
    ///
    /// `fut_fop_exchange` names the venue for a futures option chain and is
    /// empty for an equity or index one.
    pub fn req_sec_def_opt_params(
        &self, req_id: i64, underlying_symbol: &str, fut_fop_exchange: &str,
        underlying_sec_type: &str, underlying_con_id: i64,
    ) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchOptionParams {
            req_id: wire_req_id(req_id)?,
            symbol: underlying_symbol.into(),
            fut_fop_exchange: fut_fop_exchange.into(),
            underlying_sec_type: underlying_sec_type.into(),
            underlying_con_id,
        })
    }

    /// Cancel head timestamp request. Matches `cancelHeadTimestamp` in C++.
    pub fn cancel_head_time_stamp(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelHeadTimestamp { req_id: wire_req_id(req_id)? })
    }

    /// The price increments a market rule states. Matches `reqMarketRule` in C++.
    ///
    /// A rule is not asked for on its own: the venue sends the rules a contract
    /// uses along with that contract's details. So this answers from what those
    /// have already brought in, and says so when the rule is not among them
    /// rather than returning in silence.
    pub fn req_market_rule(&self, market_rule_id: i32, wrapper: &mut impl crate::api::wrapper::Wrapper) {
        match self.shared.reference.market_rule(market_rule_id) {
            Some(rule) => wrapper.market_rule(market_rule_id as i64, &rule.price_increments.iter()
                .map(|pi| crate::api::types::PriceIncrement { low_edge: pi.low_edge, increment: pi.increment })
                .collect::<Vec<_>>()),
            None => wrapper.error(
                market_rule_id as i64,
                MARKET_RULE_NOT_KNOWN,
                &format!(
                    "market rule {market_rule_id} has not been seen on this session. Rules \
                     arrive with the details of a contract that uses them, so ask for such a \
                     contract first"
                ),
                "",
            ),
        }
    }

    // ── News Bulletins ──

    /// Subscribe to news bulletins. Matches `reqNewsBulletins` in C++.
    ///
    /// `all_msgs` is taken and not applied. The subscription carries no field
    /// asking for the bulletins that came before it, so what arrives is what is
    /// published from here on.
    pub fn req_news_bulletins(&self, _all_msgs: bool) {
        self.core.subscribe_bulletins();
    }

    /// Cancel news bulletin subscription. Matches `cancelNewsBulletins` in C++.
    pub fn cancel_news_bulletins(&self) {
        self.core.unsubscribe_bulletins();
    }

    // ── Scanner ──

    /// Request scanner parameters XML. Matches `reqScannerParameters` in C++.
    pub fn req_scanner_parameters(&self) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchScannerParams)
    }

    /// Subscribe to a market scanner. Matches `reqScannerSubscription` in C++.
    ///
    /// `filters` are the scanner filter tags named by `req_scanner_parameters`,
    /// e.g. `priceAbove` = `"10"` or `stkTypes` = `"inc:ETF"`.
    pub fn req_scanner_subscription(
        &self, req_id: i64, instrument: &str, location_code: &str,
        scan_code: &str, max_items: u32, filters: &[TagValue],
    ) -> Result<(), Refusal> {
        self.send(ControlCommand::SubscribeScanner {
            req_id: wire_req_id(req_id)?,
            instrument: instrument.into(),
            location_code: location_code.into(),
            scan_code: scan_code.into(),
            max_items,
            filters: filters.iter().map(|f| (f.tag.clone(), f.value.clone())).collect(),
        })
    }

    /// Cancel a scanner subscription. Matches `cancelScannerSubscription` in C++.
    pub fn cancel_scanner_subscription(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelScanner { req_id: wire_req_id(req_id)? })
    }

    // ── News ──

    /// Request historical news headlines. Matches `reqHistoricalNews` in C++.
    pub fn req_historical_news(
        &self, req_id: i64, con_id: i64, provider_codes: &str,
        start_time: &str, end_time: &str, max_results: u32,
    ) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchHistoricalNews {
            req_id: wire_req_id(req_id)?,
            con_id: con_id as u32,
            provider_codes: provider_codes.into(),
            start_time: start_time.into(),
            end_time: end_time.into(),
            max_results,
        })
    }

    /// Request a news article by provider and article ID. Matches `reqNewsArticle` in
    /// C++.
    pub fn req_news_article(&self, req_id: i64, provider_code: &str, article_id: &str) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchNewsArticle {
            req_id: wire_req_id(req_id)?,
            provider_code: provider_code.into(),
            article_id: article_id.into(),
        })
    }

    // ── Fundamental Data ──

    /// Request fundamental data (e.g. ReportSnapshot, ReportsFinSummary). Matches
    /// `reqFundamentalData` in C++.
    pub fn req_fundamental_data(&self, req_id: i64, contract: &Contract, report_type: &str) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchFundamentalData {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id as u32,
            report_type: report_type.into(),
        })
    }

    /// Cancel fundamental data. Matches `cancelFundamentalData` in C++.
    pub fn cancel_fundamental_data(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelFundamentalData { req_id: wire_req_id(req_id)? })
    }

    // ── Histogram ──

    /// Request price histogram data. Matches `reqHistogramData` in C++.
    pub fn req_histogram_data(&self, req_id: i64, contract: &Contract, use_rth: bool, period: &str) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchHistogramData {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id as u32,
            use_rth,
            period: period.into(),
        })
    }

    /// Cancel histogram data. Matches `cancelHistogramData` in C++.
    pub fn cancel_histogram_data(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelHistogramData { req_id: wire_req_id(req_id)? })
    }

    // ── Historical Ticks ──

    /// Request historical tick data. Matches `reqHistoricalTicks` in C++.
    pub fn req_historical_ticks(
        &self, req_id: i64, contract: &Contract,
        start_date_time: &str, end_date_time: &str,
        number_of_ticks: i32, what_to_show: &str, use_rth: bool,
    ) -> Result<(), Refusal> {
        // Refused here rather than turned into trades on the way out.
        crate::control::historical::tick_data_type(what_to_show)?;
        self.send(ControlCommand::FetchHistoricalTicks {
            contract: contract.into(),
            req_id: wire_req_id(req_id)?,
            start_date_time: start_date_time.into(),
            end_date_time: end_date_time.into(),
            filters: contract.lookup_filters(),
            number_of_ticks: number_of_ticks as u32,
            what_to_show: what_to_show.into(),
            use_rth,
        })
    }

    // ── Historical Schedule ──

    /// Request historical trading schedule. Matches `reqHistoricalSchedule` in C++.
    pub fn req_historical_schedule(
        &self, req_id: i64, contract: &Contract,
        end_date_time: &str, duration: &str, use_rth: bool,
    ) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchHistoricalSchedule {
            contract: contract.into(),
            req_id: wire_req_id(req_id)?,
            filters: contract.lookup_filters(),
            end_date_time: end_date_time.into(),
            duration: duration.into(),
            use_rth,
        })
    }
}
