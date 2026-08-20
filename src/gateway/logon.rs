//! What a session says once the auth server has answered its logon.
//!
//! Two bursts, in order: the opening one that asks for everything this login
//! is entitled to, and the one sent as soon as the answer stops arriving,
//! which is what keeps the socket. Both are written as bytes to whatever the
//! caller is holding, so what goes on the wire can be read back in a test.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use num_bigint::BigUint;
use sha1::{Digest, Sha1};

use super::*;

use crate::settings::ExecutionReportScope;
use crate::config::CCP_HEARTBEAT;
use crate::protocol::fix::{fix_build, fix_parse, fix_read_deadline, SOH};
use crate::protocol::fixcomp;

/// How far back a session asks for executions when it asks for every one the
/// venue holds.
///
/// The venue answers a window starting within seven days of now and rejects
/// one starting earlier ("Invalid value in field # 6536"), which is the whole
/// message and so the whole request. Midnight six days ago is inside that
/// window at any hour of the day.
const EXECUTIONS_REACH_BACK_DAYS: u64 = 6;

/// Count of priming messages in the opening burst. The venue states no value
/// for this anywhere, and it is not derivable from the session; 92 is the
/// count observed to be accepted. Sending fewer is untested.
const PRIMING_MESSAGES: usize = 92;

/// What the auth server's logon answer states about this login.
///
/// Filled twice: once from the ACK the logon is answered with, and again from
/// the burst that follows it, because the venue states some of these in one
/// and some in the other and states a few in both. Every field keeps what it
/// was told first, so the second reading fills gaps rather than overwriting.
#[derive(Default)]
pub(super) struct LogonAck {
    /// The account a caller gets by default.
    pub account_id: String,
    /// Every account this login may trade, the default one first.
    pub accounts: Vec<String>,
    /// When the venue says this logon happened, by its own clock. The
    /// competing session it names carries a stamp from the same clock, and one
    /// clock's readings are comparable where two are not.
    ///
    /// Empty until the venue's answer states it, and empty is the careful
    /// reading: every session the venue names then counts as another client,
    /// so this one gives the account up rather than taking it.
    pub logged_in_at: String,
    pub heartbeat_interval: u64,
    pub server_session_id: String,
    pub ccp_token: String,
    pub raw_soft_dollar_tiers: String,
    pub raw_family_codes: String,
    pub raw_news_providers: String,
    pub raw_order_permissions: String,
    pub enabled_features: String,
    pub raw_enabled_features: String,
    pub white_branding_id: String,
    pub raw_misc_urls: String,
    /// Which farms this account is routed to. `usfarm`/`ushmds` are US names;
    /// an EU account is routed to eufarm/euhmds/secdefeu, and so on for every
    /// other region. Each route reads `<host>/<farm>` or `<host>/<farm>/<port>`.
    pub trading_route: String,
    /// Market data, tag 6171.
    pub mktdata_route: String,
    /// Contract definitions, tag 8008.
    pub secdef_route: String,
    /// The trading port, tag 6146. Takes precedence over a port carried in
    /// the route string.
    pub trading_port: Option<u16>,
}

