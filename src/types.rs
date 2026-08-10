/// Internal instrument identifier. Mapped from IB's conId at subscription time.
/// Used as an index into pre-allocated arrays, so values are dense and small.
pub type InstrumentId = u32;

/// Engine-assigned order identifier.
pub type OrderId = u64;

/// Fixed-point price: value * 10^8. Avoids floating-point on the hot path.
/// Example: $150.25 = 15_025_000_000
pub type Price = i64;

/// Fixed-point quantity: value * `QTY_SCALE`.
/// Example: 100 shares = 1_000_000
pub type Qty = i64;

/// How a price is held here: a whole number of hundred-millionths.
///
/// This is not how the venue sends one. The venue sends a price as a whole
/// number of the CONTRACT'S OWN smallest increment — the counterpart holds a
/// price as that count beside the increment it counts, and converts only when
/// something needs a decimal. That representation has no floor: a contract
/// quoted in millionths works exactly as well as one quoted in pennies, because
/// the count is relative to the contract rather than to a fixed scale.
///
/// Holding a price against one fixed scale instead buys simple arithmetic
/// between contracts and costs a floor. A contract whose increment is finer
/// than a hundred-millionth cannot be held at all, and its increment scales to
/// nothing — which is caught rather than silently rounded, but caught is not
/// carried. Matching the venue would mean holding the count and the increment
/// together and converting at the edge, which is a change to every price this
/// client touches.
pub const PRICE_SCALE: i64 = 100_000_000; // 10^8
/// Quantities are held to a hundred-millionth, the same as prices.
///
/// A size is a count of what the venue said sizes move in for the contract,
/// and for a crypto that is a hundred-millionth of a coin. Held to a
/// ten-thousandth, every size finer than that rounded to nothing: a quote for
/// a thousandth of a coin came back as no quote at all, which reads as an
/// empty book rather than as a limit of this client's.
///
/// A day's volume in the busiest listing is some thousands of millions of
/// shares, which at this scale is four orders of magnitude inside what the
/// field holds.
pub const QTY_SCALE: i64 = 100_000_000; // 10^8

/// Convert a wire quantity into the `QTY_SCALE` fixed-point form the `Quote`
/// fields hold. Every reader divides by `QTY_SCALE`, so a decode path that
/// stores the magnitude raw delivers quantities 10_000x too small (ibx#287).
/// Saturating: the magnitude is server-supplied, and a clamped quantity is
/// preferable to a wrapped one.
#[inline(always)]
pub fn qty_from_wire(magnitude: i64) -> Qty {
    magnitude.saturating_mul(QTY_SCALE)
}

/// Convert a counted size into the `QTY_SCALE` fixed-point form, where the
/// venue stated what it counts this instrument's sizes in.
///
/// A size on the wire is a count of the increment the venue named on the
/// subscription acknowledgement: whole ones for a share, hundred-millionths
/// for a crypto. Counting every one as whole ones reports a crypto's size a
/// hundred million times over. An instrument the venue stated no increment
/// for is counted in whole ones, which is what stating none means.
#[inline]
pub fn qty_from_counted(counted: i64, size_tick: f64) -> Qty {
    if size_tick <= 0.0 || size_tick == 1.0 {
        return qty_from_wire(counted);
    }
    (counted as f64 * size_tick * QTY_SCALE as f64).round() as Qty
}

/// Snap a fixed-point price to the nearest multiple of `tick` (ties round
/// away from zero). A non-positive tick means the grid is unknown and the
/// price is returned unchanged. Pure integer math — exact on the fixed-point
/// representation. See ibx#216.
pub fn snap_to_tick(price: Price, tick: i64) -> Price {
    if tick <= 0 {
        return price;
    }
    let half = tick / 2;
    if price >= 0 {
        ((price + half) / tick) * tick
    } else {
        -(((-price + half) / tick) * tick)
    }
}

/// Maximum number of concurrently tracked instruments.
pub const MAX_INSTRUMENTS: usize = 256;

/// Maximum pending order requests per tick cycle.
const MAX_PENDING_ORDERS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
    /// Short sell (FIX tag 54 = "5"). Used for short-selling stocks.
    ShortSell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    /// Locally queued, not yet acknowledged by server.
    PendingSubmit,
    /// Received by server, not yet accepted by exchange (FIX 39=A).
    PreSubmitted,
    /// Accepted and working on exchange (FIX 39=0 or 39=5).
    Submitted,
    /// Cancel request sent, awaiting confirmation (FIX 39=6).
    PendingCancel,
    /// Modify request sent, awaiting confirmation (FIX 39=E).
    PendingReplace,
    Filled,
    PartiallyFilled,
    Cancelled,
    Rejected,
    /// Server reports order inactive (FIX 39=I).
    Inactive,
    /// Order state is unknown due to an auth connection disconnect.
    /// Will be reconciled when reconnection completes (mass status request).
    Uncertain,
}

impl OrderStatus {
    /// Lifecycle progress rank for the monotonic status guard (ibx#212).
    /// A stale or reordered frame must not move an order's reported status
    /// backwards (e.g. a late PreSubmitted after Submitted, or a mass-status
    /// snapshot after a fill). Same-rank transitions are free — the tiers
    /// group states that legitimately alternate. Deliberate regressions
    /// (cancel-reject restore, disconnect reconciliation) bypass the guard
    /// via `Context::set_order_status_forced`.
    pub fn rank(self) -> u8 {
        match self {
            Self::Uncertain => 0,
            Self::PendingSubmit => 1,
            Self::PreSubmitted => 2,
            // Working tier: a modify ack returns PendingReplace to
            // Submitted, and Inactive orders can reactivate.
            Self::Submitted | Self::PendingReplace | Self::Inactive => 3,
            // A partially filled order can still be cancelled, and a fill
            // can land while a cancel is pending.
            Self::PendingCancel | Self::PartiallyFilled => 4,
            Self::Filled | Self::Cancelled | Self::Rejected => 5,
        }
    }

    /// Terminal states are absorbing: no ordinary frame may leave them.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::Rejected)
    }
}

/// Current quote for an instrument. Cache-line aligned for hot-path access.
#[derive(Clone, Copy)]
#[repr(C, align(64))]
#[derive(Default)]
pub struct Quote {
    pub bid: Price,
    pub ask: Price,
    pub last: Price,
    pub bid_size: Qty,
    pub ask_size: Qty,
    pub last_size: Qty,
    pub volume: Qty,
    pub open: Price,
    pub high: Price,
    pub low: Price,
    pub close: Price,
    pub timestamp_ns: u64,
    /// Bid-exchange bitmask. Each set bit indexes into smart_components by bit_number.
    /// Hypothesis pending wire-format confirmation; see deepentropy/ib-agent#120.
    pub bid_exch_mask: i64,
    pub ask_exch_mask: i64,
    pub last_exch_mask: i64,
    /// Whether the venue has halted trading in this contract.
    ///
    /// 0 not halted, 1 halted, 2 halted for news pending. The venue states it
    /// and it was decoded and dropped, so a halted contract read as a live
    /// market: every surface kept presenting the last price before the halt as
    /// a current one, and a program pricing against it is pricing against a
    /// book that is not there.
    pub halted: i64,
}


/// Execution fill report.
#[derive(Debug, Clone, Copy)]
pub struct Fill {
    pub instrument: InstrumentId,
    pub order_id: OrderId,
    pub side: Side,
    pub price: Price,
    pub qty: i64,
    pub remaining: i64,
    pub commission: Price,
    pub timestamp_ns: u64,
    /// FIX tag 14 CumQty — filled across the whole order, not this print.
    pub cum_qty: i64,
    /// FIX tag 6 AvgPx — volume-weighted across every print of this order.
    /// `price` is this print alone.
    pub avg_price: Price,
}

/// Order status change notification.
#[derive(Debug, Clone, Copy)]
pub struct OrderUpdate {
    pub order_id: OrderId,
    pub instrument: InstrumentId,
    pub status: OrderStatus,
    pub filled_qty: f64,
    pub remaining_qty: f64,
    /// What the order has paid on average so far, as the report states it.
    /// Zero when nothing has filled.
    pub avg_price: Price,
    pub perm_id: i64,
    pub parent_id: i64,
    pub timestamp_ns: u64,
}

/// A holding the venue reports that this broker does not hold itself.
///
/// The venue keeps three sets of holdings: its own, those held away at another
/// broker, and rows it marks as shown but not held. Only the first is what a
/// caller asking for positions means, so the others are kept here rather than
/// added to them.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionElsewhere {
    pub con_id: i64,
    pub symbol: String,
    pub sec_type: String,
    pub currency: String,
    pub position: f64,
    pub avg_cost: Price,
    /// Where the venue says it sits.
    pub held: HeldElsewhere,
}

/// Which of the venue's other sets of holdings a row came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeldElsewhere {
    /// Held away, at another broker.
    Away,
    /// Shown but not held.
    DisplayOnly,
    /// Reported apart from the account's own holdings, without saying why.
    Aside,
}

/// What the venue makes of an option: its own model price, the greeks and the
/// volatility that price implies.
///
/// A field the venue did not state is `f64::MAX`, the reference client's own
/// mark for a value that was not sent. Zero is a real greek.
#[derive(Debug, Clone, Copy)]
pub struct OptionComputation {
    pub instrument: InstrumentId,
    pub implied_vol: f64,
    pub delta: f64,
    pub opt_price: f64,
    pub pv_dividend: f64,
    pub gamma: f64,
    pub vega: f64,
    pub theta: f64,
    pub und_price: f64,
}

/// Cancel/modify reject notification (reject message).
#[derive(Debug, Clone, Copy)]
pub struct CancelReject {
    pub order_id: OrderId,
    pub instrument: InstrumentId,
    /// 1 = cancel rejected, 2 = modify rejected (FIX tag 434 CxlRejResponseTo).
    pub reject_type: u8,
    /// Numeric reason code (FIX tag 102 CxlRejReason). 0=TooLate, 1=UnknownOrder, etc.
    pub reason_code: i32,
    pub timestamp_ns: u64,
}

/// Multi-char OrdType discriminants (values < 32 to avoid collision with ASCII single-char types).
/// Used in `Order.ord_type` for order types whose FIX tag 40 value is more than one character.
pub const ORD_STP_PRT: u8 = 1;   // FIX "SP"  — Stop with Protection
pub const ORD_MIDPX: u8 = 2;     // FIX "MIDPX" — Mid-Price
pub const ORD_SNAP_MKT: u8 = 3;  // FIX "SMKT" — Snap to Market
pub const ORD_SNAP_MID: u8 = 4;  // FIX "SMID" — Snap to Midpoint
pub const ORD_SNAP_PRI: u8 = 5;  // FIX "SREL" — Snap to Primary
pub const ORD_PEG_MKT: u8 = 6;   // FIX "P" + ExecInst "P" — Pegged to Market
pub const ORD_PEG_MID: u8 = 7;   // FIX "P" + ExecInst "M" — Pegged to Midpoint
pub const ORD_PEG_BENCH: u8 = 8; // FIX "PB" — Pegged to Benchmark
/// A time-in-force this client does not know. A recovery record with no tag 59
/// states none, and the order was not placed by this session, so there is
/// nothing to recover it from. Distinct from every real code, so it reports as
/// unstated rather than as an ordinary value, and a replace omits tag 59 rather
/// than restating a guess as an instruction (ibx#307).
pub const TIF_UNSTATED: u8 = 0;

