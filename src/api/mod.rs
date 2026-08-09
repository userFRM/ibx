//! ibapi-compatible Rust API.
//!
//! Provides `Contract`, `Order`, `Wrapper` trait, and `EClient` — matching the
//! C++ TWS API (EClientSocket / EWrapper) pattern.

pub mod client;
pub mod direct;
pub mod reliability;
pub mod types;
pub mod wrapper;

pub use client::{EClient, EClientConfig};
pub use direct::Client;
pub use types::*;
pub use wrapper::Wrapper;
