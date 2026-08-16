//! Gateway: orchestrates auth + data connections into a running HotLoop.

mod logon;

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use std::sync::mpsc::{SyncSender, sync_channel};
use native_tls::TlsConnector;
use num_bigint::BigUint;
use sha1::{Digest, Sha1};
use zeroize::Zeroizing;

use std::net::ToSocketAddrs;

use crate::auth::crypto::strip_leading_zeros;
use crate::auth::dh::SecureChannel;
use crate::auth::session::{self, do_srp, do_soft_token};
use crate::config::*;
use std::sync::Arc;
use crate::bridge::{Event, SharedState};
use crate::engine::hot_loop::HotLoop;
use crate::protocol::connection::Connection;
use crate::protocol::fix::{self, fix_build, fix_parse, fix_read_deadline, SOH};
use crate::protocol::fixcomp;
use crate::protocol::ns;
use crate::types::ControlCommand;

/// Split the venue's per-security-type order permissions, logon tag 6652.
///
/// `SECTYPE:ORDTYPE,ORDTYPE;SECTYPE:...`. A security type named with no order
/// types after it is still permitted, so an absent list is an empty one and
/// never a missing key.
fn parse_order_permissions(raw: &str) -> std::collections::HashMap<String, Vec<String>> {
    raw.split(';')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (sec_type, types) = entry.split_once(':').unwrap_or((entry, ""));
            let types = types.split(',').filter(|t| !t.is_empty()).map(str::to_string).collect();
            (sec_type.to_string(), types)
        })
        .collect()
}

/// Parse the `PRIV_LAB_MISC_URLS` blob (FIX tag 6321) into a `{key: value}` map.
///
/// Wire format: pipe-delimited `k=v|k=v|…`, with `%7C` escaping a literal `|`
/// inside keys or values. Falls back to comma as the entry separator when the
/// payload contains no `|`. Empty input yields an empty map; entries without
/// `=` or with an empty key are dropped.
pub fn parse_misc_urls(s: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if s.is_empty() {
        return out;
    }
    let sep = if s.contains('|') { '|' } else { ',' };
    for entry in s.split(sep) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((k, v)) = entry.split_once('=') else {
            continue;
        };
        let key = k.trim().replace("%7C", "|").replace("%7c", "|");
        let val = v.trim().replace("%7C", "|").replace("%7c", "|");
        if key.is_empty() {
            continue;
        }
        out.insert(key, val);
    }
    out
}
/// Farm name used when the auth server states no trading route.
pub(crate) const DEFAULT_TRADING_FARM: &str = "usfarm";

/// Parse a farm-route string from the auth-server's routing tags.
///
/// Three accepted shapes:
///   `"<host>/<farm>"`            — tag 6145 (trading)
///   `"<host>/<farm>/<port>"`     — tags 6171 (mktdata) / 8008 (secdef)
///
/// The port is the venue's, where it states one. A route names the port to
/// reach that farm on, and the counterpart takes it from there when no tag
/// carries it separately — it treats a route with neither as an error rather
/// than substituting a default. `None` here means the route stated no port,
/// which is the only case the configured one applies to.
///
/// Returns `None` for empty or malformed input.
pub fn parse_farm_route(route: &str) -> Option<(String, String, Option<u16>)> {
    if route.is_empty() { return None; }
    let mut parts = route.splitn(3, '/');
    let host = parts.next()?.to_string();
    let farm = parts.next()?.to_string();
    if host.is_empty() || farm.is_empty() { return None; }
    let port = parts.next().and_then(|p| p.trim().parse::<u16>().ok());
    Some((host, farm, port))
}

/// The trading route a reconnect should use.
///
/// The auth server states one per account, and the reconnect uses it rather
/// than announcing a literal farm and connecting to the host the caller
/// configured, which would put a regional account on a farm it is not on
///. Empty means no route was parsed, which is the case the literals
/// were there for; the initial connect falls back the same way.
pub(crate) fn reconnect_trading_route(auth: &ReconnectAuth) -> (String, String) {
    let host = if auth.trading_host.is_empty() {
        auth.host.clone()
    } else {
        auth.trading_host.clone()
    };
    let farm = if auth.trading_farm.is_empty() {
        DEFAULT_TRADING_FARM.to_string()
    } else {
        auth.trading_farm.clone()
    };
    (host, farm)
}

/// Returns true if `buf` contains at least one complete `8=O` (binary) or
/// `8=FIXCOMP` frame. Used to terminate read drains as soon as the expected
/// response is fully buffered.
fn has_complete_response_frame(buf: &[u8]) -> bool {
    if buf.starts_with(b"8=O\x01") {
        if let Some(tag9_off) = buf[4..].windows(2).position(|w| w == b"9=") {
            let tag9_pos = 4 + tag9_off;
            if let Some(soh_off) = buf[tag9_pos..].iter().position(|&b| b == b'\x01') {
                let soh_pos = tag9_pos + soh_off;
                if let Ok(s) = std::str::from_utf8(&buf[tag9_pos + 2..soh_pos])
                    && let Ok(body_len) = s.parse::<usize>() {
                        return soh_pos + 1 + body_len <= buf.len();
                    }
            }
        }
        return false;
    }
    let mut cursor = 0usize;
    while cursor + 12 <= buf.len() {
        if buf[cursor..].starts_with(b"8=FIXCOMP\x01") {
            if let Some(total_len) = fixcomp::fixcomp_length(&buf[cursor..]) {
                return cursor + total_len <= buf.len();
            }
            return false;
        }
        cursor += 1;
    }
    false
}

/// Compute token short hash for farm logon (FIX tag 8483).
///
/// gateway always emits this as **8 hex chars padded with
/// leading zeros**. `format!("{:x}", n)` is wrong when `hash_int`'s high
/// nibble is zero — server silently rejects the FIX 35=A logon in that case.
pub fn token_short_hash(session_token: &BigUint) -> String {
    let token_bytes = session_token.to_bytes_be();
    let stripped = strip_leading_zeros(&token_bytes);
    let digest = Sha1::digest(stripped);
    // Take last 4 bytes as u32 (Java BigInteger.intValue() truncates to low 32 bits)
    let hash_int = u32::from_be_bytes([digest[16], digest[17], digest[18], digest[19]]);
    format!("{hash_int:08x}")
}

/// Build auth server logon message.
///
/// Tag 6266 (`encoded`) carries `{jdkVer}/{platform}/{locale}/{dist}`.
/// The auth server requires the `{locale}` segment to be a canonical Java
/// `Locale.toString()` value — `en_US`, `fr`, `ja_JP`, etc. Bare `en` is
/// rejected as `invalid twsInfo`. Override via `IBX_LOCALE` (locale only)
/// or `IBX_ENCODED` (full string).
///
/// Tag 8361 = `"(rolling)"` is load-bearing: it marks the client as a
/// rolling-release build, which bypasses the server's IB_BUILD allow-list
/// check. Without it the server rejects with "The TWS build you are
/// currently running is no longer supported." Per the
/// The reference client also keeps 6397/6947/8098, so they stay.
///
/// Tag 6947 carries the JVM default timezone (e.g. `Europe/Paris`,
/// `America/New_York`). The auth server doesn't validate it — `UTC` is
/// the safe default — but `IBX_TZ` overrides for users who want to mirror
/// their locale or comply with regional logging requirements. The session
/// states its own, settled when it opened.
pub fn build_ccp_logon(
    settings: &crate::api::settings::SessionSettings,
    hw_info: &str, encoded: &str, heartbeat: u64, seq: u32,
) -> Vec<u8> {
    let now = chrono_free_timestamp();
    let tz = settings.timezone.as_str();
    let hb_str = heartbeat.to_string();
    let hw_field = format!("<{}|{}>", hw_info, session::get_lan_ip());
    let build = settings.build.clone();
    let version = settings.version.clone();
    fix_build(
        &[
            (fix::TAG_MSG_TYPE, fix::MSG_LOGON),
            (fix::TAG_SENDING_TIME, &now),
            (fix::TAG_ENCRYPT_METHOD, "0"),
            (fix::TAG_HEARTBEAT_INT, &hb_str),
            (fix::TAG_RESET_SEQ_NUM, "Y"),
            (fix::TAG_IB_BUILD, &build),
            (fix::TAG_IB_VERSION, &version),
            (6490, "dark"),
            (6266, encoded),
            (6351, &hw_field),
            (6397, "1"),
            (6947, tz),
            (8361, "(rolling)"),
            (8098, "0"),
            // Declared at logon, and the server will not answer a request on a
            // user message without it: asking the same question three times
            // drew three replies with it and none without, while an idle
            // stretch of the same length drew the opposite.
            (6900, "1"),
        ],
        seq,
    )
}

/// Build encrypted farm logon message.
pub fn build_farm_encrypted_logon(
    settings: &crate::api::settings::SessionSettings,
    channel: &mut SecureChannel,
    username: &str,
    _paper: bool,
    farm_name: &str,
    session_id: &str,
    session_token: &BigUint,
    hw_info: &str,
    encoded: &str,
    slot: u32,
) -> Vec<u8> {
    let display_name = format!("S{username}");
    let farm_id = format!("{display_name}/{slot}/{farm_name}");
    let farm_id_len = farm_id.len().to_string();
    let token_hash = token_short_hash(session_token);
    let ns_range = format!("{NS_VERSION_MIN}..{NS_VERSION}");
    let now = chrono_free_timestamp();
    let hb_str = FARM_HEARTBEAT.to_string();
    let hw_field = format!("<{}|{}>", hw_info, session::get_lan_ip());
    let build = settings.build.clone();
    let version = settings.version.clone();

    let inner = fix_build(
        &[
            (fix::TAG_MSG_TYPE, fix::MSG_LOGON),
            (fix::TAG_SENDING_TIME, &now),
            (fix::TAG_ENCRYPT_METHOD, "0"),
            (fix::TAG_HEARTBEAT_INT, &hb_str),
            (95, &farm_id_len),
            (96, &farm_id),
            (fix::TAG_IB_BUILD, &build),
            (fix::TAG_IB_VERSION, &version),
            (6351, &hw_field),
            (6266, encoded),
            (6903, "1"),
            (8035, session_id),
            (8285, &ns_range),
            (8483, &token_hash),
        ],
        0,
    );

    log::info!(
        "{} FIX 35=A pre-encrypt ({} bytes): {}",
        farm_name,
        inner.len(),
        String::from_utf8_lossy(&inner).replace('\x01', "|"),
    );
    let encrypted_raw = channel.encrypt(&inner);
    let b64_str = B64.encode(&encrypted_raw);

    // Outer wrapper: 8=FIX.4.1|9=<bodylen>|90=<b64_len>|91=<b64>|10=<cksum>
    let b64_len_str = b64_str.len().to_string();
    let body = format!("90={b64_len_str}\x0191={b64_str}\x01");
    let header = format!("8=FIX.4.1\x019={:04}\x01", body.len());
    let pre_cksum = format!("{header}{body}");
    let cksum = fix::fix_checksum(pre_cksum.as_bytes());
    let mut wrapper = pre_cksum.into_bytes();
    wrapper.extend_from_slice(format!("10={cksum}\x01").as_bytes());
    wrapper
}

/// What a farm's logon leaves behind for the connection that follows it.
pub struct FarmLogon {
    /// Where verification of inbound frames resumes.
    pub read_iv: Vec<u8>,
    /// Where signing of outbound frames resumes.
    pub sign_iv: Vec<u8>,
    /// Anything read past the logon answer, to be handed to the connection
    /// rather than dropped.
    pub remaining: Vec<u8>,
    /// How often this farm said it expects to hear from the session, where it
    /// said anything. `None` leaves the interval this client proposed.
    pub heartbeat_secs: Option<u64>,
}

