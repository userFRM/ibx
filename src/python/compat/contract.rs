//! ibapi-compatible Contract, Order, TagValue, and condition classes.

use pyo3::prelude::*;

use crate::types::*;
use super::super::types::PRICE_SCALE_F;

// ── Contract ──

/// ibapi-compatible Contract class.
#[pyclass(from_py_object)]
pub struct Contract {
    #[pyo3(get, set)]
    pub con_id: i64,
    #[pyo3(get, set)]
    pub symbol: String,
    #[pyo3(get, set)]
    pub sec_type: String,
    #[pyo3(get, set)]
    pub exchange: String,
    #[pyo3(get, set)]
    pub currency: String,
    #[pyo3(get, set)]
    pub last_trade_date_or_contract_month: String,
    #[pyo3(get, set)]
    pub last_trade_date: String,
    #[pyo3(get, set)]
    pub strike: f64,
    #[pyo3(get, set)]
    pub right: String,
    #[pyo3(get, set)]
    pub multiplier: String,
    #[pyo3(get, set)]
    pub local_symbol: String,
    #[pyo3(get, set)]
    pub primary_exchange: String,
    #[pyo3(get, set)]
    pub trading_class: String,
    #[pyo3(get, set)]
    pub include_expired: bool,
    #[pyo3(get, set)]
    pub sec_id_type: String,
    #[pyo3(get, set)]
    pub sec_id: String,
    #[pyo3(get, set)]
    pub description: String,
    #[pyo3(get, set)]
    pub issuer_id: String,
    #[pyo3(get, set)]
    pub combo_legs_descrip: String,
    #[pyo3(get, set)]
    pub combo_legs: Vec<Py<PyAny>>,
    #[pyo3(get, set)]
    pub delta_neutral_contract: Option<Py<PyAny>>,
}

impl Clone for Contract {
    fn clone(&self) -> Self {
        Self {
            con_id: self.con_id,
            symbol: self.symbol.clone(),
            sec_type: self.sec_type.clone(),
            exchange: self.exchange.clone(),
            currency: self.currency.clone(),
            last_trade_date_or_contract_month: self.last_trade_date_or_contract_month.clone(),
            last_trade_date: self.last_trade_date.clone(),
            strike: self.strike,
            right: self.right.clone(),
            multiplier: self.multiplier.clone(),
            local_symbol: self.local_symbol.clone(),
            primary_exchange: self.primary_exchange.clone(),
            trading_class: self.trading_class.clone(),
            include_expired: self.include_expired,
            sec_id_type: self.sec_id_type.clone(),
            sec_id: self.sec_id.clone(),
            description: self.description.clone(),
            issuer_id: self.issuer_id.clone(),
            combo_legs_descrip: self.combo_legs_descrip.clone(),
            // Legs are Python objects here; reading them needs the interpreter,
            // which `to_api` does not take. `combo_legs_api` does that, and the
            // order path calls it.
            combo_legs: Vec::new(),
            delta_neutral_contract: None,
        }
    }
}

impl Default for Contract {
    fn default() -> Self {
        Self {
            con_id: 0,
            symbol: String::new(),
            sec_type: "STK".into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
            last_trade_date_or_contract_month: String::new(),
            last_trade_date: String::new(),
            strike: 0.0,
            right: String::new(),
            multiplier: String::new(),
            local_symbol: String::new(),
            primary_exchange: String::new(),
            trading_class: String::new(),
            include_expired: false,
            sec_id_type: String::new(),
            sec_id: String::new(),
            description: String::new(),
            issuer_id: String::new(),
            combo_legs_descrip: String::new(),
            combo_legs: Vec::new(),
            delta_neutral_contract: None,
        }
    }
}

impl Contract {
    /// The legs, read out of the Python objects that hold them.
    ///
    /// Each is any object with the ibapi ComboLeg attribute names, so a plain
    /// `ibapi.contract.ComboLeg` works and so does anything shaped like one. A
    /// leg that is missing an attribute contributes its default rather than
    /// failing the order, except the contract id, without which the leg names
    /// nothing and the whole list is refused.
    pub fn combo_legs_api(&self, py: Python<'_>) -> Result<Vec<crate::api::types::ComboLeg>, String> {
        let mut out = Vec::with_capacity(self.combo_legs.len());
        for (i, obj) in self.combo_legs.iter().enumerate() {
            let g = |n: &str| obj.getattr(py, n).ok();
            let con_id = g("conId").and_then(|v| v.extract::<i64>(py).ok()).unwrap_or(0);
            if con_id == 0 {
                return Err(format!("combo leg {i} has no conId, so it names no contract"));
            }
            out.push(crate::api::types::ComboLeg {
                con_id,
                ratio: g("ratio").and_then(|v| v.extract(py).ok()).unwrap_or(1),
                action: g("action").and_then(|v| v.extract(py).ok()).unwrap_or_default(),
                exchange: g("exchange").and_then(|v| v.extract(py).ok()).unwrap_or_default(),
                open_close: g("openClose").and_then(|v| v.extract(py).ok()).unwrap_or(0),
                shorting_policy: g("shortSaleSlot").and_then(|v| v.extract(py).ok()).unwrap_or(0),
                designated_location: g("designatedLocation")
                    .and_then(|v| v.extract(py).ok()).unwrap_or_default(),
                exempt_code: g("exemptCode").and_then(|v| v.extract(py).ok()).unwrap_or(-1),
            });
        }
        Ok(out)
    }


    /// The whole contract the engine holds, not the handful of fields a
    /// callback happens to print: an option with no strike, right or expiry
    /// names nothing the caller can act on. Combo legs and the delta-neutral
    /// contract need a `Python` token to build and are dropped here exactly as
    /// `Clone` drops them.
    pub(crate) fn from_api(c: &crate::api::types::Contract) -> Self {
        Self {
            con_id: c.con_id,
            symbol: c.symbol.clone(),
            sec_type: c.sec_type.clone(),
            exchange: c.exchange.clone(),
            currency: c.currency.clone(),
            last_trade_date_or_contract_month: c.last_trade_date_or_contract_month.clone(),
            last_trade_date: c.last_trade_date.clone(),
            strike: c.strike,
            right: c.right.clone(),
            multiplier: c.multiplier.clone(),
            local_symbol: c.local_symbol.clone(),
            primary_exchange: c.primary_exchange.clone(),
            trading_class: c.trading_class.clone(),
            include_expired: c.include_expired,
            sec_id_type: c.sec_id_type.clone(),
            sec_id: c.sec_id.clone(),
            description: c.description.clone(),
            issuer_id: c.issuer_id.clone(),
            combo_legs_descrip: c.combo_legs_descrip.clone(),
            combo_legs: Vec::new(),
            delta_neutral_contract: None,
        }
    }

    /// The same contract in the shape the rest of the client uses.
    ///
    /// Combo legs and the delta-neutral contract are dropped, exactly as
    /// `from_api` drops them: they need a `Python` token to read.
    pub(crate) fn to_api(&self) -> crate::api::types::Contract {
        crate::api::types::Contract {
            con_id: self.con_id,
            symbol: self.symbol.clone(),
            sec_type: self.sec_type.clone(),
            exchange: self.exchange.clone(),
            currency: self.currency.clone(),
            last_trade_date_or_contract_month: self.last_trade_date_or_contract_month.clone(),
            last_trade_date: self.last_trade_date.clone(),
            strike: self.strike,
            right: self.right.clone(),
            multiplier: self.multiplier.clone(),
            local_symbol: self.local_symbol.clone(),
            primary_exchange: self.primary_exchange.clone(),
            trading_class: self.trading_class.clone(),
            include_expired: self.include_expired,
            sec_id_type: self.sec_id_type.clone(),
            sec_id: self.sec_id.clone(),
            description: self.description.clone(),
            issuer_id: self.issuer_id.clone(),
            combo_legs_descrip: self.combo_legs_descrip.clone(),
            ..Default::default()
        }
    }
}


