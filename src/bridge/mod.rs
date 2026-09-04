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

mod event;
pub use event::*;
mod seq_quote;
pub use seq_quote::*;
mod market_data;
pub use market_data::*;
mod orders;
pub use orders::*;
mod reference;
pub use reference::*;
mod portfolio;
pub use portfolio::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use std::sync::{Condvar, Mutex};


/// The token the venue grants a session that may name Nasdaq by its older
/// spelling. Stated on the granted-feature list at logon.
const ISLAND_FOR_NASDAQ_GRANT: &str = "ISLAND2NASDAQ";
use crate::types::*;

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
            order_state: crate::types::model::OrderState {
                status: "Submitted".to_string(), ..Default::default()
            },
            last_exec: Default::default(),
        });

        assert!(
            s.drain_open_orders().is_empty(),
            "a filled order is not reported as working again by the replay",
        );
    }

    /// The venue echoes a working status behind a fill. The order is retired
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

/// Shared state between hot loop and external caller.
/// Composed of domain-specific containers for clear ownership boundaries.
pub struct SharedState {
    /// What the session runs under, settled when it opened.
    ///
    /// Held here so the engine reads a value rather than the process it runs
    /// in: two sessions in one process have their own, and neither can change
    /// the other's mid-flight.
    pub settings: std::sync::Mutex<std::sync::Arc<crate::settings::SessionSettings>>,
    /// Prices, books and streams.
    pub market: MarketDataState,
    /// Fills, order changes and previews.
    pub orders: OrderState,
    /// Everything that is not a price: contracts, history, news, scans.
    pub reference: ReferenceState,
    /// What the account holds and what it is worth.
    pub portfolio: PortfolioState,
    /// Last measured auth-connection round-trip time in nanoseconds
    /// (0 = never measured). Sampled from the test-request/echo cycle —
    /// see `HotLoop` liveness and `ControlCommand::Ping`.
    /// How many events this session discarded because nobody read far enough.
    ///

    ccp_rtt_ns: AtomicU64,
    /// Set by the hot loop when the session is over (connection lost, engine
    /// stopped, or reconnect exhausted). Read-and-clear by the client so the
    /// `connection_closed` callback can fire without an event channel. The
    /// `Event::Disconnected` channel path is optional; this
    /// flag is always populated.
    connection_lost: AtomicBool,
    /// Whether the loss above was deliberate. Recorded at the moment of loss,
    /// not derived later.
    connection_lost_by_design: AtomicBool,
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
    pub fn settings(&self) -> std::sync::Arc<crate::settings::SessionSettings> {
        self.settings.lock().unwrap().clone()
    }

    /// Whether a US stock trading on Nasdaq is named by the older spelling.
    ///
    /// The setting asks for it and the venue grants it, and both are required.
    /// The grant is read off the granted-feature list at logon and held beside
    /// the setting. Read once per contract definition, so
    /// the grant is settled at logon rather than scanned for here.
    pub fn island_for_nasdaq(&self) -> bool {
        self.settings().island_for_nasdaq && self.reference.island_granted()
    }

    /// Stated once, as the session opens, before the engine's threads start.
    #[doc(hidden)]
    pub fn set_settings(&self, settings: std::sync::Arc<crate::settings::SessionSettings>) {
        *self.settings.lock().unwrap() = settings;
    }

    /// An empty one.
    pub fn new() -> Self {
        Self {
            settings: std::sync::Mutex::new(std::sync::Arc::new(Default::default())),
            market: MarketDataState::new(),
            orders: OrderState::new(),
            reference: ReferenceState::new(),
            portfolio: PortfolioState::new(),
            ccp_rtt_ns: AtomicU64::new(0),
            connection_lost: AtomicBool::new(false),
            connection_lost_by_design: AtomicBool::new(false),
            connection_restored: AtomicBool::new(false),
            notify_mutex: Mutex::new(false),
            notify_condvar: Condvar::new(),
        }
    }

    /// Signal that the session is over. Hot-loop side.
    #[doc(hidden)]
    #[inline]
    pub fn set_connection_lost(&self) {
        // The two flags are one fact between them: which way the connection
        // last went. Both raised, the reader cannot tell which came first and
        // applies them in its own order — so a session that came back and went
        // again reads as connected, with nothing left to say it is not.
        self.connection_restored.store(false, Ordering::Release);
        // Decided here rather than by a later reader. A shutdown records its
        // own reason and records it after any venue-side drop, so deriving
        // this afterwards reports a caller-requested stop for a session the
        // venue ended, and every absence after it reads as tidying.
        self.connection_lost_by_design.store(
            self.reference.session_over()
                == Some(crate::reliability::retry::DisconnectReason::ByDesign.as_str()),
            Ordering::Release,
        );
        self.connection_lost.store(true, Ordering::Release);
        self.notify();
    }

