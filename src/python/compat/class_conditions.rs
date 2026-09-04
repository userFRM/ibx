//! What an order waits for before it works.

// The other families, and the two helpers every class here uses.
use super::contract::set_from_keywords;
use pyo3::prelude::*;

use super::{camel_aliases_copy, camel_aliases_owned};
use crate::types::*;
use super::super::types::PRICE_SCALE_F;

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
    #[pyo3(get, set)]
    pub is_conjunction_connection: bool,
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

    #[new]
    // Empty, which is what the reference client holds for a condition nobody
    // named an exchange on. Named as SMART here, a condition watching a
    // contract that trades elsewhere watched it on a venue nobody chose.
    #[pyo3(signature = (con_id=0, exchange=String::new(), price=0.0, is_more=true, trigger_method=0, is_conjunction_connection=true, **keywords))]
    fn new(con_id: i64, exchange: String, price: f64, is_more: bool, trigger_method: i32, is_conjunction_connection: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { con_id, exchange, price, is_more, trigger_method, is_conjunction_connection })?;
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        let op = if self.is_more { ">" } else { "<" };
        format!("PriceCondition(conId={}, price {} {})", self.con_id, op, self.price)
    }
}

impl PriceCondition {
    /// A condition this client cannot carry as stated is refused rather than
    /// changed: a price the wire's fixed point cannot hold converted in
    /// silence, and a trigger method outside the set the venue carries cast to
    /// another, and the order went to the venue waiting for something the
    /// caller never stated.
    pub fn to_internal(&self) -> Result<OrderCondition, String> {
        crate::client_core::require_finite_price("a price condition's price", self.price)?;
        // What the trigger method can state on a condition: 0 is the default,
        // 1 last, 2 bid/ask, 3 bid and 4 ask. Anything else narrows to one of
        // those or wraps on the cast, which is a different condition.
        let trigger_method = match self.trigger_method {
            0..=4 => self.trigger_method as u8,
            other => return Err(format!(
                "a price condition's trigger method {other} is not one the venue \
                 carries on a condition: it is 0 to 4, and anything else would go \
                 out as a different trigger than the one stated",
            )),
        };
        Ok(OrderCondition::Price {
            con_id: self.con_id,
            exchange: self.exchange.clone(),
            price: crate::types::price_from_f64(self.price),
            is_more: self.is_more,
            trigger_method,
            is_conjunction_connection: self.is_conjunction_connection,
        })
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
    #[pyo3(get, set)]
    pub is_conjunction_connection: bool,
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
    #[pyo3(signature = (time="".to_string(), is_more=true, is_conjunction_connection=true, **keywords))]
    fn new(time: String, is_more: bool, is_conjunction_connection: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { time, is_more, is_conjunction_connection })?;
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
        OrderCondition::Time { time: self.time.clone(), is_more: self.is_more, is_conjunction_connection: self.is_conjunction_connection }
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
    #[pyo3(get, set)]
    pub is_conjunction_connection: bool,
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
    #[pyo3(signature = (percent=0, is_more=true, is_conjunction_connection=true, **keywords))]
    fn new(percent: u32, is_more: bool, is_conjunction_connection: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { percent, is_more, is_conjunction_connection })?;
        set_from_keywords(made.bind(py).as_any(), keywords)?;
        Ok(made)
    }

    fn __repr__(&self) -> String {
        format!("MarginCondition({}% {})", self.percent, if self.is_more { "above" } else { "below" })
    }
}

impl MarginCondition {
    pub fn to_internal(&self) -> OrderCondition {
        OrderCondition::Margin { percent: self.percent, is_more: self.is_more, is_conjunction_connection: self.is_conjunction_connection }
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
    #[pyo3(get, set)]
    pub is_conjunction_connection: bool,
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

    #[new]
    #[pyo3(signature = (symbol="".to_string(), exchange="".to_string(), sec_type="".to_string(), is_conjunction_connection=true, **keywords))]
    fn new(symbol: String, exchange: String, sec_type: String, is_conjunction_connection: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { symbol, exchange, sec_type, is_conjunction_connection })?;
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
            is_conjunction_connection: self.is_conjunction_connection,
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
    #[pyo3(get, set)]
    pub is_conjunction_connection: bool,
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

    #[new]
    #[pyo3(signature = (con_id=0, exchange=String::new(), volume=0, is_more=true, is_conjunction_connection=true, **keywords))]
    fn new(con_id: i64, exchange: String, volume: i64, is_more: bool, is_conjunction_connection: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { con_id, exchange, volume, is_more, is_conjunction_connection })?;
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
            is_conjunction_connection: self.is_conjunction_connection,
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
    #[pyo3(get, set)]
    pub is_conjunction_connection: bool,
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

    #[new]
    #[pyo3(signature = (con_id=0, exchange=String::new(), change_percent=0.0, is_more=true, is_conjunction_connection=true, **keywords))]
    fn new(con_id: i64, exchange: String, change_percent: f64, is_more: bool, is_conjunction_connection: bool, keywords: Option<&Bound<'_, pyo3::types::PyDict>>, py: Python<'_>) -> PyResult<Py<Self>> {
        let made = Py::new(py, Self { con_id, exchange, change_percent, is_more, is_conjunction_connection })?;
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
            is_conjunction_connection: self.is_conjunction_connection,
        }
    }
}

