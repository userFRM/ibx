//! Calls that answer.
//!
//! The reference client's shape is a request under an id and an answer later on
//! a callback, which suits a program with its own event loop and suits asking
//! one question badly. A caller who wants a contract's id has to send, register
//! a handler, pump, and correlate — for one value.
//!
//! These send, wait, and hand the answer back. They take their answers out of
//! the shared queues by request id, so a dispatch loop running beside them
//! keeps its own. They release the interpreter lock while waiting, so other
//! threads run.
//!
//! A caller pumping `run()` on the same client should keep to the callbacks:
//! `run()` drains every queue rather than its own, so the two compete.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use crate::error_codes::Refusal;
use pyo3::prelude::*;

use std::sync::Arc;

use crate::bridge::SharedState;

use super::EClient;
use super::super::contract::{BarData, Contract, ContractDescription, ContractDetails};

/// How long a question waits for its answer.
const ANSWER_TIMEOUT: Duration =
    Duration::from_secs(crate::config::ANSWER_TIMEOUT_SECS);

/// How long to sleep between looks at the queue. Short enough that a fast
/// answer is not made to wait on the poll, long enough not to spin a core.
const POLL: Duration = Duration::from_millis(5);

/// Ids for questions this layer asks on the caller's behalf.
///
/// Counted from a high number so they read apart from a caller's in a log, but
/// nothing depends on where they fall: each is recorded as this client's own
/// while its answer is outstanding, which is what keeps a caller's dispatch
/// from taking it.
static NEXT_ASK_ID: AtomicI64 = AtomicI64::new(crate::bridge::ReferenceState::ASK_ID_BASE as i64);

/// An id this layer asked a question under, held while the answer is
/// outstanding.
///
/// Recorded where it is handed out rather than where it is waited on: the
/// request goes out first, and an answer arriving before the wait began would
/// otherwise be taken by a caller's own dispatch. Released on drop, so a
/// question given up on stops being held.
pub(crate) struct AskId {
    id: i64,
    /// The session that is waiting, so releasing it releases it there and not
    /// on another session that happens to count from the same number.
    shared: std::sync::Arc<crate::bridge::SharedState>,
}

impl AskId {
    /// The number the question went out under.
    pub(crate) fn get(&self) -> i64 {
        self.id
    }
}

impl Drop for AskId {
    fn drop(&mut self) {
        self.shared.reference.forget_ours(self.id);
    }
}

fn ask_id(shared: &std::sync::Arc<crate::bridge::SharedState>) -> AskId {
    let id = NEXT_ASK_ID.fetch_add(1, Ordering::Relaxed);
    shared.reference.note_ours(id);
    AskId { id, shared: std::sync::Arc::clone(shared) }
}

/// The id the next question will be asked under, reserved.
///
/// Lets a test put an answer in place before the question is asked, which is
/// the only way to exercise the waiting without a venue on the other end.
/// Recorded as this client's own as it is handed back, because the answer is
/// seeded before the question allocates the id and a dispatch pass in between
/// would otherwise take it — which is the very thing such a test is written to
/// catch. Asking releases it in the ordinary way.
#[doc(hidden)]
#[cfg(feature = "test-helpers")]
pub(crate) fn peek_ask_id(shared: &std::sync::Arc<crate::bridge::SharedState>) -> i64 {
    let id = NEXT_ASK_ID.load(Ordering::Relaxed);
    shared.reference.note_ours(id);
    id
}


