//! An order as this client holds it.
//!
//! From what a caller asked for through to what goes on the wire: the kind of
//! order, everything optional stated on it, the conditions it waits for, and
//! the buffer a submission is queued in.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Where an order stands, as this engine tracks it.
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
    /// Everything asked for has filled.
    Filled,
    /// Some has.
    PartiallyFilled,
    /// Withdrawn.
    Cancelled,
    /// Refused by the venue.
    Rejected,
    /// Server reports order inactive (FIX 39=I).
    Inactive,
    /// Order state is unknown due to an auth connection disconnect.
    /// Will be reconciled when reconnection completes (mass status request).
    Uncertain,
}

impl OrderStatus {
    /// Lifecycle progress rank for the monotonic status guard.
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

/// Order status change notification.
#[derive(Debug, Clone, Copy)]
pub struct OrderUpdate {
    /// The order this is about.
    pub order_id: OrderId,
    /// The contract it is on.
    pub instrument: InstrumentId,
    /// Where it stands now.
    pub status: OrderStatus,
    /// How much has filled.
    pub filled_qty: f64,
    /// How much has not.
    pub remaining_qty: f64,
    /// What the order has paid on average so far, as the report states it.
    /// Zero when nothing has filled.
    pub avg_price: Price,
    /// The venue's own id for it, stable across sessions.
    pub perm_id: i64,
    /// The order it is a child of, where the venue states one.
    pub parent_id: i64,
    /// When the venue stated this.
    pub timestamp_ns: u64,
}

/// Cancel/modify reject notification (reject message).
#[derive(Debug, Clone, Copy)]
pub struct CancelReject {
    /// The order that was not cancelled.
    pub order_id: OrderId,
    /// The contract it is on.
    pub instrument: InstrumentId,
    /// 1 = cancel rejected, 2 = modify rejected (FIX tag 434 CxlRejResponseTo).
    pub reject_type: u8,
    /// Numeric reason code (FIX tag 102 CxlRejReason). 0=TooLate, 1=UnknownOrder, etc.
    pub reason_code: i32,
    /// When the venue said so.
    pub timestamp_ns: u64,
}

/// Multi-char OrdType discriminants: values below 32, so they cannot collide
/// with the single-char ASCII types.
/// Used in `Order.ord_type` for order types whose FIX tag 40 value is more than one
/// character.
pub const ORD_STP_PRT: u8 = 1;   // FIX "SP"  — Stop with Protection
/// FIX "MIDPX" — Mid-Price
pub const ORD_MIDPX: u8 = 2;
/// FIX "SMKT" — Snap to Market
pub const ORD_SNAP_MKT: u8 = 3;
/// FIX "SMID" — Snap to Midpoint
pub const ORD_SNAP_MID: u8 = 4;
/// FIX "SREL" — Snap to Primary
pub const ORD_SNAP_PRI: u8 = 5;
/// FIX "P" + ExecInst "P" — Pegged to Market
pub const ORD_PEG_MKT: u8 = 6;
/// FIX "P" + ExecInst "M" — Pegged to Midpoint
pub const ORD_PEG_MID: u8 = 7;
/// FIX "PB" — Pegged to Benchmark
pub const ORD_PEG_BENCH: u8 = 8;
/// A time-in-force this client does not know. A recovery record with no tag 59
/// states none, and the order was not placed by this session, so there is
/// nothing to recover it from. Distinct from every real code, so it reports as
/// unstated rather than as an ordinary value, and a replace omits tag 59 rather
/// than restating a guess as an instruction.
pub const TIF_UNSTATED: u8 = 0;

/// Not a real OrdType — marker for what-if orders
pub const ORD_WHAT_IF: u8 = 9;

/// Convert an `ord_type` discriminant to the FIX tag 40 string.
/// Single-char types (ASCII >= 32) are stored as-is; multi-char types use constants
/// above.
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
    /// The preview this answers.
    pub order_id: OrderId,
    /// The contract it was previewed on.
    pub instrument: InstrumentId,
    /// What initial margin the account carried before it.
    pub init_margin_before: Price,
    /// What maintenance margin.
    pub maint_margin_before: Price,
    /// What equity with loan value.
    pub equity_with_loan_before: Price,
    /// What it would carry with the order on.
    pub init_margin_after: Price,
    /// And maintenance margin.
    pub maint_margin_after: Price,
    /// And equity with loan value.
    pub equity_with_loan_after: Price,
    /// What the order would cost.
    pub commission: Price,
    /// Where a commission is given as a range rather than a number, and the
    /// money it is quoted in. A preview that states the margin and not the cost
    /// is half a preview.
    pub min_commission: Price,
    /// The most.
    pub max_commission: Price,
    /// What those figures are in.
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
    /// What the order would change maintenance margin by.
    pub fn maint_margin_change(&self) -> Price {
        self.maint_margin_after - self.maint_margin_before
    }
    /// What it would change equity with loan value by.
    pub fn equity_with_loan_change(&self) -> Price {
        self.equity_with_loan_after - self.equity_with_loan_before
    }
}

