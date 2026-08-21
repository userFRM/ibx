//! The order classes a caller works in, as the Python API names them.

// The other families, and the two helpers every class here uses.
use super::{class_contracts::*, class_conditions::*};
use super::contract::{by_reference_name, set_from_keywords};
use pyo3::prelude::*;
use crate::types::*;

/// ibapi-compatible Order class.
#[pyclass(from_py_object)]
pub struct Order {
    // ── Original fields ──
    #[pyo3(get, set)]
    pub order_id: i64,
    #[pyo3(get, set)]
    pub action: String,
    #[pyo3(get, set)]
    pub total_quantity: f64,
    #[pyo3(get, set)]
    pub order_type: String,
    #[pyo3(get, set)]
    pub lmt_price: f64,
    #[pyo3(get, set)]
    pub aux_price: f64,
    #[pyo3(get, set)]
    pub tif: String,
    #[pyo3(get, set)]
    pub outside_rth: bool,
    #[pyo3(get, set)]
    pub display_size: i32,
    #[pyo3(get, set)]
    pub min_qty: i32,
    #[pyo3(get, set)]
    pub hidden: bool,
    #[pyo3(get, set)]
    pub good_after_time: String,
    #[pyo3(get, set)]
    pub good_till_date: String,
    #[pyo3(get, set)]
    pub oca_group: String,
    #[pyo3(get, set)]
    pub trailing_percent: f64,
    #[pyo3(get, set)]
    pub algo_strategy: String,
    #[pyo3(get, set)]
    pub algo_params: Vec<TagValue>,
    #[pyo3(get, set)]
    pub what_if: bool,
    #[pyo3(get, set)]
    pub cash_qty: f64,
    #[pyo3(get, set)]
    pub parent_id: i64,
    #[pyo3(get, set)]
    pub transmit: bool,
    #[pyo3(get, set)]
    pub discretionary_amt: f64,
    #[pyo3(get, set)]
    pub sweep_to_fill: bool,
    #[pyo3(get, set)]
    pub all_or_none: bool,
    #[pyo3(get, set)]
    pub trigger_method: i32,
    #[pyo3(get, set)]
    pub adjusted_order_type: String,
    #[pyo3(get, set)]
    pub trigger_price: f64,
    #[pyo3(get, set)]
    pub adjusted_stop_price: f64,
    #[pyo3(get, set)]
    pub adjusted_stop_limit_price: f64,
    #[pyo3(get, set)]
    pub conditions: Vec<Py<PyAny>>,
    #[pyo3(get, set)]
    pub conditions_ignore_rth: bool,
    #[pyo3(get, set)]
    pub conditions_cancel_order: bool,

    // ── New fields (ibapi ground truth) ──
    #[pyo3(get, set)]
    pub account: String,
    #[pyo3(get, set)]
    pub active_start_time: String,
    #[pyo3(get, set)]
    pub active_stop_time: String,
    #[pyo3(get, set)]
    pub adjustable_trailing_unit: i32,
    #[pyo3(get, set)]
    pub adjusted_trailing_amount: f64,
    #[pyo3(get, set)]
    pub advanced_error_override: String,
    #[pyo3(get, set)]
    pub algo_id: String,
    #[pyo3(get, set)]
    pub allow_pre_open: bool,
    #[pyo3(get, set)]
    pub auction_strategy: i32,
    #[pyo3(get, set)]
    pub auto_cancel_date: String,
    #[pyo3(get, set)]
    pub auto_cancel_parent: bool,
    #[pyo3(get, set)]
    pub basis_points: f64,
    #[pyo3(get, set)]
    pub basis_points_type: i32,
    #[pyo3(get, set)]
    pub block_order: bool,
    #[pyo3(get, set)]
    pub bond_accrued_interest: String,
    #[pyo3(get, set)]
    pub clearing_account: String,
    #[pyo3(get, set)]
    pub clearing_intent: String,
    #[pyo3(get, set)]
    pub client_id: i32,
    #[pyo3(get, set)]
    pub compete_against_best_offset: f64,
    #[pyo3(get, set)]
    pub continuous_update: bool,
    #[pyo3(get, set)]
    pub customer_account: String,
    #[pyo3(get, set)]
    pub deactivate: bool,
    /// Stand the order down if the connection goes, rather than leaving it
    /// working with nobody watching it.
    #[pyo3(get, set)]
    pub deactivate_on_disconnect: bool,
    #[pyo3(get, set)]
    pub delta: f64,
    #[pyo3(get, set)]
    pub delta_neutral_aux_price: f64,
    #[pyo3(get, set)]
    pub delta_neutral_clearing_account: String,
    #[pyo3(get, set)]
    pub delta_neutral_clearing_intent: String,
    #[pyo3(get, set)]
    pub delta_neutral_con_id: i32,
    #[pyo3(get, set)]
    pub delta_neutral_designated_location: String,
    #[pyo3(get, set)]
    pub delta_neutral_open_close: String,
    #[pyo3(get, set)]
    pub delta_neutral_order_type: String,
    #[pyo3(get, set)]
    pub delta_neutral_settling_firm: String,
    #[pyo3(get, set)]
    pub delta_neutral_short_sale: bool,
    #[pyo3(get, set)]
    pub delta_neutral_short_sale_slot: i32,
    #[pyo3(get, set)]
    pub designated_location: String,
    #[pyo3(get, set)]
    pub discretionary_up_to_limit_price: bool,
    #[pyo3(get, set)]
    pub dont_use_auto_price_for_hedge: bool,
    #[pyo3(get, set)]
    pub duration: i32,
    #[pyo3(get, set)]
    pub exempt_code: i32,
    #[pyo3(get, set)]
    pub ext_operator: String,
    #[pyo3(get, set)]
    pub fa_group: String,
    #[pyo3(get, set)]
    pub fa_method: String,
    #[pyo3(get, set)]
    pub fa_percentage: String,
    #[pyo3(get, set)]
    pub filled_quantity: f64,
    #[pyo3(get, set)]
    pub hedge_param: String,
    #[pyo3(get, set)]
    pub hedge_type: String,
    #[pyo3(get, set)]
    pub ignore_open_auction: bool,
    #[pyo3(get, set)]
    pub imbalance_only: bool,
    #[pyo3(get, set)]
    pub include_overnight: bool,
    #[pyo3(get, set)]
    pub is_oms_container: bool,
    #[pyo3(get, set)]
    pub is_pegged_change_amount_decrease: bool,
    #[pyo3(get, set)]
    pub lmt_price_offset: f64,
    #[pyo3(get, set)]
    pub manual_order_indicator: i32,
    #[pyo3(get, set)]
    pub manual_order_time: String,
    #[pyo3(get, set)]
    pub mid_offset_at_half: f64,
    #[pyo3(get, set)]
    pub mid_offset_at_whole: f64,
    #[pyo3(get, set)]
    pub mifid2_decision_algo: String,
    #[pyo3(get, set)]
    pub mifid2_decision_maker: String,
    #[pyo3(get, set)]
    pub mifid2_execution_algo: String,
    #[pyo3(get, set)]
    pub mifid2_execution_trader: String,
    #[pyo3(get, set)]
    pub min_compete_size: i32,
    #[pyo3(get, set)]
    pub min_trade_qty: i32,
    #[pyo3(get, set)]
    pub model_code: String,
    #[pyo3(get, set)]
    pub not_held: bool,
    #[pyo3(get, set)]
    pub oca_type: i32,
    #[pyo3(get, set)]
    pub open_close: String,
    #[pyo3(get, set)]
    pub opt_out_smart_routing: bool,
    #[pyo3(get, set)]
    pub order_combo_legs: Vec<Py<PyAny>>,
    #[pyo3(get, set)]
    pub order_misc_options: Vec<Py<PyAny>>,
    #[pyo3(get, set)]
    pub order_ref: String,
    #[pyo3(get, set)]
    pub origin: i32,
    #[pyo3(get, set)]
    pub override_percentage_constraints: bool,
    #[pyo3(get, set)]
    pub parent_perm_id: i64,
    #[pyo3(get, set)]
    pub pegged_change_amount: f64,
    #[pyo3(get, set)]
    pub percent_offset: f64,
    #[pyo3(get, set)]
    pub perm_id: i64,
    #[pyo3(get, set)]
    pub post_only: bool,
    #[pyo3(get, set)]
    pub post_to_ats: i32,
    #[pyo3(get, set)]
    pub professional_customer: bool,
    #[pyo3(get, set)]
    pub pt_order_id: i32,
    #[pyo3(get, set)]
    pub pt_order_type: String,
    #[pyo3(get, set)]
    pub randomize_price: bool,
    #[pyo3(get, set)]
    pub randomize_size: bool,
    #[pyo3(get, set)]
    pub ref_futures_con_id: i32,
    #[pyo3(get, set)]
    pub reference_change_amount: f64,
    #[pyo3(get, set)]
    pub reference_contract_id: i32,
    #[pyo3(get, set)]
    pub reference_exchange_id: String,
    #[pyo3(get, set)]
    pub reference_price_type: i32,
    #[pyo3(get, set)]
    pub route_marketable_to_bbo: bool,
    #[pyo3(get, set)]
    pub rule80a: String,
    #[pyo3(get, set)]
    pub scale_auto_reset: bool,
    #[pyo3(get, set)]
    pub scale_init_fill_qty: i32,
    #[pyo3(get, set)]
    pub scale_init_level_size: i32,
    #[pyo3(get, set)]
    pub scale_init_position: i32,
    #[pyo3(get, set)]
    pub scale_price_adjust_interval: i32,
    #[pyo3(get, set)]
    pub scale_price_adjust_value: f64,
    #[pyo3(get, set)]
    pub scale_price_increment: f64,
    #[pyo3(get, set)]
    pub scale_profit_offset: f64,
    #[pyo3(get, set)]
    pub scale_random_percent: bool,
    #[pyo3(get, set)]
    pub scale_subs_level_size: i32,
    #[pyo3(get, set)]
    pub scale_table: String,
    #[pyo3(get, set)]
    pub seek_price_improvement: bool,
    #[pyo3(get, set)]
    pub settling_firm: String,
    #[pyo3(get, set)]
    pub shareholder: String,
    #[pyo3(get, set)]
    pub short_sale_slot: i32,
    #[pyo3(get, set)]
    pub sl_order_id: i32,
    #[pyo3(get, set)]
    pub sl_order_type: String,
    #[pyo3(get, set)]
    pub smart_combo_routing_params: Vec<TagValue>,
    #[pyo3(get, set)]
    pub soft_dollar_tier_name: String,
    #[pyo3(get, set)]
    pub soft_dollar_tier_val: String,
    #[pyo3(get, set)]
    pub soft_dollar_tier_display_name: String,
    #[pyo3(get, set)]
    pub solicited: bool,
    #[pyo3(get, set)]
    pub starting_price: f64,
    #[pyo3(get, set)]
    pub stock_range_lower: f64,
    #[pyo3(get, set)]
    pub stock_range_upper: f64,
    #[pyo3(get, set)]
    pub stock_ref_price: f64,
    #[pyo3(get, set)]
    pub submitter: String,
    #[pyo3(get, set)]
    pub trail_stop_price: f64,
    #[pyo3(get, set)]
    pub use_price_mgmt_algo: i32,
    #[pyo3(get, set)]
    pub volatility: f64,
    #[pyo3(get, set)]
    pub volatility_type: i32,
    #[pyo3(get, set)]
    pub what_if_type: i32,
}

