//! Bridge module: shared state and events between the HotLoop and external callers.
//!
//! Architecture:
//! - `SharedState` composes four domain-specific containers:
//!   - `MarketDataState` — lock-free quotes (SeqLock), TBT, real-time bars, news ticks.
//!   - `OrderState` — fills, order updates, cancel rejects, what-if, order cache.
//!   - `ReferenceState` — historical data, contracts, scanners, news archives, market rules.
//!   - `PortfolioState` — account snapshot, position info, atomic positions.
//! - `Event` enum carries all events through a channel for the `EClient` API.
//! - The HotLoop pushes to SharedState sub-containers directly.
//! - External callers read snapshots and poll events without blocking the hot loop.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::sync::{Condvar, Mutex};

use std::collections::HashMap;

/// The token the venue grants a session that may name Nasdaq by its older
/// spelling. Stated on the granted-feature list at logon.
const ISLAND_FOR_NASDAQ_GRANT: &str = "ISLAND2NASDAQ";
use crate::control::historical::{HistoricalResponse, HeadTimestampResponse};
use crate::control::contracts::{ContractDefinition, OptionChainScope, SymbolMatch};
use crate::control::scanner::ScannerResult;
use crate::control::news::NewsHeadline;
use crate::control::histogram::HistogramEntry;
use crate::control::contracts::MarketRule;
use crate::types::*;
use crate::api::types as api;

/// Enriched order info from CCP execution reports, for open_order / completed_order callbacks.
#[derive(Clone, Debug)]
pub struct RichOrderInfo {
    pub contract: api::Contract,
    pub order: api::Order,
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
    /// The venue's own model for an option: its price, the greeks and the
    /// volatility that price implies.
    OptionComputation(crate::types::OptionComputation),
    /// Historical bar data.
    HistoricalData { req_id: u32, data: HistoricalResponse },
    /// Head timestamp response.
    HeadTimestamp { req_id: u32, data: HeadTimestampResponse },
    /// Contract details response.
    ContractDetails { req_id: u32, details: Box<ContractDefinition> },
    /// End of contract details for a request.
    ContractDetailsEnd(u32),
    /// Position update.
    PositionUpdate { instrument: InstrumentId, con_id: i64, position: f64, avg_cost: Price },
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
    /// expected by webapp REST endpoints. `misc_urls` maps logical names (e.g. `region_dam`)
    /// to host URLs as pushed by the gateway during logon. The map is empty when the
    /// gateway does not push a URL set; callers should fall back to a documented literal
    /// (e.g. `api.ibkr.com`) in that case.
    GatewayLogon {
        ccp_session_id: String,
        misc_urls: HashMap<String, String>,
    },
}

/// SeqLock-protected quote slot. Writer (hot loop) never blocks.
/// A quote published by the hot loop and read by any number of consumers.
///
/// The version counter is the freshness test: odd means a write is in flight,
/// and a reader that sees the same even value on both sides of its snapshot
/// took a whole one. The payload itself is held as words rather than a plain
/// copy of the struct, so the concurrent read and write are both defined
/// operations — a version counter can discard a torn snapshot but cannot make
/// the racing access that produced it legal (ibx#388).
#[repr(align(64))]
pub struct SeqQuote {
    version: AtomicU64,
    data: [AtomicI64; QUOTE_WORDS],
}

/// `Quote` as its fields, in order. Both directions destructure or name every
/// field, so a field added to `Quote` fails to compile here rather than being
/// silently dropped from everything the seqlock publishes.
const QUOTE_WORDS: usize = 16;

fn quote_to_words(q: &Quote) -> [i64; QUOTE_WORDS] {
    let Quote {
        bid, ask, last, bid_size, ask_size, last_size, volume,
        open, high, low, close, timestamp_ns,
        bid_exch_mask, ask_exch_mask, last_exch_mask, halted,
    } = *q;
    [
        bid, ask, last, bid_size, ask_size, last_size, volume,
        open, high, low, close, timestamp_ns as i64,
        bid_exch_mask, ask_exch_mask, last_exch_mask, halted,
    ]
}

fn quote_from_words(w: [i64; QUOTE_WORDS]) -> Quote {
    Quote {
        bid: w[0], ask: w[1], last: w[2],
        bid_size: w[3], ask_size: w[4], last_size: w[5], volume: w[6],
        open: w[7], high: w[8], low: w[9], close: w[10],
        timestamp_ns: w[11] as u64,
        bid_exch_mask: w[12], ask_exch_mask: w[13], last_exch_mask: w[14],
        halted: w[15],
    }
}

impl Default for SeqQuote {
    fn default() -> Self {
        Self::new()
    }
}

impl SeqQuote {
    pub fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            data: std::array::from_fn(|_| AtomicI64::new(0)),
        }
    }

    /// Write a quote (hot loop side). Never blocks.
    #[inline]
    pub fn write(&self, quote: &Quote) {
        // AcqRel, not Release: the payload writes below must not be reordered
        // above this store. Release alone only fences what precedes it; the
        // Acquire half is what pins *following* accesses inside the odd window.
        self.version.fetch_add(1, Ordering::AcqRel); // odd = writing
        for (slot, word) in self.data.iter().zip(quote_to_words(quote)) {
            slot.store(word, Ordering::Relaxed);
        }
        self.version.fetch_add(1, Ordering::Release); // even = stable
    }

    /// Read a consistent quote snapshot (reader side). Spins on conflict.
    #[inline]
    pub fn read(&self) -> Quote {
        loop {
            let v1 = self.version.load(Ordering::Acquire);
            if v1 & 1 != 0 { continue; } // writer active
            let mut words = [0i64; QUOTE_WORDS];
            for (word, slot) in words.iter_mut().zip(self.data.iter()) {
                *word = slot.load(Ordering::Relaxed);
            }
            // The fence is what makes the check mean anything: an Acquire
            // load constrains what comes after it, so without this the payload
            // reads above may be satisfied after the version read below and a
            // torn snapshot would pass a counter that never moved.
            std::sync::atomic::fence(Ordering::Acquire);
            let v2 = self.version.load(Ordering::Relaxed);
            if v1 == v2 { return quote_from_words(words); }
        }
    }
}

#[cfg(test)]
mod seq_quote_tests {
    use super::*;

    /// A reader and a writer working the same slot at once. The payload is
    /// accessed as atomics, so the race the version counter guards against is
    /// a defined operation rather than undefined behaviour — which is what
    /// lets this run under Miri at all.
    #[test]
    fn a_concurrent_reader_never_sees_half_a_quote() {
        let slot = std::sync::Arc::new(SeqQuote::new());
        let writer = {
            let slot = slot.clone();
            std::thread::spawn(move || {
                for i in 1..500i64 {
                    // Every field moves together, so any snapshot mixing two
                    // generations is visible as a field that disagrees.
                    slot.write(&Quote {
                        bid: i, ask: i, last: i,
                        bid_size: i, ask_size: i, last_size: i, volume: i,
                        open: i, high: i, low: i, close: i,
                        timestamp_ns: i as u64,
                        bid_exch_mask: i, ask_exch_mask: i, last_exch_mask: i,
                        halted: i,
                    });
                }
            })
        };
        for _ in 0..500 {
            let q = slot.read();
            assert_eq!(q.bid, q.last_exch_mask, "a snapshot must come from one write");
            assert_eq!(q.timestamp_ns as i64, q.bid, "including the field of another type");
        }
        writer.join().unwrap();
    }
}

#[cfg(test)]
mod order_replay_tests {
    use super::*;
    use crate::types::{OrderStatus, OrderUpdate};

    fn update(order_id: u64, status: OrderStatus, filled: f64, remaining: f64) -> OrderUpdate {
        OrderUpdate {
            order_id, instrument: 0, status,
            filled_qty: filled, remaining_qty: remaining, avg_price: 0,
            perm_id: 0, parent_id: 0, timestamp_ns: 0,
        }
    }

    /// The connect-time replay names what the server thinks is working, and it
    /// is published as it arrives. An order this client has already been paid
    /// on must not come back from it as live.
    #[test]
    fn the_replay_does_not_resurrect_an_order_that_finished() {
        let s = OrderState::new();
        s.push_completed_order(crate::types::CompletedOrder {
            order_id: 7, instrument: 0, status: OrderStatus::Filled,
            filled_qty: 1, timestamp_ns: 0,
        });

        s.push_order_info(7, RichOrderInfo {
            contract: Default::default(),
            order: Default::default(),
            order_state: crate::api::types::OrderState {
                status: "Submitted".to_string(), ..Default::default()
            },
            last_exec: Default::default(),
        });

        assert!(
            s.drain_open_orders().is_empty(),
            "a filled order is not reported as working again by the replay",
        );
    }

    /// The gateway echoes a working status behind a fill. The order is retired
    /// by then, so the echo finds no record to be refused by and reported a
    /// filled order as working with nothing filled.
    #[test]
    fn a_finished_order_is_not_reopened_by_a_frame_behind_it() {
        let s = OrderState::new();
        s.push_order_update(update(7, OrderStatus::Filled, 1.0, 0.0));
        s.push_completed_order(crate::types::CompletedOrder {
            order_id: 7, instrument: 0, status: OrderStatus::Filled,
            filled_qty: 1, timestamp_ns: 0,
        });

        // Queued before the fill was known, which is the real ordering.
        s.push_order_update(update(7, OrderStatus::PreSubmitted, 0.0, 1.0));

        let seen = s.drain_order_updates();
        assert_eq!(seen.len(), 1, "only the fill reaches the caller: {seen:?}");
        assert_eq!(seen[0].status, OrderStatus::Filled);

        // An order that has not finished is untouched by this.
        s.push_order_update(update(8, OrderStatus::PreSubmitted, 0.0, 1.0));
        assert_eq!(s.drain_order_updates().len(), 1, "a live order still reports");
    }
}

// ── Domain-specific state containers ──

/// Lock-free quotes, TBT streams, real-time bars, depth updates, and news ticks.
pub struct MarketDataState {
    quotes: Box<[SeqQuote; MAX_INSTRUMENTS]>,
    /// InstrumentId counter — set by hot loop on RegisterInstrument.
    instrument_count: AtomicU64,
    tbt_trades: Mutex<Vec<TbtTrade>>,
    tbt_quotes: Mutex<Vec<TbtQuote>>,
    real_time_bars: Mutex<Vec<(u32, RealTimeBar)>>,
    depth_updates: Mutex<Vec<DepthUpdate>>,
    tick_news: Mutex<Vec<TickNews>>,
    news_bulletins: Mutex<Vec<NewsBulletin>>,
    option_computations: Mutex<Vec<crate::types::OptionComputation>>,
    /// The last statement the venue made of its own model, per contract, kept
    /// rather than only handed over.
    last_option_model: Mutex<std::collections::HashMap<crate::types::InstrumentId, crate::types::OptionComputation>>,
    /// Subscriptions the venue was never able to be asked for, and why.
    subscription_failures: Mutex<Vec<(crate::types::InstrumentId, String)>>,
    /// What the venue has said went wrong, in its own words.
    venue_errors: Mutex<Vec<String>>,
    /// The venue's own clock, from the last message it sent.
    ///
    /// Every message it sends is stamped with the time it sent it. Reporting
    /// this machine's clock instead would answer the question a caller asked —
    /// how far apart are we and the venue — with the one number that cannot
    /// tell them.
    venue_time: Mutex<Option<String>>,
    /// Messages the venue sent that nothing here reads, named once each:
    /// which connection, and what it was. Empty is the claim that this client
    /// reads everything this venue sends it, and the only way to check it.
    unread_wire: Mutex<Vec<(&'static str, String)>>,
}

impl MarketDataState {
    fn new() -> Self {
        Self {
            quotes: Box::new(std::array::from_fn(|_| SeqQuote::new())),
            instrument_count: AtomicU64::new(0),
            tbt_trades: Mutex::new(Vec::with_capacity(256)),
            tbt_quotes: Mutex::new(Vec::with_capacity(256)),
            real_time_bars: Mutex::new(Vec::with_capacity(64)),
            depth_updates: Mutex::new(Vec::with_capacity(64)),
            tick_news: Mutex::new(Vec::with_capacity(32)),
            news_bulletins: Mutex::new(Vec::with_capacity(16)),
            option_computations: Mutex::new(Vec::with_capacity(16)),
            last_option_model: Mutex::new(std::collections::HashMap::new()),
            subscription_failures: Mutex::new(Vec::new()),
            venue_errors: Mutex::new(Vec::new()),
            venue_time: Mutex::new(None),
            unread_wire: Mutex::new(Vec::new()),
        }
    }

