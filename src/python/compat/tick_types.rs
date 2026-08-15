//! ibapi-compatible tick type constants and TickAttrib classes.

use pyo3::prelude::*;

// ── Tick type constants matching ibapi's TickTypeEnum ──

pub const TICK_BID_SIZE: i32 = 0;
pub const TICK_BID: i32 = 1;
pub const TICK_ASK: i32 = 2;
pub const TICK_ASK_SIZE: i32 = 3;
pub const TICK_LAST: i32 = 4;
pub const TICK_LAST_SIZE: i32 = 5;
pub const TICK_HIGH: i32 = 6;
pub const TICK_LOW: i32 = 7;
pub const TICK_VOLUME: i32 = 8;
pub const TICK_CLOSE: i32 = 9;
pub const TICK_OPEN: i32 = 14;
pub const TICK_BID_EXCHANGE: i32 = 32;
pub const TICK_ASK_EXCHANGE: i32 = 33;
pub const TICK_LAST_TIMESTAMP: i32 = 45;
pub const TICK_HALTED: i32 = 49;
pub const TICK_LAST_EXCHANGE: i32 = 84;

/// ibapi-compatible TickAttrib for tickPrice callbacks.
#[pyclass(from_py_object)]
#[derive(Clone, Default)]
pub struct TickAttrib {
    #[pyo3(get, set)]
    pub can_auto_execute: bool,
    #[pyo3(get, set)]
    pub past_limit: bool,
    #[pyo3(get, set)]
    pub pre_open: bool,
}

#[pymethods]
impl TickAttrib {
    #[new]
    #[pyo3(signature = (can_auto_execute=false, past_limit=false, pre_open=false))]
    fn new(can_auto_execute: bool, past_limit: bool, pre_open: bool) -> Self {
        Self { can_auto_execute, past_limit, pre_open }
    }

    fn __repr__(&self) -> String {
        format!("TickAttrib(canAutoExecute={}, pastLimit={}, preOpen={})",
            self.can_auto_execute, self.past_limit, self.pre_open)
    }
}

/// ibapi-compatible TickAttribLast for tick-by-tick last/allLast callbacks.
#[pyclass(from_py_object)]
#[derive(Clone, Default)]
pub struct TickAttribLast {
    #[pyo3(get, set)]
    pub past_limit: bool,
    #[pyo3(get, set)]
    pub unreported: bool,
}

#[pymethods]
impl TickAttribLast {
    #[new]
    #[pyo3(signature = (past_limit=false, unreported=false))]
    fn new(past_limit: bool, unreported: bool) -> Self {
        Self { past_limit, unreported }
    }

    fn __repr__(&self) -> String {
        format!("TickAttribLast(pastLimit={}, unreported={})", self.past_limit, self.unreported)
    }
}

/// ibapi-compatible TickAttribBidAsk for tick-by-tick bid/ask callbacks.
#[pyclass(from_py_object)]
#[derive(Clone, Default)]
pub struct TickAttribBidAsk {
    #[pyo3(get, set)]
    pub bid_past_low: bool,
    #[pyo3(get, set)]
    pub ask_past_high: bool,
}

#[pymethods]
impl TickAttribBidAsk {
    #[new]
    #[pyo3(signature = (bid_past_low=false, ask_past_high=false))]
    fn new(bid_past_low: bool, ask_past_high: bool) -> Self {
        Self { bid_past_low, ask_past_high }
    }

    fn __repr__(&self) -> String {
        format!("TickAttribBidAsk(bidPastLow={}, askPastHigh={})", self.bid_past_low, self.ask_past_high)
    }
}

/// Module-level TickTypeEnum class for accessing tick type constants.
#[pyclass]
pub struct TickTypeEnum;

