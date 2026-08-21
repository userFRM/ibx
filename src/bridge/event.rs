//! What the engine tells a caller happened, and what it knows about an order.

use std::collections::HashMap;
use crate::control::historical::{HistoricalResponse, HeadTimestampResponse};
use crate::control::contracts::ContractDefinition;
use crate::types::*;
use crate::types::model as api;

/// Enriched order info from CCP execution reports, for open_order / completed_order
/// callbacks.
#[derive(Clone, Debug)]
pub struct RichOrderInfo {
    /// The contract it is on.
    pub contract: api::Contract,
    /// The order as this client sent it.
    pub order: api::Order,
    /// What the venue says about it, including what it would cost.
    pub order_state: api::OrderState,
    /// Last execution details from this order's exec reports.
    pub last_exec: api::Execution,
}

/// Events emitted by the IB engine.
#[derive(Debug, Clone)]
pub enum Event {
    /// Market data tick received. Read the latest quote via `Client::quote()`.
    Tick(InstrumentId),
    /// Order filled (partial or full).
    Fill(Fill),
    /// Order status changed.
    OrderUpdate(OrderUpdate),
    /// Cancel or modify request rejected.
    CancelReject(CancelReject),
    /// Tick-by-tick trade data.
    TbtTrade(TbtTrade),
    /// Tick-by-tick bid/ask quote.
    TbtQuote(TbtQuote),
    /// What-if order response (margin/commission preview).
    WhatIf(WhatIfResponse),
    /// Real-time news headline.
    News(TickNews),
    /// The venue's model for an option: its price, the greeks and the
    /// volatility that price implies.
    OptionComputation(crate::types::OptionComputation),
    /// Historical bar data.
    HistoricalData {
        /// The request this answers.
        req_id: u32,
        /// What answered it.
        data: HistoricalResponse,
    },
    /// Head timestamp response.
    HeadTimestamp {
        /// The request this answers.
        req_id: u32,
        /// What answered it.
        data: HeadTimestampResponse,
    },
    /// Contract details response.
    ContractDetails {
        /// The request this answers.
        req_id: u32,
        /// What the venue said about the contract.
        details: Box<ContractDefinition>,
    },
    /// End of contract details for a request.
    ContractDetailsEnd(u32),
    /// Position update.
    PositionUpdate {
        /// The engine's own slot for the contract.
        instrument: InstrumentId,
        /// The venue's id for it.
        con_id: i64,
        /// How much is held.
        position: f64,
        /// What it cost on average.
        avg_cost: Price,
    },
    /// Connection lost, without the caller asking for it.
    Disconnected,
    /// The session ended because the caller asked it to.
    ///
    /// Distinct from a loss: the reference client answers `disconnect()` with
    /// `connectionClosed` and reports nothing on the error channel, so a
    /// program that stands down on connectivity loss must not be told it lost
    /// the session it just closed.
    Stopped,
    /// A transport that had announced its loss is carrying traffic again, with
    /// the subscriptions the reconnect re-established. Emitted only after a
    /// `Disconnected`, so a client that stood down on one has the signal to
    /// resume — without it an overnight outage leaves it stood down for good.
    Reconnected,
    /// Gateway logon completed. `ccp_session_id` matches the `x-ccp-session-id` header
    /// expected by webapp REST endpoints. `misc_urls` maps logical names (e.g.
    /// `region_dam`)
    /// to host URLs as pushed by the venue during logon. The map is empty when the
    /// gateway does not push a URL set; callers should fall back to a documented
    /// literal
    /// (e.g. `api.ibkr.com`) in that case.
    GatewayLogon {
        /// What a web endpoint expects as a session header.
        ccp_session_id: String,
        /// Hosts the venue pushed at logon, by name.
        misc_urls: HashMap<String, String>,
    },
}
