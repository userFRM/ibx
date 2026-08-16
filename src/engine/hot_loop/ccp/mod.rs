use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// How long a matching-symbols request waits for its reply. Matches the
/// historical-request idle timeout: both are one round trip to the gateway.
const MATCHING_SYMBOLS_TIMEOUT: Duration = Duration::from_secs(60);

/// How long an option chain request waits for its reply. Also one round trip,
/// for a reply that carries every class of an underlying at once.
const OPTION_CHAIN_TIMEOUT: Duration = Duration::from_secs(60);

use crate::bridge::{Event, RichOrderInfo, SharedState};
use crate::api::types as api;
use crate::engine::context::Context;
use crate::config::chrono_free_timestamp;
use crate::protocol::connection::{Connection, Frame};
use crate::protocol::fix;
use crate::protocol::fixcomp;
use crate::types::{
    CompletedOrder, Fill, InstrumentId, MidnightSeed, NewsBulletin,
    PositionInfo, Price, Side, PRICE_SCALE,
};
use std::sync::mpsc::SyncSender;

use super::{HeartbeatState, emit, clone_for_event, parse_price_tag, decode_tif};

/// Where to book a fill whose order this session does not track.
///
/// Both the contract and the side come off the report, and both are required:
/// a guessed side would move the position the wrong way, which is worse than
/// reporting that the fill could not be placed.
fn untracked_fill_target(
    context: &mut Context,
    parsed: &std::collections::HashMap<u32, String>,
) -> Option<(InstrumentId, Side)> {
    // A replayed execution restates history rather than reporting something
    // new. On a fresh process the gateway resends prior fills with 97=Y and
    // their original ExecIDs, for orders no session tracks; booking those
    // would build a position out of the past on top of the one the position
    // feed already reports. Within a process the ExecID window catches the
    // reconnect burst, so only the untracked case needs this.
    let replayed = |tag| parsed.get(&tag).map(|v| v.eq_ignore_ascii_case("Y")).unwrap_or(false);
    if replayed(97) || replayed(43) {
        log::debug!("Untracked fill is a replay, leaving the position alone");
        return None;
    }
    let con_id: i64 = parsed.get(&6008).and_then(|s| s.parse().ok()).unwrap_or(0);
    if con_id == 0 {
        log::warn!("Untracked fill carries no ContractID, position not updated");
        return None;
    }
    let side = match parsed.get(&54).map(|s| s.as_str()) {
        Some("1") => Side::Buy,
        Some("2") => Side::Sell,
        Some("5") => Side::ShortSell,
        other => {
            log::warn!("Untracked fill has Side={other:?}, position not updated");
            return None;
        }
    };
    // Fallible: a full instrument table must not abort the engine on an
    // inbound message.
    let Some(instrument) = context.try_register_instrument(con_id) else {
        log::warn!("Untracked fill for conId {con_id}: instrument table full, position not updated");
        return None;
    };
    if let Some(symbol) = parsed.get(&55) {
        context.set_symbol(instrument, symbol.clone());
    }
    Some((instrument, side))
}

/// Bound for an in-flight contract-details request (secdef reply or
/// per-exchange fan-out). Refreshed on fan-out activity; on expiry the
/// request surfaces error 200 + contract_details_end instead of hanging
/// forever. A gateway rejection arrives in well under a second,
/// and a full 27-exchange fan-out completes within a few.
const SECDEF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Number of most-recent ExecIDs retained for fill deduplication. Bounds the
/// memory of `seen_exec_ids` while staying large enough that a server replay
/// after a reconnect burst still hits the window.
const EXEC_ID_WINDOW: usize = 1024;

/// Synthetic ibapi error code for a parked (39=I) order's reason, delivered
/// through `Wrapper::error` since ibapi has no callback dedicated to an order
/// held with a reason. Mirrors IB's generic order-message code (399) rather
/// than the reject code (201) — an Inactive order is not rejected, it can
/// still reactivate.
const ORDER_INACTIVE_ERROR_CODE: i32 = 399;

/// The gateway's stated reason for a parked or rejected order: the tag 58 text
/// with the tag 103 reason code. Either alone is ambiguous — the text is often
/// generic and the code alone names no instrument — so both are reported when
/// the report carries both. Empty when it carries neither.
fn stated_reason(parsed: &std::collections::HashMap<u32, String>) -> String {
    let text = parsed.get(&58).map(|s| s.as_str()).unwrap_or("");
    let code = parsed.get(&103).map(|s| s.as_str()).unwrap_or("");
    match (text.is_empty(), code.is_empty()) {
        (false, false) => format!("{text} (reason code {code})"),
        (false, true) => text.to_string(),
        (true, false) => format!("reason code {code}"),
        (true, true) => String::new(),
    }
}

/// Convert a FIX OrderID hex string (e.g. "00cf16ed.000225ed.69ca0941.0001") to a stable i64 permId.
/// Uses FNV-1a hash of the first 3 dot-segments (the stable prefix) so that permId
/// remains constant across modifications (the last segment increments on each modify).
/// Extract the value of a single FIX tag from a raw message.
/// `prefix` should include the tag number and `=` (e.g. `b"6256="`).
fn extract_tag_value(msg: &[u8], prefix: &[u8]) -> Option<String> {
    use crate::protocol::fix::SOH;
    for part in msg.split(|&b| b == SOH) {
        if part.starts_with(prefix) {
            return Some(String::from_utf8_lossy(&part[prefix.len()..]).into_owned());
        }
    }
    None
}

fn perm_id_from_fix_order_id(s: &str) -> i64 {
    // Hash only the stable prefix: "00cf16ed.000225ed.69ca0941" (drop ".0001")
    let stable = match s.rmatch_indices('.').next() {
        Some((idx, _)) if s[..idx].contains('.') => &s[..idx],
        _ => s, // no dots or only one segment — hash entire string
    };
    let mut h: u64 = 0xcbf29ce484222325;
    for b in stable.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h >> 1) as i64
}

/// What the venue says went wrong.
///
/// It states the trouble as text and gives it no code and no severity. Nor,
/// for all but a narrow family of requests, does it say which request failed:
/// the vendor's own client shows these in a window and writes them to a log,
/// with nothing to attribute them to. So this reports the text against no
/// request, which is what it is, rather than guessing at an owner for it.
fn handle_venue_error(parsed: &std::collections::HashMap<u32, String>, shared: &SharedState) {
    let text = parsed.get(&58).map(String::as_str).unwrap_or("");
    if text.is_empty() {
        // Nothing said. The vendor's client logs exactly this case and shows
        // nothing, having nothing to show.
        log::warn!("The venue reported trouble and stated nothing about it");
        return;
    }
    // A code-like identifier travels separately from the text where it travels
    // at all, so it is carried along rather than parsed into a number the
    // venue never stated.
    let told = match parsed.get(&149).map(String::as_str).filter(|id| !id.is_empty()) {
        Some(id) => format!("{text} ({id})"),
        None => text.to_string(),
    };
    log::warn!("The venue reported: {told}");
    shared.market.push_venue_error(told);
}

/// Why a message this client receives is deliberately not read.
///
/// Told apart from one nobody has looked at yet. Both are discarded, but only
/// one of them is a gap, and a diagnostic that cannot tell them apart is one
/// nobody keeps listening to.
fn known_unread(subtype: &str) -> Option<&'static str> {
    match subtype {
        "93" => Some(
            "it answers the account and position subscription this client sends, and carries \
             the account, that request's own id and two flags — nothing the subscription does \
             not deliver itself. Named in the vendor's own inventory as a dimension response, \
             which is what it would carry on an account that had dimensions",
        ),
        "194" => Some(
            "it carries the order presets the vendor's own ticket fills its fields from. \
             They are defaults for a user interface, and this client has none: an order \
             here states every field it means",
        ),
        _ => None,
    }
}

/// The algorithms the venue offers, keyed `PROVIDER/SECTYPE`.
///
/// Stated once, unasked, after logon. Nothing here read it, so a caller had no
/// way to know which algorithms this account may use and would find out by
/// having an order refused.
///
/// `FOXRIVER/STK:FOXRIVER-AE,FOXRIVER-AL-COMMON;IBALGO/BAG:IBALGO-AE`
fn parse_algorithms(raw: &str) -> std::collections::HashMap<String, Vec<String>> {
    raw.split(';')
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let (key, names) = entry.split_once(':')?;
            let names: Vec<String> = names
                .split(',')
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect();
            Some((key.to_string(), names))
        })
        .collect()
}

fn handle_algorithms(parsed: &std::collections::HashMap<u32, String>, shared: &SharedState) {
    let Some(raw) = parsed.get(&6597) else { return };
    let offered = parse_algorithms(raw);
    if offered.is_empty() {
        return;
    }
    log::info!("Algorithms offered on {} provider and security type pairs", offered.len());
    shared.reference.set_algorithms(offered);
}

/// The account's own configuration, which states further feature tokens on the
/// same tag the logon used. Read only from the logon, the list is short by
/// whatever this adds.
fn handle_account_config(parsed: &std::collections::HashMap<u32, String>, shared: &SharedState) {
    let Some(raw) = parsed.get(&6542) else { return };
    let more: Vec<String> = raw.split(',').filter(|t| !t.is_empty()).map(str::to_string).collect();
    if more.is_empty() {
        return;
    }
    log::info!("Account configuration states {} further features: {raw}", more.len());
    shared.reference.add_enabled_features(more);
}

/// A market data subscription held back until the venue names its contract.
///
/// The venue answers a subscription only when it is named by contract id, and
/// says nothing at all to one named any other way. A caller who names a
/// contract the ordinary way — symbol, security type, exchange — is owed the
/// lookup that turns it into an id.
#[derive(Debug, Clone)]
pub(crate) struct PendingSubscribe {
    pub(crate) instrument: crate::types::InstrumentId,
    pub(crate) symbol: String,
    pub(crate) exchange: String,
    pub(crate) sec_type: String,
    pub(crate) currency: String,
    pub(crate) last_trade_date: String,
    pub(crate) strike: f64,
    pub(crate) right: String,
    pub(crate) multiplier: String,
    pub(crate) mode_9887: i32,
}

/// The contract a request names, when it carries one.
///
/// Only the requests that must be sent under an id are listed: everything else
/// either carries no contract or is resolved by the venue from the symbol.
pub(crate) fn contract_named(cmd: &crate::types::ControlCommand) -> Option<(&str, &str, &str, &str)> {
    match cmd {
        crate::types::ControlCommand::FetchHistorical { con_id: 0, symbol, sec_type, exchange, currency, .. }
        | crate::types::ControlCommand::FetchHeadTimestamp { con_id: 0, symbol, sec_type, exchange, currency, .. }
        | crate::types::ControlCommand::FetchHistoricalTicks { con_id: 0, symbol, sec_type, exchange, currency, .. }
        | crate::types::ControlCommand::FetchHistoricalSchedule { con_id: 0, symbol, sec_type, exchange, currency, .. }
        | crate::types::ControlCommand::SubscribeRealTimeBar { con_id: 0, symbol, sec_type, exchange, currency, .. }
        | crate::types::ControlCommand::SubscribeDepth { con_id: 0, symbol, sec_type, exchange, currency, .. } => {
            Some((symbol, sec_type, exchange, currency))
        }
        _ => None,
    }
}

/// The filters that go with that contract.
fn filters_named(cmd: &crate::types::ControlCommand) -> crate::types::SecDefFilters {
    match cmd {
        crate::types::ControlCommand::FetchHistorical { filters, .. }
        | crate::types::ControlCommand::FetchHeadTimestamp { filters, .. }
        | crate::types::ControlCommand::FetchHistoricalTicks { filters, .. }
        | crate::types::ControlCommand::FetchHistoricalSchedule { filters, .. }
        | crate::types::ControlCommand::SubscribeRealTimeBar { filters, .. }
        | crate::types::ControlCommand::SubscribeDepth { filters, .. } => filters.clone(),
        _ => crate::types::SecDefFilters::default(),
    }
}

/// Fill in the id the venue has given the contract a request named.
fn name_the_contract(cmd: &mut crate::types::ControlCommand, id: i64) {
    match cmd {
        crate::types::ControlCommand::FetchHistorical { con_id, .. }
        | crate::types::ControlCommand::FetchHeadTimestamp { con_id, .. }
        | crate::types::ControlCommand::FetchHistoricalTicks { con_id, .. }
        | crate::types::ControlCommand::FetchHistoricalSchedule { con_id, .. }
        | crate::types::ControlCommand::SubscribeRealTimeBar { con_id, .. }
        | crate::types::ControlCommand::SubscribeDepth { con_id, .. } => *con_id = id,
        _ => {}
    }
}

/// Which request a caller is waiting on.
fn request_id(cmd: &crate::types::ControlCommand) -> Option<u32> {
    match cmd {
        crate::types::ControlCommand::FetchHistorical { req_id, .. }
        | crate::types::ControlCommand::FetchHeadTimestamp { req_id, .. }
        | crate::types::ControlCommand::FetchHistoricalTicks { req_id, .. }
        | crate::types::ControlCommand::FetchHistoricalSchedule { req_id, .. }
        | crate::types::ControlCommand::SubscribeRealTimeBar { req_id, .. }
        | crate::types::ControlCommand::SubscribeDepth { req_id, .. } => Some(*req_id),
        _ => None,
    }
}

/// Which tag carries a maturity.
///
/// A full expiry date is MaturityDate (541) and a contract month is
/// MaturityMonthYear (200); they are not interchangeable, and an option asked
/// for by date on tag 200 matches nothing at all. Anything too short to be
/// either is left off rather than sent on a guess.
pub(crate) fn maturity_tag(maturity: &str) -> Option<u32> {
    match maturity.len() {
        6 => Some(200),
        n if n >= 8 => Some(541),
        _ => None,
    }
}

/// The update that says an order's state is no longer known. Emitted when the
/// connection drops with it working, and again if the recovery does not
/// account for it.
fn uncertain_update(
    order: &crate::types::Order,
    cached: Option<crate::bridge::RichOrderInfo>,
) -> crate::types::OrderUpdate {
    crate::types::OrderUpdate {
                order_id: order.order_id,
                instrument: order.instrument,
                status: crate::types::OrderStatus::Uncertain,
                filled_qty: order.filled as f64,
                // A fractional order deliberately tracks `qty` as zero — the
                // decimal it was submitted with lives only in the enriched
                // record. Both quantity fields are floating point end to
                // end — the dispatchers already hand them to the callback
                // as f64 — so the fraction itself survives exactly here
                // rather than being rounded to a whole unit.
                remaining_qty: {
                    let outstanding = |total: f64| (total - order.filled as f64).max(0.0);
                    if order.qty > 0 {
                        outstanding(order.qty as f64)
                    } else if let Some(c) = cached.as_ref() {
                        outstanding(c.order.total_quantity)
                    } else {
                        // No exec report has reached this order yet, so
                        // neither its quantity nor a fill is known — both
                        // arrive on the same message — and there is no
                        // honest quantity to give. ibapi's own "value not
                        // set" sentinel, rather than a guessed number.
                        f64::MAX
                    }
                },
                // Nothing here states what it paid, and this update exists to
                // say what is no longer known.
                avg_price: 0,
                perm_id: cached.as_ref().map(|c| c.order.perm_id).unwrap_or(0),
                parent_id: cached.as_ref().map(|c| c.order.parent_id).unwrap_or(0),
                timestamp_ns: 0,
    }
}

/// How long a reconnect waits for the recovery push before judging the orders
/// it did not mention. Generous, because a push that says nothing at all is
/// indistinguishable from one that has not started.
const RECOVERY_PUSH_GRACE: Duration = Duration::from_secs(30);

/// The same wait once the push has sent its own terminator. What is coming has
/// come; this only covers a fill report arriving just behind it.
const RECOVERY_TERMINATOR_GRACE: Duration = Duration::from_secs(2);

pub(crate) struct CcpState {
    pub(crate) seen_exec_ids: HashSet<String>,
    /// Insertion order for `seen_exec_ids`, oldest at the front. Used to evict
    /// one entry at a time once the dedup window is full, instead of clearing
    /// the whole set — a wholesale clear would let a post-reconnect server
    /// replay of a recently-seen ExecID double-count a fill.
    pub(crate) exec_id_order: VecDeque<String>,
    pub(crate) bulletin_next_id: i32,
    /// Live news subscriptions: instrument, request id, and the providers the
    /// caller asked for. The providers are kept because a reconnect has to
    /// send the same request again, and the request is the only place they
    /// appear.
    pub(crate) news_subscriptions: Vec<(InstrumentId, u32, String)>,
    pub(crate) disconnected: bool,
    /// When to account for orders the reconnect did not explain.
    ///
    /// An order that terminated while the connection was down leaves no
    /// message behind — its evidence is an absence, and absence only means
    /// something once the recovery push is known to be complete. Armed
    /// generously at reconnect so the sweep still runs when the push says
    /// nothing at all, and re-armed tightly when the push's own terminator
    /// arrives. Cleared on a disconnect so a second drop before the
    /// sweep cancels it rather than reaping against a dead session.
    pub(crate) recovery_sweep_at: Option<Instant>,
    /// Whether this connection has hydrated an order from the server's account
    /// of what is working. Separates the replay's terminator from the echo that
    /// looks like it.
    pub(crate) hydrated_any: bool,
    /// (req_id, is_single_shot). Single-shot = known-conId lookup whose
    /// first 35=d reply is also the last (server emits no 323=5/6 terminator
    /// for these). Multi-record by-symbol/matching-symbols requests push
    /// `false` and rely on the response-type sentinel for is_last.
    /// In-flight secdef requests: (req_id, single_shot, deadline). The
    /// deadline is swept by `sweep_contract_details` so a request the
    /// gateway never answers (SessionReject, dead socket, lost reply)
    /// surfaces error 200 + contract_details_end instead of hanging
    /// forever.
    pub(crate) pending_secdef: Vec<(u32, bool, Instant)>,
    /// Requests awaiting a matching-symbols reply, with the deadline after
    /// which one is given up on. Recorded only for a request that actually went
    /// out, and expired so a stale head cannot absorb a later reply.
    pub(crate) pending_matching_symbols: Vec<(u32, Instant)>,
    /// In-flight option chain requests: (req_id, symbol, underlying conId,
    /// deadline). The request states no id of its own, so the symbol is what
    /// ties a reply back to it, and the conId is held because the callback
    /// names the underlying the caller asked about.
    pub(crate) pending_option_params: Vec<(u32, String, i64, Instant)>,
    /// keepUpToDate historical queries routed through CCP: (query_id, req_id)
    pub(crate) pending_kut_historical: Vec<(String, u32)>,
    /// tickerId → req_id mapping for keepUpToDate 35=G bar updates
    pub(crate) kut_ticker_map: std::collections::HashMap<u32, u32>,
    /// tickerId → minTick for bar decoding
    pub(crate) kut_min_tick: std::collections::HashMap<u32, f64>,
    /// HMAC signing key for XML-carrying CCP messages (selective signing).
    pub(crate) ccp_sign_key: Vec<u8>,
    /// HMAC signing IV — advances only for signed messages, independent of unsigned ones.
    pub(crate) ccp_sign_iv: std::sync::Mutex<Vec<u8>>,
    /// Secdef replies awaiting paired schedule reply (joined by tag 6256).
    pub(crate) pending_schedule_pair: Vec<PendingSchedulePair>,
    /// Counter for internal schedule subscribe req IDs.
    pub(crate) next_schedule_sub_id: u32,
    /// Fan-out state for by-symbol secdef requests. Each entry tracks the
    /// per-exchange `35=c` requests issued in response to the master
    /// `35=d|320={api_req_id}|6046={list}` reply, and counts the per-exchange
    /// `35=d` replies as they arrive. `contract_details_end` fires for
    /// `api_req_id` once `received >= fanout_req_ids.len()`.
    pub(crate) pending_fanout: Vec<PendingFanout>,
    /// Contracts already handed to each caller's request. A lookup on a
    /// smart-routed symbol is answered once by the request itself and again by
    /// every venue it fans out to, and every one of those answers describes the
    /// same contract with a different `exchange` — the full venue list already
    /// rides inside each. Delivering them all reported one contract as
    /// twenty-seven listings of itself. Cleared when the request ends.
    pub(crate) details_delivered: std::collections::HashMap<u32, HashSet<i64>>,
    /// Counter for internal fan-out req IDs (tag 320 on per-exchange `35=c`).
    pub(crate) next_fanout_id: u32,
    /// Counter for internal secdef req IDs (auto-fetch on cold-cache positions).
    pub(crate) next_internal_secdef_id: u32,
    /// User-message subtypes the venue has sent that nothing here reads, so
    /// each is named once rather than on every arrival.
    unread_subtypes: std::collections::HashSet<String>,
    /// Message types the venue has sent that nothing here reads.
    unread_types: std::collections::HashSet<String>,
    /// Market data subscriptions waiting on the lookup that will name their
    /// contract, keyed by that lookup's request id.
    pub(crate) pending_md_subscribe: Vec<(u32, PendingSubscribe, Instant)>,
    /// Those whose contract the venue has now named.
    pub(crate) resolved_md_subscribe: Vec<(i64, PendingSubscribe)>,
    /// Requests that named a contract the venue has not given an id to yet,
    /// keyed by the lookup asking for that id.
    ///
    /// A subscription is sent by symbol and the venue resolves it. Everything
    /// on the historical farm is asked for by id, so a caller passing the
    /// contract it wrote down — which is what a program written against the
    /// reference client does — sent a request under id zero, and the venue
    /// answered a complete series with nothing in it.
    pub(crate) pending_named: Vec<(u32, crate::types::ControlCommand, Instant)>,
    /// Those the venue has now named, ready to be handled as though the id had
    /// been there all along.
    pub(crate) resolved_named: Vec<crate::types::ControlCommand>,
    /// conIds already auto-fetched a secdef for, keyed by con_id (dedup).
    pub(crate) auto_fetched_conids: HashSet<i64>,
    /// Scanner results awaiting per-conId contract-detail enrichment.
    /// Each entry parks a parsed `<ScanResponse>` until every cache-miss
    /// con_id has been resolved via the same 35=d path that user-initiated
    /// `reqContractDetails` uses.
    pub(crate) pending_scanner_enrichment: Vec<PendingScannerEnrichment>,
}

