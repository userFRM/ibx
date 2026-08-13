//! Opening a session: the key exchange, the login, and the second factor.
//!
//! The venue answers a login with what the session is allowed to do and
//! where its connections live. A live login enters a second-factor
//! approval window and blocks until it is approved or the deadline fires;
//! a paper login differs by one step, a token conversion and which slot
//! its hash occupies, and skips the gate.
//!
//! Not the caller-facing surface. What a program written against this client
//! touches is [`crate::api`], which is documented in full and gated on staying
//! that way. This module is the engine underneath it, exported because the
//! binaries, benchmarks and integration tests in this repository reach it.

pub mod crypto;
pub mod dh;
pub mod resume;
pub mod session;
pub mod srp;
