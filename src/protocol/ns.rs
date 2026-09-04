//! NS protocol: `#%#%` magic + 4-byte BE length + semicolon-delimited ASCII payload.
//!
//! Used for initial connection handshake before upgrading to FIX messaging.

use std::io::{self, Read};
use std::time::Instant;

/// NS framing magic bytes.
pub const NS_MAGIC: &[u8; 4] = b"#%#%";

// NsMessageType enum values
/// Error response, as the venue numbers it.
pub const NS_ERROR_RESPONSE: u32 = 519;
/// Auth start, as the venue numbers it.
pub const NS_AUTH_START: u32 = 520;
/// Connect request, as the venue numbers it.
pub const NS_CONNECT_REQUEST: u32 = 521;
/// Connect response, as the venue numbers it.
pub const NS_CONNECT_RESPONSE: u32 = 523;
/// Redirect, as the venue numbers it.
pub const NS_REDIRECT: u32 = 524;
/// Fix start, as the venue numbers it.
pub const NS_FIX_START: u32 = 525;
/// Newcommporttype, as the venue numbers it.
pub const NS_NEWCOMMPORTTYPE: u32 = 526;
/// Backup host, as the venue numbers it.
pub const NS_BACKUP_HOST: u32 = 527;
/// Misc urls request, as the venue numbers it.
pub const NS_MISC_URLS_REQUEST: u32 = 528;
/// Misc urls response, as the venue numbers it.
pub const NS_MISC_URLS_RESPONSE: u32 = 529;
/// Secure connect, as the venue numbers it.
pub const NS_SECURE_CONNECT: u32 = 532;
/// Secure connection start, as the venue numbers it.
pub const NS_SECURE_CONNECTION_START: u32 = 533;
/// Secure message, as the venue numbers it.
pub const NS_SECURE_MESSAGE: u32 = 534;
/// Secure error, as the venue numbers it.
pub const NS_SECURE_ERROR: u32 = 535;
/// Server keepalive probe sent during second-factor approval wait.
/// Payload: `MISC{ns_version};530;{server_timestamp};` — the client must echo
/// `server_timestamp` verbatim in an `NS_HEART_BEAT` reply.
pub const NS_TEST_REQUEST: u32 = 530;
/// Client keepalive reply matching a prior `NS_TEST_REQUEST`.
/// Payload: `MISC{ns_version};531;{server_timestamp};`.
pub const NS_HEART_BEAT: u32 = 531;

/// An error frame the venue sent during authentication, as an error a caller
/// can act on.
///
/// A frame of type [`NS_ERROR_RESPONSE`] or [`NS_SECURE_ERROR`] is the venue
/// answering: it read what was sent and refused it. Raised as
/// [`io::ErrorKind::Other`] that refusal reads as a door that did not open, and
/// the failover knocks on every other door with the same credentials — four
/// refused logons where the account asked for one, which is how an account is
/// locked rather than connected. Every door answers for the same account, so
/// there is no second door to ask.
///
/// What is retryable inside such a refusal — the account being busy, a session
/// limit, somebody else holding it — the venue states in its own words, and
/// those are read before this kind is looked at.
pub fn refused_by_the_venue(what: &str, stated: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, format!("{what}: {stated}"))
}

/// Build an NS message with `#%#%` framing.
///
/// Format: `#%#%` + 4-byte-BE-length + payload
/// Payload: `[prefix]{version};{msg_type};{field1};{field2};...;`
pub fn ns_build(version: u32, msg_type: u32, fields: &[&str], prefix: &str) -> Vec<u8> {
    let mut payload = String::new();
    payload.push_str(prefix);
    payload.push_str(&format!("{version};{msg_type};"));
    for f in fields {
        payload.push_str(f);
        payload.push(';');
    }
    let payload_bytes = payload.as_bytes();
    let mut msg = Vec::with_capacity(8 + payload_bytes.len());
    msg.extend_from_slice(NS_MAGIC);
    msg.extend_from_slice(&(payload_bytes.len() as u32).to_be_bytes());
    msg.extend_from_slice(payload_bytes);
    msg
}