/// Scanner result parked for contract-detail fan-out.
pub(crate) struct PendingScannerEnrichment {
    pub api_req_id: u32,
    pub result: crate::control::scanner::ScannerResult,
    pub awaiting: HashSet<i64>,
    pub deadline: Instant,
}

/// State for a secdef reply awaiting its paired schedule reply.
pub(crate) struct PendingSchedulePair {
    pub api_req_id: u32,
    pub join_key: String,
    pub def: crate::control::contracts::ContractDefinition,
    pub is_last: bool,
    pub deadline: Instant,
}

/// In-flight by-symbol fan-out: per-exchange `35=c` requests sent after
/// the master `35=d` reply. Each per-exchange `35=d` reply (matched by tag
/// 320 string) is forwarded to `api_req_id` as one `contract_details`.
pub(crate) struct PendingFanout {
    pub api_req_id: u32,
    pub fanout_req_ids: Vec<String>,
    pub received: usize,
    /// Idle deadline, refreshed on every per-exchange reply. One lost or
    /// unparseable fan-out reply out of ~27 previously left the counter
    /// short forever and contract_details_end never fired.
    pub deadline: Instant,
}


/// Every tag the execution-report handler reads.
///
/// Derived from the handler itself so it cannot fall behind as fields are
/// added, the same way a definition's is.
pub fn tags_read_from_an_execution() -> Vec<u32> {
    let source = include_str!("mod.rs");
    let mut seen: Vec<u32> = Vec::new();
    for cap in source.split("parsed.get(&").skip(1) {
        let token: String = cap.chars().take_while(|c| *c != ')').collect();
        let token = token.trim();
        let tag = token.parse::<u32>().ok().or_else(|| {
            let needle = format!("pub const {token}: u32 = ");
            let at = crate::protocol::fix::SOURCE.find(&needle)? + needle.len();
            crate::protocol::fix::SOURCE[at..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        });
        if let Some(tag) = tag
            && !seen.contains(&tag)
        {
            seen.push(tag);
        }
    }
    seen.sort_unstable();
    seen
}

/// What a report stated that nothing here reads, in the order stated.
///
/// Read from the bytes rather than the parsed map: a map holds one value per
/// tag, and a report repeats them.
pub fn unnamed_execution_fields(data: &[u8]) -> Vec<(u32, String)> {
    let read = tags_read_from_an_execution();
    let mut out = Vec::new();
    for part in data.split(|&b| b == crate::protocol::fix::SOH) {
        if part.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(part);
        let Some((tag_str, value)) = text.split_once('=') else { continue };
        let Ok(tag) = tag_str.parse::<u32>() else { continue };
        // The message's own fields are not the fill's.
        if read.contains(&tag) || matches!(tag, 8 | 9 | 10 | 34 | 35 | 43 | 49 | 52 | 56 | 115) {
            continue;
        }
        out.push((tag, value.to_string()));
    }
    out
}

impl CcpState {
    pub(crate) fn new() -> Self {
        Self {
            seen_exec_ids: HashSet::with_capacity(256),
            exec_id_order: VecDeque::with_capacity(256),
            bulletin_next_id: 0,
            news_subscriptions: Vec::new(),
            disconnected: false,
            recovery_sweep_at: None,
            hydrated_any: false,
            pending_secdef: Vec::new(),
            pending_matching_symbols: Vec::new(),
            pending_option_params: Vec::new(),
            pending_kut_historical: Vec::new(),
            kut_ticker_map: std::collections::HashMap::new(),
            kut_min_tick: std::collections::HashMap::new(),
            ccp_sign_key: Vec::new(),
            ccp_sign_iv: std::sync::Mutex::new(Vec::new()),
            pending_schedule_pair: Vec::new(),
            next_schedule_sub_id: 1,
            pending_fanout: Vec::new(),
            details_delivered: std::collections::HashMap::new(),
            next_fanout_id: 1,
            next_internal_secdef_id: 0xF000_0000,
            unread_subtypes: std::collections::HashSet::new(),
            unread_types: std::collections::HashSet::new(),
            pending_md_subscribe: Vec::new(),
            resolved_md_subscribe: Vec::new(),
            pending_named: Vec::new(),
            resolved_named: Vec::new(),
            auto_fetched_conids: HashSet::new(),
            pending_scanner_enrichment: Vec::new(),
        }
    }

    /// Record `exec_id` in the fill-dedup window. Returns `true` if it is new
    /// (the fill should be processed) and `false` if it was already seen (a
    /// duplicate to skip).
    ///
    /// Backed by a bounded rolling window: once `EXEC_ID_WINDOW` IDs are held,
    /// the oldest is evicted one at a time. This replaces a previous wholesale
    /// `clear()` that dropped the entire history at the cap, which let a
    /// post-reconnect server replay of a recently-seen ExecID double-count the
    /// fill and corrupt the position.
    pub(crate) fn record_exec_id(&mut self, exec_id: &str) -> bool {
        if !self.seen_exec_ids.insert(exec_id.to_string()) {
            return false;
        }
        self.exec_id_order.push_back(exec_id.to_string());
        while self.exec_id_order.len() > EXEC_ID_WINDOW {
            if let Some(old) = self.exec_id_order.pop_front() {
                self.seen_exec_ids.remove(&old);
            }
        }
        true
    }

    pub(crate) fn poll_executions(
        &mut self,
        ccp_conn: &mut Option<Connection>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        hb: &mut HeartbeatState,
        account_id: &str,
    ) {
        if self.disconnected { return; }
        let messages = match ccp_conn.as_mut() {
            None => return,
            Some(conn) => {
                match conn.try_recv() {
                    Ok(0) if !conn.has_buffered_data() => return,
                    Ok(0) => {}
                    Err(e) => {
                        log::error!("CCP connection lost: {e}");
                        self.handle_disconnect(context, shared, event_tx);
                        return;
                    }
                    Ok(_) => {
                        hb.last_ccp_recv = Instant::now();
                        // RTT sample: interval from the test request
                        // to the first inbound traffic after it. On a quiet
                        // link (the ping use case) that is the echo itself.
                        if let Some((_, sent_at)) = hb.pending_ccp_test.take() {
                            shared.set_ccp_rtt(hb.last_ccp_recv.duration_since(sent_at));
                        }
                    }
                }
                let frames = conn.extract_frames();
                let mut msgs = Vec::new();
                for frame in frames {
                    match frame {
                        Frame::FixComp(raw) => {
                            let Some(unsigned) = conn.unsign(&raw) else { continue };
                            match fixcomp::fixcomp_decompress(&unsigned) {
                                Ok(inner) => {
                                    if log::log_enabled!(log::Level::Trace) {
                                        for m in &inner {
                                            log::trace!("WIRE< ccp/comp {}", fix::fmt_pipe(m));
                                        }
                                    }
                                    msgs.extend(inner);
                                }
                                Err(e) => {
                                    log::warn!(
                                        "CCP: dropping malformed FIXCOMP frame ({} bytes): {}",
                                        unsigned.len(), e,
                                    );
                                }
                            }
                        }
                        Frame::Fix(raw) => {
                            let Some(unsigned) = conn.unsign(&raw) else { continue };
                            if log::log_enabled!(log::Level::Trace) {
                                log::trace!("WIRE< ccp/fix {}", fix::fmt_pipe(&unsigned));
                            }
                            msgs.push(unsigned);
                        }
                        Frame::Binary(raw) => {
                            let Some(unsigned) = conn.unsign(&raw) else { continue };
                            if log::log_enabled!(log::Level::Trace) {
                                log::trace!("WIRE< ccp/bin {}", fix::fmt_pipe(&unsigned));
                            }
                            msgs.push(unsigned);
                        }
                        Frame::Control(_) => {
                        // 8=1 / 8=X control state — not consumed on the order path.
                        }
                    }
                }
                msgs
            }
        };
        for msg in &messages {
            self.process_ccp_message(msg, ccp_conn, context, shared, event_tx, hb, account_id);
        }
    }

    pub(crate) fn process_ccp_message(
        &mut self,
        msg: &[u8],
        ccp_conn: &mut Option<Connection>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        hb: &mut HeartbeatState,
        account_id: &str,
    ) {
        let parsed = fix::fix_parse(msg);
        // Every message the venue sends carries the time it sent it. Kept for
        // any caller asking what the venue's clock says, which is a different
        // question from what this machine's clock says.
        if let Some(stamped) = parsed.get(&fix::TAG_SENDING_TIME) {
            shared.market.note_venue_time(stamped);
        }
        let msg_type = match parsed.get(&fix::TAG_MSG_TYPE) {
            Some(t) => t.as_str(),
            None => return,
        };
        if std::env::var("IBX_CAPTURE_WIRE").is_ok() {
            let hex: String = msg.iter().map(|b| format!("{b:02x}")).collect();
            shared.market.note_unread_wire("trading-msg", hex);
        }
        match msg_type {
            fix::MSG_EXEC_REPORT => self.handle_exec_report(&parsed, msg, context, shared, event_tx, account_id),
            fix::MSG_CANCEL_REJECT => self.handle_cancel_reject(&parsed, context, shared, event_tx),
            fix::MSG_NEWS => self.handle_news_bulletin(&parsed, shared),
            fix::MSG_HEARTBEAT => {}
            fix::MSG_TEST_REQUEST => {
                let test_id = parsed.get(&fix::TAG_TEST_REQ_ID).cloned().unwrap_or_default();
                if let Some(conn) = ccp_conn.as_mut() {
                    let ts = chrono_free_timestamp();
                    let _ = conn.send_fix(&[
                        (fix::TAG_MSG_TYPE, fix::MSG_HEARTBEAT),
                        (fix::TAG_SENDING_TIME, &ts),
                        (fix::TAG_TEST_REQ_ID, &test_id),
                    ]);
                    hb.last_ccp_sent = Instant::now();
                }
            }
            "3" => {
                let reason = parsed.get(&58).map(|s| s.as_str()).unwrap_or("unknown");
                let ref_tag = parsed.get(&371).map(|s| s.as_str()).unwrap_or("?");
                log::warn!("SessionReject: reason='{reason}' refTag={ref_tag}");
                // A rejection of an in-flight contract-details
                // request was warn-only — the caller saw neither error()
                // nor end (a hang until the sweep, and before that
                // forever). The reject carries no request id, so attribute
                // it only when it cannot be ambiguous: exactly one pending
                // lookup. Otherwise the sweep bounds the damage.
                if self.pending_secdef.len() == 1 && self.pending_fanout.is_empty() {
                    let (req_id, _, _) = self.pending_secdef.remove(0);
                    if req_id < 0xF000_0000 {
                        shared.reference.push_historical_error(
                            req_id, 200,
                            format!("contract details request rejected: {reason}"),
                        );
                        shared.reference.push_contract_details_end(req_id);
                        emit(event_tx, Event::ContractDetailsEnd(req_id));
                    }
                }
            }
            "U" => {
                if let Some(comm) = parsed.get(&6040) {
                    match comm.as_str() {
                        "75" => {
                            // Position + market price feed (init burst + after each fill)
                            self.handle_position_feed(msg, ccp_conn, context, shared, event_tx, hb);
                            shared.portfolio.set_account_download_complete();
                        }
                        "77" => {
                            self.handle_account_summary(&parsed, context, shared);
                            shared.portfolio.set_account_download_complete();
                        }
                        "143" => {
                            // P&L midnight seed — store for client-side daily P&L computation
                            handle_pnl_response(msg, shared);
                        }
                        "152" => handle_pnl_prices(msg, shared),
                        "186" => {
                            if let Some(matches) = crate::control::contracts::parse_matching_symbols_response(msg) {
                                // A 186 frame is the real answer only when it
                                // carries the match-count tag 146 — present
                                // even when the count is zero. Frames without
                                // it are not-ready acks: popping on one would
                                // deliver a bogus empty answer and orphan the
                                // data frame that follows (observed live; the
                                // same ack-then-data shape as the what-if
                                // path).
                                if extract_tag_value(msg, b"146=").is_none() {
                                    log::debug!("matching-symbols ack frame (no tag 146) — awaiting data frame");
                                } else {
                                // Match the reply to its request by the req_id
                                // the server echoes in tag 320, NOT by queue
                                // order: FIFO cross-attributes out-of-order
                                // replies (, same fix as pending_secdef).
                                let echoed = extract_tag_value(msg, b"320=")
                                    .and_then(|v| v.parse::<u32>().ok());
                                let pos = match echoed {
                                    Some(rid) => self.pending_matching_symbols.iter().position(|(p, _)| *p == rid),
                                    // No echo on the wire: attribution is only
                                    // safe with a single request in flight.
                                    None if self.pending_matching_symbols.len() == 1 => Some(0),
                                    None => None,
                                };
                                if let Some(pos) = pos {
                                    let (req_id, _) = self.pending_matching_symbols.remove(pos);
                                    // An empty result is a legitimate answer
                                    // ("no such symbol") and MUST be delivered:
                                    // dropping it left the caller waiting forever
                                    // and the stale queue head misattributed
                                    // every later reply.
                                    shared.reference.push_matching_symbols(req_id, matches);
                                } else {
                                    log::warn!(
                                        "matching-symbols reply not attributable: echoed={:?} pending={:?}",
                                        echoed, self.pending_matching_symbols,
                                    );
                                }
                                }
                            }
                        }
                        // The venue's own error channel. Two subtypes, one
                        // channel: which number it arrives under depends only
                        // on a capability the session negotiated at logon, not
                        // on the error.
                        "192" | "278" => handle_venue_error(&parsed, shared),
                        "81" => handle_algorithms(&parsed, shared),
                        "210" => handle_account_config(&parsed, shared),
                        "139" => self.handle_option_chain(msg, shared),
                        "102" => self.handle_exchange_list(msg, shared),
                        "107" => self.handle_schedule_reply(msg, shared, event_tx),
                        // Something the venue said that nothing here reads.
                        // Dropped in silence it is indistinguishable from the
                        // venue saying nothing, which is how an answer that had
                        // been arriving all along went unnoticed. Named once,
                        // the first time each is seen, so a session that meets
                        // one leaves a record without repeating itself.
                        other => {
                            if self.unread_subtypes.insert(other.to_string()) {
                                match known_unread(other) {
                                    Some(why) => log::debug!("Subtype {other} is not read: {why}"),
                                    None => {
                                        shared.market.note_unread_wire(
                                            "trading", format!("user message {other}"),
                                        );
                                        log::info!(
                                        "Unread user message: subtype {other}. Nothing here reads \
                                         it, so whatever it carries is being discarded"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "W" => {
                // keepUpToDate historical data responses routed through CCP
                if let Some(xml_tag) = parsed.get(&6118) {
                    if let Some(resp) = crate::control::historical::parse_bar_response(xml_tag) {
                        if let Some(pos) = self.pending_kut_historical.iter().position(|(qid, _)| *qid == resp.query_id) {
                            let (_, req_id) = self.pending_kut_historical[pos];
                            shared.reference.push_historical_data(req_id, resp.clone());
                            if resp.is_complete {
                                // Initial batch done — keep entry for streaming
                            }
                        }
                    }
                    else if let Some(ticker_id_str) = crate::control::historical::parse_ticker_id(xml_tag) {
                        let ticker_id: u32 = ticker_id_str.parse().unwrap_or(0);
                        // No unit, no bars: a price counted in a unit nobody
                        // stated is wrong and looks right.
                        let Some(min_tick) =
                            crate::control::historical::min_tick_of(xml_tag, &ticker_id_str)
                        else {
                            return;
                        };
                        // Match ticker to a pending keepUpToDate query
                        for (qid, req_id) in &self.pending_kut_historical {
                            if xml_tag.contains(qid) {
                                self.kut_ticker_map.insert(ticker_id, *req_id);
                                self.kut_min_tick.insert(ticker_id, min_tick);
                                break;
                            }
                        }
                    }
                }
            }
            "G" => {
                // keepUpToDate streaming bar updates (same binary format as rtbar)
                let body = match super::find_body_after_tag(msg, b"35=G\x01") {
                    Some(b) => b,
                    None => return,
                };
                let sig_pos = body.windows(6).position(|w| w == b"\x018349=");
                let body = if let Some(pos) = sig_pos { &body[..pos] } else { body };
                if body.len() >= 11 {
                    let ticker_id = u32::from_be_bytes([body[2], body[3], body[4], body[5]]);
                    let timestamp = u32::from_be_bytes([body[6], body[7], body[8], body[9]]);
                    let payload_len = body[10] as usize;
                    if body.len() >= 11 + payload_len
                        && let Some(&req_id) = self.kut_ticker_map.get(&ticker_id) {
                            // Only the unit the venue stated for this ticker.
                            // Absent, the bar is left rather than decoded as
                            // though it moved in pennies.
                            let Some(&min_tick) = self.kut_min_tick.get(&ticker_id) else {
                                log::warn!(
                                    "bar for ticker {ticker_id} arrived before the venue \
                                     stated what its prices are counted in; not decoded",
                                );
                                return;
                            };
                            let payload = &body[11..11 + payload_len];
                            if let Some(mut bar) = crate::control::historical::decode_bar_payload(payload, min_tick) {
                                bar.timestamp = timestamp;
                                let hist_bar = crate::control::historical::HistoricalBar {
                                    time: format!("{timestamp}"),
                                    open: bar.open,
                                    high: bar.high,
                                    low: bar.low,
                                    close: bar.close,
                                    volume: bar.volume as i64,
                                    wap: bar.wap,
                                    count: bar.count as u32,
                                };
                                let resp = crate::control::historical::HistoricalResponse {
                                    query_id: String::new(),
                                    timezone: String::new(),
                                    bars: vec![hist_bar],
                                    is_complete: true,
                                };
                                shared.reference.push_historical_data(req_id, resp);
                            }
                        }
                }
            }
            // The end of a batch the venue was sending. An account request is
            // otherwise only known to be finished when its rows arrive, and an
            // account holding nothing sends no rows — so a caller waiting on
            // the download would wait for something that was already over.
            "EB" => {
                let ends = parsed.get(&6529).map(String::as_str).unwrap_or("");
                if ends.starts_with("AR") {
                    log::info!("Account request {ends} is complete");
                    shared.portfolio.set_account_download_complete();
                }
            }
            "UT" | "UM" | "RL" => handle_account_update(msg, context, shared),
            // The same figures, for the sets of holdings the account does not
            // hold itself. Applied to the account's own they would overstate
            // what it is worth, so they are kept where the holdings they
            // describe are kept.
            "AL" => handle_account_update_elsewhere(msg, shared, crate::types::HeldElsewhere::Away),
            "UL" => handle_account_update_elsewhere(msg, shared, crate::types::HeldElsewhere::Aside),
            "UP" => handle_position_update(&parsed, context, shared, event_tx),
            // The venue keeps three sets of holdings and this client read one.
            // The others carry the same fields in the same tags — they differ
            // only in which set they belong to — and were discarded, so a
            // caller could not learn the account held anything away at all.
            "AP" => handle_position_elsewhere(&parsed, shared, crate::types::HeldElsewhere::Away),
            "DO" => handle_position_elsewhere(&parsed, shared, crate::types::HeldElsewhere::DisplayOnly),
            "DP" => handle_position_elsewhere(&parsed, shared, crate::types::HeldElsewhere::Aside),
            "d" => {
                let response_req_id = crate::control::contracts::secdef_response_req_id(msg);
                // A reply can describe several contracts: a symbol asked for
                // without a currency is answered with every listing that
                // carries it. Read as one contract it keeps whichever came
                // last, and the venue fan-out then follows that one, so the
                // rest are lost before anything can see them. Deliver them all
                // here; the row the path below delivers is deduplicated
                // against these by contract id.
                {
                    // What a definition carried that nothing here reads. The
                    // point of asking about a contract is to be told about it,
                    // and a field that arrives and is dropped is a fact about
                    // the contract nobody can Recorded rather than guessed
                    // at, so the gap is measurable from a real reply.
                    let unread = crate::control::contracts::unread_definition_tags(msg);
                    if !unread.is_empty() {
                        shared.market.note_unread_wire(
                            "definition",
                            unread.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(","),
                        );
                    }
                    let all = crate::control::contracts::parse_secdef_responses(msg, shared.island_for_nasdaq());
                    // The venue states which venues SMART routes to, in the
                    // order a quote's exchange bitmask refers to. Taking it
                    // replaces this client's own list, whose order was its own
                    // and bore no resemblance to this.
                    if let Some(venues) = all.iter().map(|d| &d.smart_venues).find(|v| !v.is_empty()) {
                        shared.reference.set_smart_components(
                            venues
                                .iter()
                                .enumerate()
                                .map(|(i, exchange)| crate::types::SmartComponent {
                                    bit_number: i as i32,
                                    exchange: exchange.clone(),
                                    exchange_letter: crate::types::exchange_letter(exchange).to_string(),
                                })
                                .collect(),
                        );
                        shared.reference.note_smart_components_provisional(false);
                    }
                    if all.len() > 1
                        && let Some(rid) = response_req_id.as_ref().and_then(|r| r.parse::<u32>().ok())
                        && rid < 0xF000_0000
                    {
                        for def in all.into_iter().filter(|d| d.con_id != 0) {
                            shared.reference.cache_contract(def.con_id as i64, api::Contract {
                                con_id: def.con_id as i64,
                                symbol: def.symbol.clone(),
                                sec_type: def.sec_type.to_api_str().to_string(),
                                exchange: def.exchange.clone(),
                                currency: def.currency.clone(),
                                local_symbol: def.local_symbol.clone(),
                                primary_exchange: def.primary_exchange.clone(),
                                trading_class: def.trading_class.clone(),
                                ..Default::default()
                            });
                            if self.details_delivered.entry(rid).or_default().insert(def.con_id as i64) {
                                let for_event = clone_for_event(event_tx, &def);
                                shared.reference.push_contract_details(rid, def);
                                if let Some(details) = for_event {
                                    emit(event_tx, Event::ContractDetails { req_id: rid, details: Box::new(details) });
                                }
                            }
                        }
                    }
                }

                let fanout_idx = response_req_id.as_ref().and_then(|rid| {
                    self.pending_fanout.iter().position(|p| {
                        p.fanout_req_ids.iter().any(|id| id == rid)
                    })
                });
                if let Some(idx) = fanout_idx {
                    if let Some(def) = crate::control::contracts::parse_secdef_response(msg, shared.island_for_nasdaq()) {
                        let api_req_id = self.pending_fanout[idx].api_req_id;
                        // No con_id is "no definition for this
                        // exchange" — cache nothing and emit no row. The leg
                        // still counts toward the fan-out below, so the
                        // request completes.
                        if def.con_id != 0 {
                            let sec_type_str = def.sec_type.to_api_str();
                            shared.reference.cache_contract(def.con_id as i64, api::Contract {
                                con_id: def.con_id as i64,
                                symbol: def.symbol.clone(),
                                sec_type: sec_type_str.to_string(),
                                exchange: def.exchange.clone(),
                                currency: def.currency.clone(),
                                local_symbol: def.local_symbol.clone(),
                                primary_exchange: def.primary_exchange.clone(),
                                trading_class: def.trading_class.clone(),
                                ..Default::default()
                            });
                            identify_position(shared, &def);
                            identify_position(shared, &def);
                        self.try_release_scanner_enrichments(def.con_id as i64, shared);
                            if self.details_delivered.entry(api_req_id).or_default().insert(def.con_id as i64) {
                                let for_event = clone_for_event(event_tx, &def);
                                shared.reference.push_contract_details(api_req_id, def);
                                if let Some(details) = for_event {
                                    emit(event_tx, Event::ContractDetails { req_id: api_req_id, details: Box::new(details) });
                                }
                            }
                        }
                        // A leg the gateway cannot resolve carries no contract:
                        // it still completes the fan-out, but a zeroed row is
                        // not a listing.
                        self.pending_fanout[idx].received += 1;
                        self.pending_fanout[idx].deadline = Instant::now() + SECDEF_TIMEOUT;
                        if self.pending_fanout[idx].received >= self.pending_fanout[idx].fanout_req_ids.len() {
                            shared.reference.push_contract_details_end(api_req_id);
                            emit(event_tx, Event::ContractDetailsEnd(api_req_id));
                            self.pending_fanout.swap_remove(idx);
                        }
                    }
                    let rules = crate::control::contracts::parse_market_rules(msg);
                    if !rules.is_empty() {
                        shared.reference.push_market_rules(rules);
                    }
                    return;
                }

                if let Some(def) = crate::control::contracts::parse_secdef_response(msg, shared.island_for_nasdaq()) {
                    let is_last_wire = crate::control::contracts::secdef_response_is_last(msg);
                    if def.con_id != 0 {
                        let sec_type_str = def.sec_type.to_api_str();
                        shared.reference.cache_contract(def.con_id as i64, api::Contract {
                            con_id: def.con_id as i64,
                            symbol: def.symbol.clone(),
                            sec_type: sec_type_str.to_string(),
                            exchange: def.exchange.clone(),
                            currency: def.currency.clone(),
                            local_symbol: def.local_symbol.clone(),
                            primary_exchange: def.primary_exchange.clone(),
                            trading_class: def.trading_class.clone(),
                            ..Default::default()
                        });
                        identify_position(shared, &def);
                        self.try_release_scanner_enrichments(def.con_id as i64, shared);
                        // A subscription held back for want of an id. Answered
                        // here rather than alongside the caller-facing rows: a
                        // by-symbol lookup fans out and its rows do not all
                        // take the same path, and this one only needs the first
                        // definition that names the contract.
                        if let Some(rid) = response_req_id.as_ref().and_then(|r| r.parse::<u32>().ok())
                            && let Some(at) = self.pending_md_subscribe.iter().position(|(pid, ..)| *pid == rid)
                        {
                            let (_, pending, _) = self.pending_md_subscribe.remove(at);
                            self.resolved_md_subscribe.push((def.con_id as i64, pending));
                        }
                        // And a request held for the same reason.
                        if let Some(rid) = response_req_id.as_ref().and_then(|r| r.parse::<u32>().ok())
                            && let Some(at) = self.pending_named.iter().position(|(pid, ..)| *pid == rid)
                        {
                            let (_, mut cmd, _) = self.pending_named.remove(at);
                            name_the_contract(&mut cmd, def.con_id as i64);
                            self.resolved_named.push(cmd);
                        }
                    }
                    // Match the response to its originating pending_secdef entry
                    // by tag 320 (response_req_id). Without this, an internal
                    // auto-fetch reply (e.g. position-driven secdef for SPY)
                    // landing while a user request is in flight would be
                    // attributed to `pending_secdef.first()` and leak as a
                    // bogus contract_details callback on the user's req_id.
                    let matched_idx: Option<usize> = response_req_id.as_ref()
                        .and_then(|rid| rid.parse::<u32>().ok())
                        .and_then(|rid_u32| {
                            self.pending_secdef.iter().position(|(pid, _, _)| *pid == rid_u32)
                        });
                    let single_shot = matched_idx
                        .map(|i| self.pending_secdef[i].1).unwrap_or(false);
                    let is_by_symbol = matched_idx
                        .map(|i| !self.pending_secdef[i].1).unwrap_or(false);
                    let is_last = is_last_wire || single_shot;
                    // Fan-out detection: by-symbol master reply carries the full
                    // exchange list in tag 6046. Drop SMART/BEST and dispatch
                    // one per-exchange `35=c` per remaining entry. The per-
                    // exchange replies arrive on new req_ids and route through
                    // the `pending_fanout` branch above.
                    let fanout_exchanges: Vec<String> = if is_by_symbol && !is_last_wire {
                        def.valid_exchanges.iter()
                            .filter(|e| !matches!(e.as_str(), "" | "SMART" | "BEST"))
                            .cloned()
                            .collect()
                    } else {
                        Vec::new()
                    };
                    if let Some(idx) = matched_idx {
                        let req_id = self.pending_secdef[idx].0;
                        // Internal sentinel req_ids (auto-fetch for cold-cache
                        // positions, scanner enrichment) start at 0xF000_0000.
                        // Their replies must populate the contract cache but
                        // never surface as user-visible contract_details
                        // callbacks.
                        let is_internal = req_id >= 0xF000_0000;
                        let join_key = def.join_key.clone();
                        if is_last {
                            self.pending_secdef.remove(idx);
                        }
                        let con_id = def.con_id as i64;
                        if con_id == 0 {
                            // Con_id=0 is the gateway saying "no
                            // security definition", not a contract. Pushed as
                            // a row it is indistinguishable from a hit —
                            // empty symbol, and min_tick carrying its 0.01
                            // default. Report it the way the reject and
                            // timeout paths do. The by-symbol leg gets its
                            // end from the fan-out branch below.
                            // The same reply also answers a symbol the gateway
                            // cannot resolve, which arrives contract-less (live:
                            // "BRK.A" for the "BRK A" listing). Drop the pending
                            // entry so the single-shot leg is not left parked
                            // waiting for a definition that will not come.
                            self.pending_secdef.retain(|(rid, ss, _)| *rid != req_id || *ss);
                            if !is_internal {
                                shared.reference.push_historical_error(
                                    req_id, 200,
                                    "No security definition has been found for the request".to_string(),
                                );
                                // Unconditional: the pending entry was dropped
                                // just above, so the fan-out branch below can no
                                // longer supply the end for the by-symbol leg and
                                // a caller blocked on it would wait forever.
                                shared.reference.push_contract_details_end(req_id);
                                emit(event_tx, Event::ContractDetailsEnd(req_id));
                            }
                        } else if join_key.is_empty() {
                            // No join key — emit immediately without schedule data.
                            if !is_internal
                                && self.details_delivered.entry(req_id).or_default().insert(def.con_id as i64)
                            {
                                let for_event = clone_for_event(event_tx, &def);
                                shared.reference.push_contract_details(req_id, def);
                                if let Some(details) = for_event {
                                    emit(event_tx, Event::ContractDetails { req_id, details: Box::new(details) });
                                }
                                if is_last {
                                    shared.reference.push_contract_details_end(req_id);
                                    emit(event_tx, Event::ContractDetailsEnd(req_id));
                                }
                            }
                        } else if is_internal {
                            // Skip schedule pairing for internal sentinels — no
                            // user is awaiting the trading_hours enrichment.
                        } else {
                            self.pending_schedule_pair.push(PendingSchedulePair {
                                api_req_id: req_id,
                                join_key: join_key.clone(),
                                def,
                                is_last,
                                deadline: Instant::now() + std::time::Duration::from_secs(3),
                            });
                            self.send_schedule_subscribe(&join_key, ccp_conn, hb);
                        }
                        // Dispatch fan-out (or fire end immediately if the
                        // symbol resolves to a single exchange and there's
                        // nothing to fan out to).
                        if is_by_symbol && !is_last_wire && con_id != 0 {
                            self.pending_secdef.retain(|(rid, ss, _)| *rid != req_id || *ss);
                            if fanout_exchanges.is_empty() {
                                // The master row may be parked awaiting its
                                // schedule pair; firing end now would order
                                // end BEFORE the row. Defer it to
                                // the pair's resolution (or its 3s sweep).
                                if let Some(pair) = self.pending_schedule_pair.iter_mut()
                                    .find(|p| p.api_req_id == req_id)
                                {
                                    pair.is_last = true;
                                } else {
                                    shared.reference.push_contract_details_end(req_id);
                                    emit(event_tx, Event::ContractDetailsEnd(req_id));
                                }
                            } else {
                                let mut fanout_req_ids = Vec::with_capacity(fanout_exchanges.len());
                                for exch in &fanout_exchanges {
                                    let fid = format!("ibxfan-{}-{}", req_id, self.next_fanout_id);
                                    self.next_fanout_id = self.next_fanout_id.wrapping_add(1);
                                    let fix_exch = if exch == "SMART" { "BEST" } else { exch.as_str() };
                                    self.send_fanout_secdef_request(&fid, con_id, fix_exch, ccp_conn, hb);
                                    fanout_req_ids.push(fid);
                                }
                                log::info!(
                                    "Secdef by-symbol fan-out: api_req_id={} con_id={} exchanges={}",
                                    req_id, con_id, fanout_req_ids.len(),
                                );
                                self.pending_fanout.push(PendingFanout {
                                    api_req_id: req_id,
                                    fanout_req_ids,
                                    received: 0,
                                                            deadline: Instant::now() + SECDEF_TIMEOUT,
                                });
                            }
                        }
                    }
                }
                let rules = crate::control::contracts::parse_market_rules(msg);
                if !rules.is_empty() {
                    shared.reference.push_market_rules(rules);
                }
            }
            // A message type nothing here reads. Named once, like an unread
            // user message: the out-of-band types carry position and account
            // data, and the vendor's own client treats an unrecognised one as
            // an error rather than as nothing.
            other => {
                if self.unread_types.insert(other.to_string()) {
                    shared.market.note_unread_wire("trading", format!("type {other}"));
                    log::info!(
                        "Unread message: type {other}, {} bytes. Nothing here reads it, so \
                         whatever it carries is being discarded",
                        msg.len(),
                    );
                }
            }
        }
    }

    fn handle_exec_report(
        &mut self,
        parsed: &std::collections::HashMap<u32, String>,
        raw: &[u8],
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        account_id: &str,
    ) {
        // CCP recovery push format A (, captured against live):
        // 35=8 with 150=0/39=0, tag 11 carries `<permId>.0`, the originating
        // orderId is in tag 6121. For these, prefer 6121 as the local key so
        // cancel_order(<prior-session orderId>) finds the right ClOrdID.
        // Format B (paper account, observed live): tag 11 carries the
        // originating orderId directly with `.0` suffix, tags 6119/6121
        // absent — the existing tag-11 split below already gives the right
        // value. The unwrap_or_else fallback handles both.
        let recovery_origin_order_id: Option<u64> = if parsed.get(&150).map(|s| s.as_str()) == Some("0")
            && parsed.get(&39).map(|s| s.as_str()) == Some("0")
            && parsed.contains_key(&6121)
        {
            parsed.get(&6121).and_then(|s| s.parse::<u64>().ok())
        } else {
            None
        };

        let clord_id = recovery_origin_order_id.unwrap_or_else(|| {
            parsed.get(&11).and_then(|s| {
                // A cancel names the order with a leading C, and a position the
                // broker liquidated with a leading L. Only the first was taken
                // off, so every report on a liquidated position parsed to no
                // order at all and the fill reached nobody: a forced
                // liquidation was the one fill a caller could not
                let stripped = s.strip_prefix('C').or_else(|| s.strip_prefix('L')).unwrap_or(s);
                // Strip versioned suffix (.0, .1, .2) from modify-chained ClOrdIDs
                let base = stripped.split('.').next().unwrap_or(stripped);
                base.parse::<u64>().ok()
            }).unwrap_or(0)
        });

        // Recovery insert: a 35=8 with status New/New (150=0/39=0) for an order
        // that is NOT in this session's context is a cross-session recovery entry
        // pushed by CCP on session establishment. Insert into context.open_orders
        // so subsequent cancel/modify ACKs at ~line 668 can match via
        // context.order(clord_id) and emit OrderUpdate events to the user.
        let is_new_ack = parsed.get(&150).map(|s| s.as_str()) == Some("0")
            && parsed.get(&39).map(|s| s.as_str()) == Some("0");
        // The sentinel is dropped further down, but this recovery insert runs
        // first — without the guard, a `11='*'` terminator registers a conId
        // and inserts the reserved order id 0 before being "discarded".
        // An order whose state is not known is also hydrated from this echo,
        // not just an absent one. A replace overwrites the tracked record
        // before it goes out, so a replace that failed left the attempted
        // definition in place; the server's account of the order is the
        // authority and replaces it. Anything with a status the engine still
        // believes is left alone.
        // What the engine already holds for this order, where it holds
        // anything. The push states what the broker has and omits the rest —
        // tag 59 among them — so an unstated field keeps what was known rather
        // than taking a default meant for an order this session never saw.
        let prior = context.order(clord_id)
            .filter(|o| o.status == crate::types::OrderStatus::Uncertain)
            .copied();
        let unknown = prior.is_some();
        // An order that already finished this session is not brought back by a
        // frame that arrives behind it. The gateway echoes a working status
        // after a fill, and the tracked record is gone by then — retired when
        // the order finished — so its absence reads as "never seen" and the
        // echo would insert it as live, with none of the fill on it.
        let already_finished = shared.orders.recently_completed(clord_id);
        if is_new_ack && clord_id != 0 && !already_finished
            && (context.order(clord_id).is_none() || unknown)
        {
            let con_id: i64 = parsed.get(&6008).and_then(|s| s.parse().ok()).unwrap_or(0);
            // The side has to be stated. A guess does not stay in the recovered
            // record: every later fill for the order books through the tracked
            // path and takes its side from here, so a recovered buy recorded as
            // a sell moves the position down by the filled quantity instead of
            // up — wrong by twice the fill, and indistinguishable afterwards
            // from a side the report actually carried.
            let side = match parsed.get(&54).map(|s| s.as_str()) {
                Some("1") => Some(Side::Buy),
                Some("2") => Some(Side::Sell),
                Some("5") => Some(Side::ShortSell),
                other => {
                    // The sentinel that terminates a recovery burst, and the
                    // mass-status echo, both parse to id 0 and carry no side.
                    // Warning about those once per connect would cry wolf on
                    // the one signal that matters when a real record is
                    // refused.
                    if clord_id != 0 {
                        log::warn!(
                            "Recovery record for order {clord_id} has Side={other:?}; not tracking it",
                        );
                    }
                    None
                }
            };
            let qty: u32 = parsed.get(&38).and_then(|s| s.parse::<f64>().ok()).map(|q| q as u32)
                .unwrap_or_else(|| prior.map_or(0, |o| o.qty));
            let limit_price_i64: i64 = parsed.get(&44)
                .and_then(|s| s.parse::<f64>().ok())
                .map(|p| (p * PRICE_SCALE as f64) as i64)
                .unwrap_or_else(|| prior.map_or(0, |o| o.price));
            let stop_price_i64: i64 = parsed.get(&99)
                .and_then(|s| s.parse::<f64>().ok())
                .map(|p| (p * PRICE_SCALE as f64) as i64)
                .unwrap_or_else(|| prior.map_or(0, |o| o.stop_price));
            let ord_type_byte: u8 = parsed.get(&40).and_then(|s| s.bytes().next())
                .unwrap_or_else(|| prior.map_or(b'2', |o| o.ord_type));
            // A recovery record with no tag 59 states no time-in-force, and this
            // order was not placed by this session, so there is nothing to
            // recover it from. Recorded as unstated rather than guessed: either
            // guess is restated as a real instruction on the next replace, and
            // an invented DAY would expire a resting GTC order at the close.
            let tif_byte: u8 = parsed.get(&59)
                .and_then(|s| s.bytes().next())
                .unwrap_or_else(|| prior.map_or(crate::types::TIF_UNSTATED, |o| o.tif));
            if let (Some(side), true) = (side, con_id != 0 && qty > 0) {
                // Recovery is fed by gateway frames, so a full instrument
                // table must degrade to a missing order rather than take the
                // engine down. The reconnect burst replays every
                // resting order, which is exactly when the table fills.
                // Skipping only the insert keeps the order in last_clord and
                // the rich-order cache, so req_open_orders still shows it —
                // but it is NOT in the engine book, so a later fill or
                // terminal status for it is dropped and no OrderUpdate
                // reaches the caller. A missing order beats taking
                // the engine down; it is not a complete answer.
                match context.try_register_instrument(con_id) {
                    None => log::warn!(
                        "recovery: instrument table full, order clord={clord_id} con_id={con_id} not tracked in the engine book",
                    ),
                    Some(instrument) => {
                if let Some(sym) = parsed.get(&55) {
                    context.set_symbol(instrument, sym.clone());
                }
                context.insert_order(crate::types::Order {
                    order_id: clord_id,
                    instrument,
                    side,
                    price: limit_price_i64,
                    qty,
                    // Seeded from the recovery push rather than assumed zero.
                    // Without it a fresh process believes nothing has filled,
                    // and the replayed executions behind this record all look
                    // like new quantity.
                    filled: parsed.get(&14)
                        .and_then(|s| s.parse::<f64>().ok())
                        .map(|c| c as u32)
                        .unwrap_or_else(|| prior.map_or(0, |o| o.filled)),
                    // An order this session never saw is working by the fact of
                    // being in the push. One whose state was not known stays
                    // not known here, so the status this very message carries
                    // moves it, and the caller who was told it was unknown is
                    // told what it is.
                    status: if prior.is_some() {
                        crate::types::OrderStatus::Uncertain
                    } else {
                        crate::types::OrderStatus::Submitted
                    },
                    ord_type: ord_type_byte,
                    tif: tif_byte,
                    stop_price: stop_price_i64,
                });
                self.hydrated_any = true;
                log::info!("CCP recovery: inserted orderId={} sym={:?} side={:?} qty={} px={}",
                    clord_id, parsed.get(&55), side, qty,
                    limit_price_i64 as f64 / PRICE_SCALE as f64);
                // Published, not just tracked. The engine knowing an order is
                // working does the caller no good on its own: `req_open_orders`
                // reads what has been published, so an order the server named
                // at connect went unreported until some later message about it
                // happened to arrive. A caller asking what it already has on,
                // at the moment it starts, was told nothing.
                let sec_type_str = context.market.order_routing(instrument).0;
                shared.orders.push_order_info(clord_id, crate::bridge::RichOrderInfo {
                    contract: api::Contract {
                        con_id,
                        symbol: parsed.get(&55).cloned().unwrap_or_default(),
                        sec_type: sec_type_str,
                        currency: parsed.get(&15).cloned().unwrap_or_default(),
                        ..Default::default()
                    },
                    order: api::Order {
                        order_id: clord_id as i64,
                        action: match side {
                            Side::Buy => "BUY".to_string(),
                            _ => "SELL".to_string(),
                        },
                        total_quantity: qty as f64,
                        order_type: crate::types::ord_type_fix_str(ord_type_byte).to_string(),
                        lmt_price: limit_price_i64 as f64 / PRICE_SCALE as f64,
                        aux_price: stop_price_i64 as f64 / PRICE_SCALE as f64,
                        account: parsed.get(&1).cloned().unwrap_or_default(),
                        ..Default::default()
                    },
                    order_state: api::OrderState {
                        status: "Submitted".to_string(),
                        ..Default::default()
                    },
                    last_exec: Default::default(),
                });
                    }
                }
            }
        }

        // Drop the sentinel/end-of-stream record (ClOrdID="*"/"0"/absent → parses
        // to 0). Real orders are assigned monotonic IDs via next_order_id and
        // never collide with 0. The recovery-push terminator (11='*') lands here.
        if clord_id == 0 {
            log::debug!("ExecReport: dropping sentinel record (ClOrdID=0/*) sym={:?} status={:?}",
                parsed.get(&55), parsed.get(&39));
            // Everything already working has now been named. The same record
            // shape also carries a mass-status echo that arrives before any
            // order, so this only counts once at least one has come through —
            // otherwise a caller is told the replay is over before it starts.
            if self.hydrated_any {
                shared.orders.set_replay_done();
            }
            // The push said everything it was going to say, so the orders it
            // left out can be judged without waiting out the whole grace.
            if self.recovery_sweep_at.is_some() {
                self.recovery_sweep_at = Some(Instant::now() + RECOVERY_TERMINATOR_GRACE);
            }
            return;
        }

        // Record the ClOrdID exactly as the server reports it so subsequent
        // cancel/modify can echo back the same string. Skip cancel-ack frames
        // (tag 11 starts with 'C' there) — those carry the cancel request's
        // own id, not the original order's.
        if let Some(raw_clord) = parsed.get(&11)
            && !raw_clord.starts_with('C') && raw_clord != "*" {
                context.last_clord.insert(clord_id, raw_clord.clone());
            }

        // What-If response: tag 6091=1 with margin data (tag 6092+).
        // The gateway emits a not-ready ack frame whose margin fields carry the
        // literal string "n/a" (parse fails), then a data frame with numbers.
        // Discriminate on parse-success, NOT positivity: a margin-reducing
        // preview (closing a position, cash-account sell) legitimately resolves
        // to init_margin_after == 0, and the gateway sends that as a numeric "0"
        // which must be delivered. Guarding on `> 0.0` silently dropped those
        // and left the caller's pending what-if to time out.
        // The not-ready ack is not always emitted — close/reject previews send a
        // single data frame — so accept the first data frame with no assumption
        // that an ack precedes it. A frame is the real preview when ANY of the
        // six margin fields (6826/6827/6828 before, 6092/6093/6094 after)
        // parses as a finite number, mirroring the gateway's own real-frame
        // test: each field is "set" when it parses, unset on nan/unparseable,
        // and the frame is real when any field is set. The
        // ack carries "n/a" in all six, so it never matches. Captured
        // byte-level in.
        if parsed.get(&6091).map(|s| s.as_str()) == Some("1") {
            const MARGIN_TAGS: [u32; 6] = [6826, 6827, 6828, 6092, 6093, 6094];
            let is_data_frame = MARGIN_TAGS.iter().any(|tag| {
                parsed.get(tag)
                    .and_then(|s| s.parse::<f64>().ok())
                    .is_some_and(|f| f.is_finite())
            });
            if is_data_frame
                && let Some(order) = context.order(clord_id).copied() {
                    let response = crate::types::WhatIfResponse {
                        order_id: clord_id,
                        instrument: order.instrument,
                        init_margin_before: parse_price_tag(parsed.get(&6826)),
                        maint_margin_before: parse_price_tag(parsed.get(&6827)),
                        equity_with_loan_before: parse_price_tag(parsed.get(&6828)),
                        init_margin_after: parse_price_tag(parsed.get(&6092)),
                        maint_margin_after: parse_price_tag(parsed.get(&6093)),
                        equity_with_loan_after: parse_price_tag(parsed.get(&6094)),
                        commission: parse_price_tag(parsed.get(&6378)),
                        min_commission: parse_price_tag(parsed.get(&6379)),
                        max_commission: parse_price_tag(parsed.get(&6380)),
                        commission_currency: parsed.get(&6381).cloned().unwrap_or_default(),
                        // The venue's own warning, which rides its own tag and
                        // not the order's text.
                        warning_text: parsed.get(&6361).cloned().unwrap_or_default(),
                    };
                    log::info!("WhatIf response: clord={} initMargin={:.2}->{:.2} commission={:.2}",
                        clord_id,
                        response.init_margin_before as f64 / PRICE_SCALE as f64,
                        response.init_margin_after as f64 / PRICE_SCALE as f64,
                        response.commission as f64 / PRICE_SCALE as f64);
                    context.retire_order(clord_id);
                    shared.orders.push_what_if(response.clone());
                    emit(event_tx, Event::WhatIf(response));
                }
            // A preview the venue refuses has no margin figures to state, so it
            // arrives shaped exactly like the not-ready ack — every field "n/a"
            // — and says why on 58 instead. Returning here on that shape threw
            // the reason away and left the caller waiting out a preview that
            // was never coming. A refusal falls through and is reported like
            // any other.
            if parsed.get(&39).map(|s| s.as_str()) != Some("8") {
                return;
            }
        }

        let ord_status = parsed.get(&39).map(|s| s.as_str()).unwrap_or("");
        let exec_type = parsed.get(&150).map(|s| s.as_str()).unwrap_or("");
        let exec_id = parsed.get(&17).map(|s| s.as_str()).unwrap_or("");
        let last_px = parsed.get(&31).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let last_shares = parsed.get(&32).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        // Absent is not zero. Without 151 the caller was told nothing was left
        // on an order that was still working; the terminal falls back to the
        // order quantity less what has filled, and so does this.
        let leaves_qty = parsed.get(&151).and_then(|s| s.parse::<i64>().ok()).unwrap_or_else(|| {
            let ordered = parsed.get(&38).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let done = parsed.get(&14).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            ((ordered - done).max(0.0)) as i64
        });
        // 14 CumQty and 6 AvgPx describe the order as a whole; 32 and 31
        // describe this print alone. The gateway sends all four on every
        // execution report.
        //
        // When the cumulative quantity is absent, the print alone is not a
        // substitute: on the second fill of an order it is smaller than what
        // was already reported, so `filled` would go backwards. Add the print
        // to what the order has already accumulated instead. The average price
        // is not reconstructible that way, so it falls back to the print — and
        // a negative average is a real value for a spread, so only an absent
        // or unparseable tag falls back at all.
        let order_cum_qty = parsed.get(&14).and_then(|s| s.parse::<f64>().ok())
            .map(|q| q as i64)
            .filter(|q| *q > 0)
            .unwrap_or_else(|| {
                context.order(clord_id).map_or(last_shares, |o| o.filled as i64 + last_shares)
            });
        let order_avg_px = parsed.get(&6)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(last_px);
        let commission = parsed.get(&12).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);

        if ord_status == "8" {
            // The venue says why it refused an order, and that was written to a
            // log where no caller could read it. A caller then saw only that
            // the order was not working, with nothing to act on, which is the
            // one thing the venue's own client never does.
            let reason = stated_reason(parsed);
            log::warn!("ExecReport REJECTED: clord={clord_id} reason='{reason}'");
            if !reason.is_empty() {
                shared.orders.push_order_inactive(clord_id, ORDER_INACTIVE_ERROR_CODE, reason);
            }
        } else {
            log::info!("ExecReport: 39={} 150={} 11={} 58={} 103={}",
                ord_status, exec_type, clord_id,
                parsed.get(&58).map(|s| s.as_str()).unwrap_or(""),
                parsed.get(&103).map(|s| s.as_str()).unwrap_or(""));
        }

        let status = match ord_status {
            "0" => {
                // 39=0 is New on the wire, but the gateway reports PreSubmitted
                // until the order is actually routed to and acknowledged by an
                // exchange (for example a limit order resting pre-market). Routing
                // shows up on the same exec report as a non-empty ExDestination
                // (tag 100) plus an exec ref (tag 198) other than "NONE"; before
                // routing both are absent/"NONE". Captured in.
                let routed = parsed.get(&100).is_some_and(|s| !s.is_empty())
                    || parsed.get(&198).is_some_and(|s| s != "NONE" && !s.is_empty());
                if routed {
                    crate::types::OrderStatus::Submitted
                } else {
                    crate::types::OrderStatus::PreSubmitted
                }
            }
            "5" => crate::types::OrderStatus::Submitted,
            "A" => crate::types::OrderStatus::PreSubmitted,
            "E" => crate::types::OrderStatus::PendingReplace,
            "6" => crate::types::OrderStatus::PendingCancel,
            "1" => crate::types::OrderStatus::PartiallyFilled,
            "2" => crate::types::OrderStatus::Filled,
            "4" | "C" => crate::types::OrderStatus::Cancelled,
            // Not cancelled. The terminal groups D with pending-cancel and its
            // own "is this terminal" test names only 2, 4, C and 8 — reading it
            // as cancelled retired an order that was still working.
            "D" => crate::types::OrderStatus::PendingCancel,
            "8" => crate::types::OrderStatus::Rejected,
            "I" => crate::types::OrderStatus::Inactive,
            other => {
                // A status this does not know is not a reason to drop the
                // report: it may carry a fill, and returning here threw the
                // fill away with it. Say so and carry on to the execution.
                log::warn!("Unknown order status 39={other} for order {clord_id} — \
                            the report is still read for its execution");
                crate::types::OrderStatus::Uncertain
            }
        };


        // A replace is acknowledged as 39=5, and the gateway reaches it through
        // 39=6 first: captured live, a modify runs PendingCancel then Replaced.
        // The monotonic guard ranks PendingCancel above the working states, so
        // that acknowledgement reads as a stale frame and is dropped, leaving a
        // successfully modified order reported as stuck mid-cancel and never
        // confirmed. It is a deliberate transition, which is what the forced
        // path is for; the ranks are left alone because a partially filled
        // order must still be able to reach PendingCancel.
        // A report can carry the reason it restates the order, and two of those
        // reasons are refusals: the gateway answers a revision it would not make
        // and a cancel it would not make on the same message it answers a
        // successful one. Read as an acknowledgement, a refused revision left
        // the caller believing an order had been changed that had not been.
        let restatement_reason = parsed.get(&378).map(|s| s.as_str()).unwrap_or("");
        let revision_refused = matches!(restatement_reason, "102" | "103");
        let is_replace_ack = ord_status == "5" && !revision_refused;
        if revision_refused {
            // The order stands as it was, so it has no new status to report —
            // but the caller asked for a change and has to learn it did not
            // happen. Reported the way a refused order is, on the channel a
            // caller already watches, rather than only to a log.
            let reason = stated_reason(parsed);
            log::warn!(
                "Order {clord_id}: the gateway refused the request (378={restatement_reason}) — \
                 the order stands as it was: {reason}",
            );
            let told = if reason.is_empty() {
                "the venue refused the change and the order stands as it was".to_string()
            } else {
                reason
            };
            shared.orders.push_order_inactive(clord_id, ORDER_INACTIVE_ERROR_CODE, told);
        }
        if is_replace_ack {
            context.set_order_status_forced(clord_id, status);
        }

        // The guard's verdict doubles as the change flag: a stale
        // frame the guard rejects must not surface as an order_status either.
        // A refusal states no new status for the order — it says the request
        // was not carried out, and the order goes on under the terms it already
        // had. Any execution the report carries is still read below.
        let status_changed = !revision_refused
            && (is_replace_ack || context.update_order_status(clord_id, status));

        // The gateway marks a report that restates history: 97=Y is PossResend
        // and 43=Y is PossDupFlag. Neither was read anywhere, and the only
        // thing standing between a replayed execution and a second booking was
        // the ExecID window — which a fresh process does not have, because it
        // has never seen the ID. At session start the gateway replays
        // recent executions, so a restart with open partially-filled orders
        // emitted a fill for something that happened before it started.
        let is_resend = ["Y", "y"].contains(&parsed.get(&97).map(|v| v.as_str()).unwrap_or(""))
            || ["Y", "y"].contains(&parsed.get(&43).map(|v| v.as_str()).unwrap_or(""));
        // A report can also undo or restate an execution rather than announce a
        // new one: a busted trade and a corrected one both arrive as executions,
        // and adding their quantity booked a fill the account no longer has.
        // The cumulative figure is the truth on those, which is the same
        // arithmetic a replayed execution needs.
        let trans_type = parsed.get(&20).map(|s| s.as_str()).unwrap_or("");
        let is_resend = is_resend || matches!(trans_type, "1" | "2");

        // CumQty — the order's cumulative filled quantity as of this report.
        let report_cum_qty = parsed.get(&14)
            .and_then(|s| s.parse::<f64>().ok())
            .map(|c| c as i64)
            .unwrap_or(0);

        // Dedup key. An execution with no ExecID skipped the window entirely,
        // so a replayed copy booked a second time — and an absent tag 17 is the
        // shape a replay takes, which is precisely when the window matters
 //. Falling back to the fields that identify an execution
        // dedups it on its content instead of trusting it.
        //
        // CumQty is what separates two otherwise identical slices: it advances
        // with every execution on the order, including across a replacement
        // that raised the total, where LastShares, price, LeavesQty and the
        // timestamp tick can all repeat.
        let dedup_key = if exec_id.is_empty() {
            format!(
                "{}|{}|{}|{}|{}",
                clord_id,
                parsed.get(&60).map(|s| s.as_str()).unwrap_or(""),
                last_shares,
                last_px,
                report_cum_qty,
            )
        } else {
            exec_id.to_string()
        };

        if matches!(exec_type, "F" | "1" | "2") && last_shares > 0 {
            // A fill can arrive for an order this session does not track: one
            // that raced its own cancel-ack out of the book, one placed from
            // another client, or one left from an earlier session. The report
            // names the contract and the side, so book it from that rather
            // than dropping a position the account actually holds. An untracked
            // order has nothing filled yet, so the arithmetic below reconciles
            // against zero.
            let target = match context.order(clord_id).copied() {
                Some(order) => Some((order.instrument, order.side, order.filled as i64)),
                None => untracked_fill_target(context, parsed).map(|(i, s)| (i, s, 0i64)),
            };
            if let Some((instrument, side, already_filled)) = target {
                let booked = if is_resend {
                    // Recorded even though the cumulative figure is what decides
                    // this copy: the same execution can arrive again without its
                    // marker, and the window is what catches that one. Recorded
                    // here rather than earlier so an execution that reaches this
                    // handler before its order does is not spent on a delivery
                    // that had nothing to book against.
                    self.record_exec_id(&dedup_key);
                    if report_cum_qty <= 0 {
                        // Nothing to reconcile against. Booking the increment
                        // would double what the recovery record already seeded.
                        log::debug!("Resent execution for order {clord_id} carries no CumQty — not booked");
                        0
                    } else {
                        let delta = (report_cum_qty - already_filled).max(0);
                        if delta != last_shares && delta > 0 {
                            // The report's own increment is not what this client
                            // is missing, so the fill that follows carries a
                            // reconciled quantity at this report's price rather
                            // than one execution's own terms. The order's total
                            // and the position are right; the execution record
                            // is approximate, and says so here.
                            log::warn!(
                                "Resent execution for order {clord_id}: booking {delta} to reach CumQty {report_cum_qty} \
                                 (report states {last_shares}) — execution detail is reconciled, not exact",
                            );
                        }
                        delta
                    }
                } else if !self.record_exec_id(&dedup_key) {
                    // A duplicate suppresses the fill and nothing else: the
                    // report still carries a status to apply and terminal
                    // bookkeeping to run, and returning here skipped both.
                    log::warn!("Duplicate execution key={dedup_key} — the fill is already booked");
                    0
                } else {
                    last_shares
                };
                if booked > 0 {
                    context.update_order_filled(clord_id, booked as u32);
                    let fill = Fill {
                        instrument,
                        order_id: clord_id,
                        side,
                        price: (last_px * PRICE_SCALE as f64) as i64,
                        qty: booked,
                        remaining: leaves_qty,
                        commission: (commission * PRICE_SCALE as f64) as i64,
                        timestamp_ns: context.now_ns(),
                        cum_qty: order_cum_qty,
                        avg_price: (order_avg_px * PRICE_SCALE as f64) as i64,
                    };
                    let delta = match side {
                        Side::Buy => booked,
                        Side::Sell | Side::ShortSell => -booked,
                    };
                    context.update_position(instrument, delta as f64);
                    // notify_fill inlined
                    shared.orders.push_fill(fill);
                    shared.portfolio.set_position(fill.instrument, context.position(fill.instrument));
                    // The holding the caller reads is keyed by contract, and
                    // the broker restates that feed on its own schedule — never
                    // because an order filled. Left to that feed alone, a
                    // position read back after a fill is the one the session
                    // started with.
                    // The report names the contract it filled. An order placed
                    // by symbol registers an instrument that knows no contract
                    // id, so taking it from the instrument attributed nothing.
                    let filled_con_id = parsed.get(&6008)
                        .and_then(|s| s.parse::<i64>().ok())
                        .filter(|id| *id != 0)
                        .or_else(|| context.market.con_id(instrument));
                    if let Some(con_id) = filled_con_id {
                        shared.portfolio.apply_fill(
                            con_id, delta as f64, (last_px * PRICE_SCALE as f64) as Price,
                        );
                    }
                    emit(event_tx, Event::Fill(fill));
                }
            }
        }

        // A report that fills an order states its new status on the same
        // report, and suppressing the status because the fill was on it meant
        // the one transition that matters most was the one never announced: a
        // caller watching order status was told about the execution and left
        // believing the order was still working. The two are different
        // questions — what traded, and where the order stands — and a report
        // that answers both is not a reason to drop one.
        if status_changed
            && let Some(order) = context.order(clord_id).copied() {
                let perm_id: i64 = parsed.get(&37).map(|s| perm_id_from_fix_order_id(s)).unwrap_or(0);
                // Tag 583 is the link id this engine sends the OCA group on, not
                // a parent order. Hashing it produced a stable non-zero value
                // shared by every order in a group, none of which has a parent,
                // and nothing distinguished it from a real link.
                //
                // 6107 is not the way to recover one either, though an order
                // *sends* its parent there: the tag is message-scoped, and the
                // vendor's own audit renderer names the inbound one
                // ParentClientId. That is what the shared non-zero value above
                // was — one client id echoed to every order in the account.
                // Reading it back as a parent gives each of them a parent that
                // does not exist. Nothing on this report carries a parent order
                // id, so report none.
                let parent_id: i64 = 0;
                let update = crate::types::OrderUpdate {
                    order_id: clord_id,
                    instrument: order.instrument,
                    status,
                    filled_qty: order.filled as f64,
                    remaining_qty: leaves_qty as f64,
                    avg_price: (order_avg_px * PRICE_SCALE as f64) as Price,
                    perm_id,
                    parent_id,
                    timestamp_ns: context.now_ns(),
                };
                shared.orders.push_order_update(update);
                emit(event_tx, Event::OrderUpdate(update));

                // A parked (39=I) order carries its reason on the same tags
                // 58/103 as a reject, but OrderState.completedStatus stays
                // empty for Inactive — it is not completed and may
                // reactivate, so there is no snapshot field to carry the
                // reason on. Route it through the same error() path a
                // cancel/modify reject already uses instead.
                if status == crate::types::OrderStatus::Inactive {
                    let reason = stated_reason(parsed);
                    if !reason.is_empty() {
                        shared.orders.push_order_inactive(clord_id, ORDER_INACTIVE_ERROR_CODE, reason);
                    }
                }
            }

        // Enrich order/contract caches block
        {
            let account = parsed.get(&1).cloned().unwrap_or_default();
            let symbol = parsed.get(&55).cloned().unwrap_or_default();
            // Where the order is working. The report states it on 207 when it
            // says so at all, and on 6004 as the destination it was routed to;
            // failing both, this client knows where it sent the order and says
            // that. An empty exchange on a completed order is a contract a
            // caller cannot re-place, and the reference client never returns one.
            let exchange = parsed.get(&207).cloned()
                .filter(|e| !e.is_empty())
                .or_else(|| parsed.get(&6004).cloned().filter(|e| !e.is_empty()))
                .or_else(|| {
                    context.order(clord_id).copied()
                        .map(|o| context.market.order_routing(o.instrument).1)
                        .filter(|e| !e.is_empty())
                })
                .unwrap_or_default();
            let sec_type = parsed.get(&167).cloned().unwrap_or_default();
            let currency = parsed.get(&15).cloned().unwrap_or_default();
            let con_id: i64 = parsed.get(&6008).and_then(|s| s.parse().ok()).unwrap_or(0);
            let local_symbol = parsed.get(&6035).cloned().unwrap_or_default();
            let perm_id: i64 = parsed.get(&37).map(|s| perm_id_from_fix_order_id(s)).unwrap_or(0);
            let total_qty: f64 = parsed.get(&38).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let ord_type_tag = parsed.get(&40).map(|s| s.as_str()).unwrap_or("");
            let limit_price: f64 = parsed.get(&44).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let tif_tag = parsed.get(&59).map(|s| s.as_str()).unwrap_or("");
            let stop_px: f64 = parsed.get(&99).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let outside_rth = parsed.get(&6433).map(|s| s == "1").unwrap_or(false);
            let clearing_intent = parsed.get(&6419).cloned().unwrap_or_default();
            let auto_cancel_date = parsed.get(&6596).cloned().unwrap_or_default();
            let exec_exchange = parsed.get(&30).cloned().unwrap_or_default();
            let transact_time = parsed.get(&60).cloned().unwrap_or_default();
            let avg_px: f64 = parsed.get(&6).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            // Absent is not zero. This value is written into a row that
            // persists, so a later report that omits the tag — a pending
            // cancel, say — would otherwise wipe a real filled quantity back
            // to nothing, which is the symptom this is correcting.
            let cum_qty: Option<f64> = parsed.get(&14).and_then(|s| s.parse().ok());
            let last_liq: i32 = parsed.get(&851).and_then(|s| s.parse().ok()).unwrap_or(0);

            let sec_type_str = match sec_type.as_str() {
                "CS" | "COMMON" => "STK",
                "FUT" => "FUT",
                "OPT" => "OPT",
                "FOR" | "CASH" => "CASH",
                "IND" => "IND",
                "FOP" => "FOP",
                "WAR" => "WAR",
                "BAG" => "BAG",
                "BOND" => "BOND",
                "CMDTY" => "CMDTY",
                "NEWS" => "NEWS",
                "FUND" => "FUND",
                _ => &sec_type,
            };

            let order_type_str = match ord_type_tag {
                "1" => "MKT", "2" => "LMT", "3" => "STP", "4" => "STP LMT",
                "P" => "TRAIL", "5" => "MOC", "B" => "LOC", "J" => "MIT",
                "K" => "MTL", "R" => "REL", _ => ord_type_tag,
            };

            // Unknown maps to empty, which is what `decode_tif` means by it and
            // what makes the fallback below reachable. A catch-all of `DAY`
            // reported a perfectly ordinary value for a code this does not know
            // and for an absent tag alike, so a caller reconciling its own
            // orders saw a plausible answer that disagreed with what it sent
            // and nothing said so.
            //
            // The sibling above passes the raw tag through instead; that works
            // there because an absent tag leaves it empty, while any non-empty
            // TIF code would suppress the fallback that knows the real answer.
            let tif_str = match tif_tag {
                "0" => "DAY", "1" => "GTC", "3" => "IOC", "4" => "FOK",
                "2" => "OPG", "6" => "GTD", "8" => "AUC",
                // Stated but unmapped: reported as stated, like the order-type
                // sibling above. The gateway is authoritative when it says
                // anything, and a code this does not name is still better seen
                // than replaced by an unrelated local value.
                other => other,
            };

            let action = match parsed.get(&54).map(|s| s.as_str()) {
                Some("1") => "BUY",
                Some("2") => "SELL",
                Some("5") => "SSHORT",
                _ => if let Some(order) = context.order(clord_id) {
                    match order.side {
                        Side::Buy => "BUY",
                        Side::Sell => "SELL",
                        Side::ShortSell => "SSHORT",
                    }
                } else { "" },
            };

            let status_str = crate::client_core::order_status_str(status);

            let resolved_con_id = if con_id != 0 {
                con_id
            } else if let Some(order) = context.order(clord_id) {
                context.market.con_id(order.instrument).unwrap_or(0)
            } else {
                0
            };

            let contract = if resolved_con_id != 0 {
                if let Some(mut cached) = shared.reference.get_contract(resolved_con_id) {
                    if !symbol.is_empty() { cached.symbol = symbol.clone(); }
                    if !sec_type_str.is_empty() { cached.sec_type = sec_type_str.to_string(); }
                    if !exchange.is_empty() { cached.exchange = exchange.clone(); }
                    if !currency.is_empty() { cached.currency = currency.clone(); }
                    if !local_symbol.is_empty() { cached.local_symbol = local_symbol.clone(); }
                    cached
                } else {
                    api::Contract {
                        con_id: resolved_con_id,
                        symbol: symbol.clone(),
                        sec_type: sec_type_str.to_string(),
                        exchange: exchange.clone(),
                        currency: currency.clone(),
                        local_symbol: local_symbol.clone(),
                        ..Default::default()
                    }
                }
            } else {
                api::Contract {
                    symbol: symbol.clone(),
                    sec_type: sec_type_str.to_string(),
                    exchange: exchange.clone(),
                    currency: currency.clone(),
                    local_symbol: local_symbol.clone(),
                    ..Default::default()
                }
            };

            let (fb_action, fb_tif, fb_ord_type) = if let Some(ctx_order) = context.order(clord_id) {
                let a = match ctx_order.side {
                    crate::types::Side::Buy => "BUY",
                    crate::types::Side::Sell | crate::types::Side::ShortSell => "SELL",
                };
                let t = decode_tif(ctx_order.tif);
                let o = match ctx_order.ord_type {
                    b'1' => "MKT", b'2' => "LMT", b'3' => "STP", b'4' => "STP LMT",
                    b'P' => "TRAIL", _ => "",
                };
                (a, t, o)
            } else {
                ("", "", "")
            };

            // Derive 3 order-dependent fields from FIX tags
            let oca_type: i32 = match parsed.get(&6209).map(|s| s.as_str()) {
                Some("CancelOnFillWBlock") => 1,
                Some("ReduceOnFillWBlock") => 2,
                Some("ReduceOnFillNonBlock") => 3,
                Some("ReduceOnFillWBlockFromTotal") => 4,
                _ => 3, // default
            };
            let algo_strategy = parsed.get(&847).cloned().unwrap_or_default();
            let use_price_mgmt_algo: i32 = if algo_strategy == "Adaptive" { 1 } else { 0 };
            let trail_stop_price: f64 = parsed.get(&6117)
                .and_then(|s| s.parse().ok())
                .unwrap_or(f64::MAX);

            let order = api::Order {
                order_id: clord_id as i64,
                action: if action.is_empty() { fb_action.to_string() } else { action.to_string() },
                total_quantity: total_qty,
                order_type: if order_type_str.is_empty() { fb_ord_type.to_string() } else { order_type_str.to_string() },
                lmt_price: limit_price,
                aux_price: stop_px,
                tif: if tif_str.is_empty() { fb_tif.to_string() } else { tif_str.to_string() },
                account: if account.is_empty() { account_id.to_string() } else { account.clone() },
                perm_id,
                // Tag 14 (CumQty), not tag 151 (LeavesQty). The two are
                // complements, so reporting the remainder as the filled amount
                // makes a completed order read as entirely unfilled.
                filled_quantity: cum_qty.unwrap_or_else(|| {
                    shared.orders.get_order_info(clord_id)
                        .map_or(0.0, |info| info.order.filled_quantity)
                }),
                outside_rth,
                clearing_intent,
                auto_cancel_date,
                submitter: account_id.to_string(),
                oca_type,
                use_price_mgmt_algo,
                trail_stop_price,
                algo_strategy,
                // The report restates the order, and a caller asking what its
                // orders are is answered from it. Everything below arrived on
                // every report and was read from none of them, so an order came
                // back naming neither the reference the caller gave it, nor the
                // client that placed it, nor how it allocates.
                order_ref: parsed.get(&6010).cloned().unwrap_or_default(),
                rule80a: parsed.get(&47).cloned().unwrap_or_default(),
                good_till_date: parsed.get(&432).cloned().unwrap_or_default(),
                client_id: parsed.get(&109).and_then(|s| s.parse().ok()).unwrap_or(0),
                // How an advisor's order is divided, which is the whole of what
                // an advisor's order is.
                fa_group: parsed.get(&6160).cloned().unwrap_or_default(),
                fa_method: parsed.get(&6159).cloned().unwrap_or_default(),
                fa_percentage: parsed.get(&6164).cloned().unwrap_or_default(),
                ..Default::default()
            };

            let completed_time = if matches!(status,
                crate::types::OrderStatus::Filled |
                crate::types::OrderStatus::Cancelled |
                crate::types::OrderStatus::Rejected
            ) {
                parsed.get(&52).cloned().unwrap_or_default()
            } else {
                String::new()
            };
            let completed_status = match status {
                crate::types::OrderStatus::Filled => "Filled".to_string(),
                crate::types::OrderStatus::Cancelled => "Cancelled".to_string(),
                crate::types::OrderStatus::Rejected => {
                    parsed.get(&58).cloned().unwrap_or_else(|| "Rejected".to_string())
                }
                _ => String::new(),
            };

            let order_state = api::OrderState {
                status: status_str.to_string(),
                commission_and_fees: commission,
                completed_time,
                completed_status,
                // `completed_status` is the reject text alone, which is what
                // ibapi's field means. The reason code lives here, where a
                // caller telling a venue's refusal from a bad request can
                // reach it.
                reject_reason: if status == crate::types::OrderStatus::Rejected {
                    stated_reason(parsed)
                } else {
                    String::new()
                },
                ..Default::default()
            };

            let last_exec = api::Execution {
                // What the report stated that nothing above names. A report
                // carries far more than any one client reads, and what is not
                // read is kept rather than dropped.
                unnamed_fields: unnamed_execution_fields(raw),
                exec_id: exec_id.to_string(),
                time: transact_time,
                acct_number: account,
                exchange: exec_exchange,
                side: if let Some(o) = context.order(clord_id) {
                    match o.side { Side::Buy => "BOT", Side::Sell | Side::ShortSell => "SLD" }.to_string()
                } else { String::new() },
                shares: last_shares as f64,
                price: last_px,
                order_id: clord_id as i64,
                // The execution record describes this report, so an absent
                // cumulative is zero here rather than the cached total.
                cum_qty: cum_qty.unwrap_or(0.0),
                avg_price: avg_px,
                last_liquidity: last_liq,
                // Not a field of its own: the broker says it liquidated the
                // position by naming the order with a leading L rather than by
                // setting anything. Read as a flag it was never set at all, and
                // a caller could not tell a liquidation from any other fill.
                liquidation: i32::from(parsed.get(&11).is_some_and(|s| s.starts_with('L'))),
                // What the instrument's economic value is reckoned by, where it
                // has one, and what the reckoning is multiplied by.
                ev_rule: parsed.get(&6858).cloned().unwrap_or_default(),
                // The multiplier is the tag beside the rule, and the venue
                // states it as a number. It was read off 6892, which the venue
                // states as text — so it parsed to nothing and every fill
                // carried a multiplier of zero. A contract whose value follows
                // something other than its own price is then valued at nothing.
                ev_multiplier: parsed
                    .get(&crate::control::contracts::TAG_EV_MULTIPLIER)
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0.0),
                // The price on this report may yet be revised.
                pending_price_revision: parsed.get(&8497)
                    .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
                ..Default::default()
            };

            if con_id != 0 {
                // An execution report states a subset of a definition: it names
                // the contract, not its long name, its trading class or the
                // venues it may trade on. Caching it whole replaced a definition
                // already fetched with a poorer one, leaving a later reader a
                // contract missing fields. Fill, do not replace.
                let merged = match shared.reference.get_contract(con_id) {
                    Some(mut known) => {
                        if !contract.symbol.is_empty() { known.symbol = contract.symbol.clone(); }
                        if !contract.sec_type.is_empty() { known.sec_type = contract.sec_type.clone(); }
                        if !contract.exchange.is_empty() { known.exchange = contract.exchange.clone(); }
                        if !contract.currency.is_empty() { known.currency = contract.currency.clone(); }
                        if !contract.local_symbol.is_empty() {
                            known.local_symbol = contract.local_symbol.clone();
                        }
                        known
                    }
                    None => contract.clone(),
                };
                shared.reference.cache_contract(con_id, merged);
            }

            // A trade cancel (150=H) or trade correction (150=G) restates an
            // execution the gateway has already reported, so it may legitimately
            // return a completed order to a working quantity. Every other report
            // that would do that is a replay.
            let info = RichOrderInfo { contract, order, order_state, last_exec };
            if matches!(exec_type, "G" | "H") {
                shared.orders.push_order_correction(clord_id, info);
            } else {
                // A late duplicate of an earlier partial must not rewrite a
                // completed order back to open. The cache is what
                // `req_open_orders` reads, so a caller polling between the two
                // frames would see a finished order listed as working.
                let already_terminal = shared.orders.get_order_info(clord_id).is_some_and(|prev| {
                    matches!(prev.order_state.status.as_str(), "Filled" | "Cancelled" | "Inactive")
                });
                if !already_terminal || matches!(
                    status,
                    crate::types::OrderStatus::Filled
                        | crate::types::OrderStatus::Cancelled
                        | crate::types::OrderStatus::Rejected
                ) {
                    shared.orders.push_order_info(clord_id, info);
                }
            }
        }

        if matches!(status,
            crate::types::OrderStatus::Filled |
            crate::types::OrderStatus::Cancelled |
            crate::types::OrderStatus::Rejected
        ) {
            // Recorded whether or not the order was being tracked. A market
            // order can finish before its acknowledgement has been handled, so
            // requiring a tracked record meant the fastest orders — the ones
            // that fill immediately — left no memory of having finished, and
            // the working status the gateway echoes behind the fill had nothing
            // to be refused by.
            let tracked = context.order(clord_id).copied();
            shared.orders.push_completed_order(CompletedOrder {
                order_id: clord_id,
                instrument: tracked.map_or(0, |o| o.instrument),
                status,
                filled_qty: tracked.map_or(0, |o| o.filled as i64),
                timestamp_ns: context.now_ns(),
            });
            context.retire_order(clord_id);
        }
    }

    fn handle_cancel_reject(
        &mut self,
        parsed: &std::collections::HashMap<u32, String>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
    ) {
        // Match handle_exec_report's tag-11 parsing: strip the gateway's
        // "C" prefix and any ".0/.1/.2" modify-chain suffix.
        let orig_clord = parsed.get(&41).and_then(|s| {
            let stripped = s.strip_prefix('C').unwrap_or(s);
            let base = stripped.split('.').next().unwrap_or(stripped);
            base.parse::<u64>().ok()
        });
        let reason = parsed.get(&58).map(|s| s.as_str()).unwrap_or("Cancel rejected");
        let reject_type: u8 = parsed.get(&434).and_then(|s| s.parse().ok()).unwrap_or(1);
        let reason_code: i32 = parsed.get(&102).and_then(|s| s.parse().ok()).unwrap_or(-1);
        log::warn!("CancelReject: origClOrd={orig_clord:?} type={reject_type} code={reason_code} reason={reason}");

        let Some(oid) = orig_clord else { return };

        // FIX CxlRejReason 1 = UnknownOrder: the gateway is stating that the
        // order does not exist on its side. Restoring it to working asserted
        // the opposite of the message being handled, and the engine's own view
        // governs subsequent cancels, modifies and reconnect bookkeeping — so a
        // phantom order persisted there while the cache row that would have
        // surfaced it was removed.
        //
        // Read as a positive statement, not as an absence: a missing or
        // unparseable tag 102 is synthesized as -1 here and says nothing, so it
        // takes the same path as the reasons that do mean the order is working.
        let unknown_order = reason_code == 1;

        // Update local context only for an order tracked in this session.
        let instrument = if let Some(order) = context.order(oid).copied() {
            if unknown_order {
                // Terminal and removed, which is what the gateway just said.
                // Holding the record in a non-working status instead is not an
                // option here: those are excluded from the open-order count
                // that guards instrument reclamation, so the slot could be
                // handed to another contract while a retained order still
                // pointed at it, and a late fill would move the wrong position.
                //
                // A fill that races the rejection is not lost with the order:
                // the untracked-fill path books it and moves the position
 //.
                context.set_order_status_forced(oid, crate::types::OrderStatus::Cancelled);
                context.retire_order(oid);
            } else {
                let restore_status = if order.filled > 0 {
                    crate::types::OrderStatus::PartiallyFilled
                } else {
                    crate::types::OrderStatus::Submitted
                };
                // Deliberate regression (PendingCancel back to working) — the
                // guard would rightly block it on the ordinary path.
                context.set_order_status_forced(oid, restore_status);
            }
            order.instrument
        } else {
            0
        };

        // Drop the stale cache entry so subsequent req_open_orders stops
        // returning it. Other reasons leave the cache alone; a follow-up exec
        // report will reconcile.
        //
        // No synthetic status update is queued alongside it. The cancel-reject
        // below is the report, and both dispatchers drain fills ahead of order
        // updates — so an update queued here would reach a caller after the
        // fill that raced it, stating the order was gone when it had just been
        // told the order filled.
        if unknown_order {
            shared.orders.remove_order_info(oid);
        }

        let reject = crate::types::CancelReject {
            order_id: oid,
            instrument,
            reject_type,
            reason_code,
            timestamp_ns: context.now_ns(),
        };
        shared.orders.push_cancel_reject(reject);
        emit(event_tx, Event::CancelReject(reject));
    }

    fn handle_news_bulletin(&mut self, parsed: &std::collections::HashMap<u32, String>, shared: &SharedState) {
        static BULLETIN_TYPE_MAP: &[(i32, i32)] = &[
            (1, 1), (2, 2), (3, 3), (8, 1), (9, 1), (10, 1),
        ];
        let fix_type: i32 = parsed.get(&fix::TAG_URGENCY)
            .and_then(|s| s.parse().ok()).unwrap_or(0);
        let api_type = BULLETIN_TYPE_MAP.iter()
            .find(|(k, _)| *k == fix_type)
            .map(|(_, v)| *v);
        let api_type = match api_type {
            Some(t) => t,
            None => return,
        };
        let message = parsed.get(&fix::TAG_HEADLINE).cloned().unwrap_or_default();
        let exchange = parsed.get(&fix::TAG_SECURITY_EXCHANGE).cloned().unwrap_or_default();
        self.bulletin_next_id += 1;
        let bulletin = NewsBulletin {
            msg_id: self.bulletin_next_id,
            msg_type: api_type,
            message,
            exchange,
        };
        shared.market.push_news_bulletin(bulletin);
    }

    /// One account figure, and a tag saying which figure it is.
    ///
    /// The message carries a single number on 9806 and a selector on 6566. The
    /// number was read as net liquidation whatever the selector said, so a
    /// figure of another kind replaced a correct net liquidation — the one the
    /// keyed account values state — and it only showed when that other figure
    /// happened to be negative: an account holding nothing, with the better
    /// part of a million in cash, reported a net liquidation of minus fourteen
    /// hundred.
    ///
    /// Which selector means net liquidation is not established, and the one
    /// this session is sent is demonstrably not it, so nothing is written from
    /// here. The selector is recorded as an unread wire rather than guessed at.
    fn handle_account_summary(&mut self, parsed: &std::collections::HashMap<u32, String>, context: &mut Context, shared: &SharedState) {
        if let Some(selector) = parsed.get(&6566) {
            shared.market.note_unread_wire(
                "trading",
                format!("account figure of kind {selector} (6040=77), kind not established"),
            );
        }
        shared.portfolio.set_account(context.account());
    }

    /// Subscribe to the schedule paired with a secdef reply, joined on tag 6256.
    /// Internal subscription (no API-client req_id exposed); reply arrives as
    /// 35=U|6040=107 and is matched back to the secdef via 6256.
    fn send_schedule_subscribe(
        &mut self,
        join_key: &str,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        if let Some(conn) = ccp_conn.as_mut() {
            let sub_id = self.next_schedule_sub_id;
            self.next_schedule_sub_id += 1;
            let sub_id_str = format!("SchedSub.{sub_id}");
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (crate::control::contracts::TAG_SUB_PROTOCOL,
                    crate::control::contracts::SUB_PROTOCOL_SCHEDULE_SUBSCRIBE),
                (320, &sub_id_str),
                (crate::control::contracts::TAG_SCHEDULE_JOIN_KEY, join_key),
            ]);
            hb.last_ccp_sent = Instant::now();
        }
    }

    /// Drop pending schedule pairs past their deadline, emitting partial details.
    /// Fail contract-details requests whose deadline has passed:
    /// both plain/by-symbol secdef lookups the gateway never answered and
    /// by-symbol fan-outs missing one or more per-exchange replies. On
    /// expiry the caller gets error 200 plus contract_details_end, so a
    /// blocked wait unblocks with no API change. Internal sentinel req_ids
    /// (>= 0xF000_0000: cache auto-fetch, scanner enrichment) are dropped
    /// silently — no user is waiting on them.
    /// How long a subscription waits for the lookup that would name its
    /// contract.
    ///
    /// Between two deadlines, and it has to stay between them. Longer than the
    /// lookup's own, so a definition or a refusal from the venue is preferred
    /// to this. Shorter than the caller's, because for a held request this is
    /// the only report there is: the lookup behind it is asked under an
    /// internal id, and an internal id is dropped silently. Sitting past the
    /// caller's wait, as it did, meant the caller was told nothing arrived
    /// while the reason was still being held, and heard it never.
    const NAMING_TIMEOUT: Duration =
        Duration::from_secs(crate::config::ANSWER_TIMEOUT_SECS - 3);

    /// A subscription whose contract the venue never named. Reported rather
    /// than left waiting: silence is what this whole path exists to remove.
    pub(crate) fn sweep_pending_subscribes(&mut self, shared: &SharedState) {
        if self.pending_md_subscribe.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut gave_up = Vec::new();
        self.pending_md_subscribe.retain(|(_, pending, asked_at)| {
            if now.duration_since(*asked_at) < Self::NAMING_TIMEOUT {
                return true;
            }
            gave_up.push(pending.clone());
            false
        });
        for p in gave_up {
            let reason = format!(
                "no security definition has been found for {} {} on {}, so no market \
                 data subscription could be made for it",
                p.sec_type, p.symbol, p.exchange,
            );
            log::warn!("Subscription abandoned: {reason}");
            shared.market.push_subscription_failure(p.instrument, reason);
        }
    }

    pub(crate) fn sweep_contract_details(
        &mut self,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
    ) {
        // A schedule pairing delivers after the lookup that asked for it has
        // been retired, so the record of what a request was already handed has
        // to outlive both. Dropping it at the lookup's end let the venue copy
        // through behind the one carrying the trading hours.
        let idle = self.pending_secdef.is_empty()
            && self.pending_fanout.is_empty()
            && self.pending_schedule_pair.is_empty();
        if idle {
            self.details_delivered.clear();
        }
        if self.pending_secdef.is_empty() && self.pending_fanout.is_empty() {
            return;
        }
        let now = Instant::now();
        // A session that has ended answers nothing, so every request waiting on
        // it is already finished — waiting out its deadline only delays the
        // caller learning that, once per request.
        let over = shared.reference.session_over();
        let mut expired: Vec<u32> = Vec::new();
        self.pending_secdef.retain(|(req_id, _, deadline)| {
            if now >= *deadline || over.is_some() {
                if *req_id < 0xF000_0000 {
                    expired.push(*req_id);
                } else {
                    log::warn!("Internal secdef timeout: req_id={req_id:#x}");
                }
                false
            } else {
                true
            }
        });
        self.pending_fanout.retain(|p| {
            if now >= p.deadline || over.is_some() {
                log::warn!(
                    "Contract-details fan-out timeout: api_req_id={} received {} of {}",
                    p.api_req_id, p.received, p.fanout_req_ids.len(),
                );
                expired.push(p.api_req_id);
                false
            } else {
                true
            }
        });
        for req_id in expired {
            let (code, why) = match over {
                Some(reason) => (
                    crate::api::error_codes::Refusal::NOT_CONNECTED,
                    format!("the session is over: {reason}"),
                ),
                None => (
                    200,
                    "contract details request timed out — no reply from the gateway".to_string(),
                ),
            };
            log::warn!("Contract-details unanswered: req_id={req_id} ({why})");
            shared.reference.push_historical_error(req_id, code, why);
            shared.reference.push_contract_details_end(req_id);
            emit(event_tx, Event::ContractDetailsEnd(req_id));
        }
    }

    pub(crate) fn sweep_pending_schedule_pairs(
        &mut self,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
    ) {
        let now = Instant::now();
        let mut emit_now: Vec<PendingSchedulePair> = Vec::new();
        self.pending_schedule_pair.retain(|p| {
            if now >= p.deadline {
                let mut def = p.def.clone();
                def.trading_hours = None;
                def.liquid_hours = None;
                def.time_zone_id = None;
                emit_now.push(PendingSchedulePair {
                    api_req_id: p.api_req_id,
                    join_key: p.join_key.clone(),
                    def,
                    is_last: p.is_last,
                    deadline: p.deadline,
                });
                log::warn!("Schedule pair timeout: api_req_id={} join_key={}",
                    p.api_req_id, p.join_key);
                false
            } else {
                true
            }
        });
        for p in emit_now {
            // Same gate as every other way a contract reaches the caller: the
            // venue fan-out describes one contract many times over.
            if !self.details_delivered.entry(p.api_req_id).or_default().insert(p.def.con_id as i64) {
                if p.is_last {
                    shared.reference.push_contract_details_end(p.api_req_id);
                    emit(event_tx, Event::ContractDetailsEnd(p.api_req_id));
                }
                continue;
            }
            let for_event = clone_for_event(event_tx, &p.def);
            shared.reference.push_contract_details(p.api_req_id, p.def);
            if let Some(details) = for_event {
                emit(event_tx, Event::ContractDetails { req_id: p.api_req_id, details: Box::new(details) });
            }
            if p.is_last {
                shared.reference.push_contract_details_end(p.api_req_id);
                emit(event_tx, Event::ContractDetailsEnd(p.api_req_id));
            }
        }
    }

    /// Match a 6040=107 schedule reply to a pending secdef pair and emit merged details.
    fn handle_schedule_reply(
        &mut self,
        msg: &[u8],
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
    ) {
        // Extract 6256 from the reply to locate the matching pair.
        let join_key = match extract_tag_value(msg, b"6256=") {
            Some(v) => v,
            None => return,
        };
        let pos = match self.pending_schedule_pair.iter().position(|p| p.join_key == join_key) {
            Some(p) => p,
            None => return,
        };
        let mut pair = self.pending_schedule_pair.swap_remove(pos);
        if let Some(sched) = crate::control::contracts::parse_schedule_response(msg) {
            pair.def.time_zone_id = if sched.timezone.is_empty() {
                None
            } else {
                Some(sched.timezone.clone())
            };
            pair.def.trading_hours = Some(
                crate::control::contracts::format_sessions_string(&sched.trading_hours)
            );
            pair.def.liquid_hours = Some(
                crate::control::contracts::format_sessions_string(&sched.liquid_hours)
            );
        }
        let for_event = clone_for_event(event_tx, &pair.def);
        // The schedule reply completes the pairing, and this is where the row
        // that carries the trading hours reaches the caller. Same gate as every
        // other path: one contract, delivered once.
        if !self.details_delivered.entry(pair.api_req_id).or_default().insert(pair.def.con_id as i64) {
            if pair.is_last {
                shared.reference.push_contract_details_end(pair.api_req_id);
                emit(event_tx, Event::ContractDetailsEnd(pair.api_req_id));
            }
            return;
        }
        shared.reference.push_contract_details(pair.api_req_id, pair.def);
        if let Some(details) = for_event {
            emit(event_tx, Event::ContractDetails { req_id: pair.api_req_id, details: Box::new(details) });
        }
        if pair.is_last {
            shared.reference.push_contract_details_end(pair.api_req_id);
            emit(event_tx, Event::ContractDetailsEnd(pair.api_req_id));
        }
    }

    /// Ask for, or replace, the advisor's own configuration.
    ///
    /// An advisor's groups, allocation profiles and models are held by the
    /// venue, not by this client, and are asked for one partition at a time.
    /// The command says which of asking, replacing or removing is meant; a
    /// replacement carries the configuration as its own document.
    ///
    /// An account that is not an advisor's holds none of this, and the venue
    /// says so rather than answering with an empty one.
    pub(crate) fn send_advisor_config(
        &mut self,
        command: i32,
        partition: &str,
        document: Option<&str>,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        if let Some(conn) = ccp_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let command = command.to_string();
            let mut fields: Vec<(u32, &str)> = vec![
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (6040, "116"),
                (6905, &command),
                (6158, partition),
            ];
            // Only a replacement carries a document; asking for one that states
            // a document would be asking and telling at once.
            if let Some(xml) = document {
                fields.push((6118, xml));
            }
            let _ = conn.send_fix(&fields);
            hb.last_ccp_sent = Instant::now();
            log::info!("Sent advisor configuration request: command={command} partition={partition}");
        }
    }

    /// Send P&L subscribe: 6040=142 with 6529=PLR.{N}|1={account}|
    pub(crate) fn send_pnl_subscribe(
        &mut self,
        req_id: i64,
        account: &str,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        if let Some(conn) = ccp_conn.as_mut() {
            let pnl_payload = format!("PLR.{req_id}|1={account}|");
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (6040, "142"),
                (6529, &pnl_payload),
            ]);
            hb.last_ccp_sent = Instant::now();
            log::info!("Sent P&L subscribe: req_id={req_id} account={account}");
        }
    }

    pub(crate) fn send_news_subscribe(
        &mut self,
        con_id: i64,
        instrument: InstrumentId,
        providers: &str,
        req_id: u32,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        self.news_subscriptions.push((instrument, req_id, providers.to_string()));
        if let Some(conn) = ccp_conn.as_mut() {
            let req_id_str = req_id.to_string();
            let con_id_str = (con_id as u32).to_string();
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                (fix::TAG_SENDING_TIME, &ts),
                (263, "1"),
                (146, "1"),
                (262, &req_id_str),
                (6008, &con_id_str),
                (207, "NEWS"),
                (167, "CS"),
                (264, "292"),
                (6472, providers),
            ]);
            hb.last_ccp_sent = Instant::now();
            log::info!("Sent news subscribe: con_id={con_id} req_id={req_id} providers={providers}");
        }
    }

    /// Say goodbye before going.
    ///
    /// A session dropped without this is one the venue has to time out, and
    /// this account permits only one at a time: the next connection then races
    /// a session the venue still believes is live. The vendor's own client
    /// sends it, and states why it is going.
    pub(crate) fn send_logout(
        &mut self,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        let Some(conn) = ccp_conn.as_mut() else { return };
        let ts = chrono_free_timestamp();
        let sent = conn.send_fix(&[
            (fix::TAG_MSG_TYPE, fix::MSG_LOGOUT),
            (fix::TAG_SENDING_TIME, &ts),
            // The vendor states a reason here; "S" is what it sends when the
            // session is being shut down rather than lost.
            (8372, "S"),
        ]);
        if sent.is_ok() {
            hb.last_ccp_sent = Instant::now();
            log::info!("Logout sent");
        }
    }

    pub(crate) fn send_news_unsubscribe(
        &mut self,
        instrument: InstrumentId,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        let req_id = match self.news_subscriptions.iter().position(|(id, _, _)| *id == instrument) {
            Some(pos) => {
                let (_, rid, _) = self.news_subscriptions.remove(pos);
                rid
            }
            None => return,
        };
        if let Some(conn) = ccp_conn.as_mut() {
            let req_id_str = req_id.to_string();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                (262, &req_id_str),
                (263, "2"),
            ]);
            hb.last_ccp_sent = Instant::now();
            log::info!("Sent news unsubscribe: instrument={instrument:?} req_id={req_id}");
        }
    }

    pub(crate) fn send_secdef_request(&mut self, req_id: u32, con_id: i64, ccp_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        if let Some(conn) = ccp_conn.as_mut() {
            let con_id_str = con_id.to_string();
            let req_id_str = req_id.to_string();
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "c"),
                (fix::TAG_SENDING_TIME, &ts),
                (crate::control::contracts::TAG_SECURITY_REQ_ID, &req_id_str),
                (crate::control::contracts::TAG_SECURITY_REQ_TYPE, "2"),
                (crate::control::contracts::TAG_IB_CON_ID, &con_id_str),
                (crate::control::contracts::TAG_IB_SOURCE, "Socket"),
            ]);
            log::info!("Sent secdef request: req_id={req_id} con_id={con_id}");
            hb.last_ccp_sent = Instant::now();
        } else {
            // No CCP socket: the entry still gets a deadline, so the caller
            // receives error 200 + end via the sweep instead of silence.
            log::warn!("secdef request req_id={req_id} queued with no CCP socket");
        }
        // Known-conId lookup: single record, no paginated terminator.
        self.pending_secdef.push((req_id, true, Instant::now() + SECDEF_TIMEOUT));
    }


    /// Ask the venue to name a contract so a subscription can be sent for it.
    pub(crate) fn resolve_for_subscribe(
        &mut self,
        pending: PendingSubscribe,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        let req_id = self.next_internal_secdef_id;
        self.next_internal_secdef_id = self.next_internal_secdef_id.wrapping_add(1);
        let filters = crate::types::SecDefFilters {
            last_trade_date_or_contract_month: pending.last_trade_date.clone(),
            strike: pending.strike,
            right: pending.right.clone(),
            multiplier: pending.multiplier.clone(),
            ..Default::default()
        };
        let (symbol, sec_type, exchange, currency) = (
            pending.symbol.clone(), pending.sec_type.clone(),
            pending.exchange.clone(), pending.currency.clone(),
        );
        self.pending_md_subscribe.push((req_id, pending, Instant::now()));
        self.send_secdef_request_by_symbol(
            req_id, &symbol, &sec_type, &exchange, &currency, &filters, ccp_conn, hb,
        );
    }

    /// Ask the venue to name the contract a request wants, and hold the
    /// request until it does.
    ///
    /// Answers `false` when the request names nothing that can be looked up,
    /// so the caller sends it as it stands rather than holding it for ever.
    pub(crate) fn hold_until_named(
        &mut self,
        cmd: crate::types::ControlCommand,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) -> Option<crate::types::ControlCommand> {
        let (symbol, sec_type, exchange, currency) = match contract_named(&cmd) {
            Some(named) if !named.0.is_empty() => (
                named.0.to_string(), named.1.to_string(),
                named.2.to_string(), named.3.to_string(),
            ),
            _ => return Some(cmd),
        };
        let filters = filters_named(&cmd);
        let req_id = self.next_internal_secdef_id;
        self.next_internal_secdef_id = self.next_internal_secdef_id.wrapping_add(1);
        self.pending_named.push((req_id, cmd, Instant::now()));
        self.send_secdef_request_by_symbol(
            req_id, &symbol, &sec_type, &exchange, &currency, &filters, ccp_conn, hb,
        );
        None
    }

    /// A held request whose contract the venue never named. Told to the caller
    /// rather than left waiting.
    pub(crate) fn sweep_pending_named(&mut self, shared: &SharedState) {
        if self.pending_named.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut gave_up = Vec::new();
        self.pending_named.retain(|(_, cmd, asked_at)| {
            if now.duration_since(*asked_at) < Self::NAMING_TIMEOUT {
                return true;
            }
            gave_up.push(cmd.clone());
            false
        });
        for cmd in gave_up {
            let Some((symbol, sec_type, exchange, _)) = contract_named(&cmd) else { continue };
            let reason = format!(
                "no security definition has been found for {sec_type} {symbol} on \
                 {exchange}, so the request could not be sent",
            );
            log::warn!("Request abandoned: {reason}");
            if let Some(req_id) = request_id(&cmd) {
                super::push_hmds_error(
                    shared, req_id, reason,
                    matches!(cmd, crate::types::ControlCommand::FetchHistorical { .. }),
                );
            }
        }
    }

    pub(crate) fn send_secdef_request_by_symbol(&mut self, req_id: u32, symbol: &str, sec_type: &str, exchange: &str, currency: &str, filters: &crate::types::SecDefFilters, ccp_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        if let Some(conn) = ccp_conn.as_mut() {
            let req_id_str = req_id.to_string();
            let ts = chrono_free_timestamp();
            let fix_exchange = if exchange == "SMART" { "BEST" } else { exchange };
            let fix_sec_type = match sec_type {
                "STK" => "CS", "FUT" => "FUT", "OPT" => "OPT", "IND" => "IND", other => other,
            };
            // Identifier lookup (ISIN/CUSIP): SecurityIDSource is the standard FIX
            // code, 1 = CUSIP, 4 = ISIN. When a known one is set the
            // lookup rides the identifier and drops the symbol/secType/filters.
            let sec_id_source = match filters.sec_id_type.to_uppercase().as_str() {
                "ISIN" => "4",
                "CUSIP" => "1",
                _ => "",
            };
            let identifier_lookup = !filters.sec_id.is_empty() && !sec_id_source.is_empty();

            let strike_str = if filters.strike > 0.0 { format!("{}", filters.strike) } else { String::new() };
            // PutOrCall: Call = 1, Put = 0.
            let right_code = match filters.right.to_uppercase().as_str() {
                "C" | "CALL" => "1",
                "P" | "PUT" => "0",
                _ => "",
            };
            // Exchange rides tag 100; primaryExchange (when set) rides tag 207 —
            // the two were previously conflated onto 207. localSymbol replaces the
            // plain symbol; the derivative/disambiguation filters are added only
            // when set. Captured in.
            let mut fields: Vec<(u32, &str)> = vec![
                (fix::TAG_MSG_TYPE, "c"),
                (fix::TAG_SENDING_TIME, &ts),
                (320, &req_id_str),
                (321, "2"),
            ];
            if identifier_lookup {
                // Identifier lookup: the identifier and its source replace the
                // symbol/secType/filters; exchange and currency still ride
 //.
                fields.push((22, sec_id_source));
                fields.push((48, &filters.sec_id));
            } else {
                if !filters.local_symbol.is_empty() {
                    fields.push((6035, &filters.local_symbol));
                } else {
                    fields.push((55, symbol));
                }
                if !filters.trading_class.is_empty() {
                    fields.push((6058, &filters.trading_class));
                }
                fields.push((167, fix_sec_type));
                if let Some(tag) = maturity_tag(&filters.last_trade_date_or_contract_month) {
                    fields.push((tag, &filters.last_trade_date_or_contract_month));
                }
                if !right_code.is_empty() {
                    fields.push((201, right_code));
                }
                if !strike_str.is_empty() {
                    fields.push((202, &strike_str));
                }
                if !filters.multiplier.is_empty() {
                    fields.push((231, &filters.multiplier));
                }
            }
            fields.push((100, fix_exchange));
            if !identifier_lookup && !filters.primary_exchange.is_empty() {
                fields.push((207, &filters.primary_exchange));
            }
            fields.push((15, currency));
            fields.push((6088, "Socket"));
            let _ = conn.send_fix(&fields);
            log::info!("Sent secdef lookup: req_id={req_id} symbol={symbol} sec_type={sec_type} identifier={identifier_lookup}");
            hb.last_ccp_sent = Instant::now();
        } else {
            // See send_secdef_request: sweep converts this to a visible error.
            log::warn!("secdef-by-symbol request req_id={req_id} queued with no CCP socket");
        }
        // By-symbol lookup: master reply carries `6046={exch_list}`. The
        // server never emits a 323=5/6 terminator; completion is detected
        // by counting per-exchange fan-out replies (see `pending_fanout`).
        self.details_delivered.remove(&req_id);
        self.pending_secdef.push((req_id, false, Instant::now() + SECDEF_TIMEOUT));
    }

    /// Send a per-exchange fan-out request after a by-symbol master reply.
    /// Wire: `35=c|320={fanout_id}|321=2|146=1|6008={conid}|6004={exch}|`
    pub(crate) fn send_fanout_secdef_request(
        &mut self,
        fanout_req_id: &str,
        con_id: i64,
        exchange: &str,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        if let Some(conn) = ccp_conn.as_mut() {
            let con_id_str = con_id.to_string();
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "c"),
                (fix::TAG_SENDING_TIME, &ts),
                (crate::control::contracts::TAG_SECURITY_REQ_ID, fanout_req_id),
                (crate::control::contracts::TAG_SECURITY_REQ_TYPE, "2"),
                (146, "1"),
                (crate::control::contracts::TAG_IB_CON_ID, &con_id_str),
                (6004, exchange),
            ]);
            hb.last_ccp_sent = Instant::now();
        }
    }

    pub(crate) fn send_matching_symbols_request(&mut self, req_id: u32, pattern: &str, ccp_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        // Recorded only where the request went out, so a request issued while
        // the transport is down is not queued as pending with nothing on the
        // wire to answer it.
        let Some(conn) = ccp_conn.as_mut() else {
            log::warn!("Matching symbols request req_id={req_id} pattern='{pattern}' not sent: no CCP transport");
            return;
        };
        let req_id_str = req_id.to_string();
        let ts = chrono_free_timestamp();
        if let Err(e) = conn.send_fix(&[
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &ts),
            (6040, "185"),
            (320, &req_id_str),
            (58, pattern),
        ]) {
            log::warn!("Matching symbols request req_id={req_id} pattern='{pattern}' not sent: {e}");
            return;
        }
        hb.last_ccp_sent = Instant::now();
        log::info!("Sent matching symbols request: req_id={req_id} pattern='{pattern}'");
        self.pending_matching_symbols.push((req_id, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));
    }

    /// Give up on matching-symbols requests the gateway never answered.
    ///
    /// Nothing expired them, so an unanswered request stayed in the queue for
    /// the life of the process — and the reply matcher falls back to the head
    /// of that queue when a reply carries no echoed request id, so a stale entry
    /// could absorb a later request's answer.
    pub(crate) fn sweep_pending_matching_symbols(&mut self) {
        if self.pending_matching_symbols.is_empty() {
            return;
        }
        let now = Instant::now();
        self.pending_matching_symbols.retain(|(req_id, deadline)| {
            if now >= *deadline {
                log::warn!("Matching symbols request req_id={req_id} unanswered after {MATCHING_SYMBOLS_TIMEOUT:?} — giving up");
                false
            } else {
                true
            }
        });
    }

    /// Ask for the option chain of an underlying.
    ///
    /// A request that cannot go out is answered with an empty chain rather than
    /// left unanswered, because the caller is waiting on the end of a request
    /// nothing on the wire will ever end.
    pub(crate) fn send_option_params_request(
        &mut self,
        req_id: u32,
        symbol: &str,
        fut_fop_exchange: &str,
        underlying_sec_type: &str,
        underlying_con_id: i64,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
        shared: &SharedState,
    ) {
        let symbol = symbol.to_uppercase();
        // The request names the UNDERLYING's own type, not the derivative being
        // enumerated. Naming the derivative is answered "Unknown contract":
        // there is no option contract by that symbol, only a stock that has
        // options on it. A caller who states nothing claims nothing.
        let underlying = if underlying_sec_type.is_empty() { "STK" } else { underlying_sec_type };
        // A futures option whose underlying is not itself a future names that
        // underlying on a tag of its own.
        let futures_option = !fut_fop_exchange.is_empty() && underlying != "FUT";
        let con_id_tag = if futures_option { 6457 } else { 6346 };
        let Some(conn) = ccp_conn.as_mut() else {
            log::warn!("Option chain request req_id={req_id} symbol={symbol} not sent: no CCP transport");
            shared.reference.push_option_params(req_id, underlying_con_id, Vec::new());
            return;
        };
        let con_id_str = underlying_con_id.to_string();
        let ts = chrono_free_timestamp();
        let mut fields: Vec<(u32, &str)> = vec![
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &ts),
            (6040, "138"),
            (55, &symbol),
        ];
        fields.push((310, underlying));
        fields.push((con_id_tag, &con_id_str));
        fields.push((6320, "1"));
        fields.push((6994, "1"));
        if underlying_sec_type == "FUT" {
            fields.push((6995, fut_fop_exchange));
        }
        if let Err(e) = conn.send_fix(&fields) {
            log::warn!("Option chain request req_id={req_id} symbol={symbol} not sent: {e}");
            shared.reference.push_option_params(req_id, underlying_con_id, Vec::new());
            return;
        }
        hb.last_ccp_sent = Instant::now();
        log::info!("Sent option chain request: req_id={req_id} symbol={symbol} con_id={underlying_con_id}");
        self.pending_option_params.push((req_id, symbol, underlying_con_id, Instant::now() + OPTION_CHAIN_TIMEOUT));
    }

    /// A chain reply names its underlying by symbol and echoes no request id,
    /// so it answers the oldest request outstanding for that symbol.
    fn handle_option_chain(&mut self, msg: &[u8], shared: &SharedState) {
        let Some(scopes) = crate::control::contracts::parse_option_chain_response(msg) else { return };

        // One reply can carry the venues of more than one underlying. Handing
        // the lot to the first one's request gives that caller another
        // underlying's strikes, and leaves the other request to time out with
        // an empty chain — so each underlying is answered to the request that
        // asked for it.
        let mut by_underlying: Vec<(String, Vec<_>)> = Vec::new();
        for scope in scopes {
            match by_underlying.iter_mut().find(|(sym, _)| sym.eq_ignore_ascii_case(&scope.symbol)) {
                Some((_, group)) => group.push(scope),
                None => by_underlying.push((scope.symbol.clone(), vec![scope])),
            }
        }
        // An underlying the venue lists nothing for still answers the request,
        // and then the symbol tag is all there is to attribute it by.
        if by_underlying.is_empty() {
            let symbol = extract_tag_value(msg, b"55=").unwrap_or_default();
            by_underlying.push((symbol, Vec::new()));
        }

        for (symbol, scopes) in by_underlying {
            let Some(pos) = self.pending_option_params.iter()
                .position(|(_, pending, _, _)| pending.eq_ignore_ascii_case(&symbol))
            else {
                log::warn!("Option chain reply for '{symbol}' matches no request");
                continue;
            };
            let (req_id, _, con_id, _) = self.pending_option_params.remove(pos);
            log::info!("Option chain reply: req_id={req_id} symbol={symbol} scopes={}", scopes.len());
            shared.reference.push_option_params(req_id, con_id, scopes);
        }
    }

    /// Give up on chain requests the gateway never answered. The request is
    /// ended so the caller stops waiting, and the entry goes with it so it
    /// cannot absorb a later reply for the same underlying.
    pub(crate) fn sweep_pending_option_params(&mut self, shared: &SharedState) {
        if self.pending_option_params.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut expired: Vec<(u32, i64)> = Vec::new();
        self.pending_option_params.retain(|(req_id, symbol, con_id, deadline)| {
            if now >= *deadline {
                log::warn!("Option chain request req_id={req_id} symbol={symbol} unanswered after {OPTION_CHAIN_TIMEOUT:?} — giving up");
                expired.push((*req_id, *con_id));
                false
            } else {
                true
            }
        });
        for (req_id, con_id) in expired {
            shared.reference.push_option_params(req_id, con_id, Vec::new());
        }
    }

    pub(crate) fn send_mkt_depth_exchanges_request(&mut self, _ccp_conn: &mut Option<Connection>, _hb: &mut HeartbeatState, shared: &SharedState) {
        // Depth exchanges are derived from the 6040=102 exchange list received during init.
        // No separate server request needed — just signal the shared state to deliver cached data.
        shared.reference.notify_depth_exchanges();
    }

    /// The exchange directory the session opens with, as the rows a caller
    /// asking which exchanges exist is answered with.
    ///
    /// The venue states an exchange and the name it goes by, in two sections —
    /// shares and derivatives. It states neither what kind of data each
    /// carries nor which group each aggregates into, so neither is stated
    /// here: a field this client filled in itself was read as though the venue
    /// had said it, and a book was gathered from sixty-six venues on four
    /// continents because a section count had been recorded as an aggregation
    /// group. Which venues a book is gathered from comes from the contract's
    /// own definition.
    fn handle_exchange_list(&self, msg: &[u8], shared: &SharedState) {
        use crate::types::DepthMktDataDescription;
        let raw = String::from_utf8_lossy(msg);
        let fields: Vec<&str> = raw.split('\x01').collect();

        // The message has repeating 100=EXCHANGE|6813=NAME pairs grouped by sections.
        // Sections: 6523=category|6811=category_name for stock categories,
        //           8128=N and 8129=N separate stock/derivative sections.
        // Every 100/6813 pair becomes a DepthMktDataDescription entry.
        let mut descs: Vec<DepthMktDataDescription> = Vec::new();
        let mut current_sec_type = "STK".to_string();

        let mut i = 0;
        while i < fields.len() {
            let f = fields[i];
            if f.starts_with("8128=") {
                // Section separator — exchanges above are shares, below are
                // derivatives. The number it carries is how many follow.
                current_sec_type = "STK".to_string();
            } else if f.starts_with("8129=") {
                current_sec_type = "FUT".to_string();
            } else if let Some(exch) = f.strip_prefix("100=") {
                // Next field should be 6813=name
                let name = if i + 1 < fields.len() {
                    fields[i + 1].strip_prefix("6813=").unwrap_or("")
                } else {
                    ""
                };
                descs.push(DepthMktDataDescription {
                    exchange: exch.to_string(),
                    sec_type: current_sec_type.clone(),
                    listing_exch: name.to_string(),
                    // Neither is stated by the venue here.
                    service_data_type: String::new(),
                    agg_group: 0,
                });
                i += 1; // skip the 6813= field
            }
            i += 1;
        }
        log::info!("Parsed {} exchanges from 6040=102", descs.len());
        shared.reference.push_depth_exchanges(descs);
    }

    pub(crate) fn handle_disconnect(
        &mut self,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
    ) {
        self.disconnected = true;
        self.recovery_sweep_at = None;
        // The engine stops believing these statuses here, and said so to
        // nobody — so the API layer went on reporting the pre-disconnect
        // status and `req_open_orders` kept asserting it.
        context.mark_orders_uncertain();
        for order in context.uncertain_orders() {
            let update = uncertain_update(&order, shared.orders.get_order_info(order.order_id));
            shared.orders.push_order_update(update);
            emit(event_tx, Event::OrderUpdate(update));
        }
        // Don't emit Event::Disconnected — auto-reconnect handles CCP drops transparently.
        // Python is only notified if reconnect exhausts retries.
    }

    /// Report the orders the recovery push did not account for.
    ///
    /// Their status stays Uncertain: the engine watched the connection die
    /// with them working and has been told nothing since, so it does not know
    /// whether they filled, were pulled, or are still resting. What it does
    /// know — and what it had no way to say before — is that the recovery is
    /// over and they were not in it. A caller waiting on the reconciliation
    /// that `Uncertain` promises was otherwise waiting on nothing.
    pub(crate) fn sweep_recovery(
        &mut self,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
    ) {
        match self.recovery_sweep_at {
            Some(at) if Instant::now() >= at => self.recovery_sweep_at = None,
            _ => return,
        }
        let stranded = context.uncertain_orders();
        if stranded.is_empty() {
            log::info!("Recovery complete — every order the drop left working is accounted for");
            return;
        }
        log::error!(
            "Recovery complete — {} order(s) it did not account for: {:?}. Their state is not known; \
             reconcile from executions before acting on them.",
            stranded.len(),
            stranded.iter().map(|o| o.order_id).collect::<Vec<_>>(),
        );
        for order in stranded {
            let update = uncertain_update(&order, shared.orders.get_order_info(order.order_id));
            shared.orders.push_order_update(update);
            emit(event_tx, Event::OrderUpdate(update));
        }
    }

    pub(crate) fn reconnect(
        &mut self,
        conn: Connection,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
        account_id: &str,
        market: &crate::engine::market_state::MarketState,
    ) {
        *ccp_conn = Some(conn);
        self.disconnected = false;
        self.recovery_sweep_at = Some(Instant::now() + RECOVERY_PUSH_GRACE);
        hb.last_ccp_sent = Instant::now();
        hb.last_ccp_recv = Instant::now();
        hb.pending_ccp_test = None;

        if let Some(conn) = ccp_conn.as_mut() {
            let ts = chrono_free_timestamp();

            // Re-subscribe to account/position data so server pushes fresh UP/UT/UM messages.
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"), (fix::TAG_SENDING_TIME, &ts),
                (6040, "91"), (1, account_id), (6556, "DR.1"), (6712, "1"),
            ]);
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"), (fix::TAG_SENDING_TIME, &ts),
                (6040, "6"), (6036, "1"), (6095, account_id), (6529, "AR.3"),
            ]);

            // Resting open orders are pushed unsolicited by CCP as 35=8 with
            // 150=0/39=0 carrying originating clientId (6119) and orderId (6121),
            // terminated by 11='*' sentinel.
            hb.last_ccp_sent = Instant::now();
            log::info!("CCP reconnected, sent account/position re-subscribe");
        }

        // News streams belonged to the dead session and are not part of what
        // the server pushes back. Left alone they went quiet for good, with
        // the connection reporting healthy the whole time.
        let stale: Vec<_> = self.news_subscriptions.drain(..).collect();
        let wanted = stale.len();
        for (instrument, req_id, providers) in stale {
            match market.con_id(instrument) {
                Some(con_id) => self.send_news_subscribe(
                    con_id, instrument, &providers, req_id, ccp_conn, hb,
                ),
                None => log::warn!(
                    "CCP reconnect: instrument {instrument} has no contract id, \
                     leaving its news stream unsubscribed",
                ),
            }
        }
        if wanted > 0 {
            log::info!(
                "CCP reconnected, re-subscribed {}/{} news streams",
                self.news_subscriptions.len(), wanted,
            );
        }
    }
}

