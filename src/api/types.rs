//! ibapi-compatible types: Contract, Order, OrderState, Execution, TagValue, BarData,
//! ContractDetails, ContractDescription, and order conditions.
//!
//! These are plain Rust structs (no PyO3) shared by both the Rust EClient and the Python bridge.

use crate::types::*;

pub const PRICE_SCALE_F: f64 = PRICE_SCALE as f64;

// ── ComboLeg ──

/// ibapi-compatible ComboLeg for combination orders.
#[derive(Clone, Debug, Default)]
pub struct ComboLeg {
    pub con_id: i64,
    pub ratio: i32,
    pub action: String,
    pub exchange: String,
    pub open_close: i32,
    pub shorting_policy: i32,
    pub designated_location: String,
    pub exempt_code: i32,
}

// ── DeltaNeutralContract ──

/// ibapi-compatible DeltaNeutralContract for delta-neutral orders.
#[derive(Clone, Debug, Default)]
pub struct DeltaNeutralContract {
    pub con_id: i64,
    pub delta: f64,
    pub price: f64,
}

// ── Contract ──

/// ibapi-compatible Contract. Matches C++ `Contract` struct fields.
#[derive(Clone, Debug, Default)]
pub struct Contract {
    pub con_id: i64,
    pub symbol: String,
    pub sec_type: String,
    pub exchange: String,
    pub currency: String,
    pub last_trade_date_or_contract_month: String,
    pub strike: f64,
    pub right: String,
    pub multiplier: String,
    pub local_symbol: String,
    pub primary_exchange: String,
    pub trading_class: String,
    pub last_trade_date: String,
    pub include_expired: bool,
    pub sec_id_type: String,
    pub sec_id: String,
    pub description: String,
    pub issuer_id: String,
    pub combo_legs_descrip: String,
    pub combo_legs: Vec<ComboLeg>,
    pub delta_neutral_contract: Option<DeltaNeutralContract>,
}

// ── Order ──

/// ibapi-compatible Order. Matches C++ `Order` struct fields.
#[derive(Clone, Debug)]
pub struct Order {
    pub order_id: i64,
    pub action: String,
    pub total_quantity: f64,
    pub order_type: String,
    pub lmt_price: f64,
    pub aux_price: f64,
    pub tif: String,
    pub outside_rth: bool,
    pub display_size: i32,
    pub min_qty: i32,
    pub hidden: bool,
    pub good_after_time: String,
    pub good_till_date: String,
    pub oca_group: String,
    pub trailing_percent: f64,
    pub algo_strategy: String,
    pub algo_params: Vec<TagValue>,
    pub what_if: bool,
    pub cash_qty: f64,
    pub parent_id: i64,
    pub transmit: bool,
    pub discretionary_amt: f64,
    pub sweep_to_fill: bool,
    pub all_or_none: bool,
    pub trigger_method: i32,
    pub adjusted_order_type: String,
    pub trigger_price: f64,
    pub adjusted_stop_price: f64,
    pub adjusted_stop_limit_price: f64,
    pub conditions: Vec<OrderCondition>,
    pub conditions_ignore_rth: bool,
    pub conditions_cancel_order: bool,
    // ── ibapi-parity fields ──
    pub account: String,
    pub active_start_time: String,
    pub active_stop_time: String,
    pub adjustable_trailing_unit: i32,
    pub adjusted_trailing_amount: f64,
    pub advanced_error_override: String,
    pub algo_id: String,
    pub allow_pre_open: bool,
    pub auction_strategy: i32,
    pub auto_cancel_date: String,
    pub auto_cancel_parent: bool,
    pub basis_points: f64,
    pub basis_points_type: i32,
    pub block_order: bool,
    pub bond_accrued_interest: String,
    pub clearing_account: String,
    pub clearing_intent: String,
    pub client_id: i32,
    pub compete_against_best_offset: f64,
    pub continuous_update: bool,
    pub customer_account: String,
    pub deactivate: bool,
    /// Stand the order down if the connection goes (tag 6661).
    pub deactivate_on_disconnect: bool,
    pub delta: f64,
    pub delta_neutral_aux_price: f64,
    pub delta_neutral_clearing_account: String,
    pub delta_neutral_clearing_intent: String,
    pub delta_neutral_con_id: i32,
    pub delta_neutral_designated_location: String,
    pub delta_neutral_open_close: String,
    pub delta_neutral_order_type: String,
    pub delta_neutral_settling_firm: String,
    pub delta_neutral_short_sale: bool,
    pub delta_neutral_short_sale_slot: i32,
    pub designated_location: String,
    pub discretionary_up_to_limit_price: bool,
    pub dont_use_auto_price_for_hedge: bool,
    pub duration: i32,
    pub exempt_code: i32,
    pub ext_operator: String,
    pub fa_group: String,
    pub fa_method: String,
    pub fa_percentage: String,
    pub filled_quantity: f64,
    pub hedge_param: String,
    pub hedge_type: String,
    pub ignore_open_auction: bool,
    pub imbalance_only: bool,
    pub include_overnight: bool,
    pub is_oms_container: bool,
    pub is_pegged_change_amount_decrease: bool,
    pub lmt_price_offset: f64,
    pub manual_order_indicator: i32,
    pub manual_order_time: String,
    pub mid_offset_at_half: f64,
    pub mid_offset_at_whole: f64,
    pub mifid2_decision_algo: String,
    pub mifid2_decision_maker: String,
    pub mifid2_execution_algo: String,
    pub mifid2_execution_trader: String,
    pub min_compete_size: i32,
    pub min_trade_qty: i32,
    pub model_code: String,
    pub not_held: bool,
    pub oca_type: i32,
    pub open_close: String,
    pub opt_out_smart_routing: bool,
    pub order_combo_legs: Vec<f64>,
    pub order_misc_options: Vec<TagValue>,
    pub order_ref: String,
    pub origin: i32,
    pub override_percentage_constraints: bool,
    pub parent_perm_id: i64,
    pub pegged_change_amount: f64,
    pub percent_offset: f64,
    pub perm_id: i64,
    pub post_only: bool,
    pub post_to_ats: i32,
    pub professional_customer: bool,
    pub pt_order_id: i32,
    pub pt_order_type: String,
    pub randomize_price: bool,
    pub randomize_size: bool,
    pub ref_futures_con_id: i32,
    pub reference_change_amount: f64,
    pub reference_contract_id: i32,
    pub reference_exchange_id: String,
    pub reference_price_type: i32,
    pub route_marketable_to_bbo: bool,
    pub rule80a: String,
    pub scale_auto_reset: bool,
    pub scale_init_fill_qty: i32,
    pub scale_init_level_size: i32,
    pub scale_init_position: i32,
    pub scale_price_adjust_interval: i32,
    pub scale_price_adjust_value: f64,
    pub scale_price_increment: f64,
    pub scale_profit_offset: f64,
    pub scale_random_percent: bool,
    pub scale_subs_level_size: i32,
    pub scale_table: String,
    pub seek_price_improvement: bool,
    pub settling_firm: String,
    pub shareholder: String,
    pub short_sale_slot: i32,
    pub sl_order_id: i32,
    pub sl_order_type: String,
    pub smart_combo_routing_params: Vec<TagValue>,
    pub soft_dollar_tier_name: String,
    pub soft_dollar_tier_val: String,
    pub soft_dollar_tier_display_name: String,
    pub solicited: bool,
    pub starting_price: f64,
    pub stock_range_lower: f64,
    pub stock_range_upper: f64,
    pub stock_ref_price: f64,
    pub submitter: String,
    pub trail_stop_price: f64,
    pub use_price_mgmt_algo: i32,
    pub volatility: f64,
    pub volatility_type: i32,
    pub what_if_type: i32,
}

