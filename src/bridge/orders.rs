//! What has been submitted, filled, and refused.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::sync::Mutex;
use std::collections::HashMap;
use crate::types::*;
use crate::types::model as api;

/// How long a caller waits for the venue to finish naming the working orders.
const REPLAY_WAIT: Duration = Duration::from_secs(3);

/// Fills, order status updates, cancel rejects, what-if responses, order
/// cache, and inactive-order reasons.
pub struct OrderState {
    /// Each fill and the report it was booked off, where there is one.
    ///
    /// One pass can carry two prints of the same order. Looked up against the
    /// order afterwards, both read the record the later print left, so the
    /// earlier one was reported under the later one's execution id, time,
    /// running quantity and average — and the charge that named that id was
    /// then attached to both.
    fills: Mutex<Vec<(Fill, Option<RichOrderInfo>)>>,
    order_updates: Mutex<Vec<OrderUpdate>>,
    cancel_rejects: Mutex<Vec<CancelReject>>,
    /// What each fill cost, as the venue states it on a record of its own.
    charges: Mutex<Vec<crate::types::model::CommissionAndFeesReport>>,
    /// Executions the venue restated rather than announced: replayed at logon
    /// for quantity the book already holds, or for an order this session never
    /// tracked. Nothing is booked from them, so none became a fill; they are
    /// kept so a caller asking for the day's executions is answered.
    restated_executions: Mutex<Vec<(api::Contract, api::Execution)>>,
    what_if_responses: Mutex<Vec<WhatIfResponse>>,
    completed_orders: Mutex<Vec<CompletedOrder>>,
    /// Enriched order info from CCP exec reports (order_id -> RichOrderInfo).
    order_cache: Mutex<HashMap<u64, RichOrderInfo>>,
    /// Orders that reached a terminal state, and when. The cache row is evicted
    /// when an order completes, so the cached status alone cannot say an order
    /// is done — a replayed frame would find nothing to refuse and insert it as
    /// open.
    pub(super) completed: Mutex<HashMap<u64, Instant>>,
    /// Set when the server finishes naming the orders already working, which
    /// it does unprompted after a connect. Until then "none" and "not yet
    /// told" look the same to a caller.
    replay_done: AtomicBool,
    /// When the wait for that naming gives up, shared by everyone waiting.
    ///
    /// An account with nothing working never sees the naming end, so a wait
    /// entered on every call ran to its bound every time. Set when a
    /// connection comes up — the venue starts the naming then — and replaced
    /// with the next one on a reconnect, so each connection pays the bound
    /// once and every caller in that window waits for the same moment.
    replay_deadline: Mutex<Option<Instant>>,
    /// The highest id the venue has named an order working under, from any
    /// session. An id is spent only while its order is live, so this is the
    /// floor a new id has to clear.
    working_id_watermark: AtomicU64,
    /// The same mark, kept to what a request can carry.
    ///
    /// An order id goes as wide as the venue lets it and a request id is four
    /// billion wide. A caller that numbers both out of one counter — which is
    /// how the client this one stands in for is written — needs a number that
    /// clears every id an order has spent and still fits a request. This is
    /// that number: the highest the venue has named which a request could
    /// carry.
    narrow_id_watermark: AtomicU64,
    /// Reason for a genuinely-Inactive (39=I) transition: (order_id, ibapi
    /// error code, message). ibapi has no callback dedicated to "order
    /// parked with reason", so this is drained into `Wrapper::error` the
    /// same way a cancel/modify reject is.
    order_inactive: Mutex<Vec<(u64, i32, String)>>,
}

/// Say, once, that this account's order ids have outgrown a request id.
///
/// A program written against the reference client numbers its orders and its
/// requests out of one signed 32-bit counter, and this protocol carries an
/// order id far wider than it carries a request id. Nothing here can undo an
/// id the account has already used, so it is said rather than worked around —
/// silently, it reads as a client that cannot place its first order.
///
/// [`OrderState::narrow_id_watermark`] is what such a program should count
/// from instead.
pub fn say_if_past_a_request_id(order_id: u64) {
    if order_id > u32::MAX as u64 {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| log::warn!(
            "this account has used an order id above {}, so the next one is {order_id}; a \
             program that numbers its requests from the same counter cannot carry it",
            u32::MAX,
        ));
    }
}

impl OrderState {
    pub(super) fn new() -> Self {
        Self {
            fills: Mutex::new(Vec::with_capacity(64)),
            order_updates: Mutex::new(Vec::with_capacity(64)),
            cancel_rejects: Mutex::new(Vec::with_capacity(16)),
            charges: Mutex::new(Vec::with_capacity(16)),
            restated_executions: Mutex::new(Vec::new()),
            what_if_responses: Mutex::new(Vec::with_capacity(8)),
            completed_orders: Mutex::new(Vec::with_capacity(64)),
            order_cache: Mutex::new(HashMap::new()),
            completed: Mutex::new(HashMap::new()),
            replay_done: AtomicBool::new(false),
            replay_deadline: Mutex::new(None),
            working_id_watermark: AtomicU64::new(0),
            narrow_id_watermark: AtomicU64::new(0),
            order_inactive: Mutex::new(Vec::with_capacity(8)),
        }
    }

