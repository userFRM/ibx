//! What is quoted, and what the venue has said about it.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use crate::types::*;

/// How many broadcast notices are kept for a caller who has not asked for them
/// yet. The venue broadcasts these unasked and only a subscriber drains them,
/// so without a bound a session that never subscribes keeps every notice of the
/// day for the life of the process.
pub const NEWS_BULLETIN_LIMIT: usize = 1000;

/// How much of a stream is kept for a caller who has stopped reading it.
///
/// The same reasoning as the bulletins above, and it applies harder: these
/// arrive at market rate rather than a few times an hour, and this library
/// documents a way of reading them that never pumps the callback loop at all.
/// Unbounded, a book or a tick-by-tick stream grows the process until it dies.
/// Bounded, a caller that stopped reading loses the oldest of what it was not
/// reading, which is the lesser of the two.
pub const STREAM_BACKLOG_LIMIT: usize = 100_000;

/// Push onto a stream that nobody may be draining, oldest out first.
fn push_bounded<T>(queue: &Mutex<Vec<T>>, item: T, limit: usize, what: &str) {
    let mut held = queue.lock().unwrap();
    if held.len() >= limit {
        // A tenth at a time rather than one at a time: dropping a single entry
        // per push leaves every later push doing a full shift of the vector.
        let drop_to = limit - limit / 10;
        let shed = held.len() - drop_to;
        held.drain(..shed);
        log::warn!(
            "{what} has gone past {limit} unread, so the oldest of them were dropped — \
             nothing is draining this stream",
        );
    }
    held.push(item);
}

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
    /// The venue's clock, from the last message it sent.
    ///
    /// Every message it sends is stamped with the time it sent it. Reporting
    /// this machine's clock instead would answer the question a caller asked —
    /// how far apart this machine and the venue are — with the one number
    /// that cannot tell them.
    venue_time: Mutex<Option<String>>,
    /// Messages the venue sent that nothing here reads, named once each:
    /// which connection, and what it was. Empty is the claim that this client
    /// reads everything this venue sends it, and the only way to check it.
    unread_wire: Mutex<Vec<(&'static str, String)>>,
}

impl MarketDataState {
    pub(super) fn new() -> Self {
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
    /// `try_quote`.
    #[inline]
    pub fn quote(&self, id: InstrumentId) -> Quote {
        self.quotes[id as usize].read()
    }

    /// Bounds-checked quote read for user-supplied instrument ids: an
    /// out-of-range id is a caller error, not a reason to panic the process
    /// through the language boundary.
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

    /// Take every tbt trades waiting, leaving none.
    pub fn drain_tbt_trades(&self) -> Vec<TbtTrade> {
        self.tbt_trades.lock().unwrap().drain(..).collect()
    }

    /// Take every tbt quotes waiting, leaving none.
    pub fn drain_tbt_quotes(&self) -> Vec<TbtQuote> {
        self.tbt_quotes.lock().unwrap().drain(..).collect()
    }

    /// Take every real time bars waiting, leaving none.
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

    /// Take every depth updates waiting, leaving none.
    pub fn drain_depth_updates(&self) -> Vec<DepthUpdate> {
        self.depth_updates.lock().unwrap().drain(..).collect()
    }

    /// Take every tick news waiting, leaving none.
    pub fn drain_tick_news(&self) -> Vec<TickNews> {
        self.tick_news.lock().unwrap().drain(..).collect()
    }

    /// Take every news bulletins waiting, leaving none.
    pub fn drain_news_bulletins(&self) -> Vec<NewsBulletin> {
        self.news_bulletins.lock().unwrap().drain(..).collect()
    }

    /// Take every option computations waiting, leaving none.
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

    /// Take every venue errors waiting, leaving none.
    pub fn drain_venue_errors(&self) -> Vec<String> {
        self.venue_errors.lock().unwrap().drain(..).collect()
    }

    #[doc(hidden)] pub fn push_venue_error(&self, text: String) {
        self.venue_errors.lock().unwrap().push(text);
    }

    /// Take every subscription failures waiting, leaving none.
    pub fn drain_subscription_failures(&self) -> Vec<(crate::types::InstrumentId, String)> {
        self.subscription_failures.lock().unwrap().drain(..).collect()
    }

    // ── Hot-loop-side writers ──

    #[doc(hidden)]
    pub fn push_quote(&self, id: InstrumentId, quote: &Quote) {
        self.quotes[id as usize].write(quote);
    }

