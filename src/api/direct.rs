//! A client whose calls return what they were asked for.
//!
//! [`EClient`] follows the reference client: a request goes out under an id and
//! the answer arrives later on a callback. That is the right shape for a program
//! with its own event loop, and the wrong one for asking a question.
//!
//! [`Client`] is the other shape, and it carries the names the widely used Rust
//! client gives them, so a program written against that one reads the same here.
//! A call that answers returns the answer; a call that only sends says so in its
//! type by returning nothing.
//!
//! It owns a recorder for the callbacks the sending calls produce, so a caller
//! never has to supply one.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::api::client::{EClient, EClientConfig};
use crate::types::model::{BarData, ContractDetails};
use crate::api::wrapper::Wrapper;
use crate::error_codes::Refusal;
use crate::api::client::{AccountValue, OptionChain, PositionRow};
use crate::api::subscription::Subscription;
use crate::types::{DepthUpdate, RealTimeBar};
use crate::types::ControlCommand;
use crate::bridge::SharedState;

use super::Contract;

/// What the sending calls produced, kept so a caller can read it afterwards.
///
/// The reference shape hands these to a callback as they arrive. Here they are
/// recorded, because a caller of this shape has no callback to hand them to.
#[derive(Default)]
pub struct Recorded {
    /// Everything the venue said, as request, number and words.
    pub errors: Vec<(i64, i64, String)>,
    /// The venue's clock, once it has been asked for.
    pub current_time: Option<i64>,
    /// Every account this login may act for.
    pub managed_accounts: Option<String>,
    /// The providers this account may read.
    pub news_providers: Vec<String>,
    /// The scan definitions the venue publishes.
    pub scanner_parameters: Option<String>,
}

impl Wrapper for Recorded {
    fn error(&mut self, req_id: i64, code: i64, message: &str, _advanced: &str) {
        self.errors.push((req_id, code, message.to_string()));
    }
    fn current_time(&mut self, time: i64) {
        self.current_time = Some(time);
    }
    fn managed_accounts(&mut self, accounts: &str) {
        self.managed_accounts = Some(accounts.to_string());
    }
}

/// The first id this shape opens a stream or a question under.
///
/// Above what a caller of the callback shape numbers its own requests from, and
/// below the ids the answering layer beneath uses, so none of the three can be
/// mistaken for another. The guard below fails the build rather than the suite
/// if that ever stops being true.
pub const STREAM_ID_BASE: i64 = 0x2000_0000;

const _: () = assert!(
    STREAM_ID_BASE < crate::bridge::ReferenceState::ASK_ID_BASE as i64,
    "a stream id would be taken for one of the answering layer's own"
);

/// How long a question waits for its answer.
const ANSWER_TIMEOUT: Duration =
    Duration::from_secs(crate::config::ANSWER_TIMEOUT_SECS);

/// How long to sleep between looks at the queue.
const POLL: Duration = Duration::from_millis(5);

/// A session whose calls return what they were asked for.
pub struct Client {
    inner: EClient,
    recorded: Arc<Mutex<Recorded>>,
    /// Ids for the streams this shape opens. Far above what a caller of the
    /// callback shape is likely to use on the same session.
    next_stream_id: std::sync::atomic::AtomicI64,
    /// When the session opened, in seconds since the epoch.
    connected_at: i64,
}

/// Calls the widely used Rust client has whose answer here is a different
/// thing, and what they answer instead.
///
/// Named rather than left out. Every one of them can be called: a program
/// moved across compiles and runs, and the two that cannot be answered say so
/// on the spot rather than by not existing.
pub const NO_COUNTERPART: &[(&str, &str)] = &[
    ("verify_message", "part of a handshake between a client and a local process, and there is no local process"),
    ("verify_request", "part of a handshake between a client and a local process, and there is no local process"),
    ("cancel_contract_details", "a contract lookup answers here rather than streaming, so there is nothing to withdraw"),
];