/// Execute farm logon exchange.
///
/// Returns (read_iv, sign_iv, remaining_buf) for message signing/verification.
pub fn farm_logon_exchange(
    stream: &mut TcpStream,
    channel: &mut SecureChannel,
    session_token: &BigUint,
    username: &str,
    password: &str,
    read_mac_key: &[u8],
    initial_read_iv: &[u8],
) -> io::Result<FarmLogon> {
    // Poll on a short read timeout and tolerate transient WouldBlock/TimedOut
    // returns until an overall deadline. A single slow response segment from a
    // high-latency regional gateway must not tear down the connection.
    stream.set_read_timeout(Some(Duration::from_millis(FARM_LOGON_POLL_MS)))?;
    let deadline = std::time::Instant::now() + Duration::from_secs_f64(TIMEOUT_FARM_LOGON);
    let mut buf = Vec::new();
    let mut read_iv = initial_read_iv.to_vec();
    // What the farm says it expects, where it says anything at all.
    let mut heartbeat_secs: Option<u64> = None;

    for _msg_num in 0..20 {
        // Read until a frame is complete
        let msg = loop {
            if let Some((msg, consumed)) = try_frame_farm_msg(&buf) {
                buf.drain(..consumed);
                break msg;
            }
            let mut tmp = [0u8; FARM_RECV_BUF];
            let n = match stream.read(&mut tmp) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
                {
                    if std::time::Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "farm logon timed out waiting for server response",
                        ));
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "farm connection closed during logon",
                ));
            }
            buf.extend_from_slice(&tmp[..n]);
        };

        // FIX.4.1 message
        if msg.starts_with(b"8=FIX.4.1\x01") {
            let has_sig = msg.windows(6).any(|w| w == b"\x018349=");
            // Check for HMAC signature → unsign
            let parsed_msg = if has_sig {
                let (unsigned, new_iv, valid) = fix::fix_unsign(&msg, read_mac_key, &read_iv);
                if !valid {
                    // Same rule as `Connection::unsign`: a frame that does not
                    // verify is neither parsed nor allowed to move the chain,
                    // since the IV it would move to is derived from a body the
                    // MAC just declined to vouch for.
                    log::warn!("auth frame failed signature verification — dropped");
                    continue;
                }
                read_iv = new_iv;
                unsigned
            } else {
                msg.clone()
            };
            let fields = fix_parse(&parsed_msg);
            // Check for encrypted content (tags 91/96)
            let enc_tag = fields.get(&91).or_else(|| fields.get(&96));
            if let Some(b64_data) = enc_tag {
                let encrypted = B64.decode(b64_data).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
                })?;
                let decrypted = channel.decrypt(&encrypted).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, e)
                })?;

                // Sync HMAC read IV with AES read IV after decryption (CBC chaining)
                if let Some(iv) = channel.read_iv() {
                    read_iv = iv.to_vec();
                }

                // Check for auth challenge → respond with token, fall back to SRP if rejected.
                // Outcome asymmetry:
                //   PASSED  — token accepted, continue
                //   UNKNOWN — server cache miss, recover via SRP on this socket
                //   FAILED  — `do_soft_token` returns Err; the OUTER reconnect loop
                //             must drop this socket and retry from scratch with a
                //             fresh soft-token (NOT SRP — captured behavior).
                if decrypted.windows(5).any(|w| w == b"35=S\x01") {
                    // Pass the farm read buffer as the auth carry buffer: the
                    // auth exchange reads on the same socket, and a high-latency
                    // gateway can coalesce its final response with the farm logon
                    // ACK. Threading `buf` through keeps those trailing ACK bytes
                    // so the loop below re-frames them instead of stalling on a
                    // read for bytes already consumed.
                    match do_soft_token(stream, session_token, &mut buf)? {
                        session::SoftTokenOutcome::Passed => {}
                        session::SoftTokenOutcome::Unknown => {
                            log::warn!("Soft token rejected — falling back to SRP farm auth");
                            stream.set_read_timeout(Some(Duration::from_millis(FARM_LOGON_POLL_MS)))?;
                            session::do_srp_farm(stream, username, password, &mut buf)?;
                        }
                    }
                }
            } else if fields.get(&35).map(|s| s.as_str()) == Some("A") {
                // Logon ACK — sign_iv is the current write_iv (mutated by encrypt)
                let sign_iv = channel
                    .write_iv()
                    .map(|iv| iv.to_vec())
                    .unwrap_or_default();
                // How often this farm expects to hear from the session, if it
                // says. The logon proposes a number and the answer may name a
                // different one; read from the answer, because the proposal is
                // not what the session is held to. The auth connection had the
                // same shape and was being answered too slowly for it.
                if let Some(stated) = fields.get(&108).and_then(|v| v.parse::<u64>().ok())
                    && stated > 0
                {
                    log::info!("a farm states a heartbeat interval of {stated}s");
                    heartbeat_secs = Some(stated);
                }
                if !buf.is_empty() {
                    log::warn!("{} bytes remaining in buffer after logon ACK",
                        buf.len());
                }
                return Ok(FarmLogon { read_iv, sign_iv, remaining: buf, heartbeat_secs });
            } else if fields.get(&35).map(|s| s.as_str()) == Some("3") {
                let text = fields.get(&58).map(|s| s.as_str()).unwrap_or("unknown");
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("Farm logon rejected: {text}"),
                ));
            } else {
                // Anything else the server says during logon.
                //
                // Dropped silently, this reads as the server having said
                // nothing: the loop waits for a message that has already been
                // sent and thrown away, the server waits for the answer, and
                // whoever gives up first ends it — which is the server, with a
                // close and no reason, about ten seconds in. A logon that fails
                // that way is indistinguishable from one the server never
                // answered, so say what arrived.
                log::warn!(
                    "farm logon: unhandled message type {:?} — nothing here answers it",
                    fields.get(&35).map(|s| s.as_str()).unwrap_or("(none)"),
                );
            }
        } else if msg.starts_with(b"8=1\x01") {
            // Token auth response
            if msg.windows(6).any(|w| w == b"PASSED") {
                log::info!("Token auth PASSED");
            }
        } else {
            log::warn!(
                "farm logon: unhandled frame, {} bytes, starting {:?}",
                msg.len(),
                String::from_utf8_lossy(&msg[..msg.len().min(16)]),
            );
        }
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "exceeded max messages without farm logon ACK",
    ))
}

/// Try to extract one complete FIX message from a buffer.
/// Returns (message, bytes_consumed) or None if incomplete.
fn try_frame_farm_msg(buf: &[u8]) -> Option<(Vec<u8>, usize)> {
    if buf.len() < 10 {
        return None;
    }
    // Look for FIX header
    if !buf.starts_with(b"8=") {
        // Skip garbage
        let next = buf.windows(2).position(|w| w == b"8=")?;
        return Some((Vec::new(), next)); // skip garbage, caller retries
    }
    // Find tag 9 body length
    let tag9_pos = buf.windows(3).position(|w| w == b"\x019=")?;
    let val_start = tag9_pos + 3;
    let soh_pos = buf[val_start..].iter().position(|&b| b == SOH)? + val_start;
    let body_len: usize = std::str::from_utf8(&buf[val_start..soh_pos]).ok()?.parse().ok()?;
    let total = soh_pos + 1 + body_len + 7; // +7 for "10=XXX\x01"
    if buf.len() < total {
        return None;
    }
    Some((buf[..total].to_vec(), total))
}

/// The connect-time credentials a reconnect needs, supplied by whichever
/// binding built the session.
///
/// A constructor parameter rather than a setter called afterwards: leaving
/// these blank for the caller to fill meant one binding filled them and the
/// other did not, and every reconnect scheduler silently refused to run on the
/// empty host. The compiler asks for them now.
pub struct CallerAuth {
    /// What this session runs under.
    pub settings: std::sync::Arc<crate::api::settings::SessionSettings>,
    /// Which host to open the session against.
    pub host: String,
    /// The login.
    pub username: String,
    /// Its password, zeroed when dropped.
    pub password: Zeroizing<String>,
    /// Whether this is a paper session. It decides one step of the logon and
    /// nothing after it.
    pub paper: bool,
    /// What the second-factor gate needs if the server bumps a reconnect back
    /// to SRP. Without these an unattended client cannot finish that handshake.
    pub code_provider: Option<session::CodeProvider>,
    /// How long to wait for the second factor before giving up.
    pub ib_key_timeout_secs: u64,
    /// Which kind of second factor to ask for.
    pub ib_key_token_sub_type: String,
}

/// Credentials cached for auto-reconnect (no SRP needed).
#[derive(Clone)]
pub struct ReconnectAuth {
    /// What this session runs under. Carried so a reconnect states what the
    /// session stated when it opened, rather than whatever the process holds
    /// by the time it drops.
    pub settings: std::sync::Arc<crate::api::settings::SessionSettings>,
    /// Which host to open the session against.
    pub host: String,
    /// When the session being rebuilt first logged in, in the venue's spelling.
    ///
    /// A reconnect that finds somebody on the account compares the two: later
    /// than this is another client, which took the account fairly and should
    /// keep it. Earlier is this session's own previous logon, not yet reaped,
    /// and taking that back is just finishing the reconnect.
    ///
    /// Empty means every session the venue names counts as another client,
    /// which is the careful reading: the reconnect gives the account up rather
    /// than taking it from somebody who may hold it fairly.
    pub logged_in_at: String,
    /// The other auth servers this session reached the venue through.
    ///
    /// Tried in order when the first cannot be reached. Every one was named by
    /// the venue on the way in, so none of them is a guess — a client that
    /// invented an address to fail over to would be reaching for a server
    /// nobody said was there.
    pub alternate_hosts: Vec<String>,
    /// The login.
    pub username: String,
    /// Wrapped in `Zeroizing` so the plaintext is wiped from memory on drop.
    pub password: Zeroizing<String>,
    /// Whether this is a paper session. It decides one step of the logon and
    /// nothing after it.
    pub paper: bool,
    /// What the second-factor gate needs if the server bumps a reconnect back
    /// to SRP. Without these an unattended client cannot finish that handshake.
    pub code_provider: Option<session::CodeProvider>,
    /// How long to wait for the second factor before giving up.
    pub ib_key_timeout_secs: u64,
    /// Which kind of second factor to ask for.
    pub ib_key_token_sub_type: String,
    /// The key the session was established with.
    pub session_key: BigUint,
    /// The token it resumes with.
    pub session_token: BigUint,
    /// The venue's own id for it.
    pub server_session_id: String,
    /// What this machine identified itself as.
    pub hw_info: String,
    /// What the client string encoded to.
    pub encoded: String,
    /// Historical-data farm routing parsed from the auth-server response.
    /// Used by HMDS reconnect — empty when no HMDS route was parsed.
    pub hmds_host: String,
    /// Which historical farm the venue routed this session to.
    pub hmds_farm: String,
    /// Trading farm routing exactly as parsed from the same response — empty
    /// when the auth server stated none, rather than carrying the fallback the
    /// initial connect materializes. The distinction matters: an empty host
    /// lets the reconnect use whatever host the session has now, which a
    /// redirect may have changed since.
    pub trading_host: String,
    /// Which trading farm.
    pub trading_farm: String,
    /// Security-definition farm routing from the same response, empty when the
    /// venue stated none. The calendar rides this one, and without the route a
    /// connection that goes cannot be rebuilt.
    pub secdef_host: String,
    /// Which security-definition farm.
    pub secdef_farm: String,
    /// The port the venue named for each farm, where it named one. A reconnect
    /// that fell back to the configured port would dial somewhere the venue
    /// never routed this session to.
    pub trading_port: Option<u16>,
    /// The port stated for the historical farm.
    pub hmds_port: Option<u16>,
    /// The port stated for the security-definition farm.
    pub secdef_port: Option<u16>,
}

/// Keep the first value the venue sends for a field it sends once, and say so
/// when it sends a second one that differs. Each of these carries its whole
/// list in one field, so a second differing value would mean the list arrives
/// in parts and keeping the first silently drops the rest — which looks
/// identical to the venue having sent nothing more.
fn keep_first(slot: &mut String, value: &str, field: &str) {
    if slot.is_empty() {
        *slot = value.to_string();
    } else if slot != value {
        log::warn!(
            "Logon sent {field} a second time with different content ({} then {} bytes); \
             keeping the first. This field is read as arriving whole.",
            slot.len(), value.len(),
        );
    }
}

/// Record an account the venue named, in the order it named them, ignoring
/// repeats. The venue names an account more than once across the logon ACK and
/// the init burst, and a login holding several accounts names each of them.
fn note_account(accounts: &mut Vec<String>, account: &str) {
    if !account.is_empty() && !accounts.iter().any(|a| a == account) {
        accounts.push(account.to_string());
    }
}

/// Another session already logged in on this account when this one connected.
///
/// The venue states this in its answer to the connect, and the counterpart
/// reads it there: a session that arrives second is told who was already here.
/// Whether that matters is the caller's to decide — the venue may serve both,
/// or may hold this one to reading only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompetingSession {
    /// Where the session that was already logged in connected from.
    pub ip: String,
    /// When it logged in, as the venue stated it: `yyyyMMdd-HH:mm:ss`, GMT.
    pub since: String,
    /// This session may look but not trade, because the other one holds the
    /// account.
    ///
    /// Worth knowing before an order is sent rather than after it is refused.
    pub read_only: bool,
}

/// The suffix the venue adds when the session being told is the one held to
/// reading only.
const COMPETING_READ_ONLY: &str = "(RO)";

/// Whether a failed connect means nothing was reached, rather than something
/// answering and saying no.
///
/// The difference decides whether another door is worth knocking on: a door
/// that cannot be reached says nothing about the account, and one that refuses
/// says everything, at every door.
fn nobody_answered(e: &io::Error) -> bool {
    use crate::engine::hot_loop::retry::DisconnectReason;
    matches!(
        DisconnectReason::from_error(e),
        DisconnectReason::Transport | DisconnectReason::NoResponse,
    )
}

/// A host the venue named, and the port it named it on.
///
/// The venue states `host:port` in a redirect. Falls back to the port this
/// session is already on when it states only a host, which is the common
/// case.
fn host_and_port(target: &str, fallback: u16) -> (&str, u16) {
    match target.split_once(':') {
        Some((host, port)) => (host, port.parse().unwrap_or(fallback)),
        None => (target, fallback),
    }
}

/// How a reconnect says it found somebody else on the account.
///
/// Read back by the failover, which is why it is one spelling rather than two,
/// and classified as a takeover by the retry ladder, which is why it says
/// "competing".
const TOOK_THE_ACCOUNT: &str = "competing logon:";

/// Whether a session the venue names belongs to another client rather than to
/// this one.
///
/// Both stamps are GMT `YYYYMMDD-HH:MM:SS`, so comparing them as text compares
/// them as moments. Equal counts as this session's own: a reconnect that lands
/// in the same second as the logon being reaped is the common case, and giving
/// the account up over it would strand a caller who has no competitor at all.
fn is_another_client(other_since: &str, our_logon: &str) -> bool {
    other_since > our_logon
}

/// Read a competing session out of the venue's answer to the connect.
///
/// The answer carries one field for this, and it is either `0` for nobody
/// else, or an address and a login time separated by a slash. Found by shape
/// rather than by position: the field's own form — an address, a slash, a
/// timestamp the venue writes as `yyyyMMdd-HH:mm:ss` — identifies it, and a
/// frame that gains a field does not move it.
pub fn parse_competing_session(frame: &str) -> Option<CompetingSession> {
    frame.split(';').find_map(|field| {
        let field = field.trim();
        if field.is_empty() || field == "0" {
            return None;
        }
        let (ip, rest) = field.split_once('/')?;
        if ip.is_empty() {
            return None;
        }
        let (since, read_only) = match rest.strip_suffix(COMPETING_READ_ONLY) {
            Some(stripped) => (stripped, true),
            None => (rest, false),
        };
        // yyyyMMdd-HH:mm:ss, and nothing else in the frame looks like it.
        let (day, clock) = since.split_once('-')?;
        if day.len() != 8 || !day.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if clock.len() != 8 || clock.as_bytes()[2] != b':' || clock.as_bytes()[5] != b':' {
            return None;
        }
        Some(CompetingSession {
            ip: ip.to_string(),
            since: since.to_string(),
            read_only,
        })
    })
}