impl LogonAck {
    /// Read the answer to a logon, up to the ACK that ends it.
    ///
    /// Five messages, because the venue puts the routing tags in whichever of
    /// them it likes and the ACK is not always the first to arrive. The reader
    /// wants a short read timeout so a slow ACK segment from a high-latency
    /// venue is retried against `deadline` rather than being fatal.
    ///
    /// `carry` holds what a read took off the socket past the end of the
    /// message it returned. Those bytes are the next message, and after the
    /// last one they are the venue's first unprompted words to this session —
    /// so the caller keeps them and reads on from there rather than losing
    /// them here.
    pub fn read(
        r: &mut impl Read,
        carry: &mut Vec<u8>,
        deadline: Instant,
    ) -> io::Result<Self> {
        // What the venue is asked for, until it answers with its own.
        let mut ack = Self { heartbeat_interval: CCP_HEARTBEAT, ..Self::default() };
        for _ in 0..5 {
            let raw_response = fix_read_deadline(r, carry, deadline)?;
            // The auth-logon ACK arrives as `8=FIXCOMP` with a DEFLATE-
            // compressed inner body containing the per-account routing tags
            // (6145/6171/8008) and other init data. Inflate before parsing.
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

            // When the venue says this logon happened, by its own clock. The
            // competing session it names carries a stamp from the same clock,
            // and one clock's readings are comparable where two are not.
            if let Some(v) = fields.get(&52)
                && ack.logged_in_at.is_empty()
            {
                ack.logged_in_at = v.clone();
                log::info!("the venue stamps this logon {}", ack.logged_in_at);
            }

            if let Some(v) = fields.get(&1) {
                if ack.account_id.is_empty() { ack.account_id = v.clone(); }
                note_account(&mut ack.accounts, v);
            }
            if let Some(v) = fields.get(&108)
                && let Ok(hb) = v.parse() { ack.heartbeat_interval = hb; }
            if let Some(v) = fields.get(&6386)
                && ack.ccp_token.is_empty() {
                    ack.ccp_token = v.clone();
                    log::info!("Auth: captured ccp_token (FIX 6386, len={}, prefix={:?})",
                        ack.ccp_token.len(),
                        if ack.ccp_token.len() > 16 { &ack.ccp_token[..16] } else { &ack.ccp_token });
                }
            // Tag 8035: try parsed fields first, then raw byte search
            if ack.server_session_id.is_empty() {
                if let Some(v) = fields.get(&8035) {
                    ack.server_session_id = v.clone();
                } else {
                    let marker = b"\x018035=";
                    if let Some(pos) = response.windows(marker.len()).position(|w| w == marker) {
                        let val_start = pos + marker.len();
                        if let Some(end) = response[val_start..].iter().position(|&b| b == SOH) {
                            ack.server_session_id = String::from_utf8_lossy(
                                &response[val_start..val_start + end],
                            ).to_string();
                        }
                    }
                }
            }

            // Farm routing — server tells us which farms
            // this account is permissioned for. EU accounts get `eufarm`,
            // US get `usfarm`, etc. Read once from whichever auth msg has it.
            if let Some(v) = fields.get(&6145)
                && ack.trading_route.is_empty() {
                    ack.trading_route = v.clone();
                    log::info!("Auth: trading farm route = {}", ack.trading_route);
                }
            if let Some(v) = fields.get(&6146)
                && ack.trading_port.is_none()
                && let Ok(p) = v.trim().parse::<u16>() {
                    ack.trading_port = Some(p);
                    log::info!("Auth: trading farm port = {p}");
                }
            if let Some(v) = fields.get(&6171)
                && ack.mktdata_route.is_empty() {
                    ack.mktdata_route = v.clone();
                    log::info!("Auth: market-data farm route = {}", ack.mktdata_route);
                }
            if let Some(v) = fields.get(&8008)
                && ack.secdef_route.is_empty() {
                    ack.secdef_route = v.clone();
                    log::info!("Auth: secdef farm route = {}", ack.secdef_route);
                }

            // Gateway-local init data from logon response
            if let Some(v) = fields.get(&6560) { keep_first(&mut ack.raw_soft_dollar_tiers, v, "6560"); }
            if let Some(v) = fields.get(&6823) { keep_first(&mut ack.raw_family_codes, v, "6823"); }
            if let Some(v) = fields.get(&6830) { keep_first(&mut ack.raw_news_providers, v, "6830"); }
            if let Some(v) = fields.get(&6652) { keep_first(&mut ack.raw_order_permissions, v, "6652"); }
            if let Some(v) = fields.get(&6542) {
                keep_first(&mut ack.enabled_features, v, "6542");
                log::info!("Enabled features: {v}");
            }
            if let Some(v) = fields.get(&6542) { keep_first(&mut ack.raw_enabled_features, v, "6542"); }
            if let Some(v) = fields.get(&6571) { keep_first(&mut ack.white_branding_id, v, "6571"); }
            // Tag 6321: PRIV_LAB_MISC_URLS — try parsed fields first, then raw byte
            // search.
            // Mirrors the 8035 defensive scan because the value can carry `|`
            // separators
            // that confuse downstream parsers if a chunk is fragmented.
            if ack.raw_misc_urls.is_empty() {
                if let Some(v) = fields.get(&6321) {
                    ack.raw_misc_urls = v.clone();
                    log::info!("Found misc URLs from logon ACK ({} bytes)", ack.raw_misc_urls.len());
                } else {
                    let marker = b"\x016321=";
                    if let Some(pos) = response.windows(marker.len()).position(|w| w == marker) {
                        let val_start = pos + marker.len();
                        if let Some(end) = response[val_start..].iter().position(|&b| b == SOH) {
                            ack.raw_misc_urls = String::from_utf8_lossy(
                                &response[val_start..val_start + end],
                            ).to_string();
                            log::info!("Found misc URLs from logon ACK byte scan ({} bytes)", ack.raw_misc_urls.len());
                        }
                    }
                }
            }

            // Stop on the logon ACK or the server config message
            if msg_type == "A" || msg_type == "U" {
                break;
            }
        }
        Ok(ack)
    }