    /// Whether the last recorded loss was one this process asked for.
    ///
    /// Read beside [`take_connection_lost`](Self::take_connection_lost): the
    /// two together say the connection went away and whether that was the
    /// intention.
    #[inline]
    pub fn connection_lost_by_design(&self) -> bool {
        self.connection_lost_by_design.load(Ordering::Acquire)
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
        // The later transition is the one that stands, as in
        // [`set_connection_lost`](Self::set_connection_lost).
        self.connection_lost.store(false, Ordering::Release);
        self.connection_restored.store(true, Ordering::Release);
        self.notify();
    }

    /// Read and clear the connection-restored flag.
    #[inline]
    pub fn take_connection_restored(&self) -> bool {
        self.connection_restored.swap(false, Ordering::AcqRel)
    }

    /// Record an auth-connection RTT sample. Hot-loop side.
    #[inline]
    pub fn set_ccp_rtt(&self, rtt: std::time::Duration) {
        self.ccp_rtt_ns.store(rtt.as_nanos().min(u64::MAX as u128) as u64, Ordering::Relaxed);
    }

    /// Last measured auth-connection round-trip time, if any.
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

    /// Wait for data notification with a timeout. Returns true if notified, false if
    /// timed out.
    pub fn wait_for_data(&self, timeout: std::time::Duration) -> bool {
        let mut pending = self.notify_mutex.lock().unwrap();
        if *pending {
            *pending = false;
            return true;
        }
        let (lock, result) = self.notify_condvar.wait_timeout(pending, timeout).unwrap();
        let had_data = *lock;
        if had_data {
            // Reset the flag via a mutable reference obtained from the MutexGuard's
            // deref.
            drop(lock);
            *self.notify_mutex.lock().unwrap() = false;
        }
        had_data || !result.timed_out()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use crate::types::model as api;
    use super::*;

    /// The venue broadcasts notices unasked and only a subscriber drains
    /// them, so a session that never subscribes would otherwise hold every
    /// notice of the day for the life of the process. Past the bound the
    /// oldest are dropped, and a late subscriber is handed the most recent.
    #[test]
    fn broadcast_notices_do_not_pile_up_unread() {
        let shared = SharedState::new();
        for id in 0..(NEWS_BULLETIN_LIMIT as i32 + 10) {
            shared.market.push_news_bulletin(crate::types::NewsBulletin {
                msg_id: id, msg_type: 1, message: String::new(), exchange: String::new(),
            });
        }
        let held = shared.market.drain_news_bulletins();
        assert_eq!(held.len(), NEWS_BULLETIN_LIMIT, "the buffer grew past its bound");
        assert_eq!(held[0].msg_id, 10, "the oldest were kept and the newest dropped");
        assert_eq!(held[held.len() - 1].msg_id, NEWS_BULLETIN_LIMIT as i32 + 9);
    }

    /// Whether a loss was asked for is decided as it is recorded.
    ///
    /// Shutting down records its own reason. Derived from that reason after
    /// the fact, a session the venue took away reports as caller-requested,
    /// and every absence after it reads as ordinary tidying.
    #[test]
    fn a_loss_remembers_whether_it_was_asked_for() {
        use crate::reliability::retry::DisconnectReason;

        // The venue takes the session away: nothing has recorded a reason.
        let shared = SharedState::new();
        shared.set_connection_lost();
        // The tidying that follows records one, as a shutdown does.
        shared.reference.set_session_over(DisconnectReason::ByDesign.as_str());
        assert!(shared.take_connection_lost());
        assert!(!shared.connection_lost_by_design(), "nobody asked for this one");

        // And a shutdown, which records its reason before the loss.
        let asked = SharedState::new();
        asked.reference.set_session_over(DisconnectReason::ByDesign.as_str());
        asked.set_connection_lost();
        assert!(asked.take_connection_lost());
        assert!(asked.connection_lost_by_design());
    }

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
        assert!(fills.iter().all(|(_, report)| report.is_none()), "none was pushed with one");
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

    /// The replay flag belongs to the connection that earned it.
    ///
    /// Set once and never cleared, it outlives that connection: after a
    /// reconnect it answers from the previous session's record instead of
    /// waiting for the new one to name its working orders.
    #[test]
    fn a_new_connection_has_not_yet_named_what_it_has_working() {
        let shared = SharedState::new();
        assert!(!shared.orders.replay_done(), "nothing has been named yet");
        shared.orders.set_replay_done();
        assert!(shared.orders.replay_done());
        shared.orders.replay_is_pending();
        assert!(!shared.orders.replay_done(), "and a reconnect starts over");
    }

    /// The bound the naming is waited on is spent from the moment the
    /// connection came up, not from the first caller to ask. A first request
    /// that waited the bound out otherwise spent it, and a global cancel
    /// issued straight after — the kill switch — waited nothing and said
    /// nothing.
    #[test]
    fn the_replay_bound_is_spent_from_the_connect_not_the_first_waiter() {
        let shared = SharedState::new();
        shared.orders.replay_is_pending();
        // The bound passes with nobody asking.
        std::thread::sleep(Duration::from_millis(3_200));
        let started = std::time::Instant::now();
        assert!(!shared.orders.wait_for_replay(), "the naming never finished");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a first waiter arriving after the bound has passed is answered at once, \
             not held for a fresh bound of its own",
        );
    }