pub const ORD_WHAT_IF: u8 = 9;   // Not a real OrdType — marker for what-if orders

/// Convert an `ord_type` discriminant to the FIX tag 40 string.
/// Single-char types (ASCII >= 32) are stored as-is; multi-char types use constants above.
pub fn ord_type_fix_str(t: u8) -> &'static str {
    match t {
        ORD_STP_PRT => "SP",
        ORD_MIDPX => "MIDPX",
        ORD_SNAP_MKT => "SMKT",
        ORD_SNAP_MID => "SMID",
        ORD_SNAP_PRI => "SREL",
        // The venue names these back as PegToMkt and PegToMid under "P".
        // Sent as "E" it named them something else entirely, so a caller asking
        // for one had an order the venue read as another type.
        ORD_PEG_MKT | ORD_PEG_MID => "P",
        ORD_PEG_BENCH => "PB",
        b'1' => "1", b'2' => "2", b'3' => "3", b'4' => "4", b'5' => "5",
        b'B' => "B", b'E' => "E", b'J' => "J", b'K' => "K",
        b'P' => "P", b'R' => "R", b'U' => "U",
        _ => "2",
    }
}

/// What-If margin/commission preview response (execution report with tag 6091=1).
/// Returned when a what-if order is submitted — the order is NOT placed.
#[derive(Debug, Clone)]
pub struct WhatIfResponse {
    pub order_id: OrderId,
    pub instrument: InstrumentId,
    pub init_margin_before: Price,
    pub maint_margin_before: Price,
    pub equity_with_loan_before: Price,
    pub init_margin_after: Price,
    pub maint_margin_after: Price,
    pub equity_with_loan_after: Price,
    pub commission: Price,
    /// Where a commission is given as a range rather than a number, and the
    /// money it is quoted in. A preview that states the margin and not the cost
    /// is half a preview.
    pub min_commission: Price,
    pub max_commission: Price,
    pub commission_currency: String,
    /// What the venue warned about, which is its own text and not the order's.
    pub warning_text: String,
}

impl WhatIfResponse {
    /// What the order does to the margin, which the venue states as before and
    /// after and leaves to be taken as the difference.
    pub fn init_margin_change(&self) -> Price {
        self.init_margin_after - self.init_margin_before
    }
    pub fn maint_margin_change(&self) -> Price {
        self.maint_margin_after - self.maint_margin_before
    }
    pub fn equity_with_loan_change(&self) -> Price {
        self.equity_with_loan_after - self.equity_with_loan_before
    }
}

/// Adjusted order type for adjustable stops (FIX tag 6261).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustedOrderType {
    Stop,       // 3
    StopLimit,  // 4
    Trail,      // 7
    TrailLimit, // 8
}

impl AdjustedOrderType {
    pub fn fix_code(&self) -> &'static str {
        match self {
            Self::Stop => "3",
            Self::StopLimit => "4",
            Self::Trail => "7",
            Self::TrailLimit => "8",
        }
    }
}

/// A tracked open order.
#[derive(Debug, Clone, Copy)]
pub struct Order {
    pub order_id: OrderId,
    pub instrument: InstrumentId,
    pub side: Side,
    pub price: Price,
    pub qty: u32,
    pub filled: u32,
    pub status: OrderStatus,
    /// FIX tag 40 OrdType: b'1'=MKT, b'2'=LMT, b'3'=STP, b'4'=STPLMT, b'P'=TRAIL, etc.
    /// For multi-char OrdTypes (MIDPX, SP, SMKT, etc.), uses ORD_* constants (values < 32).
    pub ord_type: u8,
    /// FIX tag 59 TimeInForce: b'0'=DAY, b'1'=GTC, b'3'=IOC, b'4'=FOK
    pub tif: u8,
    /// FIX tag 99 stop price (for Stop/StopLimit/MIT/LIT orders)
    pub stop_price: Price,
}

impl Order {
    /// Create a new tracked order with FIX type metadata.
    pub fn new(order_id: OrderId, instrument: InstrumentId, side: Side, qty: u32, price: Price, ord_type: u8, tif: u8, stop_price: Price) -> Self {
        Self { order_id, instrument, side, price, qty, filled: 0, status: OrderStatus::PendingSubmit, ord_type, tif, stop_price }
    }
}

/// Adaptive algo priority level (IB's "adaptivePriority" parameter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptivePriority {
    Patient,
    Normal,
    Urgent,
}

impl AdaptivePriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            AdaptivePriority::Patient => "Patient",
            AdaptivePriority::Normal => "Normal",
            AdaptivePriority::Urgent => "Urgent",
        }
    }
}

/// Optional attributes for extended order submissions.
/// All fields default to "not set" (0/false).
/// What an order was submitted as, kept so a replace can restate it in full.
#[derive(Debug, Clone)]
pub struct OrderSpec {
    pub kind: OrderKind,
    pub attrs: OrderAttrs,
}

#[derive(Debug, Clone)]
pub struct OrderAttrs {
    /// Show on book as this many shares (tag 111). 0 = not set (show full qty).
    pub display_size: u32,
    /// Minimum fill quantity (FIX tag 110). 0 = not set.
    pub min_qty: u32,
    /// Hidden order — not displayed on book (IB tag 6135).
    pub hidden: bool,
    /// Allow trading outside regular hours (IB tag 6433).
    pub outside_rth: bool,
    /// Delay order activation until this time (FIX tag 168). 0 = not set. Unix seconds.
    pub good_after: i64,
    /// Auto-expire order at this instant (FIX tag 126, time-precise GTD).
    /// 0 = not set. Unix seconds in UTC. Mutually exclusive with `good_till_date_ymd`.
    /// When set, TIF should be GTD (but IB infers it from the tag).
    pub good_till: i64,
    /// Auto-expire order on this calendar date (FIX tag 432, date-only GTD).
    /// 0 = not set. Packed `YYYYMMDD`. Mutually exclusive with `good_till`.
    pub good_till_date_ymd: u32,
    /// OCA group ID (FIX tag 583). 0 = not set. Links orders so one cancels others.
    /// Orders sharing the same non-zero oca_group are in the same OCA group.
    pub oca_group: u64,
    /// OCA group as a string (FIX tag 583). Used by Python compat for user-specified OCA names.
    /// When non-empty, takes precedence over numeric `oca_group`.
    pub oca_group_str: String,
    /// Parent order ID (IB tag 6107). 0 = no parent. Links child orders to parent in brackets.
    pub parent_id: u64,
    /// Discretionary amount (IB tag 9813). 0 = not set. Fixed-point Price value.
    /// The amount above the limit price that the order may trade at.
    pub discretionary_amt: Price,
    /// Sweep to fill (IB tag 6102). Routes aggressively across exchanges.
    pub sweep_to_fill: bool,
    /// All or none (FIX tag 18=G ExecInst). Fill entire qty or nothing.
    pub all_or_none: bool,
    /// Implied volatility for a volatility order, as a decimal (0.25 = 25%),
    /// on tag 9816. Zero means the caller set none.
    pub volatility: f64,
    /// Let the venue better the price where it can, on tag 6557.
    pub seek_price_improvement: bool,
    /// When a person entered the order by hand, on tag 6532 — which is a
    /// regulatory record, not a preference.
    pub manual_order_time: String,
    /// Override an error the venue would otherwise refuse the order for, on
    /// tag 8229.
    pub advanced_error_override: String,
    /// The window an order is live in, on tags 6670 and 6671.
    pub active_start_time: String,
    pub active_stop_time: String,
    /// Never take liquidity, on tag 6605.
    pub post_only: bool,
    /// Whether the client asked for this order or the account holder did, on
    /// tags 6488 and 1028 — which is a regulatory statement, not a preference.
    pub solicited: bool,
    pub manual_order_indicator: i32,
    /// Route to the best bid or offer where the order is marketable, on tag 8265.
    pub route_marketable_to_bbo: bool,
    /// Take part in an auction only to correct an imbalance, on tag 6737.
    pub imbalance_only: bool,
    /// Take part in the opening auction, or stand out of it, on tags 6524 and 6562.
    pub allow_pre_open: bool,
    pub ignore_open_auction: bool,
    /// One of several orders the venue treats as a unit, on tag 6406.
    pub is_oms_container: bool,
    /// Who is operating the account and on whose behalf, on tags 8089, 6207 and 6636.
    pub ext_operator: String,
    pub customer_account: String,
    pub professional_customer: bool,
    /// The future a spread prices against, on tag 6564.
    pub ref_futures_con_id: i32,
    /// Who decided the trade and who executed it, for European transaction
    /// reporting, on tags 8237, 8243, 8254 and 8255.
    pub mifid2_decision_maker: String,
    pub mifid2_decision_algo: String,
    pub mifid2_execution_trader: String,
    pub mifid2_execution_algo: String,
    /// A midpoint peg's offset stated as a whole-tick part and a half-tick
    /// part, on tags 8403 and 8404. Both set is the other form of the peg.
    pub mid_offset_at_whole: f64,
    pub mid_offset_at_half: f64,
    /// Whether to let the venue manage the price of the order, on tag 8339.
    /// Zero is the caller saying nothing.
    pub use_price_mgmt_algo: i32,
    /// How long the order runs for, where its kind takes a duration, on tag 8402.
    pub duration: i32,
    /// The smallest size worth competing for, on tag 8411, and how far past the
    /// best price to compete, on tag 8412.
    pub min_compete_size: i32,
    pub compete_against_best_offset: f64,
    /// Whether the venue keeps re-pricing a volatility order as the underlying
    /// moves, on tag 6275.
    pub continuous_update: bool,
    /// Which price a volatility order takes its reference from, on tag 6279.
    /// Zero means the caller stated none.
    pub reference_price_type: i32,
    /// The band of underlying prices a volatility order stays active within, on
    /// tags 6152 and 6153. `f64::MAX` is this API's "not set" for a price.
    pub stock_range_lower: f64,
    pub stock_range_upper: f64,
    /// Whether the volatility above is daily or annual, on tag 6280. Zero means
    /// the caller set none.
    pub volatility_type: u8,
    /// Offset from the market for a relative order, as a decimal fraction, on
    /// tag 9822. `f64::MAX` means the caller set none.
    pub percent_offset: f64,
    /// Leave the order to a floor broker's discretion, on tag 6287.
    pub not_held: bool,
    /// The caller's own reference for this order, on tag 6010.
    pub order_ref: String,
    /// Whether this order opens or closes a position, on tag 77. Empty means
    /// the caller said nothing and the venue decides.
    pub open_close: String,
    /// The ladder, when this is a scale order.
    pub scale: Option<Box<ScaleAttrs>>,
    /// The hedging leg, when the caller asked for one.
    pub delta_neutral: Option<Box<DeltaNeutralAttrs>>,
    /// Short-sale handling: which slot (6086), where the shares are located
    /// (5700, stated only for slot 2) and the exemption reason (1688).
    pub short_sale_slot: u8,
    pub designated_location: String,
    pub exempt_code: i32,
    /// How this order hedges (6665) and the parameter that goes with it: a
    /// beta on 6703, a pair ratio on 6666. Delta and FX hedges take neither.
    pub hedge_type: u8,
    pub hedge_beta: f64,
    pub hedge_ratio: f64,
    /// The soft-dollar arrangement this order's commission goes to: which
    /// tier (tag 6519) and what it is worth (tag 6520). Taken from a caller
    /// and dropped, the commission went wherever the account's default sends
    /// it, which is not what the caller asked for.
    pub soft_dollar_tier_name: String,
    pub soft_dollar_tier_val: String,
    /// The caller's own name for the algo running this order (tag 8016),
    /// which comes back on every report about it.
    pub algo_id: String,
    /// Order capacity and originator, on tag 47 — who this order is for, which
    /// the venue treats as a regulatory statement rather than a preference.
    pub rule80a: String,
    /// Seconds an order rests on the alternative venue before routing on
    /// (tag 8405). Zero means the caller set none.
    pub post_to_ats: u32,
    /// Built but not sent: the venue holds it until it is activated (tag 6521).
    pub deactivate: bool,
    /// Stand the order down if this client's connection goes (tag 6661). What
    /// a headless client wants when nothing is left watching the order.
    pub deactivate_on_disconnect: bool,
    /// Let this order work the overnight session too (tag 8534).
    pub include_overnight: bool,
    /// Cancel this order's parent when it is cancelled (tag 6965).
    pub auto_cancel_parent: bool,
    /// Smallest quantity worth filling (tag 8415). Zero means none.
    pub min_trade_qty: u32,
    /// A block order, which the venue handles apart from the book (tag 9801).
    pub block_order: bool,
    /// The date the venue cancels this order by itself (tag 6596).
    pub auto_cancel_date: String,
    /// Where this order clears (tag 440) and how (tag 6419). Distinct from the
    /// account it trades in, which the order already names.
    pub clearing_account: String,
    pub clearing_intent: String,
    /// The legs, when this order is for a combination.
    pub combo_legs: Vec<ComboLegSpec>,
    /// Where the contract is listed (tag 207), which is not where the order
    /// routes.
    pub primary_exchange: String,
    /// The contract this order hedges against, and at what (tags 6150, 6148,
    /// 6149). Stated on the contract rather than the order.
    pub delta_neutral_contract: Option<Box<DeltaNeutralContractSpec>>,
    /// Trigger method for stop/MIT/LIT orders (IB tag 6115).
    /// 0=default, 1=double-bid-ask, 2=last, 3=double-last, 4=bid-ask,
    /// 7=last-or-bid-ask, 8=mid-point.
    pub trigger_method: u8,
    /// Cash quantity — order by dollar amount instead of shares (IB tag 5920). 0 = not set.
    /// Fixed-point Price value (e.g., $1000 = 1000 * PRICE_SCALE).
    pub cash_qty: Price,
    /// Conditions that must be met before the order activates (IB tag 6136+).
    pub conditions: Vec<OrderCondition>,
    /// Cancel order if conditions are no longer met (IB tag 6128). Default false.
    pub conditions_cancel_order: bool,
    /// Evaluate conditions outside regular trading hours (IB tag 6151). Default false.
    pub conditions_ignore_rth: bool,
    /// OCA cancellation semantics (IB tag 6209), 1..=4. 0 = not set, which
    /// emits the gateway default 3 (ReduceOnFillNonBlock). Only emitted when
    /// an OCA group is present. See ibx#215.
    pub oca_type: u8,
    /// Exercise or lapse the option this order names (tag 6809): 1 exercises,
    /// 2 lapses, 0 is an ordinary order. There is no message of its own for an
    /// exercise, so it is an order carrying the action.
    pub exercise_action: u8,
}

