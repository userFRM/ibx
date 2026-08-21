//! What this client announces itself as, and where it connects by default.
//!
//! Not the caller-facing surface. What a program written against this client
//! touches is [`crate::api`], which is documented in full and gated on staying
//! that way. This module is the engine underneath it, exported because the
//! binaries, benchmarks and integration tests in this repository reach it.

/// Client version identifiers.
///
/// These are what the client states about itself at logon, and the vendor
/// moves its own every few weeks. `IBX_BUILD` and `IBX_VERSION` override them
/// so a session can be pointed at a newer pair without waiting for a release —
/// which is the difference between a stale constant costing a restart and it
/// costing an outage, on the day the server stops accepting this one.
pub const IB_BUILD: &str = "10401";
/// What this client announces as its version.
pub const IB_VERSION: &str = "c";
/// What it announces as its client string.
pub const IB_ENCODED: &str = "17.0.10.0.101/W/en_US/G";

/// The build this client announces. Overridable for a session that must
/// match a particular one.
pub fn ib_build() -> String {
    std::env::var("IBX_BUILD").unwrap_or_else(|_| IB_BUILD.to_string())
}
/// The version it announces.
pub fn ib_version() -> String {
    std::env::var("IBX_VERSION").unwrap_or_else(|_| IB_VERSION.to_string())
}

/// The doors this client knocks on, in the order it tries them.
///
/// One per region the venue serves. Whichever answers routes the session to
/// where its account actually lives, so the order is a matter of which is
/// nearest rather than which is correct — every one of them is.
pub const CCP_HOSTS: &[&str] = &[
    "cdc1.ibllc.com",
    "ndc1.ibllc.com",
    "zdc1.ibllc.com",
    "hdc1.ibllc.com",
];

/// How long a caller waits for an answer before being told none came.
///
/// Every deadline the engine keeps for a request must be shorter than this, or
/// the caller gives up first and is told nothing arrived — while the engine is
/// still holding the reason the venue gave, which it then reports to nobody. A
/// caller should hear why, not that it waited.
pub const ANSWER_TIMEOUT_SECS: u64 = 15;

/// The locale a session announces itself with, where it states none.
pub const IB_LOCALE: &str = "en_US";

/// Network ports.
pub const MISC_PORT: u16 = 4000;

/// Where the login is made.
pub const AUTH_PORT: u16 = 4001;

/// Heartbeat intervals (seconds).
pub const CCP_HEARTBEAT: u64 = 10;
/// How many seconds between heartbeats on a farm connection.
pub const FARM_HEARTBEAT: u64 = 30;

/// How much of a farm's traffic is read at once.
pub const FARM_RECV_BUF: usize = 32768;
/// Timeouts (seconds).
pub const TIMEOUT_FIX_LOGON: f64 = 10.0;
/// Overall wall-clock budget for a farm logon exchange (key exchange excluded).
/// Raised from 5 s: on a high-latency regional gateway a single response
/// segment can lag past 5 s, and the read must retry against this deadline
/// rather than treat one timeout as fatal.
pub const TIMEOUT_FARM_LOGON: f64 = 20.0;
/// Poll granularity for farm logon reads. Short so a transient WouldBlock /
/// TimedOut (os error 35 on macOS) is retried against the deadline instead of
/// aborting the connection.
pub const FARM_LOGON_POLL_MS: u64 = 250;
/// How long the login's handshake may take.
pub const TIMEOUT_SSL_AUTH: u64 = 20;
/// How long a farm connection may take to open.
pub const TIMEOUT_FARM_CONNECT: u64 = 8;

/// Protocol version.
pub const NS_VERSION: u32 = 51;
/// The oldest name-service version this client speaks.
pub const NS_VERSION_MIN: u32 = 38;

// The venue's date format moved to `protocol::datetime` when it stopped being
// a constant and started being a codec. Reachable here because that is the
// path a program written against this client already names.
pub use crate::protocol::datetime::{
    IbExpiry, TimestampBuf, chrono_free_timestamp, days_to_ymd, ib_datetime_to_unix,
    midnight_days_ago, parse_ib_expiry, unix_to_ib_datetime, unix_to_ib_utc_dash,
};
