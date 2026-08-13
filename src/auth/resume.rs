//! Carrying a session across a restart.
//!
//! A login is expensive and, on an account with a second factor, needs a human.
//! A process that must be restarted overnight cannot ask for one, which is why
//! the terminal keeps its session on disk and picks it up again — the
//! "autorestart" file, and the reason a gateway without one dies at the nightly
//! maintenance window and does not come back.
//!
//! This is the same idea in the same shape: the session written encrypted,
//! readable only by its owner, and offered on the next connect. It is an
//! optimisation, never a requirement — a session that is missing, unreadable,
//! stale or refused just means logging in again.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::auth::crypto::{aes_cbc_decrypt, aes_cbc_encrypt};

/// Bytes in the random prefix that precedes the record, so two writes of the
/// same session do not produce the same file.
const SALT_LEN: usize = 32;

/// Format marker. A file that does not open with this is not ours.
const MAGIC: &[u8; 8] = b"IBXSESS\x01";

/// What a resumed connect needs. All four are established at login and none can
/// be recomputed without one.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ResumableSession {
    /// The session token, big-endian, as `session_token_bytes` reports it.
    pub token: Vec<u8>,
    /// The venue's own id for the session being resumed.
    pub server_session_id: String,
    /// What this machine identified itself as.
    pub hw_info: String,
    /// What the client string encoded to.
    pub encoded: String,
    /// Account the session belongs to. A file holding someone else's session is
    /// refused rather than offered to the wrong login.
    pub username: String,
    /// Whether the session is a paper one, for the same reason.
    pub paper: bool,
}

impl ResumableSession {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(MAGIC);
        for field in [
            self.token.as_slice(),
            self.server_session_id.as_bytes(),
            self.hw_info.as_bytes(),
            self.encoded.as_bytes(),
            self.username.as_bytes(),
        ] {
            out.extend_from_slice(&(field.len() as u32).to_be_bytes());
            out.extend_from_slice(field);
        }
        out.push(self.paper as u8);
        out
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let rest = bytes.strip_prefix(MAGIC.as_slice())?;
        let mut cur = rest;
        let mut take = || -> Option<Vec<u8>> {
            let (len_bytes, after) = cur.split_at_checked(4)?;
            let len = u32::from_be_bytes(len_bytes.try_into().ok()?) as usize;
            let (field, after) = after.split_at_checked(len)?;
            cur = after;
            Some(field.to_vec())
        };
        let token = take()?;
        let server_session_id = String::from_utf8(take()?).ok()?;
        let hw_info = String::from_utf8(take()?).ok()?;
        let encoded = String::from_utf8(take()?).ok()?;
        let username = String::from_utf8(take()?).ok()?;
        let paper = *cur.first()? != 0;
        Some(Self { token, server_session_id, hw_info, encoded, username, paper })
    }
}

/// Where a session is kept when the caller does not name a path.
///
/// Under the user's state directory, so it is per-user by construction and
/// never lands somewhere world-readable like a temp directory.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("ibx").join("session")
}

/// The key a session is sealed with.
///
/// Derived from the account's own password, so the file is worth nothing to
/// anyone who does not already hold the credential it protects — copying it to
/// another machine, or reading it as another user, gains nothing.
fn seal_key(password: &str, salt: &[u8]) -> [u8; 16] {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(b"ibx-session-v1");
    hasher.update(salt);
    hasher.update(password.as_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

/// Write the session, replacing whatever was there.
///
/// Owner-only from the moment it exists: the file is created with its
/// permissions already set rather than tightened afterwards, so there is no
/// window in which it is readable by anyone else.
pub fn save(path: &Path, password: &str, session: &ResumableSession) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut salt = [0u8; SALT_LEN];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut salt);
    let key = seal_key(password, &salt);
    let iv = &salt[..16];

    let mut body = salt.to_vec();
    body.extend_from_slice(&aes_cbc_encrypt(&key, iv, &session.encode()));

    let tmp = path.with_extension("tmp");
    write_private(&tmp, &body)?;
    // Replacing by rename, so a reader never sees a half-written session.
    fs::rename(&tmp, path)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f =
        fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // No mode to set at creation here; the containing directory is the
    // protection, as it is for every other per-user file on this platform.
    fs::write(path, bytes)
}