    /// Take every fills waiting, leaving none.
    pub fn drain_fills(&self) -> Vec<(Fill, Option<RichOrderInfo>)> {
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

    /// Take every cancel rejects waiting, leaving none.
    pub fn drain_cancel_rejects(&self) -> Vec<CancelReject> {
        self.cancel_rejects.lock().unwrap().drain(..).collect()
    }

    /// Take what the venue has said its fills cost, leaving none.
    ///
    /// The charge is not on the execution report — that report carries no
    /// commission tag at all — but on a record of its own that follows it,
    /// naming the execution it belongs to. A caller reads it the same way:
    /// the fill first, then what it cost.
    pub fn drain_charges(&self) -> Vec<crate::types::model::CommissionAndFeesReport> {
        self.charges.lock().unwrap().drain(..).collect()
    }

    #[doc(hidden)] pub fn push_charge(&self, charge: crate::types::model::CommissionAndFeesReport) {
        self.charges.lock().unwrap().push(charge);
    }

    /// Take the executions the venue restated, leaving none.
    pub fn drain_restated_executions(&self) -> Vec<(api::Contract, api::Execution)> {
        self.restated_executions.lock().unwrap().drain(..).collect()
    }

    #[doc(hidden)] pub fn push_restated_execution(&self, contract: api::Contract, execution: api::Execution) {
        self.restated_executions.lock().unwrap().push((contract, execution));
    }

    /// Drain reasons for genuinely-Inactive (39=I) transitions, each as
    /// (order_id, ibapi error code, message) — see `order_inactive`.
    pub fn drain_order_inactive(&self) -> Vec<(u64, i32, String)> {
        self.order_inactive.lock().unwrap().drain(..).collect()
    }

    /// Take every what if responses waiting, leaving none.
    pub fn drain_what_if_responses(&self) -> Vec<WhatIfResponse> {
        self.what_if_responses.lock().unwrap().drain(..).collect()
    }

    /// Take every completed orders waiting, leaving none.
    pub fn drain_completed_orders(&self) -> Vec<CompletedOrder> {
        self.completed_orders.lock().unwrap().drain(..).collect()
    }

    /// Snapshot enriched entries that belong in the open-order book: a
    /// genuinely open IB state, or a genuinely-Inactive (39=I) order that can
    /// still reactivate. A rejected order also stringifies to "Inactive"
    /// (ibapi has no Rejected string) but always carries a non-empty
    /// `completed_status`, which is how the two are told apart
    /// `is_open_or_reactivatable`. Terminal entries (Filled /
    /// Cancelled / Rejected) are filtered out so `req_open_orders` does not
    /// leak historical orders that are still cached for `req_completed_orders`
    /// lookups.
    pub fn drain_open_orders(&self) -> Vec<(u64, RichOrderInfo)> {
        let lock = self.order_cache.lock().unwrap();
        lock.iter()
            .filter(|(_, v)| crate::types::order_status::is_open_or_reactivatable(
                &v.order_state.status, &v.order_state.completed_status))
            .map(|(&k, v)| (k, v.clone()))
            .collect()
    }

    /// Get enriched order info by order_id.
    pub fn get_order_info(&self, order_id: u64) -> Option<RichOrderInfo> {
        self.order_cache.lock().unwrap().get(&order_id).cloned()
    }

    /// Whether a fill for this order is still waiting to be read.
    ///
    /// A fill is read against the order's record, so the record outlives the
    /// fill rather than the other way round.
    pub fn has_pending_fill(&self, order_id: u64) -> bool {
        self.fills.lock().unwrap().iter().any(|(f, _)| f.order_id == order_id)
    }

    /// Remove an enriched entry. Called after a completed order has been
    /// delivered to the user, to bound `order_cache` growth in long sessions.
    pub fn remove_order_info(&self, order_id: u64) {
        self.order_cache.lock().unwrap().remove(&order_id);
    }

    // ── Hot-loop-side writers ──

    #[doc(hidden)] pub fn push_fill(&self, fill: Fill) {
        self.fills.lock().unwrap().push((fill, None));
    }

    /// A fill and the report it was booked off, which is the one that states
    /// its execution.
    #[doc(hidden)] pub fn push_fill_reported(&self, fill: Fill, report: RichOrderInfo) {
        self.fills.lock().unwrap().push((fill, Some(report)));
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

    /// A new connection has not named what is already working yet.
    ///
    /// Left set across a reconnect, the flag still reports the previous
    /// connection's replay as finished, and a caller asking what it has on is
    /// answered from the pre-drop book — every order in it Uncertain — while
    /// the venue's account is still arriving.
    /// Whether the orders already working have been received.
    pub fn replay_done(&self) -> bool {
        self.replay_done.load(Ordering::Acquire)
    }

    /// Wait for the venue to finish naming the orders already working, and
    /// say whether it did.
    ///
    /// Bounded, because an account with nothing working never sees the
    /// naming end and waiting forever for it would be worse than proceeding.
    /// The bound is one deadline per connection, anchored to the moment the
    /// connection came up rather than set by the first caller to wait: the
    /// venue starts the naming then, so a first request that waits the bound
    /// out does not spend a later caller's wait, and once it has passed
    /// nobody waits again until a reconnect.
    pub fn wait_for_replay(&self) -> bool {
        let deadline = *self
            .replay_deadline
            .lock()
            .unwrap()
            // The connection sets the deadline when it comes up; where one
            // never has, the first waiter marks the start of the wait.
            .get_or_insert_with(|| Instant::now() + REPLAY_WAIT);
        while !self.replay_done() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.replay_done()
    }

    /// The highest id the venue has named an order under, or zero where it has
    /// named none.
    ///
    /// The venue refuses an id it is still working an order under, and one a
    /// fill has spent; an id whose order was withdrawn is free again. Rather
    /// than track which is which, this counts past every id the venue has
    /// named — the venue names them at every connect, so nothing has to be
    /// remembered between runs.
    pub fn working_id_watermark(&self) -> u64 {
        self.working_id_watermark.load(Ordering::Acquire)
    }

    /// The highest id the venue has named that a request can also carry.
    ///
    /// See `narrow_id_watermark`.
    pub fn narrow_id_watermark(&self) -> u64 {
        self.narrow_id_watermark.load(Ordering::Acquire)
    }

    /// A new connection has not yet named what it has working.
    ///
    /// Set once and never cleared, this state outlived the connection that
    /// earned it: after a reconnect a caller asking what it already has on was
    /// answered straight away, from the old session's record, and never waited
    /// for the new one to say — which is how the same order gets placed twice.
    #[doc(hidden)] pub fn replay_is_pending(&self) {
        self.replay_done.store(false, Ordering::Release);
        // The venue starts the naming as the connection comes up, so the
        // bound a caller waits on starts here too. Anchored on the first
        // caller instead, a first request that waited it out spent it, and a
        // global cancel issued straight after waited nothing and said
        // nothing.
        *self.replay_deadline.lock().unwrap() = Some(Instant::now() + REPLAY_WAIT);
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

    /// Cache the enriched view of an order.
    ///
    /// An order that has completed is not returned to a working status. Nothing
    /// remembered that an order was done, so a replayed frame — the reconnect
    /// open-order burst racing a fill, or any message the venue resends —
    /// wrote `Submitted` over the terminal entry, and `req_open_orders` then
    /// reported a completed order as live.
    ///
    /// The cached status alone cannot carry that knowledge, because completing
    /// an order evicts its cache row: the replayed frame finds nothing to refuse
    /// and inserts itself. The completed-id memory is what survives the
    /// eviction, and an intervening terminal report cannot overwrite the
    /// evidence the way a cached string could.
    ///
    /// A correction from the venue is not a replay and goes through
    /// [`push_order_correction`](Self::push_order_correction).
    #[doc(hidden)] pub fn push_order_info(&self, order_id: u64, info: RichOrderInfo) {
        // Every id the venue names, whatever became of the order under it. A
        // withdrawn id is free again and a filled one is not, so counting past
        // the working set alone handed out an id a fill had spent and the
        // venue refused it.
        self.working_id_watermark.fetch_max(order_id, Ordering::AcqRel);
        if order_id <= u32::MAX as u64 {
            self.narrow_id_watermark.fetch_max(order_id, Ordering::AcqRel);
        }
        if crate::types::order_status::is_open_status(&info.order_state.status) {
            if self.recently_completed(order_id) {
                return;
            }
            // Held from the test to the insert. Taken and dropped and taken
            // again, a removal landing in the gap let a completed order be
            // written back as an open one — the test saw the record, the
            // removal took it away, and the insert put a stale view of it back.
            let mut cache = self.order_cache.lock().unwrap();
            if cache.get(&order_id).is_some_and(|e| {
                crate::types::order_status::is_terminal_status(
                    &e.order_state.status,
                    &e.order_state.completed_status,
                )
            }) {
                return;
            }
            cache.insert(order_id, info);
            return;
        }
        self.order_cache.lock().unwrap().insert(order_id, info);
    }

    /// Cache a view that supersedes a completed one.
    ///
    /// A trade cancel or trade correction restates an execution the venue has
    /// already reported, so it can legitimately return a filled order to a
    /// working quantity. That is the venue's statement rather than a
    /// replay of an older one, so it is not refused, and the order stops being
    /// remembered as completed.
    #[doc(hidden)] pub fn push_order_correction(&self, order_id: u64, info: RichOrderInfo) {
        self.completed.lock().unwrap().remove(&order_id);
        self.order_cache.lock().unwrap().insert(order_id, info);
    }
}