/// Handle account update messages (cross-cutting, called from CCP message processing).
/// Account figures describing holdings the account does not hold itself.
///
/// Stated the same way as the account's own — a name and a value — and read
/// the same way. What differs is only which set of holdings they describe, and
/// mixing them into the account's own would overstate what it is worth.
fn handle_account_update_elsewhere(
    msg: &[u8],
    shared: &SharedState,
    held: crate::types::HeldElsewhere,
) {
    let Ok(text) = std::str::from_utf8(msg) else { return };
    let mut name: Option<&str> = None;
    let mut stated = 0usize;
    for part in text.split('\x01') {
        if let Some(v) = part.strip_prefix("8001=") {
            name = Some(v);
        } else if let Some(v) = part.strip_prefix("8004=")
            && let Some(n) = name.take() {
                shared.portfolio.set_value_elsewhere(held, n.to_string(), v.to_string());
                stated += 1;
            }
    }
    if stated > 0 {
        log::info!("{stated} account figures for holdings {held:?}");
    }
}

pub(crate) fn handle_account_update(msg: &[u8], context: &mut Context, shared: &SharedState) {
    let text = match std::str::from_utf8(msg) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut key: Option<&str> = None;
    // The venue states which currency a figure is in, and it is not always the
    // account's own. Read per group, and carried rather than assumed.
    let mut currency: &str = "";
    for part in text.split('\x01') {
        if let Some(val) = part.strip_prefix("15=") {
            currency = val;
        } else if let Some(val) = part.strip_prefix("8001=") {
            key = Some(val);
        } else if let Some(val) = part.strip_prefix("8004=")
            && let Some(k) = key {
                // Kept whether or not anything below names it. A figure nobody
                // named is still a figure about the account, and dropping it
                // left no trace that the venue had stated it.
                shared.portfolio.note_account_value(k, val, currency);
                match k {
                    "NetLiquidation" => { if let Ok(v) = val.parse::<f64>() { context.account.net_liquidation = (v * PRICE_SCALE as f64) as Price; } }
                    "BuyingPower" => { if let Ok(v) = val.parse::<f64>() { context.account.buying_power = (v * PRICE_SCALE as f64) as Price; } }
                    "MaintMarginReq" => { if let Ok(v) = val.parse::<f64>() { context.account.margin_used = (v * PRICE_SCALE as f64) as Price; } }
                    "UnrealizedPnL" => { if let Ok(v) = val.parse::<f64>() { context.account.unrealized_pnl = (v * PRICE_SCALE as f64) as Price; } }
                    "RealizedPnL" => { if let Ok(v) = val.parse::<f64>() { context.account.realized_pnl = (v * PRICE_SCALE as f64) as Price; } }
                    "TotalCashValue" => { if let Ok(v) = val.parse::<f64>() { context.account.total_cash_value = (v * PRICE_SCALE as f64) as Price; } }
                    "SettledCash" => { if let Ok(v) = val.parse::<f64>() { context.account.settled_cash = (v * PRICE_SCALE as f64) as Price; } }
                    "AccruedCash" => { if let Ok(v) = val.parse::<f64>() { context.account.accrued_cash = (v * PRICE_SCALE as f64) as Price; } }
                    "EquityWithLoanValue" => { if let Ok(v) = val.parse::<f64>() { context.account.equity_with_loan = (v * PRICE_SCALE as f64) as Price; } }
                    "GrossPositionValue" => { if let Ok(v) = val.parse::<f64>() { context.account.gross_position_value = (v * PRICE_SCALE as f64) as Price; } }
                    "InitMarginReq" | "FullInitMarginReq" => { if let Ok(v) = val.parse::<f64>() { context.account.init_margin_req = (v * PRICE_SCALE as f64) as Price; } }
                    "FullMaintMarginReq" => { if let Ok(v) = val.parse::<f64>() { context.account.maint_margin_req = (v * PRICE_SCALE as f64) as Price; } }
                    "AvailableFunds" | "FullAvailableFunds" => { if let Ok(v) = val.parse::<f64>() { context.account.available_funds = (v * PRICE_SCALE as f64) as Price; } }
                    "ExcessLiquidity" | "FullExcessLiquidity" => { if let Ok(v) = val.parse::<f64>() { context.account.excess_liquidity = (v * PRICE_SCALE as f64) as Price; } }
                    "Cushion" => { if let Ok(v) = val.parse::<f64>() { context.account.cushion = (v * PRICE_SCALE as f64) as Price; } }
                    "SMA" => { if let Ok(v) = val.parse::<f64>() { context.account.sma = (v * PRICE_SCALE as f64) as Price; } }
                    "DayTradesRemaining" => { if let Ok(v) = val.parse::<i64>() { context.account.day_trades_remaining = v; } }
                    "Leverage-S" | "Leverage" => { if let Ok(v) = val.parse::<f64>() { context.account.leverage = (v * PRICE_SCALE as f64) as Price; } }
                    "DailyPnL" => { if let Ok(v) = val.parse::<f64>() { context.account.daily_pnl = (v * PRICE_SCALE as f64) as Price; } }
                    _ => {}
                }
                key = None;
            }
    }
    shared.portfolio.set_account(context.account());
}

