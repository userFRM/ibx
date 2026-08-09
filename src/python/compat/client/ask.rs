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

use super::EClient;
use super::super::contract::{Contract, ContractDetails};

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

        let Some(shared) = self.shared.lock().unwrap().clone() else {
            return Err(PyRuntimeError::new_err("not connected"));
        };

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