    /// Read a quote snapshot (lock-free via SeqLock).
    /// Unchecked hot-path accessor: `id` must be a registered InstrumentId
    /// (< MAX_INSTRUMENTS) or this panics. External surfaces go through
    /// `try_quote` (ibx#234).
    #[inline]
    pub fn quote(&self, id: InstrumentId) -> Quote {
        self.quotes[id as usize].read()
    }

    /// Bounds-checked quote read for user-supplied instrument ids: an
    /// out-of-range id is a caller error, not a reason to panic the process
    /// through the language boundary (ibx#234).
    #[inline]
    pub fn try_quote(&self, id: InstrumentId) -> Option<Quote> {
        if (id as usize) < MAX_INSTRUMENTS {
            Some(self.quotes[id as usize].read())
        } else {
            None
        }
    }

    /// Number of registered instruments.
    pub fn instrument_count(&self) -> u32 {
        self.instrument_count.load(Ordering::Relaxed) as u32
    }

    pub fn drain_tbt_trades(&self) -> Vec<TbtTrade> {
        self.tbt_trades.lock().unwrap().drain(..).collect()
    }

    pub fn drain_tbt_quotes(&self) -> Vec<TbtQuote> {
        self.tbt_quotes.lock().unwrap().drain(..).collect()
    }

    pub fn drain_real_time_bars(&self) -> Vec<(u32, RealTimeBar)> {
        self.real_time_bars.lock().unwrap().drain(..).collect()
    }

    /// Bars answering one request, leaving other requests' alone.
    pub fn take_real_time_bars_for(&self, req_id: u32) -> Vec<RealTimeBar> {
        let mut q = self.real_time_bars.lock().unwrap();
        let mut mine = Vec::new();
        let mut i = 0;
        while i < q.len() {
            if q[i].0 == req_id { mine.push(q.remove(i).1); } else { i += 1; }
        }
        mine
    }

    /// Book changes answering one request.
    pub fn take_depth_updates_for(&self, req_id: u32) -> Vec<DepthUpdate> {
        let mut q = self.depth_updates.lock().unwrap();
        let mut mine = Vec::new();
        let mut i = 0;
        while i < q.len() {
            if q[i].req_id == req_id { mine.push(q.remove(i)); } else { i += 1; }
        }
        mine
    }

    pub fn drain_depth_updates(&self) -> Vec<DepthUpdate> {
        self.depth_updates.lock().unwrap().drain(..).collect()
    }

    pub fn drain_tick_news(&self) -> Vec<TickNews> {
        self.tick_news.lock().unwrap().drain(..).collect()
    }

    pub fn drain_news_bulletins(&self) -> Vec<NewsBulletin> {
        self.news_bulletins.lock().unwrap().drain(..).collect()
    }

    pub fn drain_option_computations(&self) -> Vec<crate::types::OptionComputation> {
        self.option_computations.lock().unwrap().drain(..).collect()
    }

    /// Everything the venue has sent this session that nothing reads.
    pub fn unread_wire(&self) -> Vec<(&'static str, String)> {
        self.unread_wire.lock().unwrap().clone()
    }

    #[doc(hidden)] pub fn note_unread_wire(&self, connection: &'static str, what: String) {
        let mut seen = self.unread_wire.lock().unwrap();
        if !seen.iter().any(|(c, w)| *c == connection && *w == what) {
            seen.push((connection, what));
        }
    }

    /// Record the time the venue stamped on a message.
    pub fn note_venue_time(&self, stamped: &str) {
        *self.venue_time.lock().unwrap() = Some(stamped.to_string());
    }

    /// The venue's clock as of its last message, if it has sent one.
    pub fn venue_time(&self) -> Option<String> {
        self.venue_time.lock().unwrap().clone()
    }

    pub fn drain_venue_errors(&self) -> Vec<String> {
        self.venue_errors.lock().unwrap().drain(..).collect()
    }

    #[doc(hidden)] pub fn push_venue_error(&self, text: String) {
        self.venue_errors.lock().unwrap().push(text);
    }

    pub fn drain_subscription_failures(&self) -> Vec<(crate::types::InstrumentId, String)> {
        self.subscription_failures.lock().unwrap().drain(..).collect()
    }

    // ── Hot-loop-side writers ──

    #[doc(hidden)]
    pub fn push_quote(&self, id: InstrumentId, quote: &Quote) {
        self.quotes[id as usize].write(quote);
    }

    #[doc(hidden)] pub fn push_tbt_trade(&self, trade: TbtTrade) {
        self.tbt_trades.lock().unwrap().push(trade);
    }

    #[doc(hidden)] pub fn push_tbt_quote(&self, quote: TbtQuote) {
        self.tbt_quotes.lock().unwrap().push(quote);
    }


    #[doc(hidden)] pub fn push_real_time_bar(&self, req_id: u32, bar: RealTimeBar) {
        self.real_time_bars.lock().unwrap().push((req_id, bar));
    }

    #[doc(hidden)] pub fn push_depth_update(&self, update: DepthUpdate) {
        self.depth_updates.lock().unwrap().push(update);
    }

    /// Remove all buffered depth updates for a given req_id (called on cancel).
    #[doc(hidden)] pub fn purge_depth_updates(&self, req_id: u32) {
        self.depth_updates.lock().unwrap().retain(|u| u.req_id != req_id);
    }

    #[doc(hidden)] pub fn push_tick_news(&self, news: TickNews) {
        self.tick_news.lock().unwrap().push(news);
    }

    #[doc(hidden)] pub fn push_news_bulletin(&self, bulletin: NewsBulletin) {
        self.news_bulletins.lock().unwrap().push(bulletin);
    }

    /// What the venue last said its own model made of a contract.
    ///
    /// Kept as well as delivered. Delivered alone it is gone the moment a
    /// caller reads it, and answering "what would this be worth at another
    /// volatility" needs the venue's own statement still to hand.
    pub fn option_model(&self, instrument: crate::types::InstrumentId) -> Option<crate::types::OptionComputation> {
        self.last_option_model.lock().unwrap().get(&instrument).copied()
    }

    #[doc(hidden)] pub fn push_option_computation(&self, comp: crate::types::OptionComputation) {
        self.last_option_model.lock().unwrap().insert(comp.instrument, comp);
        self.option_computations.lock().unwrap().push(comp);
    }

    #[doc(hidden)] pub fn push_subscription_failure(&self, instrument: crate::types::InstrumentId, reason: String) {
        self.subscription_failures.lock().unwrap().push((instrument, reason));
    }

    #[doc(hidden)] pub fn set_instrument_count(&self, count: u32) {
        self.instrument_count.store(count as u64, Ordering::Relaxed);
    }
}

/// Fills, order status updates, cancel rejects, what-if responses, order
/// cache, and inactive-order reasons.
pub struct OrderState {
    fills: Mutex<Vec<Fill>>,
    order_updates: Mutex<Vec<OrderUpdate>>,
    cancel_rejects: Mutex<Vec<CancelReject>>,
    what_if_responses: Mutex<Vec<WhatIfResponse>>,
    completed_orders: Mutex<Vec<CompletedOrder>>,
    /// Enriched order info from CCP exec reports (order_id -> RichOrderInfo).
    order_cache: Mutex<HashMap<u64, RichOrderInfo>>,
    /// Orders that reached a terminal state, and when. The cache row is evicted
    /// when an order completes, so the cached status alone cannot say an order
    /// is done — a replayed frame would find nothing to refuse and insert it as
    /// open.
    completed: Mutex<HashMap<u64, Instant>>,
    /// Set when the server finishes naming the orders already working, which
    /// it does unprompted after a connect. Until then "none" and "not yet
    /// told" look the same to a caller.
    replay_done: AtomicBool,
    /// Reason for a genuinely-Inactive (39=I) transition: (order_id, ibapi
    /// error code, message). ibapi has no callback dedicated to "order
    /// parked with reason", so this is drained into `Wrapper::error` the
    /// same way a cancel/modify reject is (ibx#250).
    order_inactive: Mutex<Vec<(u64, i32, String)>>,
}

/// How long a completion is remembered.
///
/// Held by age rather than by count. What this has to outlive is the window in
/// which a stale frame for the order can still arrive — a reconnect replays
/// recent activity within seconds — and that window is a duration, not a number
/// of orders. Counting instead meant a busy session's unrelated completions
/// pushed a still-relevant one out, and the replay it was there to refuse got
/// back in.
const COMPLETED_RETENTION: Duration = Duration::from_secs(300);

/// Hard cap on how many completions are remembered at once, regardless of
/// age. Expired entries are pruned first; a session that completes orders
/// faster than they expire would otherwise leave every young entry in
/// place, so once pruning alone cannot bring the map back under this bound,
/// the oldest survivors are evicted until it does. Generous enough that
/// reaching it at all means completions are arriving far faster than any
/// legitimate replay could still be racing the ones being dropped.
const COMPLETED_MAX: usize = 65_536;

impl OrderState {
    fn new() -> Self {
        Self {
            fills: Mutex::new(Vec::with_capacity(64)),
            order_updates: Mutex::new(Vec::with_capacity(64)),
            cancel_rejects: Mutex::new(Vec::with_capacity(16)),
            what_if_responses: Mutex::new(Vec::with_capacity(8)),
            completed_orders: Mutex::new(Vec::with_capacity(64)),
            order_cache: Mutex::new(HashMap::new()),
            completed: Mutex::new(HashMap::new()),
            replay_done: AtomicBool::new(false),
            order_inactive: Mutex::new(Vec::with_capacity(8)),
        }
    }

    pub fn drain_fills(&self) -> Vec<Fill> {
        self.fills.lock().unwrap().drain(..).collect()
    }

    /// Take the queued statuses, dropping any that the order has already moved
    /// past.
    ///
    /// A working status queued a moment before the fill is still in here when
    /// the fill is delivered, and the fill is delivered first — so handing this
    /// queue over untouched reported a filled order as working with nothing
    /// filled. Seen on every market order against a paper account. The check
    /// belongs here rather than on the way in, because on the way in the order
    /// genuinely had not finished yet.
    pub fn drain_order_updates(&self) -> Vec<OrderUpdate> {
        let queued: Vec<OrderUpdate> = self.order_updates.lock().unwrap().drain(..).collect();
        queued
            .into_iter()
            .filter(|u| {
                u.status.is_terminal()
                    || u.status == crate::types::OrderStatus::Uncertain
                    || !self.recently_completed(u.order_id)
            })
            .collect()
    }

    pub fn drain_cancel_rejects(&self) -> Vec<CancelReject> {
        self.cancel_rejects.lock().unwrap().drain(..).collect()
    }

    /// Drain reasons for genuinely-Inactive (39=I) transitions, each as
    /// (order_id, ibapi error code, message) — see `order_inactive` (ibx#250).
    pub fn drain_order_inactive(&self) -> Vec<(u64, i32, String)> {
        self.order_inactive.lock().unwrap().drain(..).collect()
    }