/// Handle 6040=143, the venue's daily P&L seeds.
/// Repeating group: 146={count} × (6008=conId, 6064=qtyMidnight, 8223=qtyTraded,
/// 8233=costMidnight, 6822=moneyTraded, 6099=realizedPnl), then 8058 combo
/// buckets repeating the same five fields under an 8020 label. Both counts are
/// hints and the scan delimits itself on the tags, so neither is read.
/// Only the realized figure is stated outright; the rest are what a daily
/// figure is computed from. Values are taken as sent, unscaled.
fn handle_pnl_response(msg: &[u8], shared: &SharedState) {
    let text = match std::str::from_utf8(msg) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut seeds: Vec<MidnightSeed> = Vec::new();
    // The entry the scan is currently filling. `None` between entries and for
    // the duration of a combo bucket, so figures that belong to neither a
    // contract nor this client land nowhere rather than on the last contract.
    let mut current: Option<MidnightSeed> = None;
    let mut request_id = String::new();
    let mut reference_id = String::new();
    for part in text.split('\x01') {
        if let Some(v) = part.strip_prefix("58=") {
            // The body states its own status here. A body that has something to
            // say went wrong, so nothing in it is a figure and the whole thing
            // is abandoned rather than half-read.
            if !v.is_empty() {
                log::warn!("P&L seeds not usable: {v}");
                return;
            }
        } else if let Some(v) = part.strip_prefix("6529=") {
            request_id = v.to_string();
        } else if let Some(v) = part.strip_prefix("8292=") {
            reference_id = v.to_string();
        } else if let Some(v) = part.strip_prefix("6008=") {
            seeds.extend(current.take());
            current = v.parse::<i64>().ok().filter(|&id| id != 0)
                .map(|con_id| MidnightSeed { con_id, ..Default::default() });
        } else if part.starts_with("8020=") {
            // A combo bucket states the same five figures against a label
            // instead of a contract id. Nothing downstream is keyed by a label,
            // so the entry in hand is closed and the bucket's figures are read
            // past rather than folded into the contract that came before it.
            seeds.extend(current.take());
        } else if let Some(seed) = current.as_mut() {
            if let Some(v) = part.strip_prefix("6064=") {
                // Same rule as the position feed above: a quantity that is absent
                // or unparseable is not a flat. Reading it as zero here makes the
                // day's P&L look as though the position were opened intraday. The
                // row is still kept — dropping it says the same thing, because a
                // position with no seed row *is* the intraday case, and it would
                // discard the cash and realized figures the row does carry.
                seed.qty_midnight = v.parse::<f64>().ok().filter(|q| q.is_finite());
            } else if let Some(v) = part.strip_prefix("8223=") {
                seed.qty_traded = v.parse::<f64>().ok().filter(|q| q.is_finite());
            } else if let Some(v) = part.strip_prefix("8233=") {
                // What the venue says the position was worth at midnight, which
                // is the figure the day's change is measured from.
                seed.cost_midnight = v.parse::<f64>().ok().filter(|c| c.is_finite());
            } else if let Some(v) = part.strip_prefix("6822=") {
                // moneyTradedSinceMidnight: signed net cash, SELL positive / BUY
                // negative. Stored with the wire sign; poll_pnl adds it.
                seed.money_traded = v.parse().unwrap_or(0.0);
            } else if let Some(v) = part.strip_prefix("6099=") {
                seed.realized_pnl = v.parse().unwrap_or(0.0);
            }
        }
    }
    seeds.extend(current);
    // The venue answers against the reference it was given and falls back to
    // its own request id when it has none.
    let key = if reference_id.is_empty() { request_id } else { reference_id };
    shared.portfolio.set_midnight_seeds(key, seeds);
}