impl Clone for Order {
    fn clone(&self) -> Self {
        Self {
            // Original fields
            order_id: self.order_id,
            action: self.action.clone(),
            total_quantity: self.total_quantity,
            order_type: self.order_type.clone(),
            lmt_price: self.lmt_price,
            aux_price: self.aux_price,
            tif: self.tif.clone(),
            outside_rth: self.outside_rth,
            display_size: self.display_size,
            min_qty: self.min_qty,
            hidden: self.hidden,
            good_after_time: self.good_after_time.clone(),
            good_till_date: self.good_till_date.clone(),
            oca_group: self.oca_group.clone(),
            trailing_percent: self.trailing_percent,
            algo_strategy: self.algo_strategy.clone(),
            algo_params: self.algo_params.clone(),
            what_if: self.what_if,
            cash_qty: self.cash_qty,
            parent_id: self.parent_id,
            transmit: self.transmit,
            discretionary_amt: self.discretionary_amt,
            sweep_to_fill: self.sweep_to_fill,
            all_or_none: self.all_or_none,
            trigger_method: self.trigger_method,
            adjusted_order_type: self.adjusted_order_type.clone(),
            trigger_price: self.trigger_price,
            adjusted_stop_price: self.adjusted_stop_price,
            adjusted_stop_limit_price: self.adjusted_stop_limit_price,
            conditions: Vec::new(),
            conditions_ignore_rth: self.conditions_ignore_rth,
            conditions_cancel_order: self.conditions_cancel_order,
            // New fields
            account: self.account.clone(),
            active_start_time: self.active_start_time.clone(),
            active_stop_time: self.active_stop_time.clone(),
            adjustable_trailing_unit: self.adjustable_trailing_unit,
            adjusted_trailing_amount: self.adjusted_trailing_amount,
            advanced_error_override: self.advanced_error_override.clone(),
            algo_id: self.algo_id.clone(),
            allow_pre_open: self.allow_pre_open,
            auction_strategy: self.auction_strategy,
            auto_cancel_date: self.auto_cancel_date.clone(),
            auto_cancel_parent: self.auto_cancel_parent,
            basis_points: self.basis_points,
            basis_points_type: self.basis_points_type,
            block_order: self.block_order,
            bond_accrued_interest: self.bond_accrued_interest.clone(),
            clearing_account: self.clearing_account.clone(),
            clearing_intent: self.clearing_intent.clone(),
            client_id: self.client_id,
            compete_against_best_offset: self.compete_against_best_offset,
            continuous_update: self.continuous_update,
            customer_account: self.customer_account.clone(),
            deactivate: self.deactivate,
            deactivate_on_disconnect: self.deactivate_on_disconnect,
            delta: self.delta,
            delta_neutral_aux_price: self.delta_neutral_aux_price,
            delta_neutral_clearing_account: self.delta_neutral_clearing_account.clone(),
            delta_neutral_clearing_intent: self.delta_neutral_clearing_intent.clone(),
            delta_neutral_con_id: self.delta_neutral_con_id,
            delta_neutral_designated_location: self.delta_neutral_designated_location.clone(),
            delta_neutral_open_close: self.delta_neutral_open_close.clone(),
            delta_neutral_order_type: self.delta_neutral_order_type.clone(),
            delta_neutral_settling_firm: self.delta_neutral_settling_firm.clone(),
            delta_neutral_short_sale: self.delta_neutral_short_sale,
            delta_neutral_short_sale_slot: self.delta_neutral_short_sale_slot,
            designated_location: self.designated_location.clone(),
            discretionary_up_to_limit_price: self.discretionary_up_to_limit_price,
            dont_use_auto_price_for_hedge: self.dont_use_auto_price_for_hedge,
            duration: self.duration,
            exempt_code: self.exempt_code,
            ext_operator: self.ext_operator.clone(),
            fa_group: self.fa_group.clone(),
            fa_method: self.fa_method.clone(),
            fa_percentage: self.fa_percentage.clone(),
            filled_quantity: self.filled_quantity,
            hedge_param: self.hedge_param.clone(),
            hedge_type: self.hedge_type.clone(),
            ignore_open_auction: self.ignore_open_auction,
            imbalance_only: self.imbalance_only,
            include_overnight: self.include_overnight,
            is_oms_container: self.is_oms_container,
            is_pegged_change_amount_decrease: self.is_pegged_change_amount_decrease,
            lmt_price_offset: self.lmt_price_offset,
            manual_order_indicator: self.manual_order_indicator,
            manual_order_time: self.manual_order_time.clone(),
            mid_offset_at_half: self.mid_offset_at_half,
            mid_offset_at_whole: self.mid_offset_at_whole,
            mifid2_decision_algo: self.mifid2_decision_algo.clone(),
            mifid2_decision_maker: self.mifid2_decision_maker.clone(),
            mifid2_execution_algo: self.mifid2_execution_algo.clone(),
            mifid2_execution_trader: self.mifid2_execution_trader.clone(),
            min_compete_size: self.min_compete_size,
            min_trade_qty: self.min_trade_qty,
            model_code: self.model_code.clone(),
            not_held: self.not_held,
            oca_type: self.oca_type,
            open_close: self.open_close.clone(),
            opt_out_smart_routing: self.opt_out_smart_routing,
            order_combo_legs: Vec::new(),
            order_misc_options: Vec::new(),
            order_ref: self.order_ref.clone(),
            origin: self.origin,
            override_percentage_constraints: self.override_percentage_constraints,
            parent_perm_id: self.parent_perm_id,
            pegged_change_amount: self.pegged_change_amount,
            percent_offset: self.percent_offset,
            perm_id: self.perm_id,
            post_only: self.post_only,
            post_to_ats: self.post_to_ats,
            professional_customer: self.professional_customer,
            pt_order_id: self.pt_order_id,
            pt_order_type: self.pt_order_type.clone(),
            randomize_price: self.randomize_price,
            randomize_size: self.randomize_size,
            ref_futures_con_id: self.ref_futures_con_id,
            reference_change_amount: self.reference_change_amount,
            reference_contract_id: self.reference_contract_id,
            reference_exchange_id: self.reference_exchange_id.clone(),
            reference_price_type: self.reference_price_type,
            route_marketable_to_bbo: self.route_marketable_to_bbo,
            rule80a: self.rule80a.clone(),
            scale_auto_reset: self.scale_auto_reset,
            scale_init_fill_qty: self.scale_init_fill_qty,
            scale_init_level_size: self.scale_init_level_size,
            scale_init_position: self.scale_init_position,
            scale_price_adjust_interval: self.scale_price_adjust_interval,
            scale_price_adjust_value: self.scale_price_adjust_value,
            scale_price_increment: self.scale_price_increment,
            scale_profit_offset: self.scale_profit_offset,
            scale_random_percent: self.scale_random_percent,
            scale_subs_level_size: self.scale_subs_level_size,
            scale_table: self.scale_table.clone(),
            seek_price_improvement: self.seek_price_improvement,
            settling_firm: self.settling_firm.clone(),
            shareholder: self.shareholder.clone(),
            short_sale_slot: self.short_sale_slot,
            sl_order_id: self.sl_order_id,
            sl_order_type: self.sl_order_type.clone(),
            smart_combo_routing_params: self.smart_combo_routing_params.clone(),
            soft_dollar_tier_name: self.soft_dollar_tier_name.clone(),
            soft_dollar_tier_val: self.soft_dollar_tier_val.clone(),
            soft_dollar_tier_display_name: self.soft_dollar_tier_display_name.clone(),
            solicited: self.solicited,
            starting_price: self.starting_price,
            stock_range_lower: self.stock_range_lower,
            stock_range_upper: self.stock_range_upper,
            stock_ref_price: self.stock_ref_price,
            submitter: self.submitter.clone(),
            trail_stop_price: self.trail_stop_price,
            use_price_mgmt_algo: self.use_price_mgmt_algo,
            volatility: self.volatility,
            volatility_type: self.volatility_type,
            what_if_type: self.what_if_type,
        }
    }
}

