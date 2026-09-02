//! The contract classes a caller works in, as the Python API names them.

// The other families, and the two helpers every class here uses.
use super::contract::{by_reference_name, set_from_keywords};
use pyo3::prelude::*;
use pyo3::types::PyList;
use std::sync::OnceLock;

use super::{camel_aliases_copy, camel_aliases_owned};

/// A list a Python program holds by reference.
///
/// The reference client's classes carry plain lists, and its own samples
/// build every combination, algo and conditional order by appending to one.
/// A field handed back as a fresh list on every read took the append on a
/// copy and dropped it: the order went to the venue without its legs, its
/// parameters or its conditions, and nothing said so. So the field is the
/// list itself. Two reads hand back the same object, an append is kept, and
/// what is read at send time is what the list holds then.
///
/// Made on the first read rather than at construction, because these classes
/// are also built from Rust — `Default`, `Clone`, `from_api` — with no
/// interpreter token to hand. Unset reads as empty.
#[derive(Default)]
pub struct ListField(OnceLock<Py<PyList>>);

impl ListField {
    pub const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// The list itself, made empty if nothing has read or set it yet.
    pub fn bound<'py>(&self, py: Python<'py>) -> Bound<'py, PyList> {
        self.0.get_or_init(|| PyList::empty(py).unbind()).bind(py).clone()
    }

    /// One holding these, in this order.
    pub fn of<'py, T: IntoPyObject<'py>>(
        py: Python<'py>,
        items: impl IntoIterator<Item = T>,
    ) -> PyResult<Self> {
        let list = PyList::empty(py);
        for item in items {
            list.append(item)?;
        }
        Ok(Self(OnceLock::from(list.unbind())))
    }
}

impl Clone for ListField {
    /// The same list, as Python assignment shares it. A set slot proves an
    /// interpreter exists, so attaching here attaches to one that is there;
    /// an unset slot needs none.
    fn clone(&self) -> Self {
        match self.0.get() {
            None => Self::new(),
            Some(list) => Self(OnceLock::from(Python::attach(|py| list.clone_ref(py)))),
        }
    }
}

impl<'py> IntoPyObject<'py> for &ListField {
    type Target = PyList;
    type Output = Bound<'py, PyList>;
    type Error = std::convert::Infallible;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        Ok(self.bound(py))
    }
}

impl FromPyObject<'_, '_> for ListField {
    type Error = PyErr;

    /// The list assigned, itself: a name the caller keeps for it goes on
    /// appending to the field's list, as it does on the reference client.
    /// `None` is that client's own default for four of these and holds
    /// nothing. Anything else is copied into a list as `list(...)` would be,
    /// and what `list` refuses is refused.
    fn extract(obj: Borrowed<'_, '_, PyAny>) -> PyResult<Self> {
        let py = obj.py();
        let list = if obj.is_none() {
            PyList::empty(py)
        } else if let Ok(list) = obj.cast::<PyList>() {
            list.to_owned()
        } else {
            py.get_type::<PyList>().call1((obj,))?.cast_into::<PyList>()?
        };
        Ok(Self(OnceLock::from(list.unbind())))
    }
}

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
    pub combo_legs: ListField,
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
            combo_legs: self.combo_legs.clone(),
            delta_neutral_contract: None,
        }
    }
}

