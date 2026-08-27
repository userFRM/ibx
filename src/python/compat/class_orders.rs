//! The order classes a caller works in, as the Python API names them.

// The other families, and the two helpers every class here uses.
use super::{class_contracts::*, class_conditions::*};
use super::contract::{by_reference_name, set_from_keywords};
use pyo3::prelude::*;

use super::{camel_aliases_copy, camel_aliases_owned};
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

    // ── New camelCase aliases ──
    #[getter(activeStartTime)]
    fn get_active_start_time_alias(&self) -> String { self.active_start_time.clone() }
    #[setter(activeStartTime)]
    fn set_active_start_time_alias(&mut self, v: String) { self.active_start_time = v; }
    #[getter(algoParams)]
    fn get_algo_params_alias(&self) -> Vec<TagValue> { self.algo_params.clone() }
    // Writable as well as readable. Readable only, parameters set under the
    // reference client's name for them do not reach the order, and it goes out
    // on the venue's default settings for that algo.
    #[setter(algoParams)]
    fn set_algo_params_alias(&mut self, v: Vec<TagValue>) { self.algo_params = v; }
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

/// A margin figure the venue did not state, written the way the reference
/// client writes one.
///
/// These carry numbers as text and a caller reads them with `float`. Left
/// empty that raises, and the answer the empty figure belonged to never
/// reaches the caller at all — a preview on a contract the venue states no
/// margin for was lost whole, not merely missing a field. The reference stack
/// writes an unstated double as the largest one there is, which a caller's
/// library already reads back as nothing.
fn unstated_figure(stated: &str) -> String {
    if stated.is_empty() {
        f64::MAX.to_string()
    } else {
        stated.to_string()
    }
}

impl Default for OrderState {
    /// An order state nobody has filled in yet.
    ///
    /// The margin and equity figures carry numbers as text, and a caller reads
    /// them with `float`. Defaulted to the empty string they raise inside the
    /// callback, and the whole report is lost rather than arriving with a field
    /// unset — which is what happened to every open order reported through a
    /// path that states the status and leaves the rest to this.
    ///
    /// Only the ones carried as text. The figures already carried as numbers
    /// are read as numbers and default as they did.
    fn default() -> Self {
        let unstated = || f64::MAX.to_string();
        Self {
            status: String::new(),
            init_margin_before: unstated(),
            maint_margin_before: unstated(),
            equity_with_loan_before: unstated(),
            init_margin_change: unstated(),
            maint_margin_change: unstated(),
            equity_with_loan_change: unstated(),
            init_margin_after: unstated(),
            maint_margin_after: unstated(),
            equity_with_loan_after: unstated(),
            commission_and_fees: 0.0,
            min_commission_and_fees: 0.0,
            max_commission_and_fees: 0.0,
            commission_and_fees_currency: String::new(),
            warning_text: String::new(),
            completed_time: String::new(),
            completed_status: String::new(),
            margin_currency: String::new(),
            init_margin_before_outside_rth: 0.0,
            maint_margin_before_outside_rth: 0.0,
            equity_with_loan_before_outside_rth: 0.0,
            init_margin_change_outside_rth: 0.0,
            maint_margin_change_outside_rth: 0.0,
            equity_with_loan_change_outside_rth: 0.0,
            init_margin_after_outside_rth: 0.0,
            maint_margin_after_outside_rth: 0.0,
            equity_with_loan_after_outside_rth: 0.0,
            suggested_size: String::new(),
            reject_reason: String::new(),
            order_allocations: Vec::new(),
        }
    }
}

/// ibapi-compatible OrderState class (used in openOrder callback).
#[pyclass(from_py_object)]
#[derive(Clone)]
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
            init_margin_before: unstated_figure(&s.init_margin_before),
            maint_margin_before: unstated_figure(&s.maint_margin_before),
            equity_with_loan_before: unstated_figure(&s.equity_with_loan_before),
            init_margin_change: unstated_figure(&s.init_margin_change),
            maint_margin_change: unstated_figure(&s.maint_margin_change),
            equity_with_loan_change: unstated_figure(&s.equity_with_loan_change),
            init_margin_after: unstated_figure(&s.init_margin_after),
            maint_margin_after: unstated_figure(&s.maint_margin_after),
            equity_with_loan_after: unstated_figure(&s.equity_with_loan_after),
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