impl Default for Order {
    fn default() -> Self {
        Self {
            // Original fields
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
            // New fields
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
            dont_use_auto_price_for_hedge: false,
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

#[pymethods]
impl Order {
    #[new]
    #[pyo3(signature = (
        order_id=0, action="".to_string(), total_quantity=0.0, order_type="".to_string(),
        lmt_price=0.0, aux_price=0.0, tif="DAY".to_string(), outside_rth=false,
        display_size=0, min_qty=0, hidden=false, good_after_time="".to_string(),
        good_till_date="".to_string(), oca_group="".to_string(), trailing_percent=0.0,
        algo_strategy="".to_string(), what_if=false, cash_qty=0.0, parent_id=0,
        transmit=true, **keywords
    ))]
    fn new(
        order_id: i64,
        action: String,
        total_quantity: f64,
        order_type: String,
        lmt_price: f64,
        aux_price: f64,
        tif: String,
        outside_rth: bool,
        display_size: i32,
        min_qty: i32,
        hidden: bool,
        good_after_time: String,
        good_till_date: String,
        oca_group: String,
        trailing_percent: f64,
        algo_strategy: String,
        what_if: bool,
        cash_qty: f64,
        parent_id: i64,
        transmit: bool,
            keywords: Option<&Bound<'_, pyo3::types::PyDict>>,
        py: Python<'_>,
    ) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self {
            order_id,
            action,
            total_quantity,
            order_type,
            lmt_price,
            aux_price,
            tif,
            outside_rth,
            display_size,
            min_qty,
            hidden,
            good_after_time,
            good_till_date,
            oca_group,
            trailing_percent,
            algo_strategy,
            algo_params: Vec::new(),
            what_if,
            cash_qty,
            parent_id,
            transmit,
            ..Default::default()
        })?;
        // And whatever else the caller named, under either spelling.
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        format!("Order(orderId={}, action='{}', totalQuantity={}, orderType='{}', lmtPrice={}, auxPrice={})",
            self.order_id, self.action, self.total_quantity, self.order_type, self.lmt_price, self.aux_price)
    }

    // ── Existing camelCase aliases ──
    #[getter(auxPrice)]
    fn get_aux_price_alias(&self) -> f64 { self.aux_price }
    #[setter(auxPrice)]
    fn set_aux_price_alias(&mut self, v: f64) { self.aux_price = v; }
    #[getter(lmtPrice)]
    fn get_lmt_price_alias(&self) -> f64 { self.lmt_price }
    #[setter(lmtPrice)]
    fn set_lmt_price_alias(&mut self, v: f64) { self.lmt_price = v; }
    #[getter(orderId)]
    fn get_order_id_alias(&self) -> i64 { self.order_id }
    #[setter(orderId)]
    fn set_order_id_alias(&mut self, v: i64) { self.order_id = v; }
    #[getter(totalQuantity)]
    fn get_total_quantity_alias(&self) -> f64 { self.total_quantity }
    #[setter(totalQuantity)]
    fn set_total_quantity_alias(&mut self, v: f64) { self.total_quantity = v; }
    #[getter(orderType)]
    fn get_order_type_alias(&self) -> String { self.order_type.clone() }
    #[setter(orderType)]
    fn set_order_type_alias(&mut self, v: String) { self.order_type = v; }

    // ── New camelCase aliases ──
    #[getter(activeStartTime)]
    fn get_active_start_time_alias(&self) -> String { self.active_start_time.clone() }
    #[setter(activeStartTime)]
    fn set_active_start_time_alias(&mut self, v: String) { self.active_start_time = v; }
    #[getter(activeStopTime)]
    fn get_active_stop_time_alias(&self) -> String { self.active_stop_time.clone() }
    #[setter(activeStopTime)]
    fn set_active_stop_time_alias(&mut self, v: String) { self.active_stop_time = v; }
    #[getter(adjustableTrailingUnit)]
    fn get_adjustable_trailing_unit_alias(&self) -> i32 { self.adjustable_trailing_unit }
    #[setter(adjustableTrailingUnit)]
    fn set_adjustable_trailing_unit_alias(&mut self, v: i32) { self.adjustable_trailing_unit = v; }
    #[getter(adjustedTrailingAmount)]
    fn get_adjusted_trailing_amount_alias(&self) -> f64 { self.adjusted_trailing_amount }
    #[setter(adjustedTrailingAmount)]
    fn set_adjusted_trailing_amount_alias(&mut self, v: f64) { self.adjusted_trailing_amount = v; }
    #[getter(adjustedOrderType)]
    fn get_adjusted_order_type_alias(&self) -> String { self.adjusted_order_type.clone() }
    #[setter(adjustedOrderType)]
    fn set_adjusted_order_type_alias(&mut self, v: String) { self.adjusted_order_type = v; }
    #[getter(adjustedStopPrice)]
    fn get_adjusted_stop_price_alias(&self) -> f64 { self.adjusted_stop_price }
    #[setter(adjustedStopPrice)]
    fn set_adjusted_stop_price_alias(&mut self, v: f64) { self.adjusted_stop_price = v; }
    #[getter(adjustedStopLimitPrice)]
    fn get_adjusted_stop_limit_price_alias(&self) -> f64 { self.adjusted_stop_limit_price }
    #[setter(adjustedStopLimitPrice)]
    fn set_adjusted_stop_limit_price_alias(&mut self, v: f64) { self.adjusted_stop_limit_price = v; }
    #[getter(advancedErrorOverride)]
    fn get_advanced_error_override_alias(&self) -> String { self.advanced_error_override.clone() }
    #[setter(advancedErrorOverride)]
    fn set_advanced_error_override_alias(&mut self, v: String) { self.advanced_error_override = v; }
    #[getter(algoId)]
    fn get_algo_id_alias(&self) -> String { self.algo_id.clone() }
    #[setter(algoId)]
    fn set_algo_id_alias(&mut self, v: String) { self.algo_id = v; }
    #[getter(algoParams)]
    fn get_algo_params_alias(&self) -> Vec<TagValue> { self.algo_params.clone() }
    // Writable as well as readable. Readable only, parameters set under the
    // reference client's name for them do not reach the order, and it goes out
    // on the venue's default settings for that algo.
    #[setter(algoParams)]
    fn set_algo_params_alias(&mut self, v: Vec<TagValue>) { self.algo_params = v; }
    #[getter(algoStrategy)]
    fn get_algo_strategy_alias(&self) -> String { self.algo_strategy.clone() }
    #[setter(algoStrategy)]
    fn set_algo_strategy_alias(&mut self, v: String) { self.algo_strategy = v; }
    #[getter(allOrNone)]
    fn get_all_or_none_alias(&self) -> bool { self.all_or_none }
    #[setter(allOrNone)]
    fn set_all_or_none_alias(&mut self, v: bool) { self.all_or_none = v; }
    #[getter(allowPreOpen)]
    fn get_allow_pre_open_alias(&self) -> bool { self.allow_pre_open }
    #[setter(allowPreOpen)]
    fn set_allow_pre_open_alias(&mut self, v: bool) { self.allow_pre_open = v; }
    #[getter(auctionStrategy)]
    fn get_auction_strategy_alias(&self) -> i32 { self.auction_strategy }
    #[setter(auctionStrategy)]
    fn set_auction_strategy_alias(&mut self, v: i32) { self.auction_strategy = v; }
    #[getter(autoCancelDate)]
    fn get_auto_cancel_date_alias(&self) -> String { self.auto_cancel_date.clone() }
    #[setter(autoCancelDate)]
    fn set_auto_cancel_date_alias(&mut self, v: String) { self.auto_cancel_date = v; }
    #[getter(autoCancelParent)]
    fn get_auto_cancel_parent_alias(&self) -> bool { self.auto_cancel_parent }
    #[setter(autoCancelParent)]
    fn set_auto_cancel_parent_alias(&mut self, v: bool) { self.auto_cancel_parent = v; }
    #[getter(basisPoints)]
    fn get_basis_points_alias(&self) -> f64 { self.basis_points }
    #[setter(basisPoints)]
    fn set_basis_points_alias(&mut self, v: f64) { self.basis_points = v; }
    #[getter(basisPointsType)]
    fn get_basis_points_type_alias(&self) -> i32 { self.basis_points_type }
    #[setter(basisPointsType)]
    fn set_basis_points_type_alias(&mut self, v: i32) { self.basis_points_type = v; }
    #[getter(blockOrder)]
    fn get_block_order_alias(&self) -> bool { self.block_order }
    #[setter(blockOrder)]
    fn set_block_order_alias(&mut self, v: bool) { self.block_order = v; }
    #[getter(bondAccruedInterest)]
    fn get_bond_accrued_interest_alias(&self) -> String { self.bond_accrued_interest.clone() }
    #[setter(bondAccruedInterest)]
    fn set_bond_accrued_interest_alias(&mut self, v: String) { self.bond_accrued_interest = v; }
    #[getter(cashQty)]
    fn get_cash_qty_alias(&self) -> f64 { self.cash_qty }
    #[setter(cashQty)]
    fn set_cash_qty_alias(&mut self, v: f64) { self.cash_qty = v; }
    #[getter(clearingAccount)]
    fn get_clearing_account_alias(&self) -> String { self.clearing_account.clone() }
    #[setter(clearingAccount)]
    fn set_clearing_account_alias(&mut self, v: String) { self.clearing_account = v; }
    #[getter(clearingIntent)]
    fn get_clearing_intent_alias(&self) -> String { self.clearing_intent.clone() }
    #[setter(clearingIntent)]
    fn set_clearing_intent_alias(&mut self, v: String) { self.clearing_intent = v; }
    #[getter(clientId)]
    fn get_client_id_alias(&self) -> i32 { self.client_id }
    #[setter(clientId)]
    fn set_client_id_alias(&mut self, v: i32) { self.client_id = v; }
    #[getter(competeAgainstBestOffset)]
    fn get_compete_against_best_offset_alias(&self) -> f64 { self.compete_against_best_offset }
    #[setter(competeAgainstBestOffset)]
    fn set_compete_against_best_offset_alias(&mut self, v: f64) { self.compete_against_best_offset = v; }
    #[getter(conditionsCancelOrder)]
    fn get_conditions_cancel_order_alias(&self) -> bool { self.conditions_cancel_order }
    #[setter(conditionsCancelOrder)]
    fn set_conditions_cancel_order_alias(&mut self, v: bool) { self.conditions_cancel_order = v; }
    #[getter(conditionsIgnoreRth)]
    fn get_conditions_ignore_rth_alias(&self) -> bool { self.conditions_ignore_rth }
    #[setter(conditionsIgnoreRth)]
    fn set_conditions_ignore_rth_alias(&mut self, v: bool) { self.conditions_ignore_rth = v; }
    #[getter(continuousUpdate)]
    fn get_continuous_update_alias(&self) -> bool { self.continuous_update }
    #[setter(continuousUpdate)]
    fn set_continuous_update_alias(&mut self, v: bool) { self.continuous_update = v; }
    #[getter(customerAccount)]
    fn get_customer_account_alias(&self) -> String { self.customer_account.clone() }
    #[setter(customerAccount)]
    fn set_customer_account_alias(&mut self, v: String) { self.customer_account = v; }
    #[getter(deltaNeutralAuxPrice)]
    fn get_delta_neutral_aux_price_alias(&self) -> f64 { self.delta_neutral_aux_price }
    #[setter(deltaNeutralAuxPrice)]
    fn set_delta_neutral_aux_price_alias(&mut self, v: f64) { self.delta_neutral_aux_price = v; }
    #[getter(deltaNeutralClearingAccount)]
    fn get_delta_neutral_clearing_account_alias(&self) -> String { self.delta_neutral_clearing_account.clone() }
    #[setter(deltaNeutralClearingAccount)]
    fn set_delta_neutral_clearing_account_alias(&mut self, v: String) { self.delta_neutral_clearing_account = v; }
    #[getter(deltaNeutralClearingIntent)]
    fn get_delta_neutral_clearing_intent_alias(&self) -> String { self.delta_neutral_clearing_intent.clone() }
    #[setter(deltaNeutralClearingIntent)]
    fn set_delta_neutral_clearing_intent_alias(&mut self, v: String) { self.delta_neutral_clearing_intent = v; }
    #[getter(deltaNeutralConId)]
    fn get_delta_neutral_con_id_alias(&self) -> i32 { self.delta_neutral_con_id }
    #[setter(deltaNeutralConId)]
    fn set_delta_neutral_con_id_alias(&mut self, v: i32) { self.delta_neutral_con_id = v; }
    #[getter(deltaNeutralDesignatedLocation)]
    fn get_delta_neutral_designated_location_alias(&self) -> String { self.delta_neutral_designated_location.clone() }
    #[setter(deltaNeutralDesignatedLocation)]
    fn set_delta_neutral_designated_location_alias(&mut self, v: String) { self.delta_neutral_designated_location = v; }
    #[getter(deltaNeutralOpenClose)]
    fn get_delta_neutral_open_close_alias(&self) -> String { self.delta_neutral_open_close.clone() }
    #[setter(deltaNeutralOpenClose)]
    fn set_delta_neutral_open_close_alias(&mut self, v: String) { self.delta_neutral_open_close = v; }
    #[getter(deltaNeutralOrderType)]
    fn get_delta_neutral_order_type_alias(&self) -> String { self.delta_neutral_order_type.clone() }
    #[setter(deltaNeutralOrderType)]
    fn set_delta_neutral_order_type_alias(&mut self, v: String) { self.delta_neutral_order_type = v; }
    #[getter(deltaNeutralSettlingFirm)]
    fn get_delta_neutral_settling_firm_alias(&self) -> String { self.delta_neutral_settling_firm.clone() }
    #[setter(deltaNeutralSettlingFirm)]
    fn set_delta_neutral_settling_firm_alias(&mut self, v: String) { self.delta_neutral_settling_firm = v; }
    #[getter(deltaNeutralShortSale)]
    fn get_delta_neutral_short_sale_alias(&self) -> bool { self.delta_neutral_short_sale }
    #[setter(deltaNeutralShortSale)]
    fn set_delta_neutral_short_sale_alias(&mut self, v: bool) { self.delta_neutral_short_sale = v; }
    #[getter(deltaNeutralShortSaleSlot)]
    fn get_delta_neutral_short_sale_slot_alias(&self) -> i32 { self.delta_neutral_short_sale_slot }
    #[setter(deltaNeutralShortSaleSlot)]
    fn set_delta_neutral_short_sale_slot_alias(&mut self, v: i32) { self.delta_neutral_short_sale_slot = v; }
    #[getter(designatedLocation)]
    fn get_designated_location_alias(&self) -> String { self.designated_location.clone() }
    #[setter(designatedLocation)]
    fn set_designated_location_alias(&mut self, v: String) { self.designated_location = v; }
    #[getter(discretionaryAmt)]
    fn get_discretionary_amt_alias(&self) -> f64 { self.discretionary_amt }
    #[setter(discretionaryAmt)]
    fn set_discretionary_amt_alias(&mut self, v: f64) { self.discretionary_amt = v; }
    #[getter(discretionaryUpToLimitPrice)]
    fn get_discretionary_up_to_limit_price_alias(&self) -> bool { self.discretionary_up_to_limit_price }
    #[setter(discretionaryUpToLimitPrice)]
    fn set_discretionary_up_to_limit_price_alias(&mut self, v: bool) { self.discretionary_up_to_limit_price = v; }
    #[getter(displaySize)]
    fn get_display_size_alias(&self) -> i32 { self.display_size }
    #[setter(displaySize)]
    fn set_display_size_alias(&mut self, v: i32) { self.display_size = v; }
    #[getter(dontUseAutoPriceForHedge)]
    fn get_dont_use_auto_price_for_hedge_alias(&self) -> bool { self.dont_use_auto_price_for_hedge }
    #[setter(dontUseAutoPriceForHedge)]
    fn set_dont_use_auto_price_for_hedge_alias(&mut self, v: bool) { self.dont_use_auto_price_for_hedge = v; }
    #[getter(exemptCode)]
    fn get_exempt_code_alias(&self) -> i32 { self.exempt_code }
    #[setter(exemptCode)]
    fn set_exempt_code_alias(&mut self, v: i32) { self.exempt_code = v; }
    #[getter(extOperator)]
    fn get_ext_operator_alias(&self) -> String { self.ext_operator.clone() }
    #[setter(extOperator)]
    fn set_ext_operator_alias(&mut self, v: String) { self.ext_operator = v; }
    #[getter(faGroup)]
    fn get_fa_group_alias(&self) -> String { self.fa_group.clone() }
    #[setter(faGroup)]
    fn set_fa_group_alias(&mut self, v: String) { self.fa_group = v; }
    #[getter(faMethod)]
    fn get_fa_method_alias(&self) -> String { self.fa_method.clone() }
    #[setter(faMethod)]
    fn set_fa_method_alias(&mut self, v: String) { self.fa_method = v; }
    #[getter(faPercentage)]
    fn get_fa_percentage_alias(&self) -> String { self.fa_percentage.clone() }
    #[setter(faPercentage)]
    fn set_fa_percentage_alias(&mut self, v: String) { self.fa_percentage = v; }
    #[getter(filledQuantity)]
    fn get_filled_quantity_alias(&self) -> f64 { self.filled_quantity }
    #[setter(filledQuantity)]
    fn set_filled_quantity_alias(&mut self, v: f64) { self.filled_quantity = v; }
    #[getter(goodAfterTime)]
    fn get_good_after_time_alias(&self) -> String { self.good_after_time.clone() }
    #[setter(goodAfterTime)]
    fn set_good_after_time_alias(&mut self, v: String) { self.good_after_time = v; }
    #[getter(goodTillDate)]
    fn get_good_till_date_alias(&self) -> String { self.good_till_date.clone() }
    #[setter(goodTillDate)]
    fn set_good_till_date_alias(&mut self, v: String) { self.good_till_date = v; }
    #[getter(hedgeParam)]
    fn get_hedge_param_alias(&self) -> String { self.hedge_param.clone() }
    #[setter(hedgeParam)]
    fn set_hedge_param_alias(&mut self, v: String) { self.hedge_param = v; }
    #[getter(hedgeType)]
    fn get_hedge_type_alias(&self) -> String { self.hedge_type.clone() }
    #[setter(hedgeType)]
    fn set_hedge_type_alias(&mut self, v: String) { self.hedge_type = v; }
    #[getter(ignoreOpenAuction)]
    fn get_ignore_open_auction_alias(&self) -> bool { self.ignore_open_auction }
    #[setter(ignoreOpenAuction)]
    fn set_ignore_open_auction_alias(&mut self, v: bool) { self.ignore_open_auction = v; }
    #[getter(imbalanceOnly)]
    fn get_imbalance_only_alias(&self) -> bool { self.imbalance_only }
    #[setter(imbalanceOnly)]
    fn set_imbalance_only_alias(&mut self, v: bool) { self.imbalance_only = v; }
    #[getter(includeOvernight)]
    fn get_include_overnight_alias(&self) -> bool { self.include_overnight }
    #[setter(includeOvernight)]
    fn set_include_overnight_alias(&mut self, v: bool) { self.include_overnight = v; }
    #[getter(isOmsContainer)]
    fn get_is_oms_container_alias(&self) -> bool { self.is_oms_container }
    #[setter(isOmsContainer)]
    fn set_is_oms_container_alias(&mut self, v: bool) { self.is_oms_container = v; }
    #[getter(isPeggedChangeAmountDecrease)]
    fn get_is_pegged_change_amount_decrease_alias(&self) -> bool { self.is_pegged_change_amount_decrease }
    #[setter(isPeggedChangeAmountDecrease)]
    fn set_is_pegged_change_amount_decrease_alias(&mut self, v: bool) { self.is_pegged_change_amount_decrease = v; }
    #[getter(lmtPriceOffset)]
    fn get_lmt_price_offset_alias(&self) -> f64 { self.lmt_price_offset }
    #[setter(lmtPriceOffset)]
    fn set_lmt_price_offset_alias(&mut self, v: f64) { self.lmt_price_offset = v; }
    #[getter(manualOrderIndicator)]
    fn get_manual_order_indicator_alias(&self) -> i32 { self.manual_order_indicator }
    #[setter(manualOrderIndicator)]
    fn set_manual_order_indicator_alias(&mut self, v: i32) { self.manual_order_indicator = v; }
    #[getter(manualOrderTime)]
    fn get_manual_order_time_alias(&self) -> String { self.manual_order_time.clone() }
    #[setter(manualOrderTime)]
    fn set_manual_order_time_alias(&mut self, v: String) { self.manual_order_time = v; }
    #[getter(midOffsetAtHalf)]
    fn get_mid_offset_at_half_alias(&self) -> f64 { self.mid_offset_at_half }
    #[setter(midOffsetAtHalf)]
    fn set_mid_offset_at_half_alias(&mut self, v: f64) { self.mid_offset_at_half = v; }
    #[getter(midOffsetAtWhole)]
    fn get_mid_offset_at_whole_alias(&self) -> f64 { self.mid_offset_at_whole }
    #[setter(midOffsetAtWhole)]
    fn set_mid_offset_at_whole_alias(&mut self, v: f64) { self.mid_offset_at_whole = v; }
    #[getter(mifid2DecisionAlgo)]
    fn get_mifid2_decision_algo_alias(&self) -> String { self.mifid2_decision_algo.clone() }
    #[setter(mifid2DecisionAlgo)]
    fn set_mifid2_decision_algo_alias(&mut self, v: String) { self.mifid2_decision_algo = v; }
    #[getter(mifid2DecisionMaker)]
    fn get_mifid2_decision_maker_alias(&self) -> String { self.mifid2_decision_maker.clone() }
    #[setter(mifid2DecisionMaker)]
    fn set_mifid2_decision_maker_alias(&mut self, v: String) { self.mifid2_decision_maker = v; }
    #[getter(mifid2ExecutionAlgo)]
    fn get_mifid2_execution_algo_alias(&self) -> String { self.mifid2_execution_algo.clone() }
    #[setter(mifid2ExecutionAlgo)]
    fn set_mifid2_execution_algo_alias(&mut self, v: String) { self.mifid2_execution_algo = v; }
    #[getter(mifid2ExecutionTrader)]
    fn get_mifid2_execution_trader_alias(&self) -> String { self.mifid2_execution_trader.clone() }
    #[setter(mifid2ExecutionTrader)]
    fn set_mifid2_execution_trader_alias(&mut self, v: String) { self.mifid2_execution_trader = v; }
    #[getter(minCompeteSize)]
    fn get_min_compete_size_alias(&self) -> i32 { self.min_compete_size }
    #[setter(minCompeteSize)]
    fn set_min_compete_size_alias(&mut self, v: i32) { self.min_compete_size = v; }
    #[getter(minQty)]
    fn get_min_qty_alias(&self) -> i32 { self.min_qty }
    #[setter(minQty)]
    fn set_min_qty_alias(&mut self, v: i32) { self.min_qty = v; }
    #[getter(minTradeQty)]
    fn get_min_trade_qty_alias(&self) -> i32 { self.min_trade_qty }
    #[setter(minTradeQty)]
    fn set_min_trade_qty_alias(&mut self, v: i32) { self.min_trade_qty = v; }
    #[getter(modelCode)]
    fn get_model_code_alias(&self) -> String { self.model_code.clone() }
    #[setter(modelCode)]
    fn set_model_code_alias(&mut self, v: String) { self.model_code = v; }
    #[getter(notHeld)]
    fn get_not_held_alias(&self) -> bool { self.not_held }
    #[setter(notHeld)]
    fn set_not_held_alias(&mut self, v: bool) { self.not_held = v; }
    #[getter(ocaGroup)]
    fn get_oca_group_alias(&self) -> String { self.oca_group.clone() }
    #[setter(ocaGroup)]
    fn set_oca_group_alias(&mut self, v: String) { self.oca_group = v; }
    #[getter(ocaType)]
    fn get_oca_type_alias(&self) -> i32 { self.oca_type }
    #[setter(ocaType)]
    fn set_oca_type_alias(&mut self, v: i32) { self.oca_type = v; }
    #[getter(openClose)]
    fn get_open_close_alias(&self) -> String { self.open_close.clone() }
    #[setter(openClose)]
    fn set_open_close_alias(&mut self, v: String) { self.open_close = v; }
    #[getter(optOutSmartRouting)]
    fn get_opt_out_smart_routing_alias(&self) -> bool { self.opt_out_smart_routing }
    #[setter(optOutSmartRouting)]
    fn set_opt_out_smart_routing_alias(&mut self, v: bool) { self.opt_out_smart_routing = v; }
    // What the order holds, rather than an empty list whatever it holds: read
    // by the name the reference client uses, a combination priced per leg
    // reported no legs at all, and the same for the miscellaneous options.
    #[getter(orderComboLegs)]
    fn get_order_combo_legs_alias(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.order_combo_legs.iter().map(|l| l.clone_ref(py)).collect()
    }
    #[setter(orderComboLegs)]
    fn set_order_combo_legs_alias(&mut self, v: Vec<Py<PyAny>>) { self.order_combo_legs = v; }
    #[getter(orderMiscOptions)]
    fn get_order_misc_options_alias(&self, py: Python<'_>) -> Vec<Py<PyAny>> {
        self.order_misc_options.iter().map(|o| o.clone_ref(py)).collect()
    }
    #[setter(orderMiscOptions)]
    fn set_order_misc_options_alias(&mut self, v: Vec<Py<PyAny>>) { self.order_misc_options = v; }
    #[getter(orderRef)]
    fn get_order_ref_alias(&self) -> String { self.order_ref.clone() }
    #[setter(orderRef)]
    fn set_order_ref_alias(&mut self, v: String) { self.order_ref = v; }
    #[getter(outsideRth)]
    fn get_outside_rth_alias(&self) -> bool { self.outside_rth }
    #[setter(outsideRth)]
    fn set_outside_rth_alias(&mut self, v: bool) { self.outside_rth = v; }
    #[getter(overridePercentageConstraints)]
    fn get_override_percentage_constraints_alias(&self) -> bool { self.override_percentage_constraints }
    #[setter(overridePercentageConstraints)]
    fn set_override_percentage_constraints_alias(&mut self, v: bool) { self.override_percentage_constraints = v; }
    #[getter(parentId)]
    fn get_parent_id_alias(&self) -> i64 { self.parent_id }
    #[setter(parentId)]
    fn set_parent_id_alias(&mut self, v: i64) { self.parent_id = v; }
    #[getter(parentPermId)]
    fn get_parent_perm_id_alias(&self) -> i64 { self.parent_perm_id }
    #[setter(parentPermId)]
    fn set_parent_perm_id_alias(&mut self, v: i64) { self.parent_perm_id = v; }
    #[getter(peggedChangeAmount)]
    fn get_pegged_change_amount_alias(&self) -> f64 { self.pegged_change_amount }
    #[setter(peggedChangeAmount)]
    fn set_pegged_change_amount_alias(&mut self, v: f64) { self.pegged_change_amount = v; }
    #[getter(percentOffset)]
    fn get_percent_offset_alias(&self) -> f64 { self.percent_offset }
    #[setter(percentOffset)]
    fn set_percent_offset_alias(&mut self, v: f64) { self.percent_offset = v; }
    #[getter(permId)]
    fn get_perm_id_alias(&self) -> i64 { self.perm_id }
    #[setter(permId)]
    fn set_perm_id_alias(&mut self, v: i64) { self.perm_id = v; }
    #[getter(postOnly)]
    fn get_post_only_alias(&self) -> bool { self.post_only }
    #[setter(postOnly)]
    fn set_post_only_alias(&mut self, v: bool) { self.post_only = v; }
    #[getter(postToAts)]
    fn get_post_to_ats_alias(&self) -> i32 { self.post_to_ats }
    #[setter(postToAts)]
    fn set_post_to_ats_alias(&mut self, v: i32) { self.post_to_ats = v; }
    #[getter(professionalCustomer)]
    fn get_professional_customer_alias(&self) -> bool { self.professional_customer }
    #[setter(professionalCustomer)]
    fn set_professional_customer_alias(&mut self, v: bool) { self.professional_customer = v; }
    #[getter(ptOrderId)]
    fn get_pt_order_id_alias(&self) -> i32 { self.pt_order_id }
    #[setter(ptOrderId)]
    fn set_pt_order_id_alias(&mut self, v: i32) { self.pt_order_id = v; }
    #[getter(ptOrderType)]
    fn get_pt_order_type_alias(&self) -> String { self.pt_order_type.clone() }
    #[setter(ptOrderType)]
    fn set_pt_order_type_alias(&mut self, v: String) { self.pt_order_type = v; }
    #[getter(randomizePrice)]
    fn get_randomize_price_alias(&self) -> bool { self.randomize_price }
    #[setter(randomizePrice)]
    fn set_randomize_price_alias(&mut self, v: bool) { self.randomize_price = v; }
    #[getter(randomizeSize)]
    fn get_randomize_size_alias(&self) -> bool { self.randomize_size }
    #[setter(randomizeSize)]
    fn set_randomize_size_alias(&mut self, v: bool) { self.randomize_size = v; }
    #[getter(refFuturesConId)]
    fn get_ref_futures_con_id_alias(&self) -> i32 { self.ref_futures_con_id }
    #[setter(refFuturesConId)]
    fn set_ref_futures_con_id_alias(&mut self, v: i32) { self.ref_futures_con_id = v; }
    #[getter(referenceChangeAmount)]
    fn get_reference_change_amount_alias(&self) -> f64 { self.reference_change_amount }
    #[setter(referenceChangeAmount)]
    fn set_reference_change_amount_alias(&mut self, v: f64) { self.reference_change_amount = v; }
    #[getter(referenceContractId)]
    fn get_reference_contract_id_alias(&self) -> i32 { self.reference_contract_id }
    #[setter(referenceContractId)]
    fn set_reference_contract_id_alias(&mut self, v: i32) { self.reference_contract_id = v; }
    #[getter(referenceExchangeId)]
    fn get_reference_exchange_id_alias(&self) -> String { self.reference_exchange_id.clone() }
    #[setter(referenceExchangeId)]
    fn set_reference_exchange_id_alias(&mut self, v: String) { self.reference_exchange_id = v; }
    #[getter(referencePriceType)]
    fn get_reference_price_type_alias(&self) -> i32 { self.reference_price_type }
    #[setter(referencePriceType)]
    fn set_reference_price_type_alias(&mut self, v: i32) { self.reference_price_type = v; }
    #[getter(routeMarketableToBbo)]
    fn get_route_marketable_to_bbo_alias(&self) -> bool { self.route_marketable_to_bbo }
    #[setter(routeMarketableToBbo)]
    fn set_route_marketable_to_bbo_alias(&mut self, v: bool) { self.route_marketable_to_bbo = v; }
    #[getter(rule80A)]
    fn get_rule80a_alias(&self) -> String { self.rule80a.clone() }
    #[setter(rule80A)]
    fn set_rule80a_alias(&mut self, v: String) { self.rule80a = v; }
    #[getter(scaleAutoReset)]
    fn get_scale_auto_reset_alias(&self) -> bool { self.scale_auto_reset }
    #[setter(scaleAutoReset)]
    fn set_scale_auto_reset_alias(&mut self, v: bool) { self.scale_auto_reset = v; }
    #[getter(scaleInitFillQty)]
    fn get_scale_init_fill_qty_alias(&self) -> i32 { self.scale_init_fill_qty }
    #[setter(scaleInitFillQty)]
    fn set_scale_init_fill_qty_alias(&mut self, v: i32) { self.scale_init_fill_qty = v; }
    #[getter(scaleInitLevelSize)]
    fn get_scale_init_level_size_alias(&self) -> i32 { self.scale_init_level_size }
    #[setter(scaleInitLevelSize)]
    fn set_scale_init_level_size_alias(&mut self, v: i32) { self.scale_init_level_size = v; }
    #[getter(scaleInitPosition)]
    fn get_scale_init_position_alias(&self) -> i32 { self.scale_init_position }
    #[setter(scaleInitPosition)]
    fn set_scale_init_position_alias(&mut self, v: i32) { self.scale_init_position = v; }
    #[getter(scalePriceAdjustInterval)]
    fn get_scale_price_adjust_interval_alias(&self) -> i32 { self.scale_price_adjust_interval }
    #[setter(scalePriceAdjustInterval)]
    fn set_scale_price_adjust_interval_alias(&mut self, v: i32) { self.scale_price_adjust_interval = v; }
    #[getter(scalePriceAdjustValue)]
    fn get_scale_price_adjust_value_alias(&self) -> f64 { self.scale_price_adjust_value }
    #[setter(scalePriceAdjustValue)]
    fn set_scale_price_adjust_value_alias(&mut self, v: f64) { self.scale_price_adjust_value = v; }
    #[getter(scalePriceIncrement)]
    fn get_scale_price_increment_alias(&self) -> f64 { self.scale_price_increment }
    #[setter(scalePriceIncrement)]
    fn set_scale_price_increment_alias(&mut self, v: f64) { self.scale_price_increment = v; }
    #[getter(scaleProfitOffset)]
    fn get_scale_profit_offset_alias(&self) -> f64 { self.scale_profit_offset }
    #[setter(scaleProfitOffset)]
    fn set_scale_profit_offset_alias(&mut self, v: f64) { self.scale_profit_offset = v; }
    #[getter(scaleRandomPercent)]
    fn get_scale_random_percent_alias(&self) -> bool { self.scale_random_percent }
    #[setter(scaleRandomPercent)]
    fn set_scale_random_percent_alias(&mut self, v: bool) { self.scale_random_percent = v; }
    #[getter(scaleSubsLevelSize)]
    fn get_scale_subs_level_size_alias(&self) -> i32 { self.scale_subs_level_size }
    #[setter(scaleSubsLevelSize)]
    fn set_scale_subs_level_size_alias(&mut self, v: i32) { self.scale_subs_level_size = v; }
    #[getter(scaleTable)]
    fn get_scale_table_alias(&self) -> String { self.scale_table.clone() }
    #[setter(scaleTable)]
    fn set_scale_table_alias(&mut self, v: String) { self.scale_table = v; }
    #[getter(seekPriceImprovement)]
    fn get_seek_price_improvement_alias(&self) -> bool { self.seek_price_improvement }
    #[setter(seekPriceImprovement)]
    fn set_seek_price_improvement_alias(&mut self, v: bool) { self.seek_price_improvement = v; }
    #[getter(settlingFirm)]
    fn get_settling_firm_alias(&self) -> String { self.settling_firm.clone() }
    #[setter(settlingFirm)]
    fn set_settling_firm_alias(&mut self, v: String) { self.settling_firm = v; }
    #[getter(shortSaleSlot)]
    fn get_short_sale_slot_alias(&self) -> i32 { self.short_sale_slot }
    #[setter(shortSaleSlot)]
    fn set_short_sale_slot_alias(&mut self, v: i32) { self.short_sale_slot = v; }
    #[getter(slOrderId)]
    fn get_sl_order_id_alias(&self) -> i32 { self.sl_order_id }
    #[setter(slOrderId)]
    fn set_sl_order_id_alias(&mut self, v: i32) { self.sl_order_id = v; }
    #[getter(slOrderType)]
    fn get_sl_order_type_alias(&self) -> String { self.sl_order_type.clone() }
    #[setter(slOrderType)]
    fn set_sl_order_type_alias(&mut self, v: String) { self.sl_order_type = v; }
    #[getter(smartComboRoutingParams)]
    fn get_smart_combo_routing_params_alias(&self) -> Vec<TagValue> { self.smart_combo_routing_params.clone() }
    #[setter(smartComboRoutingParams)]
    fn set_smart_combo_routing_params_alias(&mut self, v: Vec<TagValue>) {
        self.smart_combo_routing_params = v;
    }
    #[getter(softDollarTier)]
    fn get_soft_dollar_tier(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let tier = SoftDollarTierPy {
            name: self.soft_dollar_tier_name.clone(),
            val: self.soft_dollar_tier_val.clone(),
            display_name: self.soft_dollar_tier_display_name.clone(),
        };
        Ok(Py::new(py, tier)?.into_any())
    }
    /// The three strings the tier is, taken from whatever states them.
    ///
    /// The tier is one object on the reference client and three fields here.
    /// Readable only, an order directing its commission to a tier reaches the
    /// venue with none, which is half a soft-dollar arrangement.
    #[setter(softDollarTier)]
    fn set_soft_dollar_tier(&mut self, v: &Bound<'_, PyAny>) -> PyResult<()> {
        let text = |names: [&str; 2]| -> String {
            names
                .iter()
                .find_map(|n| v.getattr(*n).ok().and_then(|a| a.extract::<String>().ok()))
                .unwrap_or_default()
        };
        self.soft_dollar_tier_name = text(["name", "name"]);
        self.soft_dollar_tier_val = text(["val", "value"]);
        self.soft_dollar_tier_display_name = text(["displayName", "display_name"]);
        Ok(())
    }
    #[getter(startingPrice)]
    fn get_starting_price_alias(&self) -> f64 { self.starting_price }
    #[setter(startingPrice)]
    fn set_starting_price_alias(&mut self, v: f64) { self.starting_price = v; }
    #[getter(stockRangeLower)]
    fn get_stock_range_lower_alias(&self) -> f64 { self.stock_range_lower }
    #[setter(stockRangeLower)]
    fn set_stock_range_lower_alias(&mut self, v: f64) { self.stock_range_lower = v; }
    #[getter(stockRangeUpper)]
    fn get_stock_range_upper_alias(&self) -> f64 { self.stock_range_upper }
    #[setter(stockRangeUpper)]
    fn set_stock_range_upper_alias(&mut self, v: f64) { self.stock_range_upper = v; }
    #[getter(stockRefPrice)]
    fn get_stock_ref_price_alias(&self) -> f64 { self.stock_ref_price }
    #[setter(stockRefPrice)]
    fn set_stock_ref_price_alias(&mut self, v: f64) { self.stock_ref_price = v; }
    #[getter(sweepToFill)]
    fn get_sweep_to_fill_alias(&self) -> bool { self.sweep_to_fill }
    #[setter(sweepToFill)]
    fn set_sweep_to_fill_alias(&mut self, v: bool) { self.sweep_to_fill = v; }
    #[getter(trailStopPrice)]
    fn get_trail_stop_price_alias(&self) -> f64 { self.trail_stop_price }
    #[setter(trailStopPrice)]
    fn set_trail_stop_price_alias(&mut self, v: f64) { self.trail_stop_price = v; }
    #[getter(trailingPercent)]
    fn get_trailing_percent_alias(&self) -> f64 { self.trailing_percent }
    #[setter(trailingPercent)]
    fn set_trailing_percent_alias(&mut self, v: f64) { self.trailing_percent = v; }
    #[getter(triggerMethod)]
    fn get_trigger_method_alias(&self) -> i32 { self.trigger_method }
    #[setter(triggerMethod)]
    fn set_trigger_method_alias(&mut self, v: i32) { self.trigger_method = v; }
    #[getter(triggerPrice)]
    fn get_trigger_price_alias(&self) -> f64 { self.trigger_price }
    #[setter(triggerPrice)]
    fn set_trigger_price_alias(&mut self, v: f64) { self.trigger_price = v; }
    #[getter(usePriceMgmtAlgo)]
    fn get_use_price_mgmt_algo_alias(&self) -> i32 { self.use_price_mgmt_algo }
    #[setter(usePriceMgmtAlgo)]
    fn set_use_price_mgmt_algo_alias(&mut self, v: i32) { self.use_price_mgmt_algo = v; }
    #[getter(volatilityType)]
    fn get_volatility_type_alias(&self) -> i32 { self.volatility_type }
    #[setter(volatilityType)]
    fn set_volatility_type_alias(&mut self, v: i32) { self.volatility_type = v; }
    #[getter(whatIf)]
    fn get_what_if_alias(&self) -> bool { self.what_if }
    #[setter(whatIf)]
    fn set_what_if_alias(&mut self, v: bool) { self.what_if = v; }
    #[getter(whatIfType)]
    fn get_what_if_type_alias(&self) -> i32 { self.what_if_type }
    #[setter(whatIfType)]
    fn set_what_if_type_alias(&mut self, v: i32) { self.what_if_type = v; }
}

