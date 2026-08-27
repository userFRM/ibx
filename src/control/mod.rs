//! Reading and writing what the venue says about things that are not
//! prices: contracts, accounts, historical series, news, scanners, the
//! corporate-events calendar.
//!
//! Each submodule owns one message family — how a request is built and
//! how the answer is read.
//!
//! Not the caller-facing surface. What a program written against this client
//! touches is [`crate::api`], which is documented in full and gated on staying
//! that way. This module is the engine underneath it, exported because the
//! binaries, benchmarks and integration tests in this repository reach it.

pub mod adjustments;
pub mod calendar;
pub mod contracts;
pub mod fundamental;
pub mod histogram;
pub mod historical;
pub mod news;
pub mod option_model;
pub mod scanner;
pub mod xml;