    pub fn drain_what_if_responses(&self) -> Vec<WhatIfResponse> {
        self.what_if_responses.lock().unwrap().drain(..).collect()
    }

    pub fn drain_completed_orders(&self) -> Vec<CompletedOrder> {
        self.completed_orders.lock().unwrap().drain(..).collect()
    }

    /// Snapshot enriched entries that belong in the open-order book: a
    /// genuinely open IB state, or a genuinely-Inactive (39=I) order that can
    /// still reactivate. A rejected order also stringifies to "Inactive"
    /// (ibapi has no Rejected string) but always carries a non-empty
    /// `completed_status`, which is how the two are told apart — see
    /// `is_open_or_reactivatable` (ibx#250). Terminal entries (Filled /
    /// Cancelled / Rejected) are filtered out so `req_open_orders` does not
    /// leak historical orders that are still cached for `req_completed_orders`
    /// lookups.
    pub fn drain_open_orders(&self) -> Vec<(u64, RichOrderInfo)> {
        let lock = self.order_cache.lock().unwrap();
        lock.iter()
            .filter(|(_, v)| crate::client_core::is_open_or_reactivatable(
                &v.order_state.status, &v.order_state.completed_status))
            .map(|(&k, v)| (k, v.clone()))
            .collect()
    }

    /// Get enriched order info by order_id.
    pub fn get_order_info(&self, order_id: u64) -> Option<RichOrderInfo> {
        self.order_cache.lock().unwrap().get(&order_id).cloned()
    }

    /// Remove an enriched entry. Called after a completed order has been
    /// delivered to the user, to bound `order_cache` growth in long sessions.
    pub fn remove_order_info(&self, order_id: u64) {
        self.order_cache.lock().unwrap().remove(&order_id);
    }

    // ── Hot-loop-side writers ──

    #[doc(hidden)] pub fn push_fill(&self, fill: Fill) {
        self.fills.lock().unwrap().push(fill);
    }

    #[doc(hidden)] pub fn push_order_update(&self, update: OrderUpdate) {
        self.order_updates.lock().unwrap().push(update);
    }

    #[doc(hidden)] pub fn push_cancel_reject(&self, reject: CancelReject) {
        self.cancel_rejects.lock().unwrap().push(reject);
    }

    #[doc(hidden)] pub fn push_order_inactive(&self, order_id: u64, code: i32, message: String) {
        self.order_inactive.lock().unwrap().push((order_id, code, message));
    }

    #[doc(hidden)] pub fn push_what_if(&self, response: WhatIfResponse) {
        self.what_if_responses.lock().unwrap().push(response);
    }

    /// The server has finished naming what is already working.
    #[doc(hidden)] pub fn set_replay_done(&self) {
        self.replay_done.store(true, Ordering::Release);
    }

    /// Whether the orders already working have been received.
    pub fn replay_done(&self) -> bool {
        self.replay_done.load(Ordering::Acquire)
    }

    #[doc(hidden)] pub fn push_completed_order(&self, order: CompletedOrder) {
        {
            let now = Instant::now();
            let mut completed = self.completed.lock().unwrap();
            completed.insert(order.order_id, now);
            // Pruned here rather than on every read: this runs once per order,
            // and a read is on the message path.
            if completed.len() > COMPLETED_MAX {
                completed.retain(|_, at| now.duration_since(*at) < COMPLETED_RETENTION);
            }
            // A burst faster than the retention window leaves nothing expired
            // for `retain` to find, so the map can still be over the cap here.
            // Evict the oldest survivors until it isn't — the actual bound,
            // not just the common case.
            if completed.len() > COMPLETED_MAX {
                let mut by_age: Vec<(u64, Instant)> = completed.iter().map(|(&id, &at)| (id, at)).collect();
                by_age.sort_unstable_by_key(|&(_, at)| at);
                for (id, _) in by_age.into_iter().take(completed.len() - COMPLETED_MAX) {
                    completed.remove(&id);
                }
            }
        }
        self.completed_orders.lock().unwrap().push(order);
    }

    /// Whether this order completed recently enough that a frame reopening it
    /// is a replay rather than news.
    pub(crate) fn recently_completed(&self, order_id: u64) -> bool {
        self.completed.lock().unwrap().get(&order_id)
            .is_some_and(|at| at.elapsed() < COMPLETED_RETENTION)
    }

    /// Whether a status ends an order's life.
    ///
    /// These are the three the engine acts on by removing the order from its
    /// book. `Inactive` is not among them — it returns to working when the
    /// condition holding the order clears — and neither is `Uncertain`, which
    /// states the opposite of a conclusion.
    fn is_terminal_status(status: &str) -> bool {
        matches!(status, "Filled" | "Cancelled" | "Rejected")
    }

    /// Cache the enriched view of an order.
    ///
    /// An order that has completed is not returned to a working status. Nothing
    /// remembered that an order was done, so a replayed frame — the reconnect
    /// open-order burst racing a fill, or any message the gateway resends —
    /// wrote `Submitted` over the terminal entry, and `req_open_orders` then
    /// reported a completed order as live (ibx#262).
    ///
    /// The cached status alone cannot carry that knowledge, because completing
    /// an order evicts its cache row: the replayed frame finds nothing to refuse
    /// and inserts itself. The completed-id memory is what survives the
    /// eviction, and an intervening terminal report cannot overwrite the
    /// evidence the way a cached string could.
    ///
    /// A correction from the gateway is not a replay and goes through
    /// [`push_order_correction`](Self::push_order_correction).
    #[doc(hidden)] pub fn push_order_info(&self, order_id: u64, info: RichOrderInfo) {
        if crate::client_core::is_open_status(&info.order_state.status) {
            if self.recently_completed(order_id) {
                return;
            }
            let cache = self.order_cache.lock().unwrap();
            if cache.get(&order_id)
                .is_some_and(|e| Self::is_terminal_status(&e.order_state.status))
            {
                return;
            }
        }
        self.order_cache.lock().unwrap().insert(order_id, info);
    }

    /// Cache a view that supersedes a completed one.
    ///
    /// A trade cancel or trade correction restates an execution the gateway has
    /// already reported, so it can legitimately return a filled order to a
    /// working quantity. That is the gateway's own statement rather than a
    /// replay of an older one, so it is not refused, and the order stops being
    /// remembered as completed.
    #[doc(hidden)] pub fn push_order_correction(&self, order_id: u64, info: RichOrderInfo) {
        self.completed.lock().unwrap().remove(&order_id);
        self.order_cache.lock().unwrap().insert(order_id, info);
    }
}

/// Historical data, contract definitions, scanners, news archives, market rules, contract cache.
pub struct ReferenceState {
    historical_data: Mutex<Vec<(u32, HistoricalResponse)>>,
    head_timestamps: Mutex<Vec<(u32, HeadTimestampResponse)>>,
    /// Set while the smart-component table is this client's own rather than
    /// the venue's.
    smart_components_provisional: AtomicBool,
    contract_details: Mutex<Vec<(u32, ContractDefinition)>>,
    contract_details_end: Mutex<Vec<u32>>,
    matching_symbols: Mutex<Vec<(u32, Vec<SymbolMatch>)>>,
    /// The calendar's answers, as the venue wrote them. Two shapes on one
    /// envelope — what event types exist, and the events themselves — kept
    /// apart so a caller waiting on one is not handed the other.
    calendar_meta_data: Mutex<Vec<(u32, String)>>,
    calendar_events: Mutex<Vec<(u32, String)>>,
    /// A whole option chain answer: the underlying's conId, and one entry per
    /// scope the venue listed. The list is what the dispatcher reports before
    /// ending the request, so an empty one still ends it.
    option_params: Mutex<Vec<(u32, i64, Vec<OptionChainScope>)>>,
    scanner_params: Mutex<Vec<String>>,
    scanner_data: Mutex<Vec<(u32, ScannerResult)>>,
    historical_news: Mutex<Vec<(u32, Vec<NewsHeadline>, bool)>>,
    news_articles: Mutex<Vec<(u32, i32, String)>>,
    fundamental_data: Mutex<Vec<(u32, String)>>,
    histogram_data: Mutex<Vec<(u32, Vec<HistogramEntry>)>>,
    historical_ticks: Mutex<Vec<(u32, HistoricalTickData, String, bool)>>,
    historical_schedules: Mutex<Vec<(u32, HistoricalScheduleResponse)>>,
    /// Errors surfaced by HMDS for in-flight reference queries (req_id, code, message).
    /// Drained by the dispatcher and forwarded to `Wrapper::error`. ibx#186.
    historical_errors: Mutex<Vec<(u32, i32, String)>>,
    market_rules: Mutex<Vec<MarketRule>>,
    depth_exchanges_cache: Mutex<Vec<DepthMktDataDescription>>,
    /// Contract id → the venues SMART routes that contract to, as its own
    /// definition states them.
    smart_venues: Mutex<HashMap<i64, Vec<String>>>,
    depth_exchanges_pending: Mutex<bool>,
    /// Contract cache from CCP exec reports (con_id -> api::Contract).
    contract_cache: Mutex<HashMap<i64, api::Contract>>,
    /// Gateway-local init data (populated during connection, read-only after).
    smart_components: Mutex<Vec<crate::types::SmartComponent>>,
    news_providers: Mutex<Vec<crate::types::NewsProvider>>,
    soft_dollar_tiers: Mutex<Vec<crate::types::SoftDollarTier>>,
    family_codes: Mutex<Vec<crate::types::FamilyCode>>,
    white_branding_id: Mutex<String>,
    /// Session ID surfaced to webapp REST clients as `x-ccp-session-id`.
    ccp_session_id: Mutex<String>,
    /// Logical-name → host URL map pushed by the gateway during logon.
    misc_urls: Mutex<HashMap<String, String>>,
    /// Security type → the order types the venue permits for it, from logon tag 6652.
    order_permissions: Mutex<HashMap<String, Vec<String>>>,
    /// Feature tokens the venue enables for this account, from logon tag 6542
    /// and from the account configuration that follows it.
    enabled_features: Mutex<Vec<String>>,
    /// Whether the venue granted the older spelling of Nasdaq. Settled when
    /// the grants are, because a contract definition is parsed under it and a
    /// lock and a scan per definition is not what that path is for.
    island_granted: AtomicBool,
    /// Which algorithms the venue offers, by provider and security type.
    algorithms: Mutex<HashMap<String, Vec<String>>>,
}

impl ReferenceState {
    fn new() -> Self {
        Self {
            historical_data: Mutex::new(Vec::with_capacity(16)),
            head_timestamps: Mutex::new(Vec::with_capacity(8)),
            smart_components_provisional: AtomicBool::new(false),
            contract_details: Mutex::new(Vec::with_capacity(16)),
            contract_details_end: Mutex::new(Vec::with_capacity(8)),
            matching_symbols: Mutex::new(Vec::with_capacity(8)),
            calendar_meta_data: Mutex::new(Vec::new()),
            calendar_events: Mutex::new(Vec::new()),
            option_params: Mutex::new(Vec::with_capacity(4)),
            scanner_params: Mutex::new(Vec::new()),
            scanner_data: Mutex::new(Vec::with_capacity(8)),
            historical_news: Mutex::new(Vec::with_capacity(8)),
            news_articles: Mutex::new(Vec::with_capacity(8)),
            fundamental_data: Mutex::new(Vec::with_capacity(4)),
            histogram_data: Mutex::new(Vec::with_capacity(4)),
            historical_ticks: Mutex::new(Vec::with_capacity(4)),
            historical_schedules: Mutex::new(Vec::with_capacity(4)),
            historical_errors: Mutex::new(Vec::with_capacity(4)),
            market_rules: Mutex::new(Vec::new()),
            depth_exchanges_cache: Mutex::new(Vec::new()),
            smart_venues: Mutex::new(HashMap::new()),
            depth_exchanges_pending: Mutex::new(false),
            contract_cache: Mutex::new(HashMap::new()),
            smart_components: Mutex::new(Vec::new()),
            news_providers: Mutex::new(Vec::new()),
            soft_dollar_tiers: Mutex::new(Vec::new()),
            family_codes: Mutex::new(Vec::new()),
            white_branding_id: Mutex::new(String::new()),
            ccp_session_id: Mutex::new(String::new()),
            misc_urls: Mutex::new(HashMap::new()),
            order_permissions: Mutex::new(HashMap::new()),
            enabled_features: Mutex::new(Vec::new()),
            island_granted: AtomicBool::new(false),
            algorithms: Mutex::new(HashMap::new()),
        }
    }

