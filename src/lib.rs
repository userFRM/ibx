//! A client for the Interactive Brokers protocol, with no gateway in between.
//!
//! This authenticates against IBKR's servers and holds the four connections a
//! session runs on — market data, trading, historical, and security
//! definitions — in one process. There is no gateway to install, no JVM, and
//! no socket on localhost to connect to, authorise or keep alive.
//!
//! # Where to start
//!
//! [`api`] is the surface a program touches, and the only part covered by the
//! compatibility promise. It carries the reference client's own shapes:
//!
//! - [`EClient`] for requests and [`Wrapper`] for what arrives
//! - [`api::direct::Client`] for the same session with answers returned rather
//!   than delivered on callbacks
//!
//! ```no_run
//! use ibx::api::client::{EClient, EClientConfig};
//!
//! let client = EClient::connect(&EClientConfig {
//!     username: "…".into(),
//!     password: "…".into(),
//!     paper: true,
//!     ..Default::default()
//! })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! In Python, the same surface is `ibx.EClient` and `ibx.EWrapper`, and an
//! unmodified `ib_async` program runs on it through `ibx.ib_async.attach`.
//!
//! # Everything else
//!
//! The other modules are the engine: how a session is opened, how a message is
//! framed and read, and what the venue says about things that are not prices.
//! They are exported because this repository's own binaries, benchmarks and
//! integration tests reach them, and they are not the compatibility surface.
//!
//! # Prices and sizes
//!
//! Held as integers, scaled by [`types::PRICE_SCALE`] and [`types::QTY_SCALE`],
//! and turned into floating point at the caller's edge and nowhere before it.

// A wire message carries the fields the protocol says it carries, and the
// function that builds one takes them. Splitting a builder into a struct to
// satisfy an arbitrary count would put a layer between the caller and the
// message without making either clearer.
#![allow(clippy::too_many_arguments)]
// Every public item says what it is. A field with no statement is one a reader
// has to infer from its name and a caller has to discover by watching what
// arrives, which is what most of this used to be.
#![deny(missing_docs)]

/// The surface a program written against this client touches: its requests,
/// its callbacks, and the types they carry.
///
/// Documented in full, and required to stay that way. A callback that says
/// nothing is one a caller has to discover by watching what arrives, which is
/// what the whole of this surface used to be.
pub mod api;
pub mod auth;
pub mod bridge;
pub mod client_core;
pub mod config;
pub mod control;
pub mod gateway;
pub mod logging;
/// The last order id handed out, kept between runs.
pub mod order_ids;
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
