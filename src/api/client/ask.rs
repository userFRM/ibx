//! Calls that answer.
//!
//! The rest of this client follows the reference client's shape: a request
//! goes out under an id and the answer arrives later on a callback, which is
//! the right shape for a program with its own event loop. It is a poor shape
//! for asking one question. These ask, wait, and hand back the answer.
//!
//! They drive `process_msgs` themselves, so a caller already pumping it from
//! another thread should keep using the callbacks: the two would compete for
//! the same events.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use crate::api::types::{BarData, ContractDetails};
use crate::api::wrapper::Wrapper;

use super::{Contract, EClient};

/// How long a question waits for its answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

/// Ids for questions this layer asks on the caller's behalf. Far above what a
/// caller is likely to use, so an answer to one of these is never mistaken for
/// an answer to theirs.
static NEXT_ASK_ID: AtomicI64 = AtomicI64::new(crate::bridge::ReferenceState::ASK_ID_BASE as i64);

fn ask_id() -> i64 {
    NEXT_ASK_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Default)]
struct Answer {
    details: Vec<ContractDetails>,
    error: Option<String>,
    done: bool,
}

struct Collector {
    req_id: i64,
    answer: Arc<Mutex<Answer>>,
}

impl Wrapper for Collector {
    fn contract_details(&mut self, req_id: i64, details: &ContractDetails) {
        if req_id == self.req_id {
            self.answer.lock().unwrap().details.push(details.clone());
        }
    }
    fn contract_details_end(&mut self, req_id: i64) {
        if req_id == self.req_id {
            self.answer.lock().unwrap().done = true;
        }
    }
    fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
        // The connection notices are not answers to anything.
        if matches!(code, 2104 | 2106 | 2107 | 2119 | 2158) {
            return;
        }
        if req_id == self.req_id {
            let mut a = self.answer.lock().unwrap();
            a.error = Some(format!("{code}: {message}"));
            a.done = true;
        }
    }
}

/// What a venue lists for options on one underlying.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionChain {
    /// The venue listing them.
    pub exchange: String,
    /// The contract the options are on.
    pub underlying_con_id: i64,
    /// Which class of the chain these strikes belong to.
    pub trading_class: String,
    /// How many units one contract of it is worth.
    pub multiplier: String,
    /// Every expiry this venue lists.
    pub expirations: Vec<String>,
    /// Every strike it lists.
    pub strikes: Vec<f64>,
}

/// One headline the venue holds, and what an article request needs to read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headline {
    pub time: String,
    pub provider_code: String,
    pub article_id: String,
    pub headline: String,
}

/// A holding, as the account states it.
#[derive(Debug, Clone)]
pub struct PositionRow {
    /// Which account it is on.
    pub account: String,
    /// What is held.
    pub contract: Contract,
    /// How much of it.
    pub position: f64,
    /// What it cost on average.
    pub avg_cost: f64,
}

/// One value the account states about itself.
#[derive(Debug, Clone)]
pub struct AccountValue {
    /// Which account it is on.
    pub account: String,
    /// Which figure this is.
    pub tag: String,
    /// What the venue states it as.
    pub value: String,
    /// What currency it is stated in.
    pub currency: String,
}

/// Where an order stands.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderReport {
    /// The order this is about.
    pub order_id: i64,
    /// The venue's own word for it: `Filled`, `Cancelled`, `Inactive`, and so on.
    pub status: String,
    /// How much has filled.
    pub filled: f64,
    /// How much has not.
    pub remaining: f64,
    /// What it has paid on average, zero until something fills.
    pub avg_price: f64,
    /// Why the venue would not work it, where it said.
    pub reason: Option<String>,
}

impl OrderReport {
    /// Whether the venue has finished with this order, one way or another.
    pub fn is_done(&self) -> bool {
        matches!(self.status.as_str(), "Filled" | "Cancelled" | "ApiCancelled" | "Inactive")
    }

