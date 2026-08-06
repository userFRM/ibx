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

use crate::api::types::ContractDetails;
use crate::api::wrapper::Wrapper;

use super::{Contract, EClient};

/// How long a question waits for its answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

/// Ids for questions this layer asks on the caller's behalf. Far above what a
/// caller is likely to use, so an answer to one of these is never mistaken for
/// an answer to theirs.
static NEXT_ASK_ID: AtomicI64 = AtomicI64::new(0x3000_0000);

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

impl EClient {
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
        // A symbol without a currency names a listing in every currency it
        // trades in. Asking for SPY that way answers with the dollar listing
        // and the Australian dollar listing both, and taking one of them is
        // how an order reaches the wrong exchange in the wrong currency. Say
        // which one.
        if contract.currency.is_empty() && contract.con_id == 0 {
            return Err(format!(
                "{} {} names a listing in every currency it trades in: state the currency",
                contract.sec_type, contract.symbol,
            ));
        }
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
