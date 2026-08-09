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
use crate::api::types::{BarData, ContractDetails};
use crate::api::wrapper::Wrapper;
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
    pub errors: Vec<(i64, i64, String)>,
    pub current_time: Option<i64>,
    pub managed_accounts: Option<String>,
    pub news_providers: Vec<String>,
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
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to sleep between looks at the queue.
const POLL: Duration = Duration::from_millis(5);

/// A session whose calls return what they were asked for.
pub struct Client {
    inner: EClient,
    recorded: Arc<Mutex<Recorded>>,
    /// Ids for the streams this shape opens. Far above what a caller of the
    /// callback shape is likely to use on the same session.
    next_stream_id: std::sync::atomic::AtomicI64,
}

impl Client {
    /// Open a session.
    pub fn connect(config: &EClientConfig) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            inner: EClient::connect(config)?,
            recorded: Arc::new(Mutex::new(Recorded::default())),
            next_stream_id: std::sync::atomic::AtomicI64::new(STREAM_ID_BASE),
        })
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

    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    pub fn disconnect(&self) {
        self.inner.disconnect();
    }

    // -- calls that answer -------------------------------------------------

    /// Everything the venue knows about the contracts matching a description.
    pub fn contract_details(&self, contract: &Contract) -> Result<Vec<ContractDetails>, String> {
        self.inner.contract_details(contract)
    }

    /// Fill in what the venue knows about a contract, above all its id.
    pub fn qualify_contract(&self, contract: &Contract) -> Result<Contract, String> {
        self.inner.qualify_contract(contract)
    }

    /// Fill in a whole list, keeping their order.
    pub fn qualify_contracts(&self, contracts: &[Contract]) -> Result<Vec<Contract>, String> {
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
    ) -> Result<Vec<BarData>, String> {
        self.inner.historical_data(
            contract, end_date_time, duration, bar_size, what_to_show,
            use_rth,
        )
    }

    /// What the account holds.
    pub fn positions(&self) -> Result<Vec<PositionRow>, String> {
        self.inner.positions()
    }

    /// The account's figures for the tags asked for.
    pub fn account_summary(&self, tags: &str) -> Result<Vec<AccountValue>, String> {
        self.inner.account_summary(tags)
    }

    /// Every expiration and strike a venue lists for an underlying.
    pub fn option_chain(&self, underlying: &Contract) -> Result<Vec<OptionChain>, String> {
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
    ) -> Result<T, String> {
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        loop {
            if let Some(v) = take(&self.inner.shared) {
                return Ok(v);
            }
            if let Some((code, message)) = self.inner.shared.reference.take_error_for(req_id as u32) {
                return Err(format!("{message} ({code})"));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "no answer within {}s to {what}",
                    ANSWER_TIMEOUT.as_secs()
                ));
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
    ) -> Result<String, String> {
        let req_id = self.stream_id();
        self.inner.req_head_time_stamp(req_id, contract, what_to_show, use_rth, 1)?;
        let what = format!("the earliest data for {} {}", contract.sec_type, contract.symbol);
        self.wait_for(req_id, &what, |sh| {
            sh.reference.take_head_timestamp_for(req_id as u32)
        })
        .map(|r| r.head_timestamp)
    }

    /// Contracts whose symbol or name matches a pattern.
    pub fn matching_symbols(&self, pattern: &str) -> Result<Vec<crate::control::contracts::SymbolMatch>, String> {
        let req_id = self.stream_id();
        self.inner.req_matching_symbols(req_id, pattern)?;
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
    ) -> Result<Vec<crate::control::histogram::HistogramEntry>, String> {
        let req_id = self.stream_id();
        self.inner.req_histogram_data(req_id, contract, use_rth, period)?;
        let what = format!("a histogram for {} {}", contract.sec_type, contract.symbol);
        self.wait_for(req_id, &what, |sh| {
            sh.reference.take_histogram_for(req_id as u32)
        })
    }

    /// A fundamental report on a contract, as the venue's own document.
    pub fn fundamental_data(
        &self,
        contract: &Contract,
        report_type: &str,
    ) -> Result<String, String> {
        let req_id = self.stream_id();
        self.inner.req_fundamental_data(req_id, contract, report_type)?;
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
    ) -> Result<Subscription<RealTimeBar>, String> {
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
    ) -> Result<Subscription<DepthUpdate>, String> {
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

    pub fn req_current_time(&self) {
        let mut r = self.recorded.lock().unwrap();
        self.inner.req_current_time(&mut *r);
    }

    pub fn managed_accounts(&self) -> Option<String> {
        self.recorded.lock().unwrap().managed_accounts.clone()
    }

    pub fn global_cancel(&self) -> Result<(), String> {
        self.inner.req_global_cancel().map_err(|e| e.to_string())
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


}
