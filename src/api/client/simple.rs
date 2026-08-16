//! Calls with the arguments a caller usually wants already filled in.
//!
//! Every request in this client states each of its arguments, because the
//! protocol does. Most callers state the same value for most of them: bars of
//! trades during regular hours, ending now; an order id taken from the last one
//! handed out; a summary of the tags a person reads first. Written out on every
//! call, the two arguments that carry the question are lost among the four that
//! do not.
//!
//! These name the question and fill in the rest. Each is a few lines over a
//! call in [`ask`](super::ask) or beside it — where a caller wants a value this
//! layer decides, the fuller call is still there and takes it.
//!
//! ```no_run
//! # use ibx::{Client, EClientConfig};
//! # use ibx::types::model::{Contract, Order};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let ib = Client::connect(&EClientConfig {
//!     username: "user".into(), password: "pass".into(),
//!     paper: true, ..Default::default()
//! })?;
//!
//! let spy = ib.qualify(Contract::stock("SPY"))?;
//! let bars = ib.bars(&spy, "2 D", "1 hour")?;
//! let preview = ib.preview(&spy, &Order::limit("BUY", 1.0, 100.0))?;
//! println!("{} bars, commission {}", bars.len(), preview.commission_and_fees);
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

use crate::error_codes::Refusal;
use crate::api::client::{AccountValue, OrderReport};
use crate::types::model::{BarData, Contract, ContractDetails, Order, OrderState};

use super::EClient;

/// What a person reads first when they look at an account: what it is worth,
/// what it can buy, and what is already committed.
const HEADLINE_TAGS: &str =
    "NetLiquidation,TotalCashValue,BuyingPower,GrossPositionValue,MaintMarginReq,AvailableFunds";

/// How long an order is watched before the caller is handed what is known.
const SETTLE: Duration = Duration::from_secs(5);

impl EClient {
    /// Bars of trades during regular hours, ending now.
    ///
    /// `duration` and `bar_size` are the venue's own words for them — `"2 D"`,
    /// `"1 hour"`. For midpoint or bid/ask bars, an end other than now, or bars
    /// through the whole session, state them with
    /// [`historical_data`](EClient::historical_data).
    pub fn bars(
        &self, contract: &Contract, duration: &str, bar_size: &str,
    ) -> Result<Vec<BarData>, Refusal> {
        self.historical_data(contract, "", duration, bar_size, "TRADES", true)
    }

    /// The one contract the venue means by this description.
    ///
    /// A description that matches more than one is refused rather than
    /// answered with whichever the venue lists first, which is a different
    /// contract from the one asked about.
    pub fn qualify(&self, contract: Contract) -> Result<Contract, Refusal> {
        self.qualify_contract(&contract)
    }

    /// Everything the venue lists under this description.
    ///
    /// Where [`qualify`](EClient::qualify) refuses an ambiguous description,
    /// this returns each contract that matches it.
    pub fn lookup(&self, contract: &Contract) -> Result<Vec<ContractDetails>, Refusal> {
        self.contract_details(contract)
    }

    /// What an order would cost and what it would do to the margin, without
    /// placing it.
    pub fn preview(
        &self, contract: &Contract, order: &Order,
    ) -> Result<OrderState, Refusal> {
        self.what_if_order(contract, order)
    }

    /// Place an order under the next id, and hand back what became of it.
    ///
    /// Waits for the venue to settle it — filled, cancelled or rejected — or
    /// for five seconds, whichever comes first, and reports what is known
    /// either way. An order that is still working is reported as working; it
    /// is not cancelled.
    ///
    /// To place without waiting, or to choose the id,
    /// [`place_order`](EClient::place_order) takes both.
    pub fn place(&self, contract: &Contract, order: &Order) -> Result<OrderReport, Refusal> {
        // The turn is taken before the order is sent and held until it settles.
        // Taken only for the waiting, a question starting between the two would
        // pump this order's reply into its own collector and discard it, and
        // the order would be reported unanswered although the venue answered.
        let _turn = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        let order_id = self.next_order_id();
        self.place_order(order_id, contract, order)?;
        self.await_order_holding_the_turn(order_id, SETTLE)
    }

    /// Start a market-data subscription, and hand back the id that withdraws it.
    ///
    /// The fuller call takes an id so that a program with its own bookkeeping
    /// can match a tick to the request that asked for it. A program that only
    /// wants the price of a thing has no use for one, and this picks from the
    /// range this client asks its own questions under — far above what a caller
    /// is likely to state, and what the dispatch loop uses to tell the two
    /// apart. A caller that states an id from that range for a request of its
    /// own is the one case where they meet. Pass what this returns to
    /// [`cancel_mkt_data`](EClient::cancel_mkt_data) to stop the stream.
    ///
    /// The contract must carry the venue's own id, which
    /// [`qualify`](EClient::qualify) supplies.
    pub fn watch(&self, contract: &Contract) -> Result<i64, Refusal> {
        let req_id = super::ask::ask_id();
        self.req_mkt_data(req_id, contract, "", false, false)?;
        Ok(req_id)
    }

    /// The latest bid, ask and last for a contract being watched.
    ///
    /// Read without waiting on the callback loop and without locking it, so a
    /// program may read from any thread and as often as it likes. `None` until
    /// the venue has sent a first tick, and for a contract nobody subscribed
    /// to — the subscription is what makes the quote exist, not this call.
    pub fn quote_of(&self, contract: &Contract) -> Option<crate::types::Quote> {
        self.quote_by_instrument(self.instrument_of(contract.con_id)?)
    }

    /// What the account is worth and what it can buy.
    ///
    /// For other tags, or for one group of a set of advised accounts,
    /// [`account_summary`](EClient::account_summary) takes them.
    pub fn summary(&self) -> Result<Vec<AccountValue>, Refusal> {
        self.account_summary(HEADLINE_TAGS)
    }
}