/// Set a field on a freshly made object from what a caller named it.
///
/// The reference client names a contract's fields one way and this binding
/// holds them under both, so a program written against that client can set
/// them by the names it already uses. The constructor took only one spelling,
/// which meant the very first line of a ported program — a contract with a
/// `secType` on it — failed before anything reached the venue.
fn set_from_keywords(
    object: &Bound<'_, PyAny>,
    keywords: Option<&Bound<'_, pyo3::types::PyDict>>,
) -> PyResult<()> {
    let Some(keywords) = keywords else { return Ok(()) };
    for (name, value) in keywords.iter() {
        let name: String = name.extract()?;
        if object.setattr(name.as_str(), &value).is_err() {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                "no such field: {name}",
            )));
        }
    }
    Ok(())
}

#[pymethods]
impl Contract {
    #[new]
    #[pyo3(signature = (con_id=0, symbol="".to_string(), sec_type="STK".to_string(), exchange="SMART".to_string(), currency="USD".to_string(), last_trade_date_or_contract_month="".to_string(), strike=0.0, right="".to_string(), multiplier="".to_string(), local_symbol="".to_string(), primary_exchange="".to_string(), trading_class="".to_string(), **keywords))]
    fn new(
        con_id: i64,
        symbol: String,
        sec_type: String,
        exchange: String,
        currency: String,
        last_trade_date_or_contract_month: String,
        strike: f64,
        right: String,
        multiplier: String,
        local_symbol: String,
        primary_exchange: String,
        trading_class: String,
        keywords: Option<&Bound<'_, pyo3::types::PyDict>>,
        py: Python<'_>,
    ) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self {
            con_id,
            symbol,
            sec_type,
            exchange,
            currency,
            last_trade_date_or_contract_month,
            strike,
            right,
            multiplier,
            local_symbol,
            primary_exchange,
            trading_class,
            ..Default::default()
        })?;
        // And whatever else the caller named, under either spelling.
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        format!("Contract(conId={}, symbol='{}', secType='{}', exchange='{}')",
            self.con_id, self.symbol, self.sec_type, self.exchange)
    }

    // ibapi camelCase aliases
    #[getter(conId)]
    fn get_con_id_alias(&self) -> i64 { self.con_id }
    #[setter(conId)]
    fn set_con_id_alias(&mut self, v: i64) { self.con_id = v; }
    #[getter(secType)]
    fn get_sec_type_alias(&self) -> String { self.sec_type.clone() }
    #[setter(secType)]
    fn set_sec_type_alias(&mut self, v: String) { self.sec_type = v; }
    #[getter(lastTradeDateOrContractMonth)]
    fn get_ltdocm_alias(&self) -> String { self.last_trade_date_or_contract_month.clone() }
    #[setter(lastTradeDateOrContractMonth)]
    fn set_ltdocm_alias(&mut self, v: String) { self.last_trade_date_or_contract_month = v; }
    #[getter(lastTradeDate)]
    fn get_ltd_alias(&self) -> String { self.last_trade_date.clone() }
    #[setter(lastTradeDate)]
    fn set_ltd_alias(&mut self, v: String) { self.last_trade_date = v; }
    #[getter(localSymbol)]
    fn get_local_symbol_alias(&self) -> String { self.local_symbol.clone() }
    #[setter(localSymbol)]
    fn set_local_symbol_alias(&mut self, v: String) { self.local_symbol = v; }
    #[getter(primaryExchange)]
    fn get_primary_exchange_alias(&self) -> String { self.primary_exchange.clone() }
    #[setter(primaryExchange)]
    fn set_primary_exchange_alias(&mut self, v: String) { self.primary_exchange = v; }
    #[getter(tradingClass)]
    fn get_trading_class_alias(&self) -> String { self.trading_class.clone() }
    #[setter(tradingClass)]
    fn set_trading_class_alias(&mut self, v: String) { self.trading_class = v; }
    #[getter(includeExpired)]
    fn get_include_expired_alias(&self) -> bool { self.include_expired }
    #[setter(includeExpired)]
    fn set_include_expired_alias(&mut self, v: bool) { self.include_expired = v; }
    #[getter(secIdType)]
    fn get_sec_id_type_alias(&self) -> String { self.sec_id_type.clone() }
    #[setter(secIdType)]
    fn set_sec_id_type_alias(&mut self, v: String) { self.sec_id_type = v; }
    #[getter(secId)]
    fn get_sec_id_alias(&self) -> String { self.sec_id.clone() }
    #[setter(secId)]
    fn set_sec_id_alias(&mut self, v: String) { self.sec_id = v; }
    #[getter(issuerId)]
    fn get_issuer_id_alias(&self) -> String { self.issuer_id.clone() }
    #[setter(issuerId)]
    fn set_issuer_id_alias(&mut self, v: String) { self.issuer_id = v; }
    #[getter(comboLegsDescrip)]
    fn get_combo_legs_descrip_alias(&self) -> String { self.combo_legs_descrip.clone() }
    #[setter(comboLegsDescrip)]
    fn set_combo_legs_descrip_alias(&mut self, v: String) { self.combo_legs_descrip = v; }
    #[getter(comboLegs)]
    fn get_combo_legs_alias(&self) -> Vec<Py<PyAny>> { Vec::new() }
    #[getter(deltaNeutralContract)]
    fn get_delta_neutral_alias(&self) -> Option<Py<PyAny>> { None }
}