/// Adjusted order type for adjustable stops (FIX tag 6261).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustedOrderType {
    /// It becomes a stop.
    Stop,       // 3
    /// It becomes a stop limit.
    StopLimit,  // 4
    /// It becomes a trailing stop.
    Trail,      // 7
    /// It becomes a trailing stop limit.
    TrailLimit, // 8
}

impl AdjustedOrderType {
    /// The code the wire carries this as.
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
    /// The caller's number for the order.
    pub order_id: OrderId,
    /// The engine's own slot for the contract.
    pub instrument: InstrumentId,
    /// Whether it buys or sells.
    pub side: Side,
    /// The price, scaled by `PRICE_SCALE`.
    pub price: Price,
    /// How much, scaled by `QTY_SCALE`.
    pub qty: u32,
    /// How much has filled.
    pub filled: u32,
    /// Where it stands.
    pub status: OrderStatus,
    /// FIX tag 40 OrdType: b'1'=MKT, b'2'=LMT, b'3'=STP, b'4'=STPLMT, b'P'=TRAIL, etc.
    /// For multi-char OrdTypes (MIDPX, SP, SMKT, etc.), uses ORD_* constants (values <
    /// 32).
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
    /// Work it slowly.
    Patient,
    /// Work it at the venue's usual pace.
    Normal,
    /// Work it quickly.
    Urgent,
}

impl AdaptivePriority {
    /// The name the venue knows this by.
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
    /// What kind of order this is, and the prices that kind needs.
    pub kind: OrderKind,
    /// Everything else the caller set on it.
    pub attrs: OrderAttrs,
}