impl Default for Order {
    fn default() -> Self {
        Self {
            order_id: 0,
            action: String::new(),
            total_quantity: 0.0,
            order_type: String::new(),
            lmt_price: 0.0,
            aux_price: 0.0,
            tif: "DAY".into(),
            outside_rth: false,
            display_size: 0,
            min_qty: 0,
            hidden: false,
            good_after_time: String::new(),
            good_till_date: String::new(),
            oca_group: String::new(),
            trailing_percent: 0.0,
            algo_strategy: String::new(),
            algo_params: Vec::new(),
            what_if: false,
            cash_qty: 0.0,
            parent_id: 0,
            transmit: true,
            discretionary_amt: 0.0,
            sweep_to_fill: false,
            all_or_none: false,
            trigger_method: 0,
            adjusted_order_type: String::new(),
            trigger_price: 0.0,
            adjusted_stop_price: 0.0,
            adjusted_stop_limit_price: 0.0,
            conditions: Vec::new(),
            conditions_ignore_rth: false,
            conditions_cancel_order: false,
            // ibapi-parity defaults
            account: String::new(),
            active_start_time: String::new(),
            active_stop_time: String::new(),
            adjustable_trailing_unit: 0,
            adjusted_trailing_amount: f64::MAX,
            advanced_error_override: String::new(),
            algo_id: String::new(),
            allow_pre_open: false,
            auction_strategy: 0,
            auto_cancel_date: String::new(),
            auto_cancel_parent: false,
            basis_points: f64::MAX,
            basis_points_type: i32::MAX,
            block_order: false,
            bond_accrued_interest: String::new(),
            clearing_account: String::new(),
            clearing_intent: String::new(),
            client_id: 0,
            compete_against_best_offset: f64::MAX,
            continuous_update: false,
            customer_account: String::new(),
            deactivate: false,
            deactivate_on_disconnect: false,
            delta: f64::MAX,
            delta_neutral_aux_price: f64::MAX,
            delta_neutral_clearing_account: String::new(),
            delta_neutral_clearing_intent: String::new(),
            delta_neutral_con_id: 0,
            delta_neutral_designated_location: String::new(),
            delta_neutral_open_close: String::new(),
            delta_neutral_order_type: String::new(),
            delta_neutral_settling_firm: String::new(),
            delta_neutral_short_sale: false,
            delta_neutral_short_sale_slot: 0,
            designated_location: String::new(),
            discretionary_up_to_limit_price: false,
            dont_use_auto_price_for_hedge: true,
            duration: i32::MAX,
            exempt_code: -1,
            ext_operator: String::new(),
            fa_group: String::new(),
            fa_method: String::new(),
            fa_percentage: String::new(),
            filled_quantity: 0.0,
            hedge_param: String::new(),
            hedge_type: String::new(),
            ignore_open_auction: false,
            imbalance_only: false,
            include_overnight: false,
            is_oms_container: false,
            is_pegged_change_amount_decrease: false,
            lmt_price_offset: f64::MAX,
            manual_order_indicator: i32::MAX,
            manual_order_time: String::new(),
            mid_offset_at_half: f64::MAX,
            mid_offset_at_whole: f64::MAX,
            mifid2_decision_algo: String::new(),
            mifid2_decision_maker: String::new(),
            mifid2_execution_algo: String::new(),
            mifid2_execution_trader: String::new(),
            min_compete_size: i32::MAX,
            min_trade_qty: i32::MAX,
            model_code: String::new(),
            not_held: false,
            oca_type: 0,
            open_close: String::new(),
            opt_out_smart_routing: false,
            order_combo_legs: Vec::new(),
            order_misc_options: Vec::new(),
            order_ref: String::new(),
            origin: 0,
            override_percentage_constraints: false,
            parent_perm_id: 0,
            pegged_change_amount: 0.0,
            percent_offset: f64::MAX,
            perm_id: 0,
            post_only: false,
            post_to_ats: i32::MAX,
            professional_customer: false,
            pt_order_id: i32::MAX,
            pt_order_type: String::new(),
            randomize_price: false,
            randomize_size: false,
            ref_futures_con_id: 0,
            reference_change_amount: 0.0,
            reference_contract_id: 0,
            reference_exchange_id: String::new(),
            reference_price_type: 0,
            route_marketable_to_bbo: false,
            rule80a: String::new(),
            scale_auto_reset: false,
            scale_init_fill_qty: i32::MAX,
            scale_init_level_size: i32::MAX,
            scale_init_position: i32::MAX,
            scale_price_adjust_interval: i32::MAX,
            scale_price_adjust_value: f64::MAX,
            scale_price_increment: f64::MAX,
            scale_profit_offset: f64::MAX,
            scale_random_percent: false,
            scale_subs_level_size: i32::MAX,
            scale_table: String::new(),
            seek_price_improvement: false,
            settling_firm: String::new(),
            shareholder: String::new(),
            short_sale_slot: 0,
            sl_order_id: i32::MAX,
            sl_order_type: String::new(),
            smart_combo_routing_params: Vec::new(),
            soft_dollar_tier_name: String::new(),
            soft_dollar_tier_val: String::new(),
            soft_dollar_tier_display_name: String::new(),
            solicited: false,
            starting_price: f64::MAX,
            stock_range_lower: f64::MAX,
            stock_range_upper: f64::MAX,
            stock_ref_price: f64::MAX,
            submitter: String::new(),
            trail_stop_price: f64::MAX,
            use_price_mgmt_algo: 0,
            volatility: f64::MAX,
            volatility_type: 0,
            what_if_type: i32::MAX,
        }
    }
}

impl Order {
    /// Parse the action string to Side.
    pub fn side(&self) -> Result<Side, String> {
        match self.action.to_uppercase().as_str() {
            "BUY" | "B" => Ok(Side::Buy),
            "SELL" | "S" => Ok(Side::Sell),
            "SSHORT" | "SS" => Ok(Side::ShortSell),
            _ => Err(format!("Invalid action '{}': use BUY or SELL", self.action)),
        }
    }

    /// Parse the TIF string to FIX byte.
    /// The order-type byte this order tracks under, or 0 when the type is one
    /// a replace cannot state.
    ///
    /// Only the types a modify is accepted for are mapped; everything else is
    /// refused before it reaches a replace, and 0 tells the encoder to keep
    /// whatever the resting order holds (ibx#349).
    pub fn ord_type_byte(&self) -> u8 {
        match self.order_type.to_uppercase().as_str() {
            "MKT" => b'1',
            "LMT" => b'2',
            "STP" => b'3',
            "STP LMT" => b'4',
            "MOC" => b'5',
            "LOC" => b'B',
            "MIT" => b'J',
            "MTL" | "BOX TOP" => b'K',
            "MKT PRT" => b'U',
            "STP PRT" => crate::types::ORD_STP_PRT,
            _ => 0,
        }
    }