/// Handle 6040=152, the venue's price table: 146={count} with a list of
/// contract ids in 6008 paired positionally with a list of prices in 8057.
/// The price is stored as text and read where it is used, so one that does not
/// parse costs its own contract and not the table.
fn handle_pnl_prices(msg: &[u8], shared: &SharedState) {
    let text = match std::str::from_utf8(msg) {
        Ok(t) => t,
        Err(_) => return,
    };
    // Both lists are collected in wire order and paired by position, so an
    // unreadable contract id holds its place instead of shifting every price
    // after it onto the wrong contract.
    let mut con_ids: Vec<Option<i64>> = Vec::new();
    let mut prices: Vec<&str> = Vec::new();
    for part in text.split('\x01') {
        if let Some(v) = part.strip_prefix("6008=") {
            con_ids.push(v.parse::<i64>().ok().filter(|&id| id != 0));
        } else if let Some(v) = part.strip_prefix("8057=") {
            prices.push(v);
        }
    }
    let table = con_ids.into_iter().zip(prices)
        .filter_map(|(con_id, price)| Some((con_id?, price.to_string())))
        .collect();
    shared.portfolio.set_venue_prices(table);
}

/// Handle 6040=75 position + market price feed.
/// Fires at init and after each fill. Contains repeating group: 146=count × (6008=conId, 6064=qty, 6101=avgCost).
/// The wire only carries conId/qty/avgCost — no symbol/secType. For any held conId not yet in the
/// reference cache, an internal secdef request goes out so the wrapper-facing Contract is
/// populated by the time `req_positions` is called.
impl CcpState {
    pub(crate) fn handle_position_feed(
        &mut self,
        msg: &[u8],
        ccp_conn: &mut Option<Connection>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        hb: &mut HeartbeatState,
    ) {
    let text = match std::str::from_utf8(msg) {
        Ok(t) => t,
        Err(_) => return,
    };
    // Parse repeating group by scanning for 6008= boundaries
    let mut con_id: i64 = 0;
    // `None` until this entry carries a parseable, finite quantity. A zero
    // default meant an entry without one flattened a live position, published
    // it to reqPositions and both P&L paths, and emitted a PositionUpdate
    // saying flat — the same defect fixed on the account-update path
 //. A genuine flat still arrives as an explicit `6064=0`.
    let mut qty: Option<f64> = None;
    // `None` where the row states no cost. Folding that into a zero made an
    // absent cost indistinguishable from a real one, and publishing it erased
    // the basis of a live holding.
    let mut avg_cost_raw: Option<f64> = None;
    let mut count = 0;
    for part in text.split('\x01') {
        if let Some(v) = part.strip_prefix("6008=") {
            // Flush previous position if any
            if count > 0 && con_id != 0 {
                if let Some(qty) = qty {
                    let avg_cost = basis_for(
                        shared, con_id,
                        avg_cost_raw.map(|c| (c * PRICE_SCALE as f64) as Price), qty,
                    );
                    shared.portfolio.set_position_info(PositionInfo {
                        con_id, position: qty, avg_cost, ..Default::default()
                    });
                    if let Some(instrument) = context.market.instrument_by_con_id(con_id) {
                        adopt_position(context, instrument, qty);
                        shared.portfolio.set_position(instrument, qty);
                        emit(event_tx, Event::PositionUpdate { instrument, con_id, position: qty, avg_cost });
                    }
                }
                self.auto_fetch_secdef_if_cold(con_id, ccp_conn, shared, hb);
            }
            con_id = v.parse().unwrap_or(0);
            qty = None;
            avg_cost_raw = None;
            count += 1;
        } else if let Some(v) = part.strip_prefix("6064=") {
            // Filtered to finite: `"NaN".parse()` succeeds and `NaN as i64`
            // is 0, which would flatten by the same route.
            qty = v.parse::<f64>().ok().filter(|f| f.is_finite());
        } else if let Some(v) = part.strip_prefix("6101=") {
            avg_cost_raw = v.parse::<f64>().ok().filter(|f| f.is_finite());
        }
    }
    // Flush last position
    if count > 0 && con_id != 0 {
        if let Some(qty) = qty {
            let avg_cost = basis_for(
                shared, con_id,
                avg_cost_raw.map(|c| (c * PRICE_SCALE as f64) as Price), qty,
            );
            shared.portfolio.set_position_info(PositionInfo {
                con_id, position: qty, avg_cost, ..Default::default()
            });
            if let Some(instrument) = context.market.instrument_by_con_id(con_id) {
                adopt_position(context, instrument, qty);
                shared.portfolio.set_position(instrument, qty);
                emit(event_tx, Event::PositionUpdate { instrument, con_id, position: qty, avg_cost });
            }
        }
        self.auto_fetch_secdef_if_cold(con_id, ccp_conn, shared, hb);
    }
    }