#[derive(Debug, Clone)]
/// Everything a caller set on an order beyond its kind, side and size.
pub struct OrderAttrs {
    /// Show on book as this many shares (tag 111). 0 = not set (show full qty).
    pub display_size: u32,
    /// Minimum fill quantity (FIX tag 110). 0 = not set.
    pub min_qty: u32,
    /// Hidden order — not displayed on book (IB tag 6135).
    pub hidden: bool,
    /// Allow trading outside regular hours (IB tag 6433).
    pub outside_rth: bool,
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
    /// OCA group as a string (FIX tag 583). Used by Python compat for user-specified
    /// OCA names.
    /// When non-empty, takes precedence over numeric `oca_group`.
    pub oca_group_str: String,
    /// Parent order ID (IB tag 6107). 0 = no parent. Links child orders to parent in
    /// brackets.
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
    /// When a conditional order stops being watched.
    pub active_stop_time: String,
    /// Never take liquidity, on tag 6605.
    pub post_only: bool,
    /// Whether the client asked for this order or the account holder did, on
    /// tags 6488 and 1028 — which is a regulatory statement, not a preference.
    pub solicited: bool,
    /// Whether a person entered it rather than a program.
    pub manual_order_indicator: i32,
    /// Route to the best bid or offer where the order is marketable, on tag 8265.
    pub route_marketable_to_bbo: bool,
    /// Take part in an auction only to correct an imbalance, on tag 6737.
    pub imbalance_only: bool,
    /// Take part in the opening auction, or stand out of it, on tags 6524 and 6562.
    pub allow_pre_open: bool,
    /// Whether it stays out of the opening auction.
    pub ignore_open_auction: bool,
    /// One of several orders the venue treats as a unit, on tag 6406.
    pub is_oms_container: bool,
    /// Who is operating the account and on whose behalf, on tags 8089, 6207 and 6636.
    pub ext_operator: String,
    /// The end customer it is placed for.
    pub customer_account: String,
    /// Whether that customer is a professional.
    pub professional_customer: bool,
    /// The future a spread prices against, on tag 6564.
    pub ref_futures_con_id: i32,
    /// Who decided the trade and who executed it, for European transaction
    /// reporting, on tags 8237, 8243, 8254 and 8255.
    pub mifid2_decision_maker: String,
    /// Under MiFID II, which algorithm decided to trade.
    pub mifid2_decision_algo: String,
    /// Which person executed it.
    pub mifid2_execution_trader: String,
    /// Which algorithm did.
    pub mifid2_execution_algo: String,
    /// A midpoint peg's offset stated as a whole-tick part and a half-tick
    /// part, on tags 8403 and 8404. Both set is the other form of the peg.
    pub mid_offset_at_whole: f64,
    /// How far a midpoint order sits from the midpoint at half the
    /// spread.
    pub mid_offset_at_half: f64,
    /// Whether to let the venue manage the price of the order, on tag 8339.
    /// Zero is the caller saying nothing.
    pub use_price_mgmt_algo: i32,
    /// How long the order runs for, where its kind takes a duration, on tag 8402.
    pub duration: i32,
    /// The smallest size worth competing for, on tag 8411, and how far past the
    /// best price to compete, on tag 8412.
    pub min_compete_size: i32,
    /// How far a pegged-to-best order may improve on the
    /// best price.
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
    /// The highest underlying price a volatility order stays active
    /// through.
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
    /// Where a short leg's borrow is located.
    pub designated_location: String,
    /// -1 unless the leg is exempt from the short-sale price test.
    pub exempt_code: i32,
    /// How this order hedges (6665) and the parameter that goes with it: a
    /// beta on 6703, a pair ratio on 6666. Delta and FX hedges take neither.
    pub hedge_type: u8,
    /// The beta a beta hedge is struck at.
    pub hedge_beta: f64,
    /// The ratio a pair hedge is struck at.
    pub hedge_ratio: f64,
    /// The soft-dollar arrangement this order's commission goes to: which
    /// tier (tag 6519) and what it is worth (tag 6520). Taken from a caller
    /// and dropped, the commission went wherever the account's default sends
    /// it, which is not what the caller asked for.
    pub soft_dollar_tier_name: String,
    /// What the soft dollar tier is worth.
    pub soft_dollar_tier_val: String,
    /// The caller's own name for the algo running this order (tag 8016),
    /// which comes back on every report about it.
    pub algo_id: String,
    /// Who settles this order, where that is not the account's own (tag 6282).
    pub settling_firm: String,
    /// Whether discretion runs up to the limit price (tag 8165).
    pub discretionary_up_to_limit: bool,
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
    /// How the trade clears: `IB`, `Away`, `PTA`.
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
    /// Cash quantity — order by dollar amount instead of shares (IB tag 5920). 0 = not
    /// set.
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
    /// an OCA group is present.
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
            settling_firm: Default::default(),
            discretionary_up_to_limit: Default::default(),
            display_size: Default::default(),
            min_qty: Default::default(),
            hidden: Default::default(),
            outside_rth: Default::default(),
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
        /// The venue's id for the contract.
        con_id: i64,
        /// Where the request is directed, or the contract is listed.
        exchange: String,
        /// The price, scaled by `PRICE_SCALE`.
        price: Price,
        /// Whether the condition is met above the value rather than below it.
        is_more: bool,
        /// 0=default, 1=last, 2=bid/ask, 3=bid, 4=ask
        trigger_method: u8,
    },
    /// Trigger at a specific time.
    Time {
        /// Format: YYYYMMDD-HH:MM:SS
        time: String,
        /// Whether the condition is met above the value rather than below it.
        is_more: bool,
    },
    /// Trigger based on margin cushion percentage.
    Margin {
        /// Percentage (e.g., 10 = 10%).
        percent: u32,
        /// Whether the condition is met above the value rather than below it.
        is_more: bool,
    },
    /// Trigger when a trade executes on a specific instrument.
    Execution {
        /// The contract's ticker, for a request that names one by description.
        symbol: String,
        /// Where the request is directed, or the contract is listed.
        exchange: String,
        /// What kind of contract: `STK`, `OPT`, `FUT`, `CASH`, `IND`, `CRYPTO`.
        sec_type: String,
    },
    /// Trigger when volume exceeds a threshold.
    Volume {
        /// The venue's id for the contract.
        con_id: i64,
        /// Where the request is directed, or the contract is listed.
        exchange: String,
        /// The volume the condition is measured against.
        volume: i64,
        /// Whether the condition is met above the value rather than below it.
        is_more: bool,
    },
    /// Trigger on percentage price change.
    PercentChange {
        /// The venue's id for the contract.
        con_id: i64,
        /// Where the request is directed, or the contract is listed.
        exchange: String,
        /// The percentage change the condition is measured against.
        percent: f64,
        /// Whether the condition is met above the value rather than below it.
        is_more: bool,
    },
}

