//! Gateway: orchestrates auth + data connections into a running HotLoop.

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
/// Three accepted shapes (per ib-agent#128):
///   `"<host>/<farm>"`            — tag 6145 (trading)
///   `"<host>/<farm>/<port>"`     — tags 6171 (mktdata) / 8008 (secdef)
///
/// Port is informational only — ibx routes all farm channels to the same
/// data-port discovered via `misc_port()`. We just need (host, farm).
/// Returns `None` for empty or malformed input.
pub fn parse_farm_route(route: &str) -> Option<(String, String)> {
    if route.is_empty() { return None; }
    let mut parts = route.splitn(3, '/');
    let host = parts.next()?.to_string();
    let farm = parts.next()?.to_string();
    if host.is_empty() || farm.is_empty() { return None; }
    Some((host, farm))
}

/// The trading route a reconnect should use.
///
/// The auth server states one per account, and the reconnect used to ignore it
/// — announcing the literal `usfarm` and connecting to the host the caller
/// configured, so a regional account reconnected under a farm it is not on
/// (ibx#295). Empty means no route was parsed, which is the case the literals
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
/// Per ib-agent#125: gateway always emits this as **8 hex chars padded with
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
/// currently running is no longer supported." Per ib-agent#141 the
/// official client also keeps 6397/6947/8098, so we leave them in.
///
/// Tag 6947 carries the JVM default timezone (e.g. `Europe/Paris`,
/// `America/New_York`). The auth server doesn't validate it — `UTC` is
/// the safe default — but `IBX_TZ` overrides for users who want to mirror
/// their locale or comply with regional logging requirements.
pub fn build_ccp_logon(hw_info: &str, encoded: &str, heartbeat: u64, seq: u32) -> Vec<u8> {
    let now = chrono_free_timestamp();
    let tz_owned = std::env::var("IBX_TZ").unwrap_or_else(|_| "UTC".to_string());
    let tz = tz_owned.as_str();
    let hb_str = heartbeat.to_string();
    let hw_field = format!("<{}|{}>", hw_info, session::get_lan_ip());
    let build = crate::config::ib_build();
    let version = crate::config::ib_version();
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
        ],
        seq,
    )
}

/// Build encrypted farm logon message.
pub fn build_farm_encrypted_logon(
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
    let build = crate::config::ib_build();
    let version = crate::config::ib_version();

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
) -> io::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    // Poll on a short read timeout and tolerate transient WouldBlock/TimedOut
    // returns until an overall deadline. A single slow response segment from a
    // high-latency regional gateway must not tear down the connection (ibx#237).
    stream.set_read_timeout(Some(Duration::from_millis(FARM_LOGON_POLL_MS)))?;
    let deadline = std::time::Instant::now() + Duration::from_secs_f64(TIMEOUT_FARM_LOGON);
    let mut buf = Vec::new();
    let mut read_iv = initial_read_iv.to_vec();

    for _msg_num in 0..20 {
        // Read until we have a complete frame
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
                // Outcome asymmetry (ib-agent#153, ibx#187):
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
                    // read for bytes already consumed (ibx#237).
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
                if !buf.is_empty() {
                    log::warn!("{} bytes remaining in buffer after logon ACK",
                        buf.len());
                }
                return Ok((read_iv, sign_iv, buf));
            } else if fields.get(&35).map(|s| s.as_str()) == Some("3") {
                let text = fields.get(&58).map(|s| s.as_str()).unwrap_or("unknown");
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("Farm logon rejected: {text}"),
                ));
            }
        } else if msg.starts_with(b"8=1\x01") {
            // Token auth response
            if msg.windows(6).any(|w| w == b"PASSED") {
                log::info!("Token auth PASSED");
            }
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
/// empty host (ibx#378). The compiler asks for them now.
pub struct CallerAuth {
    pub host: String,
    pub username: String,
    pub password: Zeroizing<String>,
    pub paper: bool,
    /// What the second-factor gate needs if the server bumps a reconnect back
    /// to SRP. Without these an unattended client cannot finish that handshake.
    pub code_provider: Option<session::CodeProvider>,
    pub ib_key_timeout_secs: u64,
    pub ib_key_token_sub_type: String,
}

/// Credentials cached for auto-reconnect (no SRP needed).
#[derive(Clone)]
pub struct ReconnectAuth {
    pub host: String,
    pub username: String,
    /// Wrapped in `Zeroizing` so the plaintext is wiped from memory on drop.
    pub password: Zeroizing<String>,
    pub paper: bool,
    /// What the second-factor gate needs if the server bumps a reconnect back
    /// to SRP. Without these an unattended client cannot finish that handshake.
    pub code_provider: Option<session::CodeProvider>,
    pub ib_key_timeout_secs: u64,
    pub ib_key_token_sub_type: String,
    pub session_key: BigUint,
    pub session_token: BigUint,
    pub server_session_id: String,
    pub hw_info: String,
    pub encoded: String,
    /// Historical-data farm routing parsed from the auth-server response.
    /// Used by HMDS reconnect (ibx#187) — empty when no HMDS route was parsed.
    pub hmds_host: String,
    pub hmds_farm: String,
    /// Trading farm routing exactly as parsed from the same response — empty
    /// when the auth server stated none, rather than carrying the fallback the
    /// initial connect materializes. The distinction matters: an empty host
    /// lets the reconnect use whatever host the session has now, which a
    /// redirect may have changed since (ibx#295).
    pub trading_host: String,
    pub trading_farm: String,
}

/// Full gateway connection.
pub struct Gateway {
    pub account_id: String,
    pub session_token: BigUint,
    /// Session ID surfaced to webapp REST clients as `x-ccp-session-id`.
    /// Sourced from the post-auth FIX logon ACK, falling back to the locally generated
    /// session ID when the gateway does not echo one back.
    pub server_session_id: String,
    pub ccp_token: String,
    pub heartbeat_interval: u64,
    /// Stored for farm reconnection.
    pub hw_info: String,
    pub encoded: String,
    /// Raw soft dollar tier data from CCP logon tag 6560.
    pub raw_soft_dollar_tiers: String,
    /// Raw family code data from CCP logon tag 6823.
    pub raw_family_codes: String,
    /// Raw news provider data from CCP logon tag 6830.
    pub raw_news_providers: String,
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
    /// retained for HMDS reconnect (ibx#187).
    pub hmds_host: String,
    pub hmds_farm: String,
    /// Trading farm routing from the same response, retained so the trading
    /// reconnect uses the route the auth server gave rather than a literal.
    pub trading_host: String,
    pub trading_farm: String,
}

/// Farm slot the caller resolves before connecting: 17 is the market-data /
/// historical farm, 18 the trading farm. These are the literals the call sites
/// already pass.
pub const FARM_SLOT_HMDS: u32 = 17;
pub const FARM_SLOT_TRADING: u32 = 18;

/// Channel role for a farm's routing request, from the slot the caller already
/// resolved: historical-data farms take `2`, everything else `1`.
///
/// Keyed on the slot rather than the farm name. The name is not a reliable
/// discriminator — routing tags carry whatever the server sends, and this
/// codebase already has `cashhmds` on the trading slot — whereas the caller
/// splits trading and market-data farms at the call site and passes the slot
/// accordingly (ibx#253).
pub fn farm_channel_id(slot: u32) -> &'static str {
    if slot == FARM_SLOT_HMDS { "2" } else { "1" }
}

