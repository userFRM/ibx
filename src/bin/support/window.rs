//! When a run may take the live account.

/// Whether the live account is free to be logged in to.
///
/// A daemon trades that account through the session, so a capture takes it
/// only outside session hours: before 09:15 and from 16:15, New York, where
/// the hours are stated. A clock that cannot be read counts as inside the
/// session, because refusing a run is recoverable and taking the account from
/// something already trading it is not.
pub fn live_window_is_open() -> bool {
    let out = std::process::Command::new("date")
        .env("TZ", "America/New_York")
        .arg("+%H%M")
        .output()
        .ok();
    let hhmm: u32 = out
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(1200);
    !(915..1615).contains(&hhmm)
}