// ── Order ──

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
    #[allow(clippy::too_many_arguments)]
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
    #[getter(orderComboLegs)]
    fn get_order_combo_legs_alias(&self) -> Vec<Py<PyAny>> { Vec::new() }
    #[getter(orderMiscOptions)]
    fn get_order_misc_options_alias(&self) -> Vec<Py<PyAny>> { Vec::new() }
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
    #[getter(softDollarTier)]
    fn get_soft_dollar_tier(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let tier = SoftDollarTierPy {
            name: self.soft_dollar_tier_name.clone(),
            val: self.soft_dollar_tier_val.clone(),
            display_name: self.soft_dollar_tier_display_name.clone(),
        };
        Ok(Py::new(py, tier)?.into_any())
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

// ── Conversions between Python compat types and Rust API types ──

impl Order {
    /// Convert Py<PyAny> conditions to internal OrderCondition list.
    pub fn convert_conditions(&self, py: Python<'_>) -> Vec<OrderCondition> {
        self.conditions.iter().filter_map(|obj| {
            let any = obj.bind(py);
            if let Ok(c) = any.cast::<PriceCondition>() { return Some(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<TimeCondition>() { return Some(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<MarginCondition>() { return Some(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<ExecutionCondition>() { return Some(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<VolumeCondition>() { return Some(c.borrow().to_internal()); }
            if let Ok(c) = any.cast::<PercentChangeCondition>() { return Some(c.borrow().to_internal()); }
            log::warn!("Unknown order condition type, skipping");
            None
        }).collect()
    }

    /// Convert to Rust API Order.
    pub fn to_api(&self) -> crate::api::types::Order {
        crate::api::types::Order {
            order_id: self.order_id,
            action: self.action.clone(),
            total_quantity: self.total_quantity,
            order_type: self.order_type.clone(),
            lmt_price: self.lmt_price,
            aux_price: self.aux_price,
            // Unset is f64::MAX on both sides, so leaving it to Default made a
            // caller's offset indistinguishable from absent and the wire fell
            // back to lmt_price: a TRAIL LIMIT could not set its offset from
            // Python at all (ibx#395).
            lmt_price_offset: self.lmt_price_offset,
            tif: self.tif.clone(),
            outside_rth: self.outside_rth,
            display_size: self.display_size,
            min_qty: self.min_qty,
            hidden: self.hidden,
            good_after_time: self.good_after_time.clone(),
            good_till_date: self.good_till_date.clone(),
            oca_group: self.oca_group.clone(),
            oca_type: self.oca_type,
            trail_stop_price: self.trail_stop_price,
            trailing_percent: self.trailing_percent,
            algo_strategy: self.algo_strategy.clone(),
            algo_params: self.algo_params.iter().map(|tv| crate::api::types::TagValue {
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
            algo_id: self.algo_id.clone(),
            perm_id: self.perm_id,
            client_id: self.client_id,
            order_ref: self.order_ref.clone(),
            ..Default::default()
        }
    }
}

// ── TagValue ──

/// ibapi-compatible TagValue for algo parameters.
#[pyclass(from_py_object)]
#[derive(Clone, Debug)]
pub struct TagValue {
    #[pyo3(get, set)]
    pub tag: String,
    #[pyo3(get, set)]
    pub value: String,
}

#[pymethods]
impl TagValue {
    #[new]
    fn new(tag: String, value: String) -> Self {
        Self { tag, value }
    }

    fn __repr__(&self) -> String {
        format!("TagValue(tag='{}', value='{}')", self.tag, self.value)
    }
}

// ── OrderAllocation (per-account allocation in OrderState) ──

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

// ── OrderState (for what-if responses) ──

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

// ── Order Conditions ──

/// Price condition: trigger when an instrument's price crosses a threshold.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PriceCondition {
    #[pyo3(get, set)]
    pub con_id: i64,
    #[pyo3(get, set)]
    pub exchange: String,
    #[pyo3(get, set)]
    pub price: f64,
    #[pyo3(get, set)]
    pub is_more: bool,
    #[pyo3(get, set)]
    pub trigger_method: i32,
}

#[pymethods]
impl PriceCondition {
    // The reference client spells these by running the words together, and
    // names an exchange `exch`. A condition built the way that client builds
    // it sets them by those names.
    #[getter(conId)]
    fn get_con_id_alias(&self) -> i64 { self.con_id }
    #[setter(conId)]
    fn set_con_id_alias(&mut self, v: i64) { self.con_id = v; }
    #[getter(exch)]
    fn get_exchange_alias(&self) -> String { self.exchange.clone() }
    #[setter(exch)]
    fn set_exchange_alias(&mut self, v: String) { self.exchange = v; }
    #[getter(isMore)]
    fn get_is_more_alias(&self) -> bool { self.is_more }
    #[setter(isMore)]
    fn set_is_more_alias(&mut self, v: bool) { self.is_more = v; }
    #[getter(triggerMethod)]
    fn get_trigger_method_alias(&self) -> i32 { self.trigger_method }
    #[setter(triggerMethod)]
    fn set_trigger_method_alias(&mut self, v: i32) { self.trigger_method = v; }

    #[new]
    #[pyo3(signature = (con_id=0, exchange="SMART".to_string(), price=0.0, is_more=true, trigger_method=0, **keywords))]
    fn new(con_id: i64, exchange: String, price: f64, is_more: bool, trigger_method: i32, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { con_id, exchange, price, is_more, trigger_method })?;
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        let op = if self.is_more { ">" } else { "<" };
        format!("PriceCondition(conId={}, price {} {})", self.con_id, op, self.price)
    }
}

impl PriceCondition {
    pub fn to_internal(&self) -> OrderCondition {
        OrderCondition::Price {
            con_id: self.con_id,
            exchange: self.exchange.clone(),
            price: (self.price * PRICE_SCALE_F) as Price,
            is_more: self.is_more,
            trigger_method: self.trigger_method as u8,
        }
    }
}

/// Time condition: trigger at a specific time.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct TimeCondition {
    #[pyo3(get, set)]
    pub time: String,
    #[pyo3(get, set)]
    pub is_more: bool,
}

#[pymethods]
impl TimeCondition {
    // The reference client spells these by running the words together, and
    // names an exchange `exch`. A condition built the way that client builds
    // it sets them by those names.
    #[getter(isMore)]
    fn get_is_more_alias(&self) -> bool { self.is_more }
    #[setter(isMore)]
    fn set_is_more_alias(&mut self, v: bool) { self.is_more = v; }

    #[new]
    #[pyo3(signature = (time="".to_string(), is_more=true, **keywords))]
    fn new(time: String, is_more: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { time, is_more })?;
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        let op = if self.is_more { ">" } else { "<" };
        format!("TimeCondition(time {} '{}')", op, self.time)
    }
}

impl TimeCondition {
    pub fn to_internal(&self) -> OrderCondition {
        OrderCondition::Time { time: self.time.clone(), is_more: self.is_more }
    }
}

/// Margin condition: trigger based on margin cushion percentage.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct MarginCondition {
    #[pyo3(get, set)]
    pub percent: u32,
    #[pyo3(get, set)]
    pub is_more: bool,
}

#[pymethods]
impl MarginCondition {
    // The reference client spells these by running the words together, and
    // names an exchange `exch`. A condition built the way that client builds
    // it sets them by those names.
    #[getter(isMore)]
    fn get_is_more_alias(&self) -> bool { self.is_more }
    #[setter(isMore)]
    fn set_is_more_alias(&mut self, v: bool) { self.is_more = v; }

    #[new]
    #[pyo3(signature = (percent=0, is_more=true, **keywords))]
    fn new(percent: u32, is_more: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { percent, is_more })?;
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        format!("MarginCondition({}% {})", self.percent, if self.is_more { "above" } else { "below" })
    }
}

impl MarginCondition {
    pub fn to_internal(&self) -> OrderCondition {
        OrderCondition::Margin { percent: self.percent, is_more: self.is_more }
    }
}

/// Execution condition: trigger on trade execution.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct ExecutionCondition {
    #[pyo3(get, set)]
    pub symbol: String,
    #[pyo3(get, set)]
    pub exchange: String,
    #[pyo3(get, set)]
    pub sec_type: String,
}

#[pymethods]
impl ExecutionCondition {
    // The reference client spells these by running the words together, and
    // names an exchange `exch`. A condition built the way that client builds
    // it sets them by those names.
    #[getter(exch)]
    fn get_exchange_alias(&self) -> String { self.exchange.clone() }
    #[setter(exch)]
    fn set_exchange_alias(&mut self, v: String) { self.exchange = v; }
    #[getter(secType)]
    fn get_sec_type_alias(&self) -> String { self.sec_type.clone() }
    #[setter(secType)]
    fn set_sec_type_alias(&mut self, v: String) { self.sec_type = v; }

    #[new]
    #[pyo3(signature = (symbol="".to_string(), exchange="".to_string(), sec_type="".to_string(), **keywords))]
    fn new(symbol: String, exchange: String, sec_type: String, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { symbol, exchange, sec_type })?;
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        format!("ExecutionCondition(symbol='{}', exchange='{}')", self.symbol, self.exchange)
    }
}

impl ExecutionCondition {
    pub fn to_internal(&self) -> OrderCondition {
        OrderCondition::Execution {
            symbol: self.symbol.clone(),
            exchange: self.exchange.clone(),
            sec_type: self.sec_type.clone(),
        }
    }
}

/// Volume condition: trigger when volume exceeds a threshold.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct VolumeCondition {
    #[pyo3(get, set)]
    pub con_id: i64,
    #[pyo3(get, set)]
    pub exchange: String,
    #[pyo3(get, set)]
    pub volume: i64,
    #[pyo3(get, set)]
    pub is_more: bool,
}

#[pymethods]
impl VolumeCondition {
    // The reference client spells these by running the words together, and
    // names an exchange `exch`. A condition built the way that client builds
    // it sets them by those names.
    #[getter(conId)]
    fn get_con_id_alias(&self) -> i64 { self.con_id }
    #[setter(conId)]
    fn set_con_id_alias(&mut self, v: i64) { self.con_id = v; }
    #[getter(exch)]
    fn get_exchange_alias(&self) -> String { self.exchange.clone() }
    #[setter(exch)]
    fn set_exchange_alias(&mut self, v: String) { self.exchange = v; }
    #[getter(isMore)]
    fn get_is_more_alias(&self) -> bool { self.is_more }
    #[setter(isMore)]
    fn set_is_more_alias(&mut self, v: bool) { self.is_more = v; }

    #[new]
    #[pyo3(signature = (con_id=0, exchange="SMART".to_string(), volume=0, is_more=true, **keywords))]
    fn new(con_id: i64, exchange: String, volume: i64, is_more: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { con_id, exchange, volume, is_more })?;
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        let op = if self.is_more { ">" } else { "<" };
        format!("VolumeCondition(conId={}, volume {} {})", self.con_id, op, self.volume)
    }
}

impl VolumeCondition {
    pub fn to_internal(&self) -> OrderCondition {
        OrderCondition::Volume {
            con_id: self.con_id,
            exchange: self.exchange.clone(),
            volume: self.volume,
            is_more: self.is_more,
        }
    }
}

/// Percentage change condition: trigger on % change from close.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PercentChangeCondition {
    #[pyo3(get, set)]
    pub con_id: i64,
    #[pyo3(get, set)]
    pub exchange: String,
    #[pyo3(get, set)]
    pub change_percent: f64,
    #[pyo3(get, set)]
    pub is_more: bool,
}

