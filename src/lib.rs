// A wire message carries the fields the protocol says it carries, and the
// function that builds one takes them. Splitting a builder into a struct to
// satisfy an arbitrary count would put a layer between the caller and the
// message without making either clearer.
#![allow(clippy::too_many_arguments)]

pub mod api;
pub mod auth;
pub mod bridge;
pub mod client_core;
pub mod config;
pub mod control;
pub mod gateway;
pub mod logging;
pub mod protocol;
pub mod types;

/// Internal engine module. Use [`api::EClient`] for the public API.
#[doc(hidden)]
pub mod engine;

#[cfg(feature = "python")]
mod python;

// Re-exports for convenience.
pub use api::error_codes::Refusal;
pub use api::{EClient, EClientConfig, Wrapper};