/// Full gateway connection.
pub struct Gateway {
    /// The account this session acts for.
    pub account_id: String,
    /// Every account this login holds, the first being [`Gateway::account_id`].
    /// A login with one account holds one entry.
    pub accounts: Vec<String>,
    /// The token it resumes with.
    pub session_token: BigUint,
    /// Session ID surfaced to webapp REST clients as `x-ccp-session-id`.
    /// Sourced from the post-auth FIX logon ACK, falling back to the locally generated
    /// session ID when the gateway does not echo one back.
    pub server_session_id: String,
    /// What the trading connection authenticates with.
    pub ccp_token: String,
    /// How often the venue expects to hear from this client.
    pub heartbeat_interval: u64,
    /// Stored for farm reconnection.
    pub hw_info: String,
    /// What the client string encoded to.
    pub encoded: String,
    /// Raw soft dollar tier data from CCP logon tag 6560.
    pub raw_soft_dollar_tiers: String,
    /// Raw family code data from CCP logon tag 6823.
    pub raw_family_codes: String,
    /// Raw news provider data from CCP logon tag 6830.
    pub raw_news_providers: String,
    /// Raw per-security-type order permissions from CCP logon tag 6652.
    pub raw_order_permissions: String,
    /// What the venue says it has turned on for this session, as it states
    /// them at logon (tag 6542).
    ///
    /// Several of the counterpart's behaviours are conditioned on these rather
    /// than on any setting: whether it hands a Nasdaq listing back under its
    /// older spelling is one. Kept so that what this client does can be told
    /// apart from what it was permitted to do.
    pub enabled_features: String,
    /// Raw enabled-feature token list from CCP logon tag 6542.
    pub raw_enabled_features: String,
    /// White branding ID from CCP logon (empty for standard accounts).
    pub white_branding_id: String,
    /// Logical-name → host URL map pushed by the gateway during logon. Empty when no
    /// URL set was pushed (callers should then fall back to a documented literal,
    /// e.g. `api.ibkr.com` for `region_dam`).
    pub misc_urls: std::collections::HashMap<String, String>,
    /// CCP HMAC signing key (kb[64..84]) for selective signing of XML messages.
    pub ccp_sign_key: Vec<u8>,
    /// CCP HMAC initial IV (kb[48..64]) for selective signing.
    pub ccp_sign_iv: Vec<u8>,
    /// Historical-data farm routing parsed from the auth-server response,
    /// retained for HMDS reconnect.
    pub hmds_host: String,
    /// Which historical farm the venue routed this session to.
    pub hmds_farm: String,
    /// Another session already logged in on this account when this one
    /// connected, as the venue stated it. `None` when this session is alone.
    pub competing: Option<CompetingSession>,
    /// When this session logged in, in the spelling the venue uses for the
    /// same fact about somebody else.
    ///
    /// The two are comparable, which is what makes a takeover tellable from a
    /// ghost: a session that logged in after this one is another client, and a
    /// session that logged in before it is this one's own, still being reaped.
    pub logged_in_at: String,
    /// The auth servers this session's logon ran through, in the order it
    /// reached them: the one it knocked on, then each one it was sent to.
    ///
    /// Every entry is a host the venue named, so a reconnect that cannot reach
    /// the one it was last on has somewhere else to try that was stated rather
    /// than guessed. A session that was never redirected has one.
    pub auth_hosts: Vec<String>,
    /// Trading farm routing from the same response, retained so the trading
    /// reconnect uses the route the auth server gave rather than a literal.
    pub trading_host: String,
    /// Which trading farm.
    pub trading_farm: String,
    /// Security-definition farm routing from the same response, retained so a
    /// connection that goes can be rebuilt. The calendar rides this one.
    pub secdef_host: String,
    /// Which security-definition farm.
    pub secdef_farm: String,
    /// The port the venue named for the trading farm, where it named one.
    pub trading_port: Option<u16>,
    /// The port stated for the historical farm.
    pub hmds_port: Option<u16>,
    /// The port stated for the security-definition farm.
    pub secdef_port: Option<u16>,
}

/// Which farm a connection is to, which decides two separate numbers the
/// logon carries and which were held as one for as long as there were only
/// two farms.
///
/// The logon states a version, and the routing request that follows states
/// which service is being asked for. They are not the same number: every
/// service logs on at seventeen except market data, which logs on at
/// eighteen, while the service number counts from zero across all of them.
/// Held as one value, a third farm cannot be stated at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Farm {
    /// Quotes, depth and the ticks that follow them.
    MarketData,
    /// Bars, historical ticks and schedules.
    Historical,
    /// Contract definitions, and the calendar that rides with them.
    SecurityDefinition,
}

impl Farm {
    /// The version this farm's logon states.
    pub fn login_version(self) -> u32 {
        match self {
            Self::MarketData => 18,
            Self::Historical | Self::SecurityDefinition => 17,
        }
    }

    /// The service this farm is, as the routing request states it.
    pub fn channel_id(self) -> &'static str {
        match self {
            Self::MarketData => "1",
            Self::Historical => "2",
            Self::SecurityDefinition => "4",
        }
    }
}

/// A session: the gateway, and the connections it opened.
///
/// Named rather than returned as a row of five, two of which are optional and
/// none of which said which farm it was.
pub struct Session {
    /// The session itself.
    pub gateway: Gateway,
    /// Quotes, depth and the ticks that follow them.
    pub market_data: Connection,
    /// Orders, positions, account figures and contract lookups.
    pub trading: Connection,
    /// Bars, historical ticks and tick-by-tick streams.
    pub historical: Option<Connection>,
    /// Contract definitions and the calendar that rides with them.
    pub security_definition: Option<Connection>,
}

/// Connect to a data farm: key exchange → encrypted logon → token auth → routing → Connection.
pub fn connect_farm(
    settings: &crate::api::settings::SessionSettings,
    host: &str,
    farm_id: &str,
    username: &str,
    password: &str,
    paper: bool,
    server_session_id: &str,
    session_key: &BigUint,
    hw_info: &str,
    encoded: &str,
    farm: Farm,
    stated_port: Option<u16>,
) -> io::Result<Connection> {
    // What the venue said to reach this farm on. The configured port applies
    // only where it said nothing.
    let port = stated_port.unwrap_or(settings.port);
    let farm_host = settings
        .market_data_host
        .clone()
        .unwrap_or_else(|| host.to_string());
    log::info!("Connecting to {farm_id} {farm_host}:{port}");
    let addr = format!("{farm_host}:{port}")
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "DNS resolution failed"))?;
    let farm_tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(TIMEOUT_FARM_CONNECT))
        .map_err(|e| io::Error::new(e.kind(), format!("{farm_id} TCP connect: {e}")))?;
    farm_tcp.set_nodelay(true)?;
    farm_tcp.set_read_timeout(Some(Duration::from_secs(TIMEOUT_FARM_CONNECT)))?;

    // Key exchange (raw TCP)
    let mut channel = SecureChannel::new();
    let dh_msg = channel.build_secure_connect(NS_VERSION, NS_VERSION);
    let mut stream = farm_tcp;
    stream.write_all(&dh_msg)?;

    let (payload, _) = ns::ns_recv(&mut stream)?;
    let text = String::from_utf8_lossy(&payload);
    let parts: Vec<&str> = text.split(';').collect();
    let msg_type: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    if msg_type != ns::NS_SECURE_CONNECTION_START {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{farm_id} DH: expected 533, got {msg_type}"),
        ));
    }
    channel.process_server_hello(parts.get(2..).unwrap_or(&[]))?;
    log::info!("{farm_id} key exchange complete");

    // Encrypted logon
    let farm_session_id = if server_session_id.is_empty() {
        session::get_session_id()
    } else {
        server_session_id.to_string()
    };
    let logon_bytes = build_farm_encrypted_logon(
        settings,
        &mut channel, username, paper, farm_id,
        &farm_session_id, session_key, hw_info, encoded, farm.login_version(),
    );
    stream.write_all(&logon_bytes)?;
    log::info!("{farm_id} encrypted logon sent");

    // Logon exchange: challenge → token auth → logon ACK
    let read_mac_key = channel.key_block().map(|kb| kb[84..104].to_vec()).unwrap_or_default();
    let initial_read_iv = channel.key_block().map(|kb| kb[48..64].to_vec()).unwrap_or_default();
    let FarmLogon { read_iv, sign_iv, remaining: logon_remaining, heartbeat_secs: stated_heartbeat } =
        farm_logon_exchange(
        &mut stream, &mut channel, session_key, username, password,
        &read_mac_key, &initial_read_iv,
    )?;
    log::info!("{} logon exchange complete, {} bytes remaining", farm_id, logon_remaining.len());

    let sign_mac_key = channel.key_block().map(|kb| kb[64..84].to_vec()).unwrap_or_default();

    // Send routing table request after logon.
    let channel_id = farm.channel_id();
    let now = chrono_free_timestamp();
    let routing_msg = fix_build(&[
        (fix::TAG_MSG_TYPE, "U"),
        (fix::TAG_SENDING_TIME, &now),
        (6040, "112"),
        (6556, channel_id),
    ], 1);
    let wrapped = fixcomp::fixcomp_build(&routing_msg);

    let (signed, new_sign_iv) = fix::fix_sign(&wrapped, &sign_mac_key, &sign_iv);
    stream.write_all(&signed)?;
    let final_sign_iv = new_sign_iv;
    log::info!("{farm_id} sent routing request (6556={channel_id})");

    // Read routing response. Frame-based termination: poll with a short
    // timeout, break as soon as one complete FIXCOMP frame is held
    // buffered. The 5-s read timeout remains as the worst-case fallback.
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut resp_buf = Vec::new();
    let routing_deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut tmp = [0u8; 8192];
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                resp_buf.extend_from_slice(&tmp[..n]);
                if has_complete_response_frame(&resp_buf) { break; }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock
                || e.kind() == io::ErrorKind::TimedOut =>
            {
                if has_complete_response_frame(&resp_buf) { break; }
                if std::time::Instant::now() >= routing_deadline { break; }
            }
            Err(e) => return Err(e),
        }
    }
    log::info!("{} routing response: {} bytes", farm_id, resp_buf.len());

    // Create Connection (switches to non-blocking), inject routing bytes
    let mut conn = Connection::new_raw(stream)?;
    conn.set_keys(sign_mac_key, final_sign_iv, read_mac_key, read_iv);
    conn.heartbeat_secs = stated_heartbeat;
    conn.seq = 1; // routing request was seq=1; next send_fix will be seq=2

    // Inject logon remaining bytes + routing response into connection buffer.
    // Python processes logon remaining before routing, but both need read_iv chaining.
    if !logon_remaining.is_empty() {
        conn.inject_buf(&logon_remaining);
    }
    if !resp_buf.is_empty() {
        conn.inject_buf(&resp_buf);
    }
    // Extract and process all frames (unsign + respond to TestRequests, like Python).
    let frames = conn.extract_frames();
    for frame in &frames {
        match frame {
            crate::protocol::connection::Frame::FixComp(raw) => {
                let Some(unsigned) = conn.unsign(raw) else { continue };
                let inner = fixcomp::fixcomp_decompress(&unsigned).unwrap_or_else(|e| {
                    log::warn!("{farm_id}: dropping malformed FIXCOMP frame: {e}");
                    Vec::new()
                });
                for m in &inner {
                    let parsed = fix_parse(m);
                    let mt = parsed.get(&35).map(|s| s.as_str()).unwrap_or("");
                    log::debug!("{farm_id} routing compressed inner 35={mt}");
                    if mt == "1" {
                        let test_id = parsed.get(&112).cloned().unwrap_or_default();
                        let ts = chrono_free_timestamp();
                        let _ = conn.send_fix(&[
                            (fix::TAG_MSG_TYPE, "0"),
                            (fix::TAG_SENDING_TIME, &ts),
                            (112, &test_id),
                        ]);
                    }
                }
            }
            crate::protocol::connection::Frame::Fix(raw) => {
                let Some(unsigned) = conn.unsign(raw) else { continue };
                let parsed = fix_parse(&unsigned);
                let mt = parsed.get(&35).map(|s| s.as_str()).unwrap_or("");
                log::debug!("{farm_id} routing FIX 35={mt}");
                if mt == "1" {
                    let test_id = parsed.get(&112).cloned().unwrap_or_default();
                    let ts = chrono_free_timestamp();
                    let _ = conn.send_fix(&[
                        (fix::TAG_MSG_TYPE, "0"),
                        (fix::TAG_SENDING_TIME, &ts),
                        (112, &test_id),
                    ]);
                }
            }
            crate::protocol::connection::Frame::Binary(raw) => {
                let Some(unsigned) = conn.unsign(raw) else { continue };
                log::info!("{} routing 8=O: {} bytes", farm_id, raw.len());
                // The table this farm answered with. Read here because this
                // is the only place it is sent: once, on the connection that
                // asked for it.
                let text = String::from_utf8_lossy(&unsigned);
                let body = text.split_once("6556=").map_or("", |(_, rest)| {
                    rest.split_once('\x01').map_or("", |(_, table)| table)
                });
                let table = crate::protocol::routing::RoutingTable::parse(body);
                if !table.is_empty() {
                    // Which farms serve a book, next to the farm this session
                    // is on: a book served from another farm is asked for on a
                    // connection that does not serve it.
                    let book_farms = table.book_farms();
                    log::info!(
                        "{farm_id} routing table: {} markets across {} farms; \
                         books served from {:?}",
                        table.rows().len(), table.farms().len(), book_farms,
                    );
                    conn.routing = table;
                }
                if let Ok(dir) = std::env::var("IBX_CAPTURE_WIRE")
                    && dir != "1"
                {
                    let path = format!("{dir}/routing-{farm_id}.bin");
                    let _ = std::fs::write(&path, &unsigned);
                    log::info!("{farm_id} routing table written to {path}");
                }
            }
            crate::protocol::connection::Frame::Control(raw) => {
            // 8=1 / 8=X control state — extracted, not routed.
                log::debug!("{} ignoring control frame: {} bytes", farm_id, raw.len());
            }
        }
    }
    if !frames.is_empty() {
        log::info!("{} post-logon frames: {} frames, seq now {}", farm_id, frames.len(), conn.seq);
    }
    Ok(conn)
}

/// The hosts left to try, given every host this session reached the venue
/// through and the one being reconnected to now.
///
/// Order is the order they were reached in. The host being tried is not among
/// them: retrying the one that just failed is not a second attempt.
fn alternates_to(seen: &[String], current: &str) -> Vec<String> {
    seen.iter().filter(|host| *host != current).cloned().collect()
}