impl Default for Contract {
    fn default() -> Self {
        Self {
            con_id: 0,
            symbol: String::new(),
            // Empty, as the reference client leaves them. Defaulting to a US
            // stock on SMART sends three terms the caller never stated: a
            // future is described as a stock, and a contract listed abroad is
            // asked for in dollars on a US venue. What the
            // caller did not state is not stated for them.
            sec_type: String::new(),
            exchange: String::new(),
            currency: String::new(),
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
            combo_legs: ListField::new(),
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
    ///
    /// An absent attribute takes the reference client's default. An attribute
    /// that is present and cannot be read is a value the caller stated and
    /// this cannot carry, and is refused.
    pub fn combo_legs_api(&self, py: Python<'_>) -> Result<Vec<crate::types::model::ComboLeg>, String> {
        let legs = self.combo_legs.bound(py);
        let mut out = Vec::with_capacity(legs.len());
        for (i, obj) in legs.iter().enumerate() {
            // Absent takes the default; stated and unreadable is refused.
            macro_rules! read {
                ($name:literal, $default:expr) => {
                    match obj.getattr($name) {
                        // Absent is the default the reference client would
                        // have used. An attribute that raises this from inside
                        // itself is indistinguishable from one that is not
                        // there — the interpreter states the same error for
                        // both — so it is read the same way. Anything else is a
                        // value the caller has and this could not read, and
                        // guessing at it puts a leg on the wire nobody
                        // described.
                        Err(e) if e.is_instance_of::<pyo3::exceptions::PyAttributeError>(py) => {
                            let _ = e;
                            $default
                        }
                        Err(e) => {
                            return Err(format!("combo leg {i} states a {} that cannot be read: {e}", $name))
                        }
                        Ok(v) => v.extract().map_err(|e| {
                            format!("combo leg {i} states a {} that cannot be read: {e}", $name)
                        })?,
                    }
                };
            }
            let con_id: i64 = read!("conId", 0);
            if con_id == 0 {
                return Err(format!("combo leg {i} has no conId, so it names no contract"));
            }
            out.push(crate::types::model::ComboLeg {
                con_id,
                // Nought, which is what the reference client leaves a leg it
                // was not given a ratio for. One made such a leg into a
                // one-for-one executable leg — an instruction the caller never
                // gave, on a leg they had not finished describing.
                ratio: read!("ratio", 0),
                action: read!("action", String::new()),
                exchange: read!("exchange", String::new()),
                open_close: read!("openClose", 0),
                shorting_policy: read!("shortSaleSlot", 0),
                designated_location: read!("designatedLocation", String::new()),
                exempt_code: read!("exemptCode", -1),
            });
        }
        Ok(out)
    }

    /// The whole contract the engine holds, not the handful of fields a
    /// callback happens to print: an option with no strike, right or expiry
    /// names nothing the caller can act on. Combo legs and the delta-neutral
    /// contract need a `Python` token to build and are left empty here.
    pub(crate) fn from_api(c: &crate::types::model::Contract) -> Self {
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
            combo_legs: ListField::new(),
            delta_neutral_contract: None,
        }
    }

    /// The same contract in the shape the rest of the client uses.
    ///
    /// Combo legs and the delta-neutral contract are left out: they need a
    /// `Python` token to read. `combo_legs_api` reads the legs, and the order
    /// path calls it.
    pub(crate) fn to_api(&self) -> crate::types::model::Contract {
        crate::types::model::Contract {
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

#[pymethods]
impl Contract {
    #[new]
    #[pyo3(signature = (con_id=0, symbol="".to_string(), sec_type="".to_string(), exchange="".to_string(), currency="".to_string(), last_trade_date_or_contract_month="".to_string(), strike=0.0, right="".to_string(), multiplier="".to_string(), local_symbol="".to_string(), primary_exchange="".to_string(), trading_class="".to_string(), **keywords))]
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
    // The list the contract holds, itself, under the name the reference client
    // uses: read by that name a combination reported no legs, and handed back
    // as a copy it lost every leg appended to it.
    #[getter(comboLegs)]
    fn get_combo_legs_alias<'py>(&self, py: Python<'py>) -> Bound<'py, PyList> {
        self.combo_legs.bound(py)
    }
    #[setter(comboLegs)]
    fn set_combo_legs_alias(&mut self, v: ListField) { self.combo_legs = v; }
    #[getter(deltaNeutralContract)]
    fn get_delta_neutral_alias(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.delta_neutral_contract.as_ref().map(|c| c.clone_ref(py))
    }
    #[setter(deltaNeutralContract)]
    fn set_delta_neutral_alias(&mut self, v: Option<Py<PyAny>>) {
        self.delta_neutral_contract = v;
    }
}

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
    /// `str()` of each, as the reference client keeps them: its own samples
    /// hand over a float or an int, and the venue reads text.
    #[new]
    #[pyo3(signature = (tag=None, value=None))]
    fn new(py: Python<'_>, tag: Option<Bound<'_, PyAny>>, value: Option<Bound<'_, PyAny>>) -> PyResult<Self> {
        let text = |given: Option<Bound<'_, PyAny>>| -> PyResult<String> {
            let given = given.unwrap_or_else(|| py.None().into_bound(py).into_any());
            Ok(given.str()?.to_cow()?.into_owned())
        };
        Ok(Self { tag: text(tag)?, value: text(value)? })
    }