impl Default for OrderAttrs {
    /// Unset has to be a value no caller would state. A percent offset of
    /// zero and an exempt code of zero are both real instructions, so
    /// deriving these asserted a relative-order offset and a short-sale
    /// exemption on every order that never asked for either.
    fn default() -> Self {
        Self {
            soft_dollar_tier_name: Default::default(),
            soft_dollar_tier_val: Default::default(),
            algo_id: Default::default(),
            display_size: Default::default(),
            min_qty: Default::default(),
            hidden: Default::default(),
            outside_rth: Default::default(),
            good_after: Default::default(),
            good_till: Default::default(),
            good_till_date_ymd: Default::default(),
            oca_group: Default::default(),
            oca_group_str: Default::default(),
            parent_id: Default::default(),
            discretionary_amt: Default::default(),
            sweep_to_fill: Default::default(),
            all_or_none: Default::default(),
            volatility: Default::default(),
            seek_price_improvement: false,
            manual_order_time: String::new(),
            advanced_error_override: String::new(),
            active_start_time: String::new(),
            active_stop_time: String::new(),
            post_only: false,
            solicited: false,
            manual_order_indicator: 0,
            route_marketable_to_bbo: false,
            imbalance_only: false,
            allow_pre_open: false,
            ignore_open_auction: false,
            is_oms_container: false,
            ext_operator: String::new(),
            customer_account: String::new(),
            professional_customer: false,
            ref_futures_con_id: 0,
            mifid2_decision_maker: String::new(),
            mifid2_decision_algo: String::new(),
            mifid2_execution_trader: String::new(),
            mifid2_execution_algo: String::new(),
            mid_offset_at_whole: f64::MAX,
            mid_offset_at_half: f64::MAX,
            use_price_mgmt_algo: 0,
            duration: i32::MAX,
            min_compete_size: 0,
            compete_against_best_offset: f64::MAX,
            continuous_update: false,
            reference_price_type: 0,
            stock_range_lower: f64::MAX,
            stock_range_upper: f64::MAX,
            volatility_type: Default::default(),
            percent_offset: f64::MAX,
            not_held: Default::default(),
            order_ref: Default::default(),
            open_close: Default::default(),
            scale: Default::default(),
            delta_neutral: Default::default(),
            short_sale_slot: Default::default(),
            designated_location: Default::default(),
            exempt_code: -1,
            hedge_type: Default::default(),
            hedge_beta: Default::default(),
            hedge_ratio: Default::default(),
            rule80a: Default::default(),
            post_to_ats: Default::default(),
            deactivate: Default::default(),
            deactivate_on_disconnect: Default::default(),
            include_overnight: Default::default(),
            auto_cancel_parent: Default::default(),
            min_trade_qty: Default::default(),
            block_order: Default::default(),
            auto_cancel_date: Default::default(),
            clearing_account: Default::default(),
            clearing_intent: Default::default(),
            combo_legs: Default::default(),
            primary_exchange: Default::default(),
            delta_neutral_contract: Default::default(),
            trigger_method: Default::default(),
            cash_qty: Default::default(),
            conditions: Default::default(),
            conditions_cancel_order: Default::default(),
            conditions_ignore_rth: Default::default(),
            oca_type: Default::default(),
            exercise_action: Default::default(),
        }
    }
}

/// A condition that must be met before an order activates.
/// IB evaluates conditions server-side; order stays PreSubmitted until triggered.
#[derive(Debug, Clone)]
pub enum OrderCondition {
    /// Trigger when an instrument's price crosses a threshold.
    Price {
        con_id: i64,
        exchange: String,
        price: Price,
        is_more: bool,
        /// 0=default, 1=last, 2=bid/ask, 3=bid, 4=ask
        trigger_method: u8,
    },
    /// Trigger at a specific time.
    Time {
        /// Format: YYYYMMDD-HH:MM:SS
        time: String,
        is_more: bool,
    },
    /// Trigger based on margin cushion percentage.
    Margin {
        /// Percentage (e.g., 10 = 10%).
        percent: u32,
        is_more: bool,
    },
    /// Trigger when a trade executes on a specific instrument.
    Execution {
        symbol: String,
        exchange: String,
        sec_type: String,
    },
    /// Trigger when volume exceeds a threshold.
    Volume {
        con_id: i64,
        exchange: String,
        volume: i64,
        is_more: bool,
    },
    /// Trigger on percentage price change.
    PercentChange {
        con_id: i64,
        exchange: String,
        percent: f64,
        is_more: bool,
    },
}

/// Risk aversion level for Arrival Price and Close Price algos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskAversion {
    GetDone,
    Aggressive,
    Neutral,
    Passive,
}

impl RiskAversion {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GetDone => "Get_Done",
            Self::Aggressive => "Aggressive",
            Self::Neutral => "Neutral",
            Self::Passive => "Passive",
        }
    }
}

/// Parameters for IB algorithmic order strategies.
/// Each variant maps to a specific tag 847 algoStrategy with its required params.
#[derive(Debug, Clone)]
pub enum AlgoParams {
    /// VWAP: Volume-weighted average price.
    /// Tag 847=Vwap, 849=max_pct_vol.
    Vwap {
        /// Maximum participation rate (0.0-1.0). Sent as tag 849.
        max_pct_vol: f64,
        /// Don't take liquidity (0 or 1).
        no_take_liq: bool,
        /// Allow algo to continue past end time.
        allow_past_end_time: bool,
        /// Start time in UTC: "YYYYMMDD-HH:MM:SS".
        start_time: String,
        /// End time in UTC: "YYYYMMDD-HH:MM:SS".
        end_time: String,
    },
    /// TWAP: Time-weighted average price.
    /// Tag 847=Twap.
    Twap {
        allow_past_end_time: bool,
        start_time: String,
        end_time: String,
    },
    /// Arrival Price: Minimize arrival price impact.
    /// Tag 847=ArrivalPx, 849=max_pct_vol.
    ArrivalPx {
        max_pct_vol: f64,
        risk_aversion: RiskAversion,
        allow_past_end_time: bool,
        force_completion: bool,
        start_time: String,
        end_time: String,
    },
    /// Close Price: Target closing price.
    /// Tag 847=ClosePx, 849=max_pct_vol.
    ClosePx {
        max_pct_vol: f64,
        risk_aversion: RiskAversion,
        force_completion: bool,
        start_time: String,
    },
    /// Dark Ice: Hidden iceberg algo.
    /// Tag 847=DarkIce.
    DarkIce {
        allow_past_end_time: bool,
        display_size: u32,
        start_time: String,
        end_time: String,
    },
    /// Percentage of Volume: Participate at % of volume.
    /// Tag 847=PctVol.
    PctVol {
        /// Target participation rate (0.0-1.0). Sent as param pctVol.
        pct_vol: f64,
        no_take_liq: bool,
        start_time: String,
        end_time: String,
    },
}

