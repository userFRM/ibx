//! Reference data: contract details, historical data, scanners, news, fundamentals.

use crate::types::*;
use crate::error_codes::Refusal;

use super::{wire_req_id, wire_text, Contract, EClient, TagValue};
use crate::client_core::ClientCore;

/// What this client reports when a market rule has not been seen.
const MARKET_RULE_NOT_KNOWN: i64 = 321;

/// Narrow a contract id to the width the requests below carry it in.
///
/// These name a contract by its venue id and carry nothing else of it,
/// so a description is not a contract they can ask about: sent as it stood, a
/// A contract with no id names contract zero, and one with a negative id names
/// the largest id there is. The venue answers both with silence, which reads
/// as the venue holding nothing.
pub(crate) fn wire_con_id(con_id: i64, what: &str) -> Result<u32, Refusal> {
    u32::try_from(con_id).ok().filter(|id| *id > 0).ok_or_else(|| {
        Refusal::validation(format!(
            "{what} names its contract by its venue id, and {con_id} is not one:              qualify the contract first and pass what comes back",
        ))
    })
}

impl EClient {
    // ── Historical Data ──

    /// Request historical data. Matches `reqHistoricalData` in C++.
    ///
    /// With `keep_up_to_date`, the bar still forming is folded here from the
    /// stream the venue sends, and it opens on a whole multiple of its own
    /// length counted from the epoch. For every size up to an hour that is the
    /// clock boundary a caller expects. For `1 day` it is midnight UTC, which
    /// is the trading day of an instrument that trades around the clock and is
    /// the middle of the evening for one that does not — a US listing's
    /// forming daily bar opens in its after-hours session and spans two of
    /// them. Bars already closed are the venue's own and are not folded here.
    pub fn req_historical_data(
        &self, req_id: i64, contract: &Contract,
        end_date_time: &str, duration: &str, bar_size: &str,
        what_to_show: &str, use_rth: bool, format_date: i32, keep_up_to_date: bool,
    ) -> Result<(), Refusal> {
        // Named by the venue where the caller named it by id alone: a
        // request states the contract's type and its exchange, and both
        // are the venue's to say.
        let contract = &*self.named_by_the_venue(contract)?;
        ClientCore::validate_historical_args(bar_size, what_to_show, keep_up_to_date)?;
        // The adjusted series folds the raw trades with the contract's own
        // corporate actions, which are asked for by the venue's id for the
        // contract. Named by anything else, that ask cannot be made — so a
        // request that could not be folded is refused here rather than answered
        // with raw trades under an adjusted name. The waiting call refuses the
        // same request the same way.
        if crate::control::historical::what_to_show_is_adjusted(what_to_show)
            && contract.con_id == 0
        {
            return Err(Refusal::validation(
                "ADJUSTED_LAST is folded from the contract's corporate actions, which are \
                 asked for by the venue's id for the contract, and this one does not carry \
                 it: qualify the contract first and pass what comes back".to_string(),
            ));
        }
        // How this request wants its bar times written. The venue states one
        // form; whichever the caller asked for is what is written.
        self.core.note_date_format(req_id, format_date);
        // And what its range is counted from, which the reply does not state.
        self.core.note_historical_span(req_id, end_date_time, duration);
        let wire = wire_req_id(req_id)?;
        // Whatever finished under this id before, this is a new request. The
        // other surface said so and this one did not: the id stayed marked
        // finished, so the bars answering the new request were delivered as
        // updates continuing the old one and its end never fired.
        self.core.historical_request_is_new(wire);
        self.send(ControlCommand::FetchHistorical {
            contract: contract.into(),
            req_id: wire,
            end_date_time: end_date_time.into(),
            duration: duration.into(),
            bar_size: bar_size.into(),
            filters: contract.lookup_filters(),
            what_to_show: what_to_show.into(),
            use_rth,
            keep_up_to_date,
            include_expired: contract.include_expired,
        })
    }

    /// Cancel historical data. Matches `cancelHistoricalData` in C++.
    pub fn cancel_historical_data(&self, req_id: i64) -> Result<(), Refusal> {
        let wire = wire_req_id(req_id)?;
        // A withdrawn stream leaves nothing running under this id.
        self.core.historical_request_is_new(wire);
        self.send(ControlCommand::CancelHistorical { req_id: wire })
    }