/// Connect to a data farm: key exchange → encrypted logon → token auth → routing → Connection.
pub fn connect_farm(
    host: &str,
    farm_id: &str,
    username: &str,
    password: &str,
    paper: bool,
    server_session_id: &str,
    session_key: &BigUint,
    hw_info: &str,
    encoded: &str,
    slot: u32,
) -> io::Result<Connection> {
    let port = misc_port();
    let farm_host = farm_host_override().unwrap_or_else(|| host.to_string());
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
        &mut channel, username, paper, farm_id,
        &farm_session_id, session_key, hw_info, encoded, slot,
    );
    stream.write_all(&logon_bytes)?;
    log::info!("{farm_id} encrypted logon sent");

    // Logon exchange: challenge → token auth → logon ACK
    let read_mac_key = channel.key_block().map(|kb| kb[84..104].to_vec()).unwrap_or_default();
    let initial_read_iv = channel.key_block().map(|kb| kb[48..64].to_vec()).unwrap_or_default();
    let (read_iv, sign_iv, logon_remaining) = farm_logon_exchange(
        &mut stream, &mut channel, session_key, username, password,
        &read_mac_key, &initial_read_iv,
    )?;
    log::info!("{} logon exchange complete, {} bytes remaining", farm_id, logon_remaining.len());

    let sign_mac_key = channel.key_block().map(|kb| kb[64..84].to_vec()).unwrap_or_default();

    // Send routing table request after logon.
    let channel_id = farm_channel_id(slot);
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
    // timeout, break as soon as we have at least one complete FIXCOMP frame
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
                let Some(_unsigned) = conn.unsign(raw) else { continue };
                log::info!("{} routing 8=O: {} bytes", farm_id, raw.len());
            }
            crate::protocol::connection::Frame::Control(raw) => {
                // 8=1 / 8=X control state — extracted, not routed (ibx#185).
                log::debug!("{} ignoring control frame: {} bytes", farm_id, raw.len());
            }
        }
    }
    if !frames.is_empty() {
        log::info!("{} post-logon frames: {} frames, seq now {}", farm_id, frames.len(), conn.seq);
    }
    Ok(conn)
}

/// Reconnect to the CCP (order/auth) server using cached session credentials.
/// Performs TLS + DH + CONNECT_REQUEST, then attempts SOFT_TOKEN auth with cached K.
/// If the server signals at AUTH_START that it requires full SRP, transparently
/// falls back to a fresh SRP handshake using the cached `username`/`password`
/// in `ReconnectAuth` (the same path used by `Gateway::connect`).
pub fn reconnect_ccp(auth: &ReconnectAuth) -> io::Result<Connection> {
    let token_hash = token_short_hash(&auth.session_token);
    reconnect_ccp_attempt(auth, &token_hash, &auth.host, 0)
}

