//! What a session says once the auth server has answered its logon.
//!
//! Two bursts, in order: the opening one that asks for everything this login
//! is entitled to, and the one sent as soon as the answer stops arriving,
//! which is what keeps the socket. Both are written as bytes to whatever the
//! caller is holding, so what goes on the wire can be read back in a test.

use std::io::{self, Read, Write};
use std::time::Instant;

use super::{init_scan_buffer, keep_first, note_account};
use crate::api::settings::ExecutionReportScope;
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

/// Ninety-two of these, which is what the counterpart sends and not a number
/// the venue states anywhere. Transcribed from its own opening burst rather
/// than derived, so it is a constant here for the same reason it is one there.
/// What would settle it is a session opened with fewer and a comparison of
/// what the venue sends back; until someone does that, sending what the
/// counterpart sends is the answer with evidence behind it.
const PRIMING_MESSAGES: usize = 92;

/// What the auth server's logon answer states about this login.
///
/// Filled twice: once from the ACK the logon is answered with, and again from
/// the burst that follows it, because the venue states some of these in one
/// and some in the other and states a few in both. Every field keeps what it
/// was told first, so the second reading fills gaps rather than overwriting.
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
    /// The trading port, stated on its own tag. Where it is stated it wins
    /// over the one in the route, which is the order the counterpart reads
    /// them in.
    pub trading_port: Option<u16>,
}

impl Default for LogonAck {
    fn default() -> Self {
        Self {
            account_id: String::new(),
            accounts: Vec::new(),
            logged_in_at: String::new(),
            // What the venue is asked for, until it answers with its own.
            heartbeat_interval: CCP_HEARTBEAT,
            server_session_id: String::new(),
            ccp_token: String::new(),
            raw_soft_dollar_tiers: String::new(),
            raw_family_codes: String::new(),
            raw_news_providers: String::new(),
            raw_order_permissions: String::new(),
            enabled_features: String::new(),
            raw_enabled_features: String::new(),
            white_branding_id: String::new(),
            raw_misc_urls: String::new(),
            trading_route: String::new(),
            mktdata_route: String::new(),
            secdef_route: String::new(),
            trading_port: None,
        }
    }
}

impl LogonAck {
    /// Read the answer to a logon, up to the ACK that ends it.
    ///
    /// Five messages, because the venue puts the routing tags in whichever of
    /// them it likes and the ACK is not always the first to arrive. The reader
    /// wants a short read timeout so a slow ACK segment from a high-latency
    /// venue is retried against `deadline` rather than being fatal.
    pub fn read(r: &mut impl Read, deadline: Instant) -> io::Result<Self> {
        let mut ack = Self::default();
        for _ in 0..5 {
            let raw_response = fix_read_deadline(r, deadline)?;
            // The auth-logon ACK arrives as `8=FIXCOMP` with a DEFLATE-
            // compressed inner body containing the per-account routing tags
            // (6145/6171/8008) and other init data. Inflate before parsing.
            // (See + #129.)
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
                    log::info!("Auth: captured ack.ccp_token (FIX 6386, len={}, prefix={:?})",
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
            // this account is permissioned for. EU ack.accounts get `eufarm`,
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
            // Tag 6321: PRIV_LAB_MISC_URLS — try parsed fields first, then raw byte search.
            // Mirrors the 8035 defensive scan because the value can carry `|` separators
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
                if val.starts_with("DU") || val.starts_with("DF") || val.starts_with("U") {
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
    seq: u32,
) -> io::Result<u32> {
    let mut seq = seq;
    let mut send = |fields: &[(u32, &str)]| -> io::Result<()> {
        seq += 1;
        w.write_all(&fix_build(fields, seq))
    };

    send(&[(35, "U"), (52, now), (6040, "91"), (1, account), (6556, "DR.1"), (6712, "1")])?;
    send(&[(35, "U"), (52, now), (6040, "193"), (6556, "OPR.2"), (8166, "L"), (8176, "1")])?;
    send(&[(35, "U"), (52, now), (6040, "101")])?;
    send(&[(35, "U"), (52, now), (6040, "209"), (1, account), (6556, "AcctConfig3")])?;
    // Which executions a session opens with. The counterpart asks for every
    // one the venue still holds; asking only for today's leaves a caller that
    // had history under the counterpart with none here.
    //
    // The window is what narrows it: stated, the venue answers within it;
    // left off, it answers with what it has. The counterpart writes the window
    // only under its own condition, so leaving it off is a path the venue
    // takes rather than one invented here.
    //
    // The window is not optional: without it the venue rejects the whole
    // message ("Message must contain field # 6536") and the session opens with
    // no executions at all — which is what asking for every one used to do.
    // Asking for every one means naming a start far enough back to cover what
    // the venue still holds.
    let window_start = match scope {
        ExecutionReportScope::Today => format!("{}-00:00:00", &now[..8]),
        _ => crate::config::midnight_days_ago(EXECUTIONS_REACH_BACK_DAYS).to_string(),
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
/// counterpart sends them: account register, a wildcard order-status request,
/// the portfolio logon, a second data request, and the core account subscribe.
///
/// Returns the sequence number the next message takes.
pub(super) fn send_post_burst_grace(
    w: &mut impl Write,
    account: &str,
    now: &str,
    seq: u32,
) -> io::Result<u32> {
    let mut seq = seq;
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

    /// A logon answer, as bytes on a socket that will not be read past.
    fn answered_with(msgs: &[&[(u32, &str)]]) -> std::io::Cursor<Vec<u8>> {
        let mut wire = Vec::new();
        for (i, fields) in msgs.iter().enumerate() {
            wire.extend_from_slice(&fix_build(fields, i as u32 + 1));
        }
        std::io::Cursor::new(wire)
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
        let ack = LogonAck::read(&mut wire, a_minute_from_now()).unwrap();

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
        let ack = LogonAck::read(&mut wire, a_minute_from_now()).unwrap();
        assert_eq!(ack.trading_route, "cdc1.ibllc.com/usfarm");
        assert!(ack.account_id.is_empty(), "read past the message that ended the logon");
    }

    /// A rejected logon is the venue's answer, not a read to retry, and it
    /// says why.
    #[test]
    fn a_rejected_logon_carries_the_reason_the_venue_gave() {
        let mut wire = answered_with(&[&[(35, "3"), (58, "Invalid username or password")]]);
        let err = LogonAck::read(&mut wire, a_minute_from_now())
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

    /// Every message of the opening burst, in order, with the sequence numbers
    /// it takes. Written out rather than compared against a capture of itself:
    /// a burst that loses a message still parses, and only a statement of what
    /// belongs in it notices.
    #[test]
    fn init_sequence_writes_the_burst_the_counterpart_writes() {
        let mut wire = Vec::new();
        let end = send_init_sequence(
            &mut wire, ExecutionReportScope::Today, "DU111111", "20260101-12:00:00", 1,
        )
        .unwrap();

        let msgs = sent(&wire);
        assert_eq!(msgs.len(), 7 + PRIMING_MESSAGES);
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

    /// The five that keep the socket, in the order the counterpart sends them,
    /// carrying the account in the tag each one carries it in.
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