    /// Request head timestamp. Matches `reqHeadTimeStamp` in C++.
    pub fn req_head_time_stamp(
        &self, req_id: i64, contract: &Contract, what_to_show: &str, use_rth: bool,
        format_date: i32,
    ) -> Result<(), Refusal> {
        // Named by the venue where the caller named it by id alone: a
        // request states the contract's type and its exchange, and both
        // are the venue's to say.
        let contract = &*self.named_by_the_venue(contract)?;
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
        wire_text("a matching-symbols pattern", pattern)?;
        self.send(ControlCommand::FetchMatchingSymbols {
            req_id: wire_req_id(req_id)?,
            pattern: pattern.into(),
        })
    }

    /// Ask what event types the corporate-events calendar carries.
    ///
    /// Independent of the events themselves: neither request needs the other,
    /// and either may be asked first.
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
        query: crate::types::CalendarQuery,
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
                .map(|pi| crate::types::model::PriceIncrement { low_edge: pi.low_edge, increment: pi.increment })
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
    /// `all_msgs` asks for the day's bulletins as well as the ones still to
    /// come. The subscription carries no field asking the venue for them, but
    /// the venue has been broadcasting them at this session since it opened
    /// and they are still queued, so a caller asking for every message of the
    /// day is answered from those. Asking only for what follows drops them,
    /// which is what stopped a subscription from opening with bulletins
    /// published before anyone asked for any.
    pub fn req_news_bulletins(&self, all_msgs: bool) {
        if self.session_over() { return self.report_reason(-1, &Refusal::not_connected("Not connected")); }
        if !all_msgs {
            let _ = self.shared.market.drain_news_bulletins();
        }
        self.core.subscribe_bulletins();
    }

    /// Cancel news bulletin subscription. Matches `cancelNewsBulletins` in C++.
    pub fn cancel_news_bulletins(&self) {
        if self.session_over() { return self.report_reason(-1, &Refusal::not_connected("Not connected")); }
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
    ///
    /// `start_time` and `end_time` are refused rather than taken and dropped:
    /// the query this client sends carries no time bounds, and `max_results` is
    /// what limits the answer.
    ///
    /// No more than three hundred are asked for however many are wanted. The
    /// reference client caps it there before the request goes out, so a bigger
    /// number is one the venue is never asked.
    pub fn req_historical_news(
        &self, req_id: i64, con_id: i64, provider_codes: &str,
        start_time: &str, end_time: &str, max_results: u32,
    ) -> Result<(), Refusal> {
        crate::control::news::validate_news_window(start_time, end_time)?;
        self.send(ControlCommand::FetchHistoricalNews {
            req_id: wire_req_id(req_id)?,
            con_id: wire_con_id(con_id, "a request for headlines")?,
            provider_codes: provider_codes.into(),
            start_time: start_time.into(),
            end_time: end_time.into(),
            max_results: max_results.min(crate::control::news::MOST_HEADLINES_ASKED_FOR),
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

    // ── Corporate actions ──

    /// Ask for a contract's corporate actions over a range of days.
    ///
    /// The answer is filed against the contract it names rather than handed to
    /// a callback under this id, because the venue answers per contract:
    /// [`EClient::adjustments`](crate::EClient::adjustments) reads it once it
    /// has arrived, and [`corporate_actions`](crate::EClient::corporate_actions)
    /// asks and waits in one call.
    ///
    /// `start_date` and `end_date` are days, as `YYYYMMDD`.
    pub fn req_adjustments(
        &self, req_id: i64, con_id: i64, sec_type: &str, exchange: &str,
        start_date: &str, end_date: &str,
    ) -> Result<(), Refusal> {
        // The reserved band is refused where every caller's number is narrowed,
        // which is every request rather than this one: a number taken from it
        // collides on any of them.
        let numbered = wire_req_id(req_id)?;
        self.ask_for_adjustments(numbered, con_id, sec_type, exchange, start_date, end_date)
    }

    /// Send a corporate-actions request under a number already settled.
    ///
    /// Not public, and not a caller's to reach: it carries no check on the
    /// number, because the only thing that reaches it with one from the
    /// reserved band is the answering call that owns that number.
    pub(crate) fn ask_for_adjustments(
        &self, req_id: u32, con_id: i64, sec_type: &str, exchange: &str,
        start_date: &str, end_date: &str,
    ) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchAdjustments {
            req_id,
            con_id: wire_con_id(con_id, "a request for corporate actions")?,
            sec_type: sec_type.into(),
            exchange: exchange.into(),
            start_date: start_date.into(),
            end_date: end_date.into(),
        })
    }

    // ── Fundamental Data ──

    /// Request fundamental data. Matches `reqFundamentalData` in C++.
    ///
    /// Three reports, which are the three the venue states: `ReportSnapshot`,
    /// `RESC` for what analysts expect, and `CalendarReport` for what the
    /// issuer has coming.
    ///
    /// The contract is named by its venue id and nothing else of it is
    /// carried, so pass one that has an id: from
    /// [`qualify_contract`](EClient::qualify_contract), or from any
    /// contract-details answer. A description is refused rather than sent as a
    /// request about contract zero.
    pub fn req_fundamental_data(&self, req_id: i64, contract: &Contract, report_type: &str) -> Result<(), Refusal> {
        self.send(ControlCommand::FetchFundamentalData {
            req_id: wire_req_id(req_id)?,
            con_id: wire_con_id(contract.con_id, "a request for a fundamental report")?,
            report_type: report_type.into(),
        })
    }

    /// Cancel fundamental data. Matches `cancelFundamentalData` in C++.
    pub fn cancel_fundamental_data(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelFundamentalData { req_id: wire_req_id(req_id)? })
    }

    /// Withdraw a historical news query. Matches `cancelHistoricalNews` in C++.
    ///
    /// One message carrying the id the query went out under, which is the whole
    /// of what a withdrawal states. Sent whether or not the query has been
    /// answered: the venue serves it past the reply, so a withdrawal gated on
    /// this client's own pending list would send nothing in the case that
    /// leaves one running.
    pub fn cancel_historical_news(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelHistoricalNews { req_id: wire_req_id(req_id)? })
    }

