//! The loop that owns the connections.
//!
//! One thread reads every socket, answers heartbeats, decodes what
//! arrives and hands it to the client through [`crate::bridge`]. A caller
//! never touches it directly: requests reach it as commands and answers
//! leave it as events.
//!
//! Not the caller-facing surface. What a program written against this client
//! touches is [`crate::api`], which is documented in full and gated on staying
//! that way. This module is the engine underneath it, exported because the
//! binaries, benchmarks and integration tests in this repository reach it.

pub mod context;
pub mod hot_loop;
pub mod market_state;
