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

/// The camelCase names the reference client gives these fields, where the whole
/// accessor is "hand the field over".
///
/// Two macros rather than one with a branch: the interpreter's own attribute
/// macro cannot see through a nested expansion, so the ownership has to be
/// decided at the call. `copy` is for fields that are copied and `owned` for
/// fields that are cloned, which keeps a clone off the ones that do not need
/// one instead of hiding it inside a macro where the lint cannot see it.
///
/// Aliases that build their answer — anything needing the interpreter, or a
/// field that is not handed over as it stands — stay written out.
macro_rules! camel_aliases_copy {
    ($cls:ident { $($get:ident $set:ident $py:ident $rust:ident $ty:ty;)* }) => {
        #[pymethods]
        impl $cls {
            $(
                #[getter($py)]
                fn $get(&self) -> $ty { self.$rust }
                #[setter($py)]
                fn $set(&mut self, v: $ty) { self.$rust = v; }
            )*
        }
    };
}

macro_rules! camel_aliases_owned {
    ($cls:ident { $($get:ident $set:ident $py:ident $rust:ident $ty:ty;)* }) => {
        #[pymethods]
        impl $cls {
            $(
                #[getter($py)]
                fn $get(&self) -> $ty { self.$rust.clone() }
                #[setter($py)]
                fn $set(&mut self, v: $ty) { self.$rust = v; }
            )*
        }
    };
}

pub(crate) use {camel_aliases_copy, camel_aliases_owned};