/// Risk aversion level for Arrival Price and Close Price algos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskAversion {
    /// Finish, whatever it costs.
    GetDone,
    /// Lean towards finishing.
    Aggressive,
    /// Neither.
    Neutral,
    /// Lean towards the price.
    Passive,
}

impl RiskAversion {
    /// The name the venue knows this by.
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
    /// An algorithm this client does not model, carried through as the caller
    /// wrote it.
    ///
    /// The venue states which algorithms an account may use — thirteen keys on
    /// an ordinary session — and refuses an order naming one it does not
    /// offer. A list of the ones this client happens to parse is a narrower
    /// answer than the venue's, and refusing on it stops a caller using an
    /// algorithm the venue would have accepted.
    ///
    /// The reference client does not interpret these either: it forwards the
    /// names and values it was handed.
    Named {
        /// The strategy, as the caller named it.
        strategy: String,
        /// Its parameters, flattened to name then value, in the order given.
        params: Vec<String>,
    },
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
        /// Whether it may keep working past that.
        allow_past_end_time: bool,
        /// When the algorithm should begin.
        start_time: String,
        /// When it should stop.
        end_time: String,
    },
    /// Arrival Price: Minimize arrival price impact.
    /// Tag 847=ArrivalPx, 849=max_pct_vol.
    ArrivalPx {
        /// The most of the market's volume the algorithm may take.
        max_pct_vol: f64,
        /// How hard it works against the price to finish.
        risk_aversion: RiskAversion,
        /// Whether it may keep working past that.
        allow_past_end_time: bool,
        /// Whether it must finish within its window.
        force_completion: bool,
        /// When the algorithm should begin.
        start_time: String,
        /// When it should stop.
        end_time: String,
    },
    /// Close Price: Target closing price.
    /// Tag 847=ClosePx, 849=max_pct_vol.
    ClosePx {
        /// The most of the market's volume the algorithm may take.
        max_pct_vol: f64,
        /// How hard it works against the price to finish.
        risk_aversion: RiskAversion,
        /// Whether it must finish within its window.
        force_completion: bool,
        /// When the algorithm should begin.
        start_time: String,
    },
    /// Dark Ice: Hidden iceberg algo.
    /// Tag 847=DarkIce.
    DarkIce {
        /// Whether it may keep working past that.
        allow_past_end_time: bool,
        /// How much of an iceberg is shown at once.
        display_size: u32,
        /// When the algorithm should begin.
        start_time: String,
        /// When it should stop.
        end_time: String,
    },
    /// Percentage of Volume: Participate at % of volume.
    /// Tag 847=PctVol.
    PctVol {
        /// Target participation rate (0.0-1.0). Sent as param pctVol.
        pct_vol: f64,
        /// Whether it may only add liquidity.
        no_take_liq: bool,
        /// When the algorithm should begin.
        start_time: String,
        /// When it should stop.
        end_time: String,
    },
}