/// Reconnect to the CCP (order/auth) server using cached session credentials.
/// Performs TLS + DH + CONNECT_REQUEST, then attempts SOFT_TOKEN auth with cached K.
/// If the server signals at AUTH_START that it requires full SRP, transparently
/// falls back to a fresh SRP handshake using the cached `username`/`password`
/// in `ReconnectAuth` (the same path used by `Gateway::connect`).
pub fn reconnect_ccp(auth: &ReconnectAuth) -> io::Result<Connection> {
    let token_hash = token_short_hash(&auth.session_token);
    let first = reconnect_ccp_attempt(auth, &token_hash, &auth.host, 0);
    let Err(why) = first else { return first };

    // The host this session was on cannot be reached. The others it reached
    // the venue through still answer for this account — the venue named them
    // on the way in — so they are tried before the session is given up on.
    // Unless the account is the thing that was lost. Every host answers for
    // the same account, so walking them re-asks the same question and, if one
    // of them answers yes, takes the account back from a client that holds it
    // fairly — the fight this refusal exists to end.
    if why.to_string().contains(TOOK_THE_ACCOUNT) {
        return Err(why);
    }

    let mut last = why;
    for host in &auth.alternate_hosts {
        log::warn!("{} did not answer ({last}); trying {host}", auth.host);
        match reconnect_ccp_attempt(auth, &token_hash, host, 0) {
            Ok(conn) => {
                log::info!("reconnected on {host}");
                return Ok(conn);
            }
            // The reason a later host gives, not the first one's. A host that
            // cannot be reached says nothing about the session; one that
            // answers and refuses the credentials says everything, and
            // returning the first meant the caller was told a transport
            // failure when the venue had stated something it could act on.
            Err(next) => {
                log::warn!("{host} did not answer either ({next})");
                last = next;
            }
        }
    }
    Err(last)
}

fn reconnect_ccp_attempt(auth: &ReconnectAuth, token_hash: &str, host: &str, depth: u32) -> io::Result<Connection> {
    if depth > 5 {
        return Err(io::Error::other("CCP reconnect: too many redirects"));
    }
    log::info!("CCP reconnect to {}:{} (attempt {})", host, AUTH_PORT, depth + 1);
    // When the venue says this logon happened, filled in from its answer
    // below. Empty until then, and empty is the careful reading: every session
    // the venue names counts as another client, so the reconnect gives the
    // account up rather than taking it from somebody who may hold it fairly.
    let mut logged_in_at = String::new();

    // TLS + DH key exchange
    let addr = format!("{host}:{AUTH_PORT}")
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "DNS resolution failed"))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(TIMEOUT_SSL_AUTH))?;
    // Bounded from the moment it is open, as the farm connection is. A peer
    // that accepts the socket and then says nothing would otherwise hold the
    // key exchange below for as long as it stays open — and on the reconnect
    // path that worker is what the scheduler waits on, so one silent endpoint
    // stops every later attempt for the life of the process.
    tcp.set_read_timeout(Some(Duration::from_secs(TIMEOUT_SSL_AUTH)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(TIMEOUT_SSL_AUTH)))?;
    let connector = TlsConnector::builder()
        .build()
        .map_err(|e| io::Error::other(e.to_string()))?;
    let mut tls = connector
        .connect(host, tcp)
        .map_err(|e| io::Error::other(e.to_string()))?;

    let mut channel = SecureChannel::new();
    let dh_msg = channel.build_secure_connect(NS_VERSION, NS_VERSION);
    tls.write_all(&dh_msg)?;

    let (payload, _) = ns::ns_recv(&mut tls)?;
    let text = String::from_utf8_lossy(&payload);
    let parts: Vec<&str> = text.split(';').collect();
    let msg_type: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    if msg_type != ns::NS_SECURE_CONNECTION_START {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("CCP reconnect DH: expected 533, got {msg_type}"),
        ));
    }
    channel.process_server_hello(parts.get(2..).unwrap_or(&[]))?;

    // CONNECT_REQUEST with SOFT_TOKEN flag + token hash (field 9)
    let flags = session::FLAG_OK_TO_REDIRECT
        | session::FLAG_VERSION
        | session::FLAG_VERSION_PRESENT
        | session::FLAG_DEVICE_INFO
        | session::FLAG_SOFT_TOKEN
        | session::FLAG_UNKNOWN_U
        | session::FLAG_UNKNOWN_19
        | session::FLAG_UNKNOWN_20
        | if auth.paper { session::FLAG_PAPER_CONNECT } else { 0 };
    let display_name = if auth.paper {
        format!("S{}", auth.username)
    } else {
        auth.username.clone()
    };
    let connect_req = format!(
        "{};{};{};{};{};27;{};{};{};{};",
        NS_VERSION_MIN,
        ns::NS_CONNECT_REQUEST,
        display_name,
        flags,
        NS_VERSION,
        auth.hw_info,
        auth.server_session_id,
        auth.encoded,
        token_hash,
    );
    session::send_secure(&mut tls, &mut channel, connect_req.as_bytes())?;
    log::info!("CCP reconnect CONNECT_REQUEST sent (session={}, hash={})", auth.server_session_id, token_hash);

    // Receive AUTH_START — may get NS_REDIRECT instead
    let auth_start = match session::recv_secure(&mut tls, &mut channel) {
        Ok(data) => data,
        Err(e) if e.to_string().starts_with("REDIRECT:") => {
            let target = e.to_string().replace("REDIRECT:", "");
            let redirect_host = target.split(':').next().unwrap_or(&target).to_string();
            log::info!("CCP reconnect redirected to {redirect_host}");
            drop(tls);
            // Floor before following: this runs on the background
            // reconnect thread, and an instant re-dial chain risks the same
            // rate limiting the backoff ladder exists for.
            std::thread::sleep(Duration::from_secs(2));
            return reconnect_ccp_attempt(auth, token_hash, &redirect_host, depth + 1);
        }
        Err(e) => return Err(e),
    };

    // Parse AUTH_START field[5] for auth mode: 2=SOFT_TOKEN, 0=SRP required
    let auth_text = auth_start_text(&auth_start)?;
    let auth_fields: Vec<&str> = auth_text.split(';').collect();
    let auth_mode: u32 = auth_fields.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);

    if auth_mode == 2 {
        // SOFT_TOKEN challenge-response (4 states)
        do_ccp_soft_token(&mut tls, &auth.session_key)?;

        // Consume AUTH_FINISH (msg_id=771) after SOFT_TOKEN PASSED
        match session::recv_msg(&mut tls) {
            Ok(session::RecvMsg::Xyz { state, fields, .. }) => {
                let result = fields.iter().rev().find(|s| !s.is_empty()).map(|s| s.as_str()).unwrap_or("");
                log::info!("CCP reconnect AUTH_FINISH: state={state} result={result}");
            }
            Ok(session::RecvMsg::Ns { msg_type, .. }) => {
                log::info!("CCP reconnect post-auth NS type={msg_type}");
            }
            Err(e) => {
                log::warn!("CCP reconnect AUTH_FINISH recv: {e}");
            }
        }
    } else {
        // Server requires full SRP (auth_mode != 2). Re-run the SRP handshake
        // with the credentials cached on ReconnectAuth — the same path
        // Gateway::connect uses on first login.
        log::info!("CCP reconnect: server requires SRP, running handshake with cached credentials");
        do_srp(&mut tls, &auth.username, &auth.password)?;
        // The same gate the first login runs. A session dropped across a
        // maintenance window comes back asking for the second factor again,
        // and a reconnect that skipped it retried a handshake the server was
        // never going to finish.
        let (token_type, token_sub_type) = parse_auth_start_token(&auth_text);
        // ponytail: a SOFT token issued here is not written back to `auth`,
        // which the reconnect thread only holds by reference — the next
        // reconnect runs SRP again rather than the cheaper token path.
        run_second_factor(&mut tls, SecondFactor {
            paper: auth.paper,
            username: &auth.username,
            token_type,
            token_sub_type,
            code_provider: auth.code_provider.as_ref(),
            timeout_secs: auth.ib_key_timeout_secs,
            default_sub_type: &auth.ib_key_token_sub_type,
        })?;
    }

    // Post-auth: wait for NS_CONNECT_RESPONSE → NEWCOMMPORTTYPE → NS_FIX_START.
    // Per-iteration read timeout aligned with the initial-connect path.
    tls.get_ref().set_read_timeout(Some(Duration::from_secs_f64(TIMEOUT_FIX_LOGON)))?;
    // A read that times out here is the data start still being on its way, not
    // its absence. Giving up on the first one failed the reconnect outright and
    // sent the whole thing round the backoff ladder again — the initial connect
    // already retries to an overall deadline and describes itself as
    // mirroring this path, which never did it.
    let fix_deadline = std::time::Instant::now()
        + std::time::Duration::from_secs_f64(TIMEOUT_FIX_LOGON * 2.0);
    let mut fix_ready = false;
    // Whoever held the account when this reconnect arrived, if the venue said,
    // and the interval it holds this connection to.
    let mut took_from: Option<(String, String, bool)> = None;
    let mut stated_heartbeat: Option<u64> = None;
    while std::time::Instant::now() < fix_deadline {
        let (payload, _) = match ns::ns_recv(&mut tls) {
            Ok(r) => r,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                log::warn!("CCP reconnect post-auth recv timeout, retrying until deadline: {e}");
                continue;
            }
            Err(e) => {
                log::warn!("CCP reconnect post-auth recv: {e}");
                break;
            }
        };
        let text = String::from_utf8_lossy(&payload);
        let parts: Vec<&str> = text.split(';').collect();
        let raw_type: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        let inner = if raw_type == ns::NS_SECURE_MESSAGE {
            let ct = B64.decode(parts[2])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            channel.decrypt(&ct)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        } else {
            payload
        };

        let inner_text = String::from_utf8_lossy(&inner);
        let inner_parts: Vec<&str> = inner_text.split(';').collect();
        let msg_type: u32 = inner_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        if msg_type == ns::NS_CONNECT_RESPONSE {
            if let Some(other) = parse_competing_session(&inner_text) {
                // Whose session it is, decided by the one fact that separates
                // them: when it logged in. A session older than this one's own
                // last logon is this one's own, still being reaped by the
                // venue, and reconnecting over it finishes the reconnect. A
                // session younger than that logged in while this one was away —
                // it is another client, it took the account fairly, and taking
                // it back starts a fight that was watched happen: both sides
                // reconnecting onto each other every twenty-odd seconds, each
                // seeing only its own data stop.
                if is_another_client(&other.since, &auth.logged_in_at) {
                    return Err(io::Error::other(format!(
                        "{TOOK_THE_ACCOUNT} another client holds this account, \
                         from {} since {}, after this session logged in at {}",
                        other.ip, other.since, auth.logged_in_at,
                    )));
                }
                log::warn!(
                    "reconnected over this session's own earlier logon, from {} at {}{}",
                    other.ip, other.since,
                    if other.read_only { ", and this one may not trade" } else { "" },
                );
                // Carried out with the connection so the engine can say so.
                took_from = Some((other.ip.clone(), other.since.clone(), other.read_only));
            }
            let newcomm = format!("{};{};0;;2;0;", NS_VERSION_MIN, ns::NS_NEWCOMMPORTTYPE);
            session::send_secure(&mut tls, &mut channel, newcomm.as_bytes())?;
        } else if msg_type == ns::NS_FIX_START {
            fix_ready = true;
            break;
        } else if msg_type == ns::NS_ERROR_RESPONSE {
            return Err(io::Error::other(
                format!("CCP reconnect post-auth error: {}", inner_parts[2..].join(";")),
            ));
        }
        // Ignore 530 keepalives and other types
    }
    tls.get_ref().set_read_timeout(None)?;
    if !fix_ready {
        return Err(io::Error::other("CCP reconnect: no FIX_START after auth"));
    }

    // FIX Logon
    let logon_msg = build_ccp_logon(&auth.settings, &auth.hw_info, &auth.encoded, CCP_HEARTBEAT, 1);
    tls.write_all(&logon_msg)?;
    tls.flush()?;

    // Short poll timeout + overall deadline so a slow response segment from a
    // high-latency gateway is retried, not treated as a fatal logon failure
    // (, same tolerance as the farm-logon path).
    tls.get_ref().set_read_timeout(Some(Duration::from_millis(FARM_LOGON_POLL_MS)))?;
    let fix_deadline = std::time::Instant::now() + Duration::from_secs_f64(TIMEOUT_FARM_LOGON);
    // What a read took past the end of the message it returned. On this path
    // the venue pushes what it holds as soon as the logon is answered, so
    // those bytes are the session's own state and are handed to the connection
    // below rather than dropped here.
    let mut carry: Vec<u8> = Vec::new();
    for _ in 0..5 {
        let response = fix_read_deadline(&mut tls, &mut carry, fix_deadline)?;
        let fields = fix_parse(&response);
        let msg_type = fields.get(&35).map(|s| s.as_str()).unwrap_or("");
        match msg_type {
            "3" | "5" => {
                let reason = fields.get(&58).map(|s| s.as_str()).unwrap_or("unknown");
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("CCP reconnect logon rejected: {reason}"),
                ));
            }
            "A" | "U" => {
                // The interval the venue holds this connection to. Read from
                // the answer for the same reason the first logon reads it: the
                // number this client proposes is not what it is held to, and a
                // reconnect that kept the old connection's number would answer
                // a new agreement at the old rate.
                if let Some(stated) = fields.get(&108).and_then(|v| v.parse::<u64>().ok())
                    && stated > 0
                {
                    log::info!("the venue holds this reconnect to a {stated}s heartbeat");
                    stated_heartbeat = Some(stated);
                }
                // When the venue says this logon happened, by its own clock.
                // The competing session it names is stamped by that clock too,
                // and one clock's readings are comparable where two machines'
                // are not.
                //
                // This clock only where the venue states none. Its readings are
                // the venue's to within the drift between the two, and drift is
                // a worse risk than the alternative: with no stamp at all every
                // session the venue names reads as another client, and a
                // reconnect that meets its own logon still being reaped would
                // hand the account back rather than finish.
                logged_in_at = fields.get(&52).cloned()
                    .unwrap_or_else(|| chrono_free_timestamp().to_string());
                break;
            }
            _ => {}
        }
    }
    tls.get_ref().set_read_timeout(None)?;

    // Order mass status request
    let mut ccp_seq: u32 = 1;
    ccp_seq += 1;
    let now = chrono_free_timestamp();
    let status_req = fix_build(&[(35, "H"), (52, &now), (11, "*"), (54, "*"), (55, "*")], ccp_seq);
    tls.write_all(&status_req)?;
    tls.flush()?;

    let mut conn = Connection::new(tls)?;
    if !carry.is_empty() {
        log::info!("{} bytes arrived with the reconnect ACK", carry.len());
        conn.seed_buffer(&carry);
    }
    conn.seq = ccp_seq;
    log::info!("CCP reconnect complete (seq={})", conn.seq);
    conn.competing = took_from;
    conn.heartbeat_secs = stated_heartbeat;
    // Where this attempt actually landed. A redirect followed here is followed
    // again on every later attempt unless the session remembers it.
    conn.connected_host = Some(host.to_string());
    conn.logged_in_at = Some(logged_in_at);
    Ok(conn)
}