    /// Whether it filled in full.
    pub fn is_filled(&self) -> bool {
        self.status == "Filled" && self.remaining == 0.0
    }
}

/// One question's answer as it accumulates.
struct Pending<T> {
    rows: Vec<T>,
    error: Option<String>,
    done: bool,
}

impl<T> Default for Pending<T> {
    fn default() -> Self {
        Self { rows: Vec::new(), error: None, done: false }
    }
}

/// The connection notices are not answers to anything.
fn is_connection_notice(code: i64) -> bool {
    matches!(code, 2104 | 2106 | 2107 | 2119 | 2158)
}

impl EClient {
    /// Pump until the collector says the answer is complete, or time runs out.
    fn wait_for<T, W: Wrapper>(
        &self, collector: &mut W, state: &Arc<Mutex<Pending<T>>>, what: &str,
    ) -> Result<Vec<T>, String> {
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        while Instant::now() < deadline {
            self.process_msgs(collector);
            if state.lock().unwrap().done {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut s = state.lock().unwrap();
        if let Some(e) = s.error.take() {
            return Err(e);
        }
        if !s.done {
            return Err(format!("no answer within {}s to {what}", ANSWER_TIMEOUT.as_secs()));
        }
        Ok(std::mem::take(&mut s.rows))
    }

    /// Bars for a contract, as `req_historical_data` asks for them.
    pub fn historical_data(
        &self, contract: &Contract, end_date_time: &str, duration: &str,
        bar_size: &str, what_to_show: &str, use_rth: bool,
    ) -> Result<Vec<BarData>, String> {
        struct Bars { req_id: i64, state: Arc<Mutex<Pending<BarData>>> }
        impl Wrapper for Bars {
            fn historical_data(&mut self, req_id: i64, bar: &BarData) {
                if req_id == self.req_id {
                    self.state.lock().unwrap().rows.push(bar.clone());
                }
            }
            fn historical_data_end(&mut self, req_id: i64, _: &str, _: &str) {
                if req_id == self.req_id {
                    self.state.lock().unwrap().done = true;
                }
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id == self.req_id && !is_connection_notice(code) {
                    let mut s = self.state.lock().unwrap();
                    s.error = Some(format!("{code}: {message}"));
                    s.done = true;
                }
            }
        }
        let req_id = ask_id();
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Bars { req_id, state: Arc::clone(&state) };
        self.req_historical_data(
            req_id, contract, end_date_time, duration, bar_size, what_to_show, use_rth, 1, false,
        )?;
        self.wait_for(&mut collector, &state, &format!("{duration} of bars for {}", contract.symbol))
    }

    /// Every expiration and strike each venue lists for an underlying.
    ///
    /// `underlying` must carry the id of the contract the options are on — the
    /// stock, not the option — which `qualify_contract` supplies.
    pub fn option_chain(
        &self, underlying: &Contract,
    ) -> Result<Vec<OptionChain>, String> {
        struct Chain { req_id: i64, state: Arc<Mutex<Pending<OptionChain>>> }
        impl Wrapper for Chain {
            fn security_definition_option_parameter(
                &mut self, req_id: i64, exchange: &str, underlying_con_id: i64,
                trading_class: &str, multiplier: &str, expirations: &[String], strikes: &[f64],
            ) {
                if req_id == self.req_id {
                    self.state.lock().unwrap().rows.push(OptionChain {
                        exchange: exchange.to_string(),
                        underlying_con_id,
                        trading_class: trading_class.to_string(),
                        multiplier: multiplier.to_string(),
                        expirations: expirations.to_vec(),
                        strikes: strikes.to_vec(),
                    });
                }
            }
            fn security_definition_option_parameter_end(&mut self, req_id: i64) {
                if req_id == self.req_id {
                    self.state.lock().unwrap().done = true;
                }
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id == self.req_id && !is_connection_notice(code) {
                    let mut s = self.state.lock().unwrap();
                    s.error = Some(format!("{code}: {message}"));
                    s.done = true;
                }
            }
        }
        if underlying.con_id == 0 {
            return Err(format!(
                "the chain is asked for by the id of the contract the options are on, and {} \
                 carries none: qualify it first",
                underlying.symbol,
            ));
        }
        let req_id = ask_id();
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Chain { req_id, state: Arc::clone(&state) };
        self.req_sec_def_opt_params(
            req_id, &underlying.symbol, "", &underlying.sec_type, underlying.con_id,
        )?;
        self.wait_for(&mut collector, &state, &format!("the option chain on {}", underlying.symbol))
    }

    /// The earliest moment the venue holds data for a contract.
    ///
    /// The same question `req_head_time_stamp` asks.
    pub fn head_timestamp(
        &self, contract: &Contract, what_to_show: &str, use_rth: bool,
    ) -> Result<String, String> {
        struct Head { req_id: i64, state: Arc<Mutex<Pending<String>>> }
        impl Wrapper for Head {
            fn head_timestamp(&mut self, req_id: i64, head_timestamp: &str) {
                if req_id == self.req_id {
                    let mut s = self.state.lock().unwrap();
                    s.rows.push(head_timestamp.to_string());
                    s.done = true;
                }
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id == self.req_id && !is_connection_notice(code) {
                    let mut s = self.state.lock().unwrap();
                    s.error = Some(format!("{code}: {message}"));
                    s.done = true;
                }
            }
        }
        let req_id = ask_id();
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Head { req_id, state: Arc::clone(&state) };
        self.req_head_time_stamp(req_id, contract, what_to_show, use_rth, 1)?;
        let what = format!("the first data the venue holds for {}", contract.symbol);
        Ok(self.wait_for(&mut collector, &state, &what)?.remove(0))
    }

    /// Contracts whose name or symbol matches a pattern.
    pub fn matching_symbols(
        &self, pattern: &str,
    ) -> Result<Vec<crate::api::types::ContractDescription>, String> {
        struct Matches {
            req_id: i64,
            state: Arc<Mutex<Pending<crate::api::types::ContractDescription>>>,
        }
        impl Wrapper for Matches {
            fn symbol_samples(
                &mut self, req_id: i64, descriptions: &[crate::api::types::ContractDescription],
            ) {
                if req_id == self.req_id {
                    let mut s = self.state.lock().unwrap();
                    s.rows.extend(descriptions.iter().cloned());
                    s.done = true;
                }
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id == self.req_id && !is_connection_notice(code) {
                    let mut s = self.state.lock().unwrap();
                    s.error = Some(format!("{code}: {message}"));
                    s.done = true;
                }
            }
        }
        let req_id = ask_id();
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Matches { req_id, state: Arc::clone(&state) };
        self.req_matching_symbols(req_id, pattern)?;
        self.wait_for(&mut collector, &state, &format!("a search for {pattern}"))
    }

    /// The headlines the venue holds for a contract.
    ///
    /// Each is the time, the provider's code, the article's id and the
    /// headline itself. Reading an article needs the first two.
    pub fn news_headlines(
        &self, con_id: i64, provider_codes: &str,
        start_date_time: &str, end_date_time: &str, total_results: i32,
    ) -> Result<Vec<Headline>, String> {
        struct Headlines { req_id: i64, state: Arc<Mutex<Pending<Headline>>> }
        impl Wrapper for Headlines {
            fn historical_news(
                &mut self, req_id: i64, time: &str, provider_code: &str,
                article_id: &str, headline: &str,
            ) {
                if req_id == self.req_id {
                    self.state.lock().unwrap().rows.push(Headline {
                        time: time.to_string(),
                        provider_code: provider_code.to_string(),
                        article_id: article_id.to_string(),
                        headline: headline.to_string(),
                    });
                }
            }
            fn historical_news_end(&mut self, req_id: i64, _has_more: bool) {
                if req_id == self.req_id {
                    self.state.lock().unwrap().done = true;
                }
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id == self.req_id && !is_connection_notice(code) {
                    let mut s = self.state.lock().unwrap();
                    s.error = Some(format!("{code}: {message}"));
                    s.done = true;
                }
            }
        }
        let req_id = ask_id();
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Headlines { req_id, state: Arc::clone(&state) };
        self.req_historical_news(
            req_id, con_id, provider_codes, start_date_time, end_date_time,
            total_results as u32,
        )?;
        self.wait_for(&mut collector, &state, &format!("headlines for contract {con_id}"))
    }

    /// How a contract's trades were spread across prices.
    pub fn histogram_data(
        &self, contract: &Contract, use_rth: bool, period: &str,
    ) -> Result<Vec<(f64, i64)>, String> {
        struct Histogram { req_id: i64, state: Arc<Mutex<Pending<(f64, i64)>>> }
        impl Wrapper for Histogram {
            fn histogram_data(&mut self, req_id: i64, items: &[(f64, i64)]) {
                if req_id == self.req_id {
                    let mut s = self.state.lock().unwrap();
                    s.rows.extend(items.iter().copied());
                    s.done = true;
                }
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id == self.req_id && !is_connection_notice(code) {
                    let mut s = self.state.lock().unwrap();
                    s.error = Some(format!("{code}: {message}"));
                    s.done = true;
                }
            }
        }
        let req_id = ask_id();
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Histogram { req_id, state: Arc::clone(&state) };
        self.req_histogram_data(req_id, contract, use_rth, period)?;
        let what = format!("how {} traded across prices", contract.symbol);
        self.wait_for(&mut collector, &state, &what)
    }

    /// A fundamental document about a contract, as the venue writes it.
    pub fn fundamental_data(
        &self, contract: &Contract, report_type: &str,
    ) -> Result<String, String> {
        struct Document { req_id: i64, state: Arc<Mutex<Pending<String>>> }
        impl Wrapper for Document {
            fn fundamental_data(&mut self, req_id: i64, data: &str) {
                if req_id == self.req_id {
                    let mut s = self.state.lock().unwrap();
                    s.rows.push(data.to_string());
                    s.done = true;
                }
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id == self.req_id && !is_connection_notice(code) {
                    let mut s = self.state.lock().unwrap();
                    s.error = Some(format!("{code}: {message}"));
                    s.done = true;
                }
            }
        }
        let req_id = ask_id();
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Document { req_id, state: Arc::clone(&state) };
        self.req_fundamental_data(req_id, contract, report_type)?;
        let what = format!("a {report_type} for {}", contract.symbol);
        Ok(self.wait_for(&mut collector, &state, &what)?.remove(0))
    }

    /// What the venue says an order would cost, without placing it.
    ///
    /// The order is marked as a question rather than an instruction, so
    /// nothing reaches the market.
    pub fn what_if_order(
        &self, contract: &Contract, order: &crate::api::types::Order,
    ) -> Result<crate::api::types::OrderState, String> {
        struct Preview {
            order_id: i64,
            state: Arc<Mutex<Pending<crate::api::types::OrderState>>>,
        }
        impl Wrapper for Preview {
            fn open_order(
                &mut self, order_id: i64, _c: &Contract, _o: &crate::api::types::Order,
                order_state: &crate::api::types::OrderState,
            ) {
                if order_id == self.order_id {
                    let mut s = self.state.lock().unwrap();
                    s.rows.push(order_state.clone());
                    s.done = true;
                }
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id == self.order_id && !is_connection_notice(code) {
                    let mut s = self.state.lock().unwrap();
                    s.error = Some(format!("{code}: {message}"));
                    s.done = true;
                }
            }
        }
        let order_id = ask_id();
        let asked = crate::api::types::Order { what_if: true, ..order.clone() };
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Preview { order_id, state: Arc::clone(&state) };
        self.place_order(order_id, contract, &asked)?;
        let what = format!("a preview of {} {} {}", asked.action, asked.total_quantity, contract.symbol);
        Ok(self.wait_for(&mut collector, &state, &what)?.remove(0))
    }

    /// Every holding in the account.
    pub fn positions(&self) -> Result<Vec<PositionRow>, String> {
        struct Held { state: Arc<Mutex<Pending<PositionRow>>> }
        impl Wrapper for Held {
            fn position(&mut self, account: &str, contract: &Contract, position: f64, avg_cost: f64) {
                self.state.lock().unwrap().rows.push(PositionRow {
                    account: account.to_string(),
                    contract: contract.clone(),
                    position,
                    avg_cost,
                });
            }
            fn position_end(&mut self) {
                self.state.lock().unwrap().done = true;
            }
        }
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Held { state: Arc::clone(&state) };
        self.req_positions(&mut collector);
        self.wait_for(&mut collector, &state, "the account's holdings")
    }

    /// The account values named by `tags`, as `req_account_summary` asks for
    /// them. `tags` is a comma-separated list, or `All`.
    pub fn account_summary(&self, tags: &str) -> Result<Vec<AccountValue>, String> {
        struct Values { req_id: i64, state: Arc<Mutex<Pending<AccountValue>>> }
        impl Wrapper for Values {
            fn account_summary(&mut self, req_id: i64, account: &str, tag: &str, value: &str, currency: &str) {
                if req_id == self.req_id {
                    self.state.lock().unwrap().rows.push(AccountValue {
                        account: account.to_string(),
                        tag: tag.to_string(),
                        value: value.to_string(),
                        currency: currency.to_string(),
                    });
                }
            }
            fn account_summary_end(&mut self, req_id: i64) {
                if req_id == self.req_id {
                    self.state.lock().unwrap().done = true;
                }
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id == self.req_id && !is_connection_notice(code) {
                    let mut s = self.state.lock().unwrap();
                    s.error = Some(format!("{code}: {message}"));
                    s.done = true;
                }
            }
        }
        let req_id = ask_id();
        let state = Arc::new(Mutex::new(Pending::default()));
        let mut collector = Values { req_id, state: Arc::clone(&state) };
        self.req_account_summary(req_id, "All", tags);
        let rows = self.wait_for(&mut collector, &state, "the account summary");
        self.cancel_account_summary(req_id);
        rows
    }

    /// Wait for an order to reach a state the venue will not move it from.
    ///
    /// Placing an order says only that it was sent. What happened to it arrives
    /// later, on a callback, spread across a status and possibly a refusal.
    /// This waits for the venue to finish with it and reports where it landed —
    /// including the refusal, which is the part a caller most needs and the
    /// part most easily missed.
    ///
    /// A wait that runs out is not a failure of the order: it says only that
    /// the venue had not finished, and the order is still working.
    pub fn await_order(
        &self, order_id: i64, timeout: Duration,
    ) -> Result<OrderReport, String> {
        struct Watch { order_id: i64, report: Arc<Mutex<Option<OrderReport>>>, done: Arc<Mutex<bool>> }
        impl Wrapper for Watch {
            fn order_status(
                &mut self, order_id: i64, status: &str, filled: f64, remaining: f64,
                avg_price: f64, _: i64, _: i64, _: f64, _: i64, _: &str, _: f64,
            ) {
                if order_id != self.order_id {
                    return;
                }
                let mut r = self.report.lock().unwrap();
                let reason = r.as_ref().and_then(|p| p.reason.clone());
                let report = OrderReport {
                    order_id, status: status.to_string(), filled, remaining,
                    // A status arriving after a fill can state no average; the
                    // one already reported is the better answer.
                    avg_price: if avg_price > 0.0 {
                        avg_price
                    } else {
                        r.as_ref().map_or(0.0, |p| p.avg_price)
                    },
                    reason,
                };
                if report.is_done() {
                    *self.done.lock().unwrap() = true;
                }
                *r = Some(report);
            }
            fn error(&mut self, req_id: i64, code: i64, message: &str, _: &str) {
                if req_id != self.order_id || is_connection_notice(code) {
                    return;
                }
                let mut r = self.report.lock().unwrap();
                match r.as_mut() {
                    Some(report) => report.reason = Some(message.to_string()),
                    None => {
                        *r = Some(OrderReport {
                            order_id: req_id,
                            status: String::new(),
                            filled: 0.0,
                            remaining: 0.0,
                            avg_price: 0.0,
                            reason: Some(message.to_string()),
                        });
                    }
                }
            }
        }
        let report = Arc::new(Mutex::new(None));
        let done = Arc::new(Mutex::new(false));
        let mut watch = Watch {
            order_id,
            report: Arc::clone(&report),
            done: Arc::clone(&done),
        };
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.process_msgs(&mut watch);
            if *done.lock().unwrap() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        report.lock().unwrap().clone().ok_or_else(|| {
            format!("order {order_id} said nothing within {}s", timeout.as_secs())
        })
    }

    /// Every contract matching the one described.
    ///
    /// The same question `req_contract_details` asks, answered here instead of
    /// on a callback.
    pub fn contract_details(&self, contract: &Contract) -> Result<Vec<ContractDetails>, String> {
        let req_id = ask_id();
        let answer = Arc::new(Mutex::new(Answer::default()));
        let mut collector = Collector { req_id, answer: Arc::clone(&answer) };
        self.req_contract_details(req_id, contract)?;

        let deadline = Instant::now() + ANSWER_TIMEOUT;
        while Instant::now() < deadline {
            self.process_msgs(&mut collector);
            if answer.lock().unwrap().done {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut a = answer.lock().unwrap();
        if let Some(e) = a.error.take() {
            return Err(e);
        }
        if !a.done {
            return Err(format!(
                "no answer within {}s to a contract lookup for {} {}",
                ANSWER_TIMEOUT.as_secs(), contract.sec_type, contract.symbol,
            ));
        }
        Ok(std::mem::take(&mut a.details))
    }

    /// Fill in what the venue knows about a contract, above all its id.
    ///
    /// Most of what this client sends carries a contract, and a contract with
    /// an id is worth more than one without: market data is answered only for
    /// a contract named by id, and an order that carries one needs to state
    /// nothing else. Ask this first and pass the result around.
    ///
    /// A description matching more than one contract is refused rather than
    /// resolved to whichever came back first — the same symbol on the same
    /// venue exists in more than one currency, and picking one silently is how
    /// an order ends up on the wrong one.
    pub fn qualify_contract(&self, contract: &Contract) -> Result<Contract, String> {
        let mut found = self.contract_details(contract)?;
        match found.len() {
            0 => Err(format!(
                "no contract matches {} {} on {}",
                contract.sec_type, contract.symbol, contract.exchange,
            )),
            1 => Ok(found.remove(0).contract),
            n => {
                let mut how: Vec<String> = found.iter()
                    .take(4)
                    .map(|d| format!(
                        "{} {} {} {}",
                        d.contract.con_id, d.contract.local_symbol,
                        d.contract.currency, d.contract.exchange,
                    ))
                    .collect();
                if n > how.len() {
                    how.push(format!("and {} more", n - how.len()));
                }
                Err(format!(
                    "{} {} matches {n} contracts, so it names none: {}",
                    contract.sec_type, contract.symbol, how.join("; "),
                ))
            }
        }
    }

    /// Fill in a batch of contracts, keeping the caller's order.
    ///
    /// Stops at the first that cannot be named, because a caller building a
    /// basket wants to know which one is wrong, not to trade the rest.
    pub fn qualify_contracts(&self, contracts: &[Contract]) -> Result<Vec<Contract>, String> {
        contracts.iter().map(|c| self.qualify_contract(c)).collect()
    }
}