    pub fn tif_byte(&self) -> u8 {
        match self.tif.as_str() {
            "GTC" => b'1',
            "IOC" => b'3',
            "FOK" => b'4',
            "OPG" => b'2',
            "GTD" | "DTC" => b'6',
            "AUC" => b'8',
            _ => b'0', // DAY
        }
    }

    /// Build OrderAttrs from Order fields.
    pub fn attrs(&self) -> OrderAttrs {
        // Parse the good-till expiry string into either a UTC instant (tag 126)
        // or a calendar date (tag 432). On a parse error, log and drop the
        // expiry — the order then surfaces a visible gateway rejection rather
        // than silently carrying a wrong expiry.
        let (good_till, good_till_date_ymd) =
            match crate::config::parse_ib_expiry(&self.good_till_date) {
                Ok(None) => (0, 0),
                Ok(Some(crate::config::IbExpiry::Instant(secs))) => (secs, 0),
                Ok(Some(crate::config::IbExpiry::DateOnly(ymd))) => (0, ymd),
                Err(e) => {
                    log::warn!("dropping good_till_date: {e}");
                    (0, 0)
                }
            };
        OrderAttrs {
            display_size: self.display_size.max(0) as u32,
            min_qty: self.min_qty.max(0) as u32,
            hidden: self.hidden,
            outside_rth: self.outside_rth,
            // good_after_time (tag 168) wire format is not yet captured against
            // the gateway; left unset until verified (see ibx#199 / ib-agent).
            good_after: 0,
            good_till,
            good_till_date_ymd,
            oca_group: self.oca_group.parse().unwrap_or(0),
            oca_group_str: if self.oca_group.parse::<u64>().is_err() && !self.oca_group.is_empty() {
                self.oca_group.clone()
            } else {
                String::new()
            },
            parent_id: self.parent_id.max(0) as u64,
            discretionary_amt: (self.discretionary_amt * PRICE_SCALE_F) as Price,
            sweep_to_fill: self.sweep_to_fill,
            all_or_none: self.all_or_none,
            // `f64::MAX` is this API's "not set" for a price-like field, and
            // it is not a volatility or an offset.
            volatility: if self.volatility == f64::MAX { 0.0 } else { self.volatility },
            volatility_type: self.volatility_type.clamp(0, 255) as u8,
            // Stated by a caller and carried nowhere until now: an order that
            // asked to be re-priced as the underlying moved, or to stay inside
            // a band of underlying prices, was accepted and sent without either.
            seek_price_improvement: self.seek_price_improvement,
            manual_order_time: self.manual_order_time.clone(),
            advanced_error_override: self.advanced_error_override.clone(),
            active_start_time: self.active_start_time.clone(),
            active_stop_time: self.active_stop_time.clone(),
            post_only: self.post_only,
            solicited: self.solicited,
            manual_order_indicator: if self.manual_order_indicator == i32::MAX { 0 } else { self.manual_order_indicator },
            route_marketable_to_bbo: self.route_marketable_to_bbo,
            imbalance_only: self.imbalance_only,
            allow_pre_open: self.allow_pre_open,
            ignore_open_auction: self.ignore_open_auction,
            is_oms_container: self.is_oms_container,
            ext_operator: self.ext_operator.clone(),
            customer_account: self.customer_account.clone(),
            professional_customer: self.professional_customer,
            ref_futures_con_id: self.ref_futures_con_id,
            mifid2_decision_maker: self.mifid2_decision_maker.clone(),
            mifid2_decision_algo: self.mifid2_decision_algo.clone(),
            mifid2_execution_trader: self.mifid2_execution_trader.clone(),
            mifid2_execution_algo: self.mifid2_execution_algo.clone(),
            mid_offset_at_whole: self.mid_offset_at_whole,
            mid_offset_at_half: self.mid_offset_at_half,
            use_price_mgmt_algo: self.use_price_mgmt_algo,
            duration: self.duration,
            min_compete_size: if self.min_compete_size == i32::MAX { 0 } else { self.min_compete_size },
            compete_against_best_offset: self.compete_against_best_offset,
            continuous_update: self.continuous_update,
            reference_price_type: self.reference_price_type,
            stock_range_lower: self.stock_range_lower,
            stock_range_upper: self.stock_range_upper,
            percent_offset: self.percent_offset,
            not_held: self.not_held,
            order_ref: self.order_ref.clone(),
            open_close: self.open_close.clone(),
            scale: self.scale_attrs(),
            delta_neutral: self.delta_neutral_attrs(),
            short_sale_slot: self.short_sale_slot.clamp(0, 255) as u8,
            designated_location: self.designated_location.clone(),
            exempt_code: self.exempt_code,
            // The wire takes a number, not the API's letter.
            hedge_type: match self.hedge_type.to_ascii_uppercase().as_str() {
                "F" => 1, "D" => 2, "P" => 3, "B" => 4, "S" => 5, _ => 0,
            },
            hedge_beta: if self.hedge_type.eq_ignore_ascii_case("B") {
                self.hedge_param.parse().unwrap_or(0.0)
            } else { 0.0 },
            hedge_ratio: if self.hedge_type.eq_ignore_ascii_case("P") {
                self.hedge_param.parse().unwrap_or(0.0)
            } else { 0.0 },
            deactivate: self.deactivate,
            deactivate_on_disconnect: self.deactivate_on_disconnect,
            include_overnight: self.include_overnight,
            auto_cancel_parent: self.auto_cancel_parent,
            min_trade_qty: if self.min_trade_qty == i32::MAX { 0 } else { self.min_trade_qty.max(0) as u32 },
            block_order: self.block_order,
            auto_cancel_date: self.auto_cancel_date.clone(),
            clearing_account: self.clearing_account.clone(),
            clearing_intent: self.clearing_intent.clone(),
            rule80a: self.rule80a.clone(),
            post_to_ats: if self.post_to_ats == i32::MAX { 0 } else { self.post_to_ats.max(0) as u32 },
            combo_legs: Vec::new(),
            primary_exchange: String::new(),
            delta_neutral_contract: None,
            // Valid trigger-method codes only (ibx#223): the raw `as u8`
            // cast wrapped the gateway's -1 (Unknown) to 255, and
            // out-of-range codes went to the wire verbatim. Anything
            // unrecognized coerces to 0 (default = not emitted), matching
            // the gateway's unknown->default handling.
            trigger_method: match self.trigger_method {
                0..=4 | 7 | 8 => self.trigger_method as u8,
                _ => 0,
            },
            cash_qty: (self.cash_qty * PRICE_SCALE_F) as Price,
            conditions: self.conditions.clone(),
            conditions_cancel_order: self.conditions_cancel_order,
            conditions_ignore_rth: self.conditions_ignore_rth,
            // Keep 1..=4; anything else is "unset" and emits the gateway
            // default 3 (ReduceOnFillNonBlock). See ibx#215.
            oca_type: match self.oca_type {
                1..=4 => self.oca_type as u8,
                _ => 0,
            },
            // No field on an order sets this. An exercise names an action and a
            // number of contracts and nothing else an order carries, so it has
            // a call of its own that builds the request directly.
            exercise_action: 0,
        }
    }