/// The order-type-specific part of an extended order submission: which
/// order type and its price parameters. Used by `OrderRequest::SubmitEx`,
/// which pairs any of these with a TIF and an `OrderAttrs` block, so every
/// order type can carry extended attributes without a per-type `*Ex`
/// variant (ibx#224).
#[derive(Debug, Clone)]
pub enum OrderKind {
    Market,
    Limit { price: Price },
    Stop { stop_price: Price },
    StopLimit { price: Price, stop_price: Price },
    /// Trailing stop by absolute amount. `trail_stop_price` is the optional
    /// initial stop trigger (tag 6117); 0 = not set.
    TrailingStop { trail_amt: Price, trail_stop_price: Price },
    /// Trailing stop limit; `lmt_offset` is the limit-vs-trail offset (tag 6370).
    /// `trail_stop_price` is the optional initial stop trigger (tag 6117); 0 = not set.
    TrailingStopLimit { lmt_offset: Price, trail_amt: Price, trail_stop_price: Price },
    /// Trailing stop by percentage. Basis points: 100 = 1%.
    /// `trail_stop_price` is the optional initial stop trigger (tag 6117); 0 = not set.
    TrailPct { trail_pct: u32, trail_stop_price: Price },
    /// Pegged to a benchmark contract's price.
    ///
    /// `ref_exchange` is where the reference contract is quoted, by name. The
    /// field it travels in refuses a number, and the gateway will not accept
    /// the order without it or without the units field the encoder supplies.
    PegBench {
        price: Price,
        ref_con_id: u32,
        is_peg_decrease: bool,
        pegged_change_amount: Price,
        ref_change_amount: Price,
        starting_price: Price,
        /// The reference contract's price, which the venue requires alongside
        /// the contract itself.
        stock_ref_price: Price,
        ref_exchange: String,
    },
    Moc,
    Loc { price: Price },
    Mit { stop_price: Price },
    Lit { price: Price, stop_price: Price },
    Mtl,
    MktPrt,
    StpPrt { stop_price: Price },
    MidPrice { price_cap: Price },
    /// Snap-to orders carry an offset from the price they snap to, and the
    /// gateway refuses one that does not state it ("Message must contain field
    /// # 211"). Taken from `aux_price`, as the pegged types are.
    SnapMkt { offset: Price },
    SnapMid { offset: Price },
    SnapPri { offset: Price },
    /// Pegs to the market price, offset by `offset`, with `price_cap` as the
    /// worst price it may reach. The cap rides the limit-price tag, which the
    /// gateway wants on both peg types — a pegged order sent without one is
    /// refused for an invalid limit price. Zero states no cap.
    PegMkt { offset: Price, price_cap: Price },
    /// Pegs to the midpoint of the NBBO, capped the same way.
    PegMid { offset: Price, price_cap: Price },
    Rel { offset: Price },
    /// Stop that converts to another order type once `trigger_price` is hit.
    /// Tags: 6257=1, 6261=adjusted type, 6258=trigger, 6259=adjusted stop,
    /// 6262=adjusted limit, 6260/6269=trailing amount + unit.
    AdjustableStop {
        stop_price: Price,
        trigger_price: Price,
        adjusted_order_type: AdjustedOrderType,
        adjusted_stop_price: Price,
        /// Only used when adjusted_order_type is StopLimit or TrailLimit. 0 = not set.
        adjusted_stop_limit_price: Price,
        /// Trailing amount for a Trail/TrailLimit conversion (tag 6260). When the
        /// unit is amount it is a price offset (scaled); when percent it is the
        /// percent value scaled (1.00% = PRICE_SCALE). 0 = not set.
        adjusted_trailing_amount: Price,
        /// Unit of `adjusted_trailing_amount` on the wire (tag 6269): 0 = amount,
        /// 100 = percent. Other values are rejected by the gateway.
        adjustable_trailing_unit: i32,
    },
    /// Adaptive limit. Tags: 18=e (adaptive wrapper), 847=Adaptive,
    /// 5957/5958/5960 = the single adaptivePriority algo parameter.
    Adaptive { price: Price, priority: AdaptivePriority },
    /// Generic algo limit. Tags: 847=strategy, 5957 + 5958/5960 per parameter.
    Algo { price: Price, algo: AlgoParams },
    /// Margin preview of an order of type `ord_type` (the wire character, tag
    /// 40). Tag 6091=1; the order is tracked under `ORD_WHAT_IF` so the
    /// response is recognised, and never becomes a live order.
    WhatIf { price: Price, ord_type: u8 },
}

/// A scale order's ladder: how much to show, how far apart, and how the price
/// moves as it works.
///
/// Boxed on the attribute block so an order that is not a scale order does not
/// carry the room for one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScaleAttrs {
    /// Size of the first component (tag 6403).
    pub init_level_size: u32,
    /// Size of each component after the first (tag 6445).
    pub subs_level_size: u32,
    /// Price step between components (tag 6405).
    pub price_increment: Price,
    /// Offset at which a filled component takes profit (tag 6446).
    pub profit_offset: Price,
    /// How far the price moves per adjustment (tag 6527).
    pub price_adjust_value: Price,
    /// How often it adjusts, in seconds (tag 6526).
    pub price_adjust_interval: u32,
    /// Start the ladder again once it is exhausted (tag 6461).
    pub auto_reset: bool,
    /// Vary the component sizes (tag 6795).
    pub random_percent: bool,
    /// A position already held, which the ladder counts against rather than
    /// starting from nothing (tag 6485).
    pub init_position: i32,
    /// How much of the first component is already filled (tag 6486).
    pub init_fill_qty: i32,
}

/// The contract an order hedges against: which one, its delta, and its price.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeltaNeutralContractSpec {
    pub con_id: i64,
    pub delta: f64,
    pub price: f64,
}

/// One leg of a combination, as the order states it.
///
/// The wire takes the contract by id, a ratio, and a side as a flag rather than
/// a letter. The exchange is stated only where the leg has one of its own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComboLegSpec {
    /// The leg's contract (tag 6080).
    pub con_id: i64,
    /// How many of it per unit of the combination (tag 6081).
    pub ratio: u32,
    /// Buy or sell, as the flag the wire takes (tag 6082).
    pub is_sell: bool,
    /// Where this leg routes, when it is not the combination's own venue
    /// (tag 616).
    pub exchange: String,
    /// Whether the leg opens or closes (tag 654).
    pub open_close: u8,
    /// Short-sale slot for the leg (tag 6086), where its shares are located
    /// (tag 6216) and the exemption that applies (tag 1689).
    pub short_sale_slot: u8,
    pub designated_location: String,
    pub exempt_code: i32,
    /// What this leg is to be done at, where the caller priced the legs
    /// separately rather than pricing the combination (tag 6879).
    ///
    /// Held with the leg it belongs to. Kept in a list of its own, one price
    /// would go out against another leg the moment the legs were reordered.
    pub price: Option<Price>,
}

/// The hedge an order carries: what to trade against the position, and at what.
///
/// Dropping this leaves the position naked, which is why it is carried rather
/// than ignored. Boxed for the same reason as the ladder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeltaNeutralAttrs {
    /// Order type for the hedging leg (tag 6290).
    pub order_type: String,
    /// Its price, where the type needs one (tag 6291).
    pub aux_price: Price,
    /// The contract to hedge with (tag 6150).
    pub con_id: i64,
}

/// Order request sent via control channel, processed by engine.
///
/// The submitting variant is much larger than the cancelling ones, because it
/// carries the attribute block. Boxing it would even the variants out and pay
/// an allocation on every order placed to do it, which is the wrong trade on
/// the path an order takes.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum OrderRequest {
    /// Bracket order: parent entry + take-profit + stop-loss, linked via OCA.
    /// Generates 3 FIX messages: parent (35=D), TP child (35=D with 6107+583), SL child (35=D with 6107+583).
    SubmitBracket {
        parent_id: OrderId,
        tp_id: OrderId,
        sl_id: OrderId,
        instrument: InstrumentId,
        side: Side,
        qty: u32,
        entry_price: Price,
        take_profit: Price,
        stop_loss: Price,
    },
    /// Extended submission for any order type: `kind` selects the order type
    /// and its prices, paired with a TIF and the full `OrderAttrs` block.
    /// This is how non-LMT types carry parent_id/oca_group/outside_rth/tif
    /// (ibx#224).
    SubmitEx {
        order_id: OrderId,
        instrument: InstrumentId,
        side: Side,
        qty: u32,
        kind: OrderKind,
        tif: u8,
        attrs: OrderAttrs,
    },
    /// Limit order for opening auction (TIF=OPG).
    /// Algorithmic order: limit order with IB algo strategy overlay (VWAP, TWAP, etc.).
    /// Pegged to Benchmark: pegs to a benchmark instrument's price. OrdType PB.
    /// Companion tags: 6941=refConId, 6938=isPegDecrease, 6939=pegChangeAmt, 6942=refChangeAmt.
    /// Limit order for auction (TIF=AUC, tag 59=8). Participates in exchange opening/closing auction.
    /// Market-to-Limit for auction (TIF=AUC, tag 59=8). MTL + auction participation.
    /// What-If order: sends a limit order with tag 6091=1 for margin/commission preview.
    /// The order is NOT placed — response comes back as 35=8 with margin fields.
    /// Fractional shares limit order. Qty is fixed-point, `QTY_SCALE`, and
    /// goes out on tag 38 as a decimal string.
    SubmitLimitFractional {
        order_id: OrderId,
        instrument: InstrumentId,
        side: Side,
        qty: Qty, // QTY_SCALE fixed-point
        price: Price,
    },
    Cancel {
        order_id: OrderId,
    },
    CancelAll {
        instrument: InstrumentId,
    },
    /// Replace a working order.
    ///
    /// Carries what the replace message states rather than restating the
    /// tracked original, so a caller changing the order type, the time-in-force
    /// or the trigger has the change reach the gateway (ibx#349, ibx#372).
    /// A zero `tif` states none and leaves the resting value in force.
    Modify {
        new_order_id: OrderId,
        order_id: OrderId,
        price: Price,
        qty: u32,
        /// Outside-RTH flag from the order the caller resubmitted. The replace
        /// asserts tag 6433 from this rather than from the tracked record,
        /// which has no field for it (ibx#247).
        outside_rth: bool,
        /// Order type and time-in-force the replacement carries, as
        /// `Order::ord_type` and `Order::tif`. A replace that restated neither
        /// left the gateway to infer them, and it inferred a plain limit.
        ord_type: u8,
        tif: u8,
        /// New trigger price. Zero means the caller did not supply one, in
        /// which case a trigger-only order type takes `price` as its trigger
        /// and every other type keeps the trigger it already had.
        stop_price: Price,
    },
}

impl OrderRequest {
    /// Extract the order_id from any variant. Returns 0 for CancelAll (no order_id).
    pub fn order_id(&self) -> OrderId {
        match self {
            Self::Cancel { order_id } => *order_id,
            Self::CancelAll { .. } => 0,
            Self::Modify { order_id, .. } => *order_id,
            | Self::SubmitLimitFractional { order_id, .. }
            | Self::SubmitEx { order_id, .. } => *order_id,
            Self::SubmitBracket { parent_id, .. } => *parent_id,
        }
    }

