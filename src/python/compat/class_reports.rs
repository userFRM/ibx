//! What the venue reports back: fills, their cost, bars, and news.

// The other families, and the two helpers every class here uses.
use super::contract::by_reference_name;
use pyo3::prelude::*;

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
    /// Which timezone `date` is stated in, as the reply states it. Without
    /// it the timestamp says nothing about what the bar times mean. Empty on
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

    // Under the reference client's own names. A program reads a commission
    // report by the names its library declares, and this one is handed
    // straight to that library's callback: a name it does not answer to is an
    // exception on every fill, which is every time money moves.
    #[getter(execId)]
    fn get_exec_id_alias(&self) -> String { self.exec_id.clone() }
    #[setter(execId)]
    fn set_exec_id_alias(&mut self, v: String) { self.exec_id = v; }

    /// What the venue charged. The reference client calls the whole of it the
    /// commission; this client names it for what it now includes.
    #[getter(commission)]
    fn get_commission_alias(&self) -> f64 { self.commission_and_fees }
    #[setter(commission)]
    fn set_commission_alias(&mut self, v: f64) { self.commission_and_fees = v; }

    #[getter(commissionAndFees)]
    fn get_commission_and_fees_alias(&self) -> f64 { self.commission_and_fees }
    #[setter(commissionAndFees)]
    fn set_commission_and_fees_alias(&mut self, v: f64) { self.commission_and_fees = v; }

    #[getter(realizedPNL)]
    fn get_realized_pnl_alias(&self) -> f64 { self.realized_pnl }
    #[setter(realizedPNL)]
    fn set_realized_pnl_alias(&mut self, v: f64) { self.realized_pnl = v; }

    /// Their spelling keeps the underscore: `yield` is a keyword in Python.
    #[getter(yield_)]
    fn get_yield_alias(&self) -> f64 { self.yield_amount }
    #[setter(yield_)]
    fn set_yield_alias(&mut self, v: f64) { self.yield_amount = v; }

    #[getter(yieldRedemptionDate)]
    fn get_yield_redemption_date_alias(&self) -> String { self.yield_redemption_date.clone() }
    #[setter(yieldRedemptionDate)]
    fn set_yield_redemption_date_alias(&mut self, v: String) { self.yield_redemption_date = v; }
}