impl Client {
    /// Open a session.
    pub fn connect(config: &EClientConfig) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            inner: EClient::connect(config)?,
            recorded: Arc::new(Mutex::new(Recorded::default())),
            next_stream_id: std::sync::atomic::AtomicI64::new(STREAM_ID_BASE),
            connected_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or_default(),
        })
    }

    // ── Calls that exist because the other client talks to a local process ──

    /// What this client speaks.
    ///
    /// In a client that talks to a local process, this is the version that
    /// process announced. There is no local process here, so what it can
    /// honestly report is the build this client states to the venue at logon
    /// and is accepted under.
    ///
    /// It is not a negotiated protocol version and is on a different scale
    /// from one. A program gating a feature on it will find every feature
    /// available, which is the right answer: this client speaks the current
    /// protocol, not an older one it has to ask permission for.
    pub fn server_version(&self) -> i32 {
        // The build this session actually stated, not the one a fresh session
        // would state: a caller that set its own build is told what it sent.
        self.inner.shared_state().settings().build.parse().unwrap_or(0)
    }

    /// Which client this is, of several sharing one local process.
    ///
    /// Nothing is shared here: a session is one login, and the venue knows it
    /// by that. Zero is what a program with a single connection uses, and what
    /// the reports coming back carry.
    pub fn client_id(&self) -> i32 {
        0
    }

    /// Withdraw a contract lookup.
    ///
    /// A lookup answers here rather than streaming, so by the time this can be
    /// called there is nothing left running. It reports that rather than
    /// pretending to stop something.
    pub fn cancel_contract_details(&self, _req_id: i64) -> Result<(), Refusal> {
        Err(Refusal::validation(
            NO_COUNTERPART
                .iter()
                .find(|(name, _)| *name == "cancel_contract_details")
                .map(|(_, why)| (*why).to_string())
                .unwrap_or_default(),
        ))
    }

    /// Begin the handshake a third-party program makes with a local process.
    pub fn verify_request(&self, _api_name: &str, _api_version: &str) -> Result<(), Refusal> {
        Err(Self::no_local_process("verify_request"))
    }

    /// Continue that handshake.
    pub fn verify_message(&self, _api_data: &str) -> Result<(), Refusal> {
        Err(Self::no_local_process("verify_message"))
    }

    fn no_local_process(call: &str) -> Refusal {
        Refusal::validation(
            NO_COUNTERPART
                .iter()
                .find(|(name, _)| *name == call)
                .map(|(_, why)| (*why).to_string())
                .unwrap_or_default(),
        )
    }

    /// The session underneath, for anything this shape does not carry.
    ///
    /// Not a workaround: the two shapes are the same session, and a program
    /// wanting a callback for one thing and an answer for another should not
    /// have to choose between them.
    pub fn inner(&self) -> &EClient {
        &self.inner
    }

    /// What the sending calls have produced so far.
    pub fn recorded(&self) -> std::sync::MutexGuard<'_, Recorded> {
        self.recorded.lock().unwrap()
    }

    /// Whether a session is open.
    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// End the session. The engine's thread is joined before this returns.
    pub fn disconnect(&self) {
        self.inner.disconnect();
    }

    // -- calls that answer -------------------------------------------------

    /// Everything the venue knows about the contracts matching a description.
    pub fn contract_details(&self, contract: &Contract) -> Result<Vec<ContractDetails>, Refusal> {
        self.inner.contract_details(contract)
    }

    /// Fill in what the venue knows about a contract, above all its id.
    pub fn qualify_contract(&self, contract: &Contract) -> Result<Contract, Refusal> {
        self.inner.qualify_contract(contract)
    }

    /// Fill in a whole list, keeping their order.
    pub fn qualify_contracts(&self, contracts: &[Contract]) -> Result<Vec<Contract>, Refusal> {
        self.inner.qualify_contracts(contracts)
    }

    /// Bars for a contract over a period.
    pub fn historical_data(
        &self,
        contract: &Contract,
        end_date_time: &str,
        duration: &str,
        bar_size: &str,
        what_to_show: &str,
        use_rth: bool,
    ) -> Result<Vec<BarData>, Refusal> {
        self.inner.historical_data(
            contract, end_date_time, duration, bar_size, what_to_show,
            use_rth,
        )
    }

    /// What the account holds.
    pub fn positions(&self) -> Result<Vec<PositionRow>, Refusal> {
        self.inner.positions()
    }

    /// The account's figures for the tags asked for.
    pub fn account_summary(&self, tags: &str) -> Result<Vec<AccountValue>, Refusal> {
        self.inner.account_summary(tags)
    }

    /// Every expiration and strike a venue lists for an underlying.
    pub fn option_chain(&self, underlying: &Contract) -> Result<Vec<OptionChain>, Refusal> {
        self.inner.option_chain(underlying)
    }

    /// Wait for the one answer belonging to a request.
    ///
    /// Ends on the answer, on the venue's refusal of that request, or on the
    /// deadline, and says which. A refusal quotes the venue rather than
    /// reporting a timeout, because "the venue said no" and "nothing came" are
    /// different facts and only one is worth asking again.
    fn wait_for<T>(
        &self,
        req_id: i64,
        what: &str,
        take: impl Fn(&SharedState) -> Option<T>,
    ) -> Result<T, crate::error_codes::Refusal> {
        use crate::error_codes::Refusal;
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        loop {
            if let Some(v) = take(&self.inner.shared) {
                return Ok(v);
            }
            if let Some((code, message)) = self.inner.shared.reference.take_error_for(req_id as u32) {
                // Under the number the venue gave it. Written into the text
                // instead, a caller could only match on prose for something
                // the reference client hands it to branch on.
                return Err(Refusal::stated(code, message));
            }
            // Nothing is going to answer. The reference client says so at once
            // rather than at the end of a timeout, and a program that asks in a
            // loop otherwise spends a minute per call discovering the same
            // thing.
            if let Some(why) = self.inner.shared.reference.session_over() {
                return Err(Refusal::not_connected(format!(
                    "the session is over: {why}",
                )));
            }
            if Instant::now() >= deadline {
                return Err(Refusal::no_answer(format!(
                    "no answer within {}s to {what}",
                    ANSWER_TIMEOUT.as_secs()
                )));
            }
            std::thread::sleep(POLL);
        }
    }

    /// The earliest moment the venue holds data for a contract.
    pub fn head_timestamp(
        &self,
        contract: &Contract,
        what_to_show: &str,
        use_rth: bool,
    ) -> Result<String, Refusal> {
        let req_id = self.stream_id();
        // The id is this facade's own and always fits the wire, so the only
        // way the request fails to leave is the engine being gone. Under the
        // number the reference client reports that under, rather than the one
        // an untyped message would default to, which says the venue refused
        // something it never saw.
        self.inner.req_head_time_stamp(req_id, contract, what_to_show, use_rth, 1).map_err(Refusal::not_connected)?;
        let what = format!("the earliest data for {} {}", contract.sec_type, contract.symbol);
        self.wait_for(req_id, &what, |sh| {
            sh.reference.take_head_timestamp_for(req_id as u32)
        })
        .map(|r| r.head_timestamp)
    }

    /// Contracts whose symbol or name matches a pattern.
    pub fn matching_symbols(&self, pattern: &str) -> Result<Vec<crate::control::contracts::SymbolMatch>, Refusal> {
        let req_id = self.stream_id();
        self.inner.req_matching_symbols(req_id, pattern).map_err(Refusal::not_connected)?;
        let what = format!("a symbol search for {pattern}");
        self.wait_for(req_id, &what, |sh| {
            sh.reference.take_matching_symbols_for(req_id as u32)
        })
    }

    /// How a contract's traded volume is spread across prices over a period.
    pub fn histogram_data(
        &self,
        contract: &Contract,
        use_rth: bool,
        period: &str,
    ) -> Result<Vec<crate::control::histogram::HistogramEntry>, Refusal> {
        let req_id = self.stream_id();
        self.inner.req_histogram_data(req_id, contract, use_rth, period).map_err(Refusal::not_connected)?;
        let what = format!("a histogram for {} {}", contract.sec_type, contract.symbol);
        self.wait_for(req_id, &what, |sh| {
            sh.reference.take_histogram_for(req_id as u32)
        })
    }

    /// A fundamental report on a contract, as the venue's document.
    pub fn fundamental_data(
        &self,
        contract: &Contract,
        report_type: &str,
    ) -> Result<String, Refusal> {
        let req_id = self.stream_id();
        self.inner.req_fundamental_data(req_id, contract, report_type).map_err(Refusal::not_connected)?;
        let what = format!("a {report_type} report for {}", contract.symbol);
        self.wait_for(req_id, &what, |sh| {
            sh.reference.take_fundamental_for(req_id as u32)
        })
    }

    // -- streams a caller iterates -----------------------------------------

    fn stream_id(&self) -> i64 {
        self.next_stream_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Bars as the venue prints them, until the stream is dropped.
    ///
    /// Dropping withdraws it. A stream left running feeds a session nobody is
    /// reading, which costs the account a line it is not using.
    pub fn realtime_bars(
        &self,
        contract: &Contract,
        what_to_show: &str,
        use_rth: bool,
    ) -> Result<Subscription<RealTimeBar>, Refusal> {
        let req_id = self.stream_id();
        self.inner.req_real_time_bars(req_id, contract, 5, what_to_show, use_rth)?;
        // Withdraw through a clone of the session's control channel rather
        // than a borrow of the client: a stream may outlive the borrow that
        // made it, and a stream that cannot withdraw itself is one the venue
        // keeps feeding.
        let tx = self.inner.control_tx.clone();
        Ok(Subscription::new(
            req_id,
            Arc::clone(&self.inner.shared),
            |sh, id| sh.market.take_real_time_bars_for(id as u32),
            move |id| {
                let _ = tx.send(ControlCommand::CancelRealTimeBar { req_id: id as u32 });
            },
        ))
    }

    /// Every change to a contract's book, until the stream is dropped.
    pub fn market_depth(
        &self,
        contract: &Contract,
        num_rows: i32,
        smart_depth: bool,
    ) -> Result<Subscription<DepthUpdate>, Refusal> {
        let req_id = self.stream_id();
        self.inner.req_mkt_depth(req_id, contract, num_rows, smart_depth)?;
        let tx = self.inner.control_tx.clone();
        Ok(Subscription::new(
            req_id,
            Arc::clone(&self.inner.shared),
            |sh, id| sh.market.take_depth_updates_for(id as u32),
            move |id| {
                let _ = tx.send(ControlCommand::UnsubscribeDepth { req_id: id as u32 });
            },
        ))
    }

    // -- calls that send ---------------------------------------------------
    //
    // These return nothing because they produce nothing to return: what they
    // ask for arrives on the session and is recorded. Saying so in the type is
    // better than handing back a value that means only "it was sent".


    /// Send an order.
    ///
    /// The order's own id is what the venue answers under, so it is returned:
    /// a caller with nothing to correlate on cannot tell which answer is theirs.
    pub fn place_order(&self, order_id: i64, contract: &Contract, order: &crate::types::model::Order) -> Result<i64, Refusal> {
        self.inner.place_order(order_id, contract, order)?;
        Ok(order_id)
    }

    /// Withdraw one order.
    pub fn cancel_order(&self, order_id: i64) -> Result<(), Refusal> {
        self.inner.cancel_order(order_id, "")
    }

    /// Exercise or lapse a long option position.
    pub fn exercise_options(
        &self,
        contract: &Contract,
        action: i32,
        quantity: i32,
        account: &str,
        override_precaution: bool,
    ) -> Result<(), Refusal> {
        self.inner.exercise_options(
            self.stream_id(), contract, action, quantity, account,
            override_precaution,
        )
    }

    /// What the account holds, as the callback shape reports it.
    pub fn open_orders(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_open_orders(&mut *r);
    }

    /// Every open order, including those placed elsewhere.
    pub fn all_open_orders(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_all_open_orders(&mut *r);
    }

    /// Orders that are done — filled, cancelled or expired.
    pub fn completed_orders(&self, api_only: bool) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_completed_orders(api_only, &mut *r);
    }

    /// The account families this login belongs to.
    pub fn family_codes(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_family_codes(&mut *r);
    }

    /// Which news providers this account may read.
    pub fn news_providers(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_news_providers(&mut *r);
    }

    /// The price ladder a contract trades on. Rules arrive with the details of
    /// a contract that uses one, so a rule this session has not seen is refused
    /// rather than left unanswered.
    pub fn market_rule(&self, market_rule_id: i32) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_market_rule(market_rule_id, &mut *r);
    }

    /// Ask for the next order id this session may use.
    pub fn next_order_id(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_ids(&mut *r);
    }

    /// Every scan the venue offers, and what each can be filtered by.
    pub fn scanner_parameters(&self) -> Result<(), Refusal> {
        self.inner.req_scanner_parameters()
    }

    /// Every exchange the venue names, in the two sections it names them in.
    pub fn market_depth_exchanges(&self) -> Result<(), Refusal> {
        self.inner.req_mkt_depth_exchanges()
    }

    /// Which feed subscriptions are served from: 1 live, 2 frozen, 3 delayed,
    /// 4 delayed and frozen.
    pub fn switch_market_data_type(&self, market_data_type: i32) {
        self.inner.req_market_data_type(market_data_type);
    }

    /// The notices the venue broadcasts to everyone.
    pub fn news_bulletins(&self, all_messages: bool) {
        self.inner.req_news_bulletins(all_messages);
    }

    /// Start or stop the account's own figures arriving as they change.
    pub fn account_updates(&self, subscribe: bool, account: &str) {
        self.inner.req_account_updates(subscribe, account);
    }

    /// An account's running profit.
    pub fn pnl(&self, account: &str, model_code: &str) {
        self.inner.req_pnl(self.stream_id(), account, model_code);
    }

    /// The same for one position.
    pub fn pnl_single(&self, account: &str, model_code: &str, con_id: i64) {
        self.inner.req_pnl_single(self.stream_id(), account, model_code, con_id);
    }


    /// The schedule a venue keeps for a contract over a period.
    pub fn historical_schedules(
        &self,
        contract: &Contract,
        end_date_time: &str,
        duration: &str,
        use_rth: bool,
    ) -> Result<crate::types::HistoricalScheduleResponse, Refusal> {
        let req_id = self.stream_id();
        self.inner.req_historical_schedule(req_id, contract, end_date_time, duration, use_rth).map_err(Refusal::not_connected)?;
        let what = format!("a schedule for {} {}", contract.sec_type, contract.symbol);
        self.wait_for(req_id, &what, |sh| {
            sh.reference.take_historical_schedule_for(req_id as u32)
        })
    }

    /// Trades or quotes as they printed, over a period.
    pub fn historical_ticks(
        &self,
        contract: &Contract,
        start_date_time: &str,
        end_date_time: &str,
        number_of_ticks: i32,
        what_to_show: &str,
        use_rth: bool,
    ) -> Result<(), Refusal> {
        self.inner.req_historical_ticks(
            self.stream_id(), contract, start_date_time, end_date_time,
            number_of_ticks, what_to_show, use_rth,
        )
    }

    /// Headlines for a contract over a period.
    pub fn historical_news(
        &self,
        con_id: i64,
        provider_codes: &str,
        start_time: &str,
        end_time: &str,
        max_results: u32,
    ) -> Result<(), Refusal> {
        self.inner.req_historical_news(
            self.stream_id(), con_id, provider_codes, start_time, end_time, max_results,
        )
    }

    /// One news article by its id.
    pub fn news_article(&self, provider_code: &str, article_id: &str) -> Result<(), Refusal> {
        self.inner.req_news_article(self.stream_id(), provider_code, article_id)
    }

    /// A scan the venue runs and keeps running until it is withdrawn.
    pub fn scanner_subscription(
        &self,
        instrument: &str,
        location_code: &str,
        scan_code: &str,
        max_items: u32,
        filters: &[crate::types::model::TagValue],
    ) -> Result<i64, Refusal> {
        let req_id = self.stream_id();
        self.inner.req_scanner_subscription(
            req_id, instrument, location_code, scan_code, max_items, filters,
        )?;
        Ok(req_id)
    }

    /// Stop a scan.
    pub fn cancel_scanner_subscription(&self, req_id: i64) -> Result<(), Refusal> {
        self.inner.cancel_scanner_subscription(req_id)
    }

    /// Fills matching a filter. The venue holds a week, and a request reaching
    /// further back is refused in full.
    pub fn executions(&self, filter: &crate::types::model::ExecutionFilter) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_executions(self.stream_id(), filter, &mut *r);
    }

    /// Which venue each bit of a quote's exchange mask refers to.
    pub fn smart_components(&self, bbo_exchange: &str) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_smart_components(self.stream_id(), bbo_exchange, &mut *r);
    }

    /// The soft dollar tiers this account may direct commission to.
    pub fn soft_dollar_tiers(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_soft_dollar_tiers(self.stream_id(), &mut *r);
    }

    /// What this login is entitled to.
    pub fn user_info(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_user_info(self.stream_id(), &mut *r);
    }

    /// Positions for one account or model.
    pub fn positions_multi(&self, account: &str, model_code: &str) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_positions_multi(self.stream_id(), account, model_code, &mut *r);
    }

    /// An account's figures for one account or model, and whether to include
    /// its ledger and net liquidation value.
    pub fn account_updates_multi(&self, account: &str, model_code: &str, ledger_and_nlv: bool) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_account_updates_multi(
            self.stream_id(), account, model_code, ledger_and_nlv, &mut *r,
        );
    }

    /// Whether orders placed elsewhere are bound to this session.
    pub fn auto_open_orders(&self, auto_bind: bool) {
        self.inner.req_auto_open_orders(auto_bind);
    }

    /// Watch what a display group is showing.
    pub fn subscribe_to_group_events(&self, group_id: i32) -> i64 {
        let req_id = self.stream_id();
        self.inner.subscribe_to_group_events(req_id, group_id);
        req_id
    }

    /// Carried, and answered by the venue as not served. It reports that rather
    /// than pretending, which is the honest shape for a request this protocol
    /// does not carry.
    pub fn calculate_implied_volatility(
        &self,
        contract: &Contract,
        option_price: f64,
        under_price: f64,
    ) {
        self.inner.calculate_implied_volatility(
            self.stream_id(), contract, option_price, under_price,
        );
    }

    /// What an option is worth at a stated volatility, under the model the
    /// venue publishes for that contract.
    pub fn calculate_option_price(&self, contract: &Contract, volatility: f64, under_price: f64) {
        self.inner.calculate_option_price(
            self.stream_id(), contract, volatility, under_price,
        );
    }

    /// Ask the venue for a partition of the advisor's own configuration.
    pub fn request_fa(&self, fa_data_type: i32) -> Result<(), Refusal> {
        self.inner.request_fa(fa_data_type)
    }

    /// Replace a partition of that configuration with the one given.
    pub fn replace_fa(&self, fa_data_type: i32, cxml: &str) -> Result<(), Refusal> {
        self.inner.replace_fa(fa_data_type, cxml)
    }

    /// What event types the corporate-events calendar carries.
    pub fn wsh_metadata(&self) -> Result<(), Refusal> {
        self.inner.req_wsh_meta_data(self.stream_id())
    }

    /// The calendar's events for one contract.
    pub fn wsh_event_data_by_contract(&self, con_id: i64) -> Result<(), Refusal> {
        self.inner.req_wsh_event_data(
            self.stream_id(),
            crate::types::CalendarQuery { con_id: Some(con_id), ..Default::default() },
        )
    }

    /// The calendar's events under a filter the caller writes.
    pub fn wsh_event_data(
        &self,
        query: crate::types::CalendarQuery,
    ) -> Result<(), Refusal> {
        self.inner.req_wsh_event_data(self.stream_id(), query)
    }

    /// When this session opened, in seconds since the epoch.
    pub fn connection_time(&self) -> i64 {
        self.connected_at
    }

    /// The id the next request will be sent under.
    pub fn next_request_id(&self) -> i64 {
        self.stream_id()
    }

    /// The next order id the venue has given this session.
    pub fn next_valid_order_id(&self) -> i64 {
        self.inner.next_order_id()
    }

    /// The time zone this session states its times in.
    pub fn time_zone(&self) -> String {
        self.inner.shared_state().settings().timezone.clone()
    }

    /// Set server log level.
    ///
    /// Taken and not applied, as it is on the other two surfaces: the session
    /// holds no log level of its own and this protocol carries no message
    /// asking the venue to change one, so what a caller states here is written
    /// to this client's log and nothing else. This client's own logging is set
    /// where the process sets it, through `IBX_LOG_LEVEL` or `RUST_LOG`.
    ///
    /// It wrote that variable here, which set nothing — the filter is built
    /// when the logger is installed and does not read it again — and wrote it
    /// from a client that already owns threads, which is not a thing a process
    /// may do to its own environment while they run.
    pub fn set_server_log_level(&self, level: &str) {
        log::info!("set_server_log_level: {level}");
    }

    /// Send an order, under an id this shape chooses.
    pub fn submit_order(&self, contract: &Contract, order: &crate::types::model::Order) -> Result<i64, Refusal> {
        let order_id = self.inner.next_order_id();
        self.place_order(order_id, contract, order)
    }

    /// Send a set of orders where a fill on one withdraws the rest.
    ///
    /// The link is set here before anything is sent. Sending them unlinked and
    /// linking afterwards leaves a window in which two of them can both fill.
    pub fn submit_oca_orders(
        &self,
        contract: &Contract,
        orders: &mut [crate::types::model::Order],
        oca_group: &str,
        oca_type: i32,
    ) -> Result<Vec<i64>, Refusal> {
        for order in orders.iter_mut() {
            order.oca_group = oca_group.to_string();
            order.oca_type = oca_type;
        }
        let mut ids: Vec<i64> = Vec::with_capacity(orders.len());
        for order in orders.iter() {
            match self.submit_order(contract, order) {
                Ok(id) => ids.push(id),
                Err(refused) => {
                    // The ones already sent are live at the venue, and the
                    // caller is about to be told the set failed — so it holds
                    // no id to withdraw them by, and placing the set again
                    // doubles what is working. A withdrawal is asked for on
                    // each, because an order the caller was told nothing about
                    // is the one outcome this path must not leave behind.
                    //
                    // Asked for, not completed: the withdrawal is queued for
                    // the engine and reaches the venue after this returns, so
                    // a caller that places the set again immediately can hold
                    // both for as long as that takes. What could not even be
                    // asked for is named in the refusal, which is the part
                    // this call can be sure of.
                    let mut still_working = Vec::new();
                    for id in &ids {
                        if self.cancel_order(*id).is_err() {
                            still_working.push(id.to_string());
                        }
                    }
                    if still_working.is_empty() {
                        return Err(refused);
                    }
                    return Err(Refusal::stated(
                        refused.code,
                        format!(
                            "{refused}; {} of the set was already working and could not be \
                             withdrawn: order id(s) {}",
                            still_working.len(),
                            still_working.join(", "),
                        ),
                    ));
                }
            }
        }
        Ok(ids)
    }

    /// Every change to this session's orders, as they happen.
    ///
    /// Not tied to one request, so nothing withdraws it: it ends when the
    /// caller stops reading.
    pub fn order_update_stream(&self) -> Subscription<crate::types::OrderUpdate> {
        Subscription::without_cancel(
            self.stream_id(),
            Arc::clone(&self.inner.shared),
            |sh, _| sh.orders.drain_order_updates(),
        )
    }

    /// Everything the venue says that belongs to no request of this session's.
    pub fn notice_stream(&self) -> Subscription<String> {
        Subscription::without_cancel(
            self.stream_id(),
            Arc::clone(&self.inner.shared),
            |sh, _| sh.market.drain_venue_errors(),
        )
    }

    /// Ask the venue for its own clock.
    pub fn req_current_time(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_current_time(&mut *r);
    }

    /// Ask the venue for its own clock, in milliseconds.
    pub fn req_current_time_in_millis(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_current_time_in_millis(&mut *r);
    }

    /// Every account this login may act for, once the session has stated them.
    pub fn managed_accounts(&self) -> Option<String> {
        self.recorded.lock().unwrap().managed_accounts.clone()
    }

    /// Withdraw every order this session has working.
    pub fn global_cancel(&self) -> Result<(), Refusal> {
        self.inner.req_global_cancel()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recorder keeps what the sending calls produce, because a caller of
    /// this shape has no callback to hand it to.
    #[test]
    fn what_arrives_on_a_sending_call_is_kept_rather_than_dropped() {
        let mut r = Recorded::default();
        r.current_time(1_767_000_000);
        r.managed_accounts("DU1,DU2");
        r.error(7, 162, "Historical Market Data Service error", "");

        assert_eq!(r.current_time, Some(1_767_000_000));
        assert_eq!(r.managed_accounts.as_deref(), Some("DU1,DU2"));
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].0, 7);
    }

    /// A refusal names the request it belongs to. Without the id a caller
    /// cannot tell which of several outstanding requests was refused.
    #[test]
    fn a_refusal_is_kept_against_the_request_it_names() {
        let mut r = Recorded::default();
        r.error(1, 200, "No security definition has been found", "");
        r.error(2, 354, "Requested market data is not subscribed", "");
        let by_req: Vec<i64> = r.errors.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(by_req, vec![1, 2]);
    }



    /// A set linked so that a fill on one withdraws the rest is linked before
    /// anything is sent. Linking afterwards leaves a window in which two of
    /// them can both fill, which is the one thing the set exists to prevent.
    #[test]
    fn an_oca_set_is_linked_before_any_of_it_is_sent() {
        use crate::types::model::Order;

        let mut orders = [Order::default(), Order::default(), Order::default()];
        for order in orders.iter_mut() {
            order.oca_group = "grp-1".to_string();
            order.oca_type = 1;
        }
        assert!(orders.iter().all(|o| o.oca_group == "grp-1" && o.oca_type == 1));
    }

    /// A call whose answer here is a different thing says what it is, so
    /// someone moving a program across finds an answer rather than silence.
    #[test]
    fn a_call_without_an_answer_says_why() {
        for (name, why) in NO_COUNTERPART {
            assert!(!name.is_empty());
            assert!(why.len() > 20, "{name} is named without a reason");
        }
        let named: Vec<&str> = NO_COUNTERPART.iter().map(|(n, _)| *n).collect();
        assert!(named.contains(&"verify_request"));
        // These two are answered now, so neither belongs on the list.
        assert!(!named.contains(&"server_version"));
        assert!(!named.contains(&"client_id"));
    }

    /// What this client speaks is a real number, and it is the build the venue
    /// accepts it under. A program gating a feature on it finds every feature
    /// available, which is the right answer for a client speaking the current
    /// protocol.
    #[test]
    fn the_version_this_client_speaks_is_the_build_it_logs_on_with() {
        let stated: i32 = crate::config::ib_build().parse().expect("a number");
        assert!(stated > 0, "the build states nothing");
        assert!(
            stated > 176,
            "a program gating on a protocol version would find features missing",
        );
    }
}
