use crate::engine::market_state::MarketState;
use crate::types::*;
use std::collections::HashMap;
use std::time::Instant;


/// TSC-calibrated clock for hot-path timestamps.
pub struct Clock {
    start: std::time::Instant,
    /// What the wall clock read when `start` was taken, in nanoseconds since
    /// the epoch. Read once, so a timestamp costs an elapsed-time read and an
    /// addition rather than a syscall, and stays monotonic across a wall-clock
    /// step that would otherwise move a fill backwards.
    epoch_base_ns: u64,
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
            epoch_base_ns: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64),
        }
    }

    /// Nanoseconds since the epoch, monotonic. Fast, no syscall.
    ///
    /// Anchored to the wall clock once at construction and advanced by elapsed
    /// time, so a caller can compare it to a time the venue stated while a
    /// wall-clock adjustment never moves it backwards.
    #[inline(always)]
    pub fn now_ns(&self) -> u64 {
        self.epoch_base_ns + self.start.elapsed().as_nanos() as u64
    }

    /// Wall-clock Unix timestamp in seconds.
    pub fn now_utc(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }
}

/// The context passed to strategy callbacks. Provides market data access and
/// order management. All hot-path data is pre-allocated.
pub struct Context {
    pub(crate) market: MarketState,
    positions: Box<[f64]>,
    open_orders: HashMap<OrderId, Order>,
    /// Slots an order has just stopped holding, for the loop to reconsider.
    ///
    /// A slot is freed when the last thing referring to it goes, and until now
    /// only a cancelled subscription asked. An order — or a preview, which is
    /// tracked as one — held its slot for the life of the session, so a
    /// program working a chain a few thousand contracts wide ran the table out
    /// and was refused the next contract it named.
    pub slots_to_reconsider: Vec<InstrumentId>,
    pub(crate) pending_orders: OrderBuffer,
    pub(crate) account: AccountState,
    clock: Clock,
    /// The counter behind the numbering helpers below, which the tests use to
    /// build a book. It floors on the working set alone, which is the rule
    /// that handed out an id a fill had spent — so it is not compiled into a
    /// shipped build at all, and the numbers a session gives out come from the
    /// mark every id the venue names raises.
    #[cfg(test)]
    next_order_id: OrderId,
    /// ClOrdID version counter per order for modify chaining (orderId.0 → .1 → .2).
    pub(crate) modify_versions: HashMap<OrderId, u32>,
    /// Last ClOrdID the server reported, or this client emitted, for each order, as
    /// it appeared on the wire. Used as the OrigClOrdID on cancel/modify so that
    /// legacy orders recorded without a `.{ver}` suffix still match.
    pub(crate) last_clord: HashMap<OrderId, String>,
    /// What an order was submitted as. A replace restates an order in full, so
    /// everything the submit stated has to still be here to be restated —
    /// without it a replaced order silently lost its algo, its all-or-none
    /// instruction and every other attribute it was placed with.
    pub(crate) submitted: HashMap<OrderId, Box<crate::types::OrderSpec>>,
    /// How many cancels have been sent for an order. A cancel names itself on
    /// tag 11, and a retry that reuses the previous name is a duplicate the
    /// server is entitled to drop — which is exactly the case a retry exists
    /// for. Counts so each attempt is a new name.
    pub(crate) cancel_attempts: HashMap<OrderId, u32>,
    /// What an order held before the latest replace went out, and the name it
    /// held it under. A replace writes its attempt into the record ahead of the
    /// venue's answer, and the answer can refuse it; this is the record that
    /// stands again then, so nothing the venue did not accept is restated by a
    /// later cancel or replace. The name goes with the terms: it is what a
    /// cancel states as the original, and the attempt's own name was written
    /// into `last_clord` ahead of the same answer.
    /// Keyed by the order rather than held beside the send, because the
    /// refusal arrives as a message of its own, later, on another path.
    pub(crate) pre_replace: HashMap<(OrderId, u32), (Order, String)>,
    /// Timestamp when the last farm socket recv returned data (for decode latency
    /// measurement).
    pub(crate) recv_at: Instant,
    /// Total hot loop iterations since start.
    pub(crate) loop_iterations: u64,
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether an order is on this slot and in a state the venue may still act on.
///
/// Stated once because two callers ask it — one wanting the orders, one only
/// wanting to know whether there are any — and a slot given back while one of
/// these is on it goes to another contract.
fn holds_the_slot(order: &Order, id: InstrumentId) -> bool {
    order.instrument == id
        && matches!(order.status,
            OrderStatus::PendingSubmit | OrderStatus::PreSubmitted | OrderStatus::Submitted |
            OrderStatus::PendingCancel | OrderStatus::PendingReplace |
            OrderStatus::PartiallyFilled | OrderStatus::Uncertain | OrderStatus::Inactive)
}

impl Context {
    pub fn new() -> Self {
        Self {
            market: MarketState::new(),
            positions: vec![0.0f64; MAX_INSTRUMENTS].into(),
            open_orders: HashMap::with_capacity(128),
            slots_to_reconsider: Vec::new(),
            pending_orders: OrderBuffer::new(),
            modify_versions: HashMap::new(),
            last_clord: HashMap::new(),
            submitted: HashMap::new(),
            cancel_attempts: HashMap::new(),
            pre_replace: HashMap::new(),
            account: AccountState::default(),
            clock: Clock::new(),
            // Settled on first use, against what the venue names as working.
            #[cfg(test)]
            next_order_id: 0,
            recv_at: Instant::now(),
            loop_iterations: 0,
        }
    }