#[cfg(test)]
mod unstated_figure_tests {
    use super::unstated_figure;

    /// A margin figure the venue did not state is written as the reference
    /// client writes one, not left empty.
    ///
    /// A caller reads these with `float`. Empty, that raises inside the
    /// callback and the whole answer is lost — a preview on a contract the
    /// venue states no margin for reached the caller as nothing at all, rather
    /// than as a preview missing a field. Seen against a crypto contract,
    /// which the venue prices without stating margin.
    #[test]
    fn an_unstated_figure_is_written_the_way_a_caller_can_read_it() {
        let written = unstated_figure("");
        assert!(
            written.parse::<f64>().is_ok(),
            "a caller reads these with float, and got {written:?}",
        );
        assert_eq!(written.parse::<f64>().unwrap(), f64::MAX, "and reads it back as unstated");
    }

    /// A figure the venue did state is passed through untouched.
    #[test]
    fn a_stated_figure_is_left_as_the_venue_wrote_it() {
        assert_eq!(unstated_figure("96525.01"), "96525.01");
        assert_eq!(unstated_figure("0"), "0", "nothing is not zero, and zero is not nothing");
    }
}

camel_aliases_copy! {
    Order {
        get_lmt_price_alias set_lmt_price_alias lmtPrice lmt_price f64;
        get_order_id_alias set_order_id_alias orderId order_id i64;
        get_total_quantity_alias set_total_quantity_alias totalQuantity total_quantity f64;
        get_adjustable_trailing_unit_alias set_adjustable_trailing_unit_alias adjustableTrailingUnit adjustable_trailing_unit i32;
        get_adjusted_trailing_amount_alias set_adjusted_trailing_amount_alias adjustedTrailingAmount adjusted_trailing_amount f64;
        get_adjusted_stop_price_alias set_adjusted_stop_price_alias adjustedStopPrice adjusted_stop_price f64;
        get_adjusted_stop_limit_price_alias set_adjusted_stop_limit_price_alias adjustedStopLimitPrice adjusted_stop_limit_price f64;
        get_all_or_none_alias set_all_or_none_alias allOrNone all_or_none bool;
        get_allow_pre_open_alias set_allow_pre_open_alias allowPreOpen allow_pre_open bool;
        get_auction_strategy_alias set_auction_strategy_alias auctionStrategy auction_strategy i32;
        get_auto_cancel_parent_alias set_auto_cancel_parent_alias autoCancelParent auto_cancel_parent bool;
        get_basis_points_alias set_basis_points_alias basisPoints basis_points f64;
        get_basis_points_type_alias set_basis_points_type_alias basisPointsType basis_points_type i32;
        get_block_order_alias set_block_order_alias blockOrder block_order bool;
        get_cash_qty_alias set_cash_qty_alias cashQty cash_qty f64;
        get_client_id_alias set_client_id_alias clientId client_id i32;
        get_compete_against_best_offset_alias set_compete_against_best_offset_alias competeAgainstBestOffset compete_against_best_offset f64;
        get_conditions_cancel_order_alias set_conditions_cancel_order_alias conditionsCancelOrder conditions_cancel_order bool;
        get_conditions_ignore_rth_alias set_conditions_ignore_rth_alias conditionsIgnoreRth conditions_ignore_rth bool;
        get_continuous_update_alias set_continuous_update_alias continuousUpdate continuous_update bool;
        get_delta_neutral_aux_price_alias set_delta_neutral_aux_price_alias deltaNeutralAuxPrice delta_neutral_aux_price f64;
        get_delta_neutral_con_id_alias set_delta_neutral_con_id_alias deltaNeutralConId delta_neutral_con_id i32;
        get_delta_neutral_short_sale_alias set_delta_neutral_short_sale_alias deltaNeutralShortSale delta_neutral_short_sale bool;
        get_delta_neutral_short_sale_slot_alias set_delta_neutral_short_sale_slot_alias deltaNeutralShortSaleSlot delta_neutral_short_sale_slot i32;
        get_discretionary_amt_alias set_discretionary_amt_alias discretionaryAmt discretionary_amt f64;
        get_discretionary_up_to_limit_price_alias set_discretionary_up_to_limit_price_alias discretionaryUpToLimitPrice discretionary_up_to_limit_price bool;
        get_display_size_alias set_display_size_alias displaySize display_size i32;
        get_dont_use_auto_price_for_hedge_alias set_dont_use_auto_price_for_hedge_alias dontUseAutoPriceForHedge dont_use_auto_price_for_hedge bool;
        get_exempt_code_alias set_exempt_code_alias exemptCode exempt_code i32;
        get_filled_quantity_alias set_filled_quantity_alias filledQuantity filled_quantity f64;
        get_ignore_open_auction_alias set_ignore_open_auction_alias ignoreOpenAuction ignore_open_auction bool;
        get_imbalance_only_alias set_imbalance_only_alias imbalanceOnly imbalance_only bool;
        get_include_overnight_alias set_include_overnight_alias includeOvernight include_overnight bool;
        get_is_oms_container_alias set_is_oms_container_alias isOmsContainer is_oms_container bool;
        get_is_pegged_change_amount_decrease_alias set_is_pegged_change_amount_decrease_alias isPeggedChangeAmountDecrease is_pegged_change_amount_decrease bool;
        get_lmt_price_offset_alias set_lmt_price_offset_alias lmtPriceOffset lmt_price_offset f64;
        get_manual_order_indicator_alias set_manual_order_indicator_alias manualOrderIndicator manual_order_indicator i32;
        get_mid_offset_at_half_alias set_mid_offset_at_half_alias midOffsetAtHalf mid_offset_at_half f64;
        get_mid_offset_at_whole_alias set_mid_offset_at_whole_alias midOffsetAtWhole mid_offset_at_whole f64;
        get_min_compete_size_alias set_min_compete_size_alias minCompeteSize min_compete_size i32;
        get_min_qty_alias set_min_qty_alias minQty min_qty i32;
        get_min_trade_qty_alias set_min_trade_qty_alias minTradeQty min_trade_qty i32;
        get_not_held_alias set_not_held_alias notHeld not_held bool;
        get_oca_type_alias set_oca_type_alias ocaType oca_type i32;
        get_opt_out_smart_routing_alias set_opt_out_smart_routing_alias optOutSmartRouting opt_out_smart_routing bool;
        get_outside_rth_alias set_outside_rth_alias outsideRth outside_rth bool;
        get_override_percentage_constraints_alias set_override_percentage_constraints_alias overridePercentageConstraints override_percentage_constraints bool;
        get_parent_id_alias set_parent_id_alias parentId parent_id i64;
        get_parent_perm_id_alias set_parent_perm_id_alias parentPermId parent_perm_id i64;
        get_pegged_change_amount_alias set_pegged_change_amount_alias peggedChangeAmount pegged_change_amount f64;
        get_percent_offset_alias set_percent_offset_alias percentOffset percent_offset f64;
        get_perm_id_alias set_perm_id_alias permId perm_id i64;
        get_post_only_alias set_post_only_alias postOnly post_only bool;
        get_post_to_ats_alias set_post_to_ats_alias postToAts post_to_ats i32;
        get_professional_customer_alias set_professional_customer_alias professionalCustomer professional_customer bool;
        get_pt_order_id_alias set_pt_order_id_alias ptOrderId pt_order_id i32;
        get_randomize_price_alias set_randomize_price_alias randomizePrice randomize_price bool;
        get_randomize_size_alias set_randomize_size_alias randomizeSize randomize_size bool;
        get_ref_futures_con_id_alias set_ref_futures_con_id_alias refFuturesConId ref_futures_con_id i32;
        get_reference_change_amount_alias set_reference_change_amount_alias referenceChangeAmount reference_change_amount f64;
        get_reference_contract_id_alias set_reference_contract_id_alias referenceContractId reference_contract_id i32;
        get_reference_price_type_alias set_reference_price_type_alias referencePriceType reference_price_type i32;
        get_route_marketable_to_bbo_alias set_route_marketable_to_bbo_alias routeMarketableToBbo route_marketable_to_bbo bool;
        get_scale_auto_reset_alias set_scale_auto_reset_alias scaleAutoReset scale_auto_reset bool;
        get_scale_init_fill_qty_alias set_scale_init_fill_qty_alias scaleInitFillQty scale_init_fill_qty i32;
        get_scale_init_level_size_alias set_scale_init_level_size_alias scaleInitLevelSize scale_init_level_size i32;
        get_scale_init_position_alias set_scale_init_position_alias scaleInitPosition scale_init_position i32;
        get_scale_price_adjust_interval_alias set_scale_price_adjust_interval_alias scalePriceAdjustInterval scale_price_adjust_interval i32;
        get_scale_price_adjust_value_alias set_scale_price_adjust_value_alias scalePriceAdjustValue scale_price_adjust_value f64;
        get_scale_price_increment_alias set_scale_price_increment_alias scalePriceIncrement scale_price_increment f64;
        get_scale_profit_offset_alias set_scale_profit_offset_alias scaleProfitOffset scale_profit_offset f64;
        get_scale_random_percent_alias set_scale_random_percent_alias scaleRandomPercent scale_random_percent bool;
        get_scale_subs_level_size_alias set_scale_subs_level_size_alias scaleSubsLevelSize scale_subs_level_size i32;
        get_seek_price_improvement_alias set_seek_price_improvement_alias seekPriceImprovement seek_price_improvement bool;
        get_short_sale_slot_alias set_short_sale_slot_alias shortSaleSlot short_sale_slot i32;
        get_sl_order_id_alias set_sl_order_id_alias slOrderId sl_order_id i32;
        get_starting_price_alias set_starting_price_alias startingPrice starting_price f64;
        get_stock_range_lower_alias set_stock_range_lower_alias stockRangeLower stock_range_lower f64;
        get_stock_range_upper_alias set_stock_range_upper_alias stockRangeUpper stock_range_upper f64;
        get_stock_ref_price_alias set_stock_ref_price_alias stockRefPrice stock_ref_price f64;
        get_sweep_to_fill_alias set_sweep_to_fill_alias sweepToFill sweep_to_fill bool;
        get_trail_stop_price_alias set_trail_stop_price_alias trailStopPrice trail_stop_price f64;
        get_trailing_percent_alias set_trailing_percent_alias trailingPercent trailing_percent f64;
        get_trigger_method_alias set_trigger_method_alias triggerMethod trigger_method i32;
        get_trigger_price_alias set_trigger_price_alias triggerPrice trigger_price f64;
        get_use_price_mgmt_algo_alias set_use_price_mgmt_algo_alias usePriceMgmtAlgo use_price_mgmt_algo i32;
        get_volatility_type_alias set_volatility_type_alias volatilityType volatility_type i32;
        get_what_if_alias set_what_if_alias whatIf what_if bool;
        get_what_if_type_alias set_what_if_type_alias whatIfType what_if_type i32;
    }
}