    /// Fill from the burst the opening messages are answered with.
    ///
    /// The venue states the account here where the logon ACK named only the
    /// username, and states the routing tags here for accounts it did not
    /// state them for above.
    pub fn scan_init(&mut self, init_data: &[u8], username: &str) {
        let scan_data = init_scan_buffer(init_data);

        // Scan init response for account ID and gateway-local init tags
        let init_str = String::from_utf8_lossy(&scan_data);
        // Log every part naming "farm" or "hmds", which is where the routing
        // tags sit.
        for part in init_str.split('\x01') {
            if part.contains("farm") || part.contains("hmds") || part.contains("secdef") {
                log::info!("Init scan: routing-shaped part = {part:?}");
            }
        }
        for part in init_str.split('\x01') {
            if part.starts_with("1=") && part.len() > 2 {
                let val = &part[2..];
                // Tag 1 is the account, whatever the account is spelled
                // like. Admitting only the prefixes a paper and a plain
                // individual account start with discards an advisor's or an
                // institution's account, and the username then stands in as
                // the account on every request that names one. The username
                // is the only value this has to tell apart.
                if val != username {
                    note_account(&mut self.accounts, val);
                    if self.account_id.is_empty() || self.account_id == username {
                        self.account_id = val.to_string();
                        log::info!("Found account ID from init response: {}", self.account_id);
                    }
                }
            } else if part.starts_with("6560=") && self.raw_soft_dollar_tiers.is_empty() {
                self.raw_soft_dollar_tiers = part[5..].to_string();
                log::info!("Found soft dollar tiers from init response ({} bytes)", self.raw_soft_dollar_tiers.len());
            } else if part.starts_with("6823=") && self.raw_family_codes.is_empty() {
                self.raw_family_codes = part[5..].to_string();
                log::info!("Found family codes from init response ({} bytes)", self.raw_family_codes.len());
            } else if part.starts_with("6830=") && self.raw_news_providers.is_empty() {
                self.raw_news_providers = part[5..].to_string();
                log::info!("Found news providers from init response ({} bytes)", self.raw_news_providers.len());
            } else if part.starts_with("6571=") && self.white_branding_id.is_empty() {
                self.white_branding_id = part[5..].to_string();
                log::info!("Found white branding ID from init response");
            } else if part.starts_with("6321=") && self.raw_misc_urls.is_empty() {
                self.raw_misc_urls = part[5..].to_string();
                log::info!("Found misc URLs from init response ({} bytes)", self.raw_misc_urls.len());
            } else if part.starts_with("6145=") && self.trading_route.is_empty() {
                self.trading_route = part[5..].to_string();
                log::info!("Found trading farm route in init response: {}", self.trading_route);
            } else if let Some(port) = part.strip_prefix("6146=")
                && self.trading_port.is_none()
                && let Ok(p) = port.trim().parse::<u16>() {
                    // Read here as well as from the acknowledgement: a port
                    // stated only in the burst is otherwise dropped and the
                    // configured one dialled in its place.
                    self.trading_port = Some(p);
                    log::info!("Found trading farm port in init response: {p}");
            } else if part.starts_with("6171=") && self.mktdata_route.is_empty() {
                self.mktdata_route = part[5..].to_string();
                log::info!("Found market-data farm route in init response: {}", self.mktdata_route);
            } else if part.starts_with("8008=") && self.secdef_route.is_empty() {
                self.secdef_route = part[5..].to_string();
                log::info!("Found secdef farm route in init response: {}", self.secdef_route);
            }
        }
    }
}

