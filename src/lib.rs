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
// Every public item states what it is. A field without one is inferred from its
// name and discovered by observing what arrives.
#![deny(missing_docs)]

/// The surface a program written against this client touches: its requests,
/// its callbacks, and the types they carry.
///
/// Documented in full, and required to stay so by `deny(missing_docs)`.
pub mod api;

// Each of these documents itself. A second doc comment here would be
// concatenated with the module's own and resolved in this file's scope, where
// the names it links to are not.
pub mod error_codes;
pub mod reliability;
pub mod settings;
pub mod types;

// ── Not the surface ─────────────────────────────────────────────────────────
//
// Public because the binaries, benchmarks and integration tests in this
// repository reach them from outside the crate, and hidden from the
// documentation for the same reason: a consumer who builds against one of
// these is building against something that will move. Every one of them
// already says so in prose; the attribute makes rustdoc agree.

#[doc(hidden)]
pub mod auth;
#[doc(hidden)]
pub mod bridge;
#[doc(hidden)]
pub mod client_core;
#[doc(hidden)]
pub mod config;
#[doc(hidden)]
pub mod control;
#[doc(hidden)]
pub mod engine;
#[doc(hidden)]
pub mod gateway;
#[doc(hidden)]
pub mod logging;
/// The last order id handed out, kept between runs.
#[doc(hidden)]
pub mod order_ids;
#[doc(hidden)]
pub mod protocol;

#[cfg(feature = "python")]
mod python;

// Re-exports for convenience.
pub use error_codes::Refusal;
pub use api::{EClient, EClientConfig, Wrapper};

/// The client, under the name a program that is not being migrated would look
/// for. The same type as [`EClient`], which keeps the reference client's name
/// for a program that is.
pub use api::EClient as Client;

/// The same session, for a program already running an asynchronous runtime.
#[cfg(feature = "async")]
pub use api::client::AsyncClient;