    fn __repr__(&self) -> String {
        format!("TagValue(tag='{}', value='{}')", self.tag, self.value)
    }
}

impl TagValue {
    /// The same pair, as the Python side names it.
    pub(crate) fn from_api(tv: &crate::types::model::TagValue) -> Self {
        Self { tag: tv.tag.clone(), value: tv.value.clone() }
    }
}

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
    /// mutated a temporary clone and was a silent no-op.
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
    /// When the contract trades, stated in UTC.
    ///
    /// The reference client states these in the zone `time_zone_id` names;
    /// this states them in UTC, as the wire carries them. Converting them by
    /// the name beside them moves every session by the offset.
    #[pyo3(get, set)]
    pub trading_hours: String,
    /// Its regular session, on the same clock as `trading_hours`.
    #[pyo3(get, set)]
    pub liquid_hours: String,
    /// The zone the exchange keeps — which is not the zone the two above are
    /// stated on.
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
    /// Default, so this replaces the derived constructor.
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
        /// A right under the letter the official API states it by, not the
        /// name this crate spells it with: `"Call"` is a Rust word, `"C"` is
        /// what goes back on the wire and what a caller compares against.
        fn right_str(right: Option<crate::control::contracts::OptionRight>) -> String {
            use crate::control::contracts::OptionRight;
            match right {
                Some(OptionRight::Call) => "C".to_string(),
                Some(OptionRight::Put) => "P".to_string(),
                None => String::new(),
            }
        }

        let c = Contract {
            con_id: def.con_id as i64,
            // Official API string ("STK"), not the Debug derive ("Stock"): the
            // returned Contract must round-trip into another request.
            sec_type: def.sec_type.to_api_str().to_string(),
            symbol: def.symbol.clone(),
            exchange: def.exchange.clone(),
            primary_exchange: def.primary_exchange.clone(),
            currency: def.currency.clone(),
            local_symbol: def.local_symbol.clone(),
            trading_class: def.trading_class.clone(),
            last_trade_date_or_contract_month: def.last_trade_date.clone(),
            strike: def.strike,
            // Under the official API's letters, as the security type above is.
            // Left off, a call and a put on the same strike are the same
            // contract, and one reused for another request names whichever the
            // venue picks.
            right: right_str(def.right),
            multiplier: if def.multiplier != 1.0 { format!("{}", def.multiplier) } else { String::new() },
            ..Default::default()
        };

        Self {
            contract: Py::new(py, c).expect("Contract allocation failed"),
            // Parsed from the reply all along but thrown away.
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
            right: right_str(def.right),
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
/// The reference client hands over an object with names on it, as the Rust
/// surface here does, so a program reading `lowEdge` off what it is given
/// finds a field rather than a tuple element.
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
    #[setter(lowEdge)]
    fn set_low_edge_camel(&mut self, v: f64) {
        self.low_edge = v;
    }

    fn __repr__(&self) -> String {
        format!("PriceIncrement(lowEdge={}, increment={})", self.low_edge, self.increment)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A leg the caller did not finish describing is not completed here.
    ///
    /// The reference client leaves a leg given no ratio at nought. Substituting
    /// one turns an incomplete leg into a one-for-one executable leg, which is
    /// an instruction the caller did not give.
    #[test]
    fn a_leg_with_no_ratio_states_none() {
        Python::initialize();
        Python::attach(|py| {
            let leg = py
                .eval(
                    pyo3::ffi::c_str!(
                        "type('Leg', (), {'conId': 756733, 'action': 'BUY', 'exchange': 'SMART'})()"
                    ),
                    None,
                    None,
                )
                .expect("a leg naming a contract and nothing else");
            let contract = Contract {
                combo_legs: ListField::of(py, [leg]).expect("one leg"),
                ..Default::default()
            };
            let built = contract.combo_legs_api(py).expect("the leg reads");
            assert_eq!(built[0].ratio, 0, "no ratio stated, so none is invented");
            assert_eq!(built[0].con_id, 756733);
        });
    }

    /// The legs appended the way the reference samples append them are the
    /// legs the order goes out with.
    ///
    /// Handed back as a fresh list on every read, the field took each append
    /// on a copy and dropped it, and a combination built the way every one of
    /// those samples builds one went to the venue with no legs at all.
    #[test]
    fn legs_appended_as_the_reference_samples_append_them_reach_the_order() {
        use crate::types::{ControlCommand, OrderRequest};
        Python::initialize();
        Python::attach(|py| {
            let contract = Py::new(py, Contract::default()).expect("a contract");
            let locals = pyo3::types::PyDict::new(py);
            locals.set_item("contract", &contract).expect("named for the sample");
            py.run(
                c"
class ComboLeg:
    pass
contract.secType = 'BAG'
leg1 = ComboLeg()
leg1.conId = 43645865
leg1.ratio = 1
leg1.action = 'BUY'
leg1.exchange = 'SMART'
leg2 = ComboLeg()
leg2.conId = 9408
leg2.ratio = 1
leg2.action = 'SELL'
leg2.exchange = 'SMART'
contract.comboLegs = []
contract.comboLegs.append(leg1)
contract.comboLegs.append(leg2)
",
                None,
                Some(&locals),
            )
            .expect("built as the sample builds it");
            let legs = contract.borrow(py).combo_legs_api(py).expect("the legs read");
            let on_the_wire = crate::types::model::Contract { combo_legs: legs, ..Default::default() };
            let order = crate::types::model::Order {
                action: "BUY".into(),
                total_quantity: 1.0,
                order_type: "LMT".into(),
                lmt_price: 10.0,
                ..Default::default()
            };
            let ControlCommand::Order(OrderRequest::SubmitEx { attrs, .. }) =
                crate::client_core::ClientCore::build_order_request(&order, 1, 0, Some(&on_the_wire))
                    .expect("a combination order builds")
            else {
                panic!("a limit order builds a SubmitEx")
            };
            assert_eq!(
                attrs.combo_legs.iter().map(|l| (l.con_id, l.ratio, l.is_sell)).collect::<Vec<_>>(),
                [(43645865, 1, false), (9408, 1, true)],
                "both legs, as appended, on the request",
            );
        });
    }
}

camel_aliases_copy! {
    Contract {
        get_include_expired_alias set_include_expired_alias includeExpired include_expired bool;
    }
}

camel_aliases_owned! {
    Contract {
        get_sec_type_alias set_sec_type_alias secType sec_type String;
        get_ltdocm_alias set_ltdocm_alias lastTradeDateOrContractMonth last_trade_date_or_contract_month String;
        get_ltd_alias set_ltd_alias lastTradeDate last_trade_date String;
        get_local_symbol_alias set_local_symbol_alias localSymbol local_symbol String;
        get_primary_exchange_alias set_primary_exchange_alias primaryExchange primary_exchange String;
        get_trading_class_alias set_trading_class_alias tradingClass trading_class String;
        get_sec_id_type_alias set_sec_id_type_alias secIdType sec_id_type String;
        get_sec_id_alias set_sec_id_alias secId sec_id String;
        get_issuer_id_alias set_issuer_id_alias issuerId issuer_id String;
        get_combo_legs_descrip_alias set_combo_legs_descrip_alias comboLegsDescrip combo_legs_descrip String;
    }
}