camel_aliases_owned! {
    Order {
        get_order_type_alias set_order_type_alias orderType order_type String;
        get_active_stop_time_alias set_active_stop_time_alias activeStopTime active_stop_time String;
        get_adjusted_order_type_alias set_adjusted_order_type_alias adjustedOrderType adjusted_order_type String;
        get_advanced_error_override_alias set_advanced_error_override_alias advancedErrorOverride advanced_error_override String;
        get_algo_id_alias set_algo_id_alias algoId algo_id String;
        get_algo_strategy_alias set_algo_strategy_alias algoStrategy algo_strategy String;
        get_auto_cancel_date_alias set_auto_cancel_date_alias autoCancelDate auto_cancel_date String;
        get_bond_accrued_interest_alias set_bond_accrued_interest_alias bondAccruedInterest bond_accrued_interest String;
        get_clearing_account_alias set_clearing_account_alias clearingAccount clearing_account String;
        get_clearing_intent_alias set_clearing_intent_alias clearingIntent clearing_intent String;
        get_customer_account_alias set_customer_account_alias customerAccount customer_account String;
        get_delta_neutral_clearing_account_alias set_delta_neutral_clearing_account_alias deltaNeutralClearingAccount delta_neutral_clearing_account String;
        get_delta_neutral_clearing_intent_alias set_delta_neutral_clearing_intent_alias deltaNeutralClearingIntent delta_neutral_clearing_intent String;
        get_delta_neutral_designated_location_alias set_delta_neutral_designated_location_alias deltaNeutralDesignatedLocation delta_neutral_designated_location String;
        get_delta_neutral_open_close_alias set_delta_neutral_open_close_alias deltaNeutralOpenClose delta_neutral_open_close String;
        get_delta_neutral_order_type_alias set_delta_neutral_order_type_alias deltaNeutralOrderType delta_neutral_order_type String;
        get_delta_neutral_settling_firm_alias set_delta_neutral_settling_firm_alias deltaNeutralSettlingFirm delta_neutral_settling_firm String;
        get_designated_location_alias set_designated_location_alias designatedLocation designated_location String;
        get_ext_operator_alias set_ext_operator_alias extOperator ext_operator String;
        get_fa_group_alias set_fa_group_alias faGroup fa_group String;
        get_fa_method_alias set_fa_method_alias faMethod fa_method String;
        get_fa_percentage_alias set_fa_percentage_alias faPercentage fa_percentage String;
        get_good_after_time_alias set_good_after_time_alias goodAfterTime good_after_time String;
        get_good_till_date_alias set_good_till_date_alias goodTillDate good_till_date String;
        get_hedge_param_alias set_hedge_param_alias hedgeParam hedge_param String;
        get_hedge_type_alias set_hedge_type_alias hedgeType hedge_type String;
        get_manual_order_time_alias set_manual_order_time_alias manualOrderTime manual_order_time String;
        get_mifid2_decision_algo_alias set_mifid2_decision_algo_alias mifid2DecisionAlgo mifid2_decision_algo String;
        get_mifid2_decision_maker_alias set_mifid2_decision_maker_alias mifid2DecisionMaker mifid2_decision_maker String;
        get_mifid2_execution_algo_alias set_mifid2_execution_algo_alias mifid2ExecutionAlgo mifid2_execution_algo String;
        get_mifid2_execution_trader_alias set_mifid2_execution_trader_alias mifid2ExecutionTrader mifid2_execution_trader String;
        get_model_code_alias set_model_code_alias modelCode model_code String;
        get_oca_group_alias set_oca_group_alias ocaGroup oca_group String;
        get_open_close_alias set_open_close_alias openClose open_close String;
        get_order_ref_alias set_order_ref_alias orderRef order_ref String;
        get_pt_order_type_alias set_pt_order_type_alias ptOrderType pt_order_type String;
        get_reference_exchange_id_alias set_reference_exchange_id_alias referenceExchangeId reference_exchange_id String;
        get_rule80a_alias set_rule80a_alias rule80A rule80a String;
        get_scale_table_alias set_scale_table_alias scaleTable scale_table String;
        get_settling_firm_alias set_settling_firm_alias settlingFirm settling_firm String;
        get_sl_order_type_alias set_sl_order_type_alias slOrderType sl_order_type String;
    }
}
