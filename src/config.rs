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

/// The locale a session announces itself with, where it states none.
pub const IB_LOCALE: &str = "en_US";

/// Network ports.
pub const MISC_PORT: u16 = 4000;

/// Which port the session opens on.
pub fn misc_port() -> u16 {
    std::env::var("IBX_MISC_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(4000)
}
/// A host to use instead of the one the venue routed to, where one
/// is set.
pub fn farm_host_override() -> Option<String> {
    std::env::var("IBX_FARM_HOST").ok()
}
/// Where the login is made.
pub const AUTH_PORT: u16 = 4001;

/// Heartbeat intervals (seconds).
pub const CCP_HEARTBEAT: u64 = 10;
/// How many seconds between heartbeats on a farm connection.
pub const FARM_HEARTBEAT: u64 = 30;

/// Recv buffer sizes (bytes).
pub const CCP_RECV_BUF: usize = 8192;
/// How much of a farm's traffic is read at once.
pub const FARM_RECV_BUF: usize = 32768;
/// How much of a FIX connection's traffic is read at once.
pub const FIX_RECV_BUF: usize = 4096;

/// Timeouts (seconds).
pub const TIMEOUT_FIX_LOGON: f64 = 10.0;
/// How long to wait for a FIX message before giving up, in seconds.
pub const TIMEOUT_FIX_READ: f64 = 30.0;
/// Overall wall-clock budget for a farm logon exchange (key exchange excluded).
/// Raised from 5 s: on a high-latency regional gateway a single response
/// segment can lag past 5 s, and the read must retry against this deadline
/// rather than treat one timeout as fatal (ibx#237).
pub const TIMEOUT_FARM_LOGON: f64 = 20.0;
/// Poll granularity for farm logon reads. Short so a transient WouldBlock /
/// TimedOut (os error 35 on macOS) is retried against the deadline instead of
/// aborting the connection (ibx#237).
pub const FARM_LOGON_POLL_MS: u64 = 250;
/// How long the login's handshake may take.
pub const TIMEOUT_SSL_AUTH: u64 = 20;
/// How long a farm connection may take to open.
pub const TIMEOUT_FARM_CONNECT: u64 = 8;

/// Protocol version.
pub const NS_VERSION: u32 = 50;
/// The oldest name-service version this client speaks.
pub const NS_VERSION_MIN: u32 = 38;

/// Stack-allocated FIX timestamp ("YYYYMMDD-HH:MM:SS"). Zero heap allocation.
pub struct TimestampBuf {
    buf: [u8; 17],
}

impl std::ops::Deref for TimestampBuf {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        // SAFETY: buf is all ASCII digits, '-', and ':'
        unsafe { std::str::from_utf8_unchecked(&self.buf) }
    }
}

impl std::fmt::Display for TimestampBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

/// FIX-compliant UTC timestamp without chrono dependency. Zero heap allocation.
pub fn chrono_free_timestamp() -> TimestampBuf {
    use std::time::SystemTime;
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let secs = dur.as_secs();
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;
    let (year, month, day) = days_to_ymd(days);
    // Write directly into a fixed buffer: "YYYYMMDD-HH:MM:SS"
    let mut buf = [b'0'; 17];
    write_u2(&mut buf[0..], (year / 100) as u8);
    write_u2(&mut buf[2..], (year % 100) as u8);
    write_u2(&mut buf[4..], month as u8);
    write_u2(&mut buf[6..], day as u8);
    buf[8] = b'-';
    write_u2(&mut buf[9..], hours as u8);
    buf[11] = b':';
    write_u2(&mut buf[12..], minutes as u8);
    buf[14] = b':';
    write_u2(&mut buf[15..], seconds as u8);
    TimestampBuf { buf }
}

/// Midnight, so many days ago, in the same form.
///
/// A window the venue is asked to answer within starts at one of these.
pub fn midnight_days_ago(days: u64) -> TimestampBuf {
    let mut stamp = chrono_free_timestamp();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (year, month, day) = days_to_ymd(secs / 86400 - days);
    write_u2(&mut stamp.buf[0..], (year / 100) as u8);
    write_u2(&mut stamp.buf[2..], (year % 100) as u8);
    write_u2(&mut stamp.buf[4..], month as u8);
    write_u2(&mut stamp.buf[6..], day as u8);
    stamp.buf[9..].copy_from_slice(b"00:00:00");
    stamp
}