#[pymethods]
impl PercentChangeCondition {
    // The reference client spells these by running the words together, and
    // names an exchange `exch`. A condition built the way that client builds
    // it sets them by those names.
    #[getter(conId)]
    fn get_con_id_alias(&self) -> i64 { self.con_id }
    #[setter(conId)]
    fn set_con_id_alias(&mut self, v: i64) { self.con_id = v; }
    #[getter(exch)]
    fn get_exchange_alias(&self) -> String { self.exchange.clone() }
    #[setter(exch)]
    fn set_exchange_alias(&mut self, v: String) { self.exchange = v; }
    #[getter(isMore)]
    fn get_is_more_alias(&self) -> bool { self.is_more }
    #[setter(isMore)]
    fn set_is_more_alias(&mut self, v: bool) { self.is_more = v; }
    #[getter(changePercent)]
    fn get_change_percent_alias(&self) -> f64 { self.change_percent }
    #[setter(changePercent)]
    fn set_change_percent_alias(&mut self, v: f64) { self.change_percent = v; }

    #[new]
    #[pyo3(signature = (con_id=0, exchange="SMART".to_string(), change_percent=0.0, is_more=true, **keywords))]
    fn new(con_id: i64, exchange: String, change_percent: f64, is_more: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { con_id, exchange, change_percent, is_more })?;
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        let op = if self.is_more { ">" } else { "<" };
        format!("PercentChangeCondition(conId={}, {}% {})", self.con_id, op, self.change_percent)
    }
}

impl PercentChangeCondition {
    pub fn to_internal(&self) -> OrderCondition {
        OrderCondition::PercentChange {
            con_id: self.con_id,
            exchange: self.exchange.clone(),
            percent: self.change_percent,
            is_more: self.is_more,
        }
    }
}

// ── BarData ──

/// ibapi-compatible BarData class for historical data callbacks.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct BarData {
    #[pyo3(get, set)]
    pub date: String,
    #[pyo3(get, set)]
    pub open: f64,
    #[pyo3(get, set)]
    pub high: f64,
    #[pyo3(get, set)]
    pub low: f64,
    #[pyo3(get, set)]
    pub close: f64,
    #[pyo3(get, set)]
    pub volume: i64,
    #[pyo3(get, set)]
    pub wap: f64,
    #[pyo3(get, set)]
    pub bar_count: i32,
    /// Timezone of `date` as reported by the reply (ibx#234) — previously
    /// parsed and then discarded, leaving the bare timestamp string as the
    /// only (unverifiable) evidence of what the bar times mean. Empty on
    /// streaming updates, which carry no timezone of their own.
    #[pyo3(get, set)]
    pub timezone: String,
}

#[pymethods]
impl BarData {
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
    #[pyo3(signature = (date="".to_string(), open=0.0, high=0.0, low=0.0, close=0.0, volume=0, wap=0.0, bar_count=0, timezone="".to_string()))]
    pub fn new(date: String, open: f64, high: f64, low: f64, close: f64, volume: i64, wap: f64, bar_count: i32, timezone: String) -> Self {
        Self { date, open, high, low, close, volume, wap, bar_count, timezone }
    }

    fn __repr__(&self) -> String {
        format!("BarData(date='{}', O={}, H={}, L={}, C={}, V={})",
            self.date, self.open, self.high, self.low, self.close, self.volume)
    }
}

// ── ContractDetails ──

impl Contract {
    /// What tells two contracts on one underlying apart, for a lookup that
    /// names this one.
    ///
    /// Every request that carries a contract rather than an id needs these:
    /// a lookup for an option by symbol alone answers whichever the venue
    /// lists first, which is a different contract from the one asked about.
    pub(crate) fn lookup_filters(&self) -> crate::types::SecDefFilters {
        crate::types::SecDefFilters {
            primary_exchange: self.primary_exchange.clone(),
            local_symbol: self.local_symbol.clone(),
            last_trade_date_or_contract_month: self.last_trade_date_or_contract_month.clone(),
            strike: self.strike,
            right: self.right.clone(),
            multiplier: self.multiplier.clone(),
            trading_class: self.trading_class.clone(),
            sec_id: self.sec_id.clone(),
            sec_id_type: self.sec_id_type.clone(),
        }
    }
}