    /// Check if the order has any extended attributes set.
    /// The ladder this order describes, if it describes one.
    ///
    /// `i32::MAX` and `f64::MAX` are this API's "not set", so a field left
    /// alone contributes nothing and an order that sets none has no ladder.
    fn scale_attrs(&self) -> Option<Box<crate::types::ScaleAttrs>> {
        // Any one of them means a ladder was asked for. Keying only off the
        // first size and the step let the rest be set on their own and dropped.
        let asked = self.scale_init_level_size != i32::MAX
            || self.scale_subs_level_size != i32::MAX
            || self.scale_price_increment != f64::MAX
            || self.scale_profit_offset != f64::MAX
            || self.scale_price_adjust_value != f64::MAX
            || self.scale_price_adjust_interval != i32::MAX
            || self.scale_auto_reset
            || self.scale_random_percent;
        if !asked {
            return None;
        }
        let px = |v: f64| if v == f64::MAX { 0 } else { (v * PRICE_SCALE_F) as i64 };
        let n = |v: i32| if v == i32::MAX { 0 } else { v.max(0) as u32 };
        Some(Box::new(crate::types::ScaleAttrs {
            init_level_size: n(self.scale_init_level_size),
            subs_level_size: n(self.scale_subs_level_size),
            price_increment: px(self.scale_price_increment),
            profit_offset: px(self.scale_profit_offset),
            price_adjust_value: px(self.scale_price_adjust_value),
            price_adjust_interval: n(self.scale_price_adjust_interval),
            auto_reset: self.scale_auto_reset,
            random_percent: self.scale_random_percent,
        }))
    }

    /// The hedging leg this order asks for, if it asks for one.
    fn delta_neutral_attrs(&self) -> Option<Box<crate::types::DeltaNeutralAttrs>> {
        if self.delta_neutral_order_type.is_empty() {
            return None;
        }
        Some(Box::new(crate::types::DeltaNeutralAttrs {
            order_type: self.delta_neutral_order_type.clone(),
            aux_price: if self.delta_neutral_aux_price == f64::MAX { 0 }
                       else { (self.delta_neutral_aux_price * PRICE_SCALE_F) as i64 },
            con_id: self.delta_neutral_con_id as i64,
        }))
    }
    /// Whether this order states anything beyond a plain one.
    ///
    /// Every order routes through the encoder now, so nothing branches on
    /// this: it is kept because it answers a question worth asking of an
    /// order, and its own tests are what check the attribute block stays
    /// complete as fields are added.
    pub fn has_extended_attrs(&self) -> bool {
        self.display_size > 0
            || self.min_qty > 0
            || self.hidden
            || self.outside_rth
            || !self.good_after_time.is_empty()
            || !self.good_till_date.is_empty()
            || !self.oca_group.is_empty()
            || self.parent_id > 0
            || self.discretionary_amt > 0.0
            || self.sweep_to_fill
            || self.all_or_none
            || self.trigger_method > 0
            || self.cash_qty > 0.0
            // Everything `attrs()` carries has to be named here, or the order
            // takes the plain encoder and the attribute is dropped without a
            // word. Conditions are the costly one: the order goes out
            // unconditional and routes immediately.
            || !self.conditions.is_empty()
            || self.conditions_cancel_order
            || self.conditions_ignore_rth
            || self.oca_type > 0
            || (self.volatility != f64::MAX && self.volatility > 0.0)
            || self.volatility_type > 0
            || self.seek_price_improvement
            || !self.manual_order_time.is_empty()
            || !self.advanced_error_override.is_empty()
            || !self.active_start_time.is_empty()
            || !self.active_stop_time.is_empty()
            || self.post_only
            || self.solicited
            || (self.manual_order_indicator != i32::MAX && self.manual_order_indicator > 0)
            || self.route_marketable_to_bbo
            || self.imbalance_only
            || self.allow_pre_open
            || self.ignore_open_auction
            || self.is_oms_container
            || !self.ext_operator.is_empty()
            || !self.customer_account.is_empty()
            || self.professional_customer
            || self.ref_futures_con_id > 0
            || !self.mifid2_decision_maker.is_empty()
            || !self.mifid2_decision_algo.is_empty()
            || !self.mifid2_execution_trader.is_empty()
            || !self.mifid2_execution_algo.is_empty()
            || self.mid_offset_at_whole != f64::MAX
            || self.mid_offset_at_half != f64::MAX
            || self.use_price_mgmt_algo > 0
            || self.duration != i32::MAX
            || (self.min_compete_size != i32::MAX && self.min_compete_size > 0)
            || self.compete_against_best_offset != f64::MAX
            || self.continuous_update
            || self.reference_price_type > 0
            || self.stock_range_lower != f64::MAX
            || self.stock_range_upper != f64::MAX
            || self.percent_offset != f64::MAX
            || self.not_held
            || !self.order_ref.is_empty()
            || !self.open_close.is_empty()
            || self.scale_attrs().is_some()
            || !self.delta_neutral_order_type.is_empty()
            || self.short_sale_slot != 0
            || !self.designated_location.is_empty()
            || self.exempt_code != -1
            || !self.hedge_type.is_empty()
            || !self.rule80a.is_empty()
            || self.post_to_ats != i32::MAX
            || self.deactivate
            || self.deactivate_on_disconnect
            || self.include_overnight
            || self.auto_cancel_parent
            || self.min_trade_qty != i32::MAX
            || self.block_order
            || !self.auto_cancel_date.is_empty()
            || !self.clearing_account.is_empty()
            || !self.clearing_intent.is_empty()
    }
}

// ── TagValue ──

/// ibapi-compatible TagValue for algo and scanner filter parameters.
#[derive(Clone, Debug)]
pub struct TagValue {
    pub tag: String,
    pub value: String,
}

// ── OrderState ──

/// Per-account allocation for grouped/allocation orders (ibapi-compatible).
/// Decimal fields are carried as strings to preserve precision.
#[derive(Clone, Debug, Default)]
pub struct OrderAllocation {
    pub account: String,
    pub position: String,
    pub position_desired: String,
    pub position_after: String,
    pub desired_alloc_qty: String,
    pub allowed_alloc_qty: String,
    pub is_monetary: bool,
}