#[pymethods]
impl TickTypeEnum {
    #[classattr]
    const BID_SIZE: i32 = TICK_BID_SIZE;
    #[classattr]
    const BID: i32 = TICK_BID;
    #[classattr]
    const ASK: i32 = TICK_ASK;
    #[classattr]
    const ASK_SIZE: i32 = TICK_ASK_SIZE;
    #[classattr]
    const LAST: i32 = TICK_LAST;
    #[classattr]
    const LAST_SIZE: i32 = TICK_LAST_SIZE;
    #[classattr]
    const HIGH: i32 = TICK_HIGH;
    #[classattr]
    const LOW: i32 = TICK_LOW;
    #[classattr]
    const VOLUME: i32 = TICK_VOLUME;
    #[classattr]
    const CLOSE: i32 = TICK_CLOSE;
    #[classattr]
    const OPEN: i32 = TICK_OPEN;
    #[classattr]
    const LAST_TIMESTAMP: i32 = TICK_LAST_TIMESTAMP;
    #[classattr]
    const HALTED: i32 = TICK_HALTED;
    #[classattr]
    const BID_EXCHANGE: i32 = TICK_BID_EXCHANGE;
    #[classattr]
    const ASK_EXCHANGE: i32 = TICK_ASK_EXCHANGE;
    #[classattr]
    const LAST_EXCHANGE: i32 = TICK_LAST_EXCHANGE;
}

/// Register tick type classes and constants on the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<TickTypeEnum>()?;
    m.add_class::<TickAttrib>()?;
    m.add_class::<TickAttribLast>()?;
    m.add_class::<TickAttribBidAsk>()?;
    m.add_class::<HistoricalTick>()?;
    m.add_class::<HistoricalTickLast>()?;
    m.add_class::<HistoricalTickBidAsk>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_type_constants_match_ibapi() {
        assert_eq!(TICK_BID_SIZE, 0);
        assert_eq!(TICK_BID, 1);
        assert_eq!(TICK_ASK, 2);
        assert_eq!(TICK_ASK_SIZE, 3);
        assert_eq!(TICK_LAST, 4);
        assert_eq!(TICK_LAST_SIZE, 5);
        assert_eq!(TICK_HIGH, 6);
        assert_eq!(TICK_LOW, 7);
        assert_eq!(TICK_VOLUME, 8);
        assert_eq!(TICK_CLOSE, 9);
        assert_eq!(TICK_OPEN, 14);
        assert_eq!(TICK_LAST_TIMESTAMP, 45);
        assert_eq!(TICK_HALTED, 49);
    }

    #[test]
    fn tick_attrib_defaults() {
        let ta = TickAttrib::default();
        assert!(!ta.can_auto_execute);
        assert!(!ta.past_limit);
        assert!(!ta.pre_open);
    }

    #[test]
    fn tick_attrib_last_defaults() {
        let ta = TickAttribLast::default();
        assert!(!ta.past_limit);
        assert!(!ta.unreported);
    }

    #[test]
    fn tick_attrib_bid_ask_defaults() {
        let ta = TickAttribBidAsk::default();
        assert!(!ta.bid_past_low);
        assert!(!ta.ask_past_high);
    }
}

/// One historical tick, as the reference client hands it over.
///
/// A tuple carries the same numbers and answers to none of the names, so a
/// program reading `tick.price` off what it was given finds nothing there.
/// `size` is stated where the venue states one; a midpoint has none.
#[pyclass(from_py_object)]
#[derive(Clone, Default)]
pub struct HistoricalTick {
    #[pyo3(get, set)]
    pub time: i64,
    #[pyo3(get, set)]
    pub price: f64,
    #[pyo3(get, set)]
    pub size: f64,
}

#[pymethods]
impl HistoricalTick {
    #[new]
    #[pyo3(signature = (time=0, price=0.0, size=0.0))]
    fn new(time: i64, price: f64, size: f64) -> Self {
        Self { time, price, size }
    }

    fn __repr__(&self) -> String {
        format!("HistoricalTick(time={}, price={}, size={})", self.time, self.price, self.size)
    }
}

/// One historical trade, with what the venue said about it.
#[pyclass(from_py_object)]
#[derive(Clone, Default)]
pub struct HistoricalTickLast {
    #[pyo3(get, set)]
    pub time: i64,
    #[pyo3(get, set)]
    pub tick_attrib_last: TickAttribLast,
    #[pyo3(get, set)]
    pub price: f64,
    #[pyo3(get, set)]
    pub size: f64,
    #[pyo3(get, set)]
    pub exchange: String,
    #[pyo3(get, set)]
    pub special_conditions: String,
}

