//! The last order id handed out, kept between runs.
//!
//! An order id belongs to the account, not to the process: the venue answers
//! an order under an id it already holds with "Duplicate ID" and places
//! nothing. A client that counts from one on every start collides with
//! everything it placed yesterday.
//!
//! The solution is to remember: keep the last id used, keyed by the client
//! id, hand out that value plus one, and write the new one back. Stored as a
//! small file of `key<TAB>value` lines, one line per account and client id.
//!
//! Starting from one where nothing is remembered will not do:
//! its file is new on a machine whose account may have traded for years
//! through something else, and one is exactly the id that collides. So a first
//! run starts from the clock instead — seconds since the epoch, which is above
//! any count an account has reached and still inside the width the wire
//! carries an id under — and every run after that continues from the file.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What a first run starts from, when nothing has been remembered yet.
///
/// Seconds since the epoch: past any id an account has counted up to, rising
/// on its own, and a third of the way through `u32` rather than a thousand
/// times past it — an id wider than that cannot be carried on the wire at all.
fn from_the_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Which counter a session is counting from.
///
/// Keying by client id alone suffices where one installation serves one
/// account. This one can be pointed at several, so the account and
/// the kind of session are part of the key as well: two accounts sharing a
/// counter would each skip the other's ids for no reason, and worse, a live
/// account would take its numbering from a paper one.
pub fn key(username: &str, paper: bool, client_id: i32) -> String {
    format!("{username}/{}/{client_id}", if paper { "paper" } else { "live" })
}

/// Where the counter lives when the caller names nothing.
pub fn default_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".ibx").join("order-ids")
}

fn read_all(path: &Path) -> BTreeMap<String, u64> {
    let mut kept = BTreeMap::new();
    let Ok(text) = fs::read_to_string(path) else { return kept };
    for line in text.lines() {
        if let Some((name, value)) = line.split_once('\t')
            && let Ok(id) = value.trim().parse::<u64>()
        {
            kept.insert(name.to_string(), id);
        }
    }
    kept
}

/// The last id handed out under this key, or nothing where none was.
pub fn last_used(path: &Path, key: &str) -> Option<u64> {
    read_all(path).get(key).copied()
}

/// The widest id a request can be asked for under.
///
/// An order may be numbered past this — a caller numbering its own orders is
/// theirs to do, and the venue takes them — but a request states its id in
/// four bytes. A session that started counting past this could place orders
/// and ask for nothing.
const WIDEST_A_REQUEST_CARRIES: u64 = u32::MAX as u64;

/// The id to hand out next: one past what was last remembered, or a first one.
///
/// Reading alone. Nothing is remembered until [`remember`] is called with an
/// id that was actually used.
///
/// A remembered id too wide for a request does not become the next session's
/// starting point. One order numbered by a caller in microseconds raised the
/// mark past four bytes and stayed there, and every session afterwards was
/// born unable to ask the venue anything: the ids were refused before they
/// left, and a program driving this client through its own request numbering
/// got that refusal on its first call. The clock is what a first session
/// counts from, and it is what a session counts from again here.
pub fn next_after_last(path: &Path, key: &str) -> u64 {
    match last_used(path, key) {
        Some(last) if last < WIDEST_A_REQUEST_CARRIES => last.saturating_add(1),
        _ => from_the_clock(),
    }
}

/// Remember an id as used, so no later run hands it out again.
///
/// Only ever moves forward: an id lower than what is already remembered is a
/// caller numbering its own orders, which is theirs to do and not something to
/// undo the account's high-water mark over.
pub fn remember(path: &Path, key: &str, id: u64) -> io::Result<()> {
    let mut kept = read_all(path);
    match kept.get(key) {
        // A mark too wide to ask a request under is not a mark to hold to. Left
        // standing it refuses every id that follows, because remembering only
        // moves forward — so nothing counting on from the clock is ever
        // written down, and two sessions starting in the same second are given
        // the same number for their first order.
        Some(last) if *last >= WIDEST_A_REQUEST_CARRIES => kept.insert(key.to_string(), id),
        Some(last) if *last >= id => return Ok(()),
        _ => kept.insert(key.to_string(), id),
    };
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        fs::create_dir_all(dir)?;
    }
    let body: String = kept.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect();
    // Replaced by rename, so a reader never sees half a file, and a run that
    // dies mid-write still finds the previous counter rather than nothing.
    let tmp = path.with_extension("tmp");
    write_private(&tmp, body.as_bytes())?;
    fs::rename(&tmp, path)
}