/// ibapi-compatible ContractDetails class.
#[pyclass(from_py_object)]
pub struct ContractDetails {
    /// Stored as `Py<Contract>` so the getter hands Python THE contained
    /// object, not a copy: with a plain field, `details.contract.con_id = x`
    /// mutated a temporary clone and was a silent no-op (ibx#230).
    #[pyo3(get, set)]
    pub contract: Py<Contract>,
    #[pyo3(get, set)]
    pub market_name: String,
    #[pyo3(get, set)]
    pub min_tick: f64,
    #[pyo3(get, set)]
    pub order_types: String,
    #[pyo3(get, set)]
    pub valid_exchanges: String,
    #[pyo3(get, set)]
    pub long_name: String,
    #[pyo3(get, set)]
    pub last_trade_date: String,
    #[pyo3(get, set)]
    pub multiplier: String,
    #[pyo3(get, set)]
    pub market_rule_id: i64,
    /// Every price-increment rule this contract trades under, as the venue
    /// states them. The reference client names it in the plural and states
    /// them all; the singular beside it is the first of them, kept because
    /// programs here already read it.
    #[pyo3(get, set)]
    pub market_rule_ids: String,
    #[pyo3(get, set)]
    pub strike: f64,
    #[pyo3(get, set)]
    pub right: String,
    #[pyo3(get, set)]
    pub primary_exchange: String,
    #[pyo3(get, set)]
    pub local_symbol: String,
    #[pyo3(get, set)]
    pub trading_class: String,
    #[pyo3(get, set)]
    pub stock_type: String,
    #[pyo3(get, set)]
    pub category: String,
    #[pyo3(get, set)]
    pub country: String,
    #[pyo3(get, set)]
    pub isin: String,
    #[pyo3(get, set)]
    pub cusip: String,
    #[pyo3(get, set)]
    pub sec_id_list: Vec<(String, String)>,
    #[pyo3(get, set)]
    pub min_size: f64,
    #[pyo3(get, set)]
    pub industry: String,
    #[pyo3(get, set)]
    pub subcategory: String,
    #[pyo3(get, set)]
    pub price_magnifier: i32,
    #[pyo3(get, set)]
    pub contract_month: String,
    #[pyo3(get, set)]
    pub under_sec_type: String,
    #[pyo3(get, set)]
    pub ev_rule: String,
    /// What that evaluation is multiplied by. A rule without its multiplier
    /// values the contract by the wrong factor.
    #[pyo3(get, set)]
    pub ev_multiplier: f64,
    #[pyo3(get, set)]
    pub under_con_id: u32,
    #[pyo3(get, set)]
    pub under_symbol: String,
    #[pyo3(get, set)]
    pub last_trade_time: String,
    #[pyo3(get, set)]
    pub issue_date: String,
    #[pyo3(get, set)]
    pub size_increment: f64,
    #[pyo3(get, set)]
    pub suggested_size_increment: f64,
    #[pyo3(get, set)]
    pub last_price_precision: f64,
    #[pyo3(get, set)]
    pub last_size_precision: f64,
    #[pyo3(get, set)]
    pub settlement_method: String,
    #[pyo3(get, set)]
    pub unnamed_fields: Vec<(u32, String)>,
    #[pyo3(get, set)]
    pub agg_group: i32,
    #[pyo3(get, set)]
    pub coupon: f64,
    #[pyo3(get, set)]
    pub bond_type: String,
    #[pyo3(get, set)]
    pub coupon_type: String,
    #[pyo3(get, set)]
    pub callable: bool,
    #[pyo3(get, set)]
    pub puttable: bool,
    #[pyo3(get, set)]
    pub convertible: bool,
    #[pyo3(get, set)]
    pub next_option_partial: bool,
    #[pyo3(get, set)]
    pub next_option_date: String,
    #[pyo3(get, set)]
    pub next_option_type: String,
    #[pyo3(get, set)]
    pub ratings: String,
    #[pyo3(get, set)]
    pub bond_notes: String,
    #[pyo3(get, set)]
    pub desc_append: String,
    #[pyo3(get, set)]
    pub real_expiration_date: String,
    #[pyo3(get, set)]
    pub fund_name: String,
    #[pyo3(get, set)]
    pub fund_family: String,
    #[pyo3(get, set)]
    pub fund_type: String,
    #[pyo3(get, set)]
    pub fund_front_load: String,
    #[pyo3(get, set)]
    pub fund_back_load: String,
    #[pyo3(get, set)]
    pub fund_back_load_time_interval: String,
    #[pyo3(get, set)]
    pub fund_management_fee: String,
    #[pyo3(get, set)]
    pub fund_closed: bool,
    #[pyo3(get, set)]
    pub fund_closed_for_new_investors: bool,
    #[pyo3(get, set)]
    pub fund_closed_for_new_money: bool,
    #[pyo3(get, set)]
    pub fund_notify_amount: String,
    #[pyo3(get, set)]
    pub fund_minimum_initial_purchase: String,
    #[pyo3(get, set)]
    pub fund_minimum_subsequent_purchase: String,
    #[pyo3(get, set)]
    pub fund_blue_sky_states: String,
    #[pyo3(get, set)]
    pub fund_blue_sky_territories: String,
    #[pyo3(get, set)]
    pub fund_distribution_policy_indicator: String,
    #[pyo3(get, set)]
    pub fund_asset_type: String,
    #[pyo3(get, set)]
    pub trading_hours: String,
    #[pyo3(get, set)]
    pub liquid_hours: String,
    #[pyo3(get, set)]
    pub time_zone_id: String,
}

#[pymethods]
impl ContractDetails {
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
        by_reference_name(
            slf.as_any(),
            name,
            &[
                // Same field, different word: the reference client writes one
                // `t`, calls the bond remark a plain note, and orders the words
                // of the fund's follow-on minimum the other way round.
                ("putable", "puttable"),
                ("notes", "bond_notes"),
                ("fundSubsequentMinimumPurchase", "fund_minimum_subsequent_purchase"),
            ],
        )
    }

    #[new]
    #[pyo3(signature = ())]
    fn py_new(py: Python<'_>) -> Self {
        Self::new_default(py)
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        format!("ContractDetails(symbol='{}', longName='{}')",
            self.contract.borrow(py).symbol, self.long_name)
    }
}

impl Clone for ContractDetails {
    /// `Py<Contract>` clones by reference under the GIL: the copy shares the
    /// same Python Contract object, matching Python assignment semantics.
    fn clone(&self) -> Self {
        Python::attach(|py| Self {
            contract: self.contract.clone_ref(py),
            industry: self.industry.clone(),
            subcategory: self.subcategory.clone(),
            price_magnifier: self.price_magnifier,
            contract_month: self.contract_month.clone(),
            under_sec_type: self.under_sec_type.clone(),
            ev_rule: self.ev_rule.clone(),
            ev_multiplier: self.ev_multiplier,
            under_con_id: self.under_con_id,
            under_symbol: self.under_symbol.clone(),
            last_trade_time: self.last_trade_time.clone(),
            issue_date: self.issue_date.clone(),
            size_increment: self.size_increment,
            suggested_size_increment: self.suggested_size_increment,
            last_price_precision: self.last_price_precision,
            last_size_precision: self.last_size_precision,
            settlement_method: self.settlement_method.clone(),
            unnamed_fields: self.unnamed_fields.clone(),
            agg_group: self.agg_group,
            coupon: self.coupon,
            bond_type: self.bond_type.clone(),
            coupon_type: self.coupon_type.clone(),
            callable: self.callable,
            puttable: self.puttable,
            convertible: self.convertible,
            next_option_partial: self.next_option_partial,
            next_option_date: self.next_option_date.clone(),
            next_option_type: self.next_option_type.clone(),
            ratings: self.ratings.clone(),
            bond_notes: self.bond_notes.clone(),
            desc_append: self.desc_append.clone(),
            real_expiration_date: self.real_expiration_date.clone(),
            fund_name: self.fund_name.clone(),
            fund_family: self.fund_family.clone(),
            fund_type: self.fund_type.clone(),
            fund_front_load: self.fund_front_load.clone(),
            fund_back_load: self.fund_back_load.clone(),
            fund_back_load_time_interval: self.fund_back_load_time_interval.clone(),
            fund_management_fee: self.fund_management_fee.clone(),
            fund_closed: self.fund_closed,
            fund_closed_for_new_investors: self.fund_closed_for_new_investors,
            fund_closed_for_new_money: self.fund_closed_for_new_money,
            fund_notify_amount: self.fund_notify_amount.clone(),
            fund_minimum_initial_purchase: self.fund_minimum_initial_purchase.clone(),
            fund_minimum_subsequent_purchase: self.fund_minimum_subsequent_purchase.clone(),
            fund_blue_sky_states: self.fund_blue_sky_states.clone(),
            fund_blue_sky_territories: self.fund_blue_sky_territories.clone(),
            fund_distribution_policy_indicator: self.fund_distribution_policy_indicator.clone(),
            fund_asset_type: self.fund_asset_type.clone(),
            market_name: self.market_name.clone(),
            min_tick: self.min_tick,
            order_types: self.order_types.clone(),
            valid_exchanges: self.valid_exchanges.clone(),
            long_name: self.long_name.clone(),
            last_trade_date: self.last_trade_date.clone(),
            multiplier: self.multiplier.clone(),
            market_rule_id: self.market_rule_id,
            market_rule_ids: self.market_rule_ids.clone(),
            strike: self.strike,
            right: self.right.clone(),
            primary_exchange: self.primary_exchange.clone(),
            local_symbol: self.local_symbol.clone(),
            trading_class: self.trading_class.clone(),
            stock_type: self.stock_type.clone(),
            category: self.category.clone(),
            country: self.country.clone(),
            isin: self.isin.clone(),
            cusip: self.cusip.clone(),
            sec_id_list: self.sec_id_list.clone(),
            min_size: self.min_size,
            trading_hours: self.trading_hours.clone(),
            liquid_hours: self.liquid_hours.clone(),
            time_zone_id: self.time_zone_id.clone(),
        })
    }
}

