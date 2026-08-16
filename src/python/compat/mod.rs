//! ibapi-compatible layer: EWrapper, EClient, Contract, Order, tick types.

pub mod client;
pub mod contract;
/// The contract classes a caller works in.
pub mod class_contracts;
/// The order classes a caller works in.
pub mod class_orders;
/// What an order waits for before it works.
pub mod class_conditions;
/// What the venue reports back.
pub mod class_reports;
pub mod tick_types;
pub mod wrapper;

use pyo3::prelude::*;

/// Register all compat classes on the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    contract::register(m)?;
    tick_types::register(m)?;
    wrapper::register(m)?;
    client::register(m)?;
    Ok(())
}