/// Read the session for this account, or nothing.
///
/// Every failure is the same answer — there is no session to resume — because
/// there is nothing a caller can do differently about a file that is missing, a
/// file it cannot decrypt, and a file belonging to another account.
pub fn load(path: &Path, username: &str, password: &str, paper: bool) -> Option<ResumableSession> {
    let body = fs::read(path).ok()?;
    let (salt, ciphertext) = body.split_at_checked(SALT_LEN)?;
    let key = seal_key(password, salt);
    let plain = aes_cbc_decrypt(&key, &salt[..16], ciphertext).ok()?;
    let session = ResumableSession::decode(&plain)?;
    if session.username != username || session.paper != paper {
        return None;
    }
    Some(session)
}

/// Forget the stored session. Called when the server refuses it, so a token it
/// has rejected is not offered again on every start.
pub fn clear(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ResumableSession {
        ResumableSession {
            token: vec![0xa1, 0xb2, 0xc3, 0x00, 0xff],
            server_session_id: "6a72be0a.00000005".into(),
            hw_info: "hw".into(),
            encoded: "21.0/L/en_US/dist".into(),
            username: "someone".into(),
            paper: true,
        }
    }

    #[test]
    fn a_session_survives_the_round_trip() {
        let dir = std::env::temp_dir().join(format!("ibx-resume-{}", std::process::id()));
        let path = dir.join("session");
        let s = sample();
        save(&path, "hunter2", &s).unwrap();
        assert_eq!(load(&path, "someone", "hunter2", true), Some(s));
        clear(&path);
        assert_eq!(load(&path, "someone", "hunter2", true), None);
    }

    /// The file is worth nothing without the credential it protects.
    #[test]
    fn a_session_does_not_open_with_the_wrong_password() {
        let dir = std::env::temp_dir().join(format!("ibx-resume-pw-{}", std::process::id()));
        let path = dir.join("session");
        save(&path, "right", &sample()).unwrap();
        assert_eq!(load(&path, "someone", "wrong", true), None);
        clear(&path);
    }

    /// A session belongs to one account on one side. Offering a paper session
    /// to a live login, or one user's to another, would fail the handshake at
    /// best and cross accounts at worst.
    #[test]
    fn a_session_is_refused_for_a_different_account() {
        let dir = std::env::temp_dir().join(format!("ibx-resume-acct-{}", std::process::id()));
        let path = dir.join("session");
        save(&path, "pw", &sample()).unwrap();
        assert_eq!(load(&path, "someone-else", "pw", true), None, "another user");
        assert_eq!(load(&path, "someone", "pw", false), None, "the live side");
        assert!(load(&path, "someone", "pw", true).is_some(), "its own account");
        clear(&path);
    }

    /// Anything unreadable is simply no session, never an error the caller has
    /// to handle: the answer is always to log in again.
    #[test]
    fn a_damaged_file_is_no_session_rather_than_a_failure() {
        let dir = std::env::temp_dir().join(format!("ibx-resume-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session");
        fs::write(&path, b"not a session at all").unwrap();
        assert_eq!(load(&path, "someone", "pw", true), None);
        fs::write(&path, vec![0u8; SALT_LEN + 16]).unwrap();
        assert_eq!(load(&path, "someone", "pw", true), None);
        clear(&path);
    }

    #[cfg(unix)]
    #[test]
    fn a_stored_session_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ibx-resume-perm-{}", std::process::id()));
        let path = dir.join("session");
        save(&path, "pw", &sample()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a credential is not left readable by anyone else");
        clear(&path);
    }
}