/// Parse NS payload into (version, msg_type, remaining_fields).
pub fn ns_parse(payload: &[u8]) -> Option<(u32, u32, Vec<String>)> {
    let text = std::str::from_utf8(payload).ok()?;
    // Strip MISC prefix if present.
    //
    // Compared in place rather than through `to_uppercase`, because
    // uppercasing is not length-preserving: U+0131 and U+017F both uppercase
    // to a single ASCII byte, so a payload beginning "m\u{131}\u{17F}c" passed the
    // check while byte 4 sat inside a character — and slicing a `&str` there
    // is a panic (same class as).
    //
    // The framed receive path does not reach it: `is_ns_text` admits only an
    // ASCII digit or a literal "MISC" before dispatching here. This is the
    // parser's own guard, for callers that hold it directly.
    let text = text
        .get(..4)
        .filter(|p| p.eq_ignore_ascii_case("MISC"))
        .and_then(|_| text.get(4..))
        .unwrap_or(text);
    let parts: Vec<&str> = text.split(';').collect();
    if parts.len() < 2 {
        return None;
    }
    let version: u32 = parts[0].parse().ok()?;
    let msg_type: u32 = parts[1].parse().ok()?;
    // Every field is a position, so an empty one is a field the venue left
    // blank and not a field that is not there. Dropping the empties closed the
    // gaps and moved everything after them one place up, and `ns_build` and
    // this stopped being inverses of each other.
    //
    // The last `;` is a terminator rather than a separator, so the empty string
    // after it is the only one that is not a field.
    let mut parts = &parts[2..];
    if parts.last() == Some(&"") {
        parts = &parts[..parts.len() - 1];
    }
    Some((version, msg_type, parts.iter().map(|s| s.to_string()).collect()))
}

/// Build an `NS_HEART_BEAT` reply that echoes the timestamp from a paired
/// `NS_TEST_REQUEST`. Uses the `MISC` prefix variant.
pub fn ns_build_heart_beat(ns_version: u32, test_req_timestamp: &str) -> Vec<u8> {
    ns_build(ns_version, NS_HEART_BEAT, &[test_req_timestamp], "MISC")
}

/// Extract the timestamp from an inbound `NS_TEST_REQUEST` payload. Returns
/// `None` if the payload doesn't parse or carries the wrong message type.
pub fn parse_test_request_timestamp(payload: &[u8]) -> Option<String> {
    let (_, msg_type, mut fields) = ns_parse(payload)?;
    if msg_type != NS_TEST_REQUEST { return None; }
    // The first field, because that is where the timestamp sits. Taking the
    // first non-empty one instead echoed whatever field followed a blank
    // timestamp, and the probe wants the value from that position or nothing.
    let first = fields.drain(..).next()?;
    (!first.is_empty()).then_some(first)
}

/// Ceiling on a frame on this wire, where the traffic is handshakes,
/// verdicts and keepalives — none of which reach a fraction of this.
pub const MAX_NS_FRAME: usize = 64 * 1024;