/// SOFT_TOKEN challenge-response over the TLS/NS channel (for CCP reconnect).
fn do_ccp_soft_token<S: Read + Write>(stream: &mut S, session_key: &BigUint) -> io::Result<()> {
    use crate::protocol::xyz;

    // State 1: Send empty init
    let msg1 = xyz::xyz_build_soft_token(1, "", "", "");
    stream.write_all(&xyz::xyz_wrap(&msg1))?;

    // State 2: Receive challenge
    let recv2 = session::recv_msg(stream)?;
    let challenge_hex = match recv2 {
        session::RecvMsg::Xyz { state: 2, fields, .. } => {
            fields.get(1).filter(|s| !s.is_empty()).cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "CCP SOFT_TOKEN: empty challenge"))?
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "CCP SOFT_TOKEN: expected XYZ state 2")),
    };

    // SHA-1(strip(challenge) || strip(token))
    let challenge_int = BigUint::parse_bytes(challenge_hex.as_bytes(), 16)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid challenge hex"))?;
    let challenge_be = challenge_int.to_bytes_be();
    let challenge_bytes = strip_leading_zeros(&challenge_be);
    let token_be = session_key.to_bytes_be();
    let token_bytes = strip_leading_zeros(&token_be);

    let mut hasher = Sha1::new();
    hasher.update(challenge_bytes);
    hasher.update(token_bytes);
    let response_hex = format!("{:x}", BigUint::from_bytes_be(&hasher.finalize()));

    // State 3: Send response
    let msg3 = xyz::xyz_build_soft_token(3, "", &response_hex, "");
    stream.write_all(&xyz::xyz_wrap(&msg3))?;

    // State 4: Receive result
    let recv4 = session::recv_msg(stream)?;
    let result = match recv4 {
        session::RecvMsg::Xyz { fields, .. } => {
            fields.iter().rev().find(|s| !s.is_empty()).cloned().unwrap_or_default()
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "CCP SOFT_TOKEN: expected XYZ state 4")),
    };

    if result == "PASSED" {
        log::info!("CCP SOFT_TOKEN auth passed");
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("CCP SOFT_TOKEN auth failed: {result}"),
        ))
    }
}

/// Configuration for connecting to IB.
pub struct GatewayConfig {
    /// What this session runs under, settled before it opened.
    ///
    /// Held per session rather than read from the process as it goes, so a
    /// second session in one process cannot write its own settings where the
    /// first session's reconnects would find them.
    pub settings: std::sync::Arc<crate::api::settings::SessionSettings>,
    /// The login.
    pub username: String,
    /// Wrapped in `Zeroizing` so the plaintext is wiped from memory on drop.
    pub password: Zeroizing<String>,
    /// Which host to open the session against.
    pub host: String,
    /// Whether this is a paper session. It decides one step of the logon and
    /// nothing after it.
    pub paper: bool,
    /// Accept invalid TLS certificates during auth. Default: `false` (secure).
    /// Only set to `true` for local testing against self-signed gateways.
    pub accept_invalid_certs: bool,
    /// Per-session second-factor approval timeout. Defaults to
    /// [`session::IB_KEY_DEFAULT_TIMEOUT_SECS`] (~18 min, matching the
    /// server-side deadline). Set lower to fail fast for unattended logins.
    /// Only consulted on non-paper logins; paper logins skip the gate entirely.
    pub ib_key_timeout_secs: u64,
    /// Fallback second-factor token sub-type for the SWCR_TOKEN state=1 init
    /// body (`M.D` field).
    ///
    /// `AUTH_START` states the sub-type for the session, and that value wins:
    /// it is account- and session-specific, so a fixed setting can only ever
    /// match the profile it came from. This is what gets used when the server
    /// states none. Default `"2a"` matches the captured reference profile.
    pub ib_key_token_sub_type: String,
    /// A session from an earlier connect, to log on with instead of the
    /// password.
    ///
    /// The server keeps a session alive past the process that made it, and will
    /// answer a request that names one with a challenge rather than a full
    /// handshake — which is how a client comes back after a restart without a
    /// person to approve it. Ignored unless it names this same account and the
    /// same kind of session, and falls back to the password whenever the server
    /// declines it. Obtained from [`EClient::session()`](crate::api::client::EClient::session).
    pub resume: Option<crate::auth::resume::ResumableSession>,

    /// Supplies the second factor's code, for whichever exchange `AUTH_START`
    /// selects.
    ///
    /// On the IBKey path it takes the **Challenge/Response** route instead of
    /// waiting for a mobile push: after the server delivers state=2 the
    /// callback is invoked once with the challenge details and the returned
    /// 8-character code is submitted as state=3. `None` there leaves behaviour
    /// unchanged, and the login completes by push approval.
    ///
    /// On an authenticator-code account it is the only way to log in — there is
    /// no push to approve — so `None` fails the connect rather than falling
    /// back. See [`session::CodeProvider`] for the contract.
    pub code_provider: Option<session::CodeProvider>,
}

/// Which second-factor exchange an `AUTH_START` token type selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecondFactorRoute {
    /// No gate at all. Paper sessions never present a second factor.
    None,
    /// `XYZ 775` — the IBKey push, or its Challenge/Response variant.
    IbKey,
    /// `XYZ 774` — an authenticator code.
    SecurityCode,
    /// A type this client has no exchange for. Better to say so than to send
    /// the wrong message and report whatever the server does about it.
    Unsupported,
}

/// The text of a decrypted frame that is `AUTH_START`, and an error for one
/// that is not.
///
/// `recv_secure` clears the outer envelope only. Everything taken out of the
/// frame after it is taken by position — the second-factor type and sub-type,
/// the auth mode — so another message type at that point in the handshake
/// supplies those from fields that mean something else, and the login fails
/// later as a rejected token or a closed socket rather than as the wrong frame
/// it was.
fn auth_start_text(payload: &[u8]) -> io::Result<String> {
    let text = String::from_utf8_lossy(payload).into_owned();
    let msg_type: u32 = text.split(';').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    if msg_type != ns::NS_AUTH_START {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Expected AUTH_START ({}), got {}", ns::NS_AUTH_START, msg_type),
        ));
    }
    Ok(text)
}

/// The second-factor token AUTH_START names, as `(type, sub-type)`.
///
/// Field 4 carries the type, optionally followed by a per-session sub-type
/// after a `.`, and carries a comma-separated list when the account has more
/// than one factor enabled. The list is split first, so a sub-type on one
/// entry cannot swallow the entries after it — `"5.2i,4"` is type `5`
/// sub-type `2i` and a second entry, not a sub-type of `"2i,4"`.
///
/// The sub-type returned is the one belonging to the type the gate will route
/// to, and the type string keeps the whole list for that routing decision.
fn parse_auth_start_token(auth_start: &str) -> (String, Option<String>) {
    let fields: Vec<&str> = auth_start.split(';').collect();
    let token = fields.get(4).map(|f| f.trim()).unwrap_or("");

    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    for entry in token.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (ty, sub) = match entry.split_once('.') {
            Some((t, s)) => (t.trim(), s.trim()),
            None => (entry, ""),
        };
        entries.push((ty.to_string(), (!sub.is_empty()).then(|| sub.to_string())));
    }

    // The sub-type has to belong to the type the gate routes to, so which type
    // that is has to be decided first — exactly as `second_factor_route`
    // decides it, IBKey when present and the authenticator otherwise. Searching
    // for a sub-type across types instead lets an entry the gate is not using
    // supply one: `4.auth,5` routes to IBKey and would send `auth`.
    let routed = ["5", "4"]
        .into_iter()
        .find(|want| entries.iter().any(|(ty, _)| ty == want))
        .or_else(|| entries.first().map(|(ty, _)| ty.as_str()));
    let sub_type = routed.and_then(|routed| {
        entries.iter()
            .find(|(ty, sub)| ty == routed && sub.is_some())
            .and_then(|(_, sub)| sub.clone())
    });

    let types: Vec<&str> = entries.iter().map(|(ty, _)| ty.as_str()).collect();
    (types.join(","), sub_type)
}

/// What the second-factor gate needs, whether this is the first login or a
/// reconnect that the server bumped back to SRP.
pub(crate) struct SecondFactor<'a> {
    pub paper: bool,
    pub username: &'a str,
    /// Token type and per-session subtype as AUTH_START stated them.
    pub token_type: String,
    pub token_sub_type: Option<String>,
    pub code_provider: Option<&'a session::CodeProvider>,
    pub timeout_secs: u64,
    pub default_sub_type: &'a str,
}

/// What the venue says between authentication and the data start.
enum PostAuth {
    /// The data start arrived, naming whoever else was already logged in when
    /// this session got here — where the venue named one.
    Ready(Option<CompetingSession>),
    /// The venue sent this session somewhere else before it got that far.
    Redirect(String, u16),
}

/// Wait for the data-farm start, answering what the venue asks for on the way.
///
/// A transient stall here must not be fatal: a single read timeout does not
/// end the wait while the data start is still pending, and keepalive chatter
/// does not exhaust a fixed iteration budget before it arrives. So this
/// retries within an overall deadline and ignores intervening messages,
/// mirroring the CCP-reconnect path.
fn wait_for_data_start(
    tls: &mut native_tls::TlsStream<TcpStream>,
    channel: &mut SecureChannel,
    port: u16,
) -> io::Result<PostAuth> {
    // Receive post-auth messages (encrypted via 534) and wait for the
    // data-farm start (NS_FIX_START). A transient stall here must not be
    // fatal. A single read timeout does not end the wait while the data
    // start is still pending, and keepalive chatter does not exhaust a
    // fixed iteration budget before it arrives.
    // Retry within an overall deadline and ignore intervening messages,
    // mirroring the CCP-reconnect path.
    tls.get_ref().set_read_timeout(Some(Duration::from_secs_f64(TIMEOUT_FIX_LOGON)))?;
    let fix_deadline = std::time::Instant::now()
        + std::time::Duration::from_secs_f64(TIMEOUT_FIX_LOGON * 2.0);
    let mut fix_ready = false;
    // Whoever was already here when this session arrived, as the venue
    // said so in its answer to the connect.
    let mut competing: Option<CompetingSession> = None;
    while std::time::Instant::now() < fix_deadline {
        let (payload, _) = match ns::ns_recv(&mut *tls) {
            Ok(r) => r,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                log::warn!("Post-auth recv timeout, retrying until deadline: {e}");
                continue;
            }
            Err(e) => {
                log::warn!("Post-auth recv error: {e}");
                break;
            }
        };
        let text = String::from_utf8_lossy(&payload);
        let parts: Vec<&str> = text.split(';').collect();
        let raw_type: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        // Decrypt if encrypted, otherwise use raw
        let inner = if raw_type == ns::NS_SECURE_MESSAGE {
            let ct = B64.decode(parts[2])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            channel.decrypt(&ct)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        } else if raw_type == ns::NS_SECURE_ERROR {
            return Err(io::Error::other(
                format!("Post-auth secure error: {}", parts[2..].join(";")),
            ));
        } else if raw_type == ns::NS_REDIRECT {
            let target = parts.get(2).unwrap_or(&"");
            let (redirect_host, redirect_port) = host_and_port(target, port);
            log::info!("Post-auth redirect to {redirect_host}:{redirect_port}");
            return Ok(PostAuth::Redirect(redirect_host.to_string(), redirect_port));
        } else {
            payload
        };

        let inner_text = String::from_utf8_lossy(&inner);
        let inner_parts: Vec<&str> = inner_text.split(';').collect();
        let msg_type: u32 = inner_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        if msg_type == ns::NS_CONNECT_RESPONSE {
            log::info!("NS_CONNECT_RESPONSE: {inner_text}");
            competing = parse_competing_session(&inner_text);
            if let Some(other) = &competing {
                log::warn!(
                    "another session was already logged in from {} at {}{}",
                    other.ip, other.since,
                    if other.read_only { ", and this one may not trade" } else { "" },
                );
            }
            // Send port type change (required before data start)
            let newcomm = format!("{};{};0;;2;0;", NS_VERSION_MIN, ns::NS_NEWCOMMPORTTYPE);
            session::send_secure(tls, channel, newcomm.as_bytes())?;
            log::info!("Port type change sent");
        } else if msg_type == ns::NS_FIX_START {
            log::info!("Data start: {inner_text}");
            fix_ready = true;
            break;
        } else if msg_type == ns::NS_ERROR_RESPONSE {
            return Err(io::Error::other(
                format!("Post-auth error: {}", inner_parts[2..].join(";")),
            ));
        } else {
            log::info!("Post-auth msg type={msg_type}: {inner_text}");
        }
    }
    if !fix_ready {
        // TimedOut (not Other) so callers can distinguish a transient
        // post-auth handshake miss — which is retryable — from a genuine
        // auth failure.
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Never received data start after auth",
        ));
    }
    Ok(PostAuth::Ready(competing))
}

/// Dial the auth server and agree a key with it.
///
/// Returns the stream and the channel every message after this is encrypted
/// with. The port a redirect named travels with the redirect for the record;
/// what is dialled is the auth port, for the reason stated below.
fn dial_auth_server(
    config: &GatewayConfig,
    host: &str,
    port: u16,
) -> io::Result<(native_tls::TlsStream<TcpStream>, SecureChannel)> {
    // --- Phase 1: TLS + auth ---
    //
    // On the auth port, not the port the redirect named. The venue does
    // state one — a redirect to this host carries 4000 — and it is not
    // where the logon is answered: connecting there is accepted at the
    // socket and then reset, and the session only completes on 4001.
    // Measured on a live account, both ports, every redirect in the chain.
    // So the port travels with the redirect for the record, and the auth
    // port is what is dialled.
    if port != AUTH_PORT {
        log::debug!("redirect named port {port}; auth is answered on {AUTH_PORT}");
    }
    log::info!("Connecting to auth server {host}:{AUTH_PORT}");
    let addr = format!("{host}:{AUTH_PORT}")
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "DNS resolution failed"))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(TIMEOUT_SSL_AUTH))?;
    // Bounded from the moment it is open, as the farm connection is. A
    // peer that accepts the socket and then says nothing would otherwise
    // hold the key exchange below for as long as it stays open, and the
    // caller's connect with it.
    tcp.set_read_timeout(Some(Duration::from_secs(TIMEOUT_SSL_AUTH)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(TIMEOUT_SSL_AUTH)))?;

    let connector = TlsConnector::builder()
        .danger_accept_invalid_certs(config.accept_invalid_certs)
        .build()
        .map_err(|e| io::Error::other(e.to_string()))?;
    let mut tls = connector
        .connect(host, tcp)
        .map_err(|e| io::Error::other(e.to_string()))?;

    // Key exchange
    let mut channel = SecureChannel::new();
    let dh_msg = channel.build_secure_connect(NS_VERSION, NS_VERSION);
    tls.write_all(&dh_msg)?;

    let (payload, _) = ns::ns_recv(&mut tls)?;
    let text = String::from_utf8_lossy(&payload);
    let parts: Vec<&str> = text.split(';').collect();
    let msg_type: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    if msg_type == ns::NS_SECURE_ERROR {
        return Err(io::Error::other(
            format!("DH error: {}", parts[2..].join(";")),
        ));
    }
    if msg_type != ns::NS_SECURE_CONNECTION_START {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Expected 533, got {msg_type}"),
        ));
    }
    channel.process_server_hello(parts.get(2..).unwrap_or(&[]))?;
    log::info!("Auth key exchange complete");
    Ok((tls, channel))
}