    // ── Histogram ──

    /// Request price histogram data. Matches `reqHistogramData` in C++.
    ///
    /// Named by its venue id, as
    /// [`req_fundamental_data`](EClient::req_fundamental_data) is.
    pub fn req_histogram_data(&self, req_id: i64, contract: &Contract, use_rth: bool, period: &str) -> Result<(), Refusal> {
        // Named by the venue where the caller named it by id alone: a
        // request states the contract's type and its exchange, and both
        // are the venue's to say.
        let contract = &*self.named_by_the_venue(contract)?;
        self.send(ControlCommand::FetchHistogramData {
            req_id: wire_req_id(req_id)?,
            con_id: wire_con_id(contract.con_id, "a request for a histogram")?,
            sec_type: contract.sec_type.clone(),
            exchange: contract.exchange.clone(),
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
    ///
    /// Named from one end and counted from there: give `start_date_time` for
    /// the ticks after a moment or `end_date_time` for the ones before it, and
    /// `number_of_ticks` says how far it reaches. Naming both, or neither, is
    /// what the venue refuses.
    pub fn req_historical_ticks(
        &self, req_id: i64, contract: &Contract,
        start_date_time: &str, end_date_time: &str,
        number_of_ticks: i32, what_to_show: &str, use_rth: bool,
    ) -> Result<(), Refusal> {
        // Before anything that reaches the venue, so an id it cannot carry is
        // named as the trouble rather than whatever is checked first.
        let wire_id = wire_req_id(req_id)?;
        // Named by the venue where the caller named it by id alone: a
        // request states the contract's type and its exchange, and both
        // are the venue's to say.
        let contract = &*self.named_by_the_venue(contract)?;
        // Refused here rather than turned into trades on the way out.
        crate::control::historical::tick_data_type(what_to_show)?;
        // A count below zero is not a count. Cast unchecked it became a
        // request for four billion ticks, which the venue answers by refusing
        // a request the caller never made.
        let number_of_ticks = u32::try_from(number_of_ticks).map_err(|_| {
            Refusal::validation(format!("number_of_ticks {number_of_ticks} is negative"))
        })?;
        crate::control::historical::validate_tick_window(start_date_time, end_date_time)?;
        self.send(ControlCommand::FetchHistoricalTicks {
            contract: contract.into(),
            req_id: wire_id,
            start_date_time: start_date_time.into(),
            end_date_time: end_date_time.into(),
            filters: contract.lookup_filters(),
            number_of_ticks,
            what_to_show: what_to_show.into(),
            use_rth,
            include_expired: contract.include_expired,
        })
    }

    // ── Historical Schedule ──

    /// Request historical trading schedule. Matches `reqHistoricalSchedule` in C++.
    pub fn req_historical_schedule(
        &self, req_id: i64, contract: &Contract,
        end_date_time: &str, duration: &str, use_rth: bool,
    ) -> Result<(), Refusal> {
        // Named by the venue where the caller named it by id alone: a
        // request states the contract's type and its exchange, and both
        // are the venue's to say.
        let contract = &*self.named_by_the_venue(contract)?;
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