/// Fill `buf` one read at a time, bounded by the clock.
///
/// `read_exact` retries a transient `Interrupted` but nothing else, and has
/// no notion of the deadline the caller is reading under, so a peer that
/// states a length and then keeps talking slowly holds it for as long as the
/// bytes keep trickling.
fn read_bounded<R: Read>(reader: &mut R, buf: &mut [u8], deadline: Instant) -> io::Result<()> {
    let mut got = 0;
    while got < buf.len() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the frame did not finish before the deadline",
            ));
        }
        match reader.read(&mut buf[got..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the connection ended before the frame did",
                ))
            }
            Ok(n) => got += n,
            // `read_exact` retries this for every other reader in this file;
            // the raw read must too rather than die on a signal.
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Receive one `#%#%` framed message. Returns (payload_bytes, total_len).
///
/// A frame is at most [`MAX_NS_FRAME`] bytes and has to be done by
/// `deadline`. A peer that states a length and then talks slowly for ever
/// would otherwise hold the reader for as long as it kept saying something —
/// and the deadlines the callers keep run between frames, so one that never
/// ends outruns every one of them.
pub fn ns_recv<R: Read>(reader: &mut R, deadline: Instant) -> io::Result<(Vec<u8>, usize)> {
    let mut header = [0u8; 8];
    read_bounded(reader, &mut header, deadline)?;
    if &header[..4] != NS_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Expected #%#% magic, got {:?}", &header[..4]),
        ));
    }
    let payload_len = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
    // Stated lengths are instructions, and this is the wire's ceiling on one.
    // Reserving a length nobody backs with bytes made a four-byte field an
    // instruction to allocate four gigabytes; reading toward one with no
    // bound made it an instruction to read for ever.
    if payload_len > MAX_NS_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("NS frame claims {payload_len} bytes; the wire's ceiling is {MAX_NS_FRAME}"),
        ));
    }
    let mut payload = Vec::with_capacity(payload_len);
    let mut buf = [0u8; 8192];
    while payload.len() < payload_len {
        let want = (payload_len - payload.len()).min(buf.len());
        read_bounded(reader, &mut buf[..want], deadline)?;
        payload.extend_from_slice(&buf[..want]);
    }
    Ok((payload, payload_len + 8))
}