/// The opening burst, from the sequence number the logon left behind.
///
/// Returns the sequence number the next message takes.
pub(super) fn send_init_sequence(
    w: &mut impl Write,
    scope: ExecutionReportScope,
    account: &str,
    now: &str,
    mut seq: u32,
) -> io::Result<u32> {
    let mut send = |fields: &[(u32, &str)]| -> io::Result<()> {
        seq += 1;
        w.write_all(&fix_build(fields, seq))
    };

    send(&[(35, "U"), (52, now), (6040, "91"), (1, account), (6556, "DR.1"), (6712, "1")])?;
    send(&[(35, "U"), (52, now), (6040, "193"), (6556, "OPR.2"), (8166, "L"), (8176, "1")])?;
    send(&[(35, "U"), (52, now), (6040, "101")])?;
    send(&[(35, "U"), (52, now), (6040, "209"), (1, account), (6556, "AcctConfig3")])?;
    // Which executions a session opens with. Asking for every execution the
    // venue still holds; asking only for today's returns no earlier history.
    //
    // The window narrows the answer: stated, the venue answers within it;
    // omitted, it answers with everything it holds. Omitting it is a
    // request shape the venue accepts.
    //
    // The window is not optional: without it the venue rejects the whole
    // message ("Message must contain field # 6536") and the session opens with
    // no executions at all — which is what asking for every one used to do.
    // Asking for every one means naming a start far enough back to cover what
    // the venue still holds.
    let window_start = match scope {
        ExecutionReportScope::Today => format!("{}-00:00:00", &now[..8]),
        _ => crate::protocol::datetime::midnight_days_ago(EXECUTIONS_REACH_BACK_DAYS).to_string(),
    };
    send(&[(35, "U"), (52, now), (6040, "72"), (6536, &window_start), (6537, now), (6556, "today4")])?;
    send(&[(35, "U"), (52, now), (6040, "74"), (1, ""), (6544, "2")])?;
    send(&[(35, "U"), (52, now), (6040, "76"), (1, ""), (6565, "1")])?;
    for _ in 0..PRIMING_MESSAGES {
        send(&[(35, "U"), (52, now), (6040, "80")])?;
    }
    w.flush()?;
    Ok(seq)
}

/// What holds the socket once the opening burst has been answered.
///
/// The CCP server FINs the connection ~12 s after the init-burst response if
/// no application-level traffic arrives in the grace window — heartbeats alone
/// do not satisfy "client alive". Five messages satisfy it, in the order the
/// protocol defines: account register, a wildcard order-status request,
/// the portfolio logon, a second data request, and the core account subscribe.
///
/// Returns the sequence number the next message takes.
pub(super) fn send_post_burst_grace(
    w: &mut impl Write,
    account: &str,
    now: &str,
    mut seq: u32,
) -> io::Result<u32> {
    let mut send = |fields: &[(u32, &str)]| -> io::Result<()> {
        seq += 1;
        w.write_all(&fix_build(fields, seq))
    };

    send(&[(35, "U"), (52, now), (6040, "6"), (6036, "1"), (6529, "AR.1"), (6095, account)])?;
    send(&[(35, "H"), (52, now), (11, "*"), (55, "*"), (54, "*")])?;
    // The account goes in tag 1 here, not 6095.
    send(&[(35, "U"), (52, now), (6040, "142"), (6529, "PLR.1"), (1, account)])?;
    send(&[(35, "U"), (52, now), (6040, "91"), (1, account), (6712, "1"), (6556, "DR.2")])?;
    send(&[(35, "U"), (52, now), (6040, "74"), (1, account), (6700, "Core"), (6544, "2")])?;
    w.flush()?;
    Ok(seq)
}

/// The opening a rebuilt connection states, which is the opening a first
/// logon states.
///
/// A reconnect once sent a mass status request and nothing else, which reads
/// as complete — the session logs on, takes orders and acknowledges them — and
/// is not: the executions subscription lives in the init sequence, so a
/// rebuilt connection reported no fill for anything placed on it, for as long
/// as it lasted. The grace burst belongs here for the same reason it belongs
/// after a logon: the venue closes a connection that has sent nothing but
/// heartbeats.
///
/// It also carries the mass status request, in the position a logon gives it.
/// Written here a second time as well, the venue was asked to replay every
/// order twice and this connection numbered every message one ahead of the
/// ones a first logon takes.
pub(super) fn send_reconnect_opening(
    w: &mut impl Write,
    scope: ExecutionReportScope,
    account: &str,
    now: &str,
) -> io::Result<u32> {
    let seq = send_init_sequence(w, scope, account, now, 1)?;
    send_post_burst_grace(w, account, now, seq)
}