    /// Take `n` consecutive ids, starting past anything working.
    ///
    /// The venue refuses an order naming an id it is still working and takes
    /// one whose order has been withdrawn or filled, so an id is spent only
    /// while its order is live. The venue names what is live at every connect,
    /// and those land in `open_orders`, so there is nothing to carry between
    /// runs.
    #[cfg(test)]
    fn take_order_ids(&mut self, n: OrderId) -> OrderId {
        let floor = self.open_orders.keys().copied().max().unwrap_or(0) + 1;
        let first = self.next_order_id.max(floor);
        self.next_order_id = first + n;
        first
    }

    // ── Market data (read, zero-copy) ──

    #[inline(always)]
    pub fn bid(&self, id: InstrumentId) -> Price {
        self.market.bid(id)
    }

    #[inline(always)]
    pub fn ask(&self, id: InstrumentId) -> Price {
        self.market.ask(id)
    }

    #[inline(always)]
    pub fn last(&self, id: InstrumentId) -> Price {
        self.market.last(id)
    }

    #[inline(always)]
    pub fn bid_size(&self, id: InstrumentId) -> Qty {
        self.market.bid_size(id)
    }

    #[inline(always)]
    pub fn ask_size(&self, id: InstrumentId) -> Qty {
        self.market.ask_size(id)
    }

    #[inline(always)]
    pub fn mid(&self, id: InstrumentId) -> Price {
        self.market.mid(id)
    }

    #[inline(always)]
    pub fn spread(&self, id: InstrumentId) -> Price {
        self.market.spread(id)
    }

    #[inline(always)]
    pub fn quote(&self, id: InstrumentId) -> &Quote {
        self.market.quote(id)
    }

    // ── Positions & orders (read) ──

    #[inline(always)]
    /// The holding, exactly as the account states it. Fractional: half a share
    /// is a holding, and a whole-number table reported it as flat — in the
    /// position, in the profit and loss, and in the guard that decides whether
    /// a contract's slot may be handed to another.
    pub fn position(&self, id: InstrumentId) -> f64 {
        self.positions[id as usize]
    }

    /// The orders on this contract a cancel-all has to reach.
    ///
    /// Includes `Inactive`. The venue holds such an order and it can return to
    /// working; a stop held until the session opens is the ordinary case. A
    /// cancel for one the venue has finished with returns an unknown-order
    /// reject, which the reject handler retires.
    pub fn open_orders_for(&self, id: InstrumentId) -> Vec<&Order> {
        self.open_orders.values().filter(|o| holds_the_slot(o, id)).collect()
    }