/// The order-type-specific part of an extended order submission: which
/// order type and its price parameters. Used by `OrderRequest::SubmitEx`,
/// which pairs any of these with a TIF and an `OrderAttrs` block, so every
/// order type can carry extended attributes without a per-type `*Ex`
/// variant.
#[derive(Debug, Clone)]
pub enum OrderKind {
    /// Fill at whatever the market is.
    Market,
    /// Fill at this price or better.
    Limit {
        /// Fill at this price or better, scaled by `PRICE_SCALE`.
        price: Price,
    },
    /// Become a market order once the trigger is reached.
    Stop {
        /// Where it triggers.
        stop_price: Price,
    },
    /// Become a limit order once the trigger is reached.
    StopLimit {
        /// Fill at this price or better, scaled by `PRICE_SCALE`.
        price: Price,
        /// Where it triggers.
        stop_price: Price,
    },
    /// Trailing stop by absolute amount. `trail_stop_price` is the optional
    /// initial stop trigger (tag 6117); 0 = not set.
    TrailingStop {
        /// How far it follows, scaled by `PRICE_SCALE`.
        trail_amt: Price,
        /// Where it starts before it has followed anything.
        trail_stop_price: Price,
    },
    /// Trailing stop limit; `lmt_offset` is the limit-vs-trail offset (tag 6370).
    /// `trail_stop_price` is the optional initial stop trigger (tag 6117); 0 = not set.
    TrailingStopLimit {
        /// How far the limit sits from the trigger.
        lmt_offset: Price,
        /// How far it follows, scaled by `PRICE_SCALE`.
        trail_amt: Price,
        /// Where it starts before it has followed anything.
        trail_stop_price: Price,
    },
    /// Trailing stop by percentage. Basis points: 100 = 1%.
    /// `trail_stop_price` is the optional initial stop trigger (tag 6117); 0 = not set.
    TrailPct {
        /// How far it follows, as a percentage.
        trail_pct: u32,
        /// Where it starts before it has followed anything.
        trail_stop_price: Price,
    },
    /// Pegged to a benchmark contract's price.
    ///
    /// `ref_exchange` is where the reference contract is quoted, by name. The
    /// field it travels in refuses a number, and the gateway will not accept
    /// the order without it or without the units field the encoder supplies.
    PegBench {
        /// The price, scaled by `PRICE_SCALE`.
        price: Price,
        /// The contract the order is priced against.
        ref_con_id: u32,
        /// Whether the reference moving down moves the order down too.
        is_peg_decrease: bool,
        /// How far the order moves when the reference does.
        pegged_change_amount: Price,
        /// How far the reference must move first.
        ref_change_amount: Price,
        /// Where the order starts before any of that applies.
        starting_price: Price,
        /// The reference contract's price, which the venue requires alongside
        /// the contract itself.
        stock_ref_price: Price,
        /// Which venue's price of it to use.
        ref_exchange: String,
    },
    /// Fill at the closing auction.
    Moc,
    /// Fill at the closing auction, at this price or better.
    Loc {
        /// Fill at this price or better, scaled by `PRICE_SCALE`.
        price: Price,
    },
    /// Become a market order once the market touches the trigger.
    Mit {
        /// Where it triggers.
        stop_price: Price,
    },
    /// Become a limit order once the market touches the trigger.
    Lit {
        /// Fill at this price or better, scaled by `PRICE_SCALE`.
        price: Price,
        /// Where it triggers.
        stop_price: Price,
    },
    /// A market order that becomes a limit if it cannot fill at the touch.
    Mtl,
    /// A market order with the venue's own protection against a bad print.
    MktPrt,
    /// A stop with that same protection.
    StpPrt {
        /// Where it triggers.
        stop_price: Price,
    },
    /// Fill at the midpoint, no worse than this cap.
    MidPrice {
        /// The furthest it will follow.
        price_cap: Price,
    },
    /// Snap-to orders carry an offset from the price they snap to, and the
    /// gateway refuses one that does not state it ("Message must contain field
    /// # 211"). Taken from `aux_price`, as the pegged types are.
    SnapMkt {
        /// How far from the reference it sits.
        offset: Price,
    },
    /// The same, measured from the midpoint.
    SnapMid {
        /// How far from the reference it sits.
        offset: Price,
    },
    /// The same, measured from the primary venue's price.
    SnapPri {
        /// How far from the reference it sits.
        offset: Price,
    },
    /// Pegs to the market price, offset by `offset`, with `price_cap` as the
    /// worst price it may reach. The cap rides the limit-price tag, which the
    /// gateway wants on both peg types — a pegged order sent without one is
    /// refused for an invalid limit price. Zero states no cap.
    PegMkt {
        /// How far from the reference it sits.
        offset: Price,
        /// The furthest it will follow.
        price_cap: Price,
    },
    /// Pegs to the midpoint of the NBBO, capped the same way.
    PegMid {
        /// How far from the reference it sits.
        offset: Price,
        /// The furthest it will follow.
        price_cap: Price,
    },
    /// Sit at the best bid or offer, improved by this much.
    Rel {
        /// How far from the reference it sits.
        offset: Price,
    },
    /// Stop that converts to another order type once `trigger_price` is hit.
    /// Tags: 6257=1, 6261=adjusted type, 6258=trigger, 6259=adjusted stop,
    /// 6262=adjusted limit, 6260/6269=trailing amount + unit.
    AdjustableStop {
        /// Where a stop triggers.
        stop_price: Price,
        /// The price at which the order becomes the adjusted one.
        trigger_price: Price,
        /// What it becomes then.
        adjusted_order_type: AdjustedOrderType,
        /// The stop it takes on with it.
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
    Adaptive {
        /// Fill at this price or better, scaled by `PRICE_SCALE`.
        price: Price,
        /// How hard the venue's adaptive algorithm works it.
        priority: AdaptivePriority,
    },
    /// Generic algo limit. Tags: 847=strategy, 5957 + 5958/5960 per parameter.
    Algo {
        /// Fill at this price or better, scaled by `PRICE_SCALE`.
        price: Price,
        /// Which algorithm runs it, and what it was given.
        algo: AlgoParams,
    },
    /// Margin preview of an order of type `ord_type` (the wire character, tag
    /// 40). Tag 6091=1; the order is tracked under `ORD_WHAT_IF` so the
    /// response is recognised, and never becomes a live order.
    WhatIf {
        /// Fill at this price or better, scaled by `PRICE_SCALE`.
        price: Price,
        /// What kind of order is being previewed.
        ord_type: u8,
    },
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
    /// The venue's id for the contract.
    pub con_id: i64,
    /// How much of the hedge goes with one of the order.
    pub delta: f64,
    /// The price, scaled by `PRICE_SCALE`.
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
    /// Where a short leg's borrow is located.
    pub designated_location: String,
    /// -1 unless the leg is exempt from the short-sale price test.
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
    /// Generates 3 FIX messages: parent (35=D), TP child (35=D with 6107+583), SL child
    /// (35=D with 6107+583).
    SubmitBracket {
        /// The order this one is a child of.
        parent_id: OrderId,
        /// The number the take-profit child is placed under.
        tp_id: OrderId,
        /// The number the stop-loss child is placed under.
        sl_id: OrderId,
        /// The engine's own slot for the contract.
        instrument: InstrumentId,
        /// Whether it buys or sells.
        side: Side,
        /// How much, scaled by `QTY_SCALE`.
        qty: u32,
        /// Where the parent enters.
        entry_price: Price,
        /// Where the profit-taking child sits.
        take_profit: Price,
        /// Where the protective child sits.
        stop_loss: Price,
    },
    /// Extended submission for any order type: `kind` selects the order type
    /// and its prices, paired with a TIF and the full `OrderAttrs` block.
    /// This is how non-LMT types carry parent_id/oca_group/outside_rth/tif.
    SubmitEx {
        /// The caller's number for the order.
        order_id: OrderId,
        /// The engine's own slot for the contract.
        instrument: InstrumentId,
        /// Whether it buys or sells.
        side: Side,
        /// How much, scaled by `QTY_SCALE`.
        qty: u32,
        /// What kind of order this is, and the prices that kind needs.
        kind: OrderKind,
        /// How long the order lives, as the wire carries it.
        tif: u8,
        /// Everything else the caller set on it.
        attrs: OrderAttrs,
    },
    /// Limit order for opening auction (TIF=OPG).
    /// Algorithmic order: limit order with IB algo strategy overlay (VWAP, TWAP, etc.).
    /// Pegged to Benchmark: pegs to a benchmark instrument's price. OrdType PB.
    /// Companion tags: 6941=refConId, 6938=isPegDecrease, 6939=pegChangeAmt,
    /// 6942=refChangeAmt.
    /// Limit order for auction (TIF=AUC, tag 59=8). Participates in exchange
    /// opening/closing auction.
    /// Market-to-Limit for auction (TIF=AUC, tag 59=8). MTL + auction participation.
    /// What-If order: sends a limit order with tag 6091=1 for margin/commission
    /// preview.
    /// The order is NOT placed — response comes back as 35=8 with margin fields.
    /// Fractional shares limit order. Qty is fixed-point, `QTY_SCALE`, and
    /// goes out on tag 38 as a decimal string.
    SubmitLimitFractional {
        /// The caller's number for the order.
        order_id: OrderId,
        /// The engine's own slot for the contract.
        instrument: InstrumentId,
        /// Whether it buys or sells.
        side: Side,
        /// How much, scaled by `QTY_SCALE`.
        qty: Qty, // QTY_SCALE fixed-point
        /// The price, scaled by `PRICE_SCALE`.
        price: Price,
    },
    /// Withdraw one order.
    Cancel {
        /// The caller's number for the order.
        order_id: OrderId,
    },
    /// Withdraw every order on one contract.
    CancelAll {
        /// The engine's own slot for the contract.
        instrument: InstrumentId,
    },
    /// Replace a working order.
    ///
    /// Carries what the replace message states rather than restating the
    /// tracked original, so a caller changing the order type, the time-in-force
    /// or the trigger has the change reach the gateway.
    /// A zero `tif` states none and leaves the resting value in force.
    Modify {
        /// The caller's number for the order.
        order_id: OrderId,
        /// The price, scaled by `PRICE_SCALE`.
        price: Price,
        /// How much, scaled by `QTY_SCALE`.
        qty: u32,
        /// Outside-RTH flag from the order the caller resubmitted. The replace
        /// asserts tag 6433 from this rather than from the tracked record,
        /// which has no field for it.
        outside_rth: bool,
        /// Order type and time-in-force the replacement carries, as
        /// `Order::ord_type` and `Order::tif`. A replace that restated neither
        /// left the gateway to infer them, and it inferred a plain limit.
        ord_type: u8,
        /// How long the order lives, as the wire carries it.
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

    /// Every order this request puts on the wire.
    ///
    /// A bracket is three messages under one request, and all three are sent
    /// whatever any one of them returns. A failure reported against the first
    /// id alone leaves the other two carrying a status the wire never
    /// confirmed, so recovery walks this rather than [`Self::order_id`].
    pub fn order_ids(&self) -> Vec<OrderId> {
        match self {
            Self::SubmitBracket { parent_id, tp_id, sl_id, .. } => {
                vec![*parent_id, *tp_id, *sl_id]
            }
            other => vec![other.order_id()],
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

    /// Snap every outbound price-like field to the instrument's tick grid. `tick` is
    /// the fixed-point tick from
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
    /// An empty buffer.
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(MAX_PENDING_ORDERS),
        }
    }

    /// Add a request.
    ///
    /// Nothing is dropped. The capacity is what a healthy backlog fits in, not
    /// a limit on what a caller may send: an order thrown away here would be
    /// one a caller was told nothing about, which is the one outcome an order
    /// path cannot have. Past that size the buffer grows, and the assertion
    /// says so in a debug build, because a backlog that deep means the
    /// transport is not draining rather than that a caller is busy.
    pub fn push(&mut self, req: OrderRequest) {
        debug_assert!(self.buf.len() < MAX_PENDING_ORDERS, "order buffer overflow");
        self.buf.push(req);
    }

    /// Put requests back at the head, ahead of anything queued since.
    ///
    /// Used where a batch was taken and part of it was not sent, so it waits
    /// for the transport rather than being reported.
    pub fn requeue_front(&mut self, reqs: Vec<OrderRequest>) {
        if reqs.is_empty() { return; }
        debug_assert!(self.buf.len() + reqs.len() <= MAX_PENDING_ORDERS, "order buffer overflow");
        self.buf.splice(0..0, reqs);
    }

    /// Take everything buffered.
    pub fn drain(&mut self) -> std::vec::Drain<'_, OrderRequest> {
        self.buf.drain(..)
    }

    /// Whether anything is buffered.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

/// A completed order record for req_completed_orders.
#[derive(Debug, Clone)]
pub struct CompletedOrder {
    /// The order.
    pub order_id: OrderId,
    /// The contract it was on.
    pub instrument: InstrumentId,
    /// What it finished as.
    pub status: OrderStatus,
    /// How much filled.
    pub filled_qty: i64,
    /// When, in nanoseconds since the epoch.
    pub timestamp_ns: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