    /// Extract the instrument from any submit variant. None for
    /// Cancel/Modify, which carry no instrument (the engine resolves it from
    /// the tracked order).
    pub fn instrument(&self) -> Option<InstrumentId> {
        match self {
            Self::Cancel { .. } | Self::Modify { .. } => None,
            Self::CancelAll { instrument }
            | Self::SubmitLimitFractional { instrument, .. }
            | Self::SubmitEx { instrument, .. }
            | Self::SubmitBracket { instrument, .. } => Some(*instrument),
        }
    }

    /// Snap every outbound price-like field to the instrument's tick grid
    /// (ibx#216). `tick` is the fixed-point tick from
    /// `MarketState::min_tick_scaled`; 0 (unknown — no market-data
    /// subscription seen yet) leaves prices unchanged. Percent-based fields
    /// (trailing percent) and non-price fields (quantities, cash amounts)
    /// are not touched.
    pub fn snap_prices(&mut self, tick: i64) {
        if tick <= 0 {
            return;
        }
        let s = |p: &mut Price| *p = snap_to_tick(*p, tick);
        match self {
            Self::Cancel { .. } | Self::CancelAll { .. } => {}
            Self::Modify { price, stop_price, .. } => { s(price); s(stop_price); }
            Self::SubmitLimitFractional { price, .. } => s(price),
            Self::SubmitBracket { entry_price, take_profit, stop_loss, .. } => {
                s(entry_price); s(take_profit); s(stop_loss);
            }
            Self::SubmitEx { kind, .. } => match kind {
                OrderKind::Market | OrderKind::Moc | OrderKind::Mtl | OrderKind::MktPrt
                | OrderKind::SnapMkt { .. } | OrderKind::SnapMid { .. }
                | OrderKind::SnapPri { .. } => {}
                OrderKind::TrailPct { trail_stop_price, .. } => s(trail_stop_price),
                OrderKind::PegBench {
                    price, pegged_change_amount, ref_change_amount, starting_price, ..
                } => { s(price); s(pegged_change_amount); s(ref_change_amount); s(starting_price); }
                OrderKind::Adaptive { price, .. }
                | OrderKind::Algo { price, .. }
                | OrderKind::WhatIf { price, .. } => s(price),
                OrderKind::AdjustableStop {
                    stop_price, trigger_price, adjusted_stop_price, adjusted_stop_limit_price,
                    adjusted_trailing_amount, adjustable_trailing_unit, ..
                } => {
                    s(stop_price); s(trigger_price); s(adjusted_stop_price); s(adjusted_stop_limit_price);
                    // Snap the trailing amount only when it is an absolute price
                    // offset; a percent (unit 100) is not a price and must not snap.
                    if *adjustable_trailing_unit == 0 { s(adjusted_trailing_amount); }
                }
                OrderKind::Limit { price } | OrderKind::Loc { price } => s(price),
                OrderKind::Stop { stop_price }
                | OrderKind::Mit { stop_price }
                | OrderKind::StpPrt { stop_price } => s(stop_price),
                OrderKind::StopLimit { price, stop_price }
                | OrderKind::Lit { price, stop_price } => { s(price); s(stop_price); }
                OrderKind::TrailingStop { trail_amt, trail_stop_price } => { s(trail_amt); s(trail_stop_price); }
                OrderKind::TrailingStopLimit { lmt_offset, trail_amt, trail_stop_price } => { s(lmt_offset); s(trail_amt); s(trail_stop_price); }
                OrderKind::MidPrice { price_cap } => s(price_cap),
                OrderKind::PegMkt { offset, .. } | OrderKind::PegMid { offset, .. }
                | OrderKind::Rel { offset } => s(offset),
            },
        }
    }
}

/// Pre-allocated buffer for pending order requests. Never allocates on the hot path.
/// Created once with capacity, then push/clear cycle each tick.
pub struct OrderBuffer {
    buf: Vec<OrderRequest>,
}

impl Default for OrderBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderBuffer {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(MAX_PENDING_ORDERS),
        }
    }

    pub fn push(&mut self, req: OrderRequest) {
        debug_assert!(self.buf.len() < MAX_PENDING_ORDERS, "order buffer overflow");
        self.buf.push(req);
    }

    /// Put requests back at the head, ahead of anything queued since.
    ///
    /// Used where a batch was taken and part of it turned out not to have been
    /// sent, so it waits for the transport rather than being reported.
    pub fn requeue_front(&mut self, reqs: Vec<OrderRequest>) {
        if reqs.is_empty() { return; }
        debug_assert!(self.buf.len() + reqs.len() <= MAX_PENDING_ORDERS, "order buffer overflow");
        self.buf.splice(0..0, reqs);
    }

    pub fn drain(&mut self) -> std::vec::Drain<'_, OrderRequest> {
        self.buf.drain(..)
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

/// Tick-by-tick data type for subscription requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TbtType {
    /// Last trade ticks (AllLast).
    Last,
    /// Bid/ask quote ticks (BidAsk).
    BidAsk,
}

/// A single tick-by-tick trade (AllLast) from 35=E.
#[derive(Debug, Clone)]
pub struct TbtTrade {
    pub instrument: InstrumentId,
    pub price: Price,
    pub size: i64,
    pub timestamp: u64,
    pub exchange: String,
    pub conditions: String,
    /// The venue may still revise this print.
    ///
    /// Stated by the venue and decoded off the wire, then thrown away and
    /// reported as false — so a caller was told a print was final when the
    /// venue had said it might not be.
    pub past_limit: bool,
    /// The print did not go to the tape.
    pub unreported: bool,
}

/// A single tick-by-tick bid/ask quote from 35=E.
#[derive(Debug, Clone, Copy)]
pub struct TbtQuote {
    pub instrument: InstrumentId,
    pub bid: Price,
    pub ask: Price,
    pub bid_size: i64,
    pub ask_size: i64,
    pub timestamp: u64,
    /// The bid is below the day's low, or the ask above its high — the venue's
    /// own words about whether this quote sits outside the day's range.
    pub bid_past_low: bool,
    pub ask_past_high: bool,
}

/// An IB news bulletin from auth server news bulletin message.
#[derive(Debug, Clone)]
pub struct NewsBulletin {
    pub msg_id: i32,
    /// 1=Regular, 2=Exchange unavailable, 3=Exchange available.
    pub msg_type: i32,
    pub message: String,
    pub exchange: String,
}

/// A market depth (L2 order book) update.
#[derive(Debug, Clone)]
pub struct DepthUpdate {
    pub req_id: u32,
    /// Book position (0-based).
    pub position: i32,
    /// Market maker ID (L2 only).
    pub market_maker: String,
    /// 0 = insert, 1 = update, 2 = delete.
    pub operation: i32,
    /// 0 = ask, 1 = bid.
    pub side: i32,
    pub price: f64,
    pub size: f64,
    pub is_smart_depth: bool,
}

/// Exchange metadata for market depth availability.
#[derive(Debug, Clone)]
pub struct DepthMktDataDescription {
    pub exchange: String,
    pub sec_type: String,
    pub listing_exch: String,
    pub service_data_type: String,
    pub agg_group: i32,
}

/// A component exchange in a SMART routing map.
/// The code a venue is known by on a quote.
///
/// Not this client's invention and not a single letter in every case: the
/// counterpart keeps this as a table of its own, and two of the codes written
/// here from memory were wrong. NASDAQ is `O`, not `Q`. ARCA is `Ar`, two
/// characters, not `P` — `P` belongs to PSE.
///
/// A venue the table does not name is left without a code rather than given the
/// first letter of its name, which would collide with a venue that has one.
pub fn exchange_letter(exchange: &str) -> &'static str {
    match exchange {
        "AMEX" => "A",
        "NYSE" => "N",
        "PHLX" => "X",
        "PSE" => "P",
        "ISE" => "I",
        "CBOE" => "C",
        "ARCA" => "Ar",
        "NASDAQ" => "O",
        _ => "",
    }
}


#[derive(Debug, Clone)]
pub struct SmartComponent {
    pub bit_number: i32,
    pub exchange: String,
    pub exchange_letter: String,
}

/// A news data provider.
#[derive(Debug, Clone)]
pub struct NewsProvider {
    pub code: String,
    pub name: String,
}

/// A soft dollar tier (commission sharing arrangement).
#[derive(Debug, Clone)]
pub struct SoftDollarTier {
    pub name: String,
    pub val: String,
    pub display_name: String,
}

/// A family code linking related accounts.
#[derive(Debug, Clone)]
pub struct FamilyCode {
    pub account_id: String,
    pub family_code_str: String,
}

/// A real-time news headline from 8=O|35=G tick type 0x1E90.
#[derive(Debug, Clone)]
pub struct TickNews {
    pub instrument: InstrumentId,
    pub provider_code: String,
    pub article_id: String,
    pub headline: String,
    pub timestamp: u64,
}

/// A historical tick (midpoint).
#[derive(Debug, Clone)]
pub struct HistoricalTickMidpoint {
    pub time: String,
    pub price: f64,
}

/// A historical tick (last trade).
#[derive(Debug, Clone)]
pub struct HistoricalTickLast {
    pub time: String,
    pub price: f64,
    pub size: i64,
    pub exchange: String,
    pub special_conditions: String,
}

/// A historical tick (bid/ask).
#[derive(Debug, Clone)]
pub struct HistoricalTickBidAsk {
    pub time: String,
    pub bid_price: f64,
    pub ask_price: f64,
    pub bid_size: i64,
    pub ask_size: i64,
}

/// Historical tick data (one of three types based on whatToShow).
#[derive(Debug, Clone)]
pub enum HistoricalTickData {
    Midpoint(Vec<HistoricalTickMidpoint>),
    Last(Vec<HistoricalTickLast>),
    BidAsk(Vec<HistoricalTickBidAsk>),
}

/// A real-time 5-second bar.
#[derive(Debug, Clone, Copy)]
pub struct RealTimeBar {
    pub timestamp: u32,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub wap: f64,
    pub count: i32,
}

/// A single trading session from a historical schedule response.
#[derive(Debug, Clone)]
pub struct ScheduleSession {
    pub ref_date: String,
    pub open_time: String,
    pub close_time: String,
}

/// Parsed historical schedule response from historical data connection.
#[derive(Debug, Clone)]
pub struct HistoricalScheduleResponse {
    pub query_id: String,
    pub timezone: String,
    pub start_date_time: String,
    pub end_date_time: String,
    pub sessions: Vec<ScheduleSession>,
}

/// A completed order record for req_completed_orders.
#[derive(Debug, Clone)]
pub struct CompletedOrder {
    pub order_id: OrderId,
    pub instrument: InstrumentId,
    pub status: OrderStatus,
    pub filled_qty: i64,
    pub timestamp_ns: u64,
}

/// Optional request-side filters for a by-symbol contract-details lookup.
/// Empty/zero fields are omitted from the request (ib-agent#171, ibx#229).
#[derive(Debug, Clone, Default)]
pub struct SecDefFilters {
    pub primary_exchange: String,
    pub local_symbol: String,
    pub last_trade_date_or_contract_month: String,
    pub strike: f64,
    pub right: String,
    pub multiplier: String,
    pub trading_class: String,
    /// Identifier lookup (e.g. ISIN): raw identifier and its type. When set, the
    /// lookup rides the identifier instead of the symbol (ib-agent#174).
    pub sec_id: String,
    pub sec_id_type: String,
}