    /// Whether anything the venue may still act on is on this slot.
    ///
    /// The same question [`open_orders_for`](Self::open_orders_for) answers,
    /// asked where only the answer is wanted: it is the first guard on every
    /// attempt to give a slot back, and building a list to ask it allocated on
    /// the loop's own path.
    pub fn has_open_orders_for(&self, id: InstrumentId) -> bool {
        self.open_orders.values().any(|o| holds_the_slot(o, id))
    }

    pub fn order(&self, order_id: OrderId) -> Option<&Order> {
        self.open_orders.get(&order_id)
    }

    pub fn account(&self) -> &AccountState {
        &self.account
    }

    // ── Order management (write to pre-allocated buffer) ──

    /// Queue an order, and answer with the number this session gave it.
    ///
    /// What one order differs from another by is its kind; the rest of a
    /// request is the same four values whatever kind it is.
    ///
    /// The quantity is in whole shares. A fraction of one goes through
    /// [`Context::submit_limit_fractional`], which states it fixed-point.
    #[cfg(test)]
    pub(crate) fn submit(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        qty: u32,
        kind: OrderKind,
        tif: u8,
        attrs: OrderAttrs,
    ) -> OrderId {
        let id = self.take_order_ids(1);
        self.pending_orders.push(OrderRequest::SubmitEx {
            order_id: id, instrument, side, qty: qty_from_wire(qty as i64), kind, tif, attrs,
        });
        id
    }

    /// Submit a bracket order: limit entry + take-profit limit + stop-loss stop.
    /// Returns (parent_id, take_profit_id, stop_loss_id).
    #[cfg(test)]
    pub(crate) fn submit_bracket(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        qty: u32,
        entry_price: Price,
        take_profit: Price,
        stop_loss: Price,
    ) -> (OrderId, OrderId, OrderId) {
        let parent_id = self.take_order_ids(3);
        let tp_id = parent_id + 1;
        let sl_id = parent_id + 2;
        self.pending_orders.push(OrderRequest::SubmitBracket {
            parent_id,
            tp_id,
            sl_id,
            instrument,
            side,
            qty: qty_from_wire(qty as i64),
            entry_price,
            take_profit,
            stop_loss,
        });
        (parent_id, tp_id, sl_id)
    }

    /// Submit a limit order for a quantity stated fixed-point, so it may be a
    /// fraction of a share.
    ///
    /// The quantity is shares multiplied by `QTY_SCALE`; half a share is
    /// `QTY_SCALE / 2`. Written out as a number, an example goes stale the
    /// day the scale changes and quietly teaches a caller to submit a
    /// fraction of what it meant.
    ///
    /// This is [`Context::submit`] with the quantity already scaled. It
    /// carries the same request, so a fractional order goes out through the
    /// same encoder as every other order rather than a path of its own.
    #[cfg(test)]
    pub(crate) fn submit_limit_fractional(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        qty: Qty,
        price: Price,
    ) -> OrderId {
        let id = self.take_order_ids(1);
        self.pending_orders.push(OrderRequest::SubmitEx {
            order_id: id,
            instrument,
            side,
            qty,
            kind: OrderKind::Limit { price },
            tif: b'0',
            attrs: OrderAttrs::default(),
        });
        id
    }

    pub fn cancel(&mut self, order_id: OrderId) {
        self.pending_orders.push(OrderRequest::Cancel { order_id });
    }

    pub fn cancel_all(&mut self, instrument: InstrumentId) {
        self.pending_orders
            .push(OrderRequest::CancelAll { instrument });
    }

    /// `outside_rth` is asserted on the replace: the tracked record has no
    /// field for it, so it has to come from the caller. Use
    /// `modify_ex` to also restate the order type, the time-in-force or the
    /// trigger.
    pub fn modify(&mut self, order_id: OrderId, price: Price, qty: u32, outside_rth: bool) -> OrderId {
        // Not supplied: on a trigger-only order the single price argument can
        // only have meant the trigger, and the builder routes it there.
        self.modify_ex(order_id, price, qty, outside_rth, 0, 0, 0)
    }

