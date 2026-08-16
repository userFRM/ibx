//! What has been submitted, filled, and refused.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use std::sync::Mutex;
use std::collections::HashMap;
use crate::types::*;

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
    pub(super) completed: Mutex<HashMap<u64, Instant>>,
    /// Set when the server finishes naming the orders already working, which
    /// it does unprompted after a connect. Until then "none" and "not yet
    /// told" look the same to a caller.
    replay_done: AtomicBool,
    /// Reason for a genuinely-Inactive (39=I) transition: (order_id, ibapi
    /// error code, message). ibapi has no callback dedicated to "order
    /// parked with reason", so this is drained into `Wrapper::error` the
    /// same way a cancel/modify reject is.
    order_inactive: Mutex<Vec<(u64, i32, String)>>,
}

impl OrderState {
    pub(super) fn new() -> Self {
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

    /// Take every fills waiting, leaving none.
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

    /// Take every cancel rejects waiting, leaving none.
    pub fn drain_cancel_rejects(&self) -> Vec<CancelReject> {
        self.cancel_rejects.lock().unwrap().drain(..).collect()
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
    /// reported a completed order as live.
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
        if crate::types::order_status::is_open_status(&info.order_state.status) {
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