/// Owner-only from the moment it exists. The counter is not a secret, but it
/// carries the account it belongs to, and it sits beside a file that is one.
#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ibx-order-ids-{name}"));
        let _ = fs::remove_dir_all(&dir);
        dir.join("order-ids")
    }

    /// The whole point: what one run used, the next run does not.
    #[test]
    fn a_later_run_carries_on_from_the_last_id_used() {
        let path = scratch("carries-on");
        let k = key("someone", false, 1);

        let first = next_after_last(&path, &k);
        assert!(first > 1_700_000_000, "a first run starts past any account's count");
        remember(&path, &k, first).unwrap();

        assert_eq!(next_after_last(&path, &k), first + 1);
        remember(&path, &k, first + 5).unwrap();
        assert_eq!(next_after_last(&path, &k), first + 6);
    }

    /// Two accounts, or the same account's two kinds of session, are two
    /// counters. Sharing one would take a live account's numbering from a
    /// paper one.
    #[test]
    fn each_account_and_session_counts_for_itself() {
        let path = scratch("separate");
        remember(&path, &key("someone", false, 1), 500).unwrap();

        assert_eq!(last_used(&path, &key("someone", false, 1)), Some(500));
        assert_eq!(last_used(&path, &key("someone", true, 1)), None, "paper is not live");
        assert_eq!(last_used(&path, &key("другой", false, 1)), None, "nor another account");
        assert_eq!(last_used(&path, &key("someone", false, 2)), None, "nor another client");
    }

    /// A counter only moves forward. A caller numbering its own orders low
    /// must not drag the account's high-water mark back with it.
    #[test]
    fn the_mark_does_not_move_backwards() {
        let path = scratch("forwards");
        let k = key("someone", false, 1);
        remember(&path, &k, 900).unwrap();
        remember(&path, &k, 100).unwrap();
        assert_eq!(last_used(&path, &k), Some(900));
    }

    /// An id too wide for a request does not become the next session's start.
    ///
    /// A caller numbering its own orders in microseconds raised the mark past
    /// four bytes, and it stayed there: every session afterwards counted from
    /// it and had every request it built refused before the request left.
    #[test]
    fn a_mark_too_wide_for_a_request_does_not_start_the_next_session() {
        let path = scratch("too-wide");
        let key = "someone";

        remember(&path, key, 1_787_352_716_770_078).unwrap();
        assert_eq!(
            last_used(&path, key),
            Some(1_787_352_716_770_078),
            "the account did use it, and that is worth remembering",
        );

        let start = next_after_last(&path, key);
        assert!(
            start <= u32::MAX as u64,
            "a session started at {start}, which no request it makes can state",
        );
        assert!(start > 1_700_000_000, "and it is still the clock, not one");
    }

    /// And a session that found one carries on from where it restarted.
    ///
    /// Stepping around the mark was not enough: it stayed, and remembering
    /// only moves forward, so every id counted from the clock was refused as
    /// lower and nothing was written down. Two sessions starting in the same
    /// second were handed the same number for their first order, which is the
    /// one thing this file exists to prevent.
    #[test]
    fn a_session_after_a_too_wide_mark_does_not_repeat_itself() {
        let path = scratch("after-too-wide");
        let key = "someone";
        remember(&path, key, 1_787_352_716_770_078).unwrap();

        // One session starts, takes an id and records it.
        let first = next_after_last(&path, key);
        remember(&path, key, first).unwrap();
        assert_eq!(last_used(&path, key), Some(first), "the id it used went unrecorded");

        // The next starts in the same second and must not be given that id.
        let second = next_after_last(&path, key);
        assert!(
            second > first,
            "two sessions were handed {first}, so both numbered an order the same",
        );
    }

    /// One inside the width is handed on as before.
    #[test]
    fn a_mark_a_request_can_carry_is_counted_on_from() {
        let path = scratch("within");
        remember(&path, "someone", 4_000).unwrap();
        assert_eq!(next_after_last(&path, "someone"), 4_001);
    }

    /// A file that is not there, or not readable, is a first run rather than a
    /// failure: the id still has to be handed out.
    #[test]
    fn nothing_remembered_is_a_first_run() {
        let path = scratch("absent").with_file_name("not-written");
        assert_eq!(last_used(&path, "anyone"), None);
        assert!(next_after_last(&path, "anyone") > 1_700_000_000);
    }
}
