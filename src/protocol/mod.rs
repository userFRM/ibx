//! The wire itself: how a message is framed, signed, compressed and read.
//!
//! Every parser here is given malformed input by `tests/malformed_input.rs`
//! — each prefix of a well-formed frame, that frame with a byte replaced
//! at each position, and runs that are not frames at all — because a
//! client that panics on one bad frame loses the session and every
//! subscription on it.
//!
//! Not the caller-facing surface. What a program written against this client
//! touches is [`crate::api`], which is documented in full and gated on staying
//! that way. This module is the engine underneath it, exported because the
//! binaries, benchmarks and integration tests in this repository reach it.

/// The venue's own way of writing a date and a time.
pub mod datetime;
pub mod connection;
pub mod fix;
pub mod tbt_stream;
pub mod trading_status;
pub mod fixcomp;
pub mod ns;
pub mod routing;
pub mod tick_decoder;
pub mod xyz;