/// Open the farm connections this login is routed to, all at once.
///
/// Each logon is about six seconds sequentially, and both servers accept
/// concurrent logons with the same credentials, so running them together
/// halves the phase — see examples/ex_parallel_farm_logon.rs. Validated
/// against paper and live.
///
/// Only the trading farm is required. Everything else works without the other
/// two, and a session that refused to open because one farm was down would be
/// worse than one that says which farm it has.
fn connect_farms(
    config: &GatewayConfig,
    session_id: &str,
    token: &BigUint,
    hw_info: &str,
    encoded: &str,
    trading: (&str, &str, Option<u16>),
    mktdata: (&str, &str, Option<u16>),
    secdef: Option<(&str, &str, Option<u16>)>,
) -> io::Result<(Connection, Option<Connection>, Option<Connection>)> {
    let (farm_conn, hmds_conn, secdef_conn) = std::thread::scope(|scope| {
        let username = &config.username;
        let password = &*config.password;
        let paper = config.paper;
        let settings = config.settings.as_ref();
        let trading_handle = scope.spawn(move || {
            connect_farm(settings, trading.0, trading.1, username, password,
                paper, session_id, token, hw_info, encoded, Farm::MarketData, trading.2)
        });
        let mktdata_handle = scope.spawn(move || {
            connect_farm(settings, mktdata.0, mktdata.1, username, password,
                paper, session_id, token, hw_info, encoded, Farm::Historical, mktdata.2)
        });
        let secdef_handle = secdef.map(|(host, farm, port)| {
            scope.spawn(move || {
                connect_farm(settings, host, farm, username, password,
                    paper, session_id, token, hw_info, encoded, Farm::SecurityDefinition, port)
            })
        });
        let trading = trading_handle.join().expect("trading farm thread panicked");
        let mktdata = mktdata_handle.join().expect("mktdata farm thread panicked");
        let secdef = secdef_handle
            .map(|h| h.join().expect("secdef farm thread panicked"));
        (trading, mktdata, secdef)
    });
    let farm_conn = farm_conn?;
    let hmds_conn = match hmds_conn {
        Ok(c) => { log::info!("Historical data farm connected"); Some(c) }
        Err(e) => { log::warn!("Historical data farm connection failed (non-fatal): {e}"); None }
    };
    let secdef_conn = match secdef_conn {
        Some(Ok(c)) => { log::info!("Security definition farm connected"); Some(c) }
        Some(Err(e)) => {
            log::warn!("Security definition farm connection failed (non-fatal): {e}");
            None
        }
        None => {
            log::info!("No security definition farm route was stated at logon");
            None
        }
    };
    Ok((farm_conn, hmds_conn, secdef_conn))
}

/// The key this session signs with, and the token it hands to the farms.
///
/// Two ways in, and the venue has already said which one it will take: it
/// answers a request that named a session it still holds with a challenge over
/// that session, and every other request with a handshake. So this reads the
/// answer rather than deciding, and a resume that is stale, or for another
/// account, or simply older than the venue keeps, logs on with the password
/// instead of failing with an error the caller would have to know how to
/// retry.
fn authenticate(
    tls: &mut native_tls::TlsStream<TcpStream>,
    config: &GatewayConfig,
    auth_start: &[u8],
    resume_key: Option<BigUint>,
) -> io::Result<(BigUint, Option<BigUint>)> {
    // AUTH_START field 4 names the second-factor token type and its
    // per-session subtype, e.g. "5.2i" = tokenType 5 (IBKey), subtype "2i".
    // The subtype is account- and session-specific, so the compiled-in
    // default only ever matches the profile it was captured from; every
    // other account has its SWCR_TOKEN rejected and the socket closed
    // before a challenge is issued.
    let auth_text = auth_start_text(auth_start)?;
    let (server_token_type, server_token_sub_type) = parse_auth_start_token(&auth_text);

    // Field 5 of AUTH_START says which of the two the server will accept.
    // It answers 2 only when the request named a session it still holds, so
    // a resume that is stale, or for another account, or simply older than
    // the server keeps, comes back asking for the handshake — and gets it,
    // rather than an error the caller has to know how to retry.
    let auth_mode: u32 = auth_text
        .split(';')
        .nth(5)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let key = match (resume_key, auth_mode) {
        (Some(key), 2) => {
            log::info!("Resuming the session for {} — no handshake", config.username);
            do_ccp_soft_token(tls, &key)?;
            // AUTH_FINISH follows the challenge exactly as it follows a
            // handshake, and carries nothing this needs.
            match session::recv_msg(tls) {
                Ok(session::RecvMsg::Xyz { state, .. }) => {
                    log::info!("Resume AUTH_FINISH: state={state}");
                }
                Ok(session::RecvMsg::Ns { msg_type, .. }) => {
                    log::info!("Resume post-auth NS type={msg_type}");
                }
                Err(e) => log::warn!("Resume AUTH_FINISH recv: {e}"),
            }
            // The stored token is the session key, so the farm logons that
            // follow have what they need without a second factor: the
            // approval that made this session is the one being resumed.
            (key, None)
        }
        (resume_key, _) => {
            if resume_key.is_some() {
                log::info!(
                    "The session offered was not accepted (mode {auth_mode}) — logging on with the password",
                );
            }
            log::info!("Starting auth for {}", config.username);
            let session_key = do_srp(tls, &config.username, &config.password)?;
            log::info!("Auth complete");

            // Per-session second-factor approval gate (IBKey / seamless push).
            // Skipped on paper logins; live logins enter a wait state if the
            // account has a second factor configured server-side.
            // Would capture a SOFT session token from AUTH_FINISH PASSED, but per
            // that body carries none, so this stays `None` on both
            // live paths and the farm logon falls back to the SRP session key.
            let soft_token = run_second_factor(tls, SecondFactor {
                paper: config.paper,
                username: &config.username,
                token_type: server_token_type,
                token_sub_type: server_token_sub_type,
                code_provider: config.code_provider.as_ref(),
                timeout_secs: config.ib_key_timeout_secs,
                default_sub_type: &config.ib_key_token_sub_type,
            })?;
            (session_key, soft_token)
        }
    };
    Ok(key)
}

/// Run the per-session second-factor gate after SRP. Returns the SOFT token
/// when the gate issued one.
///
/// A reconnect runs this too: the server drops a session across its own
/// maintenance windows, answers the next soft-token connect with SRP, and then
/// asks for the second factor again. Skipping it there left an unattended
/// client retrying a handshake it could never finish.
fn run_second_factor(
    tls: &mut native_tls::TlsStream<TcpStream>,
    sf: SecondFactor<'_>,
) -> io::Result<Option<BigUint>> {
    let mut soft_token: Option<BigUint> = None;
    // An advertised type this client cannot perform is worth saying out
    // loud: sending 775 at it gets the socket closed before any challenge,
    // and skipping the gate leaves the server waiting until the connect
    // dies with "Never received data start after auth". Neither names the
    // cause. See `second_factor_route` for why an absent type is
    // the one case that is not an error.
    let route = second_factor_route(sf.paper, &sf.token_type);
    if !sf.paper {
        log::debug!(
            "second factor: AUTH_START type {:?} sub {:?} -> {route:?}",
            sf.token_type, sf.token_sub_type,
        );
    }
    if route == SecondFactorRoute::SecurityCode {
        // Authenticator-code accounts take the 774 exchange rather than the
        // IBKey push. The same code_provider supplies the code.
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(sf.timeout_secs);
        log::info!(
            "Live login for {}: second factor is an authenticator code; awaiting code_provider",
            sf.username,
        );
        // The code is written raw, with no DH encryption — the same
        // transport the IBKey gate uses. The encrypted variant gets the
        // connection reset on receipt.
        // Poll the socket so the gate can submit as soon as the code is
        // available instead of waiting for the server's next keepalive: an
        // authenticator code is only valid for ~30s, and a 20s wait spends
        // most of it. Restored afterwards.
        tls.get_ref().set_read_timeout(Some(Duration::from_millis(500)))?;
        let gate = session::do_security_code_2fa(
            tls, deadline, sf.code_provider,
        );
        let restore = tls.get_ref().set_read_timeout(None);
        gate?;
        restore?;
        log::info!("security-code gate: passed")
    } else if route == SecondFactorRoute::Unsupported {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "second-factor token type {:?} is not supported; AUTH_START advertised it for {}",
                sf.token_type, sf.username,
            ),
        ));
    }
    if route == SecondFactorRoute::IbKey {
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(sf.timeout_secs);
        // Live logins enter a human-approval window here: connect() blocks
        // until the second factor is approved (mobile push) or this deadline
        // fires. Announce it up front so a stalled connect() reads as
        // "waiting for approval" rather than a hang.
        // Accounts with no second factor fall straight through (Skipped).
        if sf.code_provider.is_none() {
            log::info!(
                "Live login for {}: waiting for second-factor approval (mobile push); \
                 connect() blocks up to {}s. Use paper=true, a lower ib_key_timeout_secs, \
                 or a code_provider to avoid this.",
                sf.username, sf.timeout_secs,
            );
        } else {
            log::info!(
                "Live login for {}: second-factor via code_provider (Challenge/Response); \
                 connect() blocks up to {}s awaiting the challenge.",
                sf.username, sf.timeout_secs,
            );
        }
        // The server's per-session value wins. `ib_key_token_sub_type` is
        // the fallback for an AUTH_START that states none, not an override
        // — a fixed value cannot be right for a session it predates.
        let token_sub_type = sf.token_sub_type
            .as_deref()
            .unwrap_or(sf.default_sub_type);
        log::info!(
            "2FA gate: token sub-type {:?} ({})",
            token_sub_type,
            if sf.token_sub_type.is_some() { "from AUTH_START" } else { "configured default" },
        );
        // Polled, as the security-code gate beside this one is. The wait is a
        // person reaching for a phone, so the socket is quiet for most of it —
        // and a wait with no timeout on the socket cannot reach its own
        // deadline if the server stops talking rather than closing.
        tls.get_ref().set_read_timeout(Some(Duration::from_millis(500)))?;
        let gate = session::do_ib_key_2fa(
            tls,
            token_sub_type,
            deadline,
            sf.code_provider,
        );
        tls.get_ref().set_read_timeout(None)?;
        match gate? {
            session::IbKeyOutcome::Skipped => {
                log::info!("2FA gate: skipped (no second factor)");
            }
            session::IbKeyOutcome::Approved { approval_url, session_id, soft_token_hex } => {
                log::info!(
                    "2FA gate: approved (session_id={}, approval_url={}, token_hex_len={})",
                    if session_id.is_empty() { "<none>" } else { &session_id },
                    if approval_url.is_empty() { "<none>" } else { &approval_url },
                    soft_token_hex.len(),
                );
                if !soft_token_hex.is_empty() {
                    if let Some(tok) = BigUint::parse_bytes(soft_token_hex.as_bytes(), 16) {
                        soft_token = Some(tok);
                    } else {
                        log::warn!("2FA gate: SOFT token hex did not parse — falling back to session_key");
                    }
                }
            }
        }
    }
    Ok(soft_token)
}

/// An absent token type routes to the IBKey gate rather than skipping the
/// second factor. That gate opens by sending its init and reports `Skipped`
/// when the server answers `AUTH_FINISH PASSED`, which is how an account with
/// no second factor completes — so skipping it would leave the server waiting
/// on an init that never comes.
///
/// The field carries a comma-separated list when the account has more than one
/// factor enabled — `AUTH_START` advertised `4,5` for an account with both an
/// authenticator and IBKey. Reading the whole field as one type refused the
/// login outright, so each entry is considered and IBKey is preferred: it is
/// the only one that completes without a `code_provider`, and it still serves
/// a configured one through Challenge/Response.
fn second_factor_route(paper: bool, token_type: &str) -> SecondFactorRoute {
    if paper {
        return SecondFactorRoute::None;
    }
    if token_type.is_empty() {
        return SecondFactorRoute::IbKey;
    }
    let mut saw_security_code = false;
    for entry in token_type.split(',') {
        match entry.trim() {
            "5" => return SecondFactorRoute::IbKey,
            "4" => saw_security_code = true,
            _ => {}
        }
    }
    if saw_security_code {
        SecondFactorRoute::SecurityCode
    } else {
        SecondFactorRoute::Unsupported
    }
}
/// The buffer the init burst is scanned in.
///
/// The auth-server's logon ACK arrives DEFLATE-compressed inside one or more
/// `8=FIXCOMP` envelopes. The compressed body is ~30 kB on
/// the wire but expands to ~48 kB of plaintext carrying the routing tags
/// 6145/6171/8008, which the tag scan needs to
///
/// The inflated plaintext belongs to that scan and nowhere else. The engine is
/// handed the burst exactly as it arrived and decompresses the same segments
/// itself, so appending the plaintext to the engine's copy delivered every
/// message in the burst twice from a single delivery — once from the segment,
/// once from the appended copy. Takes the burst by reference so the
/// engine's copy cannot be the one that grows.
fn init_scan_buffer(init_data: &[u8]) -> Vec<u8> {
    let mut scan = init_data.to_vec();
    let mut cursor = 0usize;
    while cursor + 12 < init_data.len() {
        if init_data[cursor..].starts_with(b"8=FIXCOMP\x01")
            && let Some(total_len) = fixcomp::fixcomp_length(&init_data[cursor..]) {
                let segment = &init_data[cursor..cursor + total_len.min(init_data.len() - cursor)];
                let inflated = fixcomp::fixcomp_decompress(segment).unwrap_or_else(|e| {
                    log::warn!("Init FIXCOMP segment at offset {cursor}: dropping malformed frame: {e}");
                    Vec::new()
                });
                let inflated_bytes: usize = inflated.iter().map(|m| m.len() + 1).sum();
                log::info!(
                    "Init FIXCOMP segment at offset {}: {} compressed → {} inner messages, ~{} inflated bytes",
                    cursor, total_len, inflated.len(), inflated_bytes,
                );
                for inner in inflated {
                    scan.extend_from_slice(&inner);
                    scan.push(b'\x01');
                }
                cursor += total_len;
                continue;
            }
        cursor += 1;
    }
    scan
}