/// Classify a `#%#%` framed payload as NS text or XYZ binary.
///
/// Returns `true` if the payload looks like NS text — either an ASCII digit
/// (no prefix) or the `MISC` prefix used for some message types
/// (e.g. `NS_MISC_URLS_RESPONSE`, `NS_TEST_REQUEST`, `NS_HEART_BEAT`).
pub fn is_ns_text(payload: &[u8]) -> bool {
    match payload.first() {
        Some(b) if b.is_ascii_digit() => true,
        _ => payload.starts_with(b"MISC"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // ── Existing tests ──────────────────────────────────────────────

    #[test]
    fn build_structure() {
        let msg = ns_build(50, 521, &["user", "1234"], "");
        assert_eq!(&msg[..4], NS_MAGIC);
        let len = u32::from_be_bytes([msg[4], msg[5], msg[6], msg[7]]) as usize;
        assert_eq!(msg.len(), 8 + len);
        let payload = std::str::from_utf8(&msg[8..]).unwrap();
        assert!(payload.starts_with("50;521;"));
    }

    #[test]
    fn build_with_prefix() {
        let msg = ns_build(38, 528, &[], "MISC");
        let payload = std::str::from_utf8(&msg[8..]).unwrap();
        assert!(payload.starts_with("MISC38;528;"));
    }

    #[test]
    fn parse_roundtrip() {
        let msg = ns_build(50, 521, &["user", "1234", "info"], "");
        let payload = &msg[8..];
        let (version, msg_type, fields) = ns_parse(payload).unwrap();
        assert_eq!(version, 50);
        assert_eq!(msg_type, 521);
        assert_eq!(fields, vec!["user", "1234", "info"]);
    }

    /// A field the venue left blank still occupies its place. Dropped, every
    /// field behind it moves up one and the payload is read as a different
    /// message than the one that arrived.
    #[test]
    fn a_blank_field_keeps_its_place() {
        let msg = ns_build(50, 521, &["user", "", "info"], "");
        let (_, _, fields) = ns_parse(&msg[8..]).unwrap();
        assert_eq!(fields, vec!["user", "", "info"]);
    }

    /// And the probe reads the timestamp from its own position rather than
    /// from whichever field happens to carry something.
    #[test]
    fn a_test_request_with_no_timestamp_echoes_nothing() {
        let msg = ns_build(50, NS_TEST_REQUEST, &["", "1750000000"], "MISC");
        assert_eq!(parse_test_request_timestamp(&msg[8..]), None);
    }

    #[test]
    fn parse_misc_prefix() {
        let msg = ns_build(38, 529, &["key=val"], "MISC");
        let payload = &msg[8..];
        let (version, msg_type, fields) = ns_parse(payload).unwrap();
        assert_eq!(version, 38);
        assert_eq!(msg_type, 529);
        assert_eq!(fields, vec!["key=val"]);
    }

    #[test]
    fn heart_beat_echoes_timestamp() {
        let msg = ns_build_heart_beat(50, "20260430-22:58:25");
        let payload = std::str::from_utf8(&msg[8..]).unwrap();
        // Must use MISC prefix, type 531, and carry the original timestamp.
        assert!(payload.starts_with("MISC50;531;20260430-22:58:25;"), "got {payload}");
    }

    #[test]
    fn parse_test_request_extracts_timestamp() {
        let msg = ns_build(50, NS_TEST_REQUEST, &["20260430-22:58:25"], "MISC");
        let payload = &msg[8..];
        assert_eq!(
            parse_test_request_timestamp(payload),
            Some("20260430-22:58:25".to_string())
        );
    }

    #[test]
    fn parse_test_request_rejects_wrong_type() {
        let msg = ns_build(50, NS_HEART_BEAT, &["20260430-22:58:25"], "MISC");
        let payload = &msg[8..];
        assert_eq!(parse_test_request_timestamp(payload), None);
    }

    fn far_enough() -> std::time::Instant {
        std::time::Instant::now() + std::time::Duration::from_secs(60)
    }

    #[test]
    fn recv_roundtrip() {
        let msg = ns_build(50, 534, &["data"], "");
        let mut cursor = std::io::Cursor::new(&msg);
        let (payload, total) = ns_recv(&mut cursor, far_enough()).unwrap();
        assert_eq!(total, msg.len());
        let (version, msg_type, _) = ns_parse(&payload).unwrap();
        assert_eq!(version, 50);
        assert_eq!(msg_type, 534);
    }

    #[test]
    fn is_ns_text_checks() {
        assert!(is_ns_text(b"50;521;user;"));
        assert!(!is_ns_text(b"\x00\x00\x00\x17")); // XYZ binary
        assert!(!is_ns_text(b""));
    }

    // ── New tests ───────────────────────────────────────────────────

    #[test]
    fn build_empty_fields() {
        let msg = ns_build(1, 2, &[], "");
        let payload = std::str::from_utf8(&msg[8..]).unwrap();
        assert_eq!(payload, "1;2;");
    }

    #[test]
    fn build_many_fields() {
        let fields: Vec<&str> = (0..20).map(|_| "x").collect();
        let msg = ns_build(10, 99, &fields, "");
        let payload = std::str::from_utf8(&msg[8..]).unwrap();
        // version;msg_type; plus 20 "x;" entries
        assert!(payload.starts_with("10;99;"));
        assert_eq!(payload.matches("x;").count(), 20);
    }

    #[test]
    fn parse_empty_payload_returns_none() {
        assert!(ns_parse(b"").is_none());
    }

    #[test]
    fn parse_single_field_no_trailing_semicolon_returns_none() {
        // "50" with no semicolons → split yields ["50"], len < 2 → None
        assert!(ns_parse(b"50").is_none());
    }

    #[test]
    fn parse_non_numeric_version_returns_none() {
        assert!(ns_parse(b"abc;521;field;").is_none());
    }

    #[test]
    fn parse_misc_prefix_lowercase() {
        // "misc" in lowercase — to_uppercase converts to "MISC", so it should still
        // strip.
        let payload = b"misc38;529;val;";
        let (version, msg_type, fields) = ns_parse(payload).unwrap();
        assert_eq!(version, 38);
        assert_eq!(msg_type, 529);
        assert_eq!(fields, vec!["val"]);
    }

    #[test]
    fn recv_bad_magic_returns_error() {
        let bad = b"AAAA\x00\x00\x00\x02ok";
        let mut cursor = std::io::Cursor::new(&bad[..]);
        let result = ns_recv(&mut cursor, far_enough());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("#%#%"));
    }

    #[test]
    fn recv_zero_length_payload() {
        let mut msg = Vec::new();
        msg.extend_from_slice(NS_MAGIC);
        msg.extend_from_slice(&0u32.to_be_bytes());
        let mut cursor = std::io::Cursor::new(&msg);
        let (payload, total) = ns_recv(&mut cursor, far_enough()).unwrap();
        assert!(payload.is_empty());
        assert_eq!(total, 8);
    }

    #[test]
    fn is_ns_text_digit() {
        assert!(is_ns_text(b"0rest"));
        assert!(is_ns_text(b"9rest"));
    }

    #[test]
    fn is_ns_text_letter() {
        assert!(!is_ns_text(b"Atext"));
        assert!(!is_ns_text(b"z"));
    }

    #[test]
    fn is_ns_text_null_byte() {
        assert!(!is_ns_text(b"\x00"));
    }

    #[test]
    fn is_ns_text_space() {
        assert!(!is_ns_text(b" "));
    }

    #[test]
    fn ns_magic_value() {
        assert_eq!(NS_MAGIC, b"#%#%");
        assert_eq!(NS_MAGIC.len(), 4);
    }

    #[test]
    fn all_ns_constants_unique() {
        let values: Vec<u32> = vec![
            NS_ERROR_RESPONSE,
            NS_AUTH_START,
            NS_CONNECT_REQUEST,
            NS_CONNECT_RESPONSE,
            NS_REDIRECT,
            NS_FIX_START,
            NS_NEWCOMMPORTTYPE,
            NS_BACKUP_HOST,
            NS_MISC_URLS_REQUEST,
            NS_MISC_URLS_RESPONSE,
            NS_SECURE_CONNECT,
            NS_SECURE_CONNECTION_START,
            NS_SECURE_MESSAGE,
            NS_SECURE_ERROR,
        ];
        let set: HashSet<u32> = values.iter().copied().collect();
        assert_eq!(
            values.len(),
            set.len(),
            "Duplicate values found among NS_* constants"
        );
    }

    /// Uppercasing is not length-preserving: U+0131 and U+017F each occupy two
    /// bytes and uppercase to one ASCII byte. So a payload whose first four
    /// characters uppercase to "MISC" while occupying more than four bytes
    /// passed the prefix check, and the slice that followed landed inside a
    /// character — a panic. Reachable through this function directly; the
    /// framed receive path screens it out earlier with `is_ns_text`.
    #[test]
    fn a_prefix_that_changes_length_when_uppercased_does_not_panic() {
        // 'ı' U+0131 uppercases to 'I'; 'ſ' U+017F uppercases to 'S'.
        let hostile: &[&[u8]] = &[
            "m\u{131}\u{17F}c;1;2".as_bytes(),
            "M\u{131}SC;1;2".as_bytes(),
            "\u{17F}\u{131}sc;1;2".as_bytes(),
            "mis".as_bytes(),
            "MISC".as_bytes(),
            "".as_bytes(),
        ];
        for payload in hostile {
            let _ = ns_parse(payload);
            let _ = parse_test_request_timestamp(payload);
        }
    }

    /// The positive control: a genuine MISC prefix is still stripped, in any
    /// ASCII casing, and a payload without one is still parsed whole.
    #[test]
    fn a_genuine_misc_prefix_is_still_stripped() {
        for prefixed in ["MISC1;2;abc", "misc1;2;abc", "MiSc1;2;abc"] {
            assert_eq!(
                ns_parse(prefixed.as_bytes()),
                Some((1, 2, vec!["abc".to_string()])),
                "{prefixed}",
            );
        }
        assert_eq!(
            ns_parse(b"1;2;abc"),
            Some((1, 2, vec!["abc".to_string()])),
            "no prefix, parsed whole",
        );
    }

    #[test]
    /// What the prefix means, and why the change is confined to the panic.
    ///
    /// The prefix is an ASCII "MISC" in any casing. The old code uppercased
    /// first, so it also recognised spellings that are not ASCII — U+0131 and
    /// U+017F uppercase to "I" and "S" — but it then removed four *bytes*,
    /// and each of those characters occupies two. So a non-ASCII spelling
    /// either put byte 4 inside a character, which panicked, or shifted the cut
    /// so the remainder began mid-prefix and failed to parse. Either way no
    /// such payload ever parsed, which is what makes the outcomes agree.
    ///
    /// The guard for the change itself is the panic test above; this pins the
    /// boundary the prefix rule draws.
    fn the_prefix_is_an_ascii_misc_and_nothing_else() {
        // A non-ASCII spelling is not the prefix. It never parsed before
        // either: the four-byte cut landed one character early and left "C…".
        assert_eq!(ns_parse("M\u{131}SC;1;2".as_bytes()), None);
        assert_eq!(
            ns_parse("M\u{131}SC1;2;body".as_bytes()), None,
            "a payload that is not literally MISC-prefixed is not silently stripped",
        );
        // And the genuine prefix still is.
        assert_eq!(
            ns_parse(b"MISC1;2;body"),
            Some((1, 2, vec!["body".to_string()])),
        );
    }

    #[test]
    fn stripping_leaves_the_defined_shapes_alone() {
        // Exactly the prefix and nothing else: the old code sliced to empty and
        // fell through to the field-count check.
        assert_eq!(ns_parse(b"MISC"), None);
        assert_eq!(ns_parse(b"misc"), None);
        // A prefix followed by too few fields, same outcome.
        assert_eq!(ns_parse(b"MISC1"), None);
        // Three bytes cannot be the prefix and are parsed whole.
        assert_eq!(ns_parse(b"MIS;1;2"), None);
        // A non-ASCII body after a genuine ASCII prefix is still stripped and
        // still parsed — an is_ascii gate would have refused this.
        assert_eq!(
            ns_parse("MISC1;2;caf\u{e9}".as_bytes()),
            Some((1, 2, vec!["caf\u{e9}".to_string()])),
        );
    }

    /// A length nobody backs with bytes costs nothing.
    ///
    /// The header states a size in four bytes. Reserving that up front turns
    /// `0xffffffff` into an instruction to allocate four gigabytes before a
    /// single byte of payload has arrived. Reads are chunked instead.
    #[test]
    fn a_stated_length_is_not_an_allocation() {
        let mut framed = NS_MAGIC.to_vec();
        framed.extend_from_slice(&u32::MAX.to_be_bytes());
        framed.extend_from_slice(b"only these bytes actually follow");

        let started = std::time::Instant::now();
        let result = ns_recv(&mut framed.as_slice(), far_enough());

        assert!(result.is_err(), "the payload the header promised never arrived");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "and finding that out did not mean reserving four gigabytes first",
        );
    }

    /// A peer that states a frame and then keeps talking slowly holds a
    /// reader that believes the stated length for ever: the deadlines the
    /// callers keep run between frames, and a frame that never ends outruns
    /// every one of them. The frame is bounded by the wire's ceiling and by
    /// the caller's clock, and a deadline already passed ends it at once.
    #[test]
    fn a_frame_is_bounded_by_the_ceiling_and_by_the_clock() {
        // The ceiling refuses a frame outright, even one backed with bytes.
        let mut backed = NS_MAGIC.to_vec();
        backed.extend_from_slice(&((MAX_NS_FRAME + 1) as u32).to_be_bytes());
        backed.extend(std::iter::repeat_n(b'x', MAX_NS_FRAME + 1));
        let started = std::time::Instant::now();
        let result = ns_recv(&mut backed.as_slice(), far_enough());
        assert!(
            matches!(&result, Err(e) if e.kind() == std::io::ErrorKind::InvalidData),
            "a frame above the wire's ceiling is refused: {result:?}",
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        // A deadline already passed ends the read before it starts.
        let msg = ns_build(50, 521, &["user"], "");
        let result = ns_recv(&mut msg.as_slice(), std::time::Instant::now());
        assert!(
            matches!(&result, Err(e) if e.kind() == std::io::ErrorKind::TimedOut),
            "a read under a passed deadline is a timeout: {result:?}",
        );
    }

}