/// ibapi-compatible OrderState (used in openOrder callback).
#[derive(Clone, Debug, Default)]
pub struct OrderState {
    pub status: String,
    pub init_margin_before: String,
    pub maint_margin_before: String,
    pub equity_with_loan_before: String,
    pub init_margin_change: String,
    pub maint_margin_change: String,
    pub equity_with_loan_change: String,
    pub init_margin_after: String,
    pub maint_margin_after: String,
    pub equity_with_loan_after: String,
    pub commission_and_fees: f64,
    pub min_commission_and_fees: f64,
    pub max_commission_and_fees: f64,
    pub commission_and_fees_currency: String,
    pub warning_text: String,
    pub completed_time: String,
    pub completed_status: String,
    // ── ibapi-iso extension (2026-04-30): RTH-split margin + allocations ──
    pub margin_currency: String,
    pub init_margin_before_outside_rth: f64,
    pub maint_margin_before_outside_rth: f64,
    pub equity_with_loan_before_outside_rth: f64,
    pub init_margin_change_outside_rth: f64,
    pub maint_margin_change_outside_rth: f64,
    pub equity_with_loan_change_outside_rth: f64,
    pub init_margin_after_outside_rth: f64,
    pub maint_margin_after_outside_rth: f64,
    pub equity_with_loan_after_outside_rth: f64,
    pub suggested_size: String,
    pub reject_reason: String,
    pub order_allocations: Vec<OrderAllocation>,
}

// ── Execution ──

/// ibapi-compatible Execution (used in execDetails callback).
#[derive(Clone, Debug, Default)]
pub struct Execution {
    pub exec_id: String,
    pub time: String,
    pub acct_number: String,
    pub exchange: String,
    pub side: String,
    pub shares: f64,
    pub price: f64,
    pub perm_id: i64,
    pub client_id: i64,
    pub order_id: i64,
    pub cum_qty: f64,
    pub avg_price: f64,
    pub last_liquidity: i32,
    pub liquidation: i32,
    pub model_code: String,
    pub ev_rule: String,
    pub ev_multiplier: f64,
    pub pending_price_revision: bool,
}

// ── ExecutionFilter ──

/// ibapi-compatible ExecutionFilter (used in reqExecutions).
#[derive(Clone, Debug, Default)]
pub struct ExecutionFilter {
    pub client_id: i64,
    pub acct_code: String,
    pub time: String,
    pub symbol: String,
    pub sec_type: String,
    pub exchange: String,
    pub side: String,
}

// ── CommissionAndFeesReport ──

/// ibapi-compatible CommissionAndFeesReport.
#[derive(Clone, Debug, Default)]
pub struct CommissionAndFeesReport {
    pub exec_id: String,
    pub commission_and_fees: f64,
    pub currency: String,
    pub realized_pnl: f64,
    pub yield_amount: f64,
    pub yield_redemption_date: String,
}

// ── TickAttrib ──

/// ibapi-compatible TickAttrib for tick_price callback.
#[derive(Clone, Debug, Default)]
pub struct TickAttrib {
    pub can_auto_execute: bool,
    pub past_limit: bool,
    pub pre_open: bool,
}

// ── TickAttribLast ──

/// ibapi-compatible TickAttribLast for tick_by_tick_all_last callback.
#[derive(Clone, Debug, Default)]
pub struct TickAttribLast {
    pub past_limit: bool,
    pub unreported: bool,
}

// ── TickAttribBidAsk ──

/// ibapi-compatible TickAttribBidAsk for tick_by_tick_bid_ask callback.
#[derive(Clone, Debug, Default)]
pub struct TickAttribBidAsk {
    pub bid_past_low: bool,
    pub ask_past_high: bool,
}

// ── BarData ──

/// ibapi-compatible BarData for historical data callbacks.
#[derive(Clone, Debug)]
pub struct BarData {
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub wap: f64,
    pub bar_count: i32,
    /// Timezone of `date` as reported by the reply (ibx#234) — previously
    /// parsed and then discarded, leaving the bare timestamp string as the
    /// only (unverifiable) evidence of what the bar times mean. Empty on
    /// streaming updates, which carry no timezone of their own.
    pub timezone: String,
}

impl Default for BarData {
    fn default() -> Self {
        Self {
            date: String::new(),
            open: 0.0,
            high: 0.0,
            low: 0.0,
            close: 0.0,
            volume: 0,
            wap: 0.0,
            bar_count: 0,
            timezone: String::new(),
        }
    }
}

// ── ContractDetails ──

/// ibapi-compatible ContractDetails.
///
/// `trading_hours` / `liquid_hours` carry semicolon-delimited UTC session strings
/// (`"YYYYMMDD:HHMM-YYYYMMDD:HHMM;..."`) when populated. Consumers should convert
/// to local time using `time_zone_id` for display.
#[derive(Clone, Debug, Default)]
pub struct ContractDetails {
    pub contract: Contract,
    pub market_name: String,
    pub min_tick: f64,
    pub order_types: String,
    pub valid_exchanges: String,
    pub long_name: String,
    pub last_trade_date: String,
    pub multiplier: String,
    pub trading_hours: Option<String>,
    pub liquid_hours: Option<String>,
    pub time_zone_id: Option<String>,
    /// The price-increment rules this contract trades under, as the definition
    /// states them. Parsed all along and never surfaced, so a caller had no way
    /// to learn which rule to ask `req_market_rule` for.
    pub market_rule_ids: String,
    /// What kind of stock it is, what it does, and where it is domiciled —
    /// parsed off the definition all along and handed to nobody.
    pub stock_type: String,
    /// What a quoted price must be multiplied by to be a price. A price read
    /// without it is out by that factor, which is not a rounding error.
    /// What a bond is and what a fund is — terms, ratings, charges and where
    /// it may be sold. A caller asking about either received a symbol.
    pub coupon: f64,
    pub contract_month: String,
    pub under_sec_type: String,
    /// Every field the venue stated about this contract that this client does
    /// not yet name, as (tag, value). Kept rather than dropped: what is not
    /// named is still a fact the venue stated.
    pub under_con_id: u32,
    pub under_symbol: String,
    pub last_trade_time: String,
    pub issue_date: String,
    pub last_price_precision: f64,
    pub last_size_precision: f64,
    pub settlement_method: String,
    pub unnamed_fields: Vec<(u32, String)>,
    pub bond_notes: String,
    pub desc_append: String,
    pub bond_type: String,
    pub coupon_type: String,
    pub next_option_date: String,
    pub next_option_type: String,
    pub ratings: String,
    pub fund_name: String,
    pub fund_family: String,
    pub fund_type: String,
    pub fund_front_load: String,
    pub fund_back_load: String,
    pub fund_back_load_time_interval: String,
    pub fund_management_fee: String,
    pub fund_notify_amount: String,
    pub fund_minimum_initial_purchase: String,
    pub fund_minimum_subsequent_purchase: String,
    pub fund_blue_sky_states: String,
    pub fund_blue_sky_territories: String,
    pub fund_distribution_policy_indicator: String,
    pub fund_asset_type: String,
    pub real_expiration_date: String,
    pub callable: bool,
    pub puttable: bool,
    pub convertible: bool,
    pub next_option_partial: bool,
    pub fund_closed: bool,
    pub fund_closed_for_new_investors: bool,
    pub fund_closed_for_new_money: bool,
    pub agg_group: i32,
    pub price_magnifier: i32,
    /// What the issuer does, broadest first. The venue states all three in one
    /// field; kept whole, a caller asking for the category was handed all of
    /// them with bars between.
    pub industry: String,
    pub category: String,
    pub subcategory: String,
    pub country: String,
    /// The identifier the contract is known by outside this venue.
    pub isin: String,
    /// The identifier a contract is known by in the American market, taken from
    /// the identifiers below by its kind — it has no field of its own.
    pub cusip: String,
    /// Every identifier the contract is known by, as the kind and the value.
    pub sec_id_list: Vec<(String, String)>,
    /// The smallest quantity the contract trades in, which is not always one.
    pub min_size: f64,
}

