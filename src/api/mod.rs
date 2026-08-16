//! ibapi-compatible Rust API.
//!
//! Provides `Contract`, `Order`, `Wrapper` trait, and `EClient` — matching the
//! C++ TWS API (EClientSocket / EWrapper) pattern.

pub mod client;
pub mod direct;
pub mod subscription;
pub mod types;
pub mod wrapper;

// Reachable under `api::` because that is where callers first met them, and
// the names a published surface hands out do not move when the code does.
pub use crate::{error_codes, reliability, settings};

pub use client::{EClient, EClientConfig};
pub use direct::Client;
pub use subscription::Subscription;
pub use types::*;
pub use wrapper::Wrapper;