/// Wait for the one answer belonging to a request.
///
/// Stops on the answer, on the venue's refusal of that request, or on the
/// deadline — and says which. A refusal quotes the venue rather than reporting
/// a timeout, because "the venue said no" and "nothing came" are different
/// facts and only one of them is worth retrying.
fn wait_for<T>(
    py: Python<'_>,
    shared: &Arc<SharedState>,
    req_id: i64,
    what: &str,
    mut take: impl FnMut(&SharedState) -> Option<T> + Send,
) -> PyResult<T>
where
    T: Send,
{
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    py.detach(|| {
        loop {
            if let Some(v) = take(shared) {
                return Ok(v);
            }
            if let Some((code, msg)) = shared.reference.take_error_for(req_id as u32) {
                return Err(format!("{msg} ({code})"));
            }
            // Nothing is coming, so the caller hears it now rather than at the
            // end of a wait it pays once per call.
            if let Some(why) = shared.reference.session_over() {
                return Err(format!(
                    "the session is over: {why} ({})",
                    crate::error_codes::Refusal::NOT_CONNECTED,
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "no answer within {}s to {what}",
                    ANSWER_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }
    })
    .map_err(PyRuntimeError::new_err)
}

impl EClient {
    /// The shared state of a connected client, or a plain refusal.
    fn connected_shared(&self) -> PyResult<Arc<SharedState>> {
        self.shared
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| PyRuntimeError::new_err("not connected"))
    }

    /// A contract's corporate actions, asked for and waited on.
    ///
    /// The venue answers per contract, which says which contract an answer is
    /// about and not which question it answers, so the engine files it against
    /// the request that asked and this takes its own. Kept off the Python
    /// surface deliberately: it hands back this client's own types, and
    /// `corporate_actions` is what states them to a caller.
    fn actions_for(
        &self, py: Python<'_>, contract: &Contract, start_date: &str, end_date: &str,
    ) -> PyResult<Vec<crate::control::adjustments::Adjustment>> {
        if contract.con_id <= 0 {
            return Err(PyValueError::new_err(format!(
                "corporate actions are asked for by the venue's id for the contract, \
                 and {} is not one: qualify the contract first and pass what comes back",
                contract.con_id,
            )));
        }
        let shared = self.connected_shared()?;
        // One of these at a time per session. Each caller takes only the answer
        // to its own request, so a second caller cannot be handed the first's;
        // what serialising prevents is the two of them clearing and rewriting
        // the contract's own record around each other, which is what
        // `EClient::adjustments` reads.
        //
        // Waited for without the interpreter lock, so other threads keep
        // running while this one waits its turn. A mutex guard cannot cross
        // that boundary because it is not `Send`, so the turn is a flag and a
        // guard that puts it back.
        //
        // The flag belongs to the session, not to the process. Two clients hold
        // two sessions and two records, and one waiting on its own venue is no
        // reason for the other to wait at all.
        struct Turn(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Turn {
            fn drop(&mut self) {
                self.0.store(false, std::sync::atomic::Ordering::Release);
            }
        }
        let taken = Arc::clone(&shared.asking_adjustments);
        let _turn = py.detach(move || {
            while taken
                .compare_exchange(
                    false, true,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_err()
            {
                std::thread::sleep(POLL);
            }
            Turn(taken)
        });
        // The contract's own record is what `EClient::adjustments` reads, and
        // it is cleared here so that reader states this question's answer
        // rather than a previous one's. What this call waits on is its own
        // slot, which no other question can fill.
        shared.reference.forget_adjustments(&contract.con_id.to_string());
        let asked = ask_id(&shared);
        let req_id = asked.get();
        // Said before the request goes out, so an answer that arrives has
        // somewhere to be put. Nothing is filed for a request nobody said they
        // would wait on.
        //
        // Given up by a guard rather than by a line at the end, because the
        // send below can fail and return, and a slot left behind by a call that
        // never waited is one the session never reclaims.
        struct StopWaiting(Arc<SharedState>, u32);
        impl Drop for StopWaiting {
            fn drop(&mut self) {
                self.0.reference.stop_waiting_for_adjustments(self.1);
            }
        }
        shared.reference.expect_adjustments(req_id as u32);
        let _stop = StopWaiting(Arc::clone(&shared), req_id as u32);
        self.req_adjustments(
            py, req_id, contract.con_id, &contract.sec_type, &contract.exchange,
            start_date, end_date,
        )?;
        let what = format!("the corporate actions of {}", contract.symbol);
        // The answer to this request, not the last answer about this contract.
        // Reading the contract's own record here would hand a caller a late
        // answer to a question somebody else gave up on, over a range this one
        // never asked about.
        wait_for(py, &shared, req_id, &what, |sh| {
            sh.reference.take_adjustments_answering(req_id as u32)
        })
    }
}

/// The time zone a venue states its hours in, and each session as its
/// opening, its close, and the day it belongs to.
type TradingSchedule = (String, Vec<(String, String, String)>);

#[pymethods]
impl EClient {
    /// Everything the venue knows about the contracts matching a description.
    ///
    /// Sends the lookup, waits for the venue to say it has finished, and hands
    /// back every match. A description matching nothing returns an empty list;
    /// a venue that refuses the lookup raises with the reason it gave.
    fn contract_details(
        &self,
        py: Python<'_>,
        contract: &Contract,
    ) -> PyResult<Vec<ContractDetails>> {
        self.contract_details_stated(py, contract)
            .map_err(|refusal| PyRuntimeError::new_err(
                format!("{} ({})", refusal.message, refusal.code),
            ))
    }


    /// A contract's corporate actions, asked for and waited on.
    ///
    /// One dict per action, stating what the venue stated: its kind as the
    /// two-letter name the venue uses, the day it takes effect, its value, and
    /// the dates and dividend descriptions the kind carries. A field the kind
    /// does not carry is empty rather than invented.
    ///
    /// `contract` must carry the venue's id for it. Days are `YYYYMMDD`.
    #[pyo3(signature = (contract, start_date, end_date))]
    fn corporate_actions(
        &self, py: Python<'_>, contract: &Contract, start_date: &str, end_date: &str,
    ) -> PyResult<Vec<std::collections::BTreeMap<String, String>>> {
        Ok(self.actions_for(py, contract, start_date, end_date)?
            .into_iter()
            .map(|a| {
                [
                    ("kind", a.kind.map(|k| k.code()).unwrap_or("").to_string()),
                    ("date", a.date),
                    ("value", a.value),
                    ("currency", a.currency),
                    ("announce_date", a.announce_date),
                    ("record_date", a.record_date),
                    ("pay_date", a.pay_date),
                    ("payment_type", a.payment_type),
                    ("distribution_type", a.distribution_type),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect()
            })
            .collect())
    }

    /// Bars for a contract over a period, handed back rather than delivered a
    /// bar at a time to a callback.
    ///
    /// `ADJUSTED_LAST` is served here and refused by `reqHistoricalData`. The
    /// venue has no adjusted series to pass through: an adjusted one is built
    /// from the raw trades and the contract's actions, which means holding both
    /// before a bar is handed over. A call that waits can; one that answers on
    /// a callback would have to hand over raw bars under an adjusted name.
    #[pyo3(signature = (contract, end_date_time, duration_str, bar_size_setting, what_to_show, use_rth=1))]
    fn historical_data(
        &self,
        py: Python<'_>,
        contract: &Contract,
        end_date_time: &str,
        duration_str: &str,
        bar_size_setting: &str,
        what_to_show: &str,
        use_rth: i32,
    ) -> PyResult<Vec<BarData>> {
        if what_to_show.eq_ignore_ascii_case("ADJUSTED_LAST") {
            let bars = self.historical_data(
                py, contract, end_date_time, duration_str, bar_size_setting, "TRADES", use_rth,
            )?;
            let Some(first) = bars.first() else { return Ok(bars) };
            // From the first bar to today rather than to the last: a split
            // last month moves a series that ended last year.
            let from: String = first.date.chars().take(8).collect();
            let today: String = crate::protocol::datetime::chrono_free_timestamp()
                .chars().take(8).collect();
            let actions = self.actions_for(py, contract, &from, &today)?;
            if actions.is_empty() {
                return Ok(bars);
            }
            let raw = bars
                .iter()
                .map(|b| crate::types::model::BarData {
                    date: b.date.clone(), open: b.open, high: b.high, low: b.low,
                    close: b.close, volume: b.volume, wap: b.wap,
                    bar_count: b.bar_count, timezone: b.timezone.clone(),
                })
                .collect();
            let scaled = crate::control::adjustments::scale_bars(raw, &actions)
                .map_err(PyValueError::new_err)?;
            return Ok(scaled
                .into_iter()
                .map(|b| BarData::new(
                    b.date, b.open, b.high, b.low, b.close, b.volume, b.wap,
                    b.bar_count, b.timezone,
                ))
                .collect());
        }
        let shared = self.connected_shared()?;
        let asked = ask_id(&shared);
        let req_id = asked.get();
        self.req_historical_data(
            py, req_id, contract, end_date_time, duration_str, bar_size_setting,
            what_to_show, use_rth, 1, false, Vec::new(),
        )?;

        // The venue may answer in parts. Keep what each part carries and stop
        // on the one that says it is the last, rather than on the first: a
        // series cut at the first part is short and says nothing about it.
        let mut bars = Vec::new();
        let mut zone = String::new();
        let what = format!("a bar request for {} {}", contract.sec_type, contract.symbol);
        wait_for(py, &shared, req_id, &what, |sh| {
            for part in sh.reference.take_historical_for(req_id as u32) {
                if zone.is_empty() {
                    zone = part.timezone.clone();
                }
                let complete = part.is_complete;
                bars.extend(part.bars.iter().cloned());
                if complete {
                    return Some(());
                }
            }
            None
        })?;

        Ok(bars
            .into_iter()
            .map(|b| {
                BarData::new(
                    b.time, b.open, b.high, b.low, b.close, b.volume, b.wap,
                    b.count as i32, zone.clone(),
                )
            })
            .collect())
    }

    /// The earliest moment the venue holds data for a contract.
    #[pyo3(signature = (contract, what_to_show="TRADES", use_rth=1))]
    fn head_timestamp(
        &self,
        py: Python<'_>,
        contract: &Contract,
        what_to_show: &str,
        use_rth: i32,
    ) -> PyResult<String> {
        let shared = self.connected_shared()?;
        let asked = ask_id(&shared);
        let req_id = asked.get();
        self.req_head_time_stamp(py, req_id, contract, what_to_show, use_rth, 1)?;
        let what = format!("the earliest data for {} {}", contract.sec_type, contract.symbol);
        let r = wait_for(py, &shared, req_id, &what, |sh| {
            sh.reference.take_head_timestamp_for(req_id as u32)
        })?;
        Ok(r.head_timestamp)
    }

    /// Contracts whose symbol or name matches a pattern.
    fn matching_symbols(
        &self,
        py: Python<'_>,
        pattern: &str,
    ) -> PyResult<Vec<ContractDescription>> {
        let shared = self.connected_shared()?;
        let asked = ask_id(&shared);
        let req_id = asked.get();
        self.req_matching_symbols(py, req_id, pattern)?;
        let what = format!("a symbol search for {pattern}");
        let found = wait_for(py, &shared, req_id, &what, |sh| {
            sh.reference.take_matching_symbols_for(req_id as u32)
        })?;
        Ok(found
            .iter()
            .map(|m| ContractDescription {
                con_id: m.con_id as i64,
                symbol: m.symbol.clone(),
                sec_type: m.sec_type.to_fix().to_string(),
                currency: m.currency.clone(),
                primary_exchange: m.primary_exchange.clone(),
                derivative_sec_types: m.derivative_types.clone(),
            })
            .collect())
    }

    /// The option chains an underlying has, answered rather than only sent.
    ///
    /// The client this follows returns them. Sending the request and returning
    /// nothing left a program that assigned the result holding nothing, with
    /// no way to tell that from an underlying with no options at all.
    /// The headlines the venue holds for a contract.
    ///
    /// Answers rather than reporting through the wrapper, because a program
    /// written against the reference client reads the return value.
    fn news_headlines(
        &self,
        py: Python<'_>,
        con_id: i64,
        provider_codes: &str,
        start_date_time: &str,
        end_date_time: &str,
        total_results: i32,
    ) -> PyResult<Vec<(String, String, String, String)>> {
        let shared = self.connected_shared()?;
        let asked = ask_id(&shared);
        let req_id = asked.get();
        self.req_historical_news(
            py, req_id, con_id, provider_codes, start_date_time, end_date_time,
            total_results, Vec::new(),
        )?;
        let what = format!("the headlines for contract {con_id}");
        let (headlines, _) = wait_for(py, &shared, req_id, &what, |sh| {
            sh.reference.take_historical_news_for(req_id as u32)
        })?;
        Ok(headlines
            .into_iter()
            .map(|h| (h.time, h.provider_code, h.article_id, h.headline))
            .collect())
    }

    /// When a contract trades, over a stretch of days.
    ///
    /// Each session is its opening, its close, and the day it belongs to; the
    /// time zone they are stated in comes with them.
    fn trading_schedule(
        &self,
        py: Python<'_>,
        contract: &Contract,
        end_date_time: &str,
        duration_str: &str,
        use_rth: bool,
    ) -> PyResult<TradingSchedule> {
        let shared = self.connected_shared()?;
        let asked = ask_id(&shared);
        let req_id = asked.get();
        self.req_historical_schedule(py, req_id, contract, end_date_time, duration_str, use_rth)?;
        let what = format!("when {} trades", contract.symbol);
        let schedule = wait_for(py, &shared, req_id, &what, |sh| {
            sh.reference.take_historical_schedule_for(req_id as u32)
        })?;
        Ok((
            schedule.timezone,
            schedule
                .sessions
                .into_iter()
                .map(|s| (s.open_time, s.close_time, s.ref_date))
                .collect(),
        ))
    }

    /// Every venue's option chain for an underlying, returned rather
    /// than delivered on a callback: expiries and strikes, per venue.
    fn option_chains(
        &self,
        py: Python<'_>,
        underlying_symbol: &str,
        fut_fop_exchange: &str,
        underlying_sec_type: &str,
        underlying_con_id: i64,
    ) -> PyResult<Vec<crate::python::compat::contract::OptionChain>> {
        let shared = self.connected_shared()?;
        let asked = ask_id(&shared);
        let req_id = asked.get();
        self.req_sec_def_opt_params(
            py, req_id, underlying_symbol, fut_fop_exchange, underlying_sec_type,
            underlying_con_id,
        )?;
        let what = format!("the option chains of {underlying_symbol}");
        let (_, scopes) = wait_for(py, &shared, req_id, &what, |sh| {
            sh.reference.take_option_params_for(req_id as u32)
        })?;
        Ok(scopes
            .iter()
            .map(|s| crate::python::compat::contract::OptionChain {
                exchange: s.exchange.clone(),
                underlying_con_id,
                trading_class: s.trading_class.clone(),
                multiplier: s.multiplier.clone(),
                expirations: s.expirations.clone(),
                strikes: s.strikes.clone(),
            })
            .collect())
    }

    /// How a contract's traded volume is spread across prices over a period.
    #[pyo3(signature = (contract, use_rth=true, time_period="3 days"))]
    fn histogram_data(
        &self,
        py: Python<'_>,
        contract: &Contract,
        use_rth: bool,
        time_period: &str,
    ) -> PyResult<Vec<(f64, i64)>> {
        let shared = self.connected_shared()?;
        let asked = ask_id(&shared);
        let req_id = asked.get();
        self.req_histogram_data(py, req_id, contract, use_rth, time_period)?;
        let what = format!("a histogram for {} {}", contract.sec_type, contract.symbol);
        let rows = wait_for(py, &shared, req_id, &what, |sh| {
            sh.reference.take_histogram_for(req_id as u32)
        })?;
        Ok(rows.iter().map(|e| (e.price, e.count)).collect())
    }

    /// A fundamental report on a contract, as the venue supplies it.
    fn fundamental_data(
        &self,
        py: Python<'_>,
        contract: &Contract,
        report_type: &str,
    ) -> PyResult<String> {
        let shared = self.connected_shared()?;
        let asked = ask_id(&shared);
        let req_id = asked.get();
        self.req_fundamental_data(py, req_id, contract, report_type, Vec::new())?;
        let what = format!("a {report_type} report for {}", contract.symbol);
        wait_for(py, &shared, req_id, &what, |sh| {
            sh.reference.take_fundamental_for(req_id as u32)
        })
    }

    /// Fill in what the venue knows about a contract, above all its id.
    ///
    /// Most of what this client sends carries a contract, and a contract with
    /// an id is worth more than one without: market data is answered only for a
    /// contract named by id, and an order carrying one needs to state nothing
    /// else.
    ///
    /// A description matching more than one contract is refused rather than
    /// resolved to whichever came back first — the same symbol on the same
    /// venue exists in more than one currency, and picking one silently is how
    /// an order reaches the wrong one.
    pub(crate) fn qualify_contract(&self, py: Python<'_>, contract: &Contract) -> PyResult<Contract> {
        let mut found = self.contract_details(py, contract)?;
        match found.len() {
            0 => Err(PyValueError::new_err(format!(
                "no contract matches {} {} on {}",
                contract.sec_type, contract.symbol, contract.exchange,
            ))),
            1 => Ok(found.remove(0).contract.bind(py).borrow().clone()),
            n => Err(PyValueError::new_err(format!(
                "{} {} on {} matches {n} contracts; state the currency or the exchange",
                contract.sec_type, contract.symbol, contract.exchange,
            ))),
        }
    }

    /// Fill in a whole list of contracts, keeping their order.
    ///
    /// One that cannot be resolved fails the call rather than being dropped:
    /// a list quietly shorter than it was asked for is how a program trades
    /// something other than what it named.
    fn qualify_contracts(
        &self,
        py: Python<'_>,
        contracts: Vec<Py<Contract>>,
    ) -> PyResult<Vec<Contract>> {
        let mut out = Vec::with_capacity(contracts.len());
        for c in contracts {
            let bound = c.bind(py).borrow();
            out.push(self.qualify_contract(py, &bound)?);
        }
        Ok(out)
    }
}

/// The same lookups, handing back the venue's refusal code rather than prose.
///
/// Outside `#[pymethods]` on purpose: every function in that block becomes a
/// method on the Python object, and these are for this crate. A caller with
/// somewhere to put a code — anything that reports to `error` rather than
/// raising — uses these, because picking a code for itself is how a session
/// that ended mid-lookup comes out as a contract that does not exist.
impl EClient {
    pub(crate) fn contract_details_stated(
        &self,
        py: Python<'_>,
        contract: &Contract,
    ) -> Result<Vec<ContractDetails>, Refusal> {
        let shared = self.shared_state()
            .map_err(|e| Refusal::not_connected(e.to_string()))?;
        let asked = ask_id(&shared);
        let req_id = asked.get();
        self.req_contract_details(py, req_id, contract)
            .map_err(|e| Refusal::not_connected(e.to_string()))?;

        let shared = self.connected_shared()
            .map_err(|e| Refusal::not_connected(e.to_string()))?;

        let deadline = Instant::now() + ANSWER_TIMEOUT;
        let collected = py.detach(|| {
            let mut found = Vec::new();
            loop {
                found.extend(shared.reference.take_contract_details_for(req_id as u32));
                if let Some((code, msg)) = shared.reference.take_error_for(req_id as u32) {
                    return Err(Refusal::stated(code, msg));
                }
                if shared.reference.take_contract_details_end_for(req_id as u32) {
                    // Once more, because the definitions and the end are held
                    // apart and the engine writes a definition before the end
                    // that follows it. One arriving between the drain above and
                    // this check is in the queue but not in hand — and dropping
                    // it turns two matches into one, which reads as a contract
                    // described exactly enough to place an order on.
                    found.extend(shared.reference.take_contract_details_for(req_id as u32));
                    return Ok(found);
                }
                // Nothing is coming. Waiting the deadline out only delays the
                // caller learning that, once per request.
                if let Some(why) = shared.reference.session_over() {
                    return Err(Refusal::not_connected(
                        format!("the session is over: {why}"),
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(Refusal::no_answer(format!(
                        "no answer within {}s to a lookup for {} {}",
                        ANSWER_TIMEOUT.as_secs(),
                        contract.sec_type,
                        contract.symbol,
                    )));
                }
                std::thread::sleep(POLL);
            }
        })?;

        Ok(collected.iter().map(|d| ContractDetails::from_definition(py, d)).collect())
    }

    /// Name a description once, and remember what it was named.
    ///
    /// The venue's answer does not change while a session lasts, and asking
    /// again per order turns a program that trades one contract into one that
    /// sends a lookup per order for a name it already has.
    pub(crate) fn qualify_once(
        &self, py: Python<'_>, contract: &Contract, key: &str,
    ) -> Result<Contract, Refusal> {
        if let Some(already) = self.core.named_for(key) {
            return Ok(Contract::from_api(&already));
        }
        let answer = self.qualify_contract_stated(py, contract)?;
        self.core.remember_named(key.to_string(), answer.to_api());
        Ok(answer)
    }

    pub(crate) fn qualify_contract_stated(
        &self, py: Python<'_>, contract: &Contract,
    ) -> Result<Contract, Refusal> {
        let mut found = self.contract_details_stated(py, contract)?;
        match found.len() {
            0 => Err(Refusal::no_definition(format!(
                "no contract matches {} {} on {}",
                contract.sec_type, contract.symbol, contract.exchange,
            ))),
            1 => Ok(found.remove(0).contract.bind(py).borrow().clone()),
            n => Err(Refusal::no_definition(format!(
                "{} {} on {} matches {n} contracts; state the currency or the exchange",
                contract.sec_type, contract.symbol, contract.exchange,
            ))),
        }
    }
}