    /// Replace a working order, stating what the replace should carry.
    ///
    /// A zero `ord_type`, `tif` or `stop_price` states nothing and leaves what
    /// the resting order holds in force.
    pub fn modify_ex(
        &mut self,
        order_id: OrderId,
        price: Price,
        qty: u32,
        outside_rth: bool,
        ord_type: u8,
        tif: u8,
        stop_price: Price,
    ) -> OrderId {
        self.pending_orders.push(OrderRequest::Modify {
            order_id,
            price,
            stop_price,
            qty: qty_from_wire(qty as i64),
            outside_rth,
            ord_type,
            tif,
        });
        order_id
    }

    // ── Timing ──

    #[inline(always)]
    pub fn now_ns(&self) -> u64 {
        self.clock.now_ns()
    }

    pub fn now_utc(&self) -> i64 {
        self.clock.now_utc()
    }

    /// Timestamp when the last farm socket recv returned data.
    #[inline(always)]
    pub fn recv_timestamp(&self) -> Instant {
        self.recv_at
    }

    /// Total hot loop iterations since start.
    #[inline(always)]
    pub fn loop_iterations(&self) -> u64 {
        self.loop_iterations
    }

    // ── Instrument management ──

    pub fn register_instrument(&mut self, con_id: i64) -> InstrumentId {
        self.market.register(con_id)
    }

    /// Register without panicking when the instrument table is full. Inbound
    /// message handling must use this — a full table is a condition to report,
    /// not one to abort the engine on.
    pub fn try_register_instrument(&mut self, con_id: i64) -> Option<InstrumentId> {
        self.market.try_register(con_id)
    }

    pub fn set_symbol(&mut self, id: InstrumentId, symbol: String) {
        self.market.set_symbol(id, symbol);
    }

    /// Say what kind of contract this is and where it trades.
    ///
    /// An instrument registered by contract id alone has a symbol and nothing
    /// else, and an order restates the identity from what is known — which,
    /// with nothing said, is a stock on the default venue. A forex or futures
    /// contract sent that way names a contract that does not exist, and the
    /// venue says so. There was no way to say otherwise from here.
    pub fn set_routing(&mut self, id: InstrumentId, sec_type: &str, exchange: &str) {
        self.market.set_routing(id, sec_type, exchange);
    }

    /// State the rest of a contract's identity: expiry, strike, right and
    /// multiplier, `|`-separated, as a definition lookup reports them.
    ///
    /// A future or an option needs these on the order as well as its symbol.
    /// Without them the venue has a family and no member of it, and answers
    /// that the contract is ambiguous.
    pub fn set_order_identity(&mut self, id: InstrumentId, key: &str) {
        self.market.set_order_identity(id, key);
    }

    pub fn set_quote(&mut self, id: InstrumentId, quote: Quote) {
        *self.market.quote_mut(id) = quote;
    }

    pub fn quote_mut(&mut self, id: InstrumentId) -> &mut Quote {
        self.market.quote_mut(id)
    }

    // ── Engine-internal methods ──

    pub fn drain_pending_orders(&mut self) -> std::vec::Drain<'_, OrderRequest> {
        self.pending_orders.drain()
    }

    pub fn update_position(&mut self, instrument: InstrumentId, delta: f64) {
        self.positions[instrument as usize] += delta;
    }

    pub fn insert_order(&mut self, order: Order) {
        let oid = order.order_id;
        self.open_orders.insert(oid, order);
        // Initialize modify version to 0 for new orders (don't reset on modify).
        self.modify_versions.entry(oid).or_insert(0);
    }