    pub fn drain_historical_data(&self) -> Vec<(u32, HistoricalResponse)> {
        self.historical_data.lock().unwrap().drain(..).collect()
    }

    pub fn drain_head_timestamps(&self) -> Vec<(u32, HeadTimestampResponse)> {
        self.head_timestamps.lock().unwrap().drain(..).collect()
    }

    pub fn drain_contract_details(&self) -> Vec<(u32, ContractDefinition)> {
        self.contract_details.lock().unwrap().drain(..).collect()
    }

    /// Whether the smart-component table came from the venue or is this
    /// client's own list.
    ///
    /// The bit numbers in it decide which exchange a quote's bid, ask and last
    /// are attributed to. The venue assigns them; a list written here can only
    /// guess, and a guess that renders confidently is indistinguishable from
    /// knowledge.
    pub fn smart_components_are_provisional(&self) -> bool {
        self.smart_components_provisional.load(Ordering::Relaxed)
    }

    pub fn note_smart_components_provisional(&self, provisional: bool) {
        self.smart_components_provisional
            .store(provisional, Ordering::Relaxed);
    }

    /// The definitions a dispatch loop should deliver, leaving an answering
    /// call's own where that call will find them.
    pub fn drain_contract_details_for_dispatch(&self) -> Vec<(u32, ContractDefinition)> {
        Self::drain_dispatchable(&self.contract_details)
    }

    pub fn drain_historical_data_for_dispatch(&self) -> Vec<(u32, HistoricalResponse)> {
        Self::drain_dispatchable(&self.historical_data)
    }

    pub fn drain_head_timestamps_for_dispatch(&self) -> Vec<(u32, HeadTimestampResponse)> {
        Self::drain_dispatchable(&self.head_timestamps)
    }

    pub fn drain_calendar_meta_data_for_dispatch(&self) -> Vec<(u32, String)> {
        Self::drain_dispatchable(&self.calendar_meta_data)
    }

    pub fn drain_calendar_events_for_dispatch(&self) -> Vec<(u32, String)> {
        Self::drain_dispatchable(&self.calendar_events)
    }

    pub fn drain_matching_symbols_for_dispatch(&self) -> Vec<(u32, Vec<SymbolMatch>)> {
        Self::drain_dispatchable(&self.matching_symbols)
    }

    pub fn drain_histogram_data_for_dispatch(&self) -> Vec<(u32, Vec<HistogramEntry>)> {
        Self::drain_dispatchable(&self.histogram_data)
    }

    pub fn drain_fundamental_data_for_dispatch(&self) -> Vec<(u32, String)> {
        Self::drain_dispatchable(&self.fundamental_data)
    }

    pub fn drain_historical_schedules_for_dispatch(&self) -> Vec<(u32, HistoricalScheduleResponse)> {
        Self::drain_dispatchable(&self.historical_schedules)
    }

    pub fn drain_contract_details_end_for_dispatch(&self) -> Vec<u32> {
        let mut g = self.contract_details_end.lock().unwrap();
        let mut out = Vec::new();
        let mut i = 0;
        while i < g.len() {
            if Self::is_ask_id(g[i]) { i += 1; } else { out.push(g.remove(i)); }
        }
        out
    }

    pub fn drain_historical_errors_for_dispatch(&self) -> Vec<(u32, i32, String)> {
        let mut g = self.historical_errors.lock().unwrap();
        let mut out = Vec::new();
        let mut i = 0;
        while i < g.len() {
            if Self::is_ask_id(g[i].0) { i += 1; } else { out.push(g.remove(i)); }
        }
        out
    }

    /// The first id this client's own answering calls ask under.
    ///
    /// Far above what a caller is likely to use, so an answer to one of these
    /// is never mistaken for an answer to theirs — and, more importantly, so a
    /// dispatch loop can tell them apart from a caller's own requests and leave
    /// them where the waiting call will find them.
    pub const ASK_ID_BASE: u32 = 0x3000_0000;

    /// Whether a request id belongs to one of this client's answering calls.
    pub fn is_ask_id(req_id: u32) -> bool {
        req_id >= Self::ASK_ID_BASE
    }

    /// Drain what a dispatch loop should deliver, leaving behind what a waiting
    /// answering call is going to take.
    ///
    /// Only for a dispatch loop whose answering calls take their replies out of
    /// these queues by id. A dispatch loop that *is* how its answering calls
    /// receive must use the plain drain, or it withholds from itself — which is
    /// what happened, and no offline test could see it, because the queues were
    /// filled by hand rather than by a venue.
    pub fn drain_dispatchable<T>(q: &Mutex<Vec<(u32, T)>>) -> Vec<(u32, T)> {
        let mut g = q.lock().unwrap();
        let mut out = Vec::new();
        let mut i = 0;
        while i < g.len() {
            if Self::is_ask_id(g[i].0) { i += 1; } else { out.push(g.remove(i)); }
        }
        out
    }

    /// Take the one answer belonging to a request, leaving the rest.
    fn take_one<T>(q: &Mutex<Vec<(u32, T)>>, req_id: u32) -> Option<T> {
        let mut g = q.lock().unwrap();
        let at = g.iter().position(|(id, _)| *id == req_id)?;
        Some(g.remove(at).1)
    }

    /// Bars answering one request. The venue may answer in several parts, so
    /// this takes every part waiting and the caller stops on the one that says
    /// it is the last.
    pub fn take_historical_for(&self, req_id: u32) -> Vec<HistoricalResponse> {
        let mut q = self.historical_data.lock().unwrap();
        let mut mine = Vec::new();
        let mut i = 0;
        while i < q.len() {
            if q[i].0 == req_id { mine.push(q.remove(i).1); } else { i += 1; }
        }
        mine
    }

    pub fn take_head_timestamp_for(&self, req_id: u32) -> Option<HeadTimestampResponse> {
        Self::take_one(&self.head_timestamps, req_id)
    }

    pub fn take_matching_symbols_for(&self, req_id: u32) -> Option<Vec<SymbolMatch>> {
        Self::take_one(&self.matching_symbols, req_id)
    }

    /// The option chains answered for one request.
    pub fn take_option_params_for(&self, req_id: u32) -> Option<(i64, Vec<OptionChainScope>)> {
        let mut held = self.option_params.lock().unwrap();
        let at = held.iter().position(|(id, ..)| *id == req_id)?;
        let (_, underlying, scopes) = held.remove(at);
        Some((underlying, scopes))
    }

    pub fn take_histogram_for(&self, req_id: u32) -> Option<Vec<HistogramEntry>> {
        Self::take_one(&self.histogram_data, req_id)
    }

    pub fn take_fundamental_for(&self, req_id: u32) -> Option<String> {
        Self::take_one(&self.fundamental_data, req_id)
    }

    pub fn take_historical_schedule_for(&self, req_id: u32) -> Option<HistoricalScheduleResponse> {
        Self::take_one(&self.historical_schedules, req_id)
    }

    /// Take only the definitions answering one request, leaving every other
    /// request's alone.
    ///
    /// The plain drain empties the queue for whoever calls it first. A caller
    /// asking one question needs its own answer without swallowing the answers
    /// belonging to a dispatch loop running beside it.
    pub fn take_contract_details_for(&self, req_id: u32) -> Vec<ContractDefinition> {
        let mut q = self.contract_details.lock().unwrap();
        let mut mine = Vec::new();
        q.retain(|(id, def)| {
            if *id == req_id { mine.push(def.clone()); false } else { true }
        });
        mine
    }

    /// Whether the venue has said it has no more to say about one request.
    pub fn take_contract_details_end_for(&self, req_id: u32) -> bool {
        let mut q = self.contract_details_end.lock().unwrap();
        let before = q.len();
        q.retain(|id| *id != req_id);
        q.len() != before
    }

    /// The venue's own words about one request, if it refused it.
    pub fn take_error_for(&self, req_id: u32) -> Option<(i32, String)> {
        let mut q = self.historical_errors.lock().unwrap();
        let at = q.iter().position(|(id, _, _)| *id == req_id)?;
        let (_, code, msg) = q.remove(at);
        Some((code, msg))
    }

    pub fn drain_contract_details_end(&self) -> Vec<u32> {
        self.contract_details_end.lock().unwrap().drain(..).collect()
    }

    pub fn drain_calendar_meta_data(&self) -> Vec<(u32, String)> {
        self.calendar_meta_data.lock().unwrap().drain(..).collect()
    }

    pub fn drain_calendar_events(&self) -> Vec<(u32, String)> {
        self.calendar_events.lock().unwrap().drain(..).collect()
    }

    pub fn drain_matching_symbols(&self) -> Vec<(u32, Vec<SymbolMatch>)> {
        self.matching_symbols.lock().unwrap().drain(..).collect()
    }

    pub fn drain_option_params(&self) -> Vec<(u32, i64, Vec<OptionChainScope>)> {
        self.option_params.lock().unwrap().drain(..).collect()
    }

    /// The chains meant for a callback, leaving those a caller is waiting on.
    ///
    /// Draining everything hands an answering call's own answer to the
    /// callback pump instead, and the call then waits out its timeout for
    /// something that has already been delivered somewhere else.
    pub fn drain_option_params_for_dispatch(&self) -> Vec<(u32, i64, Vec<OptionChainScope>)> {
        let mut held = self.option_params.lock().unwrap();
        let mut out = Vec::new();
        let mut i = 0;
        while i < held.len() {
            if Self::is_ask_id(held[i].0) {
                i += 1;
            } else {
                out.push(held.remove(i));
            }
        }
        out
    }

    pub fn drain_scanner_params(&self) -> Vec<String> {
        self.scanner_params.lock().unwrap().drain(..).collect()
    }

    pub fn drain_scanner_data(&self) -> Vec<(u32, ScannerResult)> {
        self.scanner_data.lock().unwrap().drain(..).collect()
    }

    pub fn drain_historical_news(&self) -> Vec<(u32, Vec<NewsHeadline>, bool)> {
        self.historical_news.lock().unwrap().drain(..).collect()
    }

    /// The headlines answering one request, leaving anything a dispatch loop
    /// is going to deliver where it is.
    pub fn take_historical_news_for(&self, req_id: u32) -> Option<(Vec<NewsHeadline>, bool)> {
        let mut held = self.historical_news.lock().unwrap();
        let at = held.iter().position(|(id, ..)| *id == req_id)?;
        let (_, headlines, has_more) = held.remove(at);
        Some((headlines, has_more))
    }

    /// What a dispatch loop should deliver, leaving what an answering call is
    /// waiting to take.
    pub fn drain_historical_news_for_dispatch(&self) -> Vec<(u32, Vec<NewsHeadline>, bool)> {
        let mut held = self.historical_news.lock().unwrap();
        let mut out = Vec::new();
        let mut i = 0;
        while i < held.len() {
            if Self::is_ask_id(held[i].0) {
                i += 1;
            } else {
                out.push(held.remove(i));
            }
        }
        out
    }