#[pymethods]
impl HistoricalTickLast {
    #[new]
    #[pyo3(signature = (time=0, tick_attrib_last=TickAttribLast::default(), price=0.0,
                        size=0.0, exchange=String::new(), special_conditions=String::new()))]
    fn new(
        time: i64, tick_attrib_last: TickAttribLast, price: f64, size: f64,
        exchange: String, special_conditions: String,
    ) -> Self {
        Self { time, tick_attrib_last, price, size, exchange, special_conditions }
    }

    #[getter(tickAttribLast)]
    fn get_tick_attrib_last(&self) -> TickAttribLast { self.tick_attrib_last.clone() }
    #[setter(tickAttribLast)]
    fn set_tick_attrib_last(&mut self, v: TickAttribLast) { self.tick_attrib_last = v; }
    #[getter(specialConditions)]
    fn get_special_conditions(&self) -> String { self.special_conditions.clone() }
    #[setter(specialConditions)]
    fn set_special_conditions(&mut self, v: String) { self.special_conditions = v; }

    fn __repr__(&self) -> String {
        format!("HistoricalTickLast(time={}, price={}, size={}, exchange={})",
            self.time, self.price, self.size, self.exchange)
    }
}

/// One historical quote, both sides of it.
#[pyclass(from_py_object)]
#[derive(Clone, Default)]
pub struct HistoricalTickBidAsk {
    #[pyo3(get, set)]
    pub time: i64,
    #[pyo3(get, set)]
    pub tick_attrib_bid_ask: TickAttribBidAsk,
    #[pyo3(get, set)]
    pub price_bid: f64,
    #[pyo3(get, set)]
    pub price_ask: f64,
    #[pyo3(get, set)]
    pub size_bid: f64,
    #[pyo3(get, set)]
    pub size_ask: f64,
}

#[pymethods]
impl HistoricalTickBidAsk {
    #[new]
    #[pyo3(signature = (time=0, tick_attrib_bid_ask=TickAttribBidAsk::default(),
                        price_bid=0.0, price_ask=0.0, size_bid=0.0, size_ask=0.0))]
    fn new(
        time: i64, tick_attrib_bid_ask: TickAttribBidAsk,
        price_bid: f64, price_ask: f64, size_bid: f64, size_ask: f64,
    ) -> Self {
        Self { time, tick_attrib_bid_ask, price_bid, price_ask, size_bid, size_ask }
    }

    #[getter(tickAttribBidAsk)]
    fn get_tick_attrib_bid_ask(&self) -> TickAttribBidAsk { self.tick_attrib_bid_ask.clone() }
    #[setter(tickAttribBidAsk)]
    fn set_tick_attrib_bid_ask(&mut self, v: TickAttribBidAsk) { self.tick_attrib_bid_ask = v; }
    #[getter(priceBid)]
    fn get_price_bid(&self) -> f64 { self.price_bid }
    #[setter(priceBid)]
    fn set_price_bid(&mut self, v: f64) { self.price_bid = v; }
    #[getter(priceAsk)]
    fn get_price_ask(&self) -> f64 { self.price_ask }
    #[setter(priceAsk)]
    fn set_price_ask(&mut self, v: f64) { self.price_ask = v; }
    #[getter(sizeBid)]
    fn get_size_bid(&self) -> f64 { self.size_bid }
    #[setter(sizeBid)]
    fn set_size_bid(&mut self, v: f64) { self.size_bid = v; }
    #[getter(sizeAsk)]
    fn get_size_ask(&self) -> f64 { self.size_ask }
    #[setter(sizeAsk)]
    fn set_size_ask(&mut self, v: f64) { self.size_ask = v; }

    fn __repr__(&self) -> String {
        format!("HistoricalTickBidAsk(time={}, bid={}, ask={})",
            self.time, self.price_bid, self.price_ask)
    }
}