    #[doc(hidden)] pub fn push_tbt_trade(&self, trade: TbtTrade) {
        push_bounded(&self.tbt_trades, trade, STREAM_BACKLOG_LIMIT, "tbt_trades");
    }

    #[doc(hidden)] pub fn push_tbt_quote(&self, quote: TbtQuote) {
        push_bounded(&self.tbt_quotes, quote, STREAM_BACKLOG_LIMIT, "tbt_quotes");
    }


    #[doc(hidden)] pub fn push_real_time_bar(&self, req_id: u32, bar: RealTimeBar) {
        push_bounded(&self.real_time_bars, (req_id, bar), STREAM_BACKLOG_LIMIT, "real_time_bars");
    }

    #[doc(hidden)] pub fn push_depth_update(&self, update: DepthUpdate) {
        push_bounded(&self.depth_updates, update, STREAM_BACKLOG_LIMIT, "depth_updates");
    }

    /// Remove all buffered depth updates for a given req_id (called on cancel).
    #[doc(hidden)] pub fn purge_depth_updates(&self, req_id: u32) {
        self.depth_updates.lock().unwrap().retain(|u| u.req_id != req_id);
    }

    #[doc(hidden)] pub fn push_tick_news(&self, news: TickNews) {
        push_bounded(&self.tick_news, news, STREAM_BACKLOG_LIMIT, "tick_news");
    }

    /// A broadcast notice, kept until someone reads it.
    ///
    /// Bounded, because the venue broadcasts these whether or not anyone
    /// subscribed and the drain only runs once someone has: a session that
    /// never asks for bulletins would otherwise hold every notice of the day
    /// for the life of the process and free none of them. Past the bound the
    /// oldest are dropped, so a late subscriber is handed the most recent
    /// [`NEWS_BULLETIN_LIMIT`] rather than everything or nothing.
    #[doc(hidden)] pub fn push_news_bulletin(&self, bulletin: NewsBulletin) {
        let mut held = self.news_bulletins.lock().unwrap();
        if held.len() >= NEWS_BULLETIN_LIMIT {
            // ponytail: O(n) shift on a queue of a thousand, on an event that
            // arrives a few times an hour. A VecDeque if bulletins ever became
            // a hot path.
            held.remove(0);
        }
        held.push(bulletin);
    }

    /// What the venue last said its own model made of a contract.
    ///
    /// Kept as well as delivered. Delivered alone it is gone the moment a
    /// caller reads it, and answering "what would this be worth at another
    /// volatility" needs the venue's statement still to hand.
    pub fn option_model(&self, instrument: crate::types::InstrumentId) -> Option<crate::types::OptionComputation> {
        self.last_option_model.lock().unwrap().get(&instrument).copied()
    }

    #[doc(hidden)] pub fn push_option_computation(&self, comp: crate::types::OptionComputation) {
        // Only the venue's own statement becomes the model for a contract. An
        // answer worked out here belongs to the question that asked it and
        // names no contract at all — stored, it lands on slot zero, which is a
        // real contract, and the next question about that contract is answered
        // against the last caller's own volatility and price instead of the
        // venue's. It also says nothing about which model the venue used, so
        // the refusal that guards the one this client cannot solve with reads
        // as though there were nothing to guard.
        if comp.answers.is_none() {
            self.last_option_model.lock().unwrap().insert(comp.instrument, comp);
        }
        push_bounded(&self.option_computations, comp, STREAM_BACKLOG_LIMIT, "option_computations");
    }

    #[doc(hidden)] pub fn push_subscription_failure(&self, instrument: crate::types::InstrumentId, reason: String) {
        self.subscription_failures.lock().unwrap().push((instrument, reason));
    }

    #[doc(hidden)] pub fn set_instrument_count(&self, count: u32) {
        self.instrument_count.store(count as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod option_model_tests {
    use super::*;
    use crate::types::OptionComputation;

    /// An answer worked out here does not become the venue's model for a
    /// contract.
    ///
    /// It names no contract — a solve answers a request, not an instrument —
    /// so stored it lands on slot zero, which is a real contract. The next
    /// question about that contract would then be answered against the last
    /// caller's own volatility and price, and against a record saying nothing
    /// about which model the venue used.
    #[test]
    fn a_local_answer_does_not_become_the_venues_model() {
        let market = MarketDataState::new();

        // The venue's own statement for the contract in slot zero.
        market.push_option_computation(OptionComputation {
            instrument: 0,
            implied_vol: 0.25,
            price_based_vol: true,
            ..Default::default()
        });
        // And a caller's question answered here, which names no contract.
        market.push_option_computation(OptionComputation {
            implied_vol: 0.99,
            ..OptionComputation::solved(7)
        });

        let stated = market.option_model(0).expect("the venue's statement stands");
        assert_eq!(stated.implied_vol, 0.25, "the venue's volatility, not the answer's");
        assert!(stated.price_based_vol, "and what it says about the model it used");
    }
}