    pub fn drain_news_articles(&self) -> Vec<(u32, i32, String)> {
        self.news_articles.lock().unwrap().drain(..).collect()
    }

    pub fn drain_fundamental_data(&self) -> Vec<(u32, String)> {
        self.fundamental_data.lock().unwrap().drain(..).collect()
    }

    pub fn drain_histogram_data(&self) -> Vec<(u32, Vec<HistogramEntry>)> {
        self.histogram_data.lock().unwrap().drain(..).collect()
    }

    pub fn drain_historical_ticks(&self) -> Vec<(u32, HistoricalTickData, String, bool)> {
        self.historical_ticks.lock().unwrap().drain(..).collect()
    }

    pub fn drain_historical_schedules(&self) -> Vec<(u32, HistoricalScheduleResponse)> {
        self.historical_schedules.lock().unwrap().drain(..).collect()
    }

    pub fn drain_historical_errors(&self) -> Vec<(u32, i32, String)> {
        self.historical_errors.lock().unwrap().drain(..).collect()
    }

    /// Get cached market rules.
    pub fn market_rules(&self) -> Vec<MarketRule> {
        self.market_rules.lock().unwrap().clone()
    }

    /// Get a market rule by ID.
    pub fn market_rule(&self, rule_id: i32) -> Option<MarketRule> {
        self.market_rules.lock().unwrap().iter().find(|r| r.rule_id == rule_id).cloned()
    }

    /// Get cached contract by con_id.
    pub fn get_contract(&self, con_id: i64) -> Option<api::Contract> {
        self.contract_cache.lock().unwrap().get(&con_id).cloned()
    }

    // ── Hot-loop-side writers ──

    #[doc(hidden)] pub fn push_historical_data(&self, req_id: u32, response: HistoricalResponse) {
        self.historical_data.lock().unwrap().push((req_id, response));
    }

    #[doc(hidden)] pub fn push_head_timestamp(&self, req_id: u32, response: HeadTimestampResponse) {
        self.head_timestamps.lock().unwrap().push((req_id, response));
    }

    #[doc(hidden)] pub fn push_contract_details(&self, req_id: u32, def: ContractDefinition) {
        self.contract_details.lock().unwrap().push((req_id, def));
    }

    #[doc(hidden)] pub fn push_contract_details_end(&self, req_id: u32) {
        self.contract_details_end.lock().unwrap().push(req_id);
    }

    #[doc(hidden)] pub fn push_calendar_meta_data(&self, req_id: u32, json: String) {
        self.calendar_meta_data.lock().unwrap().push((req_id, json));
    }

    #[doc(hidden)] pub fn push_calendar_events(&self, req_id: u32, json: String) {
        self.calendar_events.lock().unwrap().push((req_id, json));
    }

    #[doc(hidden)] pub fn push_matching_symbols(&self, req_id: u32, matches: Vec<SymbolMatch>) {
        self.matching_symbols.lock().unwrap().push((req_id, matches));
    }

    #[doc(hidden)] pub fn push_option_params(&self, req_id: u32, underlying_con_id: i64, scopes: Vec<OptionChainScope>) {
        self.option_params.lock().unwrap().push((req_id, underlying_con_id, scopes));
    }

    #[doc(hidden)] pub fn push_scanner_params(&self, xml: String) {
        self.scanner_params.lock().unwrap().push(xml);
    }

    #[doc(hidden)] pub fn push_scanner_data(&self, req_id: u32, result: ScannerResult) {
        self.scanner_data.lock().unwrap().push((req_id, result));
    }

    #[doc(hidden)] pub fn push_historical_news(&self, req_id: u32, headlines: Vec<NewsHeadline>, has_more: bool) {
        self.historical_news.lock().unwrap().push((req_id, headlines, has_more));
    }

    #[doc(hidden)] pub fn push_news_article(&self, req_id: u32, article_type: i32, article_text: String) {
        self.news_articles.lock().unwrap().push((req_id, article_type, article_text));
    }

    #[doc(hidden)] pub fn push_fundamental_data(&self, req_id: u32, data: String) {
        self.fundamental_data.lock().unwrap().push((req_id, data));
    }

    #[doc(hidden)] pub fn push_histogram_data(&self, req_id: u32, entries: Vec<HistogramEntry>) {
        self.histogram_data.lock().unwrap().push((req_id, entries));
    }

    #[doc(hidden)] pub fn push_historical_ticks(&self, req_id: u32, data: HistoricalTickData, what_to_show: String, done: bool) {
        self.historical_ticks.lock().unwrap().push((req_id, data, what_to_show, done));
    }

    #[doc(hidden)] pub fn push_historical_schedule(&self, req_id: u32, response: HistoricalScheduleResponse) {
        self.historical_schedules.lock().unwrap().push((req_id, response));
    }

    #[doc(hidden)] pub fn push_historical_error(&self, req_id: u32, code: i32, message: String) {
        self.historical_errors.lock().unwrap().push((req_id, code, message));
    }

    #[doc(hidden)] pub fn push_market_rules(&self, rules: Vec<MarketRule>) {
        let mut lock = self.market_rules.lock().unwrap();
        for rule in rules {
            if !lock.iter().any(|r| r.rule_id == rule.rule_id) {
                lock.push(rule);
            }
        }
    }

    pub fn drain_depth_exchanges(&self) -> Vec<DepthMktDataDescription> {
        let mut pending = self.depth_exchanges_pending.lock().unwrap();
        if *pending {
            *pending = false;
            self.depth_exchanges_cache.lock().unwrap().clone()
        } else {
            Vec::new()
        }
    }

    /// Every exchange the venue named at logon, as it named them.
    ///
    /// Read rather than drained: a caller reading the list must not empty it.
    pub fn depth_exchanges(&self) -> Vec<DepthMktDataDescription> {
        self.depth_exchanges_cache.lock().unwrap().clone()
    }

    /// The venues SMART routes a contract to, which is what its aggregate book
    /// is made of. Stated on the contract's own definition, so it is per
    /// contract rather than per market.
    pub fn smart_venues(&self, con_id: i64) -> Vec<String> {
        self.smart_venues.lock().unwrap().get(&con_id).cloned().unwrap_or_default()
    }

    #[doc(hidden)] pub fn set_smart_venues(&self, con_id: i64, venues: Vec<String>) {
        if venues.is_empty() {
            return;
        }
        self.smart_venues.lock().unwrap().insert(con_id, venues);
    }

    #[doc(hidden)] pub fn push_depth_exchanges(&self, descs: Vec<DepthMktDataDescription>) {
        self.depth_exchanges_cache.lock().unwrap().extend(descs);
    }

    #[doc(hidden)] pub fn notify_depth_exchanges(&self) {
        *self.depth_exchanges_pending.lock().unwrap() = true;
    }

    #[doc(hidden)] pub fn cache_contract(&self, con_id: i64, contract: api::Contract) {
        let mut cache = self.contract_cache.lock().unwrap();
        if let Some(existing) = cache.get_mut(&con_id) {
            // Merge: only overwrite fields that are non-empty in the new contract
            if !contract.symbol.is_empty() { existing.symbol = contract.symbol; }
            if !contract.sec_type.is_empty() { existing.sec_type = contract.sec_type; }
            if !contract.exchange.is_empty() { existing.exchange = contract.exchange; }
            if !contract.currency.is_empty() { existing.currency = contract.currency; }
            if !contract.local_symbol.is_empty() { existing.local_symbol = contract.local_symbol; }
            if !contract.primary_exchange.is_empty() { existing.primary_exchange = contract.primary_exchange; }
            if !contract.trading_class.is_empty() { existing.trading_class = contract.trading_class; }
        } else {
            cache.insert(con_id, contract);
        }
    }

    // ── Gateway-local init data ──

    pub fn smart_components(&self) -> Vec<crate::types::SmartComponent> {
        self.smart_components.lock().unwrap().clone()
    }

    pub fn news_providers(&self) -> Vec<crate::types::NewsProvider> {
        self.news_providers.lock().unwrap().clone()
    }

    pub fn soft_dollar_tiers(&self) -> Vec<crate::types::SoftDollarTier> {
        self.soft_dollar_tiers.lock().unwrap().clone()
    }

    pub fn family_codes(&self) -> Vec<crate::types::FamilyCode> {
        self.family_codes.lock().unwrap().clone()
    }

    pub fn white_branding_id(&self) -> String {
        self.white_branding_id.lock().unwrap().clone()
    }

    /// Session ID surfaced to webapp REST clients as the `x-ccp-session-id` header.
    /// Empty until gateway logon completes.
    pub fn ccp_session_id(&self) -> String {
        self.ccp_session_id.lock().unwrap().clone()
    }

    /// Logical-name → host URL map pushed by the gateway during logon. Empty when
    /// no URL set was pushed; consumers should fall back to a documented literal
    /// (e.g. `api.ibkr.com` for `region_dam`).
    pub fn misc_urls(&self) -> HashMap<String, String> {
        self.misc_urls.lock().unwrap().clone()
    }

    /// Single lookup against the URL map. Returns `None` when missing.
    pub fn misc_url(&self, key: &str) -> Option<String> {
        self.misc_urls.lock().unwrap().get(key).cloned()
    }

    /// Security type → the order types the venue permits for it. Stated by the
    /// venue at logon; empty until logon completes.
    pub fn order_permissions(&self) -> HashMap<String, Vec<String>> {
        self.order_permissions.lock().unwrap().clone()
    }

    /// The order types permitted for one security type, or `None` when the venue
    /// does not permit the type at all. A combination is named `COMB`.
    pub fn permitted_order_types(&self, sec_type: &str) -> Option<Vec<String>> {
        let key = if matches!(sec_type, "BAG" | "COMBO") { "COMB" } else { sec_type };
        self.order_permissions.lock().unwrap().get(key).cloned()
    }

    /// Feature tokens the venue enables for this account.
    pub fn enabled_features(&self) -> Vec<String> {
        self.enabled_features.lock().unwrap().clone()
    }

    /// Which algorithms the venue offers, keyed `PROVIDER/SECTYPE`.
    ///
    /// The venue states this on the session; it is not a property of a
    /// contract. An algorithm absent here is one this account may not use.
    pub fn algorithms(&self) -> HashMap<String, Vec<String>> {
        self.algorithms.lock().unwrap().clone()
    }

    /// The algorithms offered for one security type, across every provider.
    pub fn algorithms_for(&self, sec_type: &str) -> Vec<String> {
        let want = format!("/{}", sec_type.to_ascii_uppercase());
        let mut out: Vec<String> = self.algorithms.lock().unwrap()
            .iter()
            .filter(|(k, _)| k.to_ascii_uppercase().ends_with(&want))
            .flat_map(|(_, v)| v.iter().cloned())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    #[doc(hidden)] pub fn set_algorithms(&self, algorithms: HashMap<String, Vec<String>>) {
        *self.algorithms.lock().unwrap() = algorithms;
    }

    /// Add feature tokens the venue states after logon. What logon already
    /// stated is kept; this only ever adds.
    #[doc(hidden)] pub fn add_enabled_features(&self, more: Vec<String>) {
        let mut have = self.enabled_features.lock().unwrap();
        for token in more {
            if !have.contains(&token) {
                have.push(token);
            }
        }
        self.settle_island_grant(&have);
    }

    /// The token that grants the older spelling of Nasdaq, as the counterpart
    /// reads it: off the granted list at logon, held on its own from then on.
    fn settle_island_grant(&self, granted: &[String]) {
        self.island_granted.store(
            granted.iter().any(|t| t == ISLAND_FOR_NASDAQ_GRANT),
            Ordering::Relaxed,
        );
    }

    /// Whether the venue grants the older spelling of Nasdaq to this account.
    pub fn island_granted(&self) -> bool {
        self.island_granted.load(Ordering::Relaxed)
    }

    pub fn feature_enabled(&self, token: &str) -> bool {
        self.enabled_features.lock().unwrap().iter().any(|t| t == token)
    }

    #[doc(hidden)] pub fn set_order_permissions(&self, perms: HashMap<String, Vec<String>>) {
        *self.order_permissions.lock().unwrap() = perms;
    }

    #[doc(hidden)] pub fn set_enabled_features(&self, features: Vec<String>) {
        self.settle_island_grant(&features);
        *self.enabled_features.lock().unwrap() = features;
    }

    #[doc(hidden)] pub fn set_smart_components(&self, components: Vec<crate::types::SmartComponent>) {
        *self.smart_components.lock().unwrap() = components;
    }

    #[doc(hidden)] pub fn set_news_providers(&self, providers: Vec<crate::types::NewsProvider>) {
        *self.news_providers.lock().unwrap() = providers;
    }

    #[doc(hidden)] pub fn set_soft_dollar_tiers(&self, tiers: Vec<crate::types::SoftDollarTier>) {
        *self.soft_dollar_tiers.lock().unwrap() = tiers;
    }

    #[doc(hidden)] pub fn set_family_codes(&self, codes: Vec<crate::types::FamilyCode>) {
        *self.family_codes.lock().unwrap() = codes;
    }

    #[doc(hidden)] pub fn set_white_branding_id(&self, id: String) {
        *self.white_branding_id.lock().unwrap() = id;
    }

    #[doc(hidden)] pub fn set_ccp_session_id(&self, id: String) {
        *self.ccp_session_id.lock().unwrap() = id;
    }

    #[doc(hidden)] pub fn set_misc_urls(&self, urls: HashMap<String, String>) {
        *self.misc_urls.lock().unwrap() = urls;
    }
}