impl ContractDetails {
    pub fn from_definition(def: &crate::control::contracts::ContractDefinition) -> Self {
        let c = Contract {
            con_id: def.con_id as i64,
            symbol: def.symbol.clone(),
            sec_type: def.sec_type.to_api_str().to_string(),
            exchange: def.exchange.clone(),
            primary_exchange: def.primary_exchange.clone(),
            currency: def.currency.clone(),
            local_symbol: def.local_symbol.clone(),
            trading_class: def.trading_class.clone(),
            last_trade_date_or_contract_month: def.last_trade_date.clone(),
            strike: def.strike,
            // Never carried across, so every option came back with its right
            // unset and a call was indistinguishable from a put outside the
            // local symbol.
            right: match def.right {
                Some(crate::control::contracts::OptionRight::Call) => "C".to_string(),
                Some(crate::control::contracts::OptionRight::Put) => "P".to_string(),
                None => String::new(),
            },
            multiplier: if def.multiplier != 1.0 { format!("{}", def.multiplier) } else { String::new() },
            ..Default::default()
        };
        Self {
            contract: c,
            // Parsed from the reply all along but thrown away (ibx#230).
            market_name: def.market_name.clone(),
            min_tick: def.min_tick,
            order_types: def.order_types.join(","),
            valid_exchanges: def.valid_exchanges.join(","),
            long_name: def.long_name.clone(),
            last_trade_date: def.last_trade_date.clone(),
            multiplier: if def.multiplier != 1.0 { format!("{}", def.multiplier) } else { String::new() },
            trading_hours: def.trading_hours.clone(),
            liquid_hours: def.liquid_hours.clone(),
            time_zone_id: def.time_zone_id.clone(),
            market_rule_ids: def.market_rule_id.map(|r| r.to_string()).unwrap_or_default(),
            stock_type: def.stock_type.clone(),
            coupon: def.coupon,
            contract_month: def.contract_month.clone(),
            under_sec_type: def.under_sec_type.clone(),
            under_con_id: def.under_con_id,
            under_symbol: def.under_symbol.clone(),
            last_trade_time: def.last_trade_time.clone(),
            issue_date: def.issue_date.clone(),
            last_price_precision: def.last_price_precision,
            last_size_precision: def.last_size_precision,
            settlement_method: def.settlement_method.clone(),
            unnamed_fields: def.unnamed_fields.clone(),
            bond_notes: def.bond_notes.clone(),
            desc_append: def.desc_append.clone(),
            bond_type: def.bond_type.clone(),
            coupon_type: def.coupon_type.clone(),
            next_option_date: def.next_option_date.clone(),
            next_option_type: def.next_option_type.clone(),
            ratings: def.ratings.clone(),
            fund_name: def.fund_name.clone(),
            fund_family: def.fund_family.clone(),
            fund_type: def.fund_type.clone(),
            fund_front_load: def.fund_front_load.clone(),
            fund_back_load: def.fund_back_load.clone(),
            fund_back_load_time_interval: def.fund_back_load_time_interval.clone(),
            fund_management_fee: def.fund_management_fee.clone(),
            fund_notify_amount: def.fund_notify_amount.clone(),
            fund_minimum_initial_purchase: def.fund_minimum_initial_purchase.clone(),
            fund_minimum_subsequent_purchase: def.fund_minimum_subsequent_purchase.clone(),
            fund_blue_sky_states: def.fund_blue_sky_states.clone(),
            fund_blue_sky_territories: def.fund_blue_sky_territories.clone(),
            fund_distribution_policy_indicator: def.fund_distribution_policy_indicator.clone(),
            fund_asset_type: def.fund_asset_type.clone(),
            real_expiration_date: def.real_expiration_date.clone(),
            callable: def.callable,
            puttable: def.puttable,
            convertible: def.convertible,
            next_option_partial: def.next_option_partial,
            fund_closed: def.fund_closed,
            fund_closed_for_new_investors: def.fund_closed_for_new_investors,
            fund_closed_for_new_money: def.fund_closed_for_new_money,
            agg_group: def.agg_group,
            price_magnifier: def.price_magnifier,
            industry: def.industry.clone(),
            category: def.category.clone(),
            subcategory: def.subcategory.clone(),
            country: def.country.clone(),
            isin: def.isin.clone(),
            cusip: def.cusip.clone(),
            sec_id_list: def.sec_id_list.clone(),
            min_size: def.min_size,
        }
    }
}

// ── ContractDescription ──

/// ibapi-compatible ContractDescription for symbol search results.
#[derive(Clone, Debug, Default)]
pub struct ContractDescription {
    pub con_id: i64,
    pub symbol: String,
    pub sec_type: String,
    pub currency: String,
    pub primary_exchange: String,
    pub derivative_sec_types: Vec<String>,
}

// ── PriceIncrement (for market rules) ──

/// ibapi-compatible PriceIncrement for market_rule callback.
#[derive(Clone, Debug)]
pub struct PriceIncrement {
    pub low_edge: f64,
    pub increment: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Contract ──

    #[test]
    fn contract_default_values() {
        let c = Contract::default();
        assert_eq!(c.con_id, 0);
        assert_eq!(c.symbol, "");
        assert_eq!(c.sec_type, "");
        assert_eq!(c.exchange, "");
        assert_eq!(c.currency, "");
        assert_eq!(c.strike, 0.0);
    }

    #[test]
    fn contract_clone() {
        let c = Contract { con_id: 265598, symbol: "AAPL".into(), ..Default::default() };
        let c2 = c.clone();
        assert_eq!(c2.con_id, 265598);
        assert_eq!(c2.symbol, "AAPL");
    }

    // ── Order ──

    #[test]
    fn order_default_values() {
        let o = Order::default();
        assert_eq!(o.order_id, 0);
        assert_eq!(o.action, "");
        assert_eq!(o.total_quantity, 0.0);
        assert_eq!(o.order_type, "");
        assert_eq!(o.tif, "DAY");
        assert!(o.transmit);
        assert!(!o.what_if);
        assert!(!o.outside_rth);
    }

    #[test]
    fn order_side_parsing() {
        let mut o = Order { action: "BUY".into(), ..Default::default() };
        assert_eq!(o.side().unwrap(), Side::Buy);
        o.action = "SELL".into();
        assert_eq!(o.side().unwrap(), Side::Sell);
        o.action = "SSHORT".into();
        assert_eq!(o.side().unwrap(), Side::ShortSell);
        o.action = "B".into();
        assert_eq!(o.side().unwrap(), Side::Buy);
        o.action = "S".into();
        assert_eq!(o.side().unwrap(), Side::Sell);
    }