    /// Apply a server-reported status. Returns true when the stored status
    /// actually changed. Guarded: a stale or reordered frame must
    /// not regress the lifecycle — terminal states are absorbing, and a
    /// lower-rank status never overwrites a higher one. A rejection does not
    /// displace a pending cancel either: it answers the request that raced
    /// the cancel, and the cancel's own verdict is still owed. Deliberate
    /// regressions go through `set_order_status_forced`.
    ///
    /// `replayed` marks a report that restates history: PossResend (tag 97)
    /// or PossDupFlag (tag 43). Such a report cannot move an order out of
    /// PendingCancel.
    pub fn update_order_status(
        &mut self,
        order_id: OrderId,
        status: OrderStatus,
        replayed: bool,
    ) -> bool {
        // Read before the record is taken mutably below, which is the only
        // reason it is read here.
        let replace_outstanding = self.replace_is_outstanding(order_id);
        if let Some(order) = self.open_orders.get_mut(&order_id) {
            let prev = order.status;
            if prev == status {
                return false;
            }
            // PendingCancel does not supersede the working states. The ranks
            // are a total order and cannot express this on their own, because
            // a partially filled order must still be able to reach
            // PendingCancel.
            //
            // A report stating the order is working moves it out of
            // PendingCancel. A remark on an order arrives as a pending cancel
            // with the acceptance immediately behind it, and a cancel the
            // venue declines leaves the order working.
            //
            // A replayed report is excluded. Recent activity is replayed when
            // a session opens, and a replayed working status predates any
            // cancel sent since.
            let resumes_working = !replayed
                && prev == OrderStatus::PendingCancel
                && matches!(status, OrderStatus::PreSubmitted | OrderStatus::Submitted);
            // A rejection arriving behind a pending cancel answers the request
            // that raced the cancel, not the cancel itself: the venue still
            // owes the cancel its own verdict, and the order stands as
            // in-flight until that lands rather than being reported done.
            // Only where a replace is actually outstanding. Without one, a
            // rejection arriving behind a cancel IS the venue's word on the
            // order — it will send nothing further, and holding the order
            // in flight for a verdict that never comes leaves it pending for
            // ever. The record of what the order held before a replace is the
            // one thing that says whether the venue owes an answer to
            // something other than the cancel.
            let cancel_still_owed = prev == OrderStatus::PendingCancel
                && status == OrderStatus::Rejected
                && replace_outstanding;
            if !resumes_working && (cancel_still_owed || prev.is_terminal() || status.rank() < prev.rank()) {
                log::debug!(
                    "Order {order_id} status guard: keeping {prev:?}, dropping stale {status:?}",
                );
                return false;
            }
            order.status = status;
            true
        } else {
            false
        }
    }

    /// Set a status unconditionally — for deliberate lifecycle regressions
    /// only (cancel-reject restore, disconnect reconciliation).
    pub fn set_order_status_forced(&mut self, order_id: OrderId, status: OrderStatus) {
        if let Some(order) = self.open_orders.get_mut(&order_id) {
            order.status = status;
        }
    }

    /// Move what an order has filled by a signed amount, `QTY_SCALE`
    /// fixed-point.
    ///
    /// A busted trade restates the order's cumulative quantity downwards, so
    /// the delta may be negative. Saturates at zero.
    pub fn adjust_order_filled(&mut self, order_id: OrderId, delta: Qty) {
        if let Some(order) = self.open_orders.get_mut(&order_id) {
            order.filled = order.filled.saturating_add(delta).max(0);
        }
    }

    /// Stop tracking an order, keeping its ClOrdID chain.
    ///
    /// Use this where the order may still exist at the broker — a replace that
    /// failed to send leaves the previous version working, and a cancel for it
    /// has to state the ClOrdID the broker last recorded.
    pub fn remove_order(&mut self, order_id: OrderId) {
        if let Some(order) = self.open_orders.remove(&order_id) {
            self.slots_to_reconsider.push(order.instrument);
        }
    }

    /// Drop an order and everything keyed to it, for an order that is over.
    ///
    /// The two ClOrdID maps only serve orders that can still be cancelled or
    /// replaced, and nothing pruned them: a process left running for weeks
    /// held one entry per order it had ever placed, in both.
    pub fn retire_order(&mut self, order_id: OrderId) {
        self.remove_order(order_id);
        self.modify_versions.remove(&order_id);
        self.last_clord.remove(&order_id);
        self.submitted.remove(&order_id);
        self.cancel_attempts.remove(&order_id);
        self.pre_replace.retain(|(id, _), _| *id != order_id);
    }