/// Account snapshot, per-position info, and atomic instrument positions.
pub struct PortfolioState {
    account: Mutex<AccountState>,
    /// Every figure the venue states about the account, under the name it
    /// states it by and in the currency it states it in.
    ///
    /// A named few were read into fields and the rest were dropped where they
    /// arrived, with nothing to say they had come. The venue states a great
    /// many more than any client names, and a figure nobody named is still a
    /// figure about the account.
    stated_account_values: Mutex<Vec<(String, String, String)>>,
    /// True once the first gateway account message ("UT"/"UM"/"RL") has been received.
    account_data_received: AtomicBool,
    /// True once the CCP init burst has been fully processed.
    account_download_complete: AtomicBool,
    /// Position info (conId -> PositionInfo) for reqPositions and P&L.
    position_infos: Mutex<HashMap<i64, PositionInfo>>,
    /// Holdings the venue reports that this broker does not itself hold:
    /// positions held away, and rows it marks as shown but not held. Kept
    /// apart from the account's own, which is what a caller asking for
    /// positions means.
    positions_elsewhere: Mutex<HashMap<i64, crate::types::PositionElsewhere>>,
    /// Account figures for the holdings the account does not hold itself,
    /// keyed by which set they describe and what they are called.
    values_elsewhere: Mutex<HashMap<(crate::types::HeldElsewhere, String), String>>,
    positions: [AtomicU64; MAX_INSTRUMENTS],
    /// Midnight seeds from 6040=143 for client-side daily P&L computation.
    midnight_seeds: Mutex<HashMap<i64, MidnightSeed>>,
    /// Correlation id the venue stamped on the seeds it last sent.
    pnl_request_key: Mutex<String>,
    /// Prices the venue states per contract (6040=152), kept as it wrote them
    /// so a price that does not read as a number cannot displace the rest.
    venue_prices: Mutex<HashMap<i64, String>>,
}

impl PortfolioState {
    fn new() -> Self {
        Self {
            account: Mutex::new(AccountState::default()),
            stated_account_values: Mutex::new(Vec::new()),
            account_data_received: AtomicBool::new(false),
            account_download_complete: AtomicBool::new(false),
            position_infos: Mutex::new(HashMap::new()),
            positions_elsewhere: Mutex::new(HashMap::new()),
            values_elsewhere: Mutex::new(HashMap::new()),
            positions: std::array::from_fn(|_| AtomicU64::new(0)),
            midnight_seeds: Mutex::new(HashMap::new()),
            pnl_request_key: Mutex::new(String::new()),
            venue_prices: Mutex::new(HashMap::new()),
        }
    }

    /// Read account state snapshot.
    pub fn account(&self) -> AccountState {
        *self.account.lock().unwrap()
    }

    /// Get all position infos (for reqPositions).
    /// Holdings the venue reports that this broker does not hold itself.
    pub fn positions_elsewhere(&self) -> Vec<crate::types::PositionElsewhere> {
        self.positions_elsewhere.lock().unwrap().values().cloned().collect()
    }

    /// Record one, as the venue states it. The venue restates a row rather
    /// than withdrawing it, so this replaces what it holds for that contract.
    #[doc(hidden)] pub fn set_position_elsewhere(&self, row: crate::types::PositionElsewhere) {
        self.positions_elsewhere.lock().unwrap().insert(row.con_id, row);
    }

    /// The account figures describing one of the sets of holdings the account
    /// does not hold itself, as name and value.
    pub fn values_elsewhere(&self, held: crate::types::HeldElsewhere) -> Vec<(String, String)> {
        self.values_elsewhere.lock().unwrap()
            .iter()
            .filter(|((set, _), _)| *set == held)
            .map(|((_, name), value)| (name.clone(), value.clone()))
            .collect()
    }

    #[doc(hidden)] pub fn set_value_elsewhere(
        &self, held: crate::types::HeldElsewhere, name: String, value: String,
    ) {
        self.values_elsewhere.lock().unwrap().insert((held, name), value);
    }

    pub fn position_infos(&self) -> Vec<PositionInfo> {
        self.position_infos.lock().unwrap().values().cloned().collect()
    }

    /// Get position info for a single conId (for pnlSingle).
    pub fn position_info(&self, con_id: i64) -> Option<PositionInfo> {
        self.position_infos.lock().unwrap().get(&con_id).cloned()
    }

    /// Read current position for an instrument.
    pub fn position(&self, id: InstrumentId) -> f64 {
        // Held as bits so the slot stays lock-free; a holding is fractional and
        // a whole-number one read half a share as flat.
        f64::from_bits(self.positions[id as usize].load(Ordering::Relaxed))
    }

    // ── Hot-loop-side writers ──

    /// True once at least one gateway account message has been processed.
    pub fn account_data_received(&self) -> bool {
        self.account_data_received.load(Ordering::Acquire)
    }

    /// Record a figure the venue stated, replacing any earlier statement of
    /// the same name in the same currency.
    #[doc(hidden)]
    pub fn note_account_value(&self, key: &str, value: &str, currency: &str) {
        // A figure the venue stated is account data having arrived. Marked
        // only when the typed copy was built, a summary asked for in between
        // was answered with nothing at all.
        self.account_data_received.store(true, Ordering::Release);
        let mut all = self.stated_account_values.lock().unwrap();
        match all.iter_mut().find(|(k, _, c)| k == key && c == currency) {
            Some(slot) => slot.1 = value.to_string(),
            None => all.push((key.to_string(), value.to_string(), currency.to_string())),
        }
    }

    /// Every figure the venue has stated about the account, as it stated them.
    pub fn stated_account_values(&self) -> Vec<(String, String, String)> {
        self.stated_account_values.lock().unwrap().clone()
    }

    #[doc(hidden)] pub fn set_account(&self, account: &AccountState) {
        *self.account.lock().unwrap() = *account;
        self.account_data_received.store(true, Ordering::Release);
    }

    /// Mark account download as complete (init burst processed).
    #[doc(hidden)] pub fn set_account_download_complete(&self) {
        self.account_download_complete.store(true, Ordering::Release);
    }

    /// True once the CCP init burst has been fully processed.
    pub fn account_download_complete(&self) -> bool {
        self.account_download_complete.load(Ordering::Acquire)
    }

    #[doc(hidden)] pub fn set_position_info(&self, info: PositionInfo) {
        let mut map = self.position_infos.lock().unwrap();
        match map.get_mut(&info.con_id) {
            Some(existing) => {
                existing.position = info.position;
                // Written as given: a cost the row did not state is filled
                // in by the caller, which is the only side that can tell an
                // absent tag from a stated zero. A broker correcting a basis
                // to zero has to be able to say so.
                existing.avg_cost = info.avg_cost;
                if !info.symbol.is_empty() { existing.symbol = info.symbol; }
                if !info.sec_type.is_empty() { existing.sec_type = info.sec_type; }
                if !info.currency.is_empty() { existing.currency = info.currency; }
                if !info.multiplier.is_empty() { existing.multiplier = info.multiplier; }
                // Marks are owned by set_position_marks; leave them untouched so
                // the lean position feed can't zero them (ib-agent#172).
            }
            None => { map.insert(info.con_id, info); }
        }
    }

    /// Apply a fill of this account's own to the holding it changes.
    ///
    /// The broker states holdings on a feed of its own and does not restate
    /// them when an order of ours fills, so a holding read back during the
    /// session was the one the session started with. The terminal keeps its
    /// own count between statements and so does this.
    ///
    /// `delta` is signed by side and `price` is what the fill paid, both per
    /// unit. Adding to a holding averages the new cost in; reducing one leaves
    /// the basis where it was, because a sale realises a gain and does not
    /// re-price what remains. A holding that closes carries no basis at all.
    #[doc(hidden)] pub fn apply_fill(&self, con_id: i64, delta: f64, price: Price) {
        if con_id == 0 || delta == 0.0 {
            return;
        }
        let mut map = self.position_infos.lock().unwrap();
        let row = map.entry(con_id).or_insert_with(|| PositionInfo { con_id, ..Default::default() });
        let before = row.position;
        let after = before + delta;
        row.position = after;
        if after == 0.0 {
            row.avg_cost = 0;
        } else if before == 0.0 || (before > 0.0) == (delta > 0.0) {
            let cost = row.avg_cost as f64 * before + price as f64 * delta;
            row.avg_cost = (cost / after) as Price;
        } else if (before > 0.0) != (after > 0.0) {
            // One fill that closed the holding and opened the opposite one.
            // Keeping the old basis prices a short against what a long paid,
            // and every profit and loss read afterwards is measured from the
            // wrong side. What is held now was bought at this price.
            row.avg_cost = price;
        }
    }

    /// Update the per-position marks (from the account-updates portfolio message).
    /// Kept separate from set_position_info so the lean position feed, which has
    /// no marks, does not overwrite them (ib-agent#172).
    #[doc(hidden)] pub fn set_position_marks(&self, con_id: i64, market_price: Price, market_value: Price, unrealized_pnl: Price, realized_pnl: Price) {
        let mut map = self.position_infos.lock().unwrap();
        let entry = map.entry(con_id).or_insert_with(|| PositionInfo { con_id, ..Default::default() });
        entry.market_price = market_price;
        entry.market_value = market_value;
        entry.unrealized_pnl = unrealized_pnl;
        entry.realized_pnl = realized_pnl;
    }

    #[doc(hidden)] pub fn set_position(&self, id: InstrumentId, pos: f64) {
        self.positions[id as usize].store(pos.to_bits(), Ordering::Relaxed);
    }

    /// Store midnight seeds from 6040=143 P&L response, under the correlation
    /// id the body was stamped with.
    #[doc(hidden)] pub fn set_midnight_seeds(&self, request_key: String, seeds: Vec<MidnightSeed>) {
        let mut map = self.midnight_seeds.lock().unwrap();
        map.clear();
        for s in seeds {
            map.insert(s.con_id, s);
        }
        *self.pnl_request_key.lock().unwrap() = request_key;
    }

    /// Read midnight seeds for client-side P&L computation.
    pub fn midnight_seeds(&self) -> Vec<MidnightSeed> {
        self.midnight_seeds.lock().unwrap().values().copied().collect()
    }

    /// The correlation id carried by the seeds now held.
    pub fn pnl_request_key(&self) -> String {
        self.pnl_request_key.lock().unwrap().clone()
    }

    /// Store the prices the venue states per contract (6040=152). A later table
    /// updates the contracts it names and leaves the others as they stood.
    #[doc(hidden)] pub fn set_venue_prices(&self, prices: HashMap<i64, String>) {
        self.venue_prices.lock().unwrap().extend(prices);
    }

    /// The price the venue states for a contract, as text.
    pub fn venue_price(&self, con_id: i64) -> Option<String> {
        self.venue_prices.lock().unwrap().get(&con_id).cloned()
    }
}