impl Order {
    /// Convert Py<PyAny> conditions to internal OrderCondition list.
    /// The conditions an order is held under.
    ///
    /// A condition this client does not know is refused rather than dropped:
    /// dropped, the order goes live at once and the caller is told nothing,
    /// which is the opposite of what a condition is for. The six the venue
    /// carries are price, time, margin, execution, volume and percent change.
    pub fn convert_conditions(&self, py: Python<'_>) -> Result<Vec<OrderCondition>, String> {
        self.conditions.iter().enumerate().map(|(at, obj)| {
            let any = obj.bind(py);
            if let Ok(c) = any.cast::<PriceCondition>() { return Ok(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<TimeCondition>() { return Ok(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<MarginCondition>() { return Ok(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<ExecutionCondition>() { return Ok(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<VolumeCondition>() { return Ok(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<PercentChangeCondition>() { return Ok(c.borrow().to_internal()); }
            Err(format!(
                "condition {at} is of a kind this client does not carry. It is \
                 one of PriceCondition, TimeCondition, MarginCondition, \
                 ExecutionCondition, VolumeCondition or PercentChangeCondition \
                 — dropped, the order would go live at once with nothing said"
            ))
        }).collect()
    }

    /// The price stated for each leg of a combination.
    ///
    /// A leg is a Python object, so reading one needs the interpreter, which
    /// the conversion does not hold. Dropped, every leg of a combination
    /// priced by the caller went out at whatever the venue struck it at.
    pub fn convert_order_combo_legs(&self, py: Python<'_>) -> Result<Vec<f64>, String> {
        self.order_combo_legs
            .iter()
            .enumerate()
            .map(|(at, leg)| {
                leg.bind(py)
                    .getattr("price")
                    .and_then(|v| v.extract::<f64>())
                    .map_err(|e| {
                        format!(
                            "leg {at} of this combination states a price that \
                             cannot be read: {e}. Read as absent, the leg would \
                             be struck at whatever the venue struck it at, which \
                             is not the price the caller stated"
                        )
                    })
            })
            .collect()
    }

    /// The caller's own tags on an order.
    ///
    /// This protocol carries no field for them, so an order stating any is
    /// refused rather than sent without them — which is what the refusal needs
    /// them read for.
    pub fn convert_misc_options(
        &self, py: Python<'_>,
    ) -> Result<Vec<crate::types::model::TagValue>, String> {
        self.order_misc_options
            .iter()
            .enumerate()
            .map(|(at, obj)| {
                let any = obj.bind(py);
                let read = |name: &str| -> Result<String, String> {
                    any.getattr(name)
                        .and_then(|v| v.extract())
                        .map_err(|e| format!("option {at} states no readable {name}: {e}"))
                };
                Ok(crate::types::model::TagValue {
                    tag: read("tag")?,
                    value: read("value")?,
                })
            })
            .collect()
    }


    /// The whole order the engine holds, not the handful of fields a callback
    /// happens to print.
    ///
    /// An order read back from the venue is placed again, so a field left at
    /// its default here is a field the caller loses by reading their own order:
    /// an outside-hours order becomes a regular-hours one, and the order they
    /// place back is not the order they were shown. Every field is stated, and
    /// no fallback, so one added to either side fails to compile rather than
    /// quietly defaulting.
    ///
    /// The fields holding Python objects are built back from what the engine
    /// keeps. The conditions are: each kind the venue carries has a class on
    /// this surface with the same fields, so an order read back states what it
    /// is waiting for and can be placed again as it stood.
    ///
    /// The price per leg of a combination is not, because nothing reads one
    /// back: the venue's report of an order names its legs without saying what
    /// each was struck at, so the engine holds none to carry. An order read
    /// back and placed again is priced as a whole. Hold the order that was
    /// placed to state them again.
    /// `under` is the client id this session connected with, and stands in only
    /// where the venue named no client. The reference client keys a trade by
    /// client id together with order id, so an order placed elsewhere keeps the
    /// client id it was placed under. Restating it as this session's collides
    /// with whatever this session holds under the same order id.
    pub(crate) fn from_api(
        py: Python<'_>,
        a: &crate::types::model::Order,
        under: i32,
    ) -> PyResult<Self> {
        Ok(Self {
            order_id: a.order_id,
            action: a.action.clone(),
            total_quantity: a.total_quantity,
            order_type: a.order_type.clone(),
            lmt_price: a.lmt_price,
            aux_price: a.aux_price,
            tif: a.tif.clone(),
            outside_rth: a.outside_rth,
            display_size: a.display_size,
            min_qty: a.min_qty,
            hidden: a.hidden,
            good_after_time: a.good_after_time.clone(),
            good_till_date: a.good_till_date.clone(),
            oca_group: a.oca_group.clone(),
            trailing_percent: a.trailing_percent,
            algo_strategy: a.algo_strategy.clone(),
            algo_params: a
                .algo_params
                .iter()
                .map(|tv| super::class_contracts::TagValue {
                    tag: tv.tag.clone(),
                    value: tv.value.clone(),
                })
                .collect(),
            what_if: a.what_if,
            cash_qty: a.cash_qty,
            parent_id: a.parent_id,
            transmit: a.transmit,
            discretionary_amt: a.discretionary_amt,
            sweep_to_fill: a.sweep_to_fill,
            all_or_none: a.all_or_none,
            trigger_method: a.trigger_method,
            adjusted_order_type: a.adjusted_order_type.clone(),
            trigger_price: a.trigger_price,
            adjusted_stop_price: a.adjusted_stop_price,
            adjusted_stop_limit_price: a.adjusted_stop_limit_price,
            conditions: a
                .conditions
                .iter()
                .map(|held| super::class_conditions::condition_from_internal(py, held))
                .collect::<PyResult<Vec<_>>>()?,
            conditions_ignore_rth: a.conditions_ignore_rth,
            conditions_cancel_order: a.conditions_cancel_order,
            account: a.account.clone(),
            active_start_time: a.active_start_time.clone(),
            active_stop_time: a.active_stop_time.clone(),
            adjustable_trailing_unit: a.adjustable_trailing_unit,
            adjusted_trailing_amount: a.adjusted_trailing_amount,
            advanced_error_override: a.advanced_error_override.clone(),
            algo_id: a.algo_id.clone(),
            allow_pre_open: a.allow_pre_open,
            auction_strategy: a.auction_strategy,
            auto_cancel_date: a.auto_cancel_date.clone(),
            auto_cancel_parent: a.auto_cancel_parent,
            basis_points: a.basis_points,
            basis_points_type: a.basis_points_type,
            block_order: a.block_order,
            bond_accrued_interest: a.bond_accrued_interest.clone(),
            clearing_account: a.clearing_account.clone(),
            clearing_intent: a.clearing_intent.clone(),
            client_id: if a.client_id != 0 { a.client_id } else { under },
            compete_against_best_offset: a.compete_against_best_offset,
            continuous_update: a.continuous_update,
            customer_account: a.customer_account.clone(),
            deactivate: a.deactivate,
            deactivate_on_disconnect: a.deactivate_on_disconnect,
            delta: a.delta,
            delta_neutral_aux_price: a.delta_neutral_aux_price,
            delta_neutral_clearing_account: a.delta_neutral_clearing_account.clone(),
            delta_neutral_clearing_intent: a.delta_neutral_clearing_intent.clone(),
            delta_neutral_con_id: a.delta_neutral_con_id,
            delta_neutral_designated_location: a.delta_neutral_designated_location.clone(),
            delta_neutral_open_close: a.delta_neutral_open_close.clone(),
            delta_neutral_order_type: a.delta_neutral_order_type.clone(),
            delta_neutral_settling_firm: a.delta_neutral_settling_firm.clone(),
            delta_neutral_short_sale: a.delta_neutral_short_sale,
            delta_neutral_short_sale_slot: a.delta_neutral_short_sale_slot,
            designated_location: a.designated_location.clone(),
            discretionary_up_to_limit_price: a.discretionary_up_to_limit_price,
            dont_use_auto_price_for_hedge: a.dont_use_auto_price_for_hedge,
            duration: a.duration,
            exempt_code: a.exempt_code,
            ext_operator: a.ext_operator.clone(),
            fa_group: a.fa_group.clone(),
            fa_method: a.fa_method.clone(),
            fa_percentage: a.fa_percentage.clone(),
            filled_quantity: a.filled_quantity,
            hedge_param: a.hedge_param.clone(),
            hedge_type: a.hedge_type.clone(),
            ignore_open_auction: a.ignore_open_auction,
            imbalance_only: a.imbalance_only,
            include_overnight: a.include_overnight,
            is_oms_container: a.is_oms_container,
            is_pegged_change_amount_decrease: a.is_pegged_change_amount_decrease,
            lmt_price_offset: a.lmt_price_offset,
            manual_order_indicator: a.manual_order_indicator,
            manual_order_time: a.manual_order_time.clone(),
            mid_offset_at_half: a.mid_offset_at_half,
            mid_offset_at_whole: a.mid_offset_at_whole,
            mifid2_decision_algo: a.mifid2_decision_algo.clone(),
            mifid2_decision_maker: a.mifid2_decision_maker.clone(),
            mifid2_execution_algo: a.mifid2_execution_algo.clone(),
            mifid2_execution_trader: a.mifid2_execution_trader.clone(),
            min_compete_size: a.min_compete_size,
            min_trade_qty: a.min_trade_qty,
            model_code: a.model_code.clone(),
            not_held: a.not_held,
            oca_type: a.oca_type,
            open_close: a.open_close.clone(),
            opt_out_smart_routing: a.opt_out_smart_routing,
            order_combo_legs: Vec::new(),
            order_misc_options: Vec::new(),
            order_ref: a.order_ref.clone(),
            origin: a.origin,
            override_percentage_constraints: a.override_percentage_constraints,
            parent_perm_id: a.parent_perm_id,
            pegged_change_amount: a.pegged_change_amount,
            percent_offset: a.percent_offset,
            perm_id: a.perm_id,
            post_only: a.post_only,
            post_to_ats: a.post_to_ats,
            professional_customer: a.professional_customer,
            pt_order_id: a.pt_order_id,
            pt_order_type: a.pt_order_type.clone(),
            randomize_price: a.randomize_price,
            randomize_size: a.randomize_size,
            ref_futures_con_id: a.ref_futures_con_id,
            reference_change_amount: a.reference_change_amount,
            reference_contract_id: a.reference_contract_id,
            reference_exchange_id: a.reference_exchange_id.clone(),
            reference_price_type: a.reference_price_type,
            route_marketable_to_bbo: a.route_marketable_to_bbo,
            rule80a: a.rule80a.clone(),
            scale_auto_reset: a.scale_auto_reset,
            scale_init_fill_qty: a.scale_init_fill_qty,
            scale_init_level_size: a.scale_init_level_size,
            scale_init_position: a.scale_init_position,
            scale_price_adjust_interval: a.scale_price_adjust_interval,
            scale_price_adjust_value: a.scale_price_adjust_value,
            scale_price_increment: a.scale_price_increment,
            scale_profit_offset: a.scale_profit_offset,
            scale_random_percent: a.scale_random_percent,
            scale_subs_level_size: a.scale_subs_level_size,
            scale_table: a.scale_table.clone(),
            seek_price_improvement: a.seek_price_improvement,
            settling_firm: a.settling_firm.clone(),
            shareholder: a.shareholder.clone(),
            short_sale_slot: a.short_sale_slot,
            sl_order_id: a.sl_order_id,
            sl_order_type: a.sl_order_type.clone(),
            smart_combo_routing_params: a
                .smart_combo_routing_params
                .iter()
                .map(|tv| super::class_contracts::TagValue {
                    tag: tv.tag.clone(),
                    value: tv.value.clone(),
                })
                .collect(),
            soft_dollar_tier_name: a.soft_dollar_tier_name.clone(),
            soft_dollar_tier_val: a.soft_dollar_tier_val.clone(),
            soft_dollar_tier_display_name: a.soft_dollar_tier_display_name.clone(),
            solicited: a.solicited,
            starting_price: a.starting_price,
            stock_range_lower: a.stock_range_lower,
            stock_range_upper: a.stock_range_upper,
            stock_ref_price: a.stock_ref_price,
            submitter: a.submitter.clone(),
            trail_stop_price: a.trail_stop_price,
            use_price_mgmt_algo: a.use_price_mgmt_algo,
            volatility: a.volatility,
            volatility_type: a.volatility_type,
            what_if_type: a.what_if_type,
        })
    }

    /// Convert to Rust API Order.
    pub fn to_api(&self) -> crate::types::model::Order {
        crate::types::model::Order {
            order_id: self.order_id,
            action: self.action.clone(),
            total_quantity: self.total_quantity,
            order_type: self.order_type.clone(),
            lmt_price: self.lmt_price,
            aux_price: self.aux_price,
            tif: self.tif.clone(),
            outside_rth: self.outside_rth,
            display_size: self.display_size,
            min_qty: self.min_qty,
            hidden: self.hidden,
            good_after_time: self.good_after_time.clone(),
            good_till_date: self.good_till_date.clone(),
            oca_group: self.oca_group.clone(),
            trailing_percent: self.trailing_percent,
            algo_strategy: self.algo_strategy.clone(),
            algo_params: self.algo_params.iter().map(|tv| crate::types::model::TagValue {
                tag: tv.tag.clone(),
                value: tv.value.clone(),
            }).collect(),
            what_if: self.what_if,
            cash_qty: self.cash_qty,
            parent_id: self.parent_id,
            transmit: self.transmit,
            discretionary_amt: self.discretionary_amt,
            sweep_to_fill: self.sweep_to_fill,
            all_or_none: self.all_or_none,
            trigger_method: self.trigger_method,
            adjusted_order_type: self.adjusted_order_type.clone(),
            trigger_price: self.trigger_price,
            adjusted_stop_price: self.adjusted_stop_price,
            adjusted_stop_limit_price: self.adjusted_stop_limit_price,
            conditions: Vec::new(), // Use convert_conditions(py) + to_api() at call sites that need conditions
            conditions_ignore_rth: self.conditions_ignore_rth,
            conditions_cancel_order: self.conditions_cancel_order,
            // Forward ibapi-parity fields
            account: self.account.clone(),
            active_start_time: self.active_start_time.clone(),
            active_stop_time: self.active_stop_time.clone(),
            adjustable_trailing_unit: self.adjustable_trailing_unit,
            adjusted_trailing_amount: self.adjusted_trailing_amount,
            advanced_error_override: self.advanced_error_override.clone(),
            algo_id: self.algo_id.clone(),
            allow_pre_open: self.allow_pre_open,
            auction_strategy: self.auction_strategy,
            auto_cancel_date: self.auto_cancel_date.clone(),
            auto_cancel_parent: self.auto_cancel_parent,
            basis_points: self.basis_points,
            basis_points_type: self.basis_points_type,
            block_order: self.block_order,
            bond_accrued_interest: self.bond_accrued_interest.clone(),
            clearing_account: self.clearing_account.clone(),
            clearing_intent: self.clearing_intent.clone(),
            client_id: self.client_id,
            compete_against_best_offset: self.compete_against_best_offset,
            continuous_update: self.continuous_update,
            customer_account: self.customer_account.clone(),
            deactivate: self.deactivate,
            deactivate_on_disconnect: self.deactivate_on_disconnect,
            delta: self.delta,
            delta_neutral_aux_price: self.delta_neutral_aux_price,
            delta_neutral_clearing_account: self.delta_neutral_clearing_account.clone(),
            delta_neutral_clearing_intent: self.delta_neutral_clearing_intent.clone(),
            delta_neutral_con_id: self.delta_neutral_con_id,
            delta_neutral_designated_location: self.delta_neutral_designated_location.clone(),
            delta_neutral_open_close: self.delta_neutral_open_close.clone(),
            delta_neutral_order_type: self.delta_neutral_order_type.clone(),
            delta_neutral_settling_firm: self.delta_neutral_settling_firm.clone(),
            delta_neutral_short_sale: self.delta_neutral_short_sale,
            delta_neutral_short_sale_slot: self.delta_neutral_short_sale_slot,
            designated_location: self.designated_location.clone(),
            discretionary_up_to_limit_price: self.discretionary_up_to_limit_price,
            dont_use_auto_price_for_hedge: self.dont_use_auto_price_for_hedge,
            duration: self.duration,
            exempt_code: self.exempt_code,
            ext_operator: self.ext_operator.clone(),
            fa_group: self.fa_group.clone(),
            fa_method: self.fa_method.clone(),
            fa_percentage: self.fa_percentage.clone(),
            filled_quantity: self.filled_quantity,
            hedge_param: self.hedge_param.clone(),
            hedge_type: self.hedge_type.clone(),
            ignore_open_auction: self.ignore_open_auction,
            imbalance_only: self.imbalance_only,
            include_overnight: self.include_overnight,
            is_oms_container: self.is_oms_container,
            is_pegged_change_amount_decrease: self.is_pegged_change_amount_decrease,
            // Unset is f64::MAX on both sides, so leaving it to Default made a
            // caller's offset indistinguishable from absent and the wire fell
            // back to lmt_price: a TRAIL LIMIT could not set its offset from
            // Python at all.
            lmt_price_offset: self.lmt_price_offset,
            manual_order_indicator: self.manual_order_indicator,
            manual_order_time: self.manual_order_time.clone(),
            mid_offset_at_half: self.mid_offset_at_half,
            mid_offset_at_whole: self.mid_offset_at_whole,
            mifid2_decision_algo: self.mifid2_decision_algo.clone(),
            mifid2_decision_maker: self.mifid2_decision_maker.clone(),
            mifid2_execution_algo: self.mifid2_execution_algo.clone(),
            mifid2_execution_trader: self.mifid2_execution_trader.clone(),
            min_compete_size: self.min_compete_size,
            min_trade_qty: self.min_trade_qty,
            model_code: self.model_code.clone(),
            not_held: self.not_held,
            oca_type: self.oca_type,
            open_close: self.open_close.clone(),
            opt_out_smart_routing: self.opt_out_smart_routing,
            // Python objects, so reading them needs the interpreter this does
            // not hold. Filled at the call site from `convert_order_combo_legs`
            // and `convert_misc_options`, beside the conditions, and a test
            // holds every field to being filled in one place or the other.
            order_combo_legs: Vec::new(),
            order_misc_options: Vec::new(),
            order_ref: self.order_ref.clone(),
            origin: self.origin,
            override_percentage_constraints: self.override_percentage_constraints,
            parent_perm_id: self.parent_perm_id,
            pegged_change_amount: self.pegged_change_amount,
            percent_offset: self.percent_offset,
            perm_id: self.perm_id,
            post_only: self.post_only,
            post_to_ats: self.post_to_ats,
            professional_customer: self.professional_customer,
            pt_order_id: self.pt_order_id,
            pt_order_type: self.pt_order_type.clone(),
            randomize_price: self.randomize_price,
            randomize_size: self.randomize_size,
            ref_futures_con_id: self.ref_futures_con_id,
            reference_change_amount: self.reference_change_amount,
            reference_contract_id: self.reference_contract_id,
            reference_exchange_id: self.reference_exchange_id.clone(),
            reference_price_type: self.reference_price_type,
            route_marketable_to_bbo: self.route_marketable_to_bbo,
            rule80a: self.rule80a.clone(),
            scale_auto_reset: self.scale_auto_reset,
            scale_init_fill_qty: self.scale_init_fill_qty,
            scale_init_level_size: self.scale_init_level_size,
            scale_init_position: self.scale_init_position,
            scale_price_adjust_interval: self.scale_price_adjust_interval,
            scale_price_adjust_value: self.scale_price_adjust_value,
            scale_price_increment: self.scale_price_increment,
            scale_profit_offset: self.scale_profit_offset,
            scale_random_percent: self.scale_random_percent,
            scale_subs_level_size: self.scale_subs_level_size,
            scale_table: self.scale_table.clone(),
            seek_price_improvement: self.seek_price_improvement,
            settling_firm: self.settling_firm.clone(),
            shareholder: self.shareholder.clone(),
            short_sale_slot: self.short_sale_slot,
            sl_order_id: self.sl_order_id,
            sl_order_type: self.sl_order_type.clone(),
            smart_combo_routing_params: self
                .smart_combo_routing_params
                .iter()
                .map(|tv| crate::types::model::TagValue {
                    tag: tv.tag.clone(),
                    value: tv.value.clone(),
                })
                .collect(),
            soft_dollar_tier_name: self.soft_dollar_tier_name.clone(),
            soft_dollar_tier_val: self.soft_dollar_tier_val.clone(),
            soft_dollar_tier_display_name: self.soft_dollar_tier_display_name.clone(),
            solicited: self.solicited,
            starting_price: self.starting_price,
            stock_range_lower: self.stock_range_lower,
            stock_range_upper: self.stock_range_upper,
            stock_ref_price: self.stock_ref_price,
            submitter: self.submitter.clone(),
            trail_stop_price: self.trail_stop_price,
            use_price_mgmt_algo: self.use_price_mgmt_algo,
            volatility: self.volatility,
            volatility_type: self.volatility_type,
            what_if_type: self.what_if_type,
        }
    }

}

/// ibapi-compatible OrderAllocation class.
/// Decimal fields are carried as strings to preserve precision.
#[pyclass(from_py_object)]
#[derive(Clone, Default)]
pub struct OrderAllocation {
    #[pyo3(get, set)] pub account: String,
    #[pyo3(get, set)] pub position: String,
    #[pyo3(get, set)] pub position_desired: String,
    #[pyo3(get, set)] pub position_after: String,
    #[pyo3(get, set)] pub desired_alloc_qty: String,
    #[pyo3(get, set)] pub allowed_alloc_qty: String,
    #[pyo3(get, set)] pub is_monetary: bool,
}

#[pymethods]
impl OrderAllocation {
    #[new]
    #[pyo3(signature = ())]
    fn new() -> Self { Self::default() }

    fn __repr__(&self) -> String {
        format!("OrderAllocation(account='{}', position={})", self.account, self.position)
    }
}

impl OrderAllocation {
    /// The same allocation, as the Python side names it.
    pub(crate) fn from_api(a: &crate::types::model::OrderAllocation) -> Self {
        Self {
            account: a.account.clone(),
            position: a.position.clone(),
            position_desired: a.position_desired.clone(),
            position_after: a.position_after.clone(),
            desired_alloc_qty: a.desired_alloc_qty.clone(),
            allowed_alloc_qty: a.allowed_alloc_qty.clone(),
            is_monetary: a.is_monetary,
        }
    }
}

/// ibapi-compatible OrderState class (used in openOrder callback).
#[pyclass(from_py_object)]
#[derive(Clone, Default)]
pub struct OrderState {
    #[pyo3(get, set)]
    pub status: String,
    #[pyo3(get, set)]
    pub init_margin_before: String,
    #[pyo3(get, set)]
    pub maint_margin_before: String,
    #[pyo3(get, set)]
    pub equity_with_loan_before: String,
    #[pyo3(get, set)]
    pub init_margin_change: String,
    #[pyo3(get, set)]
    pub maint_margin_change: String,
    #[pyo3(get, set)]
    pub equity_with_loan_change: String,
    #[pyo3(get, set)]
    pub init_margin_after: String,
    #[pyo3(get, set)]
    pub maint_margin_after: String,
    #[pyo3(get, set)]
    pub equity_with_loan_after: String,
    #[pyo3(get, set)]
    pub commission_and_fees: f64,
    #[pyo3(get, set)]
    pub min_commission_and_fees: f64,
    #[pyo3(get, set)]
    pub max_commission_and_fees: f64,
    #[pyo3(get, set)]
    pub commission_and_fees_currency: String,
    #[pyo3(get, set)]
    pub warning_text: String,
    #[pyo3(get, set)]
    pub completed_time: String,
    #[pyo3(get, set)]
    pub completed_status: String,
    // ── ibapi-iso extension: RTH-split margin + allocations ──
    #[pyo3(get, set)] pub margin_currency: String,
    #[pyo3(get, set)] pub init_margin_before_outside_rth: f64,
    #[pyo3(get, set)] pub maint_margin_before_outside_rth: f64,
    #[pyo3(get, set)] pub equity_with_loan_before_outside_rth: f64,
    #[pyo3(get, set)] pub init_margin_change_outside_rth: f64,
    #[pyo3(get, set)] pub maint_margin_change_outside_rth: f64,
    #[pyo3(get, set)] pub equity_with_loan_change_outside_rth: f64,
    #[pyo3(get, set)] pub init_margin_after_outside_rth: f64,
    #[pyo3(get, set)] pub maint_margin_after_outside_rth: f64,
    #[pyo3(get, set)] pub equity_with_loan_after_outside_rth: f64,
    #[pyo3(get, set)] pub suggested_size: String,
    #[pyo3(get, set)] pub reject_reason: String,
    #[pyo3(get, set)] pub order_allocations: Vec<OrderAllocation>,
}

impl OrderState {
    /// The same order state, as the Python side names it.
    pub(crate) fn from_api(s: &crate::types::model::OrderState) -> Self {
        Self {
            status: s.status.clone(),
            init_margin_before: s.init_margin_before.clone(),
            maint_margin_before: s.maint_margin_before.clone(),
            equity_with_loan_before: s.equity_with_loan_before.clone(),
            init_margin_change: s.init_margin_change.clone(),
            maint_margin_change: s.maint_margin_change.clone(),
            equity_with_loan_change: s.equity_with_loan_change.clone(),
            init_margin_after: s.init_margin_after.clone(),
            maint_margin_after: s.maint_margin_after.clone(),
            equity_with_loan_after: s.equity_with_loan_after.clone(),
            commission_and_fees: s.commission_and_fees,
            min_commission_and_fees: s.min_commission_and_fees,
            max_commission_and_fees: s.max_commission_and_fees,
            commission_and_fees_currency: s.commission_and_fees_currency.clone(),
            warning_text: s.warning_text.clone(),
            completed_time: s.completed_time.clone(),
            completed_status: s.completed_status.clone(),
            margin_currency: s.margin_currency.clone(),
            init_margin_before_outside_rth: s.init_margin_before_outside_rth,
            maint_margin_before_outside_rth: s.maint_margin_before_outside_rth,
            equity_with_loan_before_outside_rth: s.equity_with_loan_before_outside_rth,
            init_margin_change_outside_rth: s.init_margin_change_outside_rth,
            maint_margin_change_outside_rth: s.maint_margin_change_outside_rth,
            equity_with_loan_change_outside_rth: s.equity_with_loan_change_outside_rth,
            init_margin_after_outside_rth: s.init_margin_after_outside_rth,
            maint_margin_after_outside_rth: s.maint_margin_after_outside_rth,
            equity_with_loan_after_outside_rth: s.equity_with_loan_after_outside_rth,
            suggested_size: s.suggested_size.clone(),
            reject_reason: s.reject_reason.clone(),
            order_allocations: s.order_allocations.iter().map(OrderAllocation::from_api).collect(),
        }
    }
}

#[pymethods]
impl OrderState {
    /// Answer to the name the reference client gives a field as well as the
    /// name this one gives it.
    ///
    /// This object is handed to a caller by a callback and only ever read. Code
    /// written for the reference client reads the run-together names, and under
    /// this class they were absent — so the object arrived carrying everything
    /// and answered nothing.
    ///
    /// Only reached when the attribute was not found, so it costs nothing on
    /// the names this class defines.
    fn __getattr__(slf: Bound<'_, Self>, name: &str) -> PyResult<Py<PyAny>> {
        by_reference_name(slf.as_any(), name, &[])
    }

    #[new]
    #[pyo3(signature = ())]
    fn new() -> Self {
        Self::default()
    }

    fn __repr__(&self) -> String {
        format!("OrderState(status='{}')", self.status)
    }
}

/// ibapi-compatible SoftDollarTier class.
#[pyclass(from_py_object, name = "SoftDollarTier")]
#[derive(Clone, Debug, Default)]
pub struct SoftDollarTierPy {
    #[pyo3(get, set)]
    pub name: String,
    #[pyo3(get, set)]
    pub val: String,
    #[pyo3(get, set)]
    pub display_name: String,
}

#[pymethods]
impl SoftDollarTierPy {
    #[new]
    #[pyo3(signature = ())]
    fn new() -> Self { Self::default() }

    // The reference client runs the words together. Without this, a tier built
    // the way that client builds one carried its name and its value and lost
    // what it is called.
    #[getter(displayName)]
    fn get_display_name_alias(&self) -> String { self.display_name.clone() }
    #[setter(displayName)]
    fn set_display_name_alias(&mut self, v: String) { self.display_name = v; }
}
