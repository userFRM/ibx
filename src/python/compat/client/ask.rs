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
use pyo3::prelude::*;

use std::sync::Arc;

use crate::bridge::SharedState;

use super::EClient;
use super::super::contract::{BarData, Contract, ContractDescription, ContractDetails};

/// How long a question waits for its answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to sleep between looks at the queue. Short enough that a fast
/// answer is not made to wait on the poll, long enough not to spin a core.
const POLL: Duration = Duration::from_millis(5);

/// Ids for questions this layer asks on the caller's behalf. Far above what a
/// caller is likely to use, so an answer to one of these is never mistaken for
/// an answer to theirs.
static NEXT_ASK_ID: AtomicI64 = AtomicI64::new(0x3000_0000);

fn ask_id() -> i64 {
    NEXT_ASK_ID.fetch_add(1, Ordering::Relaxed)
}

/// The id the next question will be asked under. Lets a test put an answer in
/// place before the question is asked, which is the only way to exercise the
/// waiting without a venue on the other end.
#[doc(hidden)]
pub(crate) fn peek_ask_id() -> i64 {
    NEXT_ASK_ID.load(Ordering::Relaxed)
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
}

#[pymethods]
impl EClient {
    /// Everything the venue knows about the contracts matching a description.
    ///
    /// Sends the lookup, waits for the venue to say it has finished, and hands
    /// back every match. A description matching nothing returns an empty list;
    /// a venue that refuses the lookup raises with the venue's own words.
    fn contract_details(
        &self,
        py: Python<'_>,
        contract: &Contract,
    ) -> PyResult<Vec<ContractDetails>> {
        let req_id = ask_id();
        self.req_contract_details(py, req_id, contract)?;

        let shared = self.connected_shared()?;

        let deadline = Instant::now() + ANSWER_TIMEOUT;
        let collected = py.detach(|| {
            let mut found = Vec::new();
            loop {
                found.extend(shared.reference.take_contract_details_for(req_id as u32));
                if let Some((code, msg)) = shared.reference.take_error_for(req_id as u32) {
                    return Err(format!("{msg} ({code})"));
                }
                if shared.reference.take_contract_details_end_for(req_id as u32) {
                    return Ok(found);
                }
                if Instant::now() >= deadline {
                    return Err(format!(
                        "no answer within {}s to a lookup for {} {}",
                        ANSWER_TIMEOUT.as_secs(),
                        contract.sec_type,
                        contract.symbol,
                    ));
                }
                std::thread::sleep(POLL);
            }
        })
        .map_err(PyRuntimeError::new_err)?;

        Ok(collected.iter().map(|d| ContractDetails::from_definition(py, d)).collect())
    }


    /// Bars for a contract over a period, handed back rather than delivered a
    /// bar at a time to a callback.
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
        let req_id = ask_id();
        self.req_historical_data(
            py, req_id, contract, end_date_time, duration_str, bar_size_setting,
            what_to_show, use_rth, 1, false, Vec::new(),
        )?;
        let shared = self.connected_shared()?;

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
        let req_id = ask_id();
        self.req_head_time_stamp(py, req_id, contract, what_to_show, use_rth, 1)?;
        let shared = self.connected_shared()?;
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
        let req_id = ask_id();
        self.req_matching_symbols(py, req_id, pattern)?;
        let shared = self.connected_shared()?;
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

    /// How a contract's traded volume is spread across prices over a period.
    #[pyo3(signature = (contract, use_rth=true, time_period="3 days"))]
    fn histogram_data(
        &self,
        py: Python<'_>,
        contract: &Contract,
        use_rth: bool,
        time_period: &str,
    ) -> PyResult<Vec<(f64, i64)>> {
        let req_id = ask_id();
        self.req_histogram_data(py, req_id, contract, use_rth, time_period)?;
        let shared = self.connected_shared()?;
        let what = format!("a histogram for {} {}", contract.sec_type, contract.symbol);
        let rows = wait_for(py, &shared, req_id, &what, |sh| {
            sh.reference.take_histogram_for(req_id as u32)
        })?;
        Ok(rows.iter().map(|e| (e.price, e.count)).collect())
    }

    /// A fundamental report on a contract, as the venue's own document.
    fn fundamental_data(
        &self,
        py: Python<'_>,
        contract: &Contract,
        report_type: &str,
    ) -> PyResult<String> {
        let req_id = ask_id();
        self.req_fundamental_data(py, req_id, contract, report_type, Vec::new())?;
        let shared = self.connected_shared()?;
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
    fn qualify_contract(&self, py: Python<'_>, contract: &Contract) -> PyResult<Contract> {
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