    /// Issue an internal secdef request for `con_id` where the reference cache is cold and
    /// none has been auto-fetched this session. The reply path populates the cache through
    /// the existing 35=d handler; the response is not tracked.
    fn auto_fetch_secdef_if_cold(
        &mut self,
        con_id: i64,
        ccp_conn: &mut Option<Connection>,
        shared: &SharedState,
        hb: &mut HeartbeatState,
    ) {
        if con_id == 0 { return; }
        if self.auto_fetched_conids.contains(&con_id) { return; }
        if shared.reference.get_contract(con_id).is_some() { return; }
        let req_id = self.next_internal_secdef_id;
        self.next_internal_secdef_id = self.next_internal_secdef_id.wrapping_add(1);
        self.auto_fetched_conids.insert(con_id);
        self.send_secdef_request(req_id, con_id, ccp_conn, hb);
    }

    /// Park a scanner result and dispatch concurrent secdef requests for every cache-miss
    /// con_id. Once all replies arrive (via `try_release_scanner_enrichments`) the result
    /// is pushed to the dispatch queue with the now-warm cache. Mirrors what the gateway
    /// does internally for binary-API scanner clients.
    pub(crate) fn start_scanner_enrichment(
        &mut self,
        api_req_id: u32,
        result: crate::control::scanner::ScannerResult,
        ccp_conn: &mut Option<Connection>,
        shared: &SharedState,
        hb: &mut HeartbeatState,
    ) {
        let mut awaiting: HashSet<i64> = HashSet::new();
        for entry in &result.entries {
            let con_id = entry.con_id as i64;
            if con_id == 0 { continue; }
            if shared.reference.get_contract(con_id).is_some() { continue; }
            awaiting.insert(con_id);
        }
        if awaiting.is_empty() {
            shared.reference.push_scanner_data(api_req_id, result);
            return;
        }
        // Issue one secdef request per cold con_id. If another flow has already
        // requested the same con_id — auto_fetched_conids holds it — the send
        // is skipped and the wait stands: that reply populates the cache and
        // release this entry via try_release_scanner_enrichments.
        for &con_id in &awaiting {
            if !self.auto_fetched_conids.contains(&con_id) {
                let req_id = self.next_internal_secdef_id;
                self.next_internal_secdef_id = self.next_internal_secdef_id.wrapping_add(1);
                self.auto_fetched_conids.insert(con_id);
                self.send_secdef_request(req_id, con_id, ccp_conn, hb);
            }
        }
        self.pending_scanner_enrichment.push(PendingScannerEnrichment {
            api_req_id,
            result,
            awaiting,
            deadline: Instant::now() + Duration::from_secs(5),
        });
    }