/// A condition the engine holds, as the class a caller builds one with.
///
/// The engine keeps what a condition means rather than the object it came
/// from, so an order read back carries none and, placed again, works at once
/// with nothing holding it. Each kind the venue carries has a class here with
/// the same fields the engine keeps, so what the venue
/// reported is what comes back.
pub(crate) fn condition_from_internal(
    py: Python<'_>,
    held: &OrderCondition,
) -> PyResult<Py<PyAny>> {
    Ok(match held {
        OrderCondition::Price { con_id, exchange, price, is_more, trigger_method, is_conjunction_connection } => {
            Py::new(py, PriceCondition {
                is_conjunction_connection: *is_conjunction_connection,
                con_id: *con_id,
                exchange: exchange.clone(),
                price: *price as f64 / PRICE_SCALE_F,
                is_more: *is_more,
                trigger_method: *trigger_method as i32,
            })?.into_any()
        }
        OrderCondition::Time { time, is_more, is_conjunction_connection } => Py::new(py, TimeCondition {
            is_conjunction_connection: *is_conjunction_connection,
            time: time.clone(),
            is_more: *is_more,
        })?.into_any(),
        OrderCondition::Margin { percent, is_more, is_conjunction_connection } => Py::new(py, MarginCondition {
            is_conjunction_connection: *is_conjunction_connection,
            percent: *percent,
            is_more: *is_more,
        })?.into_any(),
        OrderCondition::Execution { symbol, exchange, sec_type, is_conjunction_connection } => {
            Py::new(py, ExecutionCondition {
                is_conjunction_connection: *is_conjunction_connection,
                symbol: symbol.clone(),
                exchange: exchange.clone(),
                sec_type: sec_type.clone(),
            })?.into_any()
        }
        OrderCondition::Volume { con_id, exchange, volume, is_more, is_conjunction_connection } => {
            Py::new(py, VolumeCondition {
                is_conjunction_connection: *is_conjunction_connection,
                con_id: *con_id,
                exchange: exchange.clone(),
                volume: *volume,
                is_more: *is_more,
            })?.into_any()
        }
        OrderCondition::PercentChange { con_id, exchange, percent, is_more, is_conjunction_connection } => {
            Py::new(py, PercentChangeCondition {
                is_conjunction_connection: *is_conjunction_connection,
                con_id: *con_id,
                exchange: exchange.clone(),
                change_percent: *percent,
                is_more: *is_more,
            })?.into_any()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An order read back states what it is waiting for.
    ///
    /// The engine keeps what a condition means rather than the object it was
    /// read from, and for a while nothing built one back — so an order read
    /// back through open orders, completed orders or a reconnect came back
    /// holding none, and placing it again placed an order that worked at once.
    #[test]
    fn a_condition_read_back_is_the_condition_that_was_sent() {
        let sent = vec![
            OrderCondition::Price {
                con_id: 756733,
                exchange: "SMART".into(),
                price: (412.25 * PRICE_SCALE_F) as Price,
                is_more: true,
                trigger_method: 0,
                is_conjunction_connection: false,
            },
            OrderCondition::Time { time: "20260101 09:30:00".into(), is_more: false, is_conjunction_connection: true },
            OrderCondition::Volume {
                con_id: 756733,
                exchange: "SMART".into(),
                volume: 1_000_000,
                is_more: true,
                is_conjunction_connection: true,
            },
        ];
        let held = crate::types::model::Order { conditions: sent.clone(), ..Default::default() };

        // The test binary embeds the interpreter rather than being loaded by
        // one, so nothing has started it yet.
        Python::initialize();
        Python::attach(|py| {
            let back = super::super::class_orders::Order::from_api(py, &held)
                .expect("the order comes back");
            assert_eq!(back.conditions.bound(py).len(), sent.len(), "each one comes back");
            assert_eq!(
                back.convert_conditions(py).expect("and reads as itself"),
                sent,
                "what the venue reported is what a caller would place again",
            );
        });
    }

    /// A price condition stating a price the wire cannot hold or a trigger the
    /// venue does not carry is refused, not changed: converted in silence, the
    /// order went to the venue waiting on a zero price and a default trigger
    /// the caller never stated.
    #[test]
    fn a_price_condition_that_cannot_be_carried_is_refused_rather_than_changed() {
        Python::initialize();
        Python::attach(|py| {
            let holding = |condition: PriceCondition| {
                let order = super::super::class_orders::Order::default();
                order.conditions.bound(py).append(condition).unwrap();
                order
            };
            let why = holding(PriceCondition {
                con_id: 756733, exchange: "SMART".into(), price: f64::NAN,
                is_more: true, trigger_method: 0, is_conjunction_connection: true,
            })
            .convert_conditions(py)
            .expect_err("a price nobody can state is refused");
            assert!(why.contains("price"), "the refusal names the price: {why}");

            let why = holding(PriceCondition {
                con_id: 756733, exchange: "SMART".into(), price: 100.0,
                is_more: true, trigger_method: 256, is_conjunction_connection: true,
            })
            .convert_conditions(py)
            .expect_err("a trigger the venue does not carry is refused");
            assert!(why.contains("trigger method"), "the refusal names the trigger: {why}");
        });

        // The same guard the other surface's orders pass through: a condition
        // built on this side in the shape the venue carries is refused there
        // too, stated with a trigger the venue does not carry.
        let built = crate::types::model::Order {
            action: "BUY".into(),
            total_quantity: 1.0,
            order_type: "LMT".into(),
            lmt_price: 10.0,
            conditions: vec![OrderCondition::Price {
                con_id: 756733,
                exchange: "SMART".into(),
                price: (100.0 * PRICE_SCALE_F) as Price,
                is_more: true,
                trigger_method: 5,
                is_conjunction_connection: true,
            }],
            ..Default::default()
        };
        let why = crate::client_core::ClientCore::validate_order(&built, "")
            .expect_err("a trigger the venue does not carry is refused on either surface");
        assert!(why.contains("trigger method"), "the refusal names the trigger: {why}");
    }
}

camel_aliases_copy! {
    PriceCondition {
        get_is_more_alias set_is_more_alias isMore is_more bool;
        get_trigger_method_alias set_trigger_method_alias triggerMethod trigger_method i32;
        get_is_conjunction_connection_alias set_is_conjunction_connection_alias isConjunctionConnection is_conjunction_connection bool;
    }
}

camel_aliases_owned! {
    PriceCondition {
        get_exchange_alias set_exchange_alias exch exchange String;
    }
}

camel_aliases_owned! {
    ExecutionCondition {
        get_sec_type_alias set_sec_type_alias secType sec_type String;
    }
}

camel_aliases_copy! {
    VolumeCondition {
        get_is_more_alias set_is_more_alias isMore is_more bool;
        get_is_conjunction_connection_alias set_is_conjunction_connection_alias isConjunctionConnection is_conjunction_connection bool;
    }
}

camel_aliases_owned! {
    VolumeCondition {
        get_exchange_alias set_exchange_alias exch exchange String;
    }
}

camel_aliases_copy! {
    PercentChangeCondition {
        get_is_more_alias set_is_more_alias isMore is_more bool;
        get_change_percent_alias set_change_percent_alias changePercent change_percent f64;
        get_is_conjunction_connection_alias set_is_conjunction_connection_alias isConjunctionConnection is_conjunction_connection bool;
    }
}

camel_aliases_owned! {
    PercentChangeCondition {
        get_exchange_alias set_exchange_alias exch exchange String;
    }
}

camel_aliases_copy! {
    TimeCondition {
        get_is_conjunction_connection_alias set_is_conjunction_connection_alias isConjunctionConnection is_conjunction_connection bool;
    }
}

camel_aliases_copy! {
    MarginCondition {
        get_is_conjunction_connection_alias set_is_conjunction_connection_alias isConjunctionConnection is_conjunction_connection bool;
    }
}

camel_aliases_copy! {
    ExecutionCondition {
        get_is_conjunction_connection_alias set_is_conjunction_connection_alias isConjunctionConnection is_conjunction_connection bool;
    }
}