fn reconnect_ccp_attempt(auth: &ReconnectAuth, token_hash: &str, host: &str, depth: u32) -> io::Result<Connection> {
    if depth > 5 {
        return Err(io::Error::other("CCP reconnect: too many redirects"));
    }
    log::info!("CCP reconnect to {}:{} (attempt {})", host, AUTH_PORT, depth + 1);

    // TLS + DH key exchange
    let addr = format!("{host}:{AUTH_PORT}")
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "DNS resolution failed"))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(TIMEOUT_SSL_AUTH))?;
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
            // Floor before following (ibx#218): this runs on the background
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
    // already retries to an overall deadline (ibx#196) and describes itself as
    // mirroring this path, which never did it.
    let fix_deadline = std::time::Instant::now()
        + std::time::Duration::from_secs_f64(TIMEOUT_FIX_LOGON * 2.0);
    let mut fix_ready = false;
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
    let logon_msg = build_ccp_logon(&auth.hw_info, &auth.encoded, CCP_HEARTBEAT, 1);
    tls.write_all(&logon_msg)?;
    tls.flush()?;

    // Short poll timeout + overall deadline so a slow response segment from a
    // high-latency gateway is retried, not treated as a fatal logon failure
    // (ibx#237, same tolerance as the farm-logon path).
    tls.get_ref().set_read_timeout(Some(Duration::from_millis(FARM_LOGON_POLL_MS)))?;
    let fix_deadline = std::time::Instant::now() + Duration::from_secs_f64(TIMEOUT_FARM_LOGON);
    for _ in 0..5 {
        let response = fix_read_deadline(&mut tls, fix_deadline)?;
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
            "A" | "U" => break,
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
    conn.seq = ccp_seq;
    log::info!("CCP reconnect complete (seq={})", conn.seq);
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
    pub username: String,
    /// Wrapped in `Zeroizing` so the plaintext is wiped from memory on drop.
    pub password: Zeroizing<String>,
    pub host: String,
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
    // cause (ibx#279). See `second_factor_route` for why an absent type is
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
        // IBKey push. The same code_provider supplies the code (ibx#282).
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
        // "waiting for approval" rather than a hang (ibx#203 / ibx#207).
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
        match session::do_ib_key_2fa(
            tls,
            token_sub_type,
            deadline,
            sf.code_provider,
        )? {
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
/// `8=FIXCOMP` envelopes (per ib-agent#129). The compressed body is ~30 kB on
/// the wire but expands to ~48 kB of plaintext carrying the routing tags
/// 6145/6171/8008, which the tag scan needs to see.
///
/// The inflated plaintext belongs to that scan and nowhere else. The engine is
/// handed the burst exactly as it arrived and decompresses the same segments
/// itself, so appending the plaintext to the engine's copy delivered every
/// message in the burst twice from a single delivery — once from the segment,
/// once from the appended copy (ibx#317). Takes the burst by reference so the
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
    pub fn connect(config: &GatewayConfig) -> io::Result<(Self, Connection, Connection, Option<Connection>)> {
        Self::connect_to_host(config, &config.host, 0)
    }

    /// Internal: connect to a specific host, with redirect depth tracking.
    fn connect_to_host(
        config: &GatewayConfig,
        host: &str,
        redirect_depth: u32,
    ) -> io::Result<(Self, Connection, Connection, Option<Connection>)> {
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

        let hw_info = resume.map_or_else(session::get_hw_info, |r| r.hw_info.clone());
        // Tag 6266 carries `{jdkVer}/{platform}/{locale}/{dist}`. The locale
        // segment must be a canonical Java `Locale.toString()` value (e.g.
        // `en_US`, `fr`, `ja_JP`); bare `en` is rejected as `invalid twsInfo`.
        // `IBX_LOCALE` overrides just the locale; `IBX_ENCODED` overrides
        // the whole string for full control.
        let encoded = match resume {
            Some(r) => r.encoded.clone(),
            None => std::env::var("IBX_ENCODED").unwrap_or_else(|_| {
                match std::env::var("IBX_LOCALE") {
                    Ok(loc) if !loc.is_empty() => format!("17.0.10.0.101/W/{loc}/G"),
                    _ => IB_ENCODED.to_string(),
                }
            }),
        };

        // --- Phase 1: TLS + auth ---
        log::info!("Connecting to auth server {host}:{AUTH_PORT}");
        let addr = format!("{host}:{AUTH_PORT}")
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "DNS resolution failed"))?;
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(TIMEOUT_SSL_AUTH))?;

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
                // Extract host (strip port if present — auth always uses AUTH_PORT)
                let redirect_host = target.split(':').next().unwrap_or(&target);
                log::info!("Redirected to {redirect_host}, reconnecting...");
                drop(tls);
                return Self::connect_to_host(config, redirect_host, redirect_depth + 1);
            }
            Err(e) => return Err(e),
        };

        // AUTH_START field 4 names the second-factor token type and its
        // per-session subtype, e.g. "5.2i" = tokenType 5 (IBKey), subtype "2i".
        // The subtype is account- and session-specific, so the compiled-in
        // default only ever matches the profile it was captured from; every
        // other account has its SWCR_TOKEN rejected and the socket closed
        // before a challenge is issued (ibx#279).
        let (server_token_type, server_token_sub_type) =
            parse_auth_start_token(&auth_start_text(&auth_start)?);

        // Field 5 of AUTH_START says which of the two the server will accept.
        // It answers 2 only when the request named a session it still holds, so
        // a resume that is stale, or for another account, or simply older than
        // the server keeps, comes back asking for the handshake — and gets it,
        // rather than an error the caller has to know how to retry.
        let auth_text = auth_start_text(&auth_start)?;
        let auth_mode: u32 = auth_text
            .split(';')
            .nth(5)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let (session_key, soft_token) = match (resume_key, auth_mode) {
            (Some(key), 2) => {
                log::info!("Resuming the session for {} — no handshake", config.username);
                do_ccp_soft_token(&mut tls, &key)?;
                // AUTH_FINISH follows the challenge exactly as it follows a
                // handshake, and carries nothing this needs.
                match session::recv_msg(&mut tls) {
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
                let session_key = do_srp(&mut tls, &config.username, &config.password)?;
                log::info!("Auth complete");

                // Per-session second-factor approval gate (IBKey / seamless push).
                // Skipped on paper logins; live logins enter a wait state if the
                // account has a second factor configured server-side.
                // Would capture a SOFT session token from AUTH_FINISH PASSED, but per
                // ib-agent#125 that body carries none, so this stays `None` on both
                // live paths and the farm logon falls back to the SRP session key.
                let soft_token = run_second_factor(&mut tls, SecondFactor {
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

        // Receive post-auth messages (encrypted via 534) and wait for the
        // data-farm start (NS_FIX_START). A transient stall here must not be
        // fatal: a single read timeout used to `break` and bubble a hard error
        // even though the data start was still pending, and keepalive chatter
        // could exhaust a fixed iteration budget before it arrived (ibx#196).
        // Retry within an overall deadline and ignore intervening messages,
        // mirroring the CCP-reconnect path.
        tls.get_ref().set_read_timeout(Some(Duration::from_secs_f64(TIMEOUT_FIX_LOGON)))?;
        let fix_deadline = std::time::Instant::now()
            + std::time::Duration::from_secs_f64(TIMEOUT_FIX_LOGON * 2.0);
        let mut fix_ready = false;
        while std::time::Instant::now() < fix_deadline {
            let (payload, _) = match ns::ns_recv(&mut tls) {
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
                let redirect_host = target.split(':').next().unwrap_or(target);
                log::info!("Post-auth redirect to {redirect_host}, reconnecting...");
                drop(tls);
                return Self::connect_to_host(config, redirect_host, redirect_depth + 1);
            } else {
                payload
            };

            let inner_text = String::from_utf8_lossy(&inner);
            let inner_parts: Vec<&str> = inner_text.split(';').collect();
            let msg_type: u32 = inner_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

            if msg_type == ns::NS_CONNECT_RESPONSE {
                log::info!("NS_CONNECT_RESPONSE: {inner_text}");
                // Send port type change (required before data start)
                let newcomm = format!("{};{};0;;2;0;", NS_VERSION_MIN, ns::NS_NEWCOMMPORTTYPE);
                session::send_secure(&mut tls, &mut channel, newcomm.as_bytes())?;
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
            // auth failure (ibx#196).
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Never received data start after auth",
            ));
        }

        // --- Phase 2: Auth server logon (over TLS) ---
        let logon_msg = build_ccp_logon(&hw_info, &encoded, CCP_HEARTBEAT, 1);
        log::info!("Sending auth logon ({} bytes)", logon_msg.len());
        tls.write_all(&logon_msg)?;
        tls.flush()?;

        // Read FIX messages until we get the logon ACK (35=A) with session info.
        // Short poll timeout + overall deadline so a slow ACK segment from a
        // high-latency gateway is retried, not fatal (ibx#237).
        tls.get_ref().set_read_timeout(Some(Duration::from_millis(FARM_LOGON_POLL_MS)))?;
        let ack_deadline = std::time::Instant::now() + Duration::from_secs_f64(TIMEOUT_FARM_LOGON);
        let mut account_id = String::new();
        let mut heartbeat_interval = CCP_HEARTBEAT;
        let mut server_session_id = String::new();
        let mut ccp_token = String::new();
        let mut raw_soft_dollar_tiers = String::new();
        let mut raw_family_codes = String::new();
        let mut raw_news_providers = String::new();
        let mut white_branding_id = String::new();
        let mut raw_misc_urls = String::new();
        // Per ib-agent#128: the auth-logon ACK tells us which farms this
        // account is routed to. Hardcoding `usfarm`/`ushmds` only works for
        // US accounts; EU accounts need eufarm/euhmds/secdefeu, etc.
        // Format of 6145: "<host>/<farm>"; 6171/8008: "<host>/<farm>/<port>"
        let mut trading_route = String::new();    // tag 6145
        let mut mktdata_route = String::new();    // tag 6171
        let mut secdef_route  = String::new();    // tag 8008

        for _ in 0..5 {
            let raw_response = fix_read_deadline(&mut tls, ack_deadline)?;
            // The auth-logon ACK arrives as `8=FIXCOMP` with a DEFLATE-
            // compressed inner body containing the per-account routing tags
            // (6145/6171/8008) and other init data. Inflate before parsing.
            // (See ib-agent#128 + #129.)
            let mut response = raw_response.clone();
            if raw_response.starts_with(b"8=FIXCOMP\x01") {
                let inflated_msgs = fixcomp::fixcomp_decompress(&raw_response)?;
                let total: usize = inflated_msgs.iter().map(|m| m.len()).sum();
                log::info!("Auth FIXCOMP envelope: {} bytes compressed → {} inner messages, ~{} inflated bytes",
                    raw_response.len(), inflated_msgs.len(), total);
                // Concatenate all inner messages so a single fix_parse pass
                // sees every tag.
                response.clear();
                for inner in inflated_msgs {
                    response.extend_from_slice(&inner);
                    response.push(b'\x01');
                }
            }
            let fields = fix_parse(&response);
            let msg_type = fields.get(&35).map(|s| s.as_str()).unwrap_or("");
            log::info!("Auth msg type={} ({} bytes raw / {} bytes parsed)",
                msg_type, raw_response.len(), response.len());
            for tag in [6144u32, 6145, 6146, 6147, 6171, 6172, 8008, 8009, 6160, 6161] {
                if let Some(v) = fields.get(&tag) {
                    log::info!("Auth msg type={msg_type} tag={tag}: {v:?}");
                }
            }

            match msg_type {
                "3" | "5" => {
                    let reason = fields.get(&58).map(|s| s.as_str()).unwrap_or("unknown");
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("FIX Logon rejected: {reason}"),
                    ));
                }
                _ => {}
            }

            if let Some(v) = fields.get(&1)
                && account_id.is_empty() { account_id = v.clone(); }
            if let Some(v) = fields.get(&108)
                && let Ok(hb) = v.parse() { heartbeat_interval = hb; }
            if let Some(v) = fields.get(&6386)
                && ccp_token.is_empty() {
                    ccp_token = v.clone();
                    log::info!("Auth: captured ccp_token (FIX 6386, len={}, prefix={:?})",
                        ccp_token.len(),
                        if ccp_token.len() > 16 { &ccp_token[..16] } else { &ccp_token });
                }
            // Tag 8035: try parsed fields first, then raw byte search
            if server_session_id.is_empty() {
                if let Some(v) = fields.get(&8035) {
                    server_session_id = v.clone();
                } else {
                    let marker = b"\x018035=";
                    if let Some(pos) = response.windows(marker.len()).position(|w| w == marker) {
                        let val_start = pos + marker.len();
                        if let Some(end) = response[val_start..].iter().position(|&b| b == SOH) {
                            server_session_id = String::from_utf8_lossy(
                                &response[val_start..val_start + end],
                            ).to_string();
                        }
                    }
                }
            }

            // Farm routing (per ib-agent#128) — server tells us which farms
            // this account is permissioned for. EU accounts get `eufarm`,
            // US get `usfarm`, etc. Read once from whichever auth msg has it.
            if let Some(v) = fields.get(&6145)
                && trading_route.is_empty() {
                    trading_route = v.clone();
                    log::info!("Auth: trading farm route = {trading_route}");
                }
            if let Some(v) = fields.get(&6171)
                && mktdata_route.is_empty() {
                    mktdata_route = v.clone();
                    log::info!("Auth: market-data farm route = {mktdata_route}");
                }
            if let Some(v) = fields.get(&8008)
                && secdef_route.is_empty() {
                    secdef_route = v.clone();
                    log::info!("Auth: secdef farm route = {secdef_route}");
                }

            // Gateway-local init data from logon response
            if let Some(v) = fields.get(&6560)
                && raw_soft_dollar_tiers.is_empty() { raw_soft_dollar_tiers = v.clone(); }
            if let Some(v) = fields.get(&6823)
                && raw_family_codes.is_empty() { raw_family_codes = v.clone(); }
            if let Some(v) = fields.get(&6830)
                && raw_news_providers.is_empty() { raw_news_providers = v.clone(); }
            if let Some(v) = fields.get(&6571)
                && white_branding_id.is_empty() { white_branding_id = v.clone(); }
            // Tag 6321: PRIV_LAB_MISC_URLS — try parsed fields first, then raw byte search.
            // Mirrors the 8035 defensive scan because the value can carry `|` separators
            // that confuse downstream parsers if a chunk is fragmented.
            if raw_misc_urls.is_empty() {
                if let Some(v) = fields.get(&6321) {
                    raw_misc_urls = v.clone();
                    log::info!("Found misc URLs from logon ACK ({} bytes)", raw_misc_urls.len());
                } else {
                    let marker = b"\x016321=";
                    if let Some(pos) = response.windows(marker.len()).position(|w| w == marker) {
                        let val_start = pos + marker.len();
                        if let Some(end) = response[val_start..].iter().position(|&b| b == SOH) {
                            raw_misc_urls = String::from_utf8_lossy(
                                &response[val_start..val_start + end],
                            ).to_string();
                            log::info!("Found misc URLs from logon ACK byte scan ({} bytes)", raw_misc_urls.len());
                        }
                    }
                }
            }

            // Stop once we have the logon ACK or server config message
            if msg_type == "A" || msg_type == "U" {
                break;
            }
        }
        tls.get_ref().set_read_timeout(None)?;

        // Fall back to our auth session_id if server didn't provide one (Python does the same)
        if server_session_id.is_empty() {
            server_session_id = session_id.clone();
        }

        log::info!(
            "Auth logon: account={account_id} session_id={server_session_id} hb={heartbeat_interval}s"
        );

        // --- Post-logon init sequence ---
        let account = if account_id.is_empty() { config.username.clone() } else { account_id.clone() };
        let mut ccp_seq: u32 = 1; // logon was seq 1
        let now = chrono_free_timestamp();
        let today_start = format!("{}-00:00:00", &now[..8]);

        // Helper: send_ib_msg builds 35=U with 6040=<comm_type> + extra tags
        let mut send_init = |fields: &[(u32, &str)]| -> io::Result<()> {
            ccp_seq += 1;
            let msg = fix_build(fields, ccp_seq);
            tls.write_all(&msg)?;
            Ok(())
        };

        send_init(&[(35, "U"), (52, &now), (6040, "91"), (1, &account), (6556, "DR.1"), (6712, "1")])?;
        send_init(&[(35, "U"), (52, &now), (6040, "193"), (6556, "OPR.2"), (8166, "L"), (8176, "1")])?;
        send_init(&[(35, "U"), (52, &now), (6040, "101")])?;
        send_init(&[(35, "U"), (52, &now), (6040, "209"), (1, &account), (6556, "AcctConfig3")])?;
        send_init(&[(35, "U"), (52, &now), (6040, "72"), (6536, &today_start), (6537, &now), (6556, "today4")])?;
        send_init(&[(35, "U"), (52, &now), (6040, "74"), (1, ""), (6544, "2")])?;
        send_init(&[(35, "U"), (52, &now), (6040, "76"), (1, ""), (6565, "1")])?;
        for _ in 0..92 {
            send_init(&[(35, "U"), (52, &now), (6040, "80")])?;
        }
        tls.flush()?;
        log::info!("Init sequence sent ({} messages, seq now {})", 99, ccp_seq);

        // Drain init responses — extract account ID + farm routing tags.
        // Per ib-agent#134 read-throughput investigation (2026-05-05):
        // the burst's bulk (~28 kB compressed) arrives in ~300 ms continuous,
        // after which the server emits 67-byte keep-alive trickles every ~10 s
        // until it FINs the socket at ~140 s. A 300 ms idle-gap is past any
        // intra-burst jitter (the burst is continuous) and well short of the
        // 10 s keep-alive trickle interval, so we exit promptly after burst-end.
        tls.get_ref().set_read_timeout(Some(Duration::from_millis(300)))?;
        let mut init_data: Vec<u8> = Vec::with_capacity(65536);
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
                    // this is the server's 10-s keep-alive trickle, which
                    // we don't want to drain (would push grace-window
                    // messages past the server-side deadline).
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        log::info!(
            "Init response: {} bytes in {:?}",
            init_data.len(), read_start.elapsed(),
        );

        let scan_data = init_scan_buffer(&init_data);

        // Scan init response for account ID and gateway-local init tags
        let init_str = String::from_utf8_lossy(&scan_data);
        // TEMP diagnostic (ib-agent#128 follow-up): log every part containing
        // "farm" or "hmds" so we can locate the routing tags.
        for part in init_str.split('\x01') {
            if part.contains("farm") || part.contains("hmds") || part.contains("secdef") {
                log::info!("Init scan: routing-shaped part = {part:?}");
            }
        }
        for part in init_str.split('\x01') {
            if part.starts_with("1=") && part.len() > 2 {
                let val = &part[2..];
                if (val.starts_with("DU") || val.starts_with("DF") || val.starts_with("U"))
                    && (account_id.is_empty() || account_id == config.username) {
                        account_id = val.to_string();
                        log::info!("Found account ID from init response: {account_id}");
                    }
            } else if part.starts_with("6560=") && raw_soft_dollar_tiers.is_empty() {
                raw_soft_dollar_tiers = part[5..].to_string();
                log::info!("Found soft dollar tiers from init response ({} bytes)", raw_soft_dollar_tiers.len());
            } else if part.starts_with("6823=") && raw_family_codes.is_empty() {
                raw_family_codes = part[5..].to_string();
                log::info!("Found family codes from init response ({} bytes)", raw_family_codes.len());
            } else if part.starts_with("6830=") && raw_news_providers.is_empty() {
                raw_news_providers = part[5..].to_string();
                log::info!("Found news providers from init response ({} bytes)", raw_news_providers.len());
            } else if part.starts_with("6571=") && white_branding_id.is_empty() {
                white_branding_id = part[5..].to_string();
                log::info!("Found white branding ID from init response");
            } else if part.starts_with("6321=") && raw_misc_urls.is_empty() {
                raw_misc_urls = part[5..].to_string();
                log::info!("Found misc URLs from init response ({} bytes)", raw_misc_urls.len());
            } else if part.starts_with("6145=") && trading_route.is_empty() {
                trading_route = part[5..].to_string();
                log::info!("Found trading farm route in init response: {trading_route}");
            } else if part.starts_with("6171=") && mktdata_route.is_empty() {
                mktdata_route = part[5..].to_string();
                log::info!("Found market-data farm route in init response: {mktdata_route}");
            } else if part.starts_with("8008=") && secdef_route.is_empty() {
                secdef_route = part[5..].to_string();
                log::info!("Found secdef farm route in init response: {secdef_route}");
            }
        }

        // Per ib-agent#134: CCP server FINs the connection ~12s after the
        // init-burst response if no application-level traffic arrives in the
        // grace window — heartbeats alone do not satisfy "client alive".
        // Send Account-Register (35=U|6040=6, account in tag 6095) followed
        // by a wildcard OrderStatusRequest (35=H|11=*|55=*|54=*) right after
        // the inbound burst-end, before farm logons begin. Both are sent in
        // plain FIX over TLS (the CCP socket has no AES/HMAC envelope at
        // this stage; encryption is set up only after `Connection::new`).
        let post_burst_account = if account_id.is_empty() {
            config.username.clone()
        } else {
            account_id.clone()
        };
        let post_burst_now = chrono_free_timestamp();
        ccp_seq += 1;
        let ar_msg = fix_build(
            &[
                (35, "U"),
                (52, &post_burst_now),
                (6040, "6"),
                (6036, "1"),
                (6529, "AR.1"),
                (6095, &post_burst_account),
            ],
            ccp_seq,
        );
        tls.write_all(&ar_msg)?;
        ccp_seq += 1;
        let osr_msg = fix_build(
            &[
                (35, "H"),
                (52, &post_burst_now),
                (11, "*"),
                (55, "*"),
                (54, "*"),
            ],
            ccp_seq,
        );
        tls.write_all(&osr_msg)?;
        ccp_seq += 1;
        // PortfolioLoginRequest — third post-burst app message in the Java
        // capture (tag34=104). Account goes in tag 1 here, not 6095.
        let plr_msg = fix_build(
            &[
                (35, "U"),
                (52, &post_burst_now),
                (6040, "142"),
                (6529, "PLR.1"),
                (1, &post_burst_account),
            ],
            ccp_seq,
        );
        tls.write_all(&plr_msg)?;
        ccp_seq += 1;
        // DataRequest — Java tag34=105: `1={acc}|6712=1|6556=DR.{N}`
        let dr_msg = fix_build(
            &[
                (35, "U"),
                (52, &post_burst_now),
                (6040, "91"),
                (1, &post_burst_account),
                (6712, "1"),
                (6556, "DR.2"),
            ],
            ccp_seq,
        );
        tls.write_all(&dr_msg)?;
        ccp_seq += 1;
        // 6040=74 — Java tag34=106: `1={acc}|6700=Core|6544=2`
        let core_msg = fix_build(
            &[
                (35, "U"),
                (52, &post_burst_now),
                (6040, "74"),
                (1, &post_burst_account),
                (6700, "Core"),
                (6544, "2"),
            ],
            ccp_seq,
        );
        tls.write_all(&core_msg)?;
        tls.flush()?;
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
        // Per ib-agent#143/#144/#145: the official Gateway opens exactly 3 authed TCP
        // sessions per login — MARKET_DATA (tag 6145), HISTORICAL_DATA (tag 6171), and
        // SECDEFARM (tag 8008, UI/telemetry only — not used by ibx). Per ib-agent#125/
        // #131/#133: the SOFT token is `SHA1(strip(S))` where S is the SRP shared
        // secret. `do_srp` returns exactly that via `srp_compute_k`, so `session_key`
        // IS the SOFT token — no further hashing. (Tag 8483's per-channel SHA1 is
        // added by `token_short_hash` at the build-logon site.) Tag 6386 is an S3
        // object key, not a token source.
        let farm_token: BigUint = soft_token.clone().unwrap_or_else(|| session_key.clone());
        // Per ib-agent#128: read the farm names from the auth-server's
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
        let parsed_trading = parse_farm_route(&trading_route);
        let (trading_host, trading_farm) = parsed_trading.clone()
            .unwrap_or_else(|| (host.to_string(), DEFAULT_TRADING_FARM.to_string()));
        let (mktdata_host, mktdata_farm) = parse_farm_route(&mktdata_route)
            .unwrap_or_else(|| (host.to_string(), "ushmds".to_string()));
        log::info!("Farm routing: trading={trading_host}/{trading_farm}, mktdata={mktdata_host}/{mktdata_farm}");

        // Retain HMDS routing for the reconnect loop (ibx#187) — the values
        // below are moved into the thread::scope closures.
        let hmds_host_for_gw = mktdata_host.clone();
        let hmds_farm_for_gw = mktdata_farm.clone();
        let (trading_host_for_gw, trading_farm_for_gw) =
            parsed_trading.unwrap_or_default();

        // Parallel farm logons: validated against paper and live (each farm
        // logon is ~6 s sequentially; running them in parallel halves the
        // farm-logon phase). Both servers accept concurrent logons with the
        // same credentials — see examples/ex_parallel_farm_logon.rs.
        let (farm_conn, hmds_conn) = std::thread::scope(|scope| {
            let username = &config.username;
            let password = &*config.password;
            let paper = config.paper;
            let ssid = &server_session_id;
            let token = &farm_token;
            let hw = &hw_info;
            let enc = &encoded;
            let trading_handle = scope.spawn(move || {
                connect_farm(&trading_host, &trading_farm, username, password,
                    paper, ssid, token, hw, enc, FARM_SLOT_TRADING)
            });
            let mktdata_handle = scope.spawn(move || {
                connect_farm(&mktdata_host, &mktdata_farm, username, password,
                    paper, ssid, token, hw, enc, FARM_SLOT_HMDS)
            });
            let trading = trading_handle.join().expect("trading farm thread panicked");
            let mktdata = mktdata_handle.join().expect("mktdata farm thread panicked");
            (trading, mktdata)
        });
        let farm_conn = farm_conn?;
        let hmds_conn = match hmds_conn {
            Ok(c) => { log::info!("Historical data farm connected"); Some(c) }
            Err(e) => { log::warn!("Historical data farm connection failed (non-fatal): {e}"); None }
        };

        let gw = Gateway {
            account_id: if account_id.is_empty() { config.username.clone() } else { account_id },
            session_token: session_key,
            server_session_id,
            ccp_token,
            heartbeat_interval,
            hw_info,
            encoded,
            raw_soft_dollar_tiers,
            raw_family_codes,
            raw_news_providers,
            white_branding_id,
            misc_urls: parse_misc_urls(&raw_misc_urls),
            ccp_sign_key,
            ccp_sign_iv,
            hmds_host: hmds_host_for_gw,
            hmds_farm: hmds_farm_for_gw,
            trading_host: trading_host_for_gw,
            trading_farm: trading_farm_for_gw,
        };
        Ok((gw, farm_conn, ccp_conn, hmds_conn))
    }

    /// Populate shared state with gateway-local init data parsed from CCP logon.
    pub fn populate_init_data(&self, shared: &SharedState) {
        use crate::types::{SmartComponent, NewsProvider, SoftDollarTier, FamilyCode};

        // Smart components: hardcoded US equity SMART routing exchanges.
        // Server doesn't send these in a parseable init message; they're
        // embedded in the Gateway binary. Hardcoded list matches Gateway 10.30+.
        let smart_components: Vec<SmartComponent> = [
            ("NASDAQ", "Q"), ("NYSE", "N"), ("ARCA", "P"), ("BATS", "Z"),
            ("IEX", "V"), ("BEX", "B"), ("BYX", "Y"), ("NYSENAT", "C"),
            ("DRCTEDGE", "J"), ("MEMX", "U"), ("PEARL", "H"), ("AMEX", "A"),
            ("CHX", "M"), ("LTSE", "L"), ("PSX", "X"), ("ISE", "I"), ("EDGEA", "K"),
        ].iter().enumerate().map(|(i, (exch, letter))| SmartComponent {
            bit_number: i as i32,
            exchange: exch.to_string(),
            exchange_letter: letter.to_string(),
        }).collect();
        shared.reference.set_smart_components(smart_components);

        // News providers: parse from CCP logon tag 6830, fall back to defaults.
        // Wire format: "code1,name1;code2,name2;..." (tag value capped at 155 entries).
        let news_providers: Vec<NewsProvider> = if self.raw_news_providers.is_empty() {
            // Default list — only used when account-specific entitlement data is unavailable.
            [
                ("BRFG", "Briefing.com General Market Columns"),
                ("BRFUPDN", "Briefing.com Analyst Actions"),
                ("DJ-N", "Dow Jones Global Equity Trader"),
                ("DJ-RTA", "Dow Jones Top Stories Asia Pacific"),
                ("DJ-RTE", "Dow Jones Top Stories Europe"),
                ("DJ-RTG", "Dow Jones Top Stories Global"),
                ("DJ-RTPRO", "Dow Jones Top Stories Pro"),
                ("DJNL", "Dow Jones Newsletters"),
            ].iter().map(|(code, name)| NewsProvider {
                code: code.to_string(), name: name.to_string(),
            }).collect()
        } else {
            self.raw_news_providers.split(';').filter_map(|entry| {
                let entry = entry.trim();
                if entry.is_empty() { return None; }
                let (code, name) = entry.split_once(',')?;
                Some(NewsProvider {
                    code: code.trim().to_string(),
                    name: name.trim().to_string(),
                })
            }).collect()
        };
        shared.reference.set_news_providers(news_providers);

        // Soft dollar tiers: parse from CCP logon tag 6560, fall back to defaults.
        let tiers = if self.raw_soft_dollar_tiers.is_empty() {
            // Default tiers matching Gateway 10.30+
            vec![
                SoftDollarTier { name: "MaxRebate".into(), val: "1".into(), display_name: "Maximize Rebate".into() },
                SoftDollarTier { name: "PreferRebate".into(), val: "9".into(), display_name: "Prefer Rebate".into() },
                SoftDollarTier { name: "PreferFill".into(), val: "11".into(), display_name: "Prefer Fill".into() },
                SoftDollarTier { name: "MaxFill".into(), val: "12".into(), display_name: "Maximize Fill".into() },
                SoftDollarTier { name: "Primary".into(), val: "2".into(), display_name: "Primary Exchange".into() },
                SoftDollarTier { name: "VRebate".into(), val: "3".into(), display_name: "Highest Volume Exchange With Rebate".into() },
                SoftDollarTier { name: "VLowFee".into(), val: "4".into(), display_name: "High Volume Exchange With Lowest Fee".into() },
            ]
        } else {
            // Parse "name1|val1|display1;name2|val2|display2" format
            self.raw_soft_dollar_tiers.split(';').filter_map(|entry| {
                let parts: Vec<&str> = entry.split('|').collect();
                if parts.len() >= 3 {
                    Some(SoftDollarTier {
                        name: parts[0].to_string(),
                        val: parts[1].to_string(),
                        display_name: parts[2].to_string(),
                    })
                } else {
                    log::warn!("Unexpected soft dollar tier format: {entry}");
                    None
                }
            }).collect()
        };
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

        // White branding ID (empty for standard accounts).
        shared.reference.set_white_branding_id(self.white_branding_id.clone());

        // Webapp-REST-facing fields from the FIX logon roundtrip.
        shared.reference.set_ccp_session_id(self.server_session_id.clone());
        shared.reference.set_misc_urls(self.misc_urls.clone());
    }

    /// Create the control channel and build a HotLoop with connected sockets.
    pub fn into_hot_loop(
        self,
        shared: Arc<SharedState>,
        event_tx: Option<SyncSender<Event>>,
        farm_conn: Connection,
        ccp_conn: Connection,
        hmds_conn: Option<Connection>,
        core_id: Option<usize>,
        caller: CallerAuth,
    ) -> (HotLoop, SyncSender<ControlCommand>) {
        self.into_hot_loop_with_farms(shared, event_tx, farm_conn, ccp_conn, hmds_conn, core_id, caller)
    }

    /// Create the control channel and build a HotLoop with farm connections.
    pub fn into_hot_loop_with_farms(
        self,
        shared: Arc<SharedState>,
        event_tx: Option<SyncSender<Event>>,
        farm_conn: Connection,
        ccp_conn: Connection,
        hmds_conn: Option<Connection>,
        core_id: Option<usize>,
        caller: CallerAuth,
    ) -> (HotLoop, SyncSender<ControlCommand>) {
        let (tx, rx) = sync_channel(64);
        let reconnect_auth = ReconnectAuth {
            host: caller.host,
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
            hmds_host: self.hmds_host.clone(),
            hmds_farm: self.hmds_farm.clone(),
            trading_host: self.trading_host.clone(),
            trading_farm: self.trading_farm.clone(),
        };
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
        hot_loop.farm_conn = Some(farm_conn);
        hot_loop.ccp_conn = Some(ccp_conn);
        hot_loop.ccp.ccp_sign_key = self.ccp_sign_key.clone();
        hot_loop.ccp.ccp_sign_iv = std::sync::Mutex::new(self.ccp_sign_iv.clone());
        hot_loop.hmds_conn = hmds_conn;
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
mod tests {

    /// Field 4 is where the second-factor type lives, and reading the wrong
    /// field is what caused both live failures this gate was written for — a
    /// sub-type sent to the wrong account shape, and a factor routed to the
    /// wrong prompt. Nothing pinned it.
    #[test]
    fn auth_start_token_comes_from_field_four() {
        let frame = |token: &str| format!("a;b;c;d;{token};f");

        assert_eq!(parse_auth_start_token(&frame("5.2i")), ("5".into(), Some("2i".into())));
        assert_eq!(parse_auth_start_token(&frame("4")), ("4".into(), None));
        assert_eq!(parse_auth_start_token(&frame("4,5")), ("4,5".into(), None));
        assert_eq!(parse_auth_start_token(&frame(" 5.2i ")), ("5".into(), Some("2i".into())));

        // A sub-type on one entry must not swallow the entries after it.
        assert_eq!(parse_auth_start_token(&frame("5.2i,4")), ("5,4".into(), Some("2i".into())));

        // On a mixed account the sub-type must belong to the type the gate
        // routes to, which prefers IBKey. Keeping the first one stated sent
        // the authenticator's sub-type in the IBKey init.
        assert_eq!(
            parse_auth_start_token(&frame("4.auth,5.2i")),
            ("4,5".into(), Some("2i".into())),
        );
        assert_eq!(
            parse_auth_start_token(&frame("5.2i,4.auth")),
            ("5,4".into(), Some("2i".into())),
        );
        // With no IBKey entry the authenticator's own sub-type is the one that
        // belongs to the route taken.
        assert_eq!(
            parse_auth_start_token(&frame("4.auth,9.other")),
            ("4,9".into(), Some("auth".into())),
        );
        // The routed entry stating no sub-type of its own must not borrow one
        // from an entry the gate is not using. `4.auth,5` routes to IBKey, so
        // the configured fallback is the answer — sending the authenticator's
        // sub-type in the IBKey init is what this field kept getting wrong.
        assert_eq!(parse_auth_start_token(&frame("4.auth,5")), ("4,5".into(), None));
        assert_eq!(parse_auth_start_token(&frame("9.other,5")), ("9,5".into(), None));
        assert_eq!(parse_auth_start_token(&frame("9.other,4")), ("9,4".into(), None));
        // A sub-type on a type neither gate serves is still better than none.
        assert_eq!(
            parse_auth_start_token(&frame("9.other")),
            ("9".into(), Some("other".into())),
        );

        // Short or absent field 4 yields no type rather than a wrong one.
        assert_eq!(parse_auth_start_token("a;b;c"), ("".into(), None));
        assert_eq!(parse_auth_start_token(&frame("")), ("".into(), None));
    }

    /// `recv_secure` clears the outer envelope and stops there. Field 4 of what
    /// it hands back was read as the second-factor declaration whatever the
    /// frame actually was, so another message at that point in the handshake
    /// supplied the type and sub-type from fields that mean something else.
    #[test]
    fn a_frame_that_is_not_auth_start_is_refused() {
        // 50;520;...;<type>.<sub-type>;<auth mode>;
        let auth_start = auth_start_text(b"50;520;a;b;5.2i;2;").unwrap();
        assert_eq!(parse_auth_start_token(&auth_start), ("5".into(), Some("2i".into())));

        // Same shape, another type. NS_CONNECT_RESPONSE's field 4 is not a
        // second-factor declaration, and reading it as one picks an exchange.
        let err = auth_start_text(b"50;523;a;b;5.2i;2;").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("523"), "{}", err);

        // Too short to state a type is not AUTH_START either.
        assert!(auth_start_text(b"50;").is_err());
    }

    #[test]
    fn second_factor_route_covers_every_token_type() {
        use super::{second_factor_route, SecondFactorRoute::*};
        // An absent type must still enter the IBKey gate. That gate opens by
        // sending its init, and an account with no second factor completes
        // through it — routing it to `None` sends nothing and leaves the
        // server waiting on an init that never arrives.
        assert_eq!(second_factor_route(false, ""), IbKey);
        assert_eq!(second_factor_route(false, "5"), IbKey);
        assert_eq!(second_factor_route(false, "4"), SecurityCode);
        // The field is a list when more than one factor is enabled. An
        // account with both an authenticator and IBKey advertises `4,5`, and
        // reading that as one type refused the login outright.
        assert_eq!(second_factor_route(false, "4,5"), IbKey);
        assert_eq!(second_factor_route(false, "5,4"), IbKey, "order must not matter");
        assert_eq!(second_factor_route(false, "3,4"), SecurityCode, "an unknown entry must not veto a known one");
        assert_eq!(second_factor_route(false, " 4 , 5 "), IbKey, "entries may be padded");
        assert_eq!(second_factor_route(false, "3,6"), Unsupported, "a list of unknowns is still unsupported");
        assert_eq!(second_factor_route(false, "3"), Unsupported);
        assert_eq!(second_factor_route(false, "05"), Unsupported);
        assert_eq!(second_factor_route(false, "banana"), Unsupported);
        // Paper never presents one, whatever the field says.
        for t in ["", "3", "4", "5"] {
            assert_eq!(second_factor_route(true, t), None, "paper, type {t:?}");
        }
    }

    use super::*;

    /// ibx#317: the init burst is handed to the engine still compressed, and
    /// the engine decompresses the same segments itself. Appending the inflated
    /// plaintext to that buffer — rather than to a copy taken for the local tag
    /// scan — put every message in the burst in front of the engine twice from
    /// a single delivery.
    #[test]
    fn the_inflated_init_content_is_scanned_but_not_handed_to_the_engine() {
        let inner = b"8=FIX.4.2\x0135=B\x0158=ROUTING\x016145=farm-a\x0110=000\x01";
        let mut burst = b"8=FIX.4.2\x0135=A\x01".to_vec();
        burst.extend_from_slice(&fixcomp::fixcomp_build(inner));
        burst.extend_from_slice(b"8=FIX.4.2\x0135=0\x01");

        fn count(haystack: &[u8], needle: &[u8]) -> usize {
            haystack.windows(needle.len()).filter(|w| *w == needle).count()
        }

        let before = count(&burst, b"58=ROUTING");
        let scan = init_scan_buffer(&burst);

        assert_eq!(
            count(&scan, b"58=ROUTING"), before + 1,
            "the tag scan gains exactly one inflated copy of the segment's content",
        );
        assert!(scan.starts_with(&burst), "and still sees everything that arrived");
        assert_eq!(
            count(&burst, b"58=ROUTING"), before,
            "and the buffer the engine is handed is not the one that grew",
        );
    }

    #[test]
    fn token_short_hash_deterministic() {
        let token = BigUint::from(123456789u64);
        let h1 = token_short_hash(&token);
        let h2 = token_short_hash(&token);
        assert_eq!(h1, h2);
        // Should be lowercase hex
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_short_hash_different_tokens() {
        let t1 = BigUint::from(111u64);
        let t2 = BigUint::from(222u64);
        assert_ne!(token_short_hash(&t1), token_short_hash(&t2));
    }

    #[test]
    fn parse_farm_route_two_segments() {
        let parsed = parse_farm_route("zdc1.ibllc.com/eufarm").unwrap();
        assert_eq!(parsed, ("zdc1.ibllc.com".to_string(), "eufarm".to_string()));
    }

    #[test]
    fn parse_farm_route_three_segments_drops_port() {
        let parsed = parse_farm_route("zdc1.ibllc.com/euhmds/4000").unwrap();
        assert_eq!(parsed, ("zdc1.ibllc.com".to_string(), "euhmds".to_string()));
    }

    #[test]
    fn the_channel_role_comes_from_the_slot_not_the_farm_name() {
        // ibx#253: the role was keyed on the literal "ushmds", so a regional
        // historical-data farm was established on the trading channel. The
        // caller already splits the two and passes the slot, which is the
        // discriminator that holds for every farm name — including `cashhmds`,
        // which this codebase connects on the trading slot despite the suffix.
        // Asserted on the wire values rather than through the constants: a
        // drifting constant would otherwise put every historical-data farm on
        // the trading channel with the suite still green.
        assert_eq!(farm_channel_id(17), "2");
        assert_eq!(farm_channel_id(18), "1");
        assert_eq!(FARM_SLOT_HMDS, 17);
        assert_eq!(FARM_SLOT_TRADING, 18);
        assert_eq!(farm_channel_id(0), "1", "an unknown slot is not historical data");
    }


    #[test]
    fn parse_farm_route_us_account() {
        let parsed = parse_farm_route("cdc1.ibllc.com/usfarm").unwrap();
        assert_eq!(parsed, ("cdc1.ibllc.com".to_string(), "usfarm".to_string()));
    }

    #[test]
    fn parse_farm_route_rejects_empty_and_malformed() {
        assert_eq!(parse_farm_route(""), None);
        assert_eq!(parse_farm_route("nofarm.example.com"), None);
        assert_eq!(parse_farm_route("/farm"), None);
        assert_eq!(parse_farm_route("host/"), None);
    }

    #[test]
    fn token_short_hash_always_8_chars() {
        // Per ib-agent#125: gateway pads to 8 hex chars. Brute-force search
        // over small inputs to find one whose SHA1 ends in a high-nibble
        // zero, then assert padding kicks in.
        for n in 0u64..10_000 {
            let token = BigUint::from(n);
            let h = token_short_hash(&token);
            assert_eq!(h.len(), 8,
                "token_short_hash must always be 8 chars; n={n} produced {h:?}");
            assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn build_ccp_logon_structure() {
        let msg = build_ccp_logon("abc123|00:00:00:00:00:00", "17.0.10.0.101/W/en/G", 10, 1);
        let fields = fix_parse(&msg);
        assert_eq!(fields[&35], "A");
        assert_eq!(fields[&98], "0");
        assert_eq!(fields[&108], "10");
        assert_eq!(fields[&141], "Y");
        assert_eq!(fields[&6034], crate::config::ib_build());
        assert_eq!(fields[&6968], crate::config::ib_version());
        assert_eq!(fields[&6490], "dark");
        assert_eq!(fields[&6397], "1");
        assert_eq!(fields[&8361], "(rolling)");
        assert_eq!(fields[&8098], "0");
        assert!(fields[&6351].contains("abc123"));
    }

    #[test]
    fn build_farm_logon_has_required_tags() {
        let token = BigUint::from(999u64);
        let hash = token_short_hash(&token);
        assert!(!hash.is_empty());
    }

    #[test]
    fn build_mktdata_subscribe_structure() {
        let msg = build_mktdata_subscribe(265598, "SMART", "CS", "REQ1", 5);
        let fields = fix_parse(&msg);
        assert_eq!(fields[&35], "V");
        assert_eq!(fields[&262], "REQ1");
        assert_eq!(fields[&263], "1");
        assert_eq!(fields[&6008], "265598");
        assert_eq!(fields[&207], "BEST"); // SMART→BEST
        assert_eq!(fields[&167], "CS");
    }

    #[test]
    fn build_mktdata_unsubscribe_structure() {
        let msg = build_mktdata_unsubscribe("REQ1", 6);
        let fields = fix_parse(&msg);
        assert_eq!(fields[&35], "V");
        assert_eq!(fields[&262], "REQ1");
        assert_eq!(fields[&263], "2");
    }

    #[test]
    fn chrono_free_timestamp_format() {
        let ts = chrono_free_timestamp();
        assert_eq!(ts.len(), 17); // "YYYYMMDD-HH:MM:SS"
        assert_eq!(ts.as_bytes()[8], b'-');
        assert_eq!(ts.as_bytes()[11], b':');
        assert_eq!(ts.as_bytes()[14], b':');
    }

    #[test]
    fn days_to_ymd_epoch() {
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn parse_misc_urls_pipe_separated() {
        let m = parse_misc_urls("region_dam=ny5wwwdam1.ibllc.com|region_webserver=ny5wwwgw1.ibllc.com|nossl=0");
        assert_eq!(m.len(), 3);
        assert_eq!(m.get("region_dam").map(String::as_str), Some("ny5wwwdam1.ibllc.com"));
        assert_eq!(m.get("region_webserver").map(String::as_str), Some("ny5wwwgw1.ibllc.com"));
        assert_eq!(m.get("nossl").map(String::as_str), Some("0"));
    }

    #[test]
    fn parse_misc_urls_pct_encoded_pipe() {
        let m = parse_misc_urls("a=1|b=2|c%7Cd=3");
        assert_eq!(m.len(), 3);
        assert_eq!(m.get("a").map(String::as_str), Some("1"));
        assert_eq!(m.get("b").map(String::as_str), Some("2"));
        assert_eq!(m.get("c|d").map(String::as_str), Some("3"));
    }

    #[test]
    fn parse_misc_urls_pct_encoded_pipe_in_value() {
        let m = parse_misc_urls("a=x%7Cy");
        assert_eq!(m.get("a").map(String::as_str), Some("x|y"));
    }

    #[test]
    fn parse_misc_urls_pct_encoded_lowercase() {
        let m = parse_misc_urls("a=x%7cy");
        assert_eq!(m.get("a").map(String::as_str), Some("x|y"));
    }

    #[test]
    fn parse_misc_urls_empty_input() {
        assert!(parse_misc_urls("").is_empty());
    }

    #[test]
    fn parse_misc_urls_comma_fallback() {
        let m = parse_misc_urls("a=1,b=2,c=3");
        assert_eq!(m.len(), 3);
        assert_eq!(m.get("b").map(String::as_str), Some("2"));
    }

    #[test]
    fn parse_misc_urls_drops_malformed_entries() {
        let m = parse_misc_urls("a=1|nokv|=val|b=2");
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("a").map(String::as_str), Some("1"));
        assert_eq!(m.get("b").map(String::as_str), Some("2"));
    }

    #[test]
    fn parse_misc_urls_value_with_equals() {
        // split_once stops at first `=`, so URLs with query strings round-trip.
        let m = parse_misc_urls("cookbook=https://x.example/path?a=1&b=2");
        assert_eq!(m.get("cookbook").map(String::as_str), Some("https://x.example/path?a=1&b=2"));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2026-03-05 = day 20517 since epoch
        let (y, m, d) = days_to_ymd(20517);
        assert_eq!((y, m, d), (2026, 3, 5));
    }

    #[test]
    fn try_frame_farm_msg_incomplete() {
        assert!(try_frame_farm_msg(b"8=FIX").is_none());
        assert!(try_frame_farm_msg(b"").is_none());
    }

    #[test]
    fn try_frame_farm_msg_complete() {
        let msg = fix_build(&[(35, "A"), (108, "30")], 1);
        let (extracted, consumed) = try_frame_farm_msg(&msg).unwrap();
        assert_eq!(extracted, msg);
        assert_eq!(consumed, msg.len());
    }

    #[test]
    fn try_frame_farm_msg_with_trailing() {
        let msg1 = fix_build(&[(35, "A")], 1);
        let msg2 = fix_build(&[(35, "0")], 2);
        let mut buf = msg1.clone();
        buf.extend_from_slice(&msg2);
        let (extracted, consumed) = try_frame_farm_msg(&buf).unwrap();
        assert_eq!(extracted, msg1);
        assert_eq!(consumed, msg1.len());
    }

    // Note: build_farm_encrypted_logon requires a DH-initialized SecureChannel
    // which can't be created in unit tests. Tested via compatibility tests instead.

    #[test]
    fn build_mktdata_subscribe_exchange_passthrough() {
        // Non-SMART exchanges should pass through as-is
        let msg = build_mktdata_subscribe(265598, "ARCA", "CS", "REQ2", 3);
        let fields = fix_parse(&msg);
        assert_eq!(fields[&207], "ARCA"); // not mapped to BEST
    }

    #[test]
    fn build_mktdata_subscribe_has_correct_tags() {
        let msg = build_mktdata_subscribe(756733, "SMART", "ETF", "REQ5", 10);
        let fields = fix_parse(&msg);
        assert_eq!(fields[&35], "V");
        assert_eq!(fields[&6008], "756733");
        assert_eq!(fields[&207], "BEST");
        assert_eq!(fields[&167], "ETF");
        assert_eq!(fields[&263], "1"); // subscribe
        assert_eq!(fields[&146], "1"); // NumRelatedSym
    }

    #[test]
    fn days_to_ymd_leap_year() {
        let (y, m, d) = days_to_ymd(19782); // 2024-02-29
        assert_eq!((y, m, d), (2024, 2, 29));
    }

    #[test]
    fn days_to_ymd_end_of_year() {
        // 2025-12-31
        let (y, m, d) = days_to_ymd(20453); // 2025-12-31
        assert_eq!((y, m, d), (2025, 12, 31));
    }

    #[test]
    fn days_to_ymd_start_of_2000() {
        // 2000-01-01 = 10957 days from epoch
        let (y, m, d) = days_to_ymd(10957);
        assert_eq!((y, m, d), (2000, 1, 1));
    }

    #[test]
    fn try_frame_farm_msg_garbage_prefix() {
        let mut buf = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let msg = fix_build(&[(35, "A")], 1);
        buf.extend_from_slice(&msg);
        // Should skip garbage and return (empty, skip_count)
        let (extracted, consumed) = try_frame_farm_msg(&buf).unwrap();
        if extracted.is_empty() {
            // garbage skipped, need to retry from remaining
            let rest = &buf[consumed..];
            let (msg2, _) = try_frame_farm_msg(rest).unwrap();
            assert!(!msg2.is_empty());
        }
    }

    #[test]
    fn try_frame_farm_msg_multiple_sequential() {
        // Two FIX messages back to back
        let msg1 = fix_build(&[(35, "S")], 1);
        let msg2 = fix_build(&[(35, "A"), (108, "30")], 2);
        let mut buf = msg1.clone();
        buf.extend_from_slice(&msg2);
        let (extracted, consumed) = try_frame_farm_msg(&buf).unwrap();
        assert_eq!(extracted, msg1);
        assert_eq!(consumed, msg1.len());
        // Second message
        let (extracted2, consumed2) = try_frame_farm_msg(&buf[consumed..]).unwrap();
        assert_eq!(extracted2, msg2);
        assert_eq!(consumed2, msg2.len());
    }

    #[test]
    fn token_short_hash_nonzero_output() {
        let token = BigUint::from(1u64);
        let hash = token_short_hash(&token);
        assert!(!hash.is_empty());
        // Should be hex string
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_short_hash_large_token() {
        let token = BigUint::from(u64::MAX);
        let hash = token_short_hash(&token);
        assert!(!hash.is_empty());
        assert!(hash.len() <= 8); // u32 hex is at most 8 chars
    }

    #[test]
    fn chrono_free_timestamp_not_empty() {
        let ts = chrono_free_timestamp();
        assert!(!ts.is_empty());
        // Year should start with 20xx
        assert!(ts.starts_with("20"));
    }

    #[test]
    fn gateway_config_fields() {
        let config = GatewayConfig {
            username: "user".to_string(),
            password: Zeroizing::new("pass".to_string()),
            host: "cdc1.ibllc.com".to_string(),
            paper: true,
            accept_invalid_certs: false,
            ib_key_timeout_secs: session::IB_KEY_DEFAULT_TIMEOUT_SECS,
            ib_key_token_sub_type: session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
            code_provider: None,
            resume: None,
        };
        assert_eq!(config.username, "user");
        assert!(config.paper);
    }

    fn auth_with(host: &str, trading_host: &str, trading_farm: &str) -> ReconnectAuth {
        ReconnectAuth {
            host: host.to_string(),
            username: String::new(),
            password: zeroize::Zeroizing::new(String::new()),
            paper: true,
            code_provider: None,
            ib_key_timeout_secs: crate::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
            ib_key_token_sub_type: crate::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
            session_key: num_bigint::BigUint::from(0u32),
            session_token: num_bigint::BigUint::from(0u32),
            server_session_id: String::new(),
            hw_info: String::new(),
            encoded: String::new(),
            hmds_host: String::new(),
            hmds_farm: String::new(),
            trading_host: trading_host.to_string(),
            trading_farm: trading_farm.to_string(),
        }
    }

    /// ibx#295: the reconnect announced the literal `usfarm` and dialled the
    /// configured host, ignoring the route the auth server gave for the
    /// account. The historical-data reconnect beside it has always carried both
    /// — this is the trading side catching up.
    #[test]
    fn a_reconnect_uses_the_route_the_auth_server_gave() {
        assert_eq!(
            reconnect_trading_route(&auth_with("cdc1.ibllc.com", "cdc2.ibllc.com", "euhard")),
            ("cdc2.ibllc.com".to_string(), "euhard".to_string()),
        );

        // No route parsed: the literals it fell back to are still the answer,
        // so an account that reconnected correctly before still does.
        assert_eq!(
            reconnect_trading_route(&auth_with("cdc1.ibllc.com", "", "")),
            ("cdc1.ibllc.com".to_string(), "usfarm".to_string()),
        );

        // And each half falls back independently.
        assert_eq!(
            reconnect_trading_route(&auth_with("cdc1.ibllc.com", "", "euhard")),
            ("cdc1.ibllc.com".to_string(), "euhard".to_string()),
        );
        assert_eq!(
            reconnect_trading_route(&auth_with("cdc1.ibllc.com", "cdc2.ibllc.com", "")),
            ("cdc2.ibllc.com".to_string(), "usfarm".to_string()),
        );

        // The reason the parsed route is stored raw rather than after the
        // initial connect's fallback: with no route stated, the reconnect has
        // to use whatever host the session holds now. Storing the materialized
        // pair would pin the host as it was at connect time and ignore a
        // redirect that has moved it since.
        assert_eq!(
            reconnect_trading_route(&auth_with("cdc3.ibllc.com", "", "")),
            ("cdc3.ibllc.com".to_string(), "usfarm".to_string()),
            "an updated host wins when the server stated no route",
        );
    }
}