    /// Called from the 35=d reply path after the contract cache has been
    /// populated for `con_id`. Removes `con_id` from any pending scanner
    /// enrichment's awaiting set; entries whose set becomes empty are
    /// dispatched to the scanner_data queue.
    pub(crate) fn try_release_scanner_enrichments(&mut self, con_id: i64, shared: &SharedState) {
        if self.pending_scanner_enrichment.is_empty() { return; }
        let mut idx = 0;
        while idx < self.pending_scanner_enrichment.len() {
            self.pending_scanner_enrichment[idx].awaiting.remove(&con_id);
            if self.pending_scanner_enrichment[idx].awaiting.is_empty() {
                let pe = self.pending_scanner_enrichment.swap_remove(idx);
                shared.reference.push_scanner_data(pe.api_req_id, pe.result);
            } else {
                idx += 1;
            }
        }
    }

    /// Flush scanner enrichments past their deadline, dispatching whatever
    /// entries are held, blank fields included where the secdef reply never
    /// arrived. Prevents an indefinite hang on a missing reply.
    pub(crate) fn sweep_scanner_enrichments(&mut self, shared: &SharedState) {
        if self.pending_scanner_enrichment.is_empty() { return; }
        let now = Instant::now();
        let mut idx = 0;
        while idx < self.pending_scanner_enrichment.len() {
            if self.pending_scanner_enrichment[idx].deadline <= now {
                let pe = self.pending_scanner_enrichment.swap_remove(idx);
                log::warn!(
                    "scanner enrichment timeout: req_id={} missing={} con_ids; dispatching partial",
                    pe.api_req_id,
                    pe.awaiting.len(),
                );
                shared.reference.push_scanner_data(pe.api_req_id, pe.result);
            } else {
                idx += 1;
            }
        }
    }
}