impl ContractDetails {
    /// Fresh instance with an owned default Contract. `Py<Contract>` has no
    /// Default, so this replaces the derived constructor (ibx#230).
    pub fn new_default(py: Python<'_>) -> Self {
        Self {
            contract: Py::new(py, Contract::default()).expect("Contract allocation failed"),
            market_name: String::new(),
            min_tick: 0.0,
            order_types: String::new(),
            valid_exchanges: String::new(),
            long_name: String::new(),
            last_trade_date: String::new(),
            multiplier: String::new(),
            market_rule_id: 0,
            market_rule_ids: String::new(),
            strike: 0.0,
            right: String::new(),
            primary_exchange: String::new(),
            local_symbol: String::new(),
            trading_class: String::new(),
            stock_type: String::new(),
            category: String::new(),
            country: String::new(),
            isin: String::new(),
            cusip: String::new(),
            sec_id_list: Vec::new(),
            min_size: 0.0,
            industry: String::new(),
            subcategory: String::new(),
            price_magnifier: 0,
            contract_month: String::new(),
            under_sec_type: String::new(),
            ev_rule: String::new(),
            ev_multiplier: 0.0,
            under_con_id: 0,
            under_symbol: String::new(),
            last_trade_time: String::new(),
            issue_date: String::new(),
            size_increment: 0.0,
            suggested_size_increment: 0.0,
            last_price_precision: 0.0,
            last_size_precision: 0.0,
            settlement_method: String::new(),
            unnamed_fields: Vec::new(),
            agg_group: 0,
            coupon: 0.0,
            bond_type: String::new(),
            coupon_type: String::new(),
            callable: false,
            puttable: false,
            convertible: false,
            next_option_partial: false,
            next_option_date: String::new(),
            next_option_type: String::new(),
            ratings: String::new(),
            bond_notes: String::new(),
            desc_append: String::new(),
            real_expiration_date: String::new(),
            fund_name: String::new(),
            fund_family: String::new(),
            fund_type: String::new(),
            fund_front_load: String::new(),
            fund_back_load: String::new(),
            fund_back_load_time_interval: String::new(),
            fund_management_fee: String::new(),
            fund_closed: false,
            fund_closed_for_new_investors: false,
            fund_closed_for_new_money: false,
            fund_notify_amount: String::new(),
            fund_minimum_initial_purchase: String::new(),
            fund_minimum_subsequent_purchase: String::new(),
            fund_blue_sky_states: String::new(),
            fund_blue_sky_territories: String::new(),
            fund_distribution_policy_indicator: String::new(),
            fund_asset_type: String::new(),
            trading_hours: String::new(),
            liquid_hours: String::new(),
            time_zone_id: String::new(),
        }
    }

    pub fn from_definition(py: Python<'_>, def: &crate::control::contracts::ContractDefinition) -> Self {
        let c = Contract {
            con_id: def.con_id as i64,
            // Official API string ("STK"), not the Debug derive ("Stock"): the
            // returned Contract must round-trip into another request (ibx#230).
            sec_type: def.sec_type.to_api_str().to_string(),
            symbol: def.symbol.clone(),
            exchange: def.exchange.clone(),
            primary_exchange: def.primary_exchange.clone(),
            currency: def.currency.clone(),
            local_symbol: def.local_symbol.clone(),
            trading_class: def.trading_class.clone(),
            last_trade_date_or_contract_month: def.last_trade_date.clone(),
            strike: def.strike,
            multiplier: if def.multiplier != 1.0 { format!("{}", def.multiplier) } else { String::new() },
            ..Default::default()
        };

        Self {
            contract: Py::new(py, c).expect("Contract allocation failed"),
            // Parsed from the reply all along but thrown away (ibx#230).
            market_name: def.market_name.clone(),
            min_tick: def.min_tick,
            order_types: def.order_types.join(","),
            valid_exchanges: def.valid_exchanges.join(","),
            long_name: def.long_name.clone(),
            last_trade_date: def.last_trade_date.clone(),
            multiplier: if def.multiplier != 1.0 { format!("{}", def.multiplier) } else { String::new() },
            market_rule_id: def.market_rule_id.map(|id| id as i64).unwrap_or(-1),
            market_rule_ids: def.market_rule_id.map(|id| id.to_string()).unwrap_or_default(),
            strike: def.strike,
            right: def.right.map(|r| format!("{r:?}")).unwrap_or_default(),
            primary_exchange: def.primary_exchange.clone(),
            local_symbol: def.local_symbol.clone(),
            trading_class: def.trading_class.clone(),
            stock_type: def.stock_type.clone(),
            category: def.category.clone(),
            country: def.country.clone(),
            isin: def.isin.clone(),
            cusip: def.cusip.clone(),
            sec_id_list: def.sec_id_list.clone(),
            min_size: def.min_size,
            industry: def.industry.clone(),
            subcategory: def.subcategory.clone(),
            price_magnifier: def.price_magnifier,
            contract_month: def.contract_month.clone(),
            under_sec_type: def.under_sec_type.clone(),
            ev_rule: def.ev_rule.clone(),
            ev_multiplier: def.ev_multiplier,
            under_con_id: def.under_con_id,
            under_symbol: def.under_symbol.clone(),
            last_trade_time: def.last_trade_time.clone(),
            issue_date: def.issue_date.clone(),
            size_increment: def.size_increment,
            suggested_size_increment: def.suggested_size_increment,
            last_price_precision: def.last_price_precision,
            last_size_precision: def.last_size_precision,
            settlement_method: def.settlement_method.clone(),
            unnamed_fields: def.unnamed_fields.clone(),
            agg_group: def.agg_group,
            coupon: def.coupon,
            bond_type: def.bond_type.clone(),
            coupon_type: def.coupon_type.clone(),
            callable: def.callable,
            puttable: def.puttable,
            convertible: def.convertible,
            next_option_partial: def.next_option_partial,
            next_option_date: def.next_option_date.clone(),
            next_option_type: def.next_option_type.clone(),
            ratings: def.ratings.clone(),
            bond_notes: def.bond_notes.clone(),
            desc_append: def.desc_append.clone(),
            real_expiration_date: def.real_expiration_date.clone(),
            fund_name: def.fund_name.clone(),
            fund_family: def.fund_family.clone(),
            fund_type: def.fund_type.clone(),
            fund_front_load: def.fund_front_load.clone(),
            fund_back_load: def.fund_back_load.clone(),
            fund_back_load_time_interval: def.fund_back_load_time_interval.clone(),
            fund_management_fee: def.fund_management_fee.clone(),
            fund_closed: def.fund_closed,
            fund_closed_for_new_investors: def.fund_closed_for_new_investors,
            fund_closed_for_new_money: def.fund_closed_for_new_money,
            fund_notify_amount: def.fund_notify_amount.clone(),
            fund_minimum_initial_purchase: def.fund_minimum_initial_purchase.clone(),
            fund_minimum_subsequent_purchase: def.fund_minimum_subsequent_purchase.clone(),
            fund_blue_sky_states: def.fund_blue_sky_states.clone(),
            fund_blue_sky_territories: def.fund_blue_sky_territories.clone(),
            fund_distribution_policy_indicator: def.fund_distribution_policy_indicator.clone(),
            fund_asset_type: def.fund_asset_type.clone(),
            trading_hours: def.trading_hours.clone().unwrap_or_default(),
            liquid_hours: def.liquid_hours.clone().unwrap_or_default(),
            time_zone_id: def.time_zone_id.clone().unwrap_or_default(),
        }
    }
}

// ── Execution ──