/// Write a u8 as 2 zero-padded decimal digits into a byte slice.
#[inline]
fn write_u2(buf: &mut [u8], val: u8) {
    buf[0] = b'0' + val / 10;
    buf[1] = b'0' + val % 10;
}

/// Read the venue's own timestamp back to unix seconds (UTC).
///
/// It stamps `YYYYMMDD-HH:MM:SS`, sometimes with a fractional part after the
/// seconds, and sometimes joined by a space rather than a dash. All three are
/// accepted; anything else returns nothing rather than a plausible wrong
/// instant, because a clock comparison is exactly where a silently wrong number
/// does the most harm.
pub fn ib_datetime_to_unix(stamped: &str) -> Option<i64> {
    let (date, time) = stamped.split_once(['-', ' '])?;
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i64 = date[0..4].parse().ok()?;
    let month: u32 = date[4..6].parse().ok()?;
    let day: u32 = date[6..8].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // A fractional second is more precision than seconds can carry, so it is
    // dropped rather than rounded.
    let time = time.split(['.', ',']).next()?;
    let mut parts = time.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    let seconds: i64 = parts.next().unwrap_or("0").parse().ok()?;
    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) || !(0..=60).contains(&seconds) {
        return None;
    }

    let days = ymd_to_days(year, month, day)?;
    Some(days * 86_400 + hours * 3_600 + minutes * 60 + seconds)
}

/// Days since the epoch for a civil date, by the same reckoning `days_to_ymd`
/// undoes.
fn ymd_to_days(year: i64, month: u32, day: u32) -> Option<i64> {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i64;
    let d = day as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Format unix timestamp (seconds) to IB's "YYYYMMDD HH:MM:SS" format (UTC).
pub fn unix_to_ib_datetime(secs: i64) -> String {
    let secs = secs as u64;
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{year:04}{month:02}{day:02} {hours:02}:{minutes:02}:{seconds:02}"
    )
}

/// Format unix timestamp (UTC seconds) to "YYYYMMDD-HH:MM:SS" — the dash-joined
/// form the gateway requires for a time-precise good-till expiry (tag 126).
/// Distinct from `unix_to_ib_datetime` (space-joined) which other callers use.
pub fn unix_to_ib_utc_dash(secs: i64) -> String {
    let secs = secs.max(0) as u64;
    let days = secs / 86400;
    let t = secs % 86400;
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{:04}{:02}{:02}-{:02}:{:02}:{:02}",
        year, month, day, t / 3600, (t % 3600) / 60, t % 60
    )
}

/// A parsed good-till expiry: either a calendar date (no time) or a precise
/// instant in UTC. The two are mutually exclusive on the wire — date-only is
/// emitted as tag 432, time-precise as tag 126 (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IbExpiry {
    /// Date with no time component. Packed `YYYYMMDD` (e.g. 20260620).
    DateOnly(u32),
    /// Precise instant, unix seconds (UTC).
    Instant(i64),
}