/// Shared state between hot loop and external caller.
/// Composed of domain-specific containers for clear ownership boundaries.
pub struct SharedState {
    /// What the session runs under, settled when it opened.
    ///
    /// Held here so the engine reads a value rather than the process it runs
    /// in: two sessions in one process have their own, and neither can change
    /// the other's mid-flight.
    pub settings: std::sync::Mutex<std::sync::Arc<crate::api::settings::SessionSettings>>,
    pub market: MarketDataState,
    pub orders: OrderState,
    pub reference: ReferenceState,
    pub portfolio: PortfolioState,
    /// Last measured auth-connection round-trip time in nanoseconds
    /// (0 = never measured). Sampled from the test-request/echo cycle —
    /// see `HotLoop` liveness and `ControlCommand::Ping` (ibx#158).
    ccp_rtt_ns: AtomicU64,
    /// Set by the hot loop when the session is over (connection lost, engine
    /// stopped, or reconnect exhausted). Read-and-clear by the client so the
    /// `connection_closed` callback can fire without an event channel
    /// (ibx#242). The `Event::Disconnected` channel path is optional; this
    /// flag is always populated.
    connection_lost: AtomicBool,
    /// Set when a reconnect recovered a loss that was announced. Read-and-clear
    /// like `connection_lost`, so a client with no event channel still learns
    /// it is back.
    connection_restored: AtomicBool,
    /// Notifier for waking consumers (e.g. Python event loop) when data arrives.
    notify_mutex: Mutex<bool>,
    notify_condvar: Condvar,
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedState {
    /// What this session runs under.
    pub fn settings(&self) -> std::sync::Arc<crate::api::settings::SessionSettings> {
        self.settings.lock().unwrap().clone()
    }

    /// Whether a US stock trading on Nasdaq is named by the older spelling.
    ///
    /// The setting asks for it and the venue grants it, and it takes both: the
    /// counterpart reads the same grant off the granted-feature list at logon
    /// and holds it beside the setting. Read once per contract definition, so
    /// the grant is settled at logon rather than scanned for here.
    pub fn island_for_nasdaq(&self) -> bool {
        self.settings().island_for_nasdaq && self.reference.island_granted()
    }

    /// Stated once, as the session opens, before the engine's threads start.
    #[doc(hidden)]
    pub fn set_settings(&self, settings: std::sync::Arc<crate::api::settings::SessionSettings>) {
        *self.settings.lock().unwrap() = settings;
    }

    pub fn new() -> Self {
        Self {
            settings: std::sync::Mutex::new(std::sync::Arc::new(Default::default())),
            market: MarketDataState::new(),
            orders: OrderState::new(),
            reference: ReferenceState::new(),
            portfolio: PortfolioState::new(),
            ccp_rtt_ns: AtomicU64::new(0),
            connection_lost: AtomicBool::new(false),
            connection_restored: AtomicBool::new(false),
            notify_mutex: Mutex::new(false),
            notify_condvar: Condvar::new(),
        }
    }

    /// Signal that the session is over. Hot-loop side (ibx#242).
    #[doc(hidden)]
    #[inline]
    pub fn set_connection_lost(&self) {
        self.connection_lost.store(true, Ordering::Release);
        self.notify();
    }

    /// Read and clear the connection-lost flag. Returns `true` at most once per
    /// signal, so the caller can fire `connection_closed` exactly once.
    #[inline]
    pub fn take_connection_lost(&self) -> bool {
        self.connection_lost.swap(false, Ordering::AcqRel)
    }

    /// Signal that an announced loss has been recovered. Hot-loop side.
    #[doc(hidden)]
    #[inline]
    pub fn set_connection_restored(&self) {
        self.connection_restored.store(true, Ordering::Release);
        self.notify();
    }

    /// Read and clear the connection-restored flag.
    #[inline]
    pub fn take_connection_restored(&self) -> bool {
        self.connection_restored.swap(false, Ordering::AcqRel)
    }

    /// Record an auth-connection RTT sample (ibx#158). Hot-loop side.
    #[inline]
    pub fn set_ccp_rtt(&self, rtt: std::time::Duration) {
        self.ccp_rtt_ns.store(rtt.as_nanos().min(u64::MAX as u128) as u64, Ordering::Relaxed);
    }

    /// Last measured auth-connection round-trip time, if any (ibx#158).
    /// A gauge, not a benchmark: the sample is the interval from a test
    /// request to the first inbound traffic that followed it, which on an
    /// active feed can undercount by racing data already in flight.
    #[inline]
    pub fn last_ccp_rtt(&self) -> Option<std::time::Duration> {
        match self.ccp_rtt_ns.load(Ordering::Relaxed) {
            0 => None,
            ns => Some(std::time::Duration::from_nanos(ns)),
        }
    }

    /// Signal that new data is available. Called by hot loop after pushing data.
    #[inline]
    pub fn notify(&self) {
        let mut pending = self.notify_mutex.lock().unwrap();
        *pending = true;
        self.notify_condvar.notify_one();
    }

    /// Wait for data notification with a timeout. Returns true if notified, false if timed out.
    pub fn wait_for_data(&self, timeout: std::time::Duration) -> bool {
        let mut pending = self.notify_mutex.lock().unwrap();
        if *pending {
            *pending = false;
            return true;
        }
        let (lock, result) = self.notify_condvar.wait_timeout(pending, timeout).unwrap();
        let had_data = *lock;
        if had_data {
            // Reset the flag via a mutable reference obtained from the MutexGuard's deref.
            drop(lock);
            *self.notify_mutex.lock().unwrap() = false;
        }
        had_data || !result.timed_out()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seqquote_write_read_roundtrip() {
        let sq = SeqQuote::new();
        let q = Quote { bid: 150 * PRICE_SCALE, ask: 151 * PRICE_SCALE, ..Default::default() };
        sq.write(&q);
        let read = sq.read();
        assert_eq!(read.bid, 150 * PRICE_SCALE);
        assert_eq!(read.ask, 151 * PRICE_SCALE);
    }

    #[test]
    fn seqquote_default_is_zero() {
        let sq = SeqQuote::new();
        let q = sq.read();
        assert_eq!(q.bid, 0);
        assert_eq!(q.ask, 0);
    }

    #[test]
    fn order_state_drain_open_orders_admits_inactive_excludes_rejected() {
        let ss = SharedState::new();
        ss.orders.push_order_info(90, RichOrderInfo {
            contract: api::Contract::default(),
            order: api::Order::default(),
            order_state: api::OrderState { status: "Inactive".into(), ..Default::default() },
            last_exec: api::Execution::default(),
        });
        ss.orders.push_order_info(91, RichOrderInfo {
            contract: api::Contract::default(),
            order: api::Order::default(),
            order_state: api::OrderState {
                status: "Inactive".into(),
                completed_status: "No valid bid/ask".into(),
                ..Default::default()
            },
            last_exec: api::Execution::default(),
        });

        let open = ss.orders.drain_open_orders();
        assert!(open.iter().any(|(id, _)| *id == 90),
            "genuinely-inactive order must be admitted to the open-order snapshot");
        assert!(!open.iter().any(|(id, _)| *id == 91),
            "rejected order (non-empty completed_status) must not resurrect");
    }

    #[test]
    fn shared_state_fills_drain() {
        let ss = SharedState::new();
        ss.orders.push_fill(Fill {
            instrument: 0, order_id: 1, side: Side::Buy,
            price: 100 * PRICE_SCALE, qty: 10, remaining: 0,
            commission: 0, timestamp_ns: 0,
            cum_qty: 10, avg_price: 100 * PRICE_SCALE,
        });
        ss.orders.push_fill(Fill {
            instrument: 0, order_id: 2, side: Side::Sell,
            price: 101 * PRICE_SCALE, qty: 5, remaining: 0,
            commission: 0, timestamp_ns: 0,
            cum_qty: 5, avg_price: 101 * PRICE_SCALE,
        });
        let fills = ss.orders.drain_fills();
        assert_eq!(fills.len(), 2);
        // Second drain should be empty
        assert!(ss.orders.drain_fills().is_empty());
    }

    #[test]
    fn shared_state_order_updates_drain() {
        let ss = SharedState::new();
        ss.orders.push_order_update(OrderUpdate {
            order_id: 1, instrument: 0, status: OrderStatus::Submitted,
            filled_qty: 0.0, remaining_qty: 100.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
        });
        let updates = ss.orders.drain_order_updates();
        assert_eq!(updates.len(), 1);
        assert!(ss.orders.drain_order_updates().is_empty());
    }

    #[test]
    fn shared_state_position_roundtrip() {
        let ss = SharedState::new();
        assert_eq!(ss.portfolio.position(0), 0.0);
        ss.portfolio.set_position(0, 42.0);
        assert_eq!(ss.portfolio.position(0), 42.0);
        ss.portfolio.set_position(0, -10.0);
        assert_eq!(ss.portfolio.position(0), -10.0);
    }

    #[test]
    fn shared_state_account_roundtrip() {
        let ss = SharedState::new();
        let a = AccountState { net_liquidation: 100_000 * PRICE_SCALE, ..Default::default() };
        ss.portfolio.set_account(&a);
        let read = ss.portfolio.account();
        assert_eq!(read.net_liquidation, 100_000 * PRICE_SCALE);
    }

    #[test]
    fn reference_state_ccp_session_id_roundtrip() {
        let ss = SharedState::new();
        assert!(ss.reference.ccp_session_id().is_empty());
        ss.reference.set_ccp_session_id("abc.0001".to_string());
        assert_eq!(ss.reference.ccp_session_id(), "abc.0001");
    }

    #[test]
    fn reference_state_misc_urls_roundtrip() {
        let ss = SharedState::new();
        assert!(ss.reference.misc_urls().is_empty());
        assert!(ss.reference.misc_url("region_dam").is_none());
        let mut urls = HashMap::new();
        urls.insert("region_dam".to_string(), "api-east.example.com".to_string());
        urls.insert("margin".to_string(), "margin.example.com".to_string());
        ss.reference.set_misc_urls(urls);
        let map = ss.reference.misc_urls();
        assert_eq!(map.len(), 2);
        assert_eq!(ss.reference.misc_url("region_dam").as_deref(), Some("api-east.example.com"));
        assert_eq!(ss.reference.misc_url("missing"), None);
    }

    #[test]
    fn event_gateway_logon_carries_fields() {
        let mut urls = HashMap::new();
        urls.insert("region_dam".to_string(), "api.example.com".to_string());
        let event = Event::GatewayLogon {
            ccp_session_id: "sid.abcd".to_string(),
            misc_urls: urls,
        };
        match event {
            Event::GatewayLogon { ccp_session_id, misc_urls } => {
                assert_eq!(ccp_session_id, "sid.abcd");
                assert_eq!(misc_urls.get("region_dam").map(String::as_str), Some("api.example.com"));
            }
            _ => panic!("expected GatewayLogon"),
        }
    }

    #[test]
    fn seqquote_concurrent_read_write() {
        use std::sync::Arc;
        use std::thread;

        let sq = Arc::new(SeqQuote::new());
        let sq_writer = sq.clone();
        let sq_reader = sq.clone();

        let writer = thread::spawn(move || {
            for i in 0..1000 {
                let q = Quote { bid: i * PRICE_SCALE, ask: (i + 1) * PRICE_SCALE, ..Default::default() };
                sq_writer.write(&q);
            }
        });

        let reader = thread::spawn(move || {
            for _ in 0..1000 {
                let q = sq_reader.read();
                // bid and ask should be consistent (ask = bid + PRICE_SCALE)
                if q.bid != 0 {
                    assert_eq!(q.ask, q.bid + PRICE_SCALE);
                }
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();
    }

    fn info(status: &str) -> RichOrderInfo {
        RichOrderInfo {
            contract: api::Contract::default(),
            order: api::Order::default(),
            order_state: api::OrderState { status: status.to_string(), ..Default::default() },
            last_exec: api::Execution::default(),
        }
    }

    fn completed(order_id: u64) -> CompletedOrder {
        CompletedOrder {
            order_id, instrument: 0, status: crate::types::OrderStatus::Filled,
            filled_qty: 100, timestamp_ns: 0,
        }
    }

    /// ibx#262: nothing remembered that an order had completed, so a replayed
    /// frame wrote `Submitted` over the terminal entry and `req_open_orders`
    /// reported a completed order as live. A strategy then re-manages a position
    /// it already has, or cancels an order that no longer exists, with the
    /// open-order snapshot corroborating the wrong picture.
    #[test]
    fn a_completed_order_is_not_returned_to_the_open_book() {
        for terminal in ["Filled", "Cancelled", "Rejected"] {
            let shared = SharedState::new();
            shared.orders.push_order_info(7, info(terminal));

            for open in ["Submitted", "PreSubmitted", "PartiallyFilled", "PendingCancel"] {
                shared.orders.push_order_info(7, info(open));
                assert_eq!(
                    shared.orders.get_order_info(7).unwrap().order_state.status, terminal,
                    "{open} must not overwrite {terminal}",
                );
            }
            assert!(
                shared.orders.drain_open_orders().is_empty(),
                "and {terminal} stays out of the open-order snapshot",
            );
        }
    }

    /// Completing an order evicts its cache row, so the cached status cannot be
    /// what remembers the order is done — the replayed frame finds nothing to
    /// refuse and inserts itself. This is the ordinary path, not an edge case.
    #[test]
    fn a_completion_outlives_the_cache_row_it_evicts() {
        let shared = SharedState::new();
        shared.orders.push_order_info(7, info("Filled"));
        shared.orders.push_completed_order(completed(7));
        shared.orders.remove_order_info(7);

        shared.orders.push_order_info(7, info("Submitted"));
        assert!(
            shared.orders.get_order_info(7).is_none(),
            "a replayed frame must not re-open an order whose row has been evicted",
        );
        assert!(shared.orders.drain_open_orders().is_empty());
    }

    /// A terminal report arriving between the completion and the replay must
    /// not become the thing the guard compares against — a cached string is
    /// overwritten by the next terminal report, and the replay then passes.
    #[test]
    fn an_intervening_report_does_not_erase_the_completion() {
        let shared = SharedState::new();
        shared.orders.push_order_info(7, info("Filled"));
        shared.orders.push_completed_order(completed(7));

        shared.orders.push_order_info(7, info("Cancelled"));
        shared.orders.push_order_info(7, info("Submitted"));

        assert_ne!(
            shared.orders.get_order_info(7).unwrap().order_state.status, "Submitted",
            "the completion survives whatever terminal report lands on top of it",
        );
        assert!(shared.orders.drain_open_orders().is_empty());
    }

    /// A trade cancel or correction restates an execution the gateway already
    /// reported, so it can return a filled order to a working quantity. It is
    /// the gateway's own statement, not a replay of an older one.
    #[test]
    fn a_trade_correction_can_reopen_a_completed_order() {
        let shared = SharedState::new();
        shared.orders.push_order_info(7, info("Filled"));
        shared.orders.push_completed_order(completed(7));
        shared.orders.remove_order_info(7);

        shared.orders.push_order_correction(7, info("PartiallyFilled"));
        assert_eq!(
            shared.orders.get_order_info(7).unwrap().order_state.status, "PartiallyFilled",
            "a correction is not a replay",
        );

        // And the order stops being remembered as completed, so its subsequent
        // ordinary reports are not refused either.
        shared.orders.push_order_info(7, info("Submitted"));
        assert_eq!(shared.orders.get_order_info(7).unwrap().order_state.status, "Submitted");
    }

    /// The ordinary direction still works — without this the guards above would
    /// pass against a cache that refuses every update.
    #[test]
    fn a_fill_still_writes_over_a_working_status() {
        let shared = SharedState::new();
        shared.orders.push_order_info(9, info("Submitted"));
        shared.orders.push_order_info(9, info("Filled"));
        assert_eq!(shared.orders.get_order_info(9).unwrap().order_state.status, "Filled");
    }

    /// Held by age, not by count. What the memory has to outlive is the window
    /// in which a stale frame for the order can still arrive; counting instead
    /// meant a busy session's unrelated completions pushed a still-relevant
    /// entry out, and the replay it was there to refuse got back in. Stays one
    /// short of COMPLETED_MAX so this exercises time-based retention only —
    /// the hard cap itself is a separate concern, proved below.
    #[test]
    fn a_completion_is_remembered_for_a_window_not_a_quota() {
        let shared = SharedState::new();
        shared.orders.push_completed_order(completed(7));

        // Far more unrelated completions than any small count-based bound
        // would hold, without reaching the hard cap.
        for id in 1000..(1000 + COMPLETED_MAX as u64 - 1) {
            shared.orders.push_completed_order(completed(id));
        }

        shared.orders.push_order_info(7, info("Submitted"));
        assert!(
            shared.orders.get_order_info(7).is_none(),
            "the completion survives however many other orders complete beside it",
        );
        assert!(shared.orders.drain_open_orders().is_empty());
    }

    /// And it does not accumulate for the life of the process: past the cap
    /// the oldest entries are dropped. None of these expire during the test
    /// (COMPLETED_RETENTION is minutes), so expiry-based pruning alone is a
    /// no-op here — only the hard eviction fallback can keep the map bounded.
    #[test]
    fn the_completed_memory_does_not_grow_without_limit() {
        let shared = SharedState::new();
        for id in 0..(COMPLETED_MAX as u64 + 10) {
            shared.orders.push_completed_order(completed(id));
        }
        let held = shared.orders.completed.lock().unwrap().len();
        assert!(held <= COMPLETED_MAX, "hard cap must hold even with nothing expired, held {held}");
    }
    #[test]
    fn seqquote_no_torn_reads() {
        use AtomicBool;
        use std::sync::Arc;
        use std::thread;

        // Every field of a given write carries the same value, so any reader
        // that ever observes a torn (half-old, half-new) struct will catch a
        // field disagreeing with `bid` here.
        fn quote_of(i: i64) -> Quote {
            Quote {
                bid: i, ask: i, last: i,
                bid_size: i, ask_size: i, last_size: i,
                volume: i, open: i, high: i, low: i, close: i,
                timestamp_ns: i as u64,
                bid_exch_mask: i, ask_exch_mask: i, last_exch_mask: i,
                // Every field moves together, so a snapshot mixing two
                // generations shows as a field disagreeing with the rest.
                halted: i,
            }
        }

        let sq = Arc::new(SeqQuote::new());
        let stop = Arc::new(AtomicBool::new(false));

        let writer = {
            let sq = sq.clone();
            thread::spawn(move || {
                for i in 1..=20_000i64 {
                    sq.write(&quote_of(i));
                }
            })
        };

        let readers: Vec<_> = (0..4).map(|_| {
            let sq = sq.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let q = sq.read();
                    let v = q.bid;
                    let fields = [
                        q.ask, q.last, q.bid_size, q.ask_size, q.last_size,
                        q.volume, q.open, q.high, q.low, q.close,
                        q.timestamp_ns as i64, q.bid_exch_mask, q.ask_exch_mask, q.last_exch_mask,
                        q.halted,
                    ];
                    assert!(fields.iter().all(|&f| f == v), "torn SeqQuote read: bid={v} fields={fields:?}");
                }
            })
        }).collect();

        writer.join().unwrap();
        stop.store(true, Ordering::Relaxed);
        for r in readers { r.join().unwrap(); }
    }

    #[test]
    fn a_fill_moves_the_holding_the_caller_reads() {
        let p = PortfolioState::new();
        // Opening.
        p.apply_fill(7, 100.0, 10 * PRICE_SCALE);
        let row = p.position_info(7).unwrap();
        assert_eq!(row.position, 100.0);
        assert_eq!(row.avg_cost, 10 * PRICE_SCALE);
        // Adding averages the cost in.
        p.apply_fill(7, 100.0, 20 * PRICE_SCALE);
        let row = p.position_info(7).unwrap();
        assert_eq!(row.position, 200.0);
        assert_eq!(row.avg_cost, 15 * PRICE_SCALE, "the two fills average");
        // Reducing realises a gain and leaves the basis alone.
        p.apply_fill(7, -150.0, 30 * PRICE_SCALE);
        let row = p.position_info(7).unwrap();
        assert_eq!(row.position, 50.0);
        assert_eq!(row.avg_cost, 15 * PRICE_SCALE, "a sale does not re-price what remains");
        // Closing leaves no basis.
        p.apply_fill(7, -50.0, 30 * PRICE_SCALE);
        assert_eq!(p.position_info(7).unwrap().position, 0.0);
        assert_eq!(p.position_info(7).unwrap().avg_cost, 0);
        // One fill that crosses through flat: what is held now was bought at
        // this price, not at what the holding it replaced had paid.
        p.apply_fill(8, 50.0, 10 * PRICE_SCALE);
        p.apply_fill(8, -100.0, 30 * PRICE_SCALE);
        let row = p.position_info(8).unwrap();
        assert_eq!(row.position, -50.0, "long fifty, sold a hundred, short fifty");
        assert_eq!(row.avg_cost, 30 * PRICE_SCALE, "priced at what the short was sold for");

        // A fill on a contract this session never saw still opens the holding.
        p.apply_fill(9, -10.0, 5 * PRICE_SCALE);
        assert_eq!(p.position_info(9).unwrap().position, -10.0, "a short opens too");
        assert_eq!(p.position_info(9).unwrap().avg_cost, 5 * PRICE_SCALE);
    }
}

#[cfg(test)]
mod grant_tests {
    use super::*;

    #[test]
    fn the_older_spelling_takes_the_setting_and_the_grant() {
        let shared = SharedState::new();
        // The setting alone asks for it; the venue has granted nothing yet.
        assert!(shared.settings().island_for_nasdaq, "the counterpart's default");
        assert!(!shared.island_for_nasdaq(), "and no grant is not a grant");

        shared.reference.set_enabled_features(vec!["NOAMOPTCHK".into()]);
        assert!(!shared.island_for_nasdaq(), "another grant is not this one");

        shared.reference.add_enabled_features(vec!["ISLAND2NASDAQ".into()]);
        assert!(shared.island_for_nasdaq(), "asked for and granted");
    }
}