/// ibapi-compatible Execution class (used in exec_details callback).
#[pyclass(from_py_object)]
#[derive(Clone, Debug, Default)]
pub struct Execution {
    /// Every field the report stated that this client does not name, as
    /// (tag, value). Kept rather than dropped.
    #[pyo3(get, set)]
    pub unnamed_fields: Vec<(u32, String)>,
    #[pyo3(get, set)]
    pub exec_id: String,
    #[pyo3(get, set)]
    pub time: String,
    #[pyo3(get, set)]
    pub acct_number: String,
    #[pyo3(get, set)]
    pub exchange: String,
    #[pyo3(get, set)]
    pub side: String,
    #[pyo3(get, set)]
    pub shares: f64,
    #[pyo3(get, set)]
    pub price: f64,
    #[pyo3(get, set)]
    pub perm_id: i64,
    #[pyo3(get, set)]
    pub client_id: i64,
    #[pyo3(get, set)]
    pub order_id: i64,
    #[pyo3(get, set)]
    pub liquidation: i32,
    #[pyo3(get, set)]
    pub cum_qty: f64,
    #[pyo3(get, set)]
    pub avg_price: f64,
    #[pyo3(get, set)]
    pub order_ref: String,
    #[pyo3(get, set)]
    pub ev_rule: String,
    #[pyo3(get, set)]
    pub ev_multiplier: f64,
    #[pyo3(get, set)]
    pub model_code: String,
    #[pyo3(get, set)]
    pub last_liquidity: i32,
    #[pyo3(get, set)]
    pub pending_price_revision: bool,
}

#[pymethods]
impl Execution {
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
    fn new() -> Self { Self::default() }
}

// ── SmartComponent ──

/// ibapi-compatible SmartComponent class.
#[pyclass(from_py_object, name = "SmartComponent")]
#[derive(Clone, Debug, Default)]
pub struct SmartComponentPy {
    #[pyo3(get, set)]
    pub bit_number: i32,
    #[pyo3(get, set)]
    pub exchange: String,
    #[pyo3(get, set)]
    pub exchange_letter: String,
}

#[pymethods]
impl SmartComponentPy {
    #[new]
    #[pyo3(signature = ())]
    fn new() -> Self { Self::default() }
}

// ── NewsProvider ──

/// ibapi-compatible NewsProvider class.
#[pyclass(from_py_object, name = "NewsProvider")]
#[derive(Clone, Debug, Default)]
pub struct NewsProviderPy {
    #[pyo3(get, set)]
    pub code: String,
    #[pyo3(get, set)]
    pub name: String,
}

#[pymethods]
impl NewsProviderPy {
    #[new]
    #[pyo3(signature = ())]
    fn new() -> Self { Self::default() }
}

// ── SoftDollarTier ──

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
}

// ── CommissionAndFeesReport ──

/// ibapi-compatible CommissionAndFeesReport class.
#[pyclass(from_py_object)]
#[derive(Clone, Debug, Default)]
pub struct CommissionAndFeesReport {
    #[pyo3(get, set)]
    pub exec_id: String,
    #[pyo3(get, set)]
    pub commission_and_fees: f64,
    #[pyo3(get, set)]
    pub currency: String,
    #[pyo3(get, set)]
    pub realized_pnl: f64,
    #[pyo3(get, set)]
    pub yield_amount: f64,
    #[pyo3(get, set)]
    pub yield_redemption_date: String,
}

#[pymethods]
impl CommissionAndFeesReport {
    #[new]
    #[pyo3(signature = ())]
    fn new() -> Self { Self::default() }
}

// ── ContractDescription ──

/// ibapi-compatible ContractDescription class for symbol search results.
#[pyclass(from_py_object)]
#[derive(Debug, Clone)]
pub struct ContractDescription {
    #[pyo3(get, set)]
    pub con_id: i64,
    #[pyo3(get, set)]
    pub symbol: String,
    #[pyo3(get, set)]
    pub sec_type: String,
    #[pyo3(get, set)]
    pub currency: String,
    #[pyo3(get, set)]
    pub primary_exchange: String,
    #[pyo3(get, set)]
    pub derivative_sec_types: Vec<String>,
}

#[pymethods]
impl ContractDescription {
    #[new]
    #[pyo3(signature = (con_id=0, symbol="".to_string(), sec_type="".to_string(), currency="".to_string(), primary_exchange="".to_string(), derivative_sec_types=Vec::new()))]
    fn new(con_id: i64, symbol: String, sec_type: String, currency: String, primary_exchange: String, derivative_sec_types: Vec<String>) -> Self {
        Self { con_id, symbol, sec_type, currency, primary_exchange, derivative_sec_types }
    }

    fn __repr__(&self) -> String {
        format!("ContractDescription(conId={}, symbol='{}', secType='{}', currency='{}')",
            self.con_id, self.symbol, self.sec_type, self.currency)
    }
}

#[pyclass(from_py_object, name = "DepthMktDataDescription")]
#[derive(Debug, Clone)]
pub struct DepthMktDataDescriptionPy {
    #[pyo3(get, set)]
    pub exchange: String,
    #[pyo3(get, set)]
    pub sec_type: String,
    #[pyo3(get, set)]
    pub listing_exch: String,
    #[pyo3(get, set)]
    pub service_data_type: String,
    #[pyo3(get, set)]
    pub agg_group: i32,
}

#[pymethods]
impl DepthMktDataDescriptionPy {
    #[new]
    #[pyo3(signature = (exchange="".to_string(), sec_type="".to_string(), listing_exch="".to_string(), service_data_type="".to_string(), agg_group=0))]
    fn new(exchange: String, sec_type: String, listing_exch: String, service_data_type: String, agg_group: i32) -> Self {
        Self { exchange, sec_type, listing_exch, service_data_type, agg_group }
    }

    fn __repr__(&self) -> String {
        format!("DepthMktDataDescription(exchange='{}', secType='{}', listingExch='{}', serviceDataType='{}', aggGroup={})",
            self.exchange, self.sec_type, self.listing_exch, self.service_data_type, self.agg_group)
    }
}

/// One step of a contract's price ladder: where it starts, and what the price
/// moves in above it.
///
/// A pair of numbers is what this used to hand over, and the reference client
/// hands over an object with names on it — so did the Rust surface here, while
/// the Python one did not, and a program reading `lowEdge` off what it was
/// given found a tuple.
#[pyclass(from_py_object, name = "PriceIncrement")]
#[derive(Debug, Clone)]
pub struct PriceIncrementPy {
    #[pyo3(get, set)]
    pub low_edge: f64,
    #[pyo3(get, set)]
    pub increment: f64,
}

#[pymethods]
impl PriceIncrementPy {
    #[new]
    #[pyo3(signature = (low_edge=0.0, increment=0.0))]
    fn new(low_edge: f64, increment: f64) -> Self {
        Self { low_edge, increment }
    }

    /// The name the reference client gives it. Both spellings resolve on
    /// every type here, so a program written against either reads it.
    #[getter(lowEdge)]
    fn low_edge_camel(&self) -> f64 {
        self.low_edge
    }

    fn __repr__(&self) -> String {
        format!("PriceIncrement(lowEdge={}, increment={})", self.low_edge, self.increment)
    }
}

/// Register all compat contract/order classes on the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Contract>()?;
    m.add_class::<Order>()?;
    m.add_class::<OptionChain>()?;
    m.add_class::<TagValue>()?;
    m.add_class::<OrderState>()?;
    m.add_class::<OrderAllocation>()?;
    m.add_class::<PriceCondition>()?;
    m.add_class::<TimeCondition>()?;
    m.add_class::<MarginCondition>()?;
    m.add_class::<ExecutionCondition>()?;
    m.add_class::<VolumeCondition>()?;
    m.add_class::<PercentChangeCondition>()?;
    m.add_class::<BarData>()?;
    m.add_class::<ContractDetails>()?;
    m.add_class::<ContractDescription>()?;
    m.add_class::<CommissionAndFeesReport>()?;
    m.add_class::<Execution>()?;
    m.add_class::<SmartComponentPy>()?;
    m.add_class::<NewsProviderPy>()?;
    m.add_class::<SoftDollarTierPy>()?;
    m.add_class::<DepthMktDataDescriptionPy>()?;
    m.add_class::<PriceIncrementPy>()?;
    Ok(())
}

