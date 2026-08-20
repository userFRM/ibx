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
    positions: [f64; MAX_INSTRUMENTS],
    open_orders: HashMap<OrderId, Order>,
    pub(crate) pending_orders: OrderBuffer,
    pub(crate) account: AccountState,
    clock: Clock,
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

impl Context {
    pub fn new() -> Self {
        Self {
            market: MarketState::new(),
            positions: [0.0f64; MAX_INSTRUMENTS],
            open_orders: HashMap::with_capacity(128),
            pending_orders: OrderBuffer::new(),
            modify_versions: HashMap::new(),
            last_clord: HashMap::new(),
            submitted: HashMap::new(),
            cancel_attempts: HashMap::new(),
            account: AccountState::default(),
            clock: Clock::new(),
            next_order_id: {
                // Epoch-based to avoid "Duplicate ID" across IB sessions
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                secs * 1000
            },
            recv_at: Instant::now(),
            loop_iterations: 0,
        }
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
        self.open_orders
            .values()
            .filter(|o| o.instrument == id && matches!(o.status,
                OrderStatus::PendingSubmit | OrderStatus::PreSubmitted | OrderStatus::Submitted |
                OrderStatus::PendingCancel | OrderStatus::PendingReplace |
                OrderStatus::PartiallyFilled | OrderStatus::Uncertain | OrderStatus::Inactive))
            .collect()
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
    pub fn submit(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        qty: u32,
        kind: OrderKind,
        tif: u8,
        attrs: OrderAttrs,
    ) -> OrderId {
        let id = self.next_order_id;
        self.next_order_id += 1;
        self.pending_orders.push(OrderRequest::SubmitEx {
            order_id: id, instrument, side, qty, kind, tif, attrs,
        });
        id
    }

    /// Submit a bracket order: limit entry + take-profit limit + stop-loss stop.
    /// Returns (parent_id, take_profit_id, stop_loss_id).
    pub fn submit_bracket(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        qty: u32,
        entry_price: Price,
        take_profit: Price,
        stop_loss: Price,
    ) -> (OrderId, OrderId, OrderId) {
        let parent_id = self.next_order_id;
        let tp_id = self.next_order_id + 1;
        let sl_id = self.next_order_id + 2;
        self.next_order_id += 3;
        self.pending_orders.push(OrderRequest::SubmitBracket {
            parent_id,
            tp_id,
            sl_id,
            instrument,
            side,
            qty,
            entry_price,
            take_profit,
            stop_loss,
        });
        (parent_id, tp_id, sl_id)
    }

    /// Submit a fractional shares limit order.
    ///
    /// The quantity is fixed-point: shares multiplied by `QTY_SCALE`. Half a
    /// share is `QTY_SCALE / 2`. Written out as a number, an example goes
    /// stale the day the scale changes and quietly teaches a caller to submit
    /// a fraction of what it meant.
    pub fn submit_limit_fractional(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        qty: Qty,
        price: Price,
    ) -> OrderId {
        let id = self.next_order_id;
        self.next_order_id += 1;
        self.pending_orders.push(OrderRequest::SubmitLimitFractional {
            order_id: id, instrument, side, qty, price,
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
            qty,
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
    /// lower-rank status never overwrites a higher one. Deliberate
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
            if !resumes_working && (prev.is_terminal() || status.rank() < prev.rank()) {
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

    pub fn update_order_filled(&mut self, order_id: OrderId, last_shares: u32) {
        self.adjust_order_filled(order_id, last_shares as i64);
    }

    /// Move what an order has filled by a signed amount.
    ///
    /// A busted trade restates the order's cumulative quantity downwards, so
    /// the delta may be negative. Saturates at zero.
    pub fn adjust_order_filled(&mut self, order_id: OrderId, delta: i64) {
        if let Some(order) = self.open_orders.get_mut(&order_id) {
            order.filled = (order.filled as i64 + delta).clamp(0, u32::MAX as i64) as u32;
        }
    }

    /// Stop tracking an order, keeping its ClOrdID chain.
    ///
    /// Use this where the order may still exist at the broker — a replace that
    /// failed to send leaves the previous version working, and a cancel for it
    /// has to state the ClOrdID the broker last recorded.
    pub fn remove_order(&mut self, order_id: OrderId) {
        self.open_orders.remove(&order_id);
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
