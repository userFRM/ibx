use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

/// How long a matching-symbols request waits for its reply. Matches the
/// historical-request idle timeout: both are one round trip to the gateway.
const MATCHING_SYMBOLS_TIMEOUT: Duration = Duration::from_secs(60);

/// How long an option chain request waits for its reply. Also one round trip,
/// for a reply that carries every class of an underlying at once.
const OPTION_CHAIN_TIMEOUT: Duration = Duration::from_secs(60);

use crate::bridge::{Event, SharedState};
use crate::engine::context::Context;
use crate::protocol::datetime::chrono_free_timestamp;
use crate::protocol::connection::Connection;
use crate::protocol::fix;
use crate::types::{
    InstrumentId, NewsBulletin,
    PositionInfo,
};

use super::{HeartbeatState, emit, clone_for_event, parse_price_tag, decode_tif, EventSink};

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

/// Convert a FIX OrderID hex string (e.g. "00cf16ed.000225ed.69ca0941.0001") to a
/// stable i64 permId.
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

/// What a fill cost, as the venue states it.
///
/// The execution report carries no commission tag — captured against real
/// fills on two instruments, it simply is not there — so a charge taken from
/// the report is always nothing. The venue states it on a record of its own
/// that follows the report, naming the execution it belongs to, the amount
/// and the currency it is charged in.
///
/// The quantities and the price on this record are the same fill the
/// execution report already carried, and are left alone: booking the fill
/// from both is how it would be counted twice. Only what it cost is taken.
fn handle_trade_charge(parsed: &std::collections::HashMap<u32, String>, shared: &SharedState) {
    let Some(exec_id) = parsed.get(&fix::TAG_EXEC_ID).filter(|s| !s.is_empty()) else {
        return;
    };
    // Absent is not nothing: a charge the venue did not state is unstated,
    // and reporting a zero for it is the number this was written to stop.
    let Some(charged) = parsed.get(&fix::TAG_TRADE_CHARGE)
        .and_then(|s| s.parse::<f64>().ok())
    else {
        return;
    };
    shared.orders.push_charge(crate::types::model::CommissionAndFeesReport::charged(
        exec_id,
        charged,
        parsed.get(&fix::TAG_TRADE_CHARGE_CURRENCY).map(String::as_str).unwrap_or(""),
    ));
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
        "18" => Some(
            "it states the venue's clock. Every message the venue sends carries the time it \
             sent it, which this client keeps as it arrives, so a message stating the same \
             clock again adds nothing to answer a caller with",
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
pub(crate) fn contract_named(cmd: &crate::types::ControlCommand) -> Option<&crate::types::ContractRef> {
    let contract = contract_of(cmd)?;
    // Only one the venue has not named yet: the rest already carry its id.
    (contract.con_id == 0).then_some(contract)
}

/// The contract a request names, whether or not the venue has numbered it.
fn contract_of(cmd: &crate::types::ControlCommand) -> Option<&crate::types::ContractRef> {
    use crate::types::ControlCommand as C;
    match cmd {
        C::FetchHistorical { contract, .. }
        | C::FetchHeadTimestamp { contract, .. }
        | C::FetchHistoricalTicks { contract, .. }
        | C::FetchHistoricalSchedule { contract, .. }
        | C::SubscribeRealTimeBar { contract, .. }
        | C::SubscribeDepth { contract, .. } => Some(contract),
        _ => None,
    }
}

/// The filters that go with that contract.
fn filters_named(cmd: &crate::types::ControlCommand) -> crate::types::SecDefFilters {
    use crate::types::ControlCommand as C;
    match cmd {
        C::FetchHistorical { filters, .. }
        | C::FetchHeadTimestamp { filters, .. }
        | C::FetchHistoricalTicks { filters, .. }
        | C::FetchHistoricalSchedule { filters, .. }
        | C::SubscribeRealTimeBar { filters, .. }
        | C::SubscribeDepth { filters, .. } => filters.clone(),
        _ => crate::types::SecDefFilters::default(),
    }
}

/// Fill in the id the venue has given the contract a request named.
fn name_the_contract(cmd: &mut crate::types::ControlCommand, id: i64) {
    use crate::types::ControlCommand as C;
    match cmd {
        C::FetchHistorical { contract, .. }
        | C::FetchHeadTimestamp { contract, .. }
        | C::FetchHistoricalTicks { contract, .. }
        | C::FetchHistoricalSchedule { contract, .. }
        | C::SubscribeRealTimeBar { contract, .. }
        | C::SubscribeDepth { contract, .. } => contract.con_id = id,
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
    /// Live news subscriptions: instrument, request id, the providers the
    /// caller asked for, and the contract. The providers are kept because a
    /// reconnect has to send the same request again, and the request is the
    /// only place they appear; the contract because the withdrawal names it.
    pub(crate) news_subscriptions: Vec<(InstrumentId, u32, String, i64, String)>,
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
    /// HMAC signing key for XML-carrying CCP messages (selective signing).
    pub(crate) ccp_sign_key: Vec<u8>,
    /// HMAC signing IV — advances only for signed messages, independent of unsigned
    /// ones.
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
    /// The number the next advisor-configuration request states as its own.
    ///
    /// These are counted from one for the session, and the request sends the
    /// count as a string, so a reply can be matched to the question that asked
    /// it.
    pub(crate) next_advisor_request: u32,
    /// The key the open account subscription was asked for under, so the
    /// withdrawal can name it. Tag 6036 carries whether the request opens the
    /// subscription or closes it; a session that only ever opens them holds one
    /// per loop for the life of the connection.
    pub(crate) account_request_key: Option<String>,
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
    /// conIds a secdef has been fetched for without a caller asking, and the
    /// request that fetched each. The request is kept so a fetch that is never
    /// answered can be forgotten; held indefinitely, one lost request leaves
    /// that contract unasked for the life of the session and every position on
    /// it unnamed.
    pub(crate) auto_fetched_conids: HashMap<i64, u32>,
    /// Scanner results awaiting per-conId contract-detail enrichment.
    /// Each entry parks a parsed `<ScanResponse>` until every con_id the
    /// cache missed has been resolved via the same 35=d path that user-initiated
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
    /// Which legs have answered.
    ///
    /// The exchanges that have answered. A fan-out ends when every exchange it
    /// asked has answered. Counting frames instead completes it twice over for
    /// a leg answered with more than one row, and drops the legs still
    /// outstanding.
    pub answered: Vec<String>,
    /// Idle deadline, refreshed on every per-exchange reply.
    ///
    /// A fan-out asks each exchange the contract lists on and ends when every
    /// one has answered. One reply lost or unreadable would leave the count
    /// short for good, so this bounds the wait — but reaching it is a failed
    /// request, reported as one, not the ordinary way a fan-out finishes.
    pub deadline: Instant,
}

impl CcpState {
    pub(crate) fn new() -> Self {
        Self {
            seen_exec_ids: HashSet::with_capacity(256),
            exec_id_order: VecDeque::with_capacity(256),
            news_subscriptions: Vec::new(),
            disconnected: false,
            recovery_sweep_at: None,
            hydrated_any: false,
            pending_secdef: Vec::new(),
            pending_matching_symbols: Vec::new(),
            pending_option_params: Vec::new(),
            ccp_sign_key: Vec::new(),
            ccp_sign_iv: std::sync::Mutex::new(Vec::new()),
            pending_schedule_pair: Vec::new(),
            next_schedule_sub_id: 1,
            pending_fanout: Vec::new(),
            details_delivered: std::collections::HashMap::new(),
            next_fanout_id: 1,
            next_internal_secdef_id: 0xF000_0000,
            next_advisor_request: 1,
            account_request_key: None,
            unread_subtypes: std::collections::HashSet::new(),
            unread_types: std::collections::HashSet::new(),
            pending_md_subscribe: Vec::new(),
            resolved_md_subscribe: Vec::new(),
            pending_named: Vec::new(),
            resolved_named: Vec::new(),
            auto_fetched_conids: HashMap::new(),
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

    pub(crate) fn process_ccp_message(
        &mut self,
        msg: &[u8],
        ccp_conn: &mut Option<Connection>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<EventSink>,
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
                            // Position + market price feed (init burst + after each
                            // fill). Not the end of the batch: the account's own
                            // figures follow it, and calling the download
                            // complete here answered a caller before they
                            // arrived. The venue ends the batch itself, below.
                            self.handle_position_feed(msg, ccp_conn, context, shared, event_tx, hb);
                        }
                        "77" => self.handle_account_summary(&parsed, context, shared),
                        "143" => {
                            // P&L midnight seed — store for client-side daily P&L
                            // computation
                            positions::handle_pnl_response(msg, shared);
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
                                    // An empty result is an answer in its own
                                    // right ("no such symbol") and is
                                    // delivered. Dropped, the caller waits
                                    // indefinitely and the stale queue head
                                    // misattributes every later reply.
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
                        // The venue's error channel. Two subtypes, one
                        // channel: which number it arrives under depends only
                        // on a capability the session negotiated at logon, not
                        // on the error.
                        "60" => handle_trade_charge(&parsed, shared),
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
            "UT" | "UM" | "RL" => positions::handle_account_update(msg, context, shared),
            // The same figures, for the sets of holdings the account does not
            // hold itself. Applied to the account's own they would overstate
            // what it is worth, so they are kept where the holdings they
            // describe are kept.
            "AL" => handle_account_update_elsewhere(msg, shared, crate::types::HeldElsewhere::Away),
            "UL" => handle_account_update_elsewhere(msg, shared, crate::types::HeldElsewhere::Aside),
            // One frame, every holding it names — see `split_position_entries`.
            "UP" => {
                for one in positions::split_position_entries(msg) {
                    positions::handle_position_update(&one, context, shared, event_tx);
                }
            }
            // The venue keeps three sets of holdings and this client read one.
            // The others carry the same fields in the same tags — they differ
            // only in which set they belong to — and were discarded, so a
            // caller could not learn the account held anything away at all.
            //
            // One frame, every holding it names, the same way the account's own
            // set is read. Handed the flat map instead, a frame naming several
            // holdings arrived as one: the generic parser keeps the last value
            // of a repeated tag, so every holding but the last was gone before
            // anything could see it.
            "AP" | "DO" | "DP" => {
                let held = match msg_type {
                    "AP" => crate::types::HeldElsewhere::Away,
                    "DO" => crate::types::HeldElsewhere::DisplayOnly,
                    _ => crate::types::HeldElsewhere::Aside,
                };
                for one in positions::split_position_entries(msg) {
                    positions::handle_position_elsewhere(&one, shared, held);
                }
            }
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
                            shared.reference.cache_contract(
                            def.con_id as i64,
                            // Mapped where every other reader of a
                            // definition maps it. Written out here, an
                            // option was cached without its strike, its
                            // right, its expiry or its multiplier — which
                            // is all that tells two options on one
                            // underlying apart.
                            crate::types::model::ContractDetails::from_definition(&def).contract,
                        );
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
                if let (Some(idx), Some(rid)) = (fanout_idx, response_req_id.as_ref()) {
                    if let Some(def) = crate::control::contracts::parse_secdef_response(msg, shared.island_for_nasdaq()) {
                        let api_req_id = self.pending_fanout[idx].api_req_id;
                        // No con_id is "no definition for this
                        // exchange" — cache nothing and emit no row. The leg
                        // still counts toward the fan-out below, so the
                        // request completes.
                        if def.con_id != 0 {
                            shared.reference.cache_contract(
                            def.con_id as i64,
                            // Mapped where every other reader of a
                            // definition maps it. Written out here, an
                            // option was cached without its strike, its
                            // right, its expiry or its multiplier — which
                            // is all that tells two options on one
                            // underlying apart.
                            crate::types::model::ContractDetails::from_definition(&def).contract,
                        );
                            identify_position(shared, &def);
                            self.try_release_scanner_enrichments(def.con_id as i64, shared);
                            // The master row for this same contract may be
                            // parked waiting for its trading hours. It is the
                            // richer of the two and claims the same dedup slot,
                            // so delivering this one first hands the caller a
                            // contract with no trading or liquid hours and
                            // drops the enriched row when it arrives.
                            let awaiting_schedule = self.pending_schedule_pair.iter().any(|p| {
                                p.api_req_id == api_req_id && p.def.con_id == def.con_id
                            });
                            if !awaiting_schedule
                                && self.details_delivered.entry(api_req_id).or_default().insert(def.con_id as i64)
                            {
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
                        if !self.pending_fanout[idx].answered.iter().any(|id| id == rid) {
                            self.pending_fanout[idx].answered.push(rid.clone());
                        }
                        self.pending_fanout[idx].deadline = Instant::now() + SECDEF_TIMEOUT;
                        if self.pending_fanout[idx].answered.len() >= self.pending_fanout[idx].fanout_req_ids.len() {
                            self.pending_fanout.swap_remove(idx);
                            // The master row may still be parked awaiting its
                            // schedule. Ending here would order the end before
                            // the row, so the pair carries it — the same way the
                            // single-exchange case above hands the end over.
                            match self.pending_schedule_pair.iter_mut()
                                .find(|p| p.api_req_id == api_req_id)
                            {
                                Some(pair) => pair.is_last = true,
                                None => {
                                    shared.reference.push_contract_details_end(api_req_id);
                                    emit(event_tx, Event::ContractDetailsEnd(api_req_id));
                                }
                            }
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
                        shared.reference.cache_contract(
                            def.con_id as i64,
                            // Mapped where every other reader of a
                            // definition maps it. Written out here, an
                            // option was cached without its strike, its
                            // right, its expiry or its multiplier — which
                            // is all that tells two options on one
                            // underlying apart.
                            crate::types::model::ContractDetails::from_definition(&def).contract,
                        );
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
                                    answered: Vec::new(),
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

    fn handle_news_bulletin(&mut self, parsed: &std::collections::HashMap<u32, String>, shared: &SharedState) {
        // The urgency the venue states, and the kind a caller is told about.
        // They are not the same numbering and they are not in the same order:
        // the venue's second kind is an exchange that has stopped trading and
        // its third is one that has started, while a caller reads those the
        // other way round. Passed straight through, a caller halting on an
        // exchange going down acted on one coming up. The last three are
        // kinds of their own — plain text, a message meant to be shown, and
        // one written as markup — and were all reported as ordinary news.
        static BULLETIN_TYPE_MAP: &[(i32, i32)] = &[
            (1, 1), (2, 3), (3, 2), (8, 4), (9, 5), (10, 6),
        ];
        let fix_type: i32 = parsed.get(&fix::TAG_URGENCY)
            .and_then(|s| s.parse().ok()).unwrap_or(0);
        let api_type = BULLETIN_TYPE_MAP.iter()
            .find(|(k, _)| *k == fix_type)
            .map(|(_, v)| *v);
        let api_type = match api_type {
            Some(t) => t,
            None => {
                // Dropped, and said so. A bulletin whose urgency this does not
                // name is still a bulletin the venue sent, and returning here in
                // silence left no callback, no log and nothing in the unread
                // record to say a message had arrived and gone nowhere.
                shared.market.note_unread_wire(
                    "trading",
                    format!("news bulletin urgency {fix_type}"),
                );
                log::warn!(
                    "news bulletin states urgency {fix_type}, which names no bulletin type here — dropped",
                );
                return;
            }
        };
        let message = parsed.get(&fix::TAG_HEADLINE).cloned().unwrap_or_default();
        let exchange = parsed.get(&fix::TAG_SECURITY_EXCHANGE).cloned().unwrap_or_default();
        // The venue numbers its own bulletins and states the number here.
        // Counted locally instead, the numbering started again at every
        // connect and named nothing the venue would recognise, so the same
        // bulletin arriving twice across a reconnect could not be told from
        // two. Absent, it stands at the widest number a bulletin id is
        // carried under, which is what says nothing was stated.
        let msg_id = parsed.get(&fix::TAG_BULLETIN_ID)
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(i32::MAX);
        let bulletin = NewsBulletin {
            msg_id,
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
        event_tx: &Option<EventSink>,
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
        let mut lost_auto_fetch: Vec<u32> = Vec::new();
        self.pending_secdef.retain(|(req_id, _, deadline)| {
            if now >= *deadline || over.is_some() {
                if *req_id < 0xF000_0000 {
                    expired.push(*req_id);
                } else {
                    log::warn!("Internal secdef timeout: req_id={req_id:#x}");
                    // Forgotten, so the next report naming that contract asks
                    // again. Held, one lost request leaves the contract unnamed
                    // for the life of the session.
                    lost_auto_fetch.push(*req_id);
                }
                false
            } else {
                true
            }
        });
        if !lost_auto_fetch.is_empty() {
            self.auto_fetched_conids.retain(|_, rid| !lost_auto_fetch.contains(rid));
        }
        self.pending_fanout.retain(|p| {
            if now >= p.deadline || over.is_some() {
                log::warn!(
                    "Contract-details fan-out timeout: api_req_id={} received {} of {}",
                    p.api_req_id, p.answered.len(), p.fanout_req_ids.len(),
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
                    crate::error_codes::Refusal::NOT_CONNECTED,
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
        event_tx: &Option<EventSink>,
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

    /// Match a 6040=107 schedule reply to a pending secdef pair and emit merged
    /// details.
    fn handle_schedule_reply(
        &mut self,
        msg: &[u8],
        shared: &SharedState,
        event_tx: &Option<EventSink>,
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
            // On the clock the venue names beside them. Where that zone is
            // one no database here answers to, the hours stay as the wire
            // carried them and the zone is reported as the UTC they are on,
            // so a caller is never given a name the times do not match.
            let named = sched.timezone.as_str();
            let stated_on_the_named_clock =
                crate::control::contracts::sessions_are_stated_on(named);
            if !stated_on_the_named_clock {
                pair.def.time_zone_id = Some("UTC".to_string());
            }
            pair.def.trading_hours = Some(
                crate::control::contracts::format_sessions_string(&sched.trading_hours, named)
            );
            pair.def.liquid_hours = Some(
                crate::control::contracts::format_sessions_string(&sched.liquid_hours, named)
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
            // Which partition, and which request. The partition rides 6906 and
            // 6158 is the request's own number, counted from one for the
            // session and sent as a string, which the reply carries back so an
            // answer can be matched to its question. Writing the partition
            // into 6158 and omitting 6906 leaves every advisor request naming
            // no partition, so a replacement carries a document for a partition
            // none of them named. The two are written in this order.
            let key = self.next_advisor_request.to_string();
            self.next_advisor_request = self.next_advisor_request.wrapping_add(1);
            let mut fields: Vec<(u32, &str)> = vec![
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (6040, "116"),
                (6905, &command),
                (6158, &key),
                (6906, partition),
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

    /// Ask the venue to state the account's figures now.
    ///
    /// The same pair the connection sends when it re-establishes itself: the
    /// keyed account request on 6040=6, and the display request on 6040=91 that
    /// carries the positions beside it. Subscribing alone does not produce
    /// them — the venue restates them on its own schedule, and a session that
    /// has just opened waits tens of seconds for its first set.
    pub(crate) fn send_account_refresh(
        &mut self,
        account: &str,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        let Some(conn) = ccp_conn.as_mut() else { return };
        let ts = chrono_free_timestamp();
        let _ = conn.send_fix(&[
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &ts),
            (6040, "91"),
            (1, account),
            (6556, "DR.1"),
            (6712, "1"),
        ]);
        // The key is unique for the life of the process, not of this state.
        // The venue keys the subscription on 6529 and answers a key it is
        // already serving with nothing; a connection outlives the loops that
        // use it, so a counter reset with each loop asks under a key the
        // connection has already seen and is not answered at all. The opening
        // sequence has used AR.1.
        let key = self.next_account_request_key();
        let _ = conn.send_fix(&[
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &ts),
            (6040, "6"),
            (6036, "1"),
            (6095, account),
            (6529, &key),
        ]);
        hb.last_ccp_sent = Instant::now();
    }

    /// The next key to ask for account and position data under, recorded as
    /// the one this state now holds.
    ///
    /// Unique for the life of the process, not of this state. The venue keys
    /// the subscription on 6529 and answers a key it is already serving with
    /// nothing, and a connection outlives the loops that use it — so every
    /// place that asks draws from here rather than naming a key of its own.
    /// A reconnect that named one directly asked under a key a refresh had
    /// already spent, was answered with nothing, and the position pushes did
    /// not resume.
    fn next_account_request_key(&mut self) -> String {
        static NEXT_ACCOUNT_REQUEST: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(2);
        let n = NEXT_ACCOUNT_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let key = format!("AR.{n}");
        self.account_request_key = Some(key.clone());
        key
    }

    /// life of the connection and the venue stops answering new ones.
    /// Send P&L subscribe: 6040=142, 6529=PLR.{N}, 1={account}.
    /// Close the account subscription this state opened.
    ///
    /// Tag 6036 states whether the request opens the subscription or closes it,
    /// and the key on 6529 names which. Left open, each loop holds one for the
    /// life of the connection and the venue stops answering new ones.
    pub(crate) fn send_account_unsubscribe(
        &mut self,
        account: &str,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        let Some(key) = self.account_request_key.take() else { return };
        let Some(conn) = ccp_conn.as_mut() else { return };
        let ts = chrono_free_timestamp();
        let _ = conn.send_fix(&[
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &ts),
            (6040, "6"),
            (6036, "0"),
            (6095, account),
            (6529, &key),
        ]);
        hb.last_ccp_sent = Instant::now();
    }

    pub(crate) fn send_pnl_subscribe(
        &mut self,
        req_id: i64,
        account: &str,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        if let Some(conn) = ccp_conn.as_mut() {
            // The key names the request; the account rides tag 1 beside it, as
            // the protocol defines it and as the opening sequence in
            // `logon::send_post_burst_grace` already wrote it. Written into the
            // key instead, the account rides as literal bytes inside a field
            // that names a request, and tag 1 is not sent at all.
            let pnl_key = format!("PLR.{req_id}");
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (6040, "142"),
                (6529, &pnl_key),
                (1, account),
            ]);
            hb.last_ccp_sent = Instant::now();
            log::info!("Sent P&L subscribe: req_id={req_id} account={account}");
        }
    }

    pub(crate) fn send_news_subscribe(
        &mut self,
        con_id: i64,
        instrument: InstrumentId,
        sec_type: &str,
        providers: &str,
        req_id: u32,
        ccp_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        self.news_subscriptions.push((instrument, req_id, providers.to_string(), con_id, sec_type.to_string()));
        if let Some(conn) = ccp_conn.as_mut() {
            let req_id_str = req_id.to_string();
            let con_id_str = (con_id as u32).to_string();
            let stated_type = crate::control::contracts::sec_type_to_fix(sec_type).to_string();
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                (fix::TAG_SENDING_TIME, &ts),
                (263, "1"),
                (146, "1"),
                (262, &req_id_str),
                (6008, &con_id_str),
                (207, "NEWS"),
                // What the contract is, as the venue states it. Stamped as a
                // US stock, headlines for a future or an index went out
                // describing something else.
                (167, &stated_type),
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
        let (req_id, con_id) =
            match self.news_subscriptions.iter().position(|(id, ..)| *id == instrument) {
                Some(pos) => {
                    let (_, rid, _, con_id, _) = self.news_subscriptions.remove(pos);
                    (rid, con_id)
                }
                None => return,
            };
        if let Some(conn) = ccp_conn.as_mut() {
            let req_id_str = req_id.to_string();
            let con_id_str = (con_id as u32).to_string();
            // Withdrawn the way it was asked for: the venue is told which tick,
            // on which contract, not merely which request. The option model
            // beside it is already withdrawn that way, and the protocol
            // writes the same group on a withdrawal as on a subscription: the
            // action, then the number of entries, then each entry's request id,
            // contract, venue and type — the entry is written the same way
            // whichever action it belongs to.
            let sent = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                (263, "2"),
                (146, "1"),
                (262, &req_id_str),
                (6008, &con_id_str),
                (207, "NEWS"),
                (167, "CS"),
                (264, "292"),
            ]);
            hb.last_ccp_sent = Instant::now();
            match sent {
                Ok(()) => log::info!(
                    "Sent news unsubscribe: instrument={instrument:?} req_id={req_id}",
                ),
                // Reported as what it is. Logged as sent, a withdrawal that
                // never left reads as one the venue took, and headlines keep
                // arriving for a subscription no caller is reading.
                Err(e) => log::warn!(
                    "News unsubscribe for instrument={instrument:?} req_id={req_id} was not \
                     sent: {e}; the venue goes on serving it until the session ends",
                ),
            }
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
        // Cloned rather than borrowed: the command is moved onto the pending
        // list below, and what it named has to outlive it.
        let named = match contract_named(&cmd) {
            Some(c) if !c.symbol.is_empty() => c.clone(),
            _ => return Some(cmd),
        };
        let filters = filters_named(&cmd);
        let req_id = self.next_internal_secdef_id;
        self.next_internal_secdef_id = self.next_internal_secdef_id.wrapping_add(1);
        self.pending_named.push((req_id, cmd, Instant::now()));
        self.send_secdef_request_by_symbol(
            req_id, &named.symbol, &named.sec_type, &named.exchange, &named.currency,
            &filters, ccp_conn, hb,
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
            let Some(named) = contract_named(&cmd) else { continue };
            let reason = format!(
                "no security definition has been found for {} {} on {}, so the \
                 request could not be sent",
                named.sec_type, named.symbol, named.exchange,
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
            // A public identifier and the tags it rides on. Each kind has its
            // own: a CUSIP goes out as 454=1|455=<id>|456=1, and 22/48 carry an
            // ISIN or a FIGI under the character that names the source rather
            // than a number. Sent as 22=1|48=<cusip>, a CUSIP named a source on
            // the pair that carries an ISIN, and a FIGI was not
            // sent at all — the lookup fell through to whatever the symbol
            // matched. When one is set the lookup rides the identifier and
            // drops the symbol/secType/filters.
            let sec_id = filters.sec_id.as_str();
            let identifier_fields: Vec<(u32, &str)> = if sec_id.is_empty() {
                Vec::new()
            } else {
                match filters.sec_id_type.to_uppercase().as_str() {
                    "CUSIP" => vec![(454, "1"), (455, sec_id), (456, "1")],
                    "ISIN" => vec![(22, "4"), (48, sec_id)],
                    "FIGI" => vec![(22, "S"), (48, sec_id)],
                    // The caller named an identifier and a kind this client
                    // states no source for, so the lookup below asks by symbol
                    // instead. That is a different question, and answering it
                    // without saying so hands back whatever the symbol matches.
                    other => {
                        log::warn!(
                            "contract lookup states a {other} identifier, which this client \
                             carries no source for; asking by symbol instead",
                        );
                        Vec::new()
                    }
                }
            };
            let identifier_lookup = !identifier_fields.is_empty();

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
                // symbol/secType/filters; exchange and currency still ride.
                fields.extend_from_slice(&identifier_fields);
            } else {
                // Both, where the caller stated both. The protocol carries
                // the symbol and then the venue's local symbol from
                // separate fields of the request, and neither suppresses the
                // other. Sending only
                // the local symbol asked a narrower question than the caller
                // put, and a symbol that disagrees with it — which the venue
                // would refuse — matched whatever the local symbol named.
                if !symbol.is_empty() {
                    fields.push((55, symbol));
                }
                if !filters.local_symbol.is_empty() {
                    fields.push((6035, &filters.local_symbol));
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

    pub(crate) fn send_matching_symbols_request(&mut self, req_id: u32, pattern: &str, ccp_conn: &mut Option<Connection>, hb: &mut HeartbeatState, shared: &SharedState) {
        // Recorded only where the request went out, so a request issued while
        // the transport is down is not queued as pending with nothing on the
        // wire to answer it.
        let Some(conn) = ccp_conn.as_mut() else {
            log::warn!("Matching symbols request req_id={req_id} pattern='{pattern}' not sent: no CCP transport");
            // Answered empty rather than left unanswered, the same as a chain
            // request that cannot go out: the caller is blocked on the end of a
            // request nothing on the wire will ever end.
            shared.reference.push_matching_symbols(req_id, Vec::new());
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
            shared.reference.push_matching_symbols(req_id, Vec::new());
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
    pub(crate) fn sweep_pending_matching_symbols(&mut self, shared: &SharedState) {
        if self.pending_matching_symbols.is_empty() {
            return;
        }
        let now = Instant::now();
        self.pending_matching_symbols.retain(|(req_id, deadline)| {
            if now >= *deadline {
                log::warn!("Matching symbols request req_id={req_id} unanswered after {MATCHING_SYMBOLS_TIMEOUT:?} — giving up");
                // The timeout is answered, not merely recorded: a caller
                // told nothing waits on a request this session has abandoned.
                // An empty answer is the shape of a lookup that found nothing,
                // which is what a timeout amounts to.
                shared.reference.push_matching_symbols(*req_id, Vec::new());
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
        // options on it.
        //
        // A caller who states nothing claims nothing. An unstated type sent as
        // STK asks about a stock of that symbol where the caller meant an index
        // or a future. Tag 310 is omitted for an empty
        // security type rather than standing one in: its chain-request writer
        // states the type only when the caller gave one, and treats its own
        // empty-named type as absent.
        let underlying = underlying_sec_type;
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
        if !underlying.is_empty() {
            fields.push((310, underlying));
        }
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
        // Depth exchanges are derived from the 6040=102 exchange list received during
        // init.
        // No separate server request needed — just signal the shared state to deliver
        // cached data.
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
        event_tx: &Option<EventSink>,
    ) {
        self.disconnected = true;
        self.recovery_sweep_at = None;
        // The engine stops believing these statuses here, and said so to
        // nobody — so the API layer went on reporting the pre-disconnect
        // status and `req_open_orders` kept asserting it.
        context.mark_orders_uncertain();
        for order in context.uncertain_orders() {
            let update = executions::uncertain_update(&order, shared.orders.get_order_info(order.order_id));
            shared.orders.push_order_update(update);
            emit(event_tx, Event::OrderUpdate(update));
        }
        // Don't emit Event::Disconnected — auto-reconnect handles CCP drops
        // transparently.
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
        event_tx: &Option<EventSink>,
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
            let update = executions::uncertain_update(&order, shared.orders.get_order_info(order.order_id));
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
        shared: &SharedState,
    ) {
        *ccp_conn = Some(conn);
        self.disconnected = false;
        // This connection has named nothing yet. Both of these said otherwise
        // from the connection before it, so a caller asking what it had on was
        // answered at once from the pre-drop book — every order in it
        // Uncertain — while the venue's account of what it holds was still
        // on its way. Cleared here, that caller waits for the new push the way
        // it waited for the first.
        self.hydrated_any = false;
        shared.orders.clear_replay_done();
        self.recovery_sweep_at = Some(Instant::now() + RECOVERY_PUSH_GRACE);
        // This connection has not yet named what it has working, and neither
        // "none" nor the last connection's answer is that. Left set from
        // before, a caller asking what it has on at the moment it reconnects is
        // answered from the old session's record without waiting for the new
        // one's, which is how the same order is placed twice.
        self.hydrated_any = false;
        shared.orders.replay_is_pending();
        hb.last_ccp_sent = Instant::now();
        hb.last_ccp_recv = Instant::now();
        hb.pending_ccp_test = None;

        if let Some(conn) = ccp_conn.as_mut() {
            let ts = chrono_free_timestamp();

            // Re-subscribe to account/position data so server pushes fresh UP/UT/UM
            // messages.
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"), (fix::TAG_SENDING_TIME, &ts),
                (6040, "91"), (1, account_id), (6556, "DR.1"), (6712, "1"),
            ]);
            // Drawn from the counter, not named here. The venue answers a key
            // it is already serving with nothing, and the refreshes on this
            // connection have been spending keys from that counter since the
            // session opened — so a fixed one is a key that has very likely
            // already been used, and the account and position pushes this asks
            // for simply do not resume. Recorded too, so the unsubscribe that
            // follows closes the key this connection is actually served under.
            let key = self.next_account_request_key();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"), (fix::TAG_SENDING_TIME, &ts),
                (6040, "6"), (6036, "1"), (6095, account_id), (6529, &key),
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
        let stale = std::mem::take(&mut self.news_subscriptions);
        let wanted = stale.len();
        for (instrument, req_id, providers, _, sec_type) in stale {
            match market.con_id(instrument) {
                Some(con_id) => self.send_news_subscribe(
                    con_id, instrument, &sec_type, &providers, req_id, ccp_conn, hb,
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
/// Fires at init and after each fill. Contains repeating group: 146=count ×
/// (6008=conId, 6064=qty, 6101=avgCost).
/// The wire only carries conId/qty/avgCost — no symbol/secType. For any held conId not
/// yet in the
/// reference cache, an internal secdef request goes out so the wrapper-facing Contract
/// is
/// populated by the time `req_positions` is called.
impl CcpState {

    /// Issue an internal secdef request for `con_id` where the reference cache is cold
    /// and
    /// none has been auto-fetched this session. The reply path populates the cache
    /// through
    /// the existing 35=d handler; the response is not tracked.
    fn auto_fetch_secdef_if_cold(
        &mut self,
        con_id: i64,
        ccp_conn: &mut Option<Connection>,
        shared: &SharedState,
        hb: &mut HeartbeatState,
    ) {
        if con_id == 0 { return; }
        if self.auto_fetched_conids.contains_key(&con_id) { return; }
        if shared.reference.get_contract(con_id).is_some() { return; }
        let req_id = self.next_internal_secdef_id;
        self.next_internal_secdef_id = self.next_internal_secdef_id.wrapping_add(1);
        self.auto_fetched_conids.insert(con_id, req_id);
        self.send_secdef_request(req_id, con_id, ccp_conn, hb);
    }

    /// Park a scanner result and dispatch concurrent secdef requests for every cache-
    /// miss
    /// con_id. Once all replies arrive (via `try_release_scanner_enrichments`) the
    /// result
    /// is pushed to the dispatch queue with the now-warm cache. Mirrors what the
    /// gateway
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
            if !self.auto_fetched_conids.contains_key(&con_id) {
                let req_id = self.next_internal_secdef_id;
                self.next_internal_secdef_id = self.next_internal_secdef_id.wrapping_add(1);
                self.auto_fetched_conids.insert(con_id, req_id);
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

pub(crate) mod executions;
pub(crate) mod positions;

#[cfg(test)]
mod tests;