/// The reference client spells a field by running its words together, and for a
/// few of them it also chose a different word than this crate did. Code written
/// against that client reads those spellings, so they resolve here too — while a
/// name that names no field is still refused.
fn by_reference_name(
    obj: &Bound<'_, PyAny>,
    name: &str,
    aliases: &[(&str, &str)],
) -> PyResult<Py<PyAny>> {
    if let Some((_, ours)) = aliases.iter().find(|(theirs, _)| *theirs == name)
        && let Ok(v) = obj.getattr(*ours)
    {
        return Ok(v.unbind());
    }
    let mut snake = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                snake.push('_');
            }
            snake.extend(c.to_lowercase());
        } else {
            snake.push(c);
        }
    }
    if snake != name
        && let Ok(v) = obj.getattr(snake.as_str())
    {
        return Ok(v.unbind());
    }
    Err(pyo3::exceptions::PyAttributeError::new_err(format!(
        "object has no attribute '{name}'"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_core::ClientCore;

    /// A TRAIL LIMIT states how far its limit sits from the trigger. Unset is
    /// `f64::MAX` on both sides, so a value dropped in conversion is not merely
    /// lost, it is indistinguishable from absent, and the wire falls back to
    /// `lmt_price` — the order goes out with an offset the caller never chose
    /// (ibx#395).
    #[test]
    fn a_python_trail_limit_offset_survives_the_conversion() {
        let o = Order {
            order_type: "TRAIL LIMIT".into(),
            lmt_price_offset: 0.25,
            ..Default::default()
        };

        let api = o.to_api();

        assert_eq!(api.lmt_price_offset, 0.25, "the offset the caller set must reach the wire");
        assert_ne!(
            api.lmt_price_offset, f64::MAX,
            "and must not arrive as the unset sentinel, which reads as never supplied",
        );
    }

    #[test]
    fn contract_default_values() {
        // The defaults ibx actually presents, which are the ones its `#[new]`
        // signature declares — not ibapi's empty strings. This test asserted the
        // latter and had never run to say otherwise (ibx#381).
        let c = Contract::default();
        assert_eq!(c.con_id, 0);
        assert_eq!(c.symbol, "");
        assert_eq!(c.sec_type, "STK");
        assert_eq!(c.exchange, "SMART");
        assert_eq!(c.currency, "USD");
        assert_eq!(c.strike, 0.0);
    }

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
        assert_eq!(o.to_api().side().unwrap(), Side::Buy);
        o.action = "SELL".into();
        assert_eq!(o.to_api().side().unwrap(), Side::Sell);
        o.action = "SSHORT".into();
        assert_eq!(o.to_api().side().unwrap(), Side::ShortSell);
    }

    #[test]
    fn order_tif_byte_mapping() {
        let mut o = Order { tif: "DAY".into(), ..Default::default() };
        assert_eq!(o.to_api().tif_byte(), b'0');
        o.tif = "GTC".into();
        assert_eq!(o.to_api().tif_byte(), b'1');
        o.tif = "IOC".into();
        assert_eq!(o.to_api().tif_byte(), b'3');
        o.tif = "FOK".into();
        assert_eq!(o.to_api().tif_byte(), b'4');
    }

    #[test]
    fn order_has_extended_attrs() {
        let o = Order::default();
        assert!(!o.to_api().has_extended_attrs());

        let o2 = Order { hidden: true, ..Default::default() };
        assert!(o2.to_api().has_extended_attrs());
    }

    #[test]
    fn order_attrs_conversion() {
        let o = Order {
            display_size: 50,
            hidden: true,
            discretionary_amt: 0.05,
            ..Default::default()
        };
        let attrs = o.to_api().attrs();
        assert_eq!(attrs.display_size, 50);
        assert!(attrs.hidden);
        assert_eq!(attrs.discretionary_amt, (0.05 * PRICE_SCALE_F) as Price);
    }

    /// A TRAIL LIMIT carries all three fields `to_api` used to drop, and each
    /// reaches the wire as a distinct value, so the assertions can't pass on a
    /// default: the limit offset is tag 6370 and falls back to `lmt_price`
    /// when unset, which is why the two are set to different numbers here.
    #[test]
    fn to_api_carries_oca_type_trail_stop_and_lmt_price_offset() {
        let o = Order {
            action: "BUY".into(),
            total_quantity: 1.0,
            order_type: "TRAIL LIMIT".into(),
            lmt_price: 10.0,
            lmt_price_offset: 0.5,
            aux_price: 1.0,
            trail_stop_price: 99.0,
            oca_type: 2,
            ..Default::default()
        };

        let cmd = ClientCore::build_order_request(&o.to_api(), 1, 0, None).unwrap();
        let ControlCommand::Order(OrderRequest::SubmitEx {
            kind: OrderKind::TrailingStopLimit { lmt_offset, trail_stop_price, .. },
            attrs,
            ..
        }) = cmd else { panic!("TRAIL LIMIT must build a TrailingStopLimit request") };

        assert_eq!(lmt_offset, (0.5 * PRICE_SCALE_F) as Price);
        assert_eq!(trail_stop_price, (99.0 * PRICE_SCALE_F) as Price);
        assert_eq!(attrs.oca_type, 2);
    }

    #[test]
    fn tag_value_fields() {
        let tv = TagValue { tag: "maxPctVol".into(), value: "0.1".into() };
        assert_eq!(tv.tag, "maxPctVol");
        assert_eq!(tv.value, "0.1");
    }

    #[test]
    fn price_condition_to_internal() {
        let pc = PriceCondition {
            con_id: 265598,
            exchange: "SMART".into(),
            price: 200.0,
            is_more: true,
            trigger_method: 1,
        };
        match pc.to_internal() {
            OrderCondition::Price { con_id, price, is_more, trigger_method, .. } => {
                assert_eq!(con_id, 265598);
                assert_eq!(price, (200.0 * PRICE_SCALE_F) as Price);
                assert!(is_more);
                assert_eq!(trigger_method, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn time_condition_to_internal() {
        let tc = TimeCondition { time: "20260311-09:30:00".into(), is_more: true };
        match tc.to_internal() {
            OrderCondition::Time { time, is_more } => {
                assert_eq!(time, "20260311-09:30:00");
                assert!(is_more);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn volume_condition_to_internal() {
        let vc = VolumeCondition {
            con_id: 265598,
            exchange: "SMART".into(),
            volume: 1_000_000,
            is_more: true,
        };
        match vc.to_internal() {
            OrderCondition::Volume { con_id, volume, is_more, .. } => {
                assert_eq!(con_id, 265598);
                assert_eq!(volume, 1_000_000);
                assert!(is_more);
            }
            _ => panic!("wrong variant"),
        }
    }
}

/// One venue's option chain for an underlying, as the client this follows
/// hands it over: the venue, the class it trades under, what one contract
/// covers, and every expiry and strike it lists.
#[pyclass(get_all, set_all, skip_from_py_object)]
#[derive(Clone, Default)]
pub struct OptionChain {
    pub exchange: String,
    #[pyo3(name = "underlyingConId")]
    pub underlying_con_id: i64,
    #[pyo3(name = "tradingClass")]
    pub trading_class: String,
    pub multiplier: String,
    pub expirations: Vec<String>,
    pub strikes: Vec<f64>,
}

#[pymethods]
impl OptionChain {
    fn __repr__(&self) -> String {
        format!(
            "OptionChain(exchange='{}', tradingClass='{}', multiplier='{}', {} expirations, {} strikes)",
            self.exchange,
            self.trading_class,
            self.multiplier,
            self.expirations.len(),
            self.strikes.len(),
        )
    }
}