    /// Take the venue's own account of where an order's naming stands.
    ///
    /// `modify_versions` counts the revisions this client has issued, and the
    /// next replace names one past it. An order the venue names at connect
    /// carries the revision it reached in an earlier session, and a counter
    /// starting at zero beside it named revisions the venue had already been
    /// given: the replace went out under a name the venue held, and the cancel
    /// behind it named a revision the venue had superseded and was answered
    /// that no such order exists — which retires the record here while the
    /// order goes on working there.
    ///
    /// What was kept against a refusal goes too. It was written ahead of an
    /// answer that is now moot, and the venue has just said what it holds;
    /// left behind, a later refusal would put the superseded terms and the
    /// superseded name back over the venue's own word.
    pub fn reconcile_recovered_revision(&mut self, order_id: OrderId, revision: u32) {
        let issued = self.modify_versions.entry(order_id).or_insert(0);
        *issued = (*issued).max(revision);
        self.pre_replace.retain(|(id, _), _| *id != order_id);
    }

    /// Whether the venue still owes an answer to a revision of this order.
    pub fn replace_is_outstanding(&self, order_id: OrderId) -> bool {
        self.pre_replace.keys().any(|(id, _)| *id == order_id)
    }

    /// Put back what the venue is known to hold, where it refused a revision.
    ///
    /// By the revision refused, not by the order: revisions overlap — the venue
    /// takes a second before it has answered the first — and one fallback per
    /// order meant the answer to one revision spent or restored another's. A
    /// refusal then put back the terms of an attempt the venue had itself
    /// refused, or found nothing to put back at all and left the record showing
    /// a price the venue had never accepted.
    ///
    /// The record took the attempt ahead of the venue's answer; where the
    /// answer is a refusal, the attempt must not stand — every later cancel
    /// and replace restates from the record, and would carry the refused
    /// terms. A fill that raced the attempt is the venue's own word and
    /// survives the restore, and the status follows it.
    ///
    /// The name the order carries goes back with its terms. It was written
    /// ahead of the same answer, for the same reason, and a cancel states it
    /// as the original: left on the refused revision, the venue answered that
    /// it knew no such order and the order went on working out of reach of a
    /// withdrawal. Nothing later corrected it, because the recorded name only
    /// ever moves forward.
    pub fn restore_pre_replace(&mut self, order_id: OrderId, revision: u32) {
        // Every later revision was built on terms the venue never held, so its
        // fallback records a state that never existed. They go with this one.
        self.pre_replace.retain(|(id, ver), _| *id != order_id || *ver <= revision);
        let Some((mut prior, name)) = self.pre_replace.remove(&(order_id, revision)) else { return };
        if let Some(current) = self.open_orders.get(&order_id) {
            prior.filled = current.filled;
        }
        if prior.filled > 0 && prior.status.rank() < OrderStatus::PartiallyFilled.rank() {
            prior.status = OrderStatus::PartiallyFilled;
        }
        self.insert_order(prior);
        self.last_clord.insert(order_id, name);
    }

    /// Mark all live open orders as Uncertain (auth disconnect — status may have
    /// changed).
    ///
    /// The engine stops believing these statuses at this point and the API
    /// layer went on reporting them, so `req_open_orders` kept asserting a
    /// status the engine no longer had. Pair with `uncertain_orders`
    /// to tell the application.
    pub fn mark_orders_uncertain(&mut self) {
        for order in self.open_orders.values_mut() {
            match order.status {
                OrderStatus::PendingSubmit | OrderStatus::PreSubmitted | OrderStatus::Submitted |
                OrderStatus::PendingCancel | OrderStatus::PendingReplace |
                OrderStatus::PartiallyFilled |
                // Includes `Inactive`: the venue holds such an order and it
                // can return to working, so its state after a disconnect is
                // unknown like any other.
                OrderStatus::Inactive => {
                    order.status = OrderStatus::Uncertain;
                }
                _ => {}
            }
        }
    }

    /// Orders still Uncertain — those the reconnect did not account for.
    pub fn uncertain_orders(&self) -> Vec<Order> {
        self.open_orders.values()
            .filter(|o| o.status == OrderStatus::Uncertain)
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests;