/// Parse a user-supplied `good_till_date` / `good_after_time` string into an
/// `IbExpiry`. Returns `Ok(None)` for an empty string.
///
/// Accepted input forms (the same the official API accepts):
///   - `YYYYMMDD`                          → date-only
///   - `YYYYMMDD HH:MM:SS`                 → time, no timezone
///   - `YYYYMMDD-HH:MM:SS`                 → time, no timezone (dash separator)
///   - `YYYYMMDD HH:MM:SS <IANA zone>`     → time in a named zone (e.g. `US/Eastern`)
///
/// A named timezone is converted to UTC with DST applied (matching what the
/// gateway does). A time with no timezone is interpreted as UTC and logged —
/// the gateway's implied-timezone behavior is deprecated, so callers should
/// pass an explicit zone or UTC.
pub fn parse_ib_expiry(input: &str) -> Result<Option<IbExpiry>, String> {
    let s = input.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.len() < 8 || !s.as_bytes()[..8].iter().all(|b| b.is_ascii_digit()) {
        return Err(format!("expiry '{input}': must start with YYYYMMDD"));
    }
    let ymd: u32 = s[..8].parse().unwrap(); // 8 ascii digits — infallible
    let year: i16 = s[0..4].parse().unwrap();
    let month: i8 = s[4..6].parse().unwrap();
    let day: i8 = s[6..8].parse().unwrap();

    // Validate the date regardless of whether a time follows.
    let date = jiff::civil::Date::new(year, month, day)
        .map_err(|e| format!("expiry '{input}': {e}"))?;

    // Strip the date, then an optional `-` or whitespace separator before the time.
    let rest = s[8..].strip_prefix('-').unwrap_or(&s[8..]).trim();
    if rest.is_empty() {
        return Ok(Some(IbExpiry::DateOnly(ymd)));
    }

    // Split the time token from an optional trailing timezone token.
    let mut it = rest.splitn(2, char::is_whitespace);
    let time_str = it.next().unwrap();
    let tz = it.next().map(str::trim).filter(|t| !t.is_empty());

    let tp: Vec<&str> = time_str.split(':').collect();
    if tp.len() != 3 {
        return Err(format!("expiry '{input}': time must be HH:MM:SS"));
    }
    let parse_u = |p: &str, what: &str| -> Result<i8, String> {
        p.parse::<i8>()
            .map_err(|_| format!("expiry '{input}': invalid {what}"))
    };
    let (h, mi, sec) = (
        parse_u(tp[0], "hour")?,
        parse_u(tp[1], "minute")?,
        parse_u(tp[2], "second")?,
    );
    let time =
        jiff::civil::Time::new(h, mi, sec, 0).map_err(|e| format!("expiry '{input}': {e}"))?;
    let dt = date.to_datetime(time);

    let zone = match tz {
        Some(z) => z,
        None => {
            log::warn!(
                "good-till expiry '{input}' has a time but no timezone; interpreting as UTC. \
                 Pass an explicit zone (e.g. 'US/Eastern') or UTC."
            );
            "UTC"
        }
    };
    // Ask for the name as given first: a host that has the legacy names, or a
    // deliberately customised database, should answer for itself. The mapping
    // is a fallback for the hosts that do not carry them, not an override.
    let zoned = match dt.in_tz(zone) {
        Ok(zoned) => zoned,
        Err(_) => dt
            .in_tz(canonical_zone(zone))
            .map_err(|e| format!("expiry '{input}': unknown timezone '{zone}': {e}"))?,
    };
    Ok(Some(IbExpiry::Instant(zoned.timestamp().as_second())))
}

/// Resolve the legacy zone names IB states its times in.
///
/// `US/Eastern` and its siblings are backward-compatibility links in the tz
/// database, and Debian and Ubuntu ship those in a separate package that is not
/// installed by default. Resolving them to the primary name keeps an expiry
/// from being dropped on a host that has only the primary names — which
/// includes a stock container image.
fn canonical_zone(zone: &str) -> &str {
    match zone {
        "US/Eastern" => "America/New_York",
        "US/Central" => "America/Chicago",
        "US/Mountain" => "America/Denver",
        "US/Pacific" => "America/Los_Angeles",
        "US/Alaska" => "America/Anchorage",
        "US/Hawaii" => "Pacific/Honolulu",
        "US/Arizona" => "America/Phoenix",
        other => other,
    }
}