    #[test]
    fn order_side_invalid() {
        let o = Order { action: "INVALID".into(), ..Default::default() };
        assert!(o.side().is_err());
    }

    #[test]
    fn order_tif_byte_mapping() {
        let mut o = Order { tif: "DAY".into(), ..Default::default() };
        assert_eq!(o.tif_byte(), b'0');
        o.tif = "GTC".into();
        assert_eq!(o.tif_byte(), b'1');
        o.tif = "IOC".into();
        assert_eq!(o.tif_byte(), b'3');
        o.tif = "FOK".into();
        assert_eq!(o.tif_byte(), b'4');
        o.tif = "OPG".into();
        assert_eq!(o.tif_byte(), b'2');
        o.tif = "GTD".into();
        assert_eq!(o.tif_byte(), b'6');
        o.tif = "AUC".into();
        assert_eq!(o.tif_byte(), b'8');
    }

    #[test]
    fn order_has_extended_attrs() {
        let o = Order::default();
        assert!(!o.has_extended_attrs());

        let o2 = Order { hidden: true, ..Default::default() };
        assert!(o2.has_extended_attrs());

        let o3 = Order { display_size: 50, ..Default::default() };
        assert!(o3.has_extended_attrs());
    }

    #[test]
    fn order_attrs_conversion() {
        let o = Order {
            display_size: 50,
            hidden: true,
            discretionary_amt: 0.05,
            ..Default::default()
        };
        let attrs = o.attrs();
        assert_eq!(attrs.display_size, 50);
        assert!(attrs.hidden);
        assert_eq!(attrs.discretionary_amt, (0.05 * PRICE_SCALE_F) as Price);
    }

    #[test]
    fn order_attrs_conditions_forwarded() {
        let o = Order {
            conditions: vec![
                OrderCondition::Time { time: "20260311-09:30:00".into(), is_more: true },
            ],
            conditions_cancel_order: true,
            ..Default::default()
        };
        let attrs = o.attrs();
        assert_eq!(attrs.conditions.len(), 1);
        assert!(attrs.conditions_cancel_order);
    }

    // ── TagValue ──

    #[test]
    fn tag_value_fields() {
        let tv = TagValue { tag: "maxPctVol".into(), value: "0.1".into() };
        assert_eq!(tv.tag, "maxPctVol");
        assert_eq!(tv.value, "0.1");
    }

    // ── OrderState ──

    #[test]
    fn order_state_default() {
        let os = OrderState::default();
        assert_eq!(os.status, "");
        assert_eq!(os.commission_and_fees, 0.0);
    }

    // ── Execution ──

    #[test]
    fn execution_default() {
        let e = Execution::default();
        assert_eq!(e.exec_id, "");
        assert_eq!(e.shares, 0.0);
        assert_eq!(e.price, 0.0);
    }

    // ── TickAttrib ──

    #[test]
    fn tick_attrib_default() {
        let ta = TickAttrib::default();
        assert!(!ta.can_auto_execute);
        assert!(!ta.past_limit);
        assert!(!ta.pre_open);
    }

    // ── BarData ──

    #[test]
    fn bar_data_default() {
        let b = BarData::default();
        assert_eq!(b.date, "");
        assert_eq!(b.open, 0.0);
        assert_eq!(b.volume, 0);
    }

    // ── ContractDetails ──

    #[test]
    fn contract_details_default() {
        let cd = ContractDetails::default();
        assert_eq!(cd.contract.con_id, 0);
        assert_eq!(cd.min_tick, 0.0);
    }

    // ibx#223: the raw cast wrapped -1 to 255 and forwarded out-of-range
    // trigger codes to the wire verbatim.
    #[test]
    fn attrs_trigger_method_coerces_invalid_codes() {
        for (input, expected) in [(-1, 0u8), (5, 0), (6, 0), (9, 0), (255, 0),
                                  (0, 0), (2, 2), (4, 4), (7, 7), (8, 8)] {
            let o = Order { trigger_method: input, ..Default::default() };
            assert_eq!(o.attrs().trigger_method, expected, "input {input}");
        }
    }

    // ibx#230: the reported Contract must round-trip — sec_type is the
    // official API string, and market_name is no longer thrown away.
    #[test]
    fn contract_details_from_definition_round_trips() {
        let def = crate::control::contracts::ContractDefinition {
            con_id: 265598,
            symbol: "AAPL".into(),
            sec_type: crate::control::contracts::SecurityType::Stock,
            market_name: "NMS".into(),
            ..Default::default()
        };
        let details = ContractDetails::from_definition(&def);
        assert_eq!(details.contract.sec_type, "STK", "not the Debug derive 'Stock'");
        assert_eq!(details.market_name, "NMS");
        // Unclassifiable instruments must not claim to be stocks.
        let def = crate::control::contracts::ContractDefinition {
            sec_type: crate::control::contracts::SecurityType::Other,
            ..Default::default()
        };
        assert_eq!(ContractDetails::from_definition(&def).contract.sec_type, "");
    }

    // ── ContractDescription ──

    #[test]
    fn contract_description_default() {
        let cd = ContractDescription::default();
        assert_eq!(cd.con_id, 0);
        assert_eq!(cd.symbol, "");
    }

    // ── CommissionAndFeesReport ──

    #[test]
    fn commission_and_fees_report_default() {
        let cr = CommissionAndFeesReport::default();
        assert_eq!(cr.exec_id, "");
        assert_eq!(cr.commission_and_fees, 0.0);
    }

    // ── PriceIncrement ──

    #[test]
    fn price_increment_fields() {
        let pi = PriceIncrement { low_edge: 0.0, increment: 0.01 };
        assert_eq!(pi.low_edge, 0.0);
        assert_eq!(pi.increment, 0.01);
    }

