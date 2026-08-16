//! What is quoted, and what the venue has said about it.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use crate::types::*;

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