/// Commands sent from the control plane to the hot loop via SPSC channel.
///
/// The submitting command is much larger than the rest, because it carries an
/// order's whole attribute block — which grew as the fields the venue reads
/// were filled in. Boxing it would even the variants out at the cost of an
/// allocation per order placed; the channel holds sixty-four of these, so the
/// size it saves is measured in tens of kilobytes and the cost is on the path
/// an order takes.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ControlCommand {
    /// Subscribe to market data for a contract.
    /// `exchange` and `sec_type` determine farm routing (empty = UsFarm default).
    /// `mode_9887` encodes per-request market-data mode via FIX field 9887:
    /// 0 = REALTIME (absent, default fan-out 264=442 BID_ASK + 264=443 LAST),
    /// 1 = DELAYED, 2 = FROZEN, 3 = DELAYED_FROZEN (single 264=1 TOP + 9887=N).
    Subscribe {
        con_id: i64, symbol: String, exchange: String, sec_type: String,
        /// Stated so a contract named by symbol resolves to one listing: the
        /// same symbol on the same venue exists in more than one currency.
        currency: String,
        last_trade_date: String, strike: f64, right: String, multiplier: String,
        mode_9887: i32,
        reply_tx: Option<std::sync::mpsc::SyncSender<Result<InstrumentId, String>>>,
    },
    /// Unsubscribe from market data for an instrument.
    Unsubscribe { instrument: InstrumentId },
    /// Subscribe to tick-by-tick data via historical data connection.
    SubscribeTbt { con_id: i64, symbol: String, sec_type: String, exchange: String, tbt_type: TbtType, reply_tx: Option<std::sync::mpsc::SyncSender<Result<InstrumentId, String>>> },
    /// Unsubscribe from tick-by-tick data.
    UnsubscribeTbt { instrument: InstrumentId },
    /// Subscribe to per-contract news ticks via CCP (264=292).
    SubscribeNews { con_id: i64, symbol: String, providers: String, reply_tx: Option<std::sync::mpsc::SyncSender<Result<InstrumentId, String>>> },
    /// Unsubscribe from per-contract news ticks.
    UnsubscribeNews { instrument: InstrumentId },
    /// Subscribe to whole-account P&L via CCP (6040=142).
    SubscribePnl { req_id: i64, account: String },
    /// Ask for, or replace, a partition of the advisor's own configuration.
    ///
    /// `command` says which of asking, replacing or removing is meant;
    /// `partition` names which part — its groups, its allocation profiles, its
    /// models. A replacement carries the configuration as its own document.
    AdvisorConfig { command: i32, partition: String, document: Option<String> },
    /// Cancel P&L subscription.
    CancelPnl { req_id: i64 },
    /// Update a strategy parameter.
    UpdateParam { key: String, value: String },
    /// Submit an order from external caller (bridge mode).
    Order(OrderRequest),
    /// Register an instrument from external caller (bridge mode).
    /// `identity` is what separates two contracts sharing a symbol: expiry,
    /// strike, right and multiplier, joined. Empty for a stock or a currency
    /// pair, which those four fields do not distinguish. An order names its
    /// contract by the instrument, so the instrument has to know this or the
    /// order goes out unable to say which strike or contract month it means.
    RegisterInstrument { con_id: i64, symbol: String, sec_type: String, exchange: String, identity: String, reply_tx: Option<std::sync::mpsc::SyncSender<Result<InstrumentId, String>>> },
    /// Request historical bar data via historical data connection.
    FetchHistorical {
        req_id: u32,
        con_id: i64,
        symbol: String,
        /// Security type and exchange from the caller's contract. Hardcoding
        /// these described a stock on SMART regardless of what was asked for,
        /// so anything venue-specific was rejected (ibx#305).
        sec_type: String,
        exchange: String,
        end_date_time: String,
        duration: String,
        bar_size: String,
        what_to_show: String,
        use_rth: bool,
        keep_up_to_date: bool,
    },
    /// Measure auth-connection round-trip time (ibx#158): sends a
    /// test request immediately; the sample lands in
    /// `SharedState::last_ccp_rtt` when the reply arrives.
    Ping,
    /// Cancel a historical data request.
    CancelHistorical { req_id: u32 },
    /// Request head timestamp via historical data connection.
    FetchHeadTimestamp {
        req_id: u32,
        con_id: i64,
        what_to_show: String,
        use_rth: bool,
    },
    /// Request contract details via auth connection.
    FetchContractDetails {
        req_id: u32,
        con_id: i64,
        symbol: String,
        sec_type: String,
        exchange: String,
        currency: String,
        filters: SecDefFilters,
    },
    /// Cancel a head timestamp request.
    CancelHeadTimestamp { req_id: u32 },
    /// Search for matching symbols via auth connection.
    FetchMatchingSymbols { req_id: u32, pattern: String },
    /// Ask what corporate-event types the calendar carries.
    FetchCalendarMetaData { req_id: u32 },
    /// Ask the calendar for events, under a filter or for one contract.
    FetchCalendarEvents { req_id: u32, query: Box<crate::control::calendar::CalendarQuery> },
    /// Request the option chain of an underlying via auth connection.
    FetchOptionParams {
        req_id: u32,
        symbol: String,
        fut_fop_exchange: String,
        underlying_sec_type: String,
        underlying_con_id: i64,
    },
    /// Request available exchanges for market depth.
    FetchMktDepthExchanges,
    /// Request scanner parameter XML via historical data connection.
    FetchScannerParams,
    /// Subscribe to a scanner scan via historical data connection.
    SubscribeScanner {
        req_id: u32,
        instrument: String,
        location_code: String,
        scan_code: String,
        max_items: u32,
        filters: Vec<(String, String)>,
    },
    /// Cancel a scanner subscription.
    CancelScanner { req_id: u32 },
    /// Request historical news via historical data connection.
    FetchHistoricalNews {
        req_id: u32,
        con_id: u32,
        provider_codes: String,
        start_time: String,
        end_time: String,
        max_results: u32,
    },
    /// Request a news article via historical data connection.
    FetchNewsArticle {
        req_id: u32,
        provider_code: String,
        article_id: String,
    },
    /// Request fundamental data via historical data connection.
    FetchFundamentalData {
        req_id: u32,
        con_id: u32,
        report_type: String,
    },
    /// Cancel fundamental data request.
    CancelFundamentalData { req_id: u32 },
    /// Request histogram data via historical data connection.
    FetchHistogramData {
        req_id: u32,
        con_id: u32,
        use_rth: bool,
        period: String,
    },
    /// Cancel histogram data request.
    CancelHistogramData { req_id: u32 },
    /// Request historical ticks via historical data connection.
    FetchHistoricalTicks {
        req_id: u32,
        con_id: i64,
        sec_type: String,
        exchange: String,
        start_date_time: String,
        end_date_time: String,
        number_of_ticks: u32,
        what_to_show: String,
        use_rth: bool,
    },
    /// Subscribe to real-time 5-second bars via historical data connection.
    SubscribeRealTimeBar {
        req_id: u32,
        con_id: i64,
        symbol: String,
        sec_type: String,
        exchange: String,
        what_to_show: String,
        use_rth: bool,
    },
    /// Cancel real-time bar subscription.
    CancelRealTimeBar { req_id: u32 },
    /// Request historical schedule via historical data connection.
    FetchHistoricalSchedule {
        req_id: u32,
        con_id: i64,
        sec_type: String,
        exchange: String,
        end_date_time: String,
        duration: String,
        use_rth: bool,
    },
    /// Subscribe to market depth (L2) for a contract.
    SubscribeDepth {
        req_id: u32,
        con_id: i64,
        exchange: String,
        sec_type: String,
        num_rows: i32,
        is_smart_depth: bool,
    },
    /// Unsubscribe from market depth.
    UnsubscribeDepth { req_id: u32 },
    /// Request news providers list (gateway-local).
    FetchNewsProviders { req_id: u32 },
    /// Request SMART routing components.
    FetchSmartComponents { req_id: u32, bbo_exchange: String },
    /// Request soft dollar tiers.
    FetchSoftDollarTiers { req_id: u32 },
    /// Request user info.
    FetchUserInfo { req_id: u32 },
    /// End the session with the venue. Sent before [`ControlCommand::Shutdown`]
    /// by a caller that is disconnecting. A caller that only stops the engine
    /// and keeps its connections does not send it, because a logout ends the
    /// session those connections belong to.
    Logout,
    /// Graceful shutdown.
    Shutdown,
}

/// Account-level state.
#[derive(Debug, Clone, Copy, Default)]
pub struct AccountState {
    pub net_liquidation: Price,
    pub buying_power: Price,
    pub margin_used: Price,
    pub unrealized_pnl: Price,
    pub realized_pnl: Price,
    pub total_cash_value: Price,
    pub settled_cash: Price,
    pub accrued_cash: Price,
    pub equity_with_loan: Price,
    pub gross_position_value: Price,
    pub init_margin_req: Price,
    pub maint_margin_req: Price,
    pub available_funds: Price,
    pub excess_liquidity: Price,
    pub cushion: Price,        // percentage * PRICE_SCALE (e.g. 0.45 = 45%)
    pub sma: Price,
    pub day_trades_remaining: i64,
    pub leverage: Price,       // ratio * PRICE_SCALE
    pub daily_pnl: Price,
}

/// Position with average cost, for P&L computation and reqPositions.
#[derive(Debug, Clone, Default)]
pub struct PositionInfo {
    pub con_id: i64,
    /// The holding exactly as the account states it. Fractional: a holding of
    /// half a share is a holding, and rounding it to a whole number reported
    /// it as flat.
    pub position: f64,
    pub avg_cost: Price,      // per-share avg cost * PRICE_SCALE
    pub symbol: String,
    pub sec_type: String,
    pub currency: String,
    pub multiplier: String,
    // Per-position marks from the account-updates snapshot (ib-agent#172).
    // Set only by the portfolio-value message, not the lean position feed.
    pub market_price: Price,     // per-share mark * PRICE_SCALE
    pub market_value: Price,     // position mark * PRICE_SCALE
    pub unrealized_pnl: Price,   // * PRICE_SCALE
    pub realized_pnl: Price,     // * PRICE_SCALE
}

/// Per-position midnight seed from 6040=143 P&L subscription.
/// Used for client-side daily P&L computation.
#[derive(Debug, Clone, Copy, Default)]
pub struct MidnightSeed {
    pub con_id: i64,
    /// Position held at midnight. `None` when the row arrived without a
    /// parseable quantity: the position exists but its overnight size is
    /// unknown, which is not the same as having opened it today (ibx#296).
    pub qty_midnight: Option<i64>,
    /// What the venue states the position was worth at midnight. `None` where
    /// the row did not state it, which is when the day's change has to be
    /// sized against a previous close the client finds for itself.
    pub cost_midnight: Option<f64>,
    /// Quantity traded since midnight, as the venue states it.
    pub qty_traded: Option<f64>,
    pub money_traded: f64,            // net cash from today's fills (signed)
    pub realized_pnl: f64,           // realized P&L since midnight
}