impl Gateway {
    /// Connect to IB: auth + logon + data farm connections.
    /// Returns Gateway + farm Connection + auth Connection + optional historical data Connection.
    pub fn connect(config: &GatewayConfig) -> io::Result<Session> {
        let first = Self::connect_to_host(config, &config.host, AUTH_PORT, 0);
        let Err(why) = first else { return first };

        // A door that does not open is not an answer. The venue runs one per
        // region and any of them will route a session to where its account
        // lives, so a session whose default door is unreachable knocks on the
        // next rather than giving up on a venue that is up.
        //
        // Only for the doors this client ships: a caller that named a host
        // meant that host, and being sent somewhere else is not failover, it
        // is the session going somewhere the caller did not ask for.
        if !crate::config::CCP_HOSTS.contains(&config.host.as_str()) {
            return Err(why);
        }
        // And only when nothing answered. A refusal is the same refusal at
        // every door, and asking again with the same credentials is how an
        // account gets locked rather than connected.
        if !nobody_answered(&why) {
            return Err(why);
        }

        let mut last = why;
        for host in alternates_to(
            &crate::config::CCP_HOSTS.iter().map(|h| h.to_string()).collect::<Vec<_>>(),
            &config.host,
        ) {
            log::warn!("{} did not answer ({last}); trying {host}", config.host);
            match Self::connect_to_host(config, &host, AUTH_PORT, 0) {
                Ok(session) => {
                    log::info!("connected through {host}");
                    return Ok(session);
                }
                // A door that answered and refused has answered for the
                // account, not for itself. Knocking on the rest asks the same
                // question with the same credentials, which is how an account
                // gets locked rather than connected.
                Err(e) if !nobody_answered(&e) => return Err(e),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Internal: connect to a specific host, with redirect depth tracking.
    fn connect_to_host(
        config: &GatewayConfig,
        host: &str,
        port: u16,
        redirect_depth: u32,
    ) -> io::Result<Session> {
        if redirect_depth > 3 {
            return Err(io::Error::other(
                "Too many redirects during auth",
            ));
        }

        // A session belongs to the account and the kind of session that made
        // it. One from another login describes a session this connect has no
        // claim on, and offering it asks the server a question about somebody
        // else's.
        let resume = config.resume.as_ref().filter(|r| {
            r.username == config.username && r.paper == config.paper
        });

        let hw_info = match resume {
            Some(r) => r.hw_info.clone(),
            None => session::get_hw_info(config.settings.hardware_id.as_deref()),
        };
        // Tag 6266 carries `{jdkVer}/{platform}/{locale}/{dist}`. The locale
        // segment must be a canonical Java `Locale.toString()` value (e.g.
        // `en_US`, `fr`, `ja_JP`); bare `en` is rejected as `invalid twsInfo`.
        // `IBX_LOCALE` overrides just the locale; `IBX_ENCODED` overrides
        // the whole string for full control.
        let encoded = match resume {
            Some(r) => r.encoded.clone(),
            None => config.settings.encoded.clone(),
        };

        let (mut tls, mut channel) = dial_auth_server(config, host, port)?;

        // Send CONNECT_REQUEST (encrypted)
        let flags = session::FLAG_OK_TO_REDIRECT
            | session::FLAG_VERSION
            | session::FLAG_VERSION_PRESENT
            | session::FLAG_DEVICE_INFO
            | session::FLAG_UNKNOWN_U
            | session::FLAG_UNKNOWN_19
            | session::FLAG_UNKNOWN_20
            | if resume.is_some() { session::FLAG_SOFT_TOKEN } else { 0 }
            | if config.paper { session::FLAG_PAPER_CONNECT } else { 0 };
        let display_name = if config.paper {
            format!("S{}", config.username)
        } else {
            config.username.clone()
        };
        // Naming the session that already exists is what lets the server
        // answer with a challenge instead of a handshake; a fresh id has no
        // session behind it and gets the handshake.
        let resume_key = resume.map(|r| BigUint::from_bytes_be(&r.token));
        let session_id = resume
            .map_or_else(session::get_session_id, |r| r.server_session_id.clone());
        let connect_req = match resume_key.as_ref() {
            Some(key) => format!(
                "{};{};{};{};{};27;{};{};{};{};",
                NS_VERSION_MIN, ns::NS_CONNECT_REQUEST, display_name, flags, NS_VERSION,
                hw_info, session_id, encoded, token_short_hash(key),
            ),
            None => format!(
                "{};{};{};{};{};27;{};{};{};",
                NS_VERSION_MIN, ns::NS_CONNECT_REQUEST, display_name, flags, NS_VERSION,
                hw_info, session_id, encoded,
            ),
        };
        session::send_secure(&mut tls, &mut channel, connect_req.as_bytes())?;

        // Receive AUTH_START (may get a redirect instead for paper accounts)
        let auth_start = match session::recv_secure(&mut tls, &mut channel) {
            Ok(data) => data,
            Err(e) if e.to_string().starts_with("REDIRECT:") => {
                let target = e.to_string().strip_prefix("REDIRECT:").unwrap().to_string();
                // Where the venue sent this session, carrying the port it
                // named alongside the host.
                let (redirect_host, redirect_port) = host_and_port(&target, port);
                log::info!("Redirected to {redirect_host}:{redirect_port}, reconnecting...");
                drop(tls);
                // The host that sent us on still answers for this account, so
                // it is worth trying if the one it named stops.
                let knocked = host.to_string();
                let mut session = Self::connect_to_host(
                    config, redirect_host, redirect_port, redirect_depth + 1,
                )?;
                if !session.gateway.auth_hosts.contains(&knocked) {
                    session.gateway.auth_hosts.push(knocked);
                }
                return Ok(session);
            }
            Err(e) => return Err(e),
        };

        let (session_key, soft_token) = authenticate(&mut tls, config, &auth_start, resume_key)?;

        let competing = match wait_for_data_start(&mut tls, &mut channel, port)? {
            PostAuth::Ready(competing) => competing,
            PostAuth::Redirect(redirect_host, redirect_port) => {
                log::info!("Reconnecting to {redirect_host}:{redirect_port}");
                drop(tls);
                return Self::connect_to_host(
                    config, &redirect_host, redirect_port, redirect_depth + 1,
                );
            }
        };

        // --- Phase 2: Auth server logon (over TLS) ---
        let logon_msg = build_ccp_logon(&config.settings, &hw_info, &encoded, CCP_HEARTBEAT, 1);
        log::info!("Sending auth logon ({} bytes)", logon_msg.len());
        tls.write_all(&logon_msg)?;
        tls.flush()?;

        // Read FIX messages until the logon ACK (35=A) carrying session info.
        // Short poll timeout + overall deadline so a slow ACK segment from a
        // high-latency gateway is retried, not fatal.
        tls.get_ref().set_read_timeout(Some(Duration::from_millis(FARM_LOGON_POLL_MS)))?;
        let ack_deadline = std::time::Instant::now() + Duration::from_secs_f64(TIMEOUT_FARM_LOGON);
        // What the venue sent past the end of the message that ended the
        // logon. It arrived before the burst below was asked for, so it leads
        // what that burst is answered with.
        let mut carry: Vec<u8> = Vec::with_capacity(65536);
        let mut ack = logon::LogonAck::read(&mut tls, &mut carry, ack_deadline)?;
        tls.get_ref().set_read_timeout(None)?;

        // Fall back to the auth session_id where the server states none
        if ack.server_session_id.is_empty() {
            ack.server_session_id = session_id.clone();
        }

        log::info!(
            "Auth logon: account={} session_id={} hb={}s",
            ack.account_id, ack.server_session_id, ack.heartbeat_interval,
        );

        // --- Post-logon init sequence ---
        let account = if ack.account_id.is_empty() { config.username.clone() } else { ack.account_id.clone() };
        let now = chrono_free_timestamp();
        let mut ccp_seq =
            logon::send_init_sequence(&mut tls, config.settings.execution_reports, &account, &now, 1)?;
        // Counted, not stated: the burst above has been edited before, and a
        // number typed beside it does not follow.
        log::info!("Init sequence sent ({} messages, seq now {})", ccp_seq - 1, ccp_seq);

        // Drain init responses — extract account ID + farm routing tags.
        // read-throughput investigation (2026-05-05):
        // the burst's bulk (~28 kB compressed) arrives in ~300 ms continuous,
        // after which the server emits 67-byte keep-alive trickles every ~10 s
        // until it FINs the socket at ~140 s. A 300 ms idle-gap is past any
        // intra-burst jitter (the burst is continuous) and well short of the
        // 10 s keep-alive trickle interval, so the read ends promptly after the burst.
        tls.get_ref().set_read_timeout(Some(Duration::from_millis(300)))?;
        let mut init_data: Vec<u8> = carry;
        let mut tmp_buf = vec![0u8; 65536];
        let read_start = std::time::Instant::now();
        loop {
            match tls.read(&mut tmp_buf) {
                Ok(0) => break,
                Ok(n) => init_data.extend_from_slice(&tmp_buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
                {
                    // First 1-s idle gap = burst is done. Anything past
                    // this is the server's 10-s keep-alive trickle, which is
                    // left undrained: draining it pushes grace-window messages
                    // past the server-side deadline.
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        log::info!(
            "Init response: {} bytes in {:?}",
            init_data.len(), read_start.elapsed(),
        );

        ack.scan_init(&init_data, &config.username);
        let logon::LogonAck {
            account_id,
            mut accounts,
            mut logged_in_at,
            heartbeat_interval,
            server_session_id,
            ccp_token,
            raw_soft_dollar_tiers,
            raw_family_codes,
            raw_news_providers,
            raw_order_permissions,
            enabled_features,
            raw_enabled_features,
            white_branding_id,
            raw_misc_urls,
            trading_route,
            mktdata_route,
            secdef_route,
            trading_port,
        } = ack;

        // Sent in plain FIX over TLS: the CCP socket has no AES/HMAC envelope
        // at this stage, as encryption is set up only after `Connection::new`.
        let post_burst_account = if account_id.is_empty() {
            config.username.clone()
        } else {
            account_id.clone()
        };
        ccp_seq = logon::send_post_burst_grace(
            &mut tls, &post_burst_account, &chrono_free_timestamp(), ccp_seq,
        )?;
        log::info!(
            "CCP post-burst grace messages sent (AR+H+PLR+DR+74), seq now {ccp_seq}"
        );

        tls.get_ref().set_read_timeout(None)?;

        // Auth connection (non-blocking TLS for hot loop)
        let mut ccp_conn = Connection::new(tls)?;
        ccp_conn.seq = ccp_seq;
        // CCP HMAC signing IV: derived by AES-CBC encrypting the logon message.
        // The logon was sent as plaintext over TLS, but the AES-CBC computation
        // evolves the IV — last 16 bytes of ciphertext = new IV for HMAC signing.
        let ccp_sign_key = channel.key_block().map(|kb| kb[64..84].to_vec()).unwrap_or_default();
        let ccp_sign_iv = if let Some(kb) = channel.key_block() {
            let aes_key = &kb[0..16];
            let initial_iv = &kb[32..48];
            let ciphertext = crate::auth::crypto::aes_cbc_encrypt(aes_key, initial_iv, &logon_msg);
            ciphertext[ciphertext.len() - 16..].to_vec()
        } else {
            Vec::new()
        };
        // Seed init burst into connection buffer so the hot loop processes 8=O account data
        ccp_conn.seed_buffer(&init_data);

        // --- Phase 3: Data farm connections ---
        // /#144/#145: the official Gateway opens exactly 3 authed TCP
        // sessions per login — MARKET_DATA (tag 6145), HISTORICAL_DATA (tag 6171), and
        // SECDEFARM (tag 8008, UI/telemetry only — not used by ibx). Per/
        // #131/#133: the SOFT token is `SHA1(strip(S))` where S is the SRP shared
        // secret. `do_srp` returns exactly that via `srp_compute_k`, so `session_key`
        // IS the SOFT token — no further hashing. (Tag 8483's per-channel SHA1 is
        // added by `token_short_hash` at the build-logon site.) Tag 6386 is an S3
        // object key, not a token source.
        let farm_token: BigUint = soft_token.clone().unwrap_or_else(|| session_key.clone());
        // read the farm names from the auth-server's
        // routing tags rather than hardcoding `usfarm`/`ushmds`. EU accounts
        // are routed to `eufarm`/`euhmds`/`secdefeu`, US to `usfarm`/`ushmds`,
        // etc. Format of the route strings:
        //   trading (6145):  "<host>/<farm>"            (port from tag 6146, default 4000)
        //   mktdata (6171):  "<host>/<farm>/<port>"
        //   secdef  (8008):  "<host>/<farm>/<port>"
        // Kept as parsed, before the fallback below materializes one. Storing
        // the materialized pair would make "no route was given" indistinguishable
        // from "the route happens to match the connect host", and the reconnect
        // would then prefer a stale host over one the caller has since updated
        // through a redirect.
        //
        // Where the venue names none, a name is invented — and an invented one
        // is a US farm on whichever host this session happened to knock at.
        // That is a guess in both halves, and a farm asked for on a host it is
        // not on answers with ten seconds of silence and a close, so it fails
        // as missing data rather than as a wrong name. Said out loud for that
        // reason: every session in this codebase's logs is named both, so if
        // this is ever read in one, it is the interesting part of it.
        let parsed_trading = parse_farm_route(&trading_route);
        let invented = |what: &str, farm: &str| {
            log::warn!(
                "the venue named no {what} route, so this session will try {farm} \
                 on {host} — a guess at both, and wrong for any account this \
                 venue does not serve from there",
            );
            (host.to_string(), farm.to_string(), None)
        };
        let (trading_host, trading_farm, trading_route_port) = parsed_trading.clone()
            .unwrap_or_else(|| invented("trading", DEFAULT_TRADING_FARM));
        // Tag first, then the route, then the configured port — the order the
        // counterpart resolves them in.
        let trading_port = trading_port.or(trading_route_port);
        let (mktdata_host, mktdata_farm, mktdata_port) = parse_farm_route(&mktdata_route)
            .unwrap_or_else(|| invented("market data", "ushmds"));
        // Contract definitions and the calendar that rides with them. Stated
        // by the venue at logon like the other two; where it states none, this
        // client simply has no such connection rather than guessing a name.
        let secdef = parse_farm_route(&secdef_route);
        log::info!("Farm routing: trading={trading_host}/{trading_farm}, mktdata={mktdata_host}/{mktdata_farm}");

        // Retain HMDS routing for the reconnect loop — the values
        // below are moved into the thread::scope closures.
        let hmds_host_for_gw = mktdata_host.clone();
        let hmds_farm_for_gw = mktdata_farm.clone();
        let (trading_host_for_gw, trading_farm_for_gw, _) =
            parsed_trading.unwrap_or_default();

        let (farm_conn, hmds_conn, secdef_conn) = connect_farms(
            config,
            &server_session_id,
            &farm_token,
            &hw_info,
            &encoded,
            (&trading_host, &trading_farm, trading_port),
            (&mktdata_host, &mktdata_farm, mktdata_port),
            secdef.as_ref().map(|(h, f, p)| (h.as_str(), f.as_str(), *p)),
        )?;

        // A family names the accounts linked to this login, which are accounts
        // this login may trade, so they belong in the list a caller asks for.
        for entry in raw_family_codes.split(';') {
            if let Some((account, _)) = entry.split_once('|') { note_account(&mut accounts, account); }
        }
        let account_id = if account_id.is_empty() { config.username.clone() } else { account_id };
        note_account(&mut accounts, &account_id);
        // The account a caller gets by default leads the list.
        if accounts.first().map(String::as_str) != Some(account_id.as_str()) {
            accounts.retain(|a| a != &account_id);
            accounts.insert(0, account_id.clone());
        }
        log::info!("Login holds {} account(s)", accounts.len());

        // This clock only where the venue stated none, for the reason the
        // reconnect states: no stamp at all reads every session the venue
        // names as another client.
        if logged_in_at.is_empty() {
            logged_in_at = chrono_free_timestamp().to_string();
            log::info!("the venue stamped no time on this logon; using this clock");
        }

        let gw = Gateway {
            competing,
            logged_in_at,
            auth_hosts: vec![host.to_string()],
            account_id,
            accounts,
            session_token: session_key,
            server_session_id,
            ccp_token,
            heartbeat_interval,
            hw_info,
            encoded,
            raw_soft_dollar_tiers,
            raw_family_codes,
            raw_news_providers,
            raw_order_permissions,
            enabled_features,
            raw_enabled_features,
            white_branding_id,
            misc_urls: parse_misc_urls(&raw_misc_urls),
            ccp_sign_key,
            ccp_sign_iv,
            hmds_host: hmds_host_for_gw,
            hmds_farm: hmds_farm_for_gw,
            trading_host: trading_host_for_gw,
            trading_farm: trading_farm_for_gw,
            secdef_host: secdef.as_ref().map(|(h, ..)| h.clone()).unwrap_or_default(),
            secdef_farm: secdef.as_ref().map(|(_, f, _)| f.clone()).unwrap_or_default(),
            trading_port,
            hmds_port: mktdata_port,
            secdef_port: secdef.as_ref().and_then(|&(_, _, p)| p),
        };
        Ok(Session {
            gateway: gw,
            market_data: farm_conn,
            trading: ccp_conn,
            historical: hmds_conn,
            security_definition: secdef_conn,
        })
    }

    /// Populate shared state with gateway-local init data parsed from CCP logon.
    pub fn populate_init_data(&self, shared: &SharedState) {
        use crate::types::{NewsProvider, SoftDollarTier, FamilyCode};

// Which venues SMART routes to, and in what order a quote's exchange
        // mask refers to them, is stated by the server on a contract's own
        // definition. Nothing is assumed before it says so: a list written
        // here carried this client's own order, and the order is the whole of
        // what the mask means, so every quote's bid, ask and last named the
        // wrong venue until a definition arrived to correct it.

        // News providers, as the logon stated them and only as it stated them.
        //
        // There is no request for this list: the counterpart serves it from the
        // logon alone, and an empty tag means the account is entitled to
        // nothing. A fallback list would state entitlements the account does
        // not hold, and a caller enumerating providers and then asking for an
        // article receives a refusal it cannot explain.
        //
        // Wire format: "code1/name1,code2/name2,…". Read as though the pairs
        // were separated by semicolons and the halves by commas, one provider
        // came back holding the whole list: its code was the first code and
        // name, and its name was every other provider. A caller then asked for
        // headlines under a code no venue knows and was answered with nothing.
        let news_providers: Vec<NewsProvider> = self
            .raw_news_providers
            .split(',')
            .filter_map(|entry| {
                let entry = entry.trim();
                if entry.is_empty() {
                    return None;
                }
                let (code, name) = entry.split_once('/')?;
                Some(NewsProvider {
                    code: code.trim().to_string(),
                    name: name.trim().to_string(),
                })
            })
            .collect();
        if news_providers.is_empty() {
            log::info!(
                "the logon stated no news providers, so this account is entitled to none"
            );
        }
        shared.reference.set_news_providers(news_providers);

        // Soft dollar tiers, as the logon states them and only as it states
        // them.
        //
        // The venue sends them comma separated, each one its value, the name it
        // is shown under, and the name it is asked for by, separated by
        // slashes: "1/Maximize Rebate/MaxRebate,9/Prefer Rebate/PreferRebate".
        //
        // This looked for a different shape entirely — semicolons between the
        // entries and bars inside them — so every entry failed to parse and a
        // list written here stood in. That list was a transcription of what the
        // venue actually sends, which is why it looked right: the data had been
        // copied out of a reply instead of read from one, and the day the venue
        // changed a tier nobody would have known.
        let tiers: Vec<SoftDollarTier> = self
            .raw_soft_dollar_tiers
            .split(',')
            .filter_map(|entry| {
                let entry = entry.trim();
                if entry.is_empty() {
                    return None;
                }
                let mut parts = entry.split('/');
                let val = parts.next()?.trim();
                let display_name = parts.next()?.trim();
                let name = parts.next()?.trim();
                if val.is_empty() || name.is_empty() {
                    return None;
                }
                Some(SoftDollarTier {
                    name: name.to_string(),
                    val: val.to_string(),
                    display_name: display_name.to_string(),
                })
            })
            .collect();
        if tiers.is_empty() && !self.raw_soft_dollar_tiers.is_empty() {
            log::warn!(
                "the logon stated soft dollar tiers in a shape this does not read: {}",
                self.raw_soft_dollar_tiers,
            );
        }
        shared.reference.set_soft_dollar_tiers(tiers);

        // Family codes: parse from CCP logon tag 6823.
        // Empty for paper/single accounts.
        let codes = if self.raw_family_codes.is_empty() {
            Vec::new()
        } else {
            self.raw_family_codes.split(';').filter_map(|entry| {
                let parts: Vec<&str> = entry.split('|').collect();
                if parts.len() >= 2 {
                    Some(FamilyCode {
                        account_id: parts[0].to_string(),
                        family_code_str: parts[1].to_string(),
                    })
                } else {
                    log::warn!("Unexpected family code format: {entry}");
                    None
                }
            }).collect()
        };
        shared.reference.set_family_codes(codes);

        // Order permissions: "SECTYPE:ORDTYPE,ORDTYPE;SECTYPE:..." from logon tag
        // 6652. A security type named with no order types after it is still
        // permitted, so an absent list is an empty one, never a missing key.
        let perms = parse_order_permissions(&self.raw_order_permissions);
        if !perms.is_empty() {
            let mut named: Vec<&str> = perms.keys().map(String::as_str).collect();
            named.sort_unstable();
            log::info!("Order permissions: {}", named.join(" "));
        }
        shared.reference.set_order_permissions(perms);

        // Enabled features: a plain comma-separated token list, logon tag 6542.
        shared.reference.set_enabled_features(
            self.raw_enabled_features.split(',').filter(|t| !t.is_empty()).map(str::to_string).collect(),
        );

        // White branding ID (empty for standard accounts).
        shared.reference.set_white_branding_id(self.white_branding_id.clone());

        // Webapp-REST-facing fields from the FIX logon roundtrip.
        shared.reference.set_ccp_session_id(self.server_session_id.clone());
        shared.reference.set_misc_urls(self.misc_urls.clone());
        shared.reference.set_competing_session(self.competing.as_ref().map(|other| {
            (other.ip.clone(), other.since.clone(), other.read_only)
        }));
    }

    /// Create the control channel and build a HotLoop with connected sockets.
    pub fn into_hot_loop(
        self,
        shared: Arc<SharedState>,
        event_tx: Option<SyncSender<Event>>,
        farm_conn: Connection,
        ccp_conn: Connection,
        hmds_conn: Option<Connection>,
        secdef_conn: Option<Connection>,
        core_id: Option<usize>,
        caller: CallerAuth,
    ) -> (HotLoop, SyncSender<ControlCommand>) {
        self.into_hot_loop_with_farms(
            shared, event_tx, farm_conn, ccp_conn, hmds_conn, secdef_conn, core_id, caller,
        )
    }

    /// Create the control channel and build a HotLoop with farm connections.
    /// The credentials and session a reconnect needs, from this gateway plus
    /// what the caller supplied.
    ///
    /// Shared with the test harness so a compatibility run recovers a dropped
    /// transport the same way a real client does, instead of failing the phase
    /// that happened to be running.
    pub fn reconnect_auth(&self, caller: CallerAuth) -> ReconnectAuth {
        // Where this session actually ended up, not where it first knocked.
        // The venue names the server an account belongs on and the session
        // follows it, so reconnecting to the address the caller configured
        // starts again at a door that only redirects — and on a session that
        // was redirected, that is every reconnect.
        let on = self.auth_hosts.first().cloned().unwrap_or_else(|| caller.host.clone());
        ReconnectAuth {
            logged_in_at: self.logged_in_at.clone(),
            settings: caller.settings,
            alternate_hosts: alternates_to(&self.auth_hosts, &on),
            host: on,
            username: caller.username,
            password: caller.password,
            paper: caller.paper,
            code_provider: caller.code_provider,
            ib_key_timeout_secs: caller.ib_key_timeout_secs,
            ib_key_token_sub_type: caller.ib_key_token_sub_type,
            session_key: self.session_token.clone(),
            session_token: self.session_token.clone(),
            server_session_id: self.server_session_id.clone(),
            hw_info: self.hw_info.clone(),
            encoded: self.encoded.clone(),
            secdef_host: self.secdef_host.clone(),
            secdef_farm: self.secdef_farm.clone(),
            hmds_host: self.hmds_host.clone(),
            hmds_farm: self.hmds_farm.clone(),
            trading_host: self.trading_host.clone(),
            trading_farm: self.trading_farm.clone(),
            trading_port: self.trading_port,
            hmds_port: self.hmds_port,
            secdef_port: self.secdef_port,
        }
    }

    /// Hand the open connections to the loop that will run them.
    pub fn into_hot_loop_with_farms(
        self,
        shared: Arc<SharedState>,
        event_tx: Option<SyncSender<Event>>,
        farm_conn: Connection,
        ccp_conn: Connection,
        hmds_conn: Option<Connection>,
        secdef_conn: Option<Connection>,
        core_id: Option<usize>,
        caller: CallerAuth,
    ) -> (HotLoop, SyncSender<ControlCommand>) {
        let (tx, rx) = sync_channel(64);
        let reconnect_auth = self.reconnect_auth(caller);
        if let Some(tx) = event_tx.as_ref() {
            let _ = tx.send(Event::GatewayLogon {
                ccp_session_id: self.server_session_id.clone(),
                misc_urls: self.misc_urls.clone(),
            });
        }
        let mut hot_loop = HotLoop::new(shared, event_tx, core_id);
        hot_loop.set_control_rx(rx);
        hot_loop.set_account_id(self.account_id.clone());
        hot_loop.set_reconnect_auth(reconnect_auth);
        // Held to the interval the venue named in its answer to the logon, not
        // to the one this client proposed.
        hot_loop.set_ccp_heartbeat_interval(self.heartbeat_interval);
        hot_loop.farm_conn = Some(farm_conn);
        hot_loop.ccp_conn = Some(ccp_conn);
        hot_loop.ccp.ccp_sign_key = self.ccp_sign_key.clone();
        hot_loop.ccp.ccp_sign_iv = std::sync::Mutex::new(self.ccp_sign_iv.clone());
        hot_loop.hmds_conn = hmds_conn;
        hot_loop.secdef_conn = secdef_conn;
        (hot_loop, tx)
    }
}

/// Build market data subscription request.
pub fn build_mktdata_subscribe(
    con_id: u32,
    exchange: &str,
    sec_type: &str,
    md_req_id: &str,
    seq: u32,
) -> Vec<u8> {
    let con_id_str = con_id.to_string();
    let exchange_fix = match exchange {
        "SMART" => "BEST",
        e => e,
    };
    fix_build(
        &[
            (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
            (262, md_req_id),
            (263, "1"), // Subscribe
            (146, "1"), // NumRelatedSym
            (6008, &con_id_str),
            (207, exchange_fix),
            (167, sec_type),
            (264, "442"), // BidAsk
            (9830, "1"),
        ],
        seq,
    )
}

/// Build market data unsubscribe request.
pub fn build_mktdata_unsubscribe(md_req_id: &str, seq: u32) -> Vec<u8> {
    fix_build(
        &[
            (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
            (262, md_req_id),
            (263, "2"), // Unsubscribe
        ],
        seq,
    )
}

/// Format timestamp as YYYYMMDD-HH:MM:SS (no chrono dependency).
/// Re-exports for backward compatibility.
pub use crate::config::{chrono_free_timestamp, days_to_ymd};

#[cfg(test)]
mod tests;