/// Split the venue's per-security-type order permissions, logon tag 6652.
///
/// `SECTYPE:ORDTYPE,ORDTYPE;SECTYPE:...`. A security type named with no order
/// types after it is still permitted, so an absent list is an empty one and
/// never a missing key.
pub(super) fn parse_order_permissions(raw: &str) -> std::collections::HashMap<String, Vec<String>> {
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

/// Parse a farm-route string from the auth-server's routing tags.
///
/// Three accepted shapes:
///   `"<host>/<farm>"`            — tag 6145 (trading)
///   `"<host>/<farm>/<port>"`     — tags 6171 (mktdata) / 8008 (secdef)
///
/// A route may name the port to reach that farm on. Where no separate tag
/// carries the port, the one in the route applies; a route carrying neither is
/// an error rather than a case for a default. `None` means the route stated no
/// port, which is the only case the configured port applies to.
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

/// Returns true if `buf` contains at least one complete `8=O` (binary) or
/// `8=FIXCOMP` frame. Used to terminate read drains as soon as the expected
/// response is fully buffered.
pub(super) fn has_complete_response_frame(buf: &[u8]) -> bool {
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
    settings: &crate::settings::SessionSettings,
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
    settings: &crate::settings::SessionSettings,
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

                // Check for auth challenge → respond with token, fall back to SRP if
                // rejected.
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
pub(super) fn try_frame_farm_msg(buf: &[u8]) -> Option<(Vec<u8>, usize)> {
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

/// Keep the first value the venue sends for a field it sends once, and say so
/// when it sends a second one that differs. Each of these carries its whole
/// list in one field, so a second differing value would mean the list arrives
/// in parts and keeping the first silently drops the rest — which looks
/// identical to the venue having sent nothing more.
pub(super) fn keep_first(slot: &mut String, value: &str, field: &str) {
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
pub(super) fn note_account(accounts: &mut Vec<String>, account: &str) {
    if !account.is_empty() && !accounts.iter().any(|a| a == account) {
        accounts.push(account.to_string());
    }
}

/// Another session already logged in on this account when this one connected.
///
/// Stated in the venue's answer to the connect: a session that arrives second
/// is told which session was already logged in. The venue may serve both
/// sessions, or hold this one to reading only.
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

/// A host the venue named, and the port it named it on.
///
/// The venue states `host:port` in a redirect. Falls back to the port this
/// session is already on when it states only a host, which is the common
/// case.
pub(super) fn host_and_port(target: &str, fallback: u16) -> (&str, u16) {
    match target.split_once(':') {
        Some((host, port)) => (host, port.parse().unwrap_or(fallback)),
        None => (target, fallback),
    }
}

/// Whether a session the venue names belongs to another client rather than to
/// this one.
///
/// Both stamps are GMT `YYYYMMDD-HH:MM:SS`, so comparing them as text compares
/// them as moments. Equal counts as this session's own: a reconnect that lands
/// in the same second as the logon being reaped is the common case, and giving
/// the account up over it would strand a caller who has no competitor at all.
pub(super) fn is_another_client(other_since: &str, our_logon: &str) -> bool {
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
pub(super) fn init_scan_buffer(init_data: &[u8]) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::fix::fix_parse;
    use std::collections::HashMap;

    /// The messages a session wrote, split where each one ends: a checksum
    /// field, which is the last field of a FIX message and the only place the
    /// tag appears.
    fn sent(bytes: &[u8]) -> Vec<HashMap<u32, String>> {
        let mut msgs = Vec::new();
        let mut rest = bytes;
        while let Some(idx) = rest.windows(4).position(|w| w == b"\x0110=") {
            let end = idx + 4 + rest[idx + 4..].iter().position(|&b| b == 1).unwrap() + 1;
            msgs.push(fix_parse(&rest[..end]));
            rest = &rest[end..];
        }
        assert!(rest.is_empty(), "trailing bytes after the last message");
        msgs
    }

    /// A logon answer, one message per read, as a socket delivers it.
    struct Answer(std::collections::VecDeque<Vec<u8>>);

    impl Read for Answer {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.pop_front() {
                Some(msg) => {
                    buf[..msg.len()].copy_from_slice(&msg);
                    Ok(msg.len())
                }
                None => Ok(0),
            }
        }
    }

    fn answered_with(msgs: &[&[(u32, &str)]]) -> Answer {
        Answer(
            msgs.iter()
                .enumerate()
                .map(|(i, fields)| fix_build(fields, i as u32 + 1))
                .collect(),
        )
    }

    /// The same answer, with every message in one read — which is what a venue
    /// that sends them back to back looks like from this side.
    fn answered_all_at_once(msgs: &[&[(u32, &str)]]) -> Answer {
        let mut one = Vec::new();
        for (i, fields) in msgs.iter().enumerate() {
            one.extend_from_slice(&fix_build(fields, i as u32 + 1));
        }
        Answer([one].into_iter().collect())
    }

    fn a_minute_from_now() -> Instant {
        Instant::now() + std::time::Duration::from_secs(60)
    }

    /// Where this login is routed, which farm connection it opens on, and
    /// which account it holds — all stated in the answer to its logon.
    #[test]
    fn the_ack_states_the_routes_this_login_connects_on() {
        let mut wire = answered_with(&[&[
            (35, "A"),
            (1, "DU111111"),
            (108, "45"),
            (52, "20260101-12:00:00"),
            (6145, "cdc1.ibllc.com/usfarm"),
            (6146, "4001"),
            (6171, "hdc1.ibllc.com/ushmds/4002"),
            (8008, "sdc1.ibllc.com/secdefil/4003"),
        ]]);
        let ack = LogonAck::read(&mut wire, &mut Vec::new(), a_minute_from_now()).unwrap();

        assert_eq!(ack.trading_route, "cdc1.ibllc.com/usfarm");
        assert_eq!(ack.trading_port, Some(4001));
        assert_eq!(ack.mktdata_route, "hdc1.ibllc.com/ushmds/4002");
        assert_eq!(ack.secdef_route, "sdc1.ibllc.com/secdefil/4003");
        assert_eq!(ack.account_id, "DU111111");
        assert_eq!(ack.accounts, ["DU111111"]);
        assert_eq!(ack.heartbeat_interval, 45);
        assert_eq!(ack.logged_in_at, "20260101-12:00:00");
    }

    /// Reading stops at the logon ACK or the server config message, whichever
    /// arrives first. What follows it belongs to the session, not to the
    /// logon, and is left on the socket for the hot loop to read.
    #[test]
    fn reading_stops_at_the_message_that_ends_the_logon() {
        let mut wire = answered_with(&[
            &[(35, "U"), (6145, "cdc1.ibllc.com/usfarm")],
            &[(35, "A"), (1, "DU999999"), (6145, "elsewhere/eufarm")],
        ]);
        let ack = LogonAck::read(&mut wire, &mut Vec::new(), a_minute_from_now()).unwrap();
        assert_eq!(ack.trading_route, "cdc1.ibllc.com/usfarm");
        assert!(ack.account_id.is_empty(), "read past the message that ended the logon");
    }

    /// The ACK is not always the first message the venue sends, and what it
    /// states is read wherever in the answer it arrives.
    #[test]
    fn the_ack_is_read_however_many_messages_precede_it() {
        let mut wire = answered_with(&[
            &[(35, "0")],
            &[(35, "0")],
            &[(35, "0")],
            &[(35, "A"), (1, "DU111111"), (6145, "cdc1.ibllc.com/usfarm")],
        ]);
        let ack = LogonAck::read(&mut wire, &mut Vec::new(), a_minute_from_now()).unwrap();
        assert_eq!(ack.account_id, "DU111111");
        assert_eq!(ack.trading_route, "cdc1.ibllc.com/usfarm");
    }

    /// The venue puts what it likes in one segment, and the messages behind
    /// the first one are not this reader's to drop: they carry the routes the
    /// session connects on and the stamp it is reaped against.
    #[test]
    fn messages_arriving_together_are_all_read() {
        let msgs: &[&[(u32, &str)]] = &[
            &[(35, "0")],
            &[(35, "0"), (6145, "cdc1.ibllc.com/usfarm"), (6146, "4001")],
            &[(35, "A"), (1, "DU111111"), (52, "20260101-12:00:00"),
              (6171, "hdc1.ibllc.com/ushmds/4002")],
        ];
        let mut wire = answered_all_at_once(msgs);
        let ack = LogonAck::read(&mut wire, &mut Vec::new(), a_minute_from_now()).unwrap();

        assert_eq!(ack.trading_route, "cdc1.ibllc.com/usfarm");
        assert_eq!(ack.trading_port, Some(4001));
        assert_eq!(ack.mktdata_route, "hdc1.ibllc.com/ushmds/4002");
        assert_eq!(ack.account_id, "DU111111");
        assert_eq!(ack.logged_in_at, "20260101-12:00:00");
    }

    /// What arrives behind the message that ended the logon is the venue's
    /// first unprompted words to this session, and the caller reads on from
    /// them rather than losing them here.
    #[test]
    fn what_follows_the_ack_is_left_for_the_caller() {
        let msgs: &[&[(u32, &str)]] = &[
            &[(35, "A"), (1, "DU111111")],
            &[(35, "8"), (11, "42"), (39, "2")],
        ];
        let mut wire = answered_all_at_once(msgs);
        let mut carry = Vec::new();
        let ack = LogonAck::read(&mut wire, &mut carry, a_minute_from_now()).unwrap();

        assert_eq!(ack.account_id, "DU111111");
        assert_eq!(fix_parse(&carry)[&11], "42", "the fill behind the ACK was dropped");
    }

    /// A logout is the venue refusing the logon as much as a reject is, and it
    /// arrives in place of the ACK rather than after it.
    #[test]
    fn a_logout_in_place_of_the_ack_is_a_refusal() {
        let mut wire = answered_with(&[&[(35, "5"), (58, "Too many sessions")]]);
        let err = LogonAck::read(&mut wire, &mut Vec::new(), a_minute_from_now())
            .err()
            .expect("a logout is not an answer to read past");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("Too many sessions"), "{err}");
    }

    /// A rejected logon is the venue's answer, not a read to retry, and it
    /// says why.
    #[test]
    fn a_rejected_logon_carries_the_reason_the_venue_gave() {
        let mut wire = answered_with(&[&[(35, "3"), (58, "Invalid username or password")]]);
        let err = LogonAck::read(&mut wire, &mut Vec::new(), a_minute_from_now())
            .err()
            .expect("a rejected logon is not an answer to read past");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("Invalid username or password"), "{err}");
    }

    /// What the venue states only in the burst that follows the ACK, which is
    /// where an account is named for logins the ACK names only by username.
    #[test]
    fn the_init_burst_fills_what_the_ack_left_empty() {
        let mut ack = LogonAck {
            trading_route: "cdc1.ibllc.com/usfarm".into(),
            ..Default::default()
        };
        let burst = b"\x011=DU222222\x016145=elsewhere/eufarm\x016171=hdc1.ibllc.com/ushmds/4002\
                      \x016830=BRFG+BRFUPDN\x01";
        ack.scan_init(burst, "someone");

        assert_eq!(ack.account_id, "DU222222");
        assert_eq!(ack.accounts, ["DU222222"]);
        assert_eq!(ack.raw_news_providers, "BRFG+BRFUPDN");
        assert_eq!(ack.mktdata_route, "hdc1.ibllc.com/ushmds/4002");
        // Already stated at the ACK, so the burst does not move it.
        assert_eq!(ack.trading_route, "cdc1.ibllc.com/usfarm");
    }

    /// A login whose ACK named only the username takes the account the burst
    /// names, which is the account it will trade.
    #[test]
    fn an_account_named_in_the_burst_replaces_the_username() {
        let mut ack = LogonAck { account_id: "someone".into(), ..Default::default() };
        ack.scan_init(b"\x011=U333333\x01", "someone");
        assert_eq!(ack.account_id, "U333333");
    }

    /// A rebuilt connection opens exactly as a first logon does.
    ///
    /// The mass status request is stated once, in the position the logon gives
    /// it. Written a second time before the burst, the venue was asked to
    /// replay every order twice and the connection numbered every message one
    /// ahead of a first logon's.
    #[test]
    fn a_reconnect_opens_the_way_a_logon_opens() {
        let mut reconnect = Vec::new();
        let end = send_reconnect_opening(
            &mut reconnect, ExecutionReportScope::Today, "DU111111", "20260101-12:00:00",
        )
        .unwrap();

        let mut logon = Vec::new();
        let seq = send_init_sequence(
            &mut logon, ExecutionReportScope::Today, "DU111111", "20260101-12:00:00", 1,
        )
        .unwrap();
        let logon_end =
            send_post_burst_grace(&mut logon, "DU111111", "20260101-12:00:00", seq).unwrap();

        assert_eq!(reconnect, logon, "a reconnect states what a logon states");
        assert_eq!(end, logon_end, "and numbers its messages the same way");

        let text = String::from_utf8_lossy(&reconnect);
        assert_eq!(
            text.matches("\u{1}35=H\u{1}").count(),
            1,
            "the mass status request is stated once: {text:?}",
        );
    }

    /// An account is whatever the venue spells it, and the port it names in
    /// the burst is the port the session dials.
    ///
    /// Admitting only the prefixes a paper and a plain individual account
    /// start with discards an advisor's or an institution's account, and the
    /// username stands in as the account on every request that names one.
    /// Reading the port from the acknowledgement alone leaves the configured
    /// port standing when the burst is the only place it is stated.
    #[test]
    fn the_burst_names_the_account_and_the_port_whatever_they_are() {
        let mut ack = LogonAck { account_id: "someone".into(), ..Default::default() };
        ack.scan_init(b"\x011=F1234567\x016146=4001\x01", "someone");
        assert_eq!(ack.account_id, "F1234567", "the account the venue named");
        assert_eq!(ack.accounts, ["F1234567"]);
        assert_eq!(ack.trading_port, Some(4001), "the port the venue named");
    }

    /// The username is not an account, and a burst that repeats it does not
    /// make it one.
    #[test]
    fn the_username_is_not_taken_for_an_account() {
        let mut ack = LogonAck::default();
        ack.scan_init(b"\x011=someone\x01", "someone");
        assert!(ack.account_id.is_empty(), "got {:?}", ack.account_id);
        assert!(ack.accounts.is_empty(), "got {:?}", ack.accounts);
    }

    /// Every message of the opening burst, in order, with the sequence numbers
    /// it takes. Written out rather than compared against a capture of itself:
    /// a burst that loses a message still parses, and only a statement of what
    /// belongs in it notices.
    #[test]
    fn the_opening_burst_states_five_messages_in_order() {
        let mut wire = Vec::new();
        let end = send_init_sequence(
            &mut wire, ExecutionReportScope::Today, "DU111111", "20260101-12:00:00", 1,
        )
        .unwrap();

        let msgs = sent(&wire);
        // Ninety-nine, written out rather than derived from the constant the
        // burst is built from: a length checked against its own input agrees
        // with any value that input takes.
        assert_eq!(msgs.len(), 99);
        assert_eq!(end, 1 + msgs.len() as u32);

        let comm_type = |m: &HashMap<u32, String>| m[&6040].clone();
        assert_eq!(
            msgs[..7].iter().map(comm_type).collect::<Vec<_>>(),
            ["91", "193", "101", "209", "72", "74", "76"],
        );
        assert!(msgs[7..].iter().all(|m| comm_type(m) == "80"));
        assert_eq!(msgs[0][&34], "000002");
        assert_eq!(msgs.last().unwrap()[&34], format!("{end:06}"));
        assert_eq!(msgs[0][&1], "DU111111");

        // The window the executions request names, which the venue rejects the
        // whole message without.
        assert_eq!(msgs[4][&6536], "20260101-00:00:00");
        assert_eq!(msgs[4][&6537], "20260101-12:00:00");
    }

    /// Asking for every execution the venue holds names a start far enough
    /// back to cover them, and it is not today's midnight.
    #[test]
    fn every_execution_reaches_further_back_than_today() {
        let mut wire = Vec::new();
        send_init_sequence(
            &mut wire, ExecutionReportScope::All, "DU111111", "20260101-12:00:00", 1,
        )
        .unwrap();
        let start = sent(&wire)[4][&6536].clone();
        assert!(start.ends_with("-00:00:00"), "{start}");
        assert_ne!(start, "20260101-00:00:00");
    }

    /// The five messages that keep the socket, in order, each carrying the
    /// account in its own tag.
    #[test]
    fn post_burst_grace_writes_five_messages_carrying_the_account() {
        let mut wire = Vec::new();
        let end = send_post_burst_grace(&mut wire, "DU111111", "20260101-12:00:00", 101).unwrap();
        assert_eq!(end, 106);

        let msgs = sent(&wire);
        assert_eq!(
            msgs.iter().map(|m| m[&35].clone()).collect::<Vec<_>>(),
            ["U", "H", "U", "U", "U"],
        );
        assert_eq!(msgs[0][&6095], "DU111111");
        assert!(!msgs[0].contains_key(&1), "account register carries no tag 1");
        assert_eq!((&msgs[1][&11], &msgs[1][&55], &msgs[1][&54]), (&"*".into(), &"*".into(), &"*".into()));
        for m in &msgs[2..] {
            assert_eq!(m[&1], "DU111111");
        }
        assert_eq!(
            msgs.iter().map(|m| m[&34].clone()).collect::<Vec<_>>(),
            ["000102", "000103", "000104", "000105", "000106"],
        );
    }
}