/// Fill in a holding's contract once its definition arrives.
///
/// The position feed states a contract id, a quantity and often a cost, and
/// nothing else — so a holding was reported with no symbol at all until some
/// richer message happened to arrive first. The definition is already being
/// fetched for exactly this reason; this is what puts it on the row.
fn identify_position(shared: &SharedState, def: &crate::control::contracts::ContractDefinition) {
    let con_id = def.con_id as i64;
    let Some(existing) = shared.portfolio.position_info(con_id) else { return };
    if !existing.symbol.is_empty() {
        return;
    }
    shared.portfolio.set_position_info(PositionInfo {
        con_id,
        position: existing.position,
        avg_cost: existing.avg_cost,
        symbol: def.symbol.clone(),
        sec_type: def.sec_type.to_api_str().to_string(),
        currency: def.currency.clone(),
        multiplier: if def.multiplier != 1.0 { format!("{}", def.multiplier) } else { String::new() },
        ..Default::default()
    });
}

/// The basis to publish for a position row.
///
/// A row that states one states it. A row that does not leaves the one on file
/// standing, since an absent cost is not a cost of zero — but only while the
/// holding is open: a row closing it takes the basis with it, or the next
/// position in the same contract would inherit the last one's.
fn basis_for(shared: &SharedState, con_id: i64, stated: Option<Price>, qty: f64) -> Price {
    // A closed holding has no basis, and a row closing one has been seen to
    // carry the cost it was closed at. Keeping that leaves the next position
    // in the contract opening against the last one's price.
    // The quantity decides this, and a fractional holding is a holding: taking
    // it from the whole-number field would read half a share as flat and throw
    // away the basis of a position that is open.
    if qty == 0.0 {
        return 0;
    }
    if let Some(c) = stated {
        return c;
    }
    shared.portfolio.position_info(con_id).map(|p| p.avg_cost).unwrap_or(0)
}

/// Take the server's position as the engine's own.
///
/// The callback side reads `context.position`, and a snapshot that reached
/// only the portfolio left it deciding from a number the account had not held
/// since before the connection — flat, on a process that restarted holding
/// stock. The server is the authority here, so the difference is adopted
/// rather than accumulated.
fn adopt_position(context: &mut Context, instrument: InstrumentId, position: f64) {
    let delta = position - context.position(instrument);
    if delta != 0.0 {
        context.update_position(instrument, delta);
    }
}

/// Handle position update messages (cross-cutting, called from CCP message processing).
/// A holding the venue reports apart from the account's own.
///
/// The same fields in the same tags as a holding of the account's own, so it
/// is read the same way — and kept apart, because a caller asking what the
/// account holds does not mean what it holds somewhere else.
fn handle_position_elsewhere(
    parsed: &std::collections::HashMap<u32, String>,
    shared: &SharedState,
    held: crate::types::HeldElsewhere,
) {
    let Some(con_id) = parsed.get(&6008).and_then(|s| s.parse::<i64>().ok()).filter(|id| *id != 0)
    else {
        return;
    };
    // An absent quantity means this frame carries no quantity, not that the
    // holding is gone. Defaulting to zero publishes a real holding as flat, and
    // `"NaN".parse()` succeeds, so a non-finite value does the same by another
    // route. Both are how the two sibling paths went wrong;
    // this one kept the defect after they were fixed.
    let Some(position) = parsed.get(&6064)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite())
    else {
        return;
    };
    let avg_cost = parsed.get(&6101)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .map(|v| (v * PRICE_SCALE as f64) as Price)
        .unwrap_or(0);
    let row = crate::types::PositionElsewhere {
        con_id,
        symbol: parsed.get(&6068).map(|s| s.trim_end().to_string()).unwrap_or_default(),
        sec_type: parsed.get(&167).cloned().unwrap_or_default(),
        currency: parsed.get(&15).cloned().unwrap_or_default(),
        position,
        avg_cost,
        held,
    };
    log::info!("Held elsewhere: {} {:?} x{position}", row.symbol, held);
    shared.portfolio.set_position_elsewhere(row);
}

pub(crate) fn handle_position_update(
    parsed: &std::collections::HashMap<u32, String>,
    context: &mut Context,
    shared: &SharedState,
    event_tx: &Option<SyncSender<Event>>,
) {
    let con_id: i64 = match parsed.get(&6008).and_then(|s| s.parse().ok()) {
        Some(v) => v,
        None => return,
    };
    // An absent quantity means this frame carries no quantity, not that the
    // account is flat. Defaulting to 0 reconciled the engine's position to zero
    // off a marks-only frame and published a flat book to reqPositions and both
    // P&L paths until the next frame that did carry 6064.
    let position_raw: Option<f64> = parsed.get(&6064)
        .and_then(|s| s.parse::<f64>().ok())
        // `"NaN".parse()` succeeds and `NaN as i64` is 0, so a non-finite
        // value would flatten a live position by the same route the absent
        // tag did. Route it to no-data instead.
        .filter(|v| v.is_finite());
    let position: Option<f64> = position_raw;
    // Tag map verified against the updatePortfolio callback:
    // 6101 = averageCost, 6065 = marketPrice (per share), 6067 = marketValue,
    // 6100 = unrealizedPNL, 6099 = realizedPNL. Earlier code read 6065 as the
    // average cost, which is actually the market price.
    let price_tag = |tag: u32| parsed.get(&tag)
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| (v * PRICE_SCALE as f64) as Price)
        .unwrap_or(0);
    // The average cost is written into a row that persists, so an absent tag
    // must not overwrite a real one with zero — the same rule the quantity
    // above follows. Marks are refreshed every frame and are handled apart.
    let avg_cost: Option<Price> = parsed.get(&6101)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .map(|v| (v * PRICE_SCALE as f64) as Price);
    let market_price: Price = price_tag(6065);
    let market_value: Price = price_tag(6067);
    let unrealized_pnl: Price = price_tag(6100);
    let realized_pnl: Price = price_tag(6099);
    // Symbol arrives space-padded; trim trailing whitespace.
    let symbol = parsed.get(&6068).map(|s| s.trim_end().to_string()).unwrap_or_default();
    let sec_type = parsed.get(&167).cloned().unwrap_or_default();
    let currency = parsed.get(&15).cloned().unwrap_or_default();
    let multiplier = parsed.get(&8002).cloned().unwrap_or_default();

    let Some(position) = position else {
        // Marks-only frame. Apply the marks to a row that already exists, but do
        // not create one: set_position_marks inserts a default PositionInfo, and
        // a row conjured here would carry position 0 and read as flat — the very
        // thing this is fixing. Ordering matters for the same reason, so the
        // quantity-bearing path below still writes the info row first.
        if shared.portfolio.position_info(con_id).is_some() {
            shared.portfolio.set_position_marks(con_id, market_price, market_value, unrealized_pnl, realized_pnl);
        }
        return;
    };

    // Always store position info for reqPositions/pnlSingle, regardless of instrument registry.
    let avg_cost = basis_for(shared, con_id, avg_cost, position_raw.unwrap_or(0.0));
    shared.portfolio.set_position_info(PositionInfo {
        con_id, position, avg_cost,
        symbol, sec_type, currency, multiplier,
        ..Default::default()
    });
    shared.portfolio.set_position_marks(con_id, market_price, market_value, unrealized_pnl, realized_pnl);

    if let Some(instrument) = context.market.instrument_by_con_id(con_id) {
        let current = context.position(instrument);
        let delta = position - current;
        if delta != 0.0 {
            context.update_position(instrument, delta);
        }
        shared.portfolio.set_position(instrument, position);
        emit(event_tx, Event::PositionUpdate { instrument, con_id, position, avg_cost });
    }
}

#[cfg(test)]
mod tests;