#[cfg(test)]
mod tests {

    /// The producer and the consumer have to agree on the units. This is the
    /// disagreement that shipped: the decode path stored the wire magnitude
    /// and every reader divided by `QTY_SCALE`, so one contract arrived as
    /// 0.0001. Pinning both halves together is what catches it — either side
    /// changing alone fails here.
    #[test]
    fn wire_quantity_survives_the_round_trip_through_qty_scale() {
        for wire in [0i64, 1, 2, 7, 500, 10_000, 1_000_000] {
            let stored = qty_from_wire(wire);
            let delivered = stored as f64 / QTY_SCALE as f64;
            assert_eq!(delivered, wire as f64, "wire quantity {wire} came back as {delivered}");
        }
    }

    #[test]
    fn qty_from_wire_clamps_instead_of_wrapping() {
        // Server-supplied magnitude; a wrapped quantity would read as a
        // plausible negative size rather than an obvious ceiling.
        assert_eq!(qty_from_wire(i64::MAX), i64::MAX);
        assert_eq!(qty_from_wire(i64::MIN), i64::MIN);
        // Not a fixed point of the identity function, so this fails if the
        // conversion is dropped as well as if it wraps.
        assert_eq!(qty_from_wire(i64::MAX / 2), i64::MAX);
    }

    use super::*;
    use std::mem;

    // --- Quote layout ---

    #[test]
    fn quote_alignment_is_64() {
        assert_eq!(mem::align_of::<Quote>(), 64);
    }

    #[test]
    fn quote_size_is_128() {
        // 11 × i64 (88) + 1 × u64 (8) = 96 bytes data, padded to 128 (2 cache lines)
        assert_eq!(mem::size_of::<Quote>(), 128);
    }

    #[test]
    fn quote_is_copy() {
        let q = Quote::default();
        let q2 = q; // Copy
        assert_eq!(q.bid, q2.bid);
    }

    // --- Price fixed-point ---

    #[test]
    fn price_150_25() {
        let p: Price = 15_025 * (PRICE_SCALE / 100);
        assert_eq!(p, 15_025_000_000);
    }

    #[test]
    fn price_to_float() {
        let p: Price = 15_025_000_000;
        let f = p as f64 / PRICE_SCALE as f64;
        assert!((f - 150.25).abs() < 1e-10);
    }

    #[test]
    fn price_negative() {
        let p: Price = -500 * PRICE_SCALE;
        assert_eq!(p, -50_000_000_000);
    }

    // --- Qty fixed-point ---

    #[test]
    fn qty_100_shares() {
        let q: Qty = 100 * QTY_SCALE;
        assert_eq!(q as f64 / QTY_SCALE as f64, 100.0);
    }

    #[test]
    fn qty_fractional() {
        // 0.5 shares (fractional shares)
        let q: Qty = QTY_SCALE / 2;
        assert_eq!(q as f64 / QTY_SCALE as f64, 0.5);
    }

    // --- OrderBuffer ---

    #[test]
    fn order_buffer_starts_empty() {
        let buf = OrderBuffer::new();
        assert!(buf.is_empty());
    }

    #[test]
    fn order_buffer_push_and_drain() {
        let mut buf = OrderBuffer::new();
        buf.push(OrderRequest::SubmitEx {
            order_id: 1, instrument: 0, side: Side::Buy, qty: 100,
            kind: OrderKind::Limit { price: 150 * PRICE_SCALE },
            tif: b'0', attrs: OrderAttrs::default(),
        });
        buf.push(OrderRequest::Cancel { order_id: 42 });
        assert!(!buf.is_empty());

        let drained: Vec<_> = buf.drain().collect();
        assert_eq!(drained.len(), 2);
        assert!(buf.is_empty());
    }

    #[test]
    fn order_buffer_no_realloc() {
        let mut buf = OrderBuffer::new();
        let cap_before = buf.buf.capacity();
        for i in 0..MAX_PENDING_ORDERS {
            buf.push(OrderRequest::Cancel { order_id: i as u64 });
        }
        // Capacity should not have grown (pre-allocated)
        assert_eq!(buf.buf.capacity(), cap_before);
    }

    #[test]
    fn order_buffer_drain_reusable() {
        let mut buf = OrderBuffer::new();
        buf.push(OrderRequest::SubmitEx {
            order_id: 1, instrument: 0, side: Side::Sell, qty: 50,
            kind: OrderKind::Market,
            tif: b'0', attrs: OrderAttrs::default(),
        });
        let _: Vec<_> = buf.drain().collect();
        assert!(buf.is_empty());

        // Can push again after drain
        buf.push(OrderRequest::CancelAll { instrument: 1 });
        assert!(!buf.is_empty());
    }

    // --- OrderRequest variants ---

    #[test]
    fn order_request_is_copy() {
        let req = OrderRequest::Modify {
            new_order_id: 2,
            order_id: 1,
            price: 100 * PRICE_SCALE,
            qty: 200,
            outside_rth: false,
            ord_type: 0,
            tif: 0,
            stop_price: 0,
        };
        let req2 = req.clone();
        match (req, req2) {
            (
                OrderRequest::Modify { order_id: a, .. },
                OrderRequest::Modify { order_id: b, .. },
            ) => assert_eq!(a, b),
            _ => panic!("should both be Modify"),
        }
    }

    // --- Quote field independence ---

    #[test]
    fn quote_default_all_zeros() {
        let q = Quote::default();
        assert_eq!(q.bid, 0);
        assert_eq!(q.ask, 0);
        assert_eq!(q.last, 0);
        assert_eq!(q.bid_size, 0);
        assert_eq!(q.ask_size, 0);
        assert_eq!(q.last_size, 0);
        assert_eq!(q.volume, 0);
        assert_eq!(q.open, 0);
        assert_eq!(q.high, 0);
        assert_eq!(q.low, 0);
        assert_eq!(q.close, 0);
        assert_eq!(q.timestamp_ns, 0);
    }

    #[test]
    fn quote_field_independence() {
        let mut q = Quote { bid: 100 * PRICE_SCALE, ..Default::default() };
        assert_eq!(q.ask, 0); // other fields untouched
        assert_eq!(q.last, 0);
        q.ask = 101 * PRICE_SCALE;
        assert_eq!(q.bid, 100 * PRICE_SCALE); // bid unchanged
    }

    #[test]
    fn quote_in_array_no_false_sharing() {
        // Two adjacent quotes should be on different cache lines
        let quotes = [Quote::default(); 4];
        let ptr0 = &quotes[0] as *const Quote as usize;
        let ptr1 = &quotes[1] as *const Quote as usize;
        // Each quote is 128 bytes (2 cache lines), so stride should be 128
        assert_eq!(ptr1 - ptr0, 128);
    }

    // --- Price edge cases ---

    #[test]
    fn price_zero() {
        let p: Price = 0;
        assert_eq!(p as f64 / PRICE_SCALE as f64, 0.0);
    }

    #[test]
    fn price_one_cent() {
        let p: Price = PRICE_SCALE / 100; // $0.01
        let f = p as f64 / PRICE_SCALE as f64;
        assert!((f - 0.01).abs() < 1e-10);
    }

    #[test]
    fn price_sub_penny() {
        // $0.0001 (minimum tick for some instruments)
        let p: Price = PRICE_SCALE / 10_000;
        assert_eq!(p, 10_000); // 10^4
        let f = p as f64 / PRICE_SCALE as f64;
        assert!((f - 0.0001).abs() < 1e-12);
    }

    #[test]
    fn price_large_value() {
        // $100,000.00 (like BRK.A)
        let p: Price = 100_000 * PRICE_SCALE;
        assert_eq!(p, 10_000_000_000_000);
        // Should be well within i64 range (max ~9.2 * 10^18)
        assert!(p < i64::MAX);
    }

    #[test]
    fn price_max_representable() {
        // Maximum price: i64::MAX / PRICE_SCALE = ~92,233,720,368
        let max_price = i64::MAX / PRICE_SCALE;
        let p: Price = max_price * PRICE_SCALE;
        // Should not overflow
        assert!(p > 0);
    }

    // --- Qty edge cases ---

    #[test]
    fn qty_zero() {
        let q: Qty = 0;
        assert_eq!(q, 0);
    }

    #[test]
    fn qty_negative() {
        let q: Qty = -100 * QTY_SCALE;
        assert_eq!(q as f64 / QTY_SCALE as f64, -100.0);
    }

    #[test]
    fn qty_smallest_representable() {
        let q: Qty = 1;
        let f = q as f64 / QTY_SCALE as f64;
        assert!((f - 1e-8).abs() < 1e-12, "the smallest size a venue counts in");
    }

    // --- OrderBuffer edge cases ---

    #[test]
    fn order_buffer_multiple_drain_cycles() {
        let mut buf = OrderBuffer::new();
        for cycle in 0..10 {
            for i in 0..5 {
                buf.push(OrderRequest::Cancel { order_id: (cycle * 5 + i) as u64 });
            }
            let drained: Vec<_> = buf.drain().collect();
            assert_eq!(drained.len(), 5);
            assert!(buf.is_empty());
        }
    }

    #[test]
    fn order_buffer_drain_empty() {
        let mut buf = OrderBuffer::new();
        let drained: Vec<_> = buf.drain().collect();
        assert!(drained.is_empty());
    }

    // --- All OrderRequest variants ---

    // ── ibx#216: snap-to-tick ──

    const TICK_CENT: i64 = PRICE_SCALE / 100; // 0.01

    #[test]
    fn snap_to_tick_rounds_to_nearest() {
        // 150.123 on a 0.01 grid -> 150.12
        assert_eq!(snap_to_tick(15_012_300_000, TICK_CENT), 15_012_000_000);
        // 150.126 -> 150.13
        assert_eq!(snap_to_tick(15_012_600_000, TICK_CENT), 15_013_000_000);
        // Exact multiples unchanged.
        assert_eq!(snap_to_tick(15_012_000_000, TICK_CENT), 15_012_000_000);
        // Tie (150.125) rounds away from zero -> 150.13
        assert_eq!(snap_to_tick(15_012_500_000, TICK_CENT), 15_013_000_000);
        // Negative price mirrors: -150.125 -> -150.13
        assert_eq!(snap_to_tick(-15_012_500_000, TICK_CENT), -15_013_000_000);
        // 0.05 grid: 10.02 -> 10.00, 10.03 -> 10.05
        let nickel = 5 * TICK_CENT;
        assert_eq!(snap_to_tick(1_002_000_000, nickel), 1_000_000_000);
        assert_eq!(snap_to_tick(1_003_000_000, nickel), 1_005_000_000);
        // Unknown tick: unchanged.
        assert_eq!(snap_to_tick(15_012_345_678, 0), 15_012_345_678);
        assert_eq!(snap_to_tick(15_012_345_678, -1), 15_012_345_678);
        // Zero price stays zero (MidPrice "no cap" sentinel).
        assert_eq!(snap_to_tick(0, TICK_CENT), 0);
    }