    /// A completed order is remembered as completed, so a replayed frame
    /// cannot write `Submitted` over the terminal entry and have
    /// `req_open_orders` report it as live. A strategy reading that would
    /// re-manage a position it already holds, or cancel an order that no
    /// longer exists, with the open-order snapshot corroborating it.
    #[test]
    fn a_completed_order_is_not_returned_to_the_open_book() {
        for terminal in ["Filled", "Cancelled", "Rejected"] {
            let shared = SharedState::new();
            shared.orders.push_order_info(7, info(terminal));

            for open in ["Submitted", "PreSubmitted", "PendingCancel", "PendingReplace"] {
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

    /// A refused order is cached in the shape of a parked one — the status
    /// vocabulary has no refused string, and the refusal rides the completed
    /// status beside it. That shape is finished too: once the completion
    /// window has passed there is nothing left to refuse a replayed frame but
    /// the terminal-status guard, and a refused entry reported as live sends
    /// a strategy hedging a position it does not hold.
    #[test]
    fn a_refused_order_is_not_returned_to_the_open_book() {
        let shared = SharedState::new();
        shared.orders.push_order_info(7, RichOrderInfo {
            contract: api::Contract::default(),
            order: api::Order::default(),
            order_state: api::OrderState {
                status: "Inactive".to_string(),
                completed_status: "No valid bid/ask".to_string(),
                ..Default::default()
            },
            last_exec: api::Execution::default(),
        });

        for open in ["Submitted", "PreSubmitted", "PendingCancel", "PendingReplace"] {
            shared.orders.push_order_info(7, info(open));
            let cached = shared.orders.get_order_info(7).unwrap();
            assert_eq!(
                cached.order_state.status, "Inactive",
                "{open} must not overwrite a refusal",
            );
            assert_eq!(cached.order_state.completed_status, "No valid bid/ask");
        }
        assert!(
            shared.orders.drain_open_orders().is_empty(),
            "and the refusal stays out of the open-order snapshot",
        );

        // A genuinely parked order carries no completed status and has not
        // finished: the venue can bring it back to working, so a working
        // frame still supersedes it.
        shared.orders.push_order_info(8, info("Inactive"));
        shared.orders.push_order_info(8, info("Submitted"));
        assert_eq!(
            shared.orders.get_order_info(8).unwrap().order_state.status, "Submitted",
            "a parked order is not finished and still moves",
        );
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

    /// A trade cancel or correction restates an execution the venue already
    /// reported, so it can return a filled order to a working quantity. It is
    /// the venue's statement, not a replay of an older one.
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
}

#[cfg(test)]
mod grant_tests {
    use super::*;

    #[test]
    fn the_older_spelling_takes_the_setting_and_the_grant() {
        let shared = SharedState::new();
        // The setting alone asks for it; the venue has granted nothing yet.
        assert!(shared.settings().island_for_nasdaq, "the documented default");
        assert!(!shared.island_for_nasdaq(), "and no grant is not a grant");

        shared.reference.set_enabled_features(vec!["NOAMOPTCHK".into()]);
        assert!(!shared.island_for_nasdaq(), "another grant is not this one");

        shared.reference.add_enabled_features(vec!["ISLAND2NASDAQ".into()]);
        assert!(shared.island_for_nasdaq(), "asked for and granted");
    }
}