    /// `has_extended_attrs` decides whether an order routes through the encoder
    /// that emits the attribute block. Anything `attrs()` carries but this does
    /// not name is copied into `OrderAttrs` and then thrown away, with no error
    /// and nothing on the wire — so the two have to agree field for field.
    ///
    /// One entry per attribute `attrs()` carries. Adding a field there without
    /// adding it here is the bug this guards.
    #[test]
    fn every_carried_attribute_routes_through_the_extended_encoder() {
        /// Attribute name paired with the setter that turns it on.
        type Case = (&'static str, fn(&mut Order));

        let cases: Vec<Case> = vec![
            ("display_size", |o| o.display_size = 100),
            ("min_qty", |o| o.min_qty = 50),
            ("hidden", |o| o.hidden = true),
            ("outside_rth", |o| o.outside_rth = true),
            // Named by the predicate but deliberately not carried: `attrs()`
            // hardcodes `good_after` to 0 pending a wire capture (ibx#199), so
            // this entry pins the routing rather than an emitted tag.
            ("good_after_time", |o| o.good_after_time = "20260311 09:30:00".into()),
            ("good_till_date", |o| o.good_till_date = "20260311 16:00:00".into()),
            ("oca_group", |o| o.oca_group = "G1".into()),
            ("oca_type", |o| o.oca_type = 2),
            ("parent_id", |o| o.parent_id = 7),
            ("discretionary_amt", |o| o.discretionary_amt = 0.05),
            ("sweep_to_fill", |o| o.sweep_to_fill = true),
            ("all_or_none", |o| o.all_or_none = true),
            ("trigger_method", |o| o.trigger_method = 2),
            ("cash_qty", |o| o.cash_qty = 1000.0),
            ("conditions", |o| o.conditions.push(
                OrderCondition::Time { time: "20260311-09:30:00".into(), is_more: true },
            )),
            ("conditions_cancel_order", |o| o.conditions_cancel_order = true),
            ("conditions_ignore_rth", |o| o.conditions_ignore_rth = true),
            ("volatility", |o| o.volatility = 0.25),
            ("volatility_type", |o| o.volatility_type = 2),
            ("percent_offset", |o| o.percent_offset = 0.5),
            ("not_held", |o| o.not_held = true),
            ("order_ref", |o| o.order_ref = "ref-1".into()),
            ("open_close", |o| o.open_close = "O".into()),
            ("scale", |o| o.scale_init_level_size = 100),
            ("delta_neutral", |o| o.delta_neutral_order_type = "MKT".into()),
            ("short_sale_slot", |o| o.short_sale_slot = 2),
            ("designated_location", |o| o.designated_location = "IBKR".into()),
            ("exempt_code", |o| o.exempt_code = 3),
            ("hedge_type", |o| o.hedge_type = "B".into()),
            ("rule80a", |o| o.rule80a = "I".into()),
            ("post_to_ats", |o| o.post_to_ats = 30),
            ("deactivate", |o| o.deactivate = true),
            ("deactivate_on_disconnect", |o| o.deactivate_on_disconnect = true),
            ("include_overnight", |o| o.include_overnight = true),
            ("auto_cancel_parent", |o| o.auto_cancel_parent = true),
            ("min_trade_qty", |o| o.min_trade_qty = 50),
            ("block_order", |o| o.block_order = true),
            ("auto_cancel_date", |o| o.auto_cancel_date = "20261231".into()),
            ("clearing_account", |o| o.clearing_account = "U123".into()),
            ("clearing_intent", |o| o.clearing_intent = "IB".into()),
            ("seek_price_improvement", |o| o.seek_price_improvement = true),
            ("manual_order_time", |o| o.manual_order_time = "20260101-09:30:00".into()),
            ("advanced_error_override", |o| o.advanced_error_override = "1".into()),
            ("active_start_time", |o| o.active_start_time = "20260101-09:30:00".into()),
            ("active_stop_time", |o| o.active_stop_time = "20260101-16:00:00".into()),
            ("post_only", |o| o.post_only = true),
            ("solicited", |o| o.solicited = true),
            ("manual_order_indicator", |o| o.manual_order_indicator = 1),
            ("route_marketable_to_bbo", |o| o.route_marketable_to_bbo = true),
            ("imbalance_only", |o| o.imbalance_only = true),
            ("allow_pre_open", |o| o.allow_pre_open = true),
            ("ignore_open_auction", |o| o.ignore_open_auction = true),
            ("is_oms_container", |o| o.is_oms_container = true),
            ("ext_operator", |o| o.ext_operator = "OP1".into()),
            ("customer_account", |o| o.customer_account = "CUST".into()),
            ("professional_customer", |o| o.professional_customer = true),
            ("ref_futures_con_id", |o| o.ref_futures_con_id = 12345),
            ("mifid2_decision_maker", |o| o.mifid2_decision_maker = "DM".into()),
            ("mifid2_decision_algo", |o| o.mifid2_decision_algo = "DA".into()),
            ("mifid2_execution_trader", |o| o.mifid2_execution_trader = "ET".into()),
            ("mifid2_execution_algo", |o| o.mifid2_execution_algo = "EA".into()),
            ("mid_offset_at_whole", |o| o.mid_offset_at_whole = 0.01),
            ("mid_offset_at_half", |o| o.mid_offset_at_half = 0.005),
            ("use_price_mgmt_algo", |o| o.use_price_mgmt_algo = 1),
            ("duration", |o| o.duration = 60),
            ("min_compete_size", |o| o.min_compete_size = 100),
            ("compete_against_best_offset", |o| o.compete_against_best_offset = 0.02),
            ("continuous_update", |o| o.continuous_update = true),
            ("reference_price_type", |o| o.reference_price_type = 2),
            ("stock_range_lower", |o| o.stock_range_lower = 100.0),
            ("stock_range_upper", |o| o.stock_range_upper = 200.0),
        ];

        // Structural link to `attrs()`: destructured without `..`, so adding a
        // field to `OrderAttrs` stops compiling here until it is accounted for
        // both in the predicate and in the list above.
        let crate::types::OrderAttrs {
            display_size: _, min_qty: _, hidden: _, outside_rth: _, good_after: _,
            good_till: _, good_till_date_ymd: _, oca_group: _, oca_group_str: _,
            oca_type: _, parent_id: _, discretionary_amt: _, sweep_to_fill: _,
            all_or_none: _, trigger_method: _, cash_qty: _, conditions: _,
            conditions_cancel_order: _, conditions_ignore_rth: _,
            volatility: _, volatility_type: _, use_price_mgmt_algo: _, duration: _,
            seek_price_improvement: _, manual_order_time: _,
            advanced_error_override: _,
            active_start_time: _, active_stop_time: _, post_only: _, solicited: _,
            manual_order_indicator: _, route_marketable_to_bbo: _, imbalance_only: _,
            allow_pre_open: _, ignore_open_auction: _, is_oms_container: _,
            ext_operator: _, customer_account: _, professional_customer: _,
            ref_futures_con_id: _, mifid2_decision_maker: _, mifid2_decision_algo: _,
            mifid2_execution_trader: _, mifid2_execution_algo: _,
            mid_offset_at_whole: _, mid_offset_at_half: _,
            min_compete_size: _, compete_against_best_offset: _,
            continuous_update: _, reference_price_type: _,
            stock_range_lower: _, stock_range_upper: _,
            percent_offset: _, not_held: _, order_ref: _, open_close: _,
            scale: _, delta_neutral: _, short_sale_slot: _, designated_location: _,
            exempt_code: _, hedge_type: _, hedge_beta: _, hedge_ratio: _,
            combo_legs: _, rule80a: _, post_to_ats: _, deactivate: _,
            deactivate_on_disconnect: _,
            include_overnight: _, auto_cancel_parent: _, min_trade_qty: _,
            block_order: _, auto_cancel_date: _, clearing_account: _, clearing_intent: _,
            primary_exchange: _, delta_neutral_contract: _,
            // Reached by `exercise_options` rather than by an order, so there is
            // no setter to list above and nothing for the predicate to name.
            exercise_action: _,
        } = Order::default().attrs();

        assert!(
            !Order::default().has_extended_attrs(),
            "a default order carries nothing extended",
        );
        for (name, set) in cases {
            let mut order = Order::default();
            set(&mut order);
            assert!(
                order.has_extended_attrs(),
                "{name} is carried by attrs() but does not route through the extended encoder",
            );
        }
    }
}