/// Convert days since Unix epoch to (year, month, day).
pub fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod expiry_tests {
    use super::*;

    fn instant(s: &str) -> i64 {
        match parse_ib_expiry(s).unwrap().unwrap() {
            IbExpiry::Instant(secs) => secs,
            other => panic!("expected Instant, got {other:?}"),
        }
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(parse_ib_expiry("").unwrap(), None);
        assert_eq!(parse_ib_expiry("   ").unwrap(), None);
    }

    #[test]
    fn date_only() {
        assert_eq!(
            parse_ib_expiry("20260620").unwrap(),
            Some(IbExpiry::DateOnly(20260620))
        );
    }

    #[test]
    fn named_zone_converts_with_dst() {
        // June -> US/Eastern is EDT (UTC-4): 18:00 local == 22:00 UTC.
        // Matches the gateway capture (ib-agent#158).
        let eastern = instant("20260620 18:00:00 US/Eastern");
        let utc = instant("20260620 22:00:00 UTC");
        assert_eq!(eastern, utc, "EDT 18:00 must equal 22:00 UTC");
    }

    #[test]
    fn no_timezone_is_utc() {
        // Both separators accepted; absent zone treated as UTC.
        let dash = instant("20260620-18:00:00");
        let space = instant("20260620 18:00:00");
        let utc = instant("20260620 18:00:00 UTC");
        assert_eq!(dash, space);
        assert_eq!(dash, utc);
    }

    #[test]
    fn instant_round_trips_to_wire() {
        // parse -> seconds -> tag 126 wire string must be the dash UTC form.
        let secs = instant("20260620 18:00:00 US/Eastern");
        assert_eq!(unix_to_ib_utc_dash(secs), "20260620-22:00:00");
    }

    /// IB states its times in the legacy zone names, and those are a separate,
    /// not-installed-by-default package on Debian and Ubuntu. Without resolving
    /// them the expiry fails to parse, `attrs()` logs and drops it, and a GTD
    /// order goes out with no expiry at all.
    #[test]
    fn legacy_zone_names_map_to_their_documented_targets() {
        // Asserted directly, because comparing one summer instant cannot tell a
        // correct target from an offset-equivalent wrong one: Alaska and
        // Pitcairn agree in June and differ by an hour in December.
        for (legacy, primary) in [
            ("US/Eastern", "America/New_York"),
            ("US/Central", "America/Chicago"),
            ("US/Mountain", "America/Denver"),
            ("US/Pacific", "America/Los_Angeles"),
            ("US/Alaska", "America/Anchorage"),
            ("US/Hawaii", "Pacific/Honolulu"),
            ("US/Arizona", "America/Phoenix"),
        ] {
            assert_eq!(canonical_zone(legacy), primary, "{legacy}");
        }
        // Anything already primary, and anything non-US, passes through.
        for untouched in ["America/New_York", "Europe/London", "Asia/Tokyo", "UTC", ""] {
            assert_eq!(canonical_zone(untouched), untouched);
        }
    }

    #[test]
    fn legacy_zone_names_resolve_to_the_same_instant() {
        for (legacy, primary) in [
            ("US/Eastern", "America/New_York"),
            ("US/Central", "America/Chicago"),
            ("US/Mountain", "America/Denver"),
            ("US/Pacific", "America/Los_Angeles"),
            ("US/Alaska", "America/Anchorage"),
            ("US/Hawaii", "Pacific/Honolulu"),
            ("US/Arizona", "America/Phoenix"),
        ] {
            // Both seasons: a wrong target that happens to share an offset in
            // summer usually differs in winter.
            for date in ["20260620", "20261215"] {
                assert_eq!(
                    instant(&format!("{date} 18:00:00 {legacy}")),
                    instant(&format!("{date} 18:00:00 {primary}")),
                    "{legacy} must resolve like {primary} on {date}",
                );
            }
        }
        // A name that is already primary is untouched.
        assert_eq!(canonical_zone("Europe/London"), "Europe/London");
        assert_eq!(canonical_zone("UTC"), "UTC");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_ib_expiry("2026").is_err());
        assert!(parse_ib_expiry("20260620 18:00").is_err()); // needs seconds
        assert!(parse_ib_expiry("20261320").is_err()); // month 13
        assert!(parse_ib_expiry("20260620 18:00:00 Mars/Olympus").is_err());
    }
}

#[cfg(test)]
mod venue_clock_tests {
    use super::*;

    /// The venue's stamp and this client's own formatting are inverses, so a
    /// time read back is the time that was sent.
    #[test]
    fn a_venue_timestamp_reads_back_to_the_instant_it_names() {
        for secs in [0_i64, 1_000_000_000, 1_767_225_600, 2_000_000_000] {
            let written = unix_to_ib_utc_dash(secs);
            assert_eq!(ib_datetime_to_unix(&written), Some(secs), "{written}");
        }
    }

    /// The venue joins with a dash; some messages use a space. Both are its
    /// own timestamp and both must read.
    #[test]
    fn both_joins_the_venue_uses_are_read() {
        assert_eq!(
            ib_datetime_to_unix("20260101-00:00:00"),
            ib_datetime_to_unix("20260101 00:00:00"),
        );
    }

    /// More precision than seconds can carry is dropped, not rounded.
    #[test]
    fn a_fractional_second_is_dropped_rather_than_rounded() {
        let whole = ib_datetime_to_unix("20260101-12:30:45").unwrap();
        assert_eq!(ib_datetime_to_unix("20260101-12:30:45.999"), Some(whole));
    }

    /// Anything that is not one of its timestamps returns nothing rather than
    /// a plausible instant. A clock comparison is where a silently wrong
    /// number does the most harm.
    #[test]
    fn something_that_is_not_a_timestamp_is_refused() {
        for bad in ["", "not a time", "20260101", "2026010-12:00:00", "20261301-00:00:00", "20260101-25:00:00"] {
            assert_eq!(ib_datetime_to_unix(bad), None, "{bad}");
        }
    }
}
