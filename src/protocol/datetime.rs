//! The date and time format the venue uses.
//!
//! Every stamp on the wire is `yyyyMMdd-HH:mm:ss`, or a date on its own, or a
//! date with a zone after it. Reading and writing them is a codec like every
//! other format in this module — it lived among the venue's constants because
//! that is where the first one was needed.
//!
//! No calendar crate: the conversions are civil-date arithmetic on a Unix
//! second, and what this client needs from a date library is all here.

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
        .unwrap_or_default()
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

/// How many days that month has, leap years included.
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Read the venue's timestamp back to unix seconds (UTC).
///
/// It stamps `YYYYMMDD-HH:MM:SS`, sometimes with a fractional part after the
/// seconds, and sometimes joined by a space rather than a dash. All three are
/// accepted; anything else returns nothing rather than a plausible wrong
/// instant, because a clock comparison is exactly where a silently wrong number
/// does the most harm.
///
/// A fractional second is more precision than seconds can carry, so it is
/// dropped here. [`ib_datetime_to_unix_millis`] keeps it.
pub fn ib_datetime_to_unix(stamped: &str) -> Option<i64> {
    ib_datetime_to_unix_millis(stamped).map(|ms| ms.div_euclid(1_000))
}

/// Read the venue's timestamp back to unix milliseconds (UTC).
///
/// The same stamp as [`ib_datetime_to_unix`] reads, keeping the fractional
/// second where the venue states one. Where it does not, the answer lands on a
/// whole second, which is the precision the venue gave and not a rounding of
/// something finer.
///
/// A fraction is read as a decimal: `.5` is five hundred milliseconds, not
/// five. More than three digits are the venue stating more precision than
/// milliseconds hold, and the extra is dropped rather than rounded — rounding
/// up would put the answer in a millisecond the venue did not state.
pub fn ib_datetime_to_unix_millis(stamped: &str) -> Option<i64> {
    let (date, time) = stamped.split_once(['-', ' '])?;
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i64 = date[0..4].parse().ok()?;
    let month: u32 = date[4..6].parse().ok()?;
    let day: u32 = date[6..8].parse().ok()?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        // A day past the end of its month is not a date. Admitted on a bare
        // 1..=31, the thirty-first of February reads as the second or third of
        // March: a plausible instant, days from the one stated, which is what
        // a clock comparison must not be given.
        return None;
    }

    let (time, fraction) = match time.split_once(['.', ',']) {
        Some((whole, rest)) => (whole, rest),
        None => (time, ""),
    };
    let millis: i64 = if fraction.is_empty() {
        0
    } else {
        if !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // Read as a decimal and cut at milliseconds: three digits, padded
        // where the venue stated fewer.
        let mut digits = fraction.as_bytes().to_vec();
        digits.resize(3, b'0');
        std::str::from_utf8(&digits[..3]).ok()?.parse().ok()?
    };
    let mut parts = time.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    let seconds: i64 = parts.next().unwrap_or("0").parse().ok()?;
    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) || !(0..=60).contains(&seconds) {
        return None;
    }

    let days = ymd_to_days(year, month, day)?;
    Some((days * 86_400 + hours * 3_600 + minutes * 60 + seconds) * 1_000 + millis)
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
/// form required for a time-precise good-till expiry (tag 126).
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
/// A named timezone is converted to UTC with DST applied. A time with no
/// timezone is interpreted as UTC and logged. Implied timezones are deprecated
/// in the API, so callers should pass an explicit zone or UTC.
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
        // Matches the captured value.
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


    /// A fraction the venue states is kept when the answer is in milliseconds.
    ///
    /// Reading the stamp in seconds throws the fraction away, which is the
    /// whole reason a caller asks in milliseconds. It is a decimal, so `.5` is
    /// five hundred milliseconds and not five, and a stamp with no fraction
    /// lands on a whole second rather than being rounded to something finer
    /// than the venue stated.
    #[test]
    fn a_stated_fraction_survives_into_milliseconds() {
        let whole = ib_datetime_to_unix_millis("20260815-12:00:00").expect("a stamp");
        assert_eq!(whole, 1_786_795_200_000);
        assert_eq!(ib_datetime_to_unix_millis("20260815-12:00:00.250"), Some(whole + 250));
        // A decimal, not a count of milliseconds: one digit is tenths.
        assert_eq!(ib_datetime_to_unix_millis("20260815-12:00:00.5"), Some(whole + 500));
        assert_eq!(ib_datetime_to_unix_millis("20260815-12:00:00,25"), Some(whole + 250));
        // More precision than milliseconds hold is cut, not rounded up into a
        // millisecond the venue did not state.
        assert_eq!(ib_datetime_to_unix_millis("20260815-12:00:00.2509"), Some(whole + 250));
        // And the seconds reading still drops it.
        assert_eq!(ib_datetime_to_unix("20260815-12:00:00.999"), Some(1_786_795_200));
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

    /// A day past the end of its month is not a date. Admitted on a bare
    /// 1..=31, the thirty-first of February reads as the second or third of
    /// March: a plausible instant, days from the one stated, which is what a
    /// clock comparison must not be given.
    #[test]
    fn a_day_its_month_does_not_have_is_refused() {
        for bad in [
            "20260231-00:00:00", "20260230-00:00:00", "20260431-00:00:00",
            "20260631-00:00:00", "20260931-00:00:00", "20261131-00:00:00",
            "20260229-00:00:00", "21000229-00:00:00", "20260100-00:00:00",
        ] {
            assert_eq!(ib_datetime_to_unix(bad), None, "{bad}");
        }
        // And the days those months do have still read.
        for good in [
            "20260228-00:00:00", "20240229-00:00:00", "20000229-00:00:00",
            "20260131-00:00:00", "20260430-00:00:00", "20261231-23:59:59",
        ] {
            assert!(ib_datetime_to_unix(good).is_some(), "{good}");
        }
    }
}