    #[test]
    fn snap_prices_limit_and_stop_fields() {
        let mut req = OrderRequest::SubmitEx {
            order_id: 1, instrument: 0, side: Side::Buy, qty: 1,
            kind: OrderKind::StopLimit { price: 15_012_345_678, stop_price: 15_099_999_999 },
            tif: b'0', attrs: OrderAttrs::default(),
        };
        req.snap_prices(TICK_CENT);
        match req {
            OrderRequest::SubmitEx { kind: OrderKind::StopLimit { price, stop_price }, .. } => {
                assert_eq!(price, 15_012_000_000);
                assert_eq!(stop_price, 15_100_000_000);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn snap_prices_submit_ex_kind() {
        let mut req = OrderRequest::SubmitEx {
            order_id: 1, instrument: 0, side: Side::Sell, qty: 1,
            kind: OrderKind::Stop { stop_price: 24_000_123_456 },
            tif: b'1', attrs: OrderAttrs::default(),
        };
        req.snap_prices(TICK_CENT);
        match req {
            OrderRequest::SubmitEx { kind: OrderKind::Stop { stop_price }, .. } => {
                assert_eq!(stop_price, 24_000_000_000);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn snap_prices_leaves_percent_trail_alone() {
        // trail_pct is basis points, not a price — must never be snapped.
        let mut req = OrderRequest::SubmitEx {
            order_id: 1, instrument: 0, side: Side::Sell, qty: 1,
            kind: OrderKind::TrailPct { trail_pct: 137, trail_stop_price: 0 },
            tif: b'0', attrs: OrderAttrs::default(),
        };
        req.snap_prices(TICK_CENT);
        match req {
            OrderRequest::SubmitEx { kind: OrderKind::TrailPct { trail_pct, .. }, .. } =>
                assert_eq!(trail_pct, 137),
            _ => unreachable!(),
        }
    }

    #[test]
    fn snap_prices_unknown_tick_is_noop() {
        let mut req = OrderRequest::SubmitEx {
            order_id: 1, instrument: 0, side: Side::Buy, qty: 1,
            kind: OrderKind::Limit { price: 15_012_345_678 },
            tif: b'0', attrs: OrderAttrs::default(),
        };
        req.snap_prices(0);
        match req {
            OrderRequest::SubmitEx { kind: OrderKind::Limit { price }, .. } =>
                assert_eq!(price, 15_012_345_678),
            _ => unreachable!(),
        }
    }

    #[test]
    fn instrument_accessor_covers_submits() {
        let req = OrderRequest::SubmitEx {
            order_id: 1, instrument: 7, side: Side::Buy, qty: 1,
            kind: OrderKind::Market, tif: b'0', attrs: OrderAttrs::default(),
        };
        assert_eq!(req.instrument(), Some(7));
        assert_eq!(OrderRequest::Cancel { order_id: 1 }.instrument(), None);
        assert_eq!(
            OrderRequest::Modify { new_order_id: 2, order_id: 1, price: 0, qty: 1, outside_rth: false, ord_type: 0, tif: 0, stop_price: 0 }.instrument(),
            None
        );
    }

    #[test]
    fn order_request_modify_fields() {
        let req = OrderRequest::Modify { new_order_id: 100, order_id: 99, price: 200 * PRICE_SCALE, qty: 10, outside_rth: false, ord_type: 0, tif: 0, stop_price: 0 };
        match req {
            OrderRequest::Modify { order_id, price, qty, .. } => {
                assert_eq!(order_id, 99);
                assert_eq!(price, 200 * PRICE_SCALE);
                assert_eq!(qty, 10);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn order_request_cancel_all_fields() {
        let req = OrderRequest::CancelAll { instrument: 7 };
        match req {
            OrderRequest::CancelAll { instrument } => assert_eq!(instrument, 7),
            _ => panic!("wrong variant"),
        }
    }

    // --- AccountState ---

    #[test]
    fn account_state_default() {
        let a = AccountState::default();
        assert_eq!(a.net_liquidation, 0);
        assert_eq!(a.buying_power, 0);
        assert_eq!(a.margin_used, 0);
        assert_eq!(a.unrealized_pnl, 0);
        assert_eq!(a.realized_pnl, 0);
    }

    #[test]
    fn account_state_copy() {
        let a = AccountState { net_liquidation: 100_000 * PRICE_SCALE, ..Default::default() };
        let b = a; // Copy
        assert_eq!(b.net_liquidation, 100_000 * PRICE_SCALE);
    }

    // --- ControlCommand ---

    #[test]
    fn control_command_subscribe() {
        let cmd = ControlCommand::Subscribe { con_id: 265598, symbol: "AAPL".into(), exchange: String::new(), sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None };
        match cmd {
            ControlCommand::Subscribe { con_id, .. } => assert_eq!(con_id, 265598),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn control_command_unsubscribe() {
        let cmd = ControlCommand::Unsubscribe { instrument: 3 };
        match cmd {
            ControlCommand::Unsubscribe { instrument } => assert_eq!(instrument, 3),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn control_command_update_param() {
        let cmd = ControlCommand::UpdateParam { key: "k".into(), value: "v".into() };
        match cmd {
            ControlCommand::UpdateParam { key, value } => {
                assert_eq!(key, "k");
                assert_eq!(value, "v");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn control_command_clone() {
        let cmd = ControlCommand::Subscribe { con_id: 42, symbol: "TEST".into(), exchange: String::new(), sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None };
        let cmd2 = cmd.clone();
        match cmd2 {
            ControlCommand::Subscribe { con_id, .. } => assert_eq!(con_id, 42),
            _ => panic!("wrong variant"),
        }
    }

    // --- Fill ---

    #[test]
    fn fill_is_copy() {
        let f = Fill {
            instrument: 0,
            order_id: 1,
            side: Side::Buy,
            price: 150 * PRICE_SCALE,
            qty: 100,
            remaining: 0,
            commission: 0,
            timestamp_ns: 123456789,
            cum_qty: 100, avg_price: 150 * PRICE_SCALE,
        };
        let f2 = f; // Copy
        assert_eq!(f.order_id, f2.order_id);
        assert_eq!(f.timestamp_ns, f2.timestamp_ns);
    }

    // --- Order ---

    #[test]
    fn order_is_copy() {
        let o = Order {
            order_id: 42,
            instrument: 0,
            side: Side::Sell,
            price: 200 * PRICE_SCALE,
            qty: 50,
            filled: 10,
            status: OrderStatus::PartiallyFilled,
            ord_type: b'2',
            tif: b'0',
            stop_price: 0,
        };
        let o2 = o; // Copy
        assert_eq!(o.order_id, o2.order_id);
        assert_eq!(o.filled, o2.filled);
    }

    // --- Side ---

    #[test]
    fn side_equality() {
        assert_eq!(Side::Buy, Side::Buy);
        assert_eq!(Side::Sell, Side::Sell);
        assert_ne!(Side::Buy, Side::Sell);
    }

    // --- OrderStatus ---

    #[test]
    fn order_status_equality() {
        assert_eq!(OrderStatus::Submitted, OrderStatus::Submitted);
        assert_ne!(OrderStatus::Filled, OrderStatus::Cancelled);
        assert_ne!(OrderStatus::PartiallyFilled, OrderStatus::Filled);
    }

    // --- WhatIfResponse ---

    #[test]
    fn what_if_response_is_copy() {
        let r = WhatIfResponse {
            order_id: 1,
            instrument: 0,
            init_margin_before: 136_401 * (PRICE_SCALE / 100),
            maint_margin_before: 113_167 * (PRICE_SCALE / 100),
            equity_with_loan_before: 75_425_514 * (PRICE_SCALE / 100),
            init_margin_after: 895_786 * (PRICE_SCALE / 100),
            maint_margin_after: 814_351 * (PRICE_SCALE / 100),
            equity_with_loan_after: 75_425_514 * (PRICE_SCALE / 100),
            commission: PRICE_SCALE,
            min_commission: 0,
            max_commission: 0,
            commission_currency: String::new(),
            warning_text: String::new(),
        };
        // The reply carries the venue's own words now, so it is cloned rather
        // than copied.
        let r2 = r.clone();
        assert_eq!(r.init_margin_after, r2.init_margin_after);
        assert_eq!(r.commission, r2.commission);
        // The change is the difference, which the venue leaves to be taken.
        assert_eq!(r.init_margin_change(), r.init_margin_after - r.init_margin_before);
    }

    // --- AdjustedOrderType ---

    #[test]
    fn adjusted_order_type_fix_codes() {
        assert_eq!(AdjustedOrderType::Stop.fix_code(), "3");
        assert_eq!(AdjustedOrderType::StopLimit.fix_code(), "4");
        assert_eq!(AdjustedOrderType::Trail.fix_code(), "7");
        assert_eq!(AdjustedOrderType::TrailLimit.fix_code(), "8");
    }

    // --- OrderAttrs cash_qty ---

    #[test]
    fn order_attrs_cash_qty_default_zero() {
        let attrs = OrderAttrs::default();
        assert_eq!(attrs.cash_qty, 0);
    }
}

#[cfg(test)]
mod counted_size_tests {
    use super::{qty_from_counted, qty_from_wire, QTY_SCALE};

    /// A share is counted in whole ones, and reads the way it always did.
    #[test]
    fn a_share_is_counted_in_whole_ones() {
        assert_eq!(qty_from_counted(300, 1.0), qty_from_wire(300));
        assert_eq!(qty_from_counted(300, 1.0), 300 * QTY_SCALE);
    }

    /// An instrument the venue stated no increment for is counted in whole
    /// ones, which is what stating none means.
    #[test]
    fn no_stated_increment_is_whole_ones() {
        assert_eq!(qty_from_counted(300, 0.0), qty_from_wire(300));
    }

    /// A crypto is counted in hundred-millionths. Taken as whole ones, a
    /// hundredth of a coin reads as a million of them.
    #[test]
    fn a_crypto_is_counted_in_hundred_millionths() {
        let hundredth_of_a_coin = 1_000_000;
        let scaled = qty_from_counted(hundredth_of_a_coin, 1e-8);
        assert_eq!(scaled, (0.01 * QTY_SCALE as f64) as i64);
        assert_ne!(scaled, qty_from_wire(hundredth_of_a_coin));
    }
}

#[cfg(test)]
mod quantity_scale_tests {
    use super::{qty_from_counted, Qty, QTY_SCALE};

    /// The smallest size a venue counts in survives being held. At a
    /// ten-thousandth it did not: everything finer rounded to nothing, and a
    /// quote for a thousandth of a coin came back as no quote at all.
    #[test]
    fn the_smallest_counted_size_survives() {
        let one_count = qty_from_counted(1, 1e-8);
        assert!(one_count > 0, "a hundred-millionth rounded away");
        assert_eq!(one_count as f64 / QTY_SCALE as f64, 1e-8);
    }

    /// A day's volume in the busiest listing still fits.
    #[test]
    fn a_whole_market_day_still_fits() {
        let shares = 5_000_000_000i64;
        let held = qty_from_counted(shares, 1.0);
        assert_eq!(held / QTY_SCALE, shares);
        assert!(held < Qty::MAX / 2, "a day's volume is nowhere near the ceiling");
    }
}
