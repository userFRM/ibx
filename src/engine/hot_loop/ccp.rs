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
/// forever (ibx#227). A gateway rejection arrives in well under a second,
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
/// still reactivate (ibx#250).
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
                perm_id: cached.as_ref().map(|c| c.order.perm_id).unwrap_or(0),
                parent_id: cached.as_ref().map(|c| c.order.parent_id).unwrap_or(0),
                timestamp_ns: 0,
    }
}

/// How long a reconnect waits for the recovery push before judging the orders
/// it did not mention. Generous, because a push that says nothing at all is
/// indistinguishable from one that has not started (ibx#251).
const RECOVERY_PUSH_GRACE: Duration = Duration::from_secs(30);

/// The same wait once the push has sent its own terminator. What is coming has
/// come; this only covers a fill report arriving just behind it.
const RECOVERY_TERMINATOR_GRACE: Duration = Duration::from_secs(2);

pub(crate) struct CcpState {
    pub(crate) seen_exec_ids: HashSet<String>,
    /// Insertion order for `seen_exec_ids`, oldest at the front. Used to evict
    /// one entry at a time once the dedup window is full, instead of clearing
    /// the whole set — a wholesale clear would let a post-reconnect server
    /// replay of a recently-seen ExecID double-count a fill (ibx#198).
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
    /// arrives (ibx#251). Cleared on a disconnect so a second drop before the
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
    /// forever (ibx#227).
    pub(crate) pending_secdef: Vec<(u32, bool, Instant)>,
    /// Requests awaiting a matching-symbols reply, with the deadline after
    /// which one is given up on. Recorded only for a request that actually went
    /// out, and expired so a stale head cannot absorb a later reply (ibx#369).
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
    /// per-exchange `35=c` requests we issued in response to the master
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
    /// conIds we've already auto-fetched secdef for, keyed by con_id (dedup).
    pub(crate) auto_fetched_conids: HashSet<i64>,
    /// Scanner results awaiting per-conId contract-detail enrichment.
    /// Each entry parks a parsed `<ScanResponse>` until every cache-miss
    /// con_id has been resolved via the same 35=d path that user-initiated
    /// `reqContractDetails` uses. See ibx#156, ib-agent#142.
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

/// In-flight by-symbol fan-out: per-exchange `35=c` requests we sent after
/// the master `35=d` reply. Each per-exchange `35=d` reply (matched by tag
/// 320 string) is forwarded to `api_req_id` as one `contract_details`.
pub(crate) struct PendingFanout {
    pub api_req_id: u32,
    pub fanout_req_ids: Vec<String>,
    pub received: usize,
    /// Idle deadline, refreshed on every per-exchange reply. One lost or
    /// unparseable fan-out reply out of ~27 previously left the counter
    /// short forever and contract_details_end never fired (ibx#227).
    pub deadline: Instant,
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
    /// fill and corrupt the position (ibx#198).
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
                        // RTT sample (ibx#158): interval from the test request
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
                            // 8=1 / 8=X control state — not consumed on the order path (ibx#185).
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
        let msg_type = match parsed.get(&fix::TAG_MSG_TYPE) {
            Some(t) => t.as_str(),
            None => return,
        };
        match msg_type {
            fix::MSG_EXEC_REPORT => self.handle_exec_report(&parsed, context, shared, event_tx, account_id),
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
                // ibx#229: a rejection of an in-flight contract-details
                // request was warn-only — the caller saw neither error()
                // nor end (a hang until the ibx#227 sweep, and before that
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
                                // path). See ibx#228.
                                if extract_tag_value(msg, b"146=").is_none() {
                                    log::debug!("matching-symbols ack frame (no tag 146) — awaiting data frame");
                                } else {
                                // Match the reply to its request by the req_id
                                // the server echoes in tag 320, NOT by queue
                                // order: FIFO cross-attributes out-of-order
                                // replies (ibx#228, same fix as pending_secdef).
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
                                    // every later reply (ibx#228).
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
                        "139" => self.handle_option_chain(msg, shared),
                        "102" => self.handle_exchange_list(msg, shared),
                        "107" => self.handle_schedule_reply(msg, shared, event_tx),
                        _ => {}
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
                        let min_tick = crate::control::historical::extract_xml_tag(xml_tag, "minTick")
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(0.01);
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
                            let min_tick = self.kut_min_tick.get(&ticker_id).copied().unwrap_or(0.01);
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
            "UT" | "UM" | "RL" => handle_account_update(msg, context, shared),
            "UP" => handle_position_update(&parsed, context, shared, event_tx),
            "d" => {
                let response_req_id = crate::control::contracts::secdef_response_req_id(msg);
                let fanout_idx = response_req_id.as_ref().and_then(|rid| {
                    self.pending_fanout.iter().position(|p| {
                        p.fanout_req_ids.iter().any(|id| id == rid)
                    })
                });
                if let Some(idx) = fanout_idx {
                    if let Some(def) = crate::control::contracts::parse_secdef_response(msg) {
                        let api_req_id = self.pending_fanout[idx].api_req_id;
                        // ibx#229: no con_id is "no definition for this
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
                        // not a listing (ibx#400).
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

                if let Some(def) = crate::control::contracts::parse_secdef_response(msg) {
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
                            // ibx#229: con_id=0 is the gateway saying "no
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
                            // waiting for a definition that will not come (ibx#400).
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
                                // end BEFORE the row (ibx#227). Defer it to
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
            other => {
                log::debug!("CCP unhandled 35={}: {} bytes", other, msg.len());
            }
        }
    }

    fn handle_exec_report(
        &mut self,
        parsed: &std::collections::HashMap<u32, String>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        account_id: &str,
    ) {
        // CCP recovery push format A (ib-agent#155, captured against live):
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
                let stripped = s.strip_prefix('C').unwrap_or(s);
                // Strip versioned suffix (.0, .1, .2) from modify-chained ClOrdIDs
                let base = stripped.split('.').next().unwrap_or(stripped);
                base.parse::<u64>().ok()
            }).unwrap_or(0)
        });

        // Recovery insert: a 35=8 with status New/New (150=0/39=0) for an order
        // that is NOT in this session's context is a cross-session recovery entry
        // pushed by CCP on session establishment. Insert into context.open_orders
        // so subsequent cancel/modify ACKs at ~line 668 can match via
        // context.order(clord_id) and emit OrderUpdate events to the user. ibx#191.
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
                // engine down (ibx#257). The reconnect burst replays every
                // resting order, which is exactly when the table fills.
                // Skipping only the insert keeps the order in last_clord and
                // the rich-order cache, so req_open_orders still shows it —
                // but it is NOT in the engine book, so a later fill or
                // terminal status for it is dropped and no OrderUpdate
                // reaches the caller (ibx#297). A missing order beats taking
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
                    // like new quantity (ibx#320).
                    filled: parsed.get(&14)
                        .and_then(|s| s.parse::<f64>().ok())
                        .map(|c| c as u32)
                        .unwrap_or_else(|| prior.map_or(0, |o| o.filled)),
                    // An order this session never saw is working by the fact of
                    // being in the push. One whose state was not known stays
                    // not known here, so the status this very message carries
                    // moves it — and the caller who was told it was unknown is
                    // told what it turned out to be.
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
        // own id, not the original order's. See ibx#179.
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
        // and left the caller's pending what-if to time out (ibx#205).
        // The not-ready ack is not always emitted — close/reject previews send a
        // single data frame — so accept the first data frame with no assumption
        // that an ack precedes it. A frame is the real preview when ANY of the
        // six margin fields (6826/6827/6828 before, 6092/6093/6094 after)
        // parses as a finite number, mirroring the gateway's own real-frame
        // test: each field is "set" when it parses, unset on nan/unparseable,
        // and the frame is real when any field is set (ibx#213, ibx#214). The
        // ack carries "n/a" in all six, so it never matches. Captured
        // byte-level in ib-agent#160.
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
                    };
                    log::info!("WhatIf response: clord={} initMargin={:.2}->{:.2} commission={:.2}",
                        clord_id,
                        response.init_margin_before as f64 / PRICE_SCALE as f64,
                        response.init_margin_after as f64 / PRICE_SCALE as f64,
                        response.commission as f64 / PRICE_SCALE as f64);
                    context.retire_order(clord_id);
                    shared.orders.push_what_if(response);
                    emit(event_tx, Event::WhatIf(response));
                }
            return;
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
                // routing both are absent/"NONE". Captured in ib-agent#162 (ibx#210).
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
            log::warn!(
                "Order {clord_id}: the gateway refused the request (378={restatement_reason}) — \
                 the order stands as it was",
            );
        }
        if is_replace_ack {
            context.set_order_status_forced(clord_id, status);
        }

        // The guard's verdict doubles as the change flag (ibx#212): a stale
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
        // has never seen the ID (ibx#320). At session start the gateway replays
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
        // (ibx#260). Falling back to the fields that identify an execution
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
                // cancel/modify reject already uses instead (ibx#250).
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
            let exchange = parsed.get(&207).cloned().unwrap_or_default();
            let sec_type = parsed.get(&167).cloned().unwrap_or_default();
            let currency = parsed.get(&15).cloned().unwrap_or_default();
            let con_id: i64 = parsed.get(&6008).and_then(|s| s.parse().ok()).unwrap_or(0);
            let local_symbol = parsed.get(&6035).cloned().unwrap_or_default();
            let _routing_exchange = parsed.get(&6004).cloned().unwrap_or_default();
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
            // and nothing said so (ibx#307).
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
                // makes a completed order read as entirely unfilled (ibx#309).
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
                ..Default::default()
            };

            if con_id != 0 {
                shared.reference.cache_contract(con_id, contract.clone());
            }

            // A trade cancel (150=H) or trade correction (150=G) restates an
            // execution the gateway has already reported, so it may legitimately
            // return a completed order to a working quantity. Every other report
            // that would do that is a replay (ibx#262).
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
        // surfaced it was removed (ibx#252).
        //
        // Read as a positive statement, not as an absence: a missing or
        // unparseable tag 102 is synthesized as -1 here and says nothing, so it
        // takes the same path as the reasons that do mean the order is working.
        let unknown_order = reason_code == 1;

        // Update local context only if we tracked the order in this session.
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
                // (ibx#314).
                context.set_order_status_forced(oid, crate::types::OrderStatus::Cancelled);
                context.retire_order(oid);
            } else {
                let restore_status = if order.filled > 0 {
                    crate::types::OrderStatus::PartiallyFilled
                } else {
                    crate::types::OrderStatus::Submitted
                };
                // Deliberate regression (PendingCancel back to working) — the
                // ibx#212 guard would rightly block it on the ordinary path.
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

    fn handle_account_summary(&mut self, parsed: &std::collections::HashMap<u32, String>, context: &mut Context, shared: &SharedState) {
        if let Some(val) = parsed.get(&9806).and_then(|s| s.parse::<f64>().ok()) {
            context.account.net_liquidation = (val * PRICE_SCALE as f64) as Price;
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
    /// Fail contract-details requests whose deadline has passed (ibx#227):
    /// both plain/by-symbol secdef lookups the gateway never answered and
    /// by-symbol fan-outs missing one or more per-exchange replies. On
    /// expiry the caller gets error 200 plus contract_details_end, so a
    /// blocked wait unblocks with no API change. Internal sentinel req_ids
    /// (>= 0xF000_0000: cache auto-fetch, scanner enrichment) are dropped
    /// silently — no user is waiting on them.
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
        let mut expired: Vec<u32> = Vec::new();
        self.pending_secdef.retain(|(req_id, _, deadline)| {
            if now >= *deadline {
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
            if now >= p.deadline {
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
            log::warn!("Contract-details timeout: req_id={req_id} — no gateway reply within {SECDEF_TIMEOUT:?}");
            shared.reference.push_historical_error(
                req_id, 200,
                "contract details request timed out — no reply from the gateway".to_string(),
            );
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
            // receives error 200 + end via the sweep instead of silence (ibx#227).
            log::warn!("secdef request req_id={req_id} queued with no CCP socket");
        }
        // Known-conId lookup: single record, no paginated terminator.
        self.pending_secdef.push((req_id, true, Instant::now() + SECDEF_TIMEOUT));
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
            // code, 1 = CUSIP, 4 = ISIN (ib-agent#174). When a known one is set the
            // lookup rides the identifier and drops the symbol/secType/filters.
            let sec_id_source = match filters.sec_id_type.to_uppercase().as_str() {
                "ISIN" => "4",
                "CUSIP" => "1",
                _ => "",
            };
            let identifier_lookup = !filters.sec_id.is_empty() && !sec_id_source.is_empty();

            let strike_str = if filters.strike > 0.0 { format!("{}", filters.strike) } else { String::new() };
            // PutOrCall: Call = 1, Put = 0 (ib-agent#171).
            let right_code = match filters.right.to_uppercase().as_str() {
                "C" | "CALL" => "1",
                "P" | "PUT" => "0",
                _ => "",
            };
            // Exchange rides tag 100; primaryExchange (when set) rides tag 207 —
            // the two were previously conflated onto 207. localSymbol replaces the
            // plain symbol; the derivative/disambiguation filters are added only
            // when set. Captured in ib-agent#171 (ibx#229).
            let mut fields: Vec<(u32, &str)> = vec![
                (fix::TAG_MSG_TYPE, "c"),
                (fix::TAG_SENDING_TIME, &ts),
                (320, &req_id_str),
                (321, "2"),
            ];
            if identifier_lookup {
                // Identifier lookup: the identifier and its source replace the
                // symbol/secType/filters; exchange and currency still ride
                // (ib-agent#174).
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
        // Recorded only where the request went out. It used to be recorded
        // whether or not it was sent — the send error was discarded, and the
        // push sat outside the block that needs a connection at all — so a
        // request issued while the transport was down was queued as pending
        // with nothing on the wire to answer it (ibx#369).
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
    /// could absorb a later request's answer (ibx#369).
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
        // Which derivative to enumerate follows from the underlying: an option
        // on a future is a future option, and a caller who names a futures
        // exchange is asking for those too. An underlying whose type the caller
        // left blank says nothing about the derivative, so nothing is claimed.
        let derivative = match (underlying_sec_type, fut_fop_exchange.is_empty()) {
            ("", true) => "",
            ("FUT", _) | (_, false) => "FOP",
            _ => "OPT",
        };
        // A future option whose underlying is not itself a future names that
        // underlying on a tag of its own.
        let con_id_tag = if derivative == "FOP" && underlying_sec_type != "FUT" { 6457 } else { 6346 };
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
        if !derivative.is_empty() {
            fields.push((310, derivative));
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
        // An underlying the venue lists nothing for still answers the request,
        // and then the symbol tag is all there is to attribute it by.
        let symbol = scopes.first().map(|s| s.symbol.clone())
            .or_else(|| extract_tag_value(msg, b"55="))
            .unwrap_or_default();
        let Some(pos) = self.pending_option_params.iter()
            .position(|(_, pending, _, _)| pending.eq_ignore_ascii_case(&symbol))
        else {
            log::warn!("Option chain reply for '{symbol}' matches no request");
            return;
        };
        let (req_id, _, con_id, _) = self.pending_option_params.remove(pos);
        log::info!("Option chain reply: req_id={req_id} symbol={symbol} scopes={}", scopes.len());
        shared.reference.push_option_params(req_id, con_id, scopes);
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

    /// Parse 6040=102 exchange directory from CCP init into DepthMktDataDescription entries.
    fn handle_exchange_list(&self, msg: &[u8], shared: &SharedState) {
        use crate::types::DepthMktDataDescription;
        let raw = String::from_utf8_lossy(msg);
        let fields: Vec<&str> = raw.split('\x01').collect();

        // The message has repeating 100=EXCHANGE|6813=NAME pairs grouped by sections.
        // Sections: 6523=category|6811=category_name for stock categories,
        //           8128=N and 8129=N separate stock/derivative sections.
        // We parse all 100/6813 pairs into DepthMktDataDescription entries.
        let mut descs: Vec<DepthMktDataDescription> = Vec::new();
        let mut current_sec_type = "STK".to_string();
        let mut current_agg_group: i32 = 0;

        let mut i = 0;
        while i < fields.len() {
            let f = fields[i];
            if let Some(val) = f.strip_prefix("8128=") {
                // Section separator — exchanges above are stocks, below are derivatives
                current_sec_type = "STK".to_string();
                current_agg_group = val.parse().unwrap_or(0);
            } else if let Some(val) = f.strip_prefix("8129=") {
                current_sec_type = "FUT".to_string();
                current_agg_group = val.parse().unwrap_or(0);
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
                    service_data_type: "L1".to_string(),
                    agg_group: current_agg_group,
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
        // status and `req_open_orders` kept asserting it (ibx#251).
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
            // terminated by 11='*' sentinel. See ib-agent#155, ibx#191.
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
pub(crate) fn handle_account_update(msg: &[u8], context: &mut Context, shared: &SharedState) {
    let text = match std::str::from_utf8(msg) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut key: Option<&str> = None;
    for part in text.split('\x01') {
        if let Some(val) = part.strip_prefix("8001=") {
            key = Some(val);
        } else if let Some(val) = part.strip_prefix("8004=")
            && let Some(k) = key {
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
                seed.qty_midnight = v.parse::<f64>().ok().filter(|q| q.is_finite()).map(|q| q as i64);
            } else if let Some(v) = part.strip_prefix("8223=") {
                seed.qty_traded = v.parse::<f64>().ok().filter(|q| q.is_finite());
            } else if let Some(v) = part.strip_prefix("8233=") {
                // What the venue says the position was worth at midnight, which
                // is the figure the day's change is measured from.
                seed.cost_midnight = v.parse::<f64>().ok().filter(|c| c.is_finite());
            } else if let Some(v) = part.strip_prefix("6822=") {
                // moneyTradedSinceMidnight: signed net cash, SELL positive / BUY
                // negative. Stored with the wire sign; poll_pnl adds it (ib-agent#163).
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
/// reference cache, we issue an internal secdef request so the wrapper-facing Contract is populated
/// by the time `req_positions` is called (#154).
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
    // saying flat — the same defect ibx#261 fixed on the account-update path
    // (ibx#296). A genuine flat still arrives as an explicit `6064=0`.
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

    /// Issue an internal secdef request for `con_id` if the reference cache is cold and we
    /// haven't already auto-fetched it this session. The reply path populates the cache via
    /// the existing 35=d handler; we don't track the response.
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
        // requested the same con_id (auto_fetched_conids contains it) we skip
        // the send but still wait — its reply will populate the cache and
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
    /// entries we have (some may still have blank fields if the secdef reply
    /// never arrived). Prevents indefinite hangs on a missing reply.
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
    // P&L paths until the next frame that did carry 6064 (ibx#261).
    let position_raw: Option<f64> = parsed.get(&6064)
        .and_then(|s| s.parse::<f64>().ok())
        // `"NaN".parse()` succeeds and `NaN as i64` is 0, so a non-finite
        // value would flatten a live position by the same route the absent
        // tag did. Route it to no-data instead.
        .filter(|v| v.is_finite());
    let position: Option<f64> = position_raw;
    // Tag map verified against the updatePortfolio callback (ib-agent#172):
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
mod tests {
    use super::*;

    /// `Uncertain` promises the caller a reconciliation when the reconnect
    /// completes. Nothing completed it, so an order the recovery push left out
    /// waited on a message that was never coming.
    #[test]
    fn the_recovery_reports_the_orders_it_did_not_account_for() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let instrument = context.register_instrument(756733);
        context.insert_order(crate::types::Order::new(
            7, instrument, crate::types::Side::Buy, 100,
            150 * crate::types::PRICE_SCALE, b'2', b'0', 0,
        ));
        context.mark_orders_uncertain();

        // Still inside the grace: the push may yet speak for it.
        ccp.recovery_sweep_at = Some(Instant::now() + Duration::from_secs(30));
        ccp.sweep_recovery(&mut context, &shared, &None);
        assert!(shared.orders.drain_order_updates().is_empty(), "nothing is due yet");

        ccp.recovery_sweep_at = Some(Instant::now() - Duration::from_secs(1));
        ccp.sweep_recovery(&mut context, &shared, &None);
        let updates = shared.orders.drain_order_updates();
        assert_eq!(updates.len(), 1, "the stranded order is reported");
        assert_eq!(updates[0].order_id, 7);
        assert_eq!(
            updates[0].status, crate::types::OrderStatus::Uncertain,
            "and reported as what it is — unknown, not a fate the engine invented",
        );

        ccp.sweep_recovery(&mut context, &shared, &None);
        assert!(shared.orders.drain_order_updates().is_empty(), "one report per recovery");
    }

    fn position_frame(pairs: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
        let mut m = std::collections::HashMap::new();
        m.insert(6008u32, "265598".to_string());
        for (t, v) in pairs { m.insert(*t, v.to_string()); }
        m
    }

    /// ibx#261: a frame carrying marks but no quantity must leave the position
    /// alone. Reading absent as zero reconciled a live position to flat and
    /// published it to reqPositions and both P&L paths.
    /// The average cost is written into a row that persists, so a frame that
    /// omits the tag must not replace a real one with zero — the same rule the
    /// quantity follows, on the price side.
    #[test]
    fn a_frame_without_an_average_cost_keeps_the_stored_one() {
        let mut context = Context::new();
        let shared = SharedState::new();
        let frame = |pairs: &[(u32, &str)]| {
            let mut m = std::collections::HashMap::new();
            for (t, v) in pairs { m.insert(*t, v.to_string()); }
            m
        };

        // A frame stating both.
        handle_position_update(
            &frame(&[(6008, "756733"), (6064, "100"), (6101, "150.00"), (6068, "SPY")]),
            &mut context, &shared, &None,
        );
        let stored = shared.portfolio.position_info(756733).expect("row").avg_cost;
        assert_eq!(stored, 150 * PRICE_SCALE);

        // A later frame stating the quantity but not the cost.
        handle_position_update(
            &frame(&[(6008, "756733"), (6064, "120"), (6068, "SPY")]),
            &mut context, &shared, &None,
        );
        let after = shared.portfolio.position_info(756733).expect("row");
        assert_eq!(after.position, 120.0, "the quantity it did state is applied");
        assert_eq!(
            after.avg_cost, 150 * PRICE_SCALE,
            "and the cost it did not state is kept, not zeroed",
        );
    }

    #[test]
    fn marks_only_frame_does_not_flatten_a_live_position() {
        let mut context = Context::new();
        let instrument = context.register_instrument(265598);
        let shared = SharedState::new();

        handle_position_update(&position_frame(&[(6064, "100"), (6101, "150.0")]),
            &mut context, &shared, &None);
        assert_eq!(context.position(instrument), 100.0);

        // Marks move, no 6064 on the frame.
        handle_position_update(&position_frame(&[(6065, "151.0"), (6100, "100.0")]),
            &mut context, &shared, &None);
        assert_eq!(context.position(instrument), 100.0,
            "a marks-only frame must not flatten the position");
        assert_eq!(shared.portfolio.position_infos().iter()
            .find(|p| p.con_id == 265598).map(|p| p.position), Some(100.0),
            "reqPositions must still report the held quantity");

        // The marks from that frame did land on the existing row.
        let row = shared.portfolio.position_infos().into_iter()
            .find(|p| p.con_id == 265598).expect("row still present");
        assert_eq!(row.market_price, (151.0 * PRICE_SCALE as f64) as Price,
            "a marks-only frame must still update the marks");

        // A frame that really does carry a flat quantity still flattens it.
        handle_position_update(&position_frame(&[(6064, "0")]), &mut context, &shared, &None);
        assert_eq!(context.position(instrument), 0.0);
    }

    /// A marks-only frame for a contract never seen before must not conjure a
    /// row: set_position_marks inserts a default PositionInfo, and that row
    /// would report position 0 to reqPositions and both P&L paths.
    /// Same class as the absent tag: `"NaN".parse::<f64>()` succeeds and
    /// `NaN as i64` is 0, so a non-finite value reached the flatten path by
    /// exactly the route ibx#261 closed.
    #[test]
    fn a_non_finite_quantity_is_treated_as_no_quantity() {
        for bad in ["NaN", "inf", "-inf"] {
            let mut context = Context::new();
            let shared = SharedState::new();
            handle_position_update(
                &position_frame(&[(6064, "100"), (6101, "150.0")]), &mut context, &shared, &None);
            assert_eq!(
                shared.portfolio.position_info(265598).map(|p| p.position), Some(100.0),
                "seed must establish a live position");

            handle_position_update(
                &position_frame(&[(6064, bad), (6101, "151.0")]), &mut context, &shared, &None);
            assert_eq!(
                shared.portfolio.position_info(265598).map(|p| p.position), Some(100.0),
                "{bad} must not flatten a live position");
        }
    }

    #[test]
    fn marks_only_frame_for_an_unknown_contract_creates_no_row() {
        let mut context = Context::new();
        let shared = SharedState::new();
        handle_position_update(&position_frame(&[(6065, "151.0"), (6100, "100.0")]),
            &mut context, &shared, &None);
        assert!(shared.portfolio.position_infos().iter().all(|p| p.con_id != 265598),
            "no position row may be fabricated from a marks-only frame");
    }

    // Regression for ibx#198: the fill-dedup set must NOT be wiped wholesale
    // when it reaches its cap. A recently-seen ExecID has to stay deduplicated
    // so a post-reconnect server replay can't double-count the fill.
    #[test]
    fn record_exec_id_dedupes_within_window() {
        let mut ccp = CcpState::new();
        assert!(ccp.record_exec_id("exec-A"), "first sighting is new");
        assert!(!ccp.record_exec_id("exec-A"), "immediate replay is a duplicate");
    }

    #[test]
    fn record_exec_id_evicts_oldest_not_whole_set() {
        let mut ccp = CcpState::new();
        // The very first ExecID — the one a reconnect is most likely to replay.
        assert!(ccp.record_exec_id("exec-first"));
        // Push the window exactly to its cap. Together with "exec-first" this is
        // EXEC_ID_WINDOW + 1 inserts, which evicts exactly one entry: the oldest
        // ("exec-first"). Every other recent ID must remain deduplicated.
        for i in 0..EXEC_ID_WINDOW {
            assert!(ccp.record_exec_id(&format!("exec-{i}")));
        }
        assert_eq!(ccp.seen_exec_ids.len(), EXEC_ID_WINDOW);
        // Oldest was evicted, so a replay now reads as new (unavoidable past the
        // window) — but the most recent IDs are still caught as duplicates.
        assert!(!ccp.record_exec_id("exec-0"), "recent ID still deduped");
        assert!(!ccp.record_exec_id(&format!("exec-{}", EXEC_ID_WINDOW - 1)),
            "newest ID still deduped");
    }

    // A wholesale clear() would have made "exec-first" re-insertable as new
    // after just one extra fill past the cap; assert the rolling window keeps
    // the bound without that cliff.
    #[test]
    fn record_exec_id_window_is_bounded() {
        let mut ccp = CcpState::new();
        for i in 0..(EXEC_ID_WINDOW * 3) {
            ccp.record_exec_id(&format!("exec-{i}"));
        }
        assert_eq!(ccp.seen_exec_ids.len(), EXEC_ID_WINDOW);
        assert_eq!(ccp.exec_id_order.len(), EXEC_ID_WINDOW);
    }

    // Build a what-if (6091=1) ExecReport map for order 42. `margin_fields`
    // holds (tag, literal wire value) pairs exactly as the gateway puts them
    // on the wire (ib-agent#160).
    fn what_if_frame(margin_fields: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
        let mut m = std::collections::HashMap::new();
        m.insert(11u32, "42".to_string()); // ClOrdID
        m.insert(6091u32, "1".to_string()); // what-if marker
        for (tag, val) in margin_fields {
            m.insert(*tag, val.to_string());
        }
        m
    }

    // The full six margin fields of the captured true-zero close preview
    // (ib-agent#160 scenario 2b).
    const ZERO_CLOSE_FIELDS: [(u32, &str); 6] = [
        (6826, "976.07"), (6827, "887.34"), (6828, "945924.53"),
        (6092, "0"), (6093, "0"), (6094, "945923.47"),
    ];

    fn what_if_test_state() -> (CcpState, Context, SharedState) {
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.insert_order(crate::types::Order {
            order_id: 42,
            instrument,
            side: Side::Buy,
            price: 0,
            qty: 100,
            filled: 0,
            status: crate::types::OrderStatus::Submitted,
            ord_type: b'2',
            tif: b'0',
            stop_price: 0,
        });
        (CcpState::new(), context, SharedState::new())
    }

    /// A replace is acknowledged as 39=5, and the gateway sends 39=6 first.
    /// Captured live against a paper account, a modify runs PendingCancel then
    /// Replaced. The monotonic guard ranks PendingCancel above the working
    /// states, so the acknowledgement looked like a stale frame: the caller was
    /// told the order was cancelling and never told the replacement was live.
    #[test]
    fn a_replace_acknowledgement_is_not_dropped_as_a_stale_frame() {
        let (mut ccp, mut context, shared) = ord_status_test_state();

        // The order is working, then the replace puts a cancel in flight.
        ccp.handle_exec_report(&exec_report_frame(&[(150, "0"), (39, "0")]),
            &mut context, &shared, &None, "");
        ccp.handle_exec_report(&exec_report_frame(&[(150, "6"), (39, "6")]),
            &mut context, &shared, &None, "");
        assert_eq!(context.order(42).map(|o| o.status),
            Some(crate::types::OrderStatus::PendingCancel), "the cancel is in flight");
        let _ = shared.orders.drain_open_orders();

        ccp.handle_exec_report(&exec_report_frame(&[(150, "5"), (39, "5")]),
            &mut context, &shared, &None, "");

        assert_eq!(
            context.order(42).map(|o| o.status),
            Some(crate::types::OrderStatus::Submitted),
            "the replacement is working, not still cancelling",
        );
        assert!(
            shared.orders.drain_open_orders().iter().any(|(id, _)| *id == 42),
            "and the caller is told, rather than the frame being dropped",
        );
    }

    /// The recovery-push terminator carries `11='*'`, which parses to the
    /// reserved order id 0. It is dropped further down the handler, but the
    /// recovery insert runs first — so without a guard there it registers the
    /// frame's conId and inserts order 0 before the "discard".
    #[test]
    fn the_recovery_terminator_mutates_no_state_before_it_is_dropped() {
        // A clean context: the shared fixture pre-registers its conId, which
        // would mask exactly what this test is looking for.
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let frame: std::collections::HashMap<u32, String> = [
            (11u32, "*"), (150, "0"), (39, "0"), (6008, "265598"), (38, "1"), (54, "1"),
        ].iter().map(|(k, v)| (*k, v.to_string())).collect();

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        assert!(context.order(0).is_none(), "the reserved order id must not be inserted");
        assert!(
            context.market.instrument_by_con_id(265598).is_none(),
            "the terminator must not register an instrument",
        );
    }
    /// LeavesQty is still the remainder everywhere it was already right. The
    /// two are complements, so a change that confuses them shows up here as
    /// well as on the filled side.
    #[test]
    fn leaves_qty_is_still_reported_as_the_remainder() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (150, "2"), (39, "1"), (32, "30"), (31, "150.00"),
            (14, "30"), (151, "70"), (6, "150.00"), (17, "E1"),
        ]);

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let fills = shared.orders.drain_fills();
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].remaining, 70, "the fill reports what is still working");
    }

    /// `filled_quantity` was taken from tag 151 (LeavesQty), the *unfilled*
    /// remainder, rather than tag 14 (CumQty). The two are complements, so a
    /// partially filled order reported the wrong number and a completed one —
    /// LeavesQty zero — reported as entirely unfilled (ibx#309).
    #[test]
    fn filled_quantity_is_the_filled_amount_not_the_remainder() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let instrument = context.market.register(265598);
        context.insert_order(crate::types::Order {
            order_id: 77, instrument, side: Side::Buy, price: 0, qty: 100,
            filled: 0, status: crate::types::OrderStatus::Submitted,
            ord_type: b'2', tif: b'0', stop_price: 0,
        });

        // 100 ordered, 30 filled, 70 still working.
        let frame: std::collections::HashMap<u32, String> = [
            (11u32, "77"), (150, "1"), (39, "1"), (6008, "265598"),
            (38, "100"), (14, "30"), (151, "70"), (54, "1"), (6, "150.0"),
        ].iter().map(|(k, v)| (*k, v.to_string())).collect();
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let orders = shared.orders.drain_open_orders();
        let (_, info) = orders.iter().find(|(id, _)| *id == 77)
            .expect("the order must be reported");
        assert_eq!(info.order.filled_quantity, 30.0,
            "filled must be CumQty (30), not LeavesQty (70)");

        // On a consistent frame the complement `total - leaves` gives the same
        // number, so it has to be told apart on a frame without tag 151 —
        // where the complement would report the whole order as filled.
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let instrument = context.market.register(265598);
        context.insert_order(crate::types::Order {
            order_id: 78, instrument, side: Side::Buy, price: 0, qty: 100,
            filled: 0, status: crate::types::OrderStatus::Submitted,
            ord_type: b'2', tif: b'0', stop_price: 0,
        });
        let frame: std::collections::HashMap<u32, String> = [
            (11u32, "78"), (150, "1"), (39, "1"), (6008, "265598"),
            (38, "100"), (14, "30"), (54, "1"), (6, "150.0"),
        ].iter().map(|(k, v)| (*k, v.to_string())).collect();
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let orders = shared.orders.drain_open_orders();
        let (_, info) = orders.iter().find(|(id, _)| *id == 78)
            .expect("the order must be reported");
        assert_eq!(
            info.order.filled_quantity, 30.0,
            "still CumQty with no LeavesQty on the frame, not the complement (100)",
        );

        // A later report that omits tag 14 must not wipe what was established.
        // A pending cancel is exactly that shape, and zeroing there would put
        // back the symptom this corrects.
        let later: std::collections::HashMap<u32, String> = [
            (11u32, "78"), (150, "6"), (39, "6"), (6008, "265598"),
            (38, "100"), (151, "70"), (54, "1"),
        ].iter().map(|(k, v)| (*k, v.to_string())).collect();
        ccp.handle_exec_report(&later, &mut context, &shared, &None, "");

        let orders = shared.orders.drain_open_orders();
        let (_, info) = orders.iter().find(|(id, _)| *id == 78)
            .expect("the order must still be reported");
        assert_eq!(
            info.order.filled_quantity, 30.0,
            "a report without tag 14 keeps the filled quantity, it does not zero it",
        );

        // And the remainder is still the remainder, on the same reports.
        assert_eq!(info.order.total_quantity, 100.0);
    }
    /// The midnight seed carries the same quantity tag and had the same
    /// defect: reading an absent one as zero makes the day's P&L look as
    /// though the position were opened intraday, when it was held overnight.
    ///
    /// The row is kept with an unknown quantity rather than dropped, because
    /// dropping it says the same wrong thing — a position with no seed row *is*
    /// the intraday case — and would discard the cash and realized figures the
    /// row does state.
    #[test]
    fn a_midnight_seed_without_a_quantity_is_not_seeded_flat() {
        let shared = SharedState::new();
        // Two entries: one stating its quantity, one omitting it.
        let body = [
            "6008=756733", "6064=100", "6822=-50.0", "6099=7.5",
            "6008=265598", "6822=-10.0", "6099=2.5",
        ].join("\x01");
        handle_pnl_response(body.as_bytes(), &shared);

        let mut seeds = shared.portfolio.midnight_seeds();
        seeds.sort_by_key(|s| s.con_id);
        assert_eq!(seeds.len(), 2, "both entries are seeded");

        let stated = seeds.iter().find(|s| s.con_id == 756733).expect("stated entry");
        assert_eq!(stated.qty_midnight, Some(100));

        let silent = seeds.iter().find(|s| s.con_id == 265598).expect("silent entry");
        assert_eq!(silent.qty_midnight, None, "absent is unknown, not flat");
        assert_eq!(silent.money_traded, -10.0, "the figures it did state survive");
        assert_eq!(silent.realized_pnl, 2.5);
    }

    /// The venue states what each position was worth at midnight and what has
    /// been traded against it since. Those are the figures the day's change is
    /// measured from, so they have to arrive intact rather than be recomputed.
    ///
    /// A combo bucket restates the same five fields against a label. Nothing
    /// here is keyed by a label, so a bucket's figures must land nowhere at
    /// all instead of on whichever contract happened to come before it.
    #[test]
    fn the_venue_states_what_a_position_was_worth_at_midnight() {
        let shared = SharedState::new();
        let body = [
            "146=2",
            "6008=756733", "6064=100", "8223=25", "8233=44000.5", "6822=-1250.0", "6099=7.5",
            "6008=265598", "6064=-3", "8223=0", "8233=-1200.0", "6822=0", "6099=0",
            "8058=1",
            "8020=SPY 26JUN CALENDAR", "6064=9", "8233=999999.0", "6822=888888.0", "6099=777777.0",
        ].join("\x01");
        handle_pnl_response(body.as_bytes(), &shared);

        let seeds = shared.portfolio.midnight_seeds();
        assert_eq!(seeds.len(), 2, "the combo bucket is not a contract");

        let long = seeds.iter().find(|s| s.con_id == 756733).expect("first contract");
        assert_eq!(long.qty_midnight, Some(100));
        assert_eq!(long.qty_traded, Some(25.0));
        assert_eq!(long.cost_midnight, Some(44000.5), "taken as sent, unscaled");
        assert_eq!(long.money_traded, -1250.0);
        assert_eq!(long.realized_pnl, 7.5);

        let short = seeds.iter().find(|s| s.con_id == 265598).expect("second contract");
        assert_eq!(short.qty_midnight, Some(-3));
        assert_eq!(short.cost_midnight, Some(-1200.0), "a short is worth a negative amount");
        assert_eq!(
            short.realized_pnl, 0.0,
            "the combo bucket's figures did not fall through onto the last contract",
        );
    }

    /// The body says whether it is an answer. One that reports a problem is
    /// reporting that instead of stating figures, so nothing in it is read.
    #[test]
    fn a_pnl_body_that_reports_a_problem_states_no_seeds() {
        let shared = SharedState::new();
        let body = [
            "58=No security definition has been found",
            "6008=756733", "6064=100", "8233=44000.5", "6099=7.5",
        ].join("\x01");
        handle_pnl_response(body.as_bytes(), &shared);
        assert!(shared.portfolio.midnight_seeds().is_empty(), "a problem is not a figure");
    }

    /// The venue answers against the reference it was handed and falls back to
    /// its own request id only when it has none.
    #[test]
    fn the_reference_id_names_the_request_the_seeds_answer() {
        let shared = SharedState::new();
        let both = ["6529=PLR.2", "8292=PLR.1", "6008=756733", "6064=1"].join("\x01");
        handle_pnl_response(both.as_bytes(), &shared);
        assert_eq!(shared.portfolio.pnl_request_key(), "PLR.1");

        let neither = ["6529=PLR.2", "8292=", "6008=756733", "6064=1"].join("\x01");
        handle_pnl_response(neither.as_bytes(), &shared);
        assert_eq!(shared.portfolio.pnl_request_key(), "PLR.2");
    }

    /// The price table is two lists paired by position. An unreadable contract
    /// id has to hold its place, because dropping it slides every price after
    /// it onto the wrong contract.
    #[test]
    fn the_price_table_pairs_each_contract_with_its_own_price() {
        let shared = SharedState::new();
        let body = [
            "146=3",
            "6008=756733", "6008=not-a-contract", "6008=265598",
            "8057=612.34", "8057=9.99", "8057=1.005",
        ].join("\x01");
        handle_pnl_prices(body.as_bytes(), &shared);

        assert_eq!(shared.portfolio.venue_price(756733).as_deref(), Some("612.34"));
        assert_eq!(
            shared.portfolio.venue_price(265598).as_deref(), Some("1.005"),
            "the third price belongs to the third contract",
        );

        // A later table restates what it names and leaves the rest standing.
        handle_pnl_prices(["6008=756733", "8057=615.00"].join("\x01").as_bytes(), &shared);
        assert_eq!(shared.portfolio.venue_price(756733).as_deref(), Some("615.00"));
        assert_eq!(shared.portfolio.venue_price(265598).as_deref(), Some("1.005"));
    }

    /// One lookup describes one contract. The venues answer separately and
    /// each answer is the same contract with a different exchange, so reporting
    /// every one of them returned a single stock as twenty-seven listings.
    #[test]
    fn a_contract_reaches_the_caller_once_per_request() {
        let mut ccp = CcpState::new();
        let mut seen = |req_id: u32, con_id: i64| {
            ccp.details_delivered.entry(req_id).or_default().insert(con_id)
        };
        assert!(seen(9, 756733), "the first answer is the caller's row");
        assert!(!seen(9, 756733), "and every later venue saying the same is not");
        assert!(seen(9, 885901989), "a different contract still comes through");
        assert!(seen(10, 756733), "as does the same one under another request");
    }

    /// An option is asked for by expiry date and a future by contract month.
    /// Both went out on MaturityMonthYear, so the option lookup asked for a
    /// month that does not exist and matched nothing.
    #[test]
    fn a_maturity_rides_the_tag_its_precision_belongs_to() {
        assert_eq!(maturity_tag("202609"), Some(200), "a contract month");
        assert_eq!(maturity_tag("20260918"), Some(541), "a full expiry date");
        assert_eq!(maturity_tag("20260918 14:30:00"), Some(541), "a date with a time on it");
        assert_eq!(maturity_tag(""), None, "nothing to state");
        assert_eq!(maturity_tag("2026"), None, "too short to be either, so it is not guessed");
    }

    /// A holding arrives as a contract id and a quantity. Reported before its
    /// definition lands, it named no instrument at all — a position in a
    /// contract the caller cannot identify.
    #[test]
    fn a_holding_takes_its_contract_from_the_definition_that_follows() {
        use crate::control::contracts::{ContractDefinition, SecurityType};
        let shared = SharedState::new();
        shared.portfolio.set_position_info(PositionInfo {
            con_id: 793356217, position: 1.0, avg_cost: 38270,
            ..Default::default()
        });
        assert_eq!(
            shared.portfolio.position_info(793356217).map(|p| p.symbol.clone()),
            Some(String::new()),
            "the feed states no symbol",
        );

        let def = ContractDefinition {
            con_id: 793356217,
            symbol: "MES".to_string(),
            sec_type: SecurityType::Future,
            currency: "USD".to_string(),
            ..ContractDefinition::default()
        };
        identify_position(&shared, &def);

        let row = shared.portfolio.position_info(793356217).unwrap();
        assert_eq!(row.symbol, "MES", "and the definition names it");
        assert_eq!(row.position, 1.0, "without disturbing the quantity");
        assert_eq!(row.avg_cost, 38270, "or the basis");
    }

    /// The lean feed states a quantity and often no cost. Reading the absence
    /// as a cost of zero erased the basis of a live holding, and the P&L path
    /// reads a zero basis as having acquired it for nothing.
    #[test]
    fn a_row_without_a_cost_keeps_the_one_on_file() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        context.market.register(265598);

        ccp.handle_position_feed(
            "6008=265598\x016064=100\x016101=150.0\x01".as_bytes(),
            &mut None, &mut context, &shared, &None, &mut hb,
        );
        let basis = shared.portfolio.position_info(265598).map(|i| i.avg_cost);
        assert_eq!(basis, Some(150 * crate::types::PRICE_SCALE));

        // Same holding, stated without a cost.
        ccp.handle_position_feed(
            "6008=265598\x016064=100\x01".as_bytes(),
            &mut None, &mut context, &shared, &None, &mut hb,
        );
        assert_eq!(
            shared.portfolio.position_info(265598).map(|i| i.avg_cost), basis,
            "the basis on file stands where the row states none",
        );

        // A row that closes the holding takes the basis with it, whether or not
        // it states one, or the next position in this contract opens against
        // the last one's cost.
        ccp.handle_position_feed(
            "6008=265598\x016064=0\x016101=151.0\x01".as_bytes(),
            &mut None, &mut context, &shared, &None, &mut hb,
        );
        assert_eq!(
            shared.portfolio.position_info(265598).map(|i| i.avg_cost), Some(0),
            "a closed holding keeps no basis, not even one the row states",
        );
        ccp.handle_position_feed(
            "6008=265598\x016064=100\x016101=150.0\x01".as_bytes(),
            &mut None, &mut context, &shared, &None, &mut hb,
        );

        ccp.handle_position_feed(
            "6008=265598\x016064=0\x01".as_bytes(),
            &mut None, &mut context, &shared, &None, &mut hb,
        );
        assert_eq!(
            shared.portfolio.position_info(265598).map(|i| i.avg_cost), Some(0),
            "a closed holding leaves no basis behind",
        );
        ccp.handle_position_feed(
            "6008=265598\x016064=100\x016101=150.0\x01".as_bytes(),
            &mut None, &mut context, &shared, &None, &mut hb,
        );

        // Stated as zero, which is the broker saying zero.
        ccp.handle_position_feed(
            "6008=265598\x016064=100\x016101=0\x01".as_bytes(),
            &mut None, &mut context, &shared, &None, &mut hb,
        );
        assert_eq!(
            shared.portfolio.position_info(265598).map(|i| i.avg_cost), Some(0),
            "a stated zero is a value, not an absence",
        );
    }

    /// The feed is the account's own statement of what it holds. It reached the
    /// portfolio and the event, and not the table the callback side reads — so
    /// a process that restarted holding stock ran its first decisions against
    /// flat, and a strategy sizing from `position()` bought what it already had.
    #[test]
    fn a_position_feed_is_adopted_by_the_engine_not_only_published() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let instrument = context.market.register(265598);
        assert_eq!(context.position(instrument), 0.0, "the engine starts knowing nothing");

        ccp.handle_position_feed(
            "6008=265598\x016064=500\x016101=151.0\x01".as_bytes(),
            &mut None, &mut context, &shared, &None, &mut hb,
        );

        assert_eq!(context.position(instrument), 500.0, "the account holds 500 and so does the engine");
        assert_eq!(shared.portfolio.position(instrument), 500.0);

        // A later statement is adopted too, not accumulated on top.
        ccp.handle_position_feed(
            "6008=265598\x016064=300\x016101=151.0\x01".as_bytes(),
            &mut None, &mut context, &shared, &None, &mut hb,
        );
        assert_eq!(context.position(instrument), 300.0, "the server's number wins, it is not added");
    }

    /// ibx#296: the 75 feed defaulted its running quantity to zero, so an entry
    /// carrying a conId but no parseable 6064 flattened a live position and
    /// published it — the same defect ibx#261 fixed on the account-update path.
    #[test]
    fn a_position_feed_entry_without_a_quantity_leaves_the_position_alone() {
        for body in [
            // no 6064 at all
            "6008=265598\x016101=151.0\x01",
            // present but not a number
            "6008=265598\x016064=abc\x016101=151.0\x01",
            // parses, but is not a quantity
            "6008=265598\x016064=NaN\x016101=151.0\x01",
            // the same entry flushed by the next conId rather than by the end
            // of the message — a repeating group publishes at both boundaries.
            "6008=265598\x016101=151.0\x016008=756733\x016064=5\x01",
            "6008=265598\x016064=abc\x016101=151.0\x016008=756733\x016064=5\x01",
        ] {
            let mut ccp = CcpState::new();
            let mut context = Context::new();
            let shared = SharedState::new();
            let mut hb = HeartbeatState::new();
            let (tx, rx) = std::sync::mpsc::sync_channel(4096);
            let event_tx = Some(tx);
            let instrument = context.market.register(265598);
            shared.portfolio.set_position_info(PositionInfo {
                con_id: 265598, position: 100.0, avg_cost: 0, ..Default::default()
            });
            shared.portfolio.set_position(instrument, 100.0);

            ccp.handle_position_feed(
                body.as_bytes(), &mut None, &mut context, &shared, &event_tx, &mut hb);

            // All three stores move together, so all three are asserted: the
            // row callers read, the atomic the engine reads, and the event.
            assert_eq!(
                shared.portfolio.position_info(265598).map(|p| p.position), Some(100.0),
                "{body:?} must not flatten the position row",
            );
            assert_eq!(
                shared.portfolio.position(instrument), 100.0,
                "{body:?} must not flatten the shared position",
            );
            let flattened = rx.try_iter().any(|e| matches!(
                e, Event::PositionUpdate { con_id: 265598, position: 0.0, .. }));
            assert!(!flattened, "{body:?} must not publish a flat");
        }
    }

    /// The positive control for the test above: an entry that does state a
    /// quantity has to reach all three stores, and at the flush triggered by
    /// the next conId rather than only at the end of the message. Without this
    /// the absence assertions pass just as well against a feed that publishes
    /// nothing at all.
    #[test]
    fn a_position_feed_entry_with_a_quantity_publishes_it_everywhere() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        let event_tx = Some(tx);
        let instrument = context.market.register(265598);

        // Two entries, so the first is flushed by the second's conId.
        let body = "6008=265598\x016064=42\x016101=151.0\x016008=756733\x016064=5\x01";
        ccp.handle_position_feed(
            body.as_bytes(), &mut None, &mut context, &shared, &event_tx, &mut hb);

        assert_eq!(
            shared.portfolio.position_info(265598).map(|p| p.position), Some(42.0),
            "the position row",
        );
        assert_eq!(shared.portfolio.position(instrument), 42.0, "the shared position");
        assert!(
            rx.try_iter().any(|e| matches!(
                e, Event::PositionUpdate { con_id: 265598, position: 42.0, .. })),
            "the published event",
        );
    }

    /// An explicit zero is a genuine flat and must still be published.
    #[test]
    fn a_position_feed_entry_with_an_explicit_zero_still_flattens() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let instrument = context.market.register(265598);
        shared.portfolio.set_position_info(PositionInfo {
            con_id: 265598, position: 100.0, avg_cost: 0, ..Default::default()
        });
        shared.portfolio.set_position(instrument, 100.0);

        ccp.handle_position_feed(
            b"6008=265598\x016064=0\x016101=151.0\x01",
            &mut None, &mut context, &shared, &None, &mut hb);

        assert_eq!(
            shared.portfolio.position_info(265598).map(|p| p.position), Some(0.0),
            "an explicit zero is a genuine flat",
        );
    }

    // ibx#205: a margin-reducing preview (close, cash-account sell) resolves to a
    // post-trade init margin of exactly 0, which the gateway sends as numeric "0"
    // (ib-agent#160). The old `> 0.0` guard dropped it and the caller timed out.
    #[test]
    fn what_if_zero_init_margin_is_delivered() {
        let (mut ccp, mut context, shared) = what_if_test_state();
        let frame = what_if_frame(&ZERO_CLOSE_FIELDS);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let responses = shared.orders.drain_what_if_responses();
        assert_eq!(responses.len(), 1, "zero-margin preview must be delivered");
        assert_eq!(responses[0].init_margin_after, 0);
        // The completed preview consumes the pending order.
        assert!(context.order(42).is_none());
    }

    // The not-ready ack carries the literal "n/a" in all six margin fields
    // (ib-agent#160); it must be skipped so only the real data frame surfaces.
    #[test]
    fn what_if_not_ready_ack_is_skipped() {
        let (mut ccp, mut context, shared) = what_if_test_state();
        let frame = what_if_frame(&[
            (6826, "n/a"), (6827, "n/a"), (6828, "n/a"),
            (6092, "n/a"), (6093, "n/a"), (6094, "n/a"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        assert!(shared.orders.drain_what_if_responses().is_empty(),
            "n/a ack must not surface as a response");
        // The order stays pending for the subsequent data frame.
        assert!(context.order(42).is_some());
    }

    // ibx#213: the gateway's real-frame test is "any of the six margin fields
    // is set", not "6092 is set". A preview that omits 6092 but carries
    // numeric siblings must be delivered, with the absent field read as 0.
    #[test]
    fn what_if_without_6092_but_numeric_siblings_is_delivered() {
        let (mut ccp, mut context, shared) = what_if_test_state();
        let frame = what_if_frame(&[(6093, "0"), (6094, "945923.47")]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let responses = shared.orders.drain_what_if_responses();
        assert_eq!(responses.len(), 1, "sibling-only preview must be delivered");
        assert_eq!(responses[0].init_margin_after, 0);
        assert_eq!(responses[0].equity_with_loan_after,
            (945923.47 * PRICE_SCALE as f64) as Price);
        assert!(context.order(42).is_none());
    }

    // ibx#214: "nan" parses as f64::NAN, so it passed the old parse-success
    // gate and surfaced as a bogus zero-margin preview. The gateway treats
    // nan as unset, so an all-nan frame is not a data frame.
    #[test]
    fn what_if_nan_sentinels_are_skipped() {
        let (mut ccp, mut context, shared) = what_if_test_state();
        let frame = what_if_frame(&[
            (6826, "nan"), (6827, "nan"), (6828, "nan"),
            (6092, "nan"), (6093, "nan"), (6094, "nan"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        assert!(shared.orders.drain_what_if_responses().is_empty(),
            "all-nan frame must not surface as a response");
        assert!(context.order(42).is_some());
    }

    // Mixed frame: a nan field is unset, but one finite sibling makes the
    // frame real. The nan field itself must read as 0, not poison the price.
    #[test]
    fn what_if_nan_field_with_finite_sibling_is_delivered() {
        let (mut ccp, mut context, shared) = what_if_test_state();
        let frame = what_if_frame(&[(6092, "nan"), (6094, "945923.47")]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let responses = shared.orders.drain_what_if_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].init_margin_after, 0, "nan field reads as unset/0");
        assert_eq!(responses[0].equity_with_loan_after,
            (945923.47 * PRICE_SCALE as f64) as Price);
    }

    // ibx#210: a working order carries wire 39=0 whether it is routed or not.
    // The gateway reports PreSubmitted while it waits (e.g. placed pre-market)
    // and Submitted only once routed to an exchange. The discriminator is the
    // routing tags on the same exec report, not a distinct wire status
    // (ib-agent#162).
    fn ord_status_test_state() -> (CcpState, Context, SharedState) {
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.insert_order(crate::types::Order::new(
            42, instrument, Side::Buy, 1, 100 * PRICE_SCALE, b'2', b'0', 0,
        )); // starts at PendingSubmit
        (CcpState::new(), context, SharedState::new())
    }

    /// A recovery record arriving with the instrument table already full used
    /// to take the engine down. A missing order beats a dead hot loop, and the
    /// conversion to the fallible register is what makes that true — nothing
    /// else in the suite fails if it is reverted.
    #[test]
    fn a_full_instrument_table_does_not_abort_the_recovery_path() {
        let mut context = Context::new();
        let mut ccp = CcpState::new();
        let shared = SharedState::new();

        // Fill every slot, so the next registration has nowhere to go.
        for con_id in 1..=(crate::types::MAX_INSTRUMENTS as i64) {
            assert!(context.try_register_instrument(con_id).is_some(), "slot {con_id}");
        }
        assert!(
            context.try_register_instrument(999_999).is_none(),
            "the table really is full",
        );

        let mut frame = std::collections::HashMap::new();
        for (tag, val) in [
            (11u32, "42"), (150, "0"), (39, "0"), (6008, "888888"),
            (38, "100"), (55, "SPY"), (54, "1"),
        ] {
            frame.insert(tag, val.to_string());
        }

        // The point of the test: this must return rather than panic.
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        assert!(
            context.order(42).is_none(),
            "the order is not tracked, which is the acknowledged cost",
        );
    }
    /// Build a fill report for order 42. `extra` adds or overrides tags.
    fn fill_frame(extra: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
        let mut m = std::collections::HashMap::new();
        for (tag, val) in [
            (11u32, "42"), (150u32, "F"), (39u32, "1"),
            (17u32, "EXEC-1"), (31u32, "100.0"), (32u32, "10"), (151u32, "90"),
            (14u32, "10"),
            (60u32, "20260101-16:00:00"),
        ] {
            m.insert(tag, val.to_string());
        }
        for (tag, val) in extra {
            m.insert(*tag, val.to_string());
        }
        m
    }

    fn tracked_order_state() -> (CcpState, Context, SharedState) {
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.insert_order(crate::types::Order::new(
            42, instrument, Side::Buy, 100, 100 * PRICE_SCALE, b'2', b'0', 0,
        ));
        (CcpState::new(), context, SharedState::new())
    }

    /// ibx#320: at session start the gateway replays recent executions, each
    /// carrying its original ExecID and a resend marker. A fresh process has
    /// never seen that ID, so the dedup window cannot stop it — and the order
    /// is tracked by then, because the recovery insert ran first. The result
    /// was a fill event and a position move for something that happened before
    /// the process started.
    #[test]
    fn a_resent_execution_does_not_book_a_fill() {
        for marker in [(97u32, "Y"), (43u32, "Y")] {
            let (mut ccp, mut context, shared) = tracked_order_state();
            context.update_order_filled(42, 10); // already counted
            let frame = fill_frame(&[marker]);
            ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

            assert!(
                shared.orders.drain_fills().is_empty(),
                "tag {} = Y restates history and must not book", marker.0,
            );
            assert_eq!(context.position(0), 0.0, "and must not move the position");
        }

        // The positive control: the same report without a marker is a real
        // execution and still books, so the assertions above are not passing
        // against a handler that books nothing.
        let (mut ccp, mut context, shared) = tracked_order_state();
        ccp.handle_exec_report(&fill_frame(&[]), &mut context, &shared, &None, "");
        assert_eq!(shared.orders.drain_fills().len(), 1, "a live execution books");
        assert_eq!(context.position(0), 10.0);
    }

    /// ibx#320 end to end, as a fresh process sees it: the gateway replays the
    /// order as a recovery record and then replays its executions. The record
    /// carries the cumulative quantity already filled, so the executions behind
    /// it state nothing new. Treating that record as unfilled made every one of
    /// them look like fresh quantity, and each emitted a fill for something
    /// that happened before the process started.
    #[test]
    fn a_fresh_process_does_not_book_the_history_it_is_replayed() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();

        // 1. The recovery record: not tracked locally, ten of a hundred filled.
        let mut recovery = std::collections::HashMap::new();
        for (tag, val) in [
            (11u32, "78"), (150u32, "0"), (39u32, "0"), (6008u32, "756733"),
            (38u32, "100"), (14u32, "10"), (55u32, "SPY"), (54u32, "1"), (40u32, "2"),
        ] {
            recovery.insert(tag, val.to_string());
        }
        ccp.handle_exec_report(&recovery, &mut context, &shared, &None, "");
        let _ = shared.orders.drain_fills();

        assert_eq!(
            context.order(78).expect("recovered").filled, 10,
            "the record's own cumulative quantity is the baseline",
        );

        // 2. Its replayed execution, carrying the same cumulative quantity.
        let mut replay = std::collections::HashMap::new();
        for (tag, val) in [
            (11u32, "78"), (150u32, "F"), (39u32, "1"), (97u32, "Y"),
            (17u32, "OLD-EXEC"), (14u32, "10"), (32u32, "10"), (31u32, "100.0"),
            (151u32, "90"), (60u32, "20260101-16:00:00"),
        ] {
            replay.insert(tag, val.to_string());
        }
        ccp.handle_exec_report(&replay, &mut context, &shared, &None, "");

        assert!(
            shared.orders.drain_fills().is_empty(),
            "the replayed execution states nothing the record did not already carry",
        );
        assert_eq!(context.order(78).expect("tracked").filled, 10, "and nothing is double-counted");
    }

    /// The case a blanket suppression of marked reports loses. A CCP reconnect
    /// keeps this state — window and order book both survive — and the gateway
    /// replays recent executions on the new session. A fill that executed
    /// during the outage therefore arrives marked, with an ExecID this session
    /// has never seen, and is the first news of it. Refusing it would leave the
    /// order permanently short a real fill.
    #[test]
    fn a_resent_execution_carrying_new_quantity_is_still_booked() {
        let (mut ccp, mut context, shared) = tracked_order_state();

        // Five already booked before the outage.
        context.update_order_filled(42, 5);

        // The replay carries eight cumulative — three of which are news.
        let frame = fill_frame(&[(97, "Y"), (14, "8"), (32, "3"), (151, "92")]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        assert_eq!(
            shared.orders.drain_fills().len(), 1,
            "a marked report carrying quantity the order does not have is a real fill",
        );
        assert_eq!(context.position(0), 3.0);

        // And a second copy of that same replay states no more, so it is history.
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        assert!(
            shared.orders.drain_fills().is_empty(),
            "restating the same cumulative quantity is not new",
        );
        assert_eq!(context.position(0), 3.0);
    }

    /// Two genuine slices of one order, same size and price inside one
    /// timestamp tick — the ordinary shape of algo and iceberg execution. The
    /// synthesised key must tell them apart, which the cumulative quantity does
    /// because it advances with every execution on the order.
    #[test]
    fn two_same_priced_slices_in_one_tick_are_not_one_execution() {
        let (mut ccp, mut context, shared) = tracked_order_state();

        let mut first = fill_frame(&[(32, "10"), (151, "90"), (14, "10")]);
        first.remove(&17);
        let mut second = fill_frame(&[(32, "10"), (151, "80"), (14, "20")]);
        second.remove(&17);

        ccp.handle_exec_report(&first, &mut context, &shared, &None, "");
        ccp.handle_exec_report(&second, &mut context, &shared, &None, "");

        assert_eq!(shared.orders.drain_fills().len(), 2, "both slices book");
        assert_eq!(context.position(0), 20.0);
    }

    /// ibx#260: the dedup window was skipped entirely when tag 17 was absent,
    /// which is the shape a replay takes — so the copy booked a second time and
    /// the position doubled. Without an ExecID the execution is keyed on the
    /// fields that identify it instead.
    #[test]
    fn an_execution_without_an_exec_id_is_still_deduplicated() {
        let (mut ccp, mut context, shared) = tracked_order_state();
        let mut frame = fill_frame(&[]);
        frame.remove(&17);

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        assert_eq!(shared.orders.drain_fills().len(), 1, "booked once, not twice");
        assert_eq!(context.position(0), 10.0, "and the position moved once");

        // A genuinely different execution on the same order is not swallowed by
        // the synthesised key.
        let mut other = fill_frame(&[]);
        other.remove(&17);
        other.insert(32, "5".to_string());
        ccp.handle_exec_report(&other, &mut context, &shared, &None, "");
        assert_eq!(shared.orders.drain_fills().len(), 1, "a distinct execution still books");
        assert_eq!(context.position(0), 15.0);
    }

    /// A long session rolls executions out of the ExecID window, and a replay
    /// arrives unordered and without ExecIDs of its own. Summing what each
    /// report says it executed counts quantity the order already holds; reading
    /// the cumulative figure it reports settles on the true total whatever
    /// order the copies arrive in.
    #[test]
    fn a_replay_of_booked_history_adds_nothing_to_the_order() {
        let (mut ccp, mut context, shared) = tracked_order_state();
        context.update_order_filled(42, 12); // both executions already booked

        let mut later = fill_frame(&[(97, "Y"), (14, "12"), (32, "4"), (151, "88")]);
        later.remove(&17);
        let mut earlier = fill_frame(&[(97, "Y"), (14, "8"), (32, "3"), (151, "92")]);
        earlier.remove(&17);
        ccp.handle_exec_report(&later, &mut context, &shared, &None, "");
        ccp.handle_exec_report(&earlier, &mut context, &shared, &None, "");

        assert!(shared.orders.drain_fills().is_empty(), "history restated is not new quantity");
        assert_eq!(context.order(42).unwrap().filled, 12, "and the order is not overcounted");
        assert_eq!(context.position(0), 0.0);

        // A fill from the same replay that this session has not booked is news
        // and still reaches the caller.
        let mut fresh = fill_frame(&[(97, "Y"), (14, "15"), (32, "3"), (151, "85")]);
        fresh.remove(&17);
        ccp.handle_exec_report(&fresh, &mut context, &shared, &None, "");
        assert_eq!(shared.orders.drain_fills().len(), 1, "quantity the order lacks still books");
        assert_eq!(context.position(0), 3.0);
    }

    /// The same execution delivered marked and then unmarked. The cumulative
    /// figure decides the marked copy, but the unmarked one is an ordinary
    /// report and the window is the only thing that can catch it — so a marked
    /// report has to be remembered even though it was not judged by the window.
    #[test]
    fn a_marked_execution_is_remembered_for_its_unmarked_twin() {
        let (mut ccp, mut context, shared) = tracked_order_state();
        context.update_order_filled(42, 5);

        let marked = fill_frame(&[(97, "Y"), (17, "E-9"), (14, "9"), (32, "4"), (151, "91")]);
        ccp.handle_exec_report(&marked, &mut context, &shared, &None, "");
        assert_eq!(shared.orders.drain_fills().len(), 1, "the marked copy books what is new");
        assert_eq!(context.order(42).unwrap().filled, 9);

        // The same execution again, this time without its marker.
        let unmarked = fill_frame(&[(17, "E-9"), (14, "9"), (32, "4"), (151, "91")]);
        ccp.handle_exec_report(&unmarked, &mut context, &shared, &None, "");

        assert!(
            shared.orders.drain_fills().is_empty(),
            "the window catches the copy the cumulative figure cannot judge",
        );
        assert_eq!(context.order(42).unwrap().filled, 9, "and nothing is double-booked");
    }

    /// ibx#344: the ExecID window evicts oldest-first, so a replay batch deeper
    /// than the window no longer holds its own head and the duplicate booked a
    /// second time. For an order this session tracks, that window was the only
    /// guard.
    ///
    /// A replayed execution is marked, so it is booked on the cumulative
    /// quantity it reports rather than on the increment — and a copy that
    /// restates quantity the order already holds adds nothing whether or not
    /// its ExecID is still in the window. The window stops being the guard.
    #[test]
    fn a_replay_deeper_than_the_exec_id_window_does_not_double_count() {
        let (mut ccp, mut context, shared) = tracked_order_state();
        context.update_order_filled(42, 12);

        // The window has rolled past this execution, so its ID is unseen here —
        // which is the whole point: the dedup window cannot be what saves this.
        let replayed = fill_frame(&[(97, "Y"), (17, "EVICTED-1"), (14, "12"), (32, "4"), (151, "88")]);

        ccp.handle_exec_report(&replayed, &mut context, &shared, &None, "");

        assert!(
            shared.orders.drain_fills().is_empty(),
            "a replay the window has forgotten still adds no quantity the order holds",
        );
        assert_eq!(context.order(42).unwrap().filled, 12);
        assert_eq!(context.position(0), 0.0);
    }

    /// The same marked execution delivered twice, both copies carrying more
    /// cumulative quantity than the order held when the first arrived.
    #[test]
    fn a_marked_execution_delivered_twice_books_once() {
        let (mut ccp, mut context, shared) = tracked_order_state();
        context.update_order_filled(42, 5);

        let frame = fill_frame(&[(97, "Y"), (14, "12"), (32, "4"), (151, "88")]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        assert_eq!(shared.orders.drain_fills().len(), 1, "the second copy adds nothing");
        assert_eq!(context.order(42).unwrap().filled, 12);
        assert_eq!(context.position(0), 7.0);
    }

    /// A replacement that raises the total lets an order fill the same size at
    /// the same price and leave the same quantity behind twice. Everything the
    /// synthesised key had to work with repeats except the cumulative figure.
    #[test]
    fn a_raised_total_does_not_collapse_two_slices_into_one() {
        let (mut ccp, mut context, shared) = tracked_order_state();

        let mut first = fill_frame(&[(32, "10"), (151, "90"), (14, "10")]);
        first.remove(&17);
        // Total raised from 100 to 110; the next slice again leaves 90.
        let mut second = fill_frame(&[(32, "10"), (151, "90"), (14, "20")]);
        second.remove(&17);

        ccp.handle_exec_report(&first, &mut context, &shared, &None, "");
        ccp.handle_exec_report(&second, &mut context, &shared, &None, "");

        assert_eq!(shared.orders.drain_fills().len(), 2, "both slices book");
        assert_eq!(context.position(0), 20.0);
    }

    /// An execution with no ExecID that arrives ahead of the recovery record
    /// for its order. The key must not be spent on the copy that had nothing to
    /// book against, or the delivery that finally could is refused.
    #[test]
    fn a_key_is_not_spent_before_the_order_exists() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let mut frame = fill_frame(&[]);
        frame.remove(&17);

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        assert!(shared.orders.drain_fills().is_empty(), "nothing to book against yet");

        let instrument = context.register_instrument(756733);
        context.insert_order(crate::types::Order::new(
            42, instrument, Side::Buy, 100, 100 * PRICE_SCALE, b'2', b'0', 0,
        ));
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        assert_eq!(shared.orders.drain_fills().len(), 1, "the execution is still bookable");
        assert_eq!(context.position(0), 10.0);
    }
    /// ibx#369: the request was recorded as pending whether or not it went out.
    /// The send error was discarded and the push sat outside the block that
    /// needs a connection at all, so a request issued while the transport was
    /// down was queued with nothing on the wire to answer it.
    #[test]
    fn a_matching_symbols_request_that_was_not_sent_is_not_recorded() {
        let mut ccp = CcpState::new();
        let mut hb = HeartbeatState::new();

        // No transport at all.
        let mut no_conn: Option<Connection> = None;
        ccp.send_matching_symbols_request(7, "AAPL", &mut no_conn, &mut hb);
        assert!(
            ccp.pending_matching_symbols.is_empty(),
            "nothing was sent, so nothing is awaiting a reply",
        );

        // And with one, it is recorded.
        let listener = std::net::TcpListener::bind("127.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        ccp.send_matching_symbols_request(8, "AAPL", &mut conn, &mut hb);
        assert_eq!(ccp.pending_matching_symbols.len(), 1, "a sent request is awaited");
        assert_eq!(ccp.pending_matching_symbols[0].0, 8);
    }

    /// The venue reads a chain request positionally, so the tags have to be
    /// stated in the order it expects them and the underlying has to be named
    /// on the tag that suits the derivative being asked for.
    #[test]
    fn a_chain_request_states_its_tags_in_order() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut ccp = CcpState::new();
        let mut hb = HeartbeatState::new();
        let shared = SharedState::new();
        let mut buf = [0u8; 4096];
        // The tags a caller cannot see are the session's own; the request
        // itself starts at the sub-message type.
        let sent = |peer: &mut std::net::TcpStream, buf: &mut [u8]| -> Vec<(String, String)> {
            let n = peer.read(buf).unwrap();
            String::from_utf8_lossy(&buf[..n])
                .split('\u{1}')
                .filter_map(|f| f.split_once('=').map(|(t, v)| (t.to_string(), v.to_string())))
                .skip_while(|(t, _)| t != "6040")
                .take_while(|(t, _)| t != "10")
                .collect()
        };

        ccp.send_option_params_request(7, "aapl", "", "STK", 265598, &mut conn, &mut hb, &shared);
        let fields = sent(&mut peer, &mut buf);
        let names: Vec<&str> = fields.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(names, ["6040", "55", "310", "6346", "6320", "6994"], "an equity chain: {fields:?}");
        assert_eq!(fields[0].1, "138");
        assert_eq!(fields[1].1, "AAPL", "the symbol is stated upper cased");
        assert_eq!(fields[2].1, "OPT");
        assert_eq!(fields[3].1, "265598");
        assert_eq!(ccp.pending_option_params.len(), 1, "and the request awaits its reply");

        ccp.send_option_params_request(8, "ES", "CME", "FUT", 495512563, &mut conn, &mut hb, &shared);
        let fields = sent(&mut peer, &mut buf);
        let names: Vec<&str> = fields.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(names, ["6040", "55", "310", "6346", "6320", "6994", "6995"], "a futures chain: {fields:?}");
        assert_eq!(fields[2].1, "FOP", "options on a future are future options");
        assert_eq!(fields[6].1, "CME", "and the venue rides only for a future");

        ccp.send_option_params_request(9, "SPX", "CME", "IND", 416904, &mut conn, &mut hb, &shared);
        let fields = sent(&mut peer, &mut buf);
        let names: Vec<&str> = fields.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(names, ["6040", "55", "310", "6457", "6320", "6994"], "a futures chain on an index: {fields:?}");
        assert_eq!(fields[2].1, "FOP");
    }

    /// A caller is waiting for the end of a request that never reached the
    /// wire. Nothing on the socket will ever end it, so the client does.
    #[test]
    fn a_chain_request_that_could_not_be_sent_is_still_answered() {
        let mut ccp = CcpState::new();
        let mut hb = HeartbeatState::new();
        let shared = SharedState::new();
        let mut no_conn: Option<Connection> = None;

        ccp.send_option_params_request(7, "AAPL", "", "STK", 265598, &mut no_conn, &mut hb, &shared);

        assert!(ccp.pending_option_params.is_empty(), "nothing was sent, so nothing is awaited");
        let answered = shared.reference.drain_option_params();
        assert_eq!(answered.len(), 1, "the request still ends");
        assert!(answered[0].2.is_empty(), "with nothing in the chain");
    }

    /// The reply states no request id, so the symbol it names is what ties it
    /// back to the request, and the conId the caller asked under is what the
    /// callback reports.
    #[test]
    fn a_chain_reply_answers_the_request_that_named_its_underlying() {
        let mut ccp = CcpState::new();
        let shared = SharedState::new();
        ccp.pending_option_params.push((3, "SPY".into(), 756733, Instant::now() + OPTION_CHAIN_TIMEOUT));
        ccp.pending_option_params.push((7, "AAPL".into(), 265598, Instant::now() + OPTION_CHAIN_TIMEOUT));
        let msg = fix::fix_build(
            &[
                (fix::TAG_MSG_TYPE, "U"),
                (6040, "139"),
                (55, "AAPL"),
                (6775, "20260116/20260320/EXPW=20260109"),
                (6346, "265598"),
                (100, "SMART"),
                (6058, "AAPL"),
                (231, "100"),
                (6997, "140.0;145.0"),
            ],
            1,
        );

        ccp.handle_option_chain(&msg, &shared);

        assert_eq!(ccp.pending_option_params.len(), 1, "only the request it answers is spent");
        assert_eq!(ccp.pending_option_params[0].0, 3);
        let answered = shared.reference.drain_option_params();
        assert_eq!(answered.len(), 1);
        let (req_id, con_id, scopes) = &answered[0];
        assert_eq!(*req_id, 7);
        assert_eq!(*con_id, 265598, "the underlying the caller asked about");
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].exchange, "SMART");
        assert_eq!(scopes[0].trading_class, "AAPL");
        assert_eq!(scopes[0].multiplier, "100");
        assert_eq!(scopes[0].expirations, vec!["20260116", "20260320"]);
        assert_eq!(scopes[0].strikes, vec![140.0, 145.0]);
    }

    /// An entry left in the queue would both hang its caller and stand ready
    /// to absorb the answer to a later request for the same underlying.
    #[test]
    fn an_unanswered_chain_request_is_given_up_on() {
        let mut ccp = CcpState::new();
        let shared = SharedState::new();
        ccp.pending_option_params.push((7, "AAPL".into(), 265598, Instant::now() - Duration::from_secs(1)));
        ccp.pending_option_params.push((8, "SPY".into(), 756733, Instant::now() + OPTION_CHAIN_TIMEOUT));

        ccp.sweep_pending_option_params(&shared);

        assert_eq!(ccp.pending_option_params.len(), 1, "the expired one is dropped");
        assert_eq!(ccp.pending_option_params[0].0, 8, "and the live one is kept");
        let answered = shared.reference.drain_option_params();
        assert_eq!(answered.len(), 1, "the caller of the expired one is told it is over");
        assert_eq!(answered[0].0, 7);
    }

    /// Nothing expired an unanswered request, so it stayed queued for the life
    /// of the process — and the reply matcher falls back to the head of that
    /// queue when a reply carries no echoed request id, so a stale entry could
    /// absorb a later request's answer.
    #[test]
    fn an_unanswered_matching_symbols_request_is_given_up_on() {
        let mut ccp = CcpState::new();
        ccp.pending_matching_symbols.push((7, Instant::now() - Duration::from_secs(1)));
        ccp.pending_matching_symbols.push((8, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

        ccp.sweep_pending_matching_symbols();

        assert_eq!(ccp.pending_matching_symbols.len(), 1, "the expired one is dropped");
        assert_eq!(ccp.pending_matching_symbols[0].0, 8, "and the live one is kept");
    }
    /// Tag 583 is the link id the engine sends the OCA group on. Reading it
    /// back as a parent produced a stable non-zero value shared by every order
    /// in the group — none of which has a parent — and nothing told it apart
    /// from a real link.
    #[test]
    fn an_oca_group_is_not_reported_as_a_parent() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1"),
            (583, "PROBE-OCA-1"),
        ]);

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let updates = shared.orders.drain_order_updates();
        assert_eq!(updates.len(), 1, "the status is still reported");
        assert_eq!(
            updates[0].parent_id, 0,
            "an order in an OCA group has no parent, so none is reported",
        );
    }

    /// Not just the one group name, and not just one status: any value on 583
    /// is a link id rather than a parent, at every point in the order's life.
    #[test]
    fn no_group_name_or_status_produces_a_parent() {
        for group in ["PROBE-OCA-1", "G", "12345", "a name with spaces"] {
            for (ord_status, exec_type) in [("0", "0"), ("1", "2"), ("2", "2"), ("4", "4")] {
                let (mut ccp, mut context, shared) = ord_status_test_state();
                let frame = exec_report_frame(&[
                    (39, ord_status), (150, exec_type), (100, "ARCA"), (198, "ARCA:1"),
                    (583, group),
                ]);
                ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
                let updates = shared.orders.drain_order_updates();
                // Without this a case that produced no update at all would
                // pass the loop below by never entering it.
                assert_eq!(
                    updates.len(), 1,
                    "group {group:?} at status {ord_status} must produce one update",
                );
                assert_eq!(
                    updates[0].parent_id, 0,
                    "group {group:?} at status {ord_status} must not become a parent",
                );
            }
        }
    }

    /// A report carrying no group reported no parent before this change too, so
    /// that case alone cannot tell the fix from the bug.
    #[test]
    fn a_report_without_a_group_still_has_no_parent() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[(39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1")]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let updates = shared.orders.drain_order_updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].parent_id, 0);
    }

    /// Tag 6107 is what the bracket path *sends* a parent on. Whether the
    /// gateway ever echoes it on a report has not been established here, and
    /// the engine does not read it either way; this pins that, so wiring it up
    /// becomes a deliberate change with evidence behind it rather than a
    /// silent one. It passes on the old implementation too — it guards a
    /// different invariant from the rest of this change.
    #[test]
    fn tag_6107_is_not_read_back_as_a_parent() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1"), (6107, "4242"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let updates = shared.orders.drain_order_updates();
        assert_eq!(updates[0].parent_id, 0, "6107 is a client id, not a parent order");
    }

    /// A refused revision arrives on the same message as an accepted one and
    /// was read as the acceptance, so a modify the gateway would not make was
    /// reported to the caller as made.
    #[test]
    fn a_refused_revision_is_not_an_acknowledgement() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "5"), (150, "5"), (100, "ARCA"), (198, "ARCA:1"), (378, "102"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let updates = shared.orders.drain_order_updates();
        assert!(
            !updates.iter().any(|u| u.status == crate::types::OrderStatus::Submitted),
            "a refused revision does not put the order back to working: {updates:?}",
        );
    }

    /// A busted trade arrives as an execution like any other. Adding its
    /// quantity booked a fill the account no longer has.
    #[test]
    fn a_busted_execution_reconciles_rather_than_adds() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "1"), (150, "F"), (100, "ARCA"), (198, "ARCA:1"),
            (20, "1"), (32, "50"), (31, "412.25"), (14, "0"), (38, "100"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let fills = shared.orders.drain_fills();
        assert!(
            fills.iter().all(|f| f.qty == 0),
            "a bust does not add to what is filled: {fills:?}",
        );
    }

    /// A correction restates an execution that was already booked. Adding its
    /// quantity on top counts the same trade twice; the cumulative figure is
    /// what the order actually holds.
    #[test]
    fn a_corrected_execution_reconciles_to_the_cumulative_figure() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        // The order already holds 50. The correction restates the trade at 60.
        let first = exec_report_frame(&[
            (39, "1"), (150, "F"), (100, "ARCA"), (198, "ARCA:1"),
            (17, "exec-1"), (32, "50"), (31, "412.25"), (14, "50"), (38, "100"),
        ]);
        ccp.handle_exec_report(&first, &mut context, &shared, &None, "");
        let booked: i64 = shared.orders.drain_fills().iter().map(|f| f.qty).sum();
        assert_eq!(booked, 50, "the original execution books what it states");

        let corrected = exec_report_frame(&[
            (39, "1"), (150, "F"), (100, "ARCA"), (198, "ARCA:1"),
            (17, "exec-2"), (20, "2"), (32, "60"), (31, "412.25"), (14, "60"), (38, "100"),
        ]);
        ccp.handle_exec_report(&corrected, &mut context, &shared, &None, "");
        let after: i64 = shared.orders.drain_fills().iter().map(|f| f.qty).sum();
        assert_eq!(after, 10, "the correction books the difference, not the whole trade again");
    }

    /// A live order was retired by this: `D` is not in the terminal's terminal
    /// set, and reading it as cancelled told the caller an order was gone while
    /// it was still working and still able to fill.
    #[test]
    fn a_pending_status_does_not_retire_the_order() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[(39, "D"), (150, "D"), (100, "ARCA"), (198, "ARCA:1")]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let updates = shared.orders.drain_order_updates();
        assert_eq!(updates[0].status, crate::types::OrderStatus::PendingCancel,
            "D is pending, not cancelled");
        assert_ne!(updates[0].status, crate::types::OrderStatus::Cancelled);
    }

    /// The fill was thrown away with the report: an unrecognised status returned
    /// before anything read the execution, so a real fill on a status this did
    /// not know about was silently lost.
    #[test]
    fn an_unknown_status_still_books_its_fill() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "\u{7}"), (150, "F"), (100, "ARCA"), (198, "ARCA:1"),
            (32, "50"), (31, "412.25"), (14, "50"), (6, "412.25"), (38, "100"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let fills = shared.orders.drain_fills();
        assert_eq!(fills.len(), 1, "the fill survives a status this does not know");
        assert_eq!(fills[0].qty, 50);
    }

    /// Absent is not zero. Without 151 the caller was told nothing was left on an
    /// order that was still working, which reads as done.
    #[test]
    fn a_missing_leaves_qty_falls_back_to_what_is_unfilled() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "1"), (150, "1"), (100, "ARCA"), (198, "ARCA:1"),
            (38, "100"), (14, "30"), (32, "30"), (31, "412.25"), (6, "412.25"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let updates = shared.orders.drain_order_updates();
        assert_eq!(updates[0].remaining_qty, 70.0, "100 ordered less 30 filled, not 0");
    }

    /// The order id hash on tag 37 is a separate concern and must keep working.
    #[test]
    fn the_order_id_still_produces_a_stable_perm_id() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1"),
            (37, "0256d0f1.0001417e.6a6982d2.0001"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        let updates = shared.orders.drain_order_updates();
        assert_ne!(updates[0].perm_id, 0, "the order id still yields a permId");
    }

    /// The recovered side is not confined to the recovered record: every later
    /// fill for that order books through the tracked path and takes its side
    /// from here, so a guess moves the position by twice the fill in the wrong
    /// direction, and nothing afterwards distinguishes it from a stated side.
    #[test]
    fn a_recovery_record_without_a_side_is_not_tracked() {
        for missing in ["", "9", "X"] {
            let mut context = Context::new();
            let mut ccp = CcpState::new();
            let shared = SharedState::new();
            let mut frame = std::collections::HashMap::new();
            for (tag, val) in [
                (11u32, "77"), (150, "0"), (39, "0"), (6008, "756733"), (38, "100"), (55, "SPY"),
            ] {
                frame.insert(tag, val.to_string());
            }
            if !missing.is_empty() {
                frame.insert(54, missing.to_string());
            }

            ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

            assert!(
                context.order(77).is_none(),
                "Side={missing:?} must not be guessed into a tracked order",
            );
        }
    }

    /// A stated side is still recovered.
    #[test]
    fn a_recovery_record_with_a_side_is_tracked() {
        for (tag54, expected) in [("1", Side::Buy), ("2", Side::Sell), ("5", Side::ShortSell)] {
            let mut context = Context::new();
            let mut ccp = CcpState::new();
            let shared = SharedState::new();
            let mut frame = std::collections::HashMap::new();
            for (tag, val) in [
                (11u32, "77"), (150, "0"), (39, "0"), (6008, "756733"), (38, "100"),
                (55, "SPY"), (54, tag54),
            ] {
                frame.insert(tag, val.to_string());
            }

            ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

            let order = context.order(77).expect("a stated side is recovered");
            assert_eq!(order.side, expected, "Side={tag54}");
        }
    }
    /// ibx#307: an unrecognised or absent tag 59 was reported as `DAY`, and the
    /// fallback that knows what the caller actually submitted could never run,
    /// because every arm of the wire match produced a non-empty string.
    ///
    /// `DAY` is an ordinary value, so a caller reconciling its own orders got a
    /// plausible answer that disagreed with what it sent, with nothing to say so.
    #[test]
    fn an_unknown_time_in_force_falls_back_to_the_one_that_was_submitted() {
        // A tracked order submitted GTC, so a wrong answer is visibly wrong.
        let tracked = |ccp: &mut CcpState, context: &mut Context, shared: &SharedState, tif59: Option<&str>| {
            context.insert_order(crate::types::Order::new(
                42, 0, Side::Buy, 1, 100 * PRICE_SCALE, b'2', b'1', 0,
            ));
            let mut pairs = vec![(39u32, "0"), (150u32, "0"), (100u32, "ARCA"), (198u32, "ARCA:1")];
            if let Some(v) = tif59 {
                pairs.push((59, v));
            }
            let frame = exec_report_frame(&pairs);
            ccp.handle_exec_report(&frame, context, shared, &None, "");
            shared.orders.get_order_info(42).expect("published").order.tif.clone()
        };

        // Absence is the only case the fallback answers: the report states no
        // time-in-force, and this client knows what it submitted.
        let (mut ccp, mut context, shared) = ord_status_test_state();
        assert_eq!(
            tracked(&mut ccp, &mut context, &shared, None), "GTC",
            "the submitted time-in-force, not a plausible default",
        );

        // A stated code is still taken from the wire, including one that
        // happens to differ from the tracked order — the gateway is
        // authoritative when it says anything at all.
        let (mut ccp, mut context, shared) = ord_status_test_state();
        assert_eq!(tracked(&mut ccp, &mut context, &shared, Some("0")), "DAY");
        let (mut ccp, mut context, shared) = ord_status_test_state();
        assert_eq!(tracked(&mut ccp, &mut context, &shared, Some("4")), "FOK");

        // Including a code this does not name: seen as stated rather than
        // silently replaced by the local order's unrelated value.
        let (mut ccp, mut context, shared) = ord_status_test_state();
        assert_eq!(tracked(&mut ccp, &mut context, &shared, Some("5")), "5");
    }

    /// The case the test above cannot reach: an order this session never
    /// placed, arriving on the session-start recovery push with no tag 59.
    ///
    /// There is nothing to recover the time-in-force from, and the recovery
    /// insert used to invent `GTC` — which the report path then read as though
    /// it were the caller's own. An invented GTC rests until cancelled; an
    /// invented DAY expires with the session. Neither is knowledge, so the
    /// safer guess is the one that does not leave an order resting.
    #[test]
    fn a_recovered_order_without_a_time_in_force_states_none() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();

        // A recovery record: not tracked locally, states a contract and size,
        // states no time-in-force.
        let mut frame = std::collections::HashMap::new();
        for (tag, val) in [
            (11u32, "78"), (150u32, "0"), (39u32, "0"), (6008u32, "756733"),
            (38u32, "100"), (55u32, "SPY"), (54u32, "1"), (40u32, "2"),
        ] {
            frame.insert(tag, val.to_string());
        }
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        assert_eq!(
            context.order(78).expect("recovered").tif, crate::types::TIF_UNSTATED,
            "an absent time-in-force is recorded as unstated, not guessed",
        );
        assert_eq!(
            shared.orders.get_order_info(78).expect("published").order.tif, "",
            "and is reported as unstated rather than as an ordinary value",
        );

        // And a replace of it carries no tag 59, so the guess is never sent to
        // the gateway as an instruction — a fabricated DAY would expire an
        // order that is resting until cancelled.
        let listener = std::net::TcpListener::bind("127.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared_arc = std::sync::Arc::new(SharedState::new());

        context.modify(78, 100 * PRICE_SCALE, 100, false);
        crate::engine::hot_loop::order_builder::drain_and_send_orders(
            &mut conn, &mut context, "DU1", &mut hb, false, &shared_arc, false, &None,
        );

        let mut buf = [0u8; 4096];
        let n = std::io::Read::read(&mut peer, &mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        assert!(msg.contains("35=G"), "a replace was sent: {msg}");
        assert!(!msg.split('\u{1}').any(|f| f.starts_with("59=")),
            "a replace must not restate a time-in-force the order never had: {msg}");
    }

    fn cancel_reject_frame(reason_code: &str) -> std::collections::HashMap<u32, String> {
        let mut m = std::collections::HashMap::new();
        m.insert(41u32, "C42".to_string()); // OrigClOrdID
        m.insert(434u32, "1".to_string());
        m.insert(102u32, reason_code.to_string());
        m
    }

    fn tracked_for_cancel(context: &mut Context) {
        let instrument = context.register_instrument(756733);
        context.insert_order(crate::types::Order::new(
            42, instrument, Side::Buy, 100, 100 * PRICE_SCALE, b'2', b'0', 0,
        ));
        context.update_order_status(42, crate::types::OrderStatus::PendingCancel);
    }

    /// ibx#252: a cancel answered with UnknownOrder says the order does not
    /// exist on the gateway's side. Forcing it back to working asserted the
    /// opposite of the message being handled, and the engine's own view governs
    /// subsequent cancels, modifies and reconnect bookkeeping — so a phantom
    /// order persisted there while the cache row that would have surfaced it
    /// was removed.
    #[test]
    fn an_unknown_order_rejection_retires_the_order() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        tracked_for_cancel(&mut context);
        shared.orders.push_order_info(42, RichOrderInfo {
            contract: api::Contract::default(),
            order: api::Order::default(),
            order_state: api::OrderState::default(),
            last_exec: api::Execution::default(),
        });

        ccp.handle_cancel_reject(&cancel_reject_frame("1"), &mut context, &shared, &None);

        assert!(
            context.order(42).is_none(),
            "the engine must not keep asserting an order the gateway says is not there",
        );
        assert!(
            shared.orders.get_order_info(42).is_none(),
            "and the cache row goes with it",
        );
        // The rejection itself is the report. A synthetic status update queued
        // here would reach the caller behind a fill that raced it, because both
        // dispatchers drain fills ahead of order updates.
        assert!(shared.orders.drain_order_updates().is_empty());
        assert_eq!(shared.orders.drain_cancel_rejects().len(), 1);
    }

    /// A fill that raced the rejection is recoverable, on the terms the
    /// untracked-fill path sets (ibx#314): the execution has to carry its
    /// contract id, because nothing else says which instrument moved, and it
    /// must not be resend-marked, because a replayed execution for an order
    /// this session does not track is history rather than news. An execution
    /// that carries neither is dropped — the same as it was before this change
    /// for any order already removed from the book.
    #[test]
    fn an_execution_racing_an_unknown_order_rejection_still_books() {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        tracked_for_cancel(&mut context);

        ccp.handle_cancel_reject(&cancel_reject_frame("1"), &mut context, &shared, &None);
        let frame = exec_report_frame(&[
            (39, "1"), (17, "e-1"), (150, "F"), (32, "40"), (31, "100.0"), (151, "60"),
            (6008, "756733"), (38, "100"), (54, "1"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        assert_eq!(shared.orders.drain_fills().len(), 1, "the fill books");
        assert_eq!(context.position(0), 40.0, "and the position moves");
    }

    /// Only a stated UnknownOrder retires the order. Every other stated reason
    /// means it is still working and the cancel arrived at the wrong moment; an
    /// absent or unparseable tag 102 states nothing at all and is synthesized
    /// as -1, so it takes the same path rather than retiring on an absence.
    #[test]
    fn any_other_rejection_leaves_the_order_in_place() {
        for code in ["0", "2", "-1", ""] {
            let mut ccp = CcpState::new();
            let mut context = Context::new();
            let shared = SharedState::new();
            tracked_for_cancel(&mut context);

            ccp.handle_cancel_reject(&cancel_reject_frame(code), &mut context, &shared, &None);

            assert_eq!(
                context.order(42).expect("still tracked").status,
                crate::types::OrderStatus::Submitted,
                "reason {code:?} does not say the order is gone",
            );
        }
    }

    fn exec_report_frame(pairs: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
        let mut m = std::collections::HashMap::new();
        m.insert(11u32, "42".to_string()); // ClOrdID
        for (tag, val) in pairs {
            m.insert(*tag, val.to_string());
        }
        m
    }

    #[test]
    fn ord_status_new_unrouted_is_presubmitted() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        // 39=0, no ExDestination, exec ref "NONE" — waiting, not yet routed.
        let frame = exec_report_frame(&[(39, "0"), (150, "0"), (198, "NONE")]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        assert_eq!(context.order(42).unwrap().status,
            crate::types::OrderStatus::PreSubmitted);
    }

    #[test]
    fn ord_status_new_routed_is_submitted() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        // 39=0 with the order routed to ARCA — working.
        let frame = exec_report_frame(&[(39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1")]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");
        assert_eq!(context.order(42).unwrap().status,
            crate::types::OrderStatus::Submitted);
    }

    #[test]
    fn ord_status_presubmitted_then_routed_advances_to_submitted() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let waiting = exec_report_frame(&[(39, "0"), (150, "0"), (198, "NONE")]);
        ccp.handle_exec_report(&waiting, &mut context, &shared, &None, "");
        assert_eq!(context.order(42).unwrap().status,
            crate::types::OrderStatus::PreSubmitted);
        let routed = exec_report_frame(&[(39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1")]);
        ccp.handle_exec_report(&routed, &mut context, &shared, &None, "");
        assert_eq!(context.order(42).unwrap().status,
            crate::types::OrderStatus::Submitted);
    }

    // ibx#250: 39=I (Inactive) and 39=8 (Rejected) both stringify to
    // "Inactive" downstream (client_core::order_status_str), but must not be
    // treated the same here. A parked (39=I) order's reason is queued for
    // delivery through Wrapper::error, and its completed_status stays empty
    // (it is not completed and may reactivate). A rejected order's reason
    // stays on the order snapshot, and nothing is queued for it — the
    // engine still holds the order at this point, so context still knows it
    // as Inactive/reactivatable while a Rejected order is retired below.
    /// The report that fills an order states its new status on the same
    /// report. Announcing the execution and withholding the status left a
    /// caller watching order status believing the order was still working,
    /// which is the one thing it most needed not to believe.
    #[test]
    fn a_report_that_fills_an_order_also_says_the_order_is_filled() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        // 39=2 filled, 150=F the execution, with a quantity and a price on it.
        let frame = exec_report_frame(&[
            (39, "2"), (150, "F"), (32, "100"), (31, "150.00"), (14, "100"), (151, "0"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &Some(tx), "");

        let events: Vec<_> = rx.try_iter().collect();
        assert!(
            events.iter().any(|e| matches!(e, Event::Fill(_))),
            "the execution is reported: {events:?}",
        );
        assert!(
            events.iter().any(|e| matches!(
                e, Event::OrderUpdate(u) if u.status == crate::types::OrderStatus::Filled
            )),
            "and so is the status it left the order in: {events:?}",
        );
    }

    #[test]
    fn ord_status_inactive_reason_reaches_inactive_queue() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "I"), (150, "0"),
            (58, "Order held pending margin check"), (103, "0"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        assert_eq!(context.order(42).unwrap().status, crate::types::OrderStatus::Inactive);

        let inactive = shared.orders.drain_order_inactive();
        assert_eq!(inactive.len(), 1);
        assert_eq!(inactive[0].0, 42);
        assert_eq!(inactive[0].2, "Order held pending margin check (reason code 0)");

        let info = shared.orders.get_order_info(42).unwrap();
        assert!(info.order_state.completed_status.is_empty());
    }

    #[test]
    fn a_refused_order_tells_the_caller_why() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "8"), (150, "0"),
            (58, "No valid bid/ask"), (103, "1"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        // Rejected is terminal — the engine retires the order.
        assert!(context.order(42).is_none());

        // The venue said why. A caller that has to read a log to find out is a
        // caller that cannot act on it, so the reason goes out on the channel a
        // refusal is reported on.
        let reported = shared.orders.drain_order_inactive();
        assert_eq!(reported.len(), 1, "the refusal reaches the caller: {reported:?}");
        assert_eq!(reported[0].0, 42);
        assert!(reported[0].2.contains("No valid bid/ask"), "and says why: {reported:?}");

        // It stays on the order's own record too, which is where a caller that
        // asks after the fact looks.
        let info = shared.orders.get_order_info(42).unwrap();
        assert_eq!(info.order_state.completed_status, "No valid bid/ask");
    }

    /// `completed_status` carries the reject text alone, so a caller reading it
    /// cannot tell a venue refusing an order type from a malformed request when
    /// the text is generic. The reason code (tag 103) is what separates them.
    #[test]
    fn ord_status_rejected_records_the_reason_with_its_code() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = exec_report_frame(&[
            (39, "8"), (150, "0"),
            (58, "No valid bid/ask"), (103, "1"),
        ]);
        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let info = shared.orders.get_order_info(42).unwrap();
        assert_eq!(info.order_state.reject_reason, "No valid bid/ask (reason code 1)");
    }

    // ibx#238 / ib-agent#172: in the UP portfolio snapshot the average cost is
    // tag 6101 and 6065 is the market price. The handler previously read 6065 as
    // the average cost. Verify the mapping and that all marks are stored.
    #[test]
    fn position_update_maps_marks_and_avg_cost_from_correct_tags() {
        let mut context = Context::new();
        let shared = SharedState::new();
        let mut m = std::collections::HashMap::new();
        m.insert(6008u32, "756733".to_string());   // conId
        m.insert(6064u32, "10".to_string());        // position
        m.insert(6101u32, "100.50".to_string());    // averageCost
        m.insert(6065u32, "110.25".to_string());    // marketPrice
        m.insert(6067u32, "1102.50".to_string());   // marketValue
        m.insert(6100u32, "97.50".to_string());     // unrealizedPNL
        m.insert(6099u32, "5.00".to_string());      // realizedPNL
        handle_position_update(&m, &mut context, &shared, &None);

        let pi = shared.portfolio.position_info(756733).expect("position stored");
        assert_eq!(pi.position, 10.0);
        assert_eq!(pi.avg_cost, (100.50 * PRICE_SCALE as f64) as Price);
        assert_eq!(pi.market_price, (110.25 * PRICE_SCALE as f64) as Price);
        assert_eq!(pi.market_value, (1102.50 * PRICE_SCALE as f64) as Price);
        assert_eq!(pi.unrealized_pnl, (97.50 * PRICE_SCALE as f64) as Price);
        assert_eq!(pi.realized_pnl, (5.00 * PRICE_SCALE as f64) as Price);
    }

    // The lean position feed carries no marks; it must not zero the marks the
    // portfolio snapshot set (ibx#238).
    #[test]
    fn lean_position_feed_does_not_clobber_marks() {
        let shared = SharedState::new();
        shared.portfolio.set_position_info(PositionInfo {
            con_id: 1, position: 10.0, avg_cost: 100 * PRICE_SCALE, ..Default::default()
        });
        shared.portfolio.set_position_marks(1, 110 * PRICE_SCALE, 1100 * PRICE_SCALE, 100 * PRICE_SCALE, 5 * PRICE_SCALE);
        // Lean feed updates position + avg_cost only.
        shared.portfolio.set_position_info(PositionInfo {
            con_id: 1, position: 12.0, avg_cost: 101 * PRICE_SCALE, ..Default::default()
        });
        let pi = shared.portfolio.position_info(1).unwrap();
        assert_eq!(pi.position, 12.0);
        assert_eq!(pi.avg_cost, 101 * PRICE_SCALE);
        assert_eq!(pi.market_price, 110 * PRICE_SCALE, "marks survive the lean feed");
        assert_eq!(pi.market_value, 1100 * PRICE_SCALE);
        assert_eq!(pi.unrealized_pnl, 100 * PRICE_SCALE);
    }

    // ibx#220: the TIF decoder must be the exact inverse of the outbound
    // encoder. The old map decoded '7' (never emitted) as OPG and dropped
    // OPG and AUC to "".
    #[test]
    fn tif_round_trips_through_encoder_and_decoder() {
        for tif in ["DAY", "GTC", "OPG", "IOC", "FOK", "GTD", "AUC"] {
            let order = api::Order { tif: tif.to_string(), ..Default::default() };
            assert_eq!(decode_tif(order.tif_byte()), tif,
                "TIF {tif} must survive encode->decode");
        }
        // DTC shares the GTD wire byte and decodes as GTD.
        let dtc = api::Order { tif: "DTC".to_string(), ..Default::default() };
        assert_eq!(decode_tif(dtc.tif_byte()), "GTD");
        // Unknown bytes decode to empty, not a wrong TIF.
        assert_eq!(decode_tif(b'7'), "");
    }

    // ── ibx#227: contract-details deadline sweep ──

    #[test]
    fn sweep_times_out_pending_secdef_with_error_and_end() {
        let mut ccp = CcpState::new();
        let shared = SharedState::new();
        let past = Instant::now() - std::time::Duration::from_secs(1);
        ccp.pending_secdef.push((7, true, past));

        ccp.sweep_contract_details(&shared, &None);

        assert!(ccp.pending_secdef.is_empty(), "expired entry must be reclaimed");
        let errors = shared.reference.drain_historical_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, 7);
        assert_eq!(errors[0].1, 200);
        assert_eq!(shared.reference.drain_contract_details_end(), vec![7],
            "end must fire so a blocked wait unblocks");
    }

    #[test]
    fn sweep_drops_internal_secdef_silently() {
        let mut ccp = CcpState::new();
        let shared = SharedState::new();
        let past = Instant::now() - std::time::Duration::from_secs(1);
        // Internal sentinel (cache auto-fetch): no user is waiting on it.
        ccp.pending_secdef.push((0xF000_0001, true, past));

        ccp.sweep_contract_details(&shared, &None);

        assert!(ccp.pending_secdef.is_empty());
        assert!(shared.reference.drain_historical_errors().is_empty());
        assert!(shared.reference.drain_contract_details_end().is_empty());
    }

    #[test]
    fn sweep_times_out_incomplete_fanout() {
        let mut ccp = CcpState::new();
        let shared = SharedState::new();
        ccp.pending_fanout.push(PendingFanout {
            api_req_id: 9,
            fanout_req_ids: (0..27).map(|i| format!("ibxfan-9-{i}")).collect(),
            received: 26, // one reply lost — previously hung forever
            deadline: Instant::now() - std::time::Duration::from_secs(1),
        });

        ccp.sweep_contract_details(&shared, &None);

        assert!(ccp.pending_fanout.is_empty());
        let errors = shared.reference.drain_historical_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, 9);
        assert_eq!(errors[0].1, 200);
        assert_eq!(shared.reference.drain_contract_details_end(), vec![9]);
    }

    // ── ibx#229: a con_id=0 secdef reply is "not found", not a contract ──

    /// The gateway's "no security definition" answer: a `35=d` echoing the
    /// request id and carrying con_id 0 — no symbol, no price-increment block.
    fn secdef_not_found(req_id: &str) -> Vec<u8> {
        crate::protocol::fix::fix_build(&[
            (fix::TAG_MSG_TYPE, "d"),
            (crate::control::contracts::TAG_SECURITY_REQ_ID, req_id),
            (crate::control::contracts::TAG_SECURITY_RESPONSE_TYPE, "4"),
            (crate::control::contracts::TAG_IB_CON_ID, "0"),
        ], 1)
    }

    #[test]
    fn secdef_not_found_by_symbol_is_an_error_not_a_row() {
        let (mut ccp, mut context, shared) = u186_test_state();
        ccp.pending_secdef.push((7, false, Instant::now() + SECDEF_TIMEOUT));

        ccp.process_ccp_message(&secdef_not_found("7"), &mut None, &mut context, &shared,
            &None, &mut HeartbeatState::new(), "DU1");

        assert!(shared.reference.drain_contract_details().is_empty(),
            "con_id=0 is the gateway saying 'no definition' — emitting it as a row \
             hands the caller a fabricated min_tick that reads like a hit");
        let errors = shared.reference.drain_historical_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, 7);
        assert_eq!(errors[0].1, 200);
        assert_eq!(shared.reference.drain_contract_details_end(), vec![7],
            "end must still fire so a blocked wait unblocks");
    }

    #[test]
    fn secdef_not_found_by_conid_errors_and_ends() {
        let (mut ccp, mut context, shared) = u186_test_state();
        // Known-conId lookup: single record, is_last regardless of the wire flag.
        ccp.pending_secdef.push((7, true, Instant::now() + SECDEF_TIMEOUT));

        ccp.process_ccp_message(&secdef_not_found("7"), &mut None, &mut context, &shared,
            &None, &mut HeartbeatState::new(), "DU1");

        assert!(shared.reference.drain_contract_details().is_empty());
        let errors = shared.reference.drain_historical_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, 7);
        assert_eq!(errors[0].1, 200);
        assert_eq!(shared.reference.drain_contract_details_end(), vec![7]);
        assert!(ccp.pending_secdef.is_empty(), "the request is finished");
    }

    #[test]
    fn secdef_not_found_stays_silent_for_an_internal_fetch() {
        let (mut ccp, mut context, shared) = u186_test_state();
        // Cache auto-fetch sentinel: no user is waiting on it.
        ccp.pending_secdef.push((0xF000_0001, true, Instant::now() + SECDEF_TIMEOUT));

        ccp.process_ccp_message(&secdef_not_found("4026531841"), &mut None, &mut context,
            &shared, &None, &mut HeartbeatState::new(), "DU1");

        assert!(shared.reference.drain_contract_details().is_empty());
        assert!(shared.reference.drain_historical_errors().is_empty());
        assert!(shared.reference.drain_contract_details_end().is_empty());
    }

    #[test]
    fn a_fanout_reply_without_a_con_id_is_not_a_row() {
        let (mut ccp, mut context, shared) = u186_test_state();
        ccp.pending_fanout.push(PendingFanout {
            api_req_id: 9,
            fanout_req_ids: vec!["ibxfan-9-0".to_string()],
            received: 0,
            deadline: Instant::now() + SECDEF_TIMEOUT,
        });

        ccp.process_ccp_message(&secdef_not_found("ibxfan-9-0"), &mut None, &mut context,
            &shared, &None, &mut HeartbeatState::new(), "DU1");

        assert!(shared.reference.drain_contract_details().is_empty(),
            "a per-exchange leg with no con_id is not a contract either");
        assert_eq!(shared.reference.drain_contract_details_end(), vec![9],
            "the fan-out still completes");
        assert!(ccp.pending_fanout.is_empty());
    }

    // ── ibx#228: matching-symbols attribution ──

    fn matching_symbols_msg(req_id: &str, symbols: &[(&str, &str)]) -> Vec<u8> {
        let count = symbols.len().to_string();
        let mut fields: Vec<(u32, &str)> = vec![
            (crate::protocol::fix::TAG_MSG_TYPE, "U"),
            (6040, "186"),
            (320, req_id),
            (146, &count), // match count — marks a data frame (even when 0)
        ];
        for (sym, con_id) in symbols {
            fields.push((55, sym));
            fields.push((167, "CS"));
            fields.push((15, "USD"));
            fields.push((6008, con_id));
        }
        crate::protocol::fix::fix_build(&fields, 1)
    }

    /// A 186 frame with no match-count tag: the not-ready ack that precedes
    /// the data frame.
    fn matching_symbols_ack(req_id: &str) -> Vec<u8> {
        crate::protocol::fix::fix_build(&[
            (crate::protocol::fix::TAG_MSG_TYPE, "U"),
            (6040, "186"),
            (320, req_id),
        ], 1)
    }

    fn u186_test_state() -> (CcpState, Context, SharedState) {
        (CcpState::new(), Context::new(), SharedState::new())
    }

    #[test]
    fn matching_symbols_matched_by_echoed_req_id_not_fifo() {
        let (mut ccp, mut context, shared) = u186_test_state();
        ccp.pending_matching_symbols.push((1, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));
        ccp.pending_matching_symbols.push((2, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

        // Request 2's reply arrives FIRST (out of order).
        let msg = matching_symbols_msg("2", &[("AAPL", "265598")]);
        ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

        let delivered = shared.reference.drain_matching_symbols();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, 2, "reply must land on the echoed req_id, not the queue head");
        assert_eq!(delivered[0].1.len(), 1);
        assert_eq!(ccp.pending_matching_symbols.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn matching_symbols_empty_result_pops_and_delivers() {
        let (mut ccp, mut context, shared) = u186_test_state();
        ccp.pending_matching_symbols.push((1, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));
        ccp.pending_matching_symbols.push((2, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

        // Unknown pattern: zero matches. Must still pop req 1 and deliver
        // the empty answer — previously this poisoned the queue head and
        // every later reply was off by one, forever (ibx#228).
        let msg = matching_symbols_msg("1", &[]);
        ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

        let delivered = shared.reference.drain_matching_symbols();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, 1);
        assert!(delivered[0].1.is_empty(), "empty result is a legitimate answer");
        assert_eq!(ccp.pending_matching_symbols.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![2],
            "queue must not be poisoned by an empty result");

        // The next reply attributes correctly.
        let msg = matching_symbols_msg("2", &[("MSFT", "272093")]);
        ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");
        let delivered = shared.reference.drain_matching_symbols();
        assert_eq!(delivered[0].0, 2);
        assert!(ccp.pending_matching_symbols.is_empty());
    }

    #[test]
    fn matching_symbols_ack_frame_does_not_consume_the_request() {
        let (mut ccp, mut context, shared) = u186_test_state();
        ccp.pending_matching_symbols.push((1, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

        // The not-ready ack (no tag 146) arrives first — it must not pop the
        // request; delivering it as an empty answer orphans the data frame
        // that follows (observed live, ibx#228).
        let msg = matching_symbols_ack("1");
        ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");
        assert!(shared.reference.drain_matching_symbols().is_empty());
        assert_eq!(ccp.pending_matching_symbols.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![1]);

        // The data frame then delivers.
        let msg = matching_symbols_msg("1", &[("AAPL", "265598")]);
        ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");
        let delivered = shared.reference.drain_matching_symbols();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, 1);
        assert_eq!(delivered[0].1.len(), 1);
        assert!(ccp.pending_matching_symbols.is_empty());
    }

    #[test]
    fn matching_symbols_unattributable_reply_is_dropped_not_misattributed() {
        let (mut ccp, mut context, shared) = u186_test_state();
        ccp.pending_matching_symbols.push((1, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));
        ccp.pending_matching_symbols.push((2, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

        // Echoed id matches nothing pending: with two in flight, guessing
        // would cross-attribute — drop with a warn instead.
        let msg = matching_symbols_msg("99", &[("AAPL", "265598")]);
        ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

        assert!(shared.reference.drain_matching_symbols().is_empty());
        assert_eq!(ccp.pending_matching_symbols.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn sweep_spares_live_entries() {
        let mut ccp = CcpState::new();
        let shared = SharedState::new();
        let future = Instant::now() + SECDEF_TIMEOUT;
        ccp.pending_secdef.push((7, true, future));
        ccp.pending_fanout.push(PendingFanout {
            api_req_id: 9,
            fanout_req_ids: vec!["ibxfan-9-0".to_string()],
            received: 0,
            deadline: future,
        });

        ccp.sweep_contract_details(&shared, &None);

        assert_eq!(ccp.pending_secdef.len(), 1);
        assert_eq!(ccp.pending_fanout.len(), 1);
        assert!(shared.reference.drain_historical_errors().is_empty());
        assert!(shared.reference.drain_contract_details_end().is_empty());
    }

    /// A fill whose ClOrdID this session never tracked. Every field the engine
    /// needs to book it is on the report itself.
    fn untracked_fill(pairs: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
        let mut m = std::collections::HashMap::new();
        for (tag, val) in [
            (11u32, "99"),      // ClOrdID the context does not know
            (150, "2"),         // ExecType: trade
            (39, "2"),          // OrdStatus: filled
            (32, "5"),          // LastShares
            (31, "100.00"),     // LastPx
            (54, "1"),          // Side: buy
            (6008, "888888"),   // ContractID
            (55, "ZZZ"),
            (17, "EXEC-1"),
        ] {
            m.insert(tag, val.to_string());
        }
        for (tag, val) in pairs {
            if val.is_empty() {
                m.remove(tag);
            } else {
                m.insert(*tag, val.to_string());
            }
        }
        m
    }

    /// A fill for an order this session does not track is still a position the
    /// account holds. Dropping it leaves the engine short of the truth with
    /// nothing to say so — the cancel/fill race reaches this every time.
    #[test]
    fn a_fill_for_an_untracked_order_is_still_booked() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = untracked_fill(&[]);

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let fills = shared.orders.drain_fills();
        assert_eq!(fills.len(), 1, "the fill must be reported");
        assert_eq!(fills[0].qty, 5);
        assert_eq!(fills[0].order_id, 99);
        assert_eq!(fills[0].side, Side::Buy);
        assert_eq!(
            context.position(fills[0].instrument), 5.0,
            "the position must move by the filled quantity",
        );
    }

    /// A sell books the other way. Taking the side from the report rather than
    /// defaulting is the whole point: the wrong sign is worse than no fill.
    #[test]
    fn an_untracked_sell_moves_the_position_down() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = untracked_fill(&[(54, "2")]);

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let fills = shared.orders.drain_fills();
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].side, Side::Sell);
        assert_eq!(context.position(fills[0].instrument), -5.0);
    }

    /// Without a contract or a side there is nothing to book against, and
    /// guessing either one would move a real position the wrong way.
    #[test]
    fn an_untracked_fill_is_not_booked_on_a_guess() {
        for missing in [6008u32, 54] {
            let (mut ccp, mut context, shared) = ord_status_test_state();
            let frame = untracked_fill(&[(missing, "")]);

            ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

            assert!(
                shared.orders.drain_fills().is_empty(),
                "tag {missing} missing: must not book a guessed fill",
            );
        }
    }

    /// On a fresh process the gateway resends prior executions with 97=Y and
    /// their original ExecIDs, for orders no session tracks. Booking those
    /// builds a position out of history on top of the one the position feed
    /// already reports.
    #[test]
    fn a_replayed_execution_is_not_booked_as_a_new_position() {
        for (tag, name) in [(97u32, "PossResend"), (43, "PossDupFlag")] {
            let (mut ccp, mut context, shared) = ord_status_test_state();

            ccp.handle_exec_report(&untracked_fill(&[(tag, "Y")]), &mut context, &shared, &None, "");

            assert!(
                shared.orders.drain_fills().is_empty(),
                "{name}=Y restates history and must not move the position",
            );
        }

        // The same report without the marker is booked, so the guard is the
        // marker and not something else about the frame.
        let (mut ccp, mut context, shared) = ord_status_test_state();
        ccp.handle_exec_report(&untracked_fill(&[(97, "N")]), &mut context, &shared, &None, "");
        assert_eq!(shared.orders.drain_fills().len(), 1);
    }

    /// An execution that could not be booked must stay replayable. Consuming
    /// the ExecID for a fill that was dropped makes the loss permanent: the
    /// replay after a reconnect is then rejected as a duplicate.
    #[test]
    fn an_unbookable_fill_does_not_consume_its_exec_id() {
        let (mut ccp, mut context, shared) = ord_status_test_state();

        // Same execution, first seen without the contract that would let the
        // engine place it.
        ccp.handle_exec_report(&untracked_fill(&[(6008, "")]), &mut context, &shared, &None, "");
        assert!(shared.orders.drain_fills().is_empty());

        // Replayed in full — it must not be rejected as already seen.
        ccp.handle_exec_report(&untracked_fill(&[]), &mut context, &shared, &None, "");
        assert_eq!(
            shared.orders.drain_fills().len(), 1,
            "the replay must be booked, not dropped as a duplicate",
        );
    }

    /// The cumulative pair has to come off the wire. Tag 14 is the order's
    /// filled total and tag 6 its volume-weighted average; 32 and 31 describe
    /// only the print that triggered the report.
    #[test]
    fn the_fill_carries_the_orders_totals_not_the_prints() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        // Second print of 5 at 101, taking the order to 12 filled at 100.50.
        let frame = untracked_fill(&[
            (32, "5"), (31, "101.00"), (14, "12"), (6, "100.50"), (151, "3"), (39, "1"),
        ]);

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let fills = shared.orders.drain_fills();
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].qty, 5, "qty stays the print");
        assert_eq!(fills[0].price, 101 * PRICE_SCALE, "price stays the print");
        assert_eq!(fills[0].cum_qty, 12, "cum_qty is the order total from tag 14");
        assert_eq!(
            fills[0].avg_price, 100 * PRICE_SCALE + PRICE_SCALE / 2,
            "avg_price is the volume-weighted average from tag 6",
        );
    }

    /// Without tag 14 the print alone is not a substitute: on a later fill it
    /// is smaller than what was already reported, so `filled` would go
    /// backwards. The order's own accumulated quantity carries it instead.
    #[test]
    fn a_missing_cumulative_quantity_does_not_walk_backwards() {
        let (mut ccp, mut context, shared) = ord_status_test_state();

        // Seven filled so far, stated.
        ccp.handle_exec_report(
            &exec_report_frame(&[
                (150, "2"), (39, "1"), (32, "7"), (31, "100.00"), (14, "7"), (6, "100.00"),
                (151, "3"), (17, "E1"),
            ]),
            &mut context, &shared, &None, "",
        );
        let first = shared.orders.drain_fills();
        assert_eq!(first[0].cum_qty, 7);

        // One more, with the cumulative fields absent.
        ccp.handle_exec_report(
            &exec_report_frame(&[
                (150, "2"), (39, "1"), (32, "1"), (31, "101.00"), (151, "2"), (17, "E2"),
            ]),
            &mut context, &shared, &None, "",
        );
        let second = shared.orders.drain_fills();
        assert_eq!(
            second[0].cum_qty, 8,
            "the order's own total carries it, rather than dropping back to the print",
        );
    }

    /// A negative average price is a real value for a spread quoted as a net
    /// credit, so only an absent or unparseable tag falls back.
    #[test]
    fn a_negative_average_price_is_not_treated_as_absent() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = untracked_fill(&[(32, "5"), (31, "-2.00"), (14, "5"), (6, "-1.50")]);

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let fills = shared.orders.drain_fills();
        assert_eq!(fills[0].avg_price, -(PRICE_SCALE + PRICE_SCALE / 2), "-1.50 is kept");
    }

    /// With no order to accumulate against and no tags, the print is all there
    /// is — which is what the callback reported before.
    #[test]
    fn the_fill_falls_back_to_the_print_when_the_totals_are_absent() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = untracked_fill(&[(32, "5"), (31, "101.00"), (14, ""), (6, "")]);

        ccp.handle_exec_report(&frame, &mut context, &shared, &None, "");

        let fills = shared.orders.drain_fills();
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].cum_qty, 5);
        assert_eq!(fills[0].avg_price, 101 * PRICE_SCALE);
    }

    /// The side mapping is the whole sign of the position delta, so every arm
    /// is pinned — a short sale booked as an ordinary sell is the same
    /// direction, but a buy booked as a sell is twice the fill in the wrong one.
    #[test]
    fn every_side_maps_to_the_right_position_delta() {
        for (tag54, expected_side, expected_delta) in [
            ("1", Side::Buy, 5),
            ("2", Side::Sell, -5),
            ("5", Side::ShortSell, -5),
        ] {
            let (mut ccp, mut context, shared) = ord_status_test_state();
            ccp.handle_exec_report(
                &untracked_fill(&[(54, tag54)]), &mut context, &shared, &None, "",
            );
            let fills = shared.orders.drain_fills();
            assert_eq!(fills.len(), 1, "Side={tag54} books");
            assert_eq!(fills[0].side, expected_side, "Side={tag54}");
            assert_eq!(
                context.position(fills[0].instrument), expected_delta as f64,
                "Side={tag54} moves the position {expected_delta}",
            );
        }
    }

    /// Deduplication exists to stop a fill being counted twice. Returning out
    /// of the whole handler also skips the status and the terminal bookkeeping,
    /// so a replayed final fill leaves the order in `open_orders` for good and
    /// `req_open_orders` keeps reporting a filled order as working.
    #[test]
    fn a_duplicate_exec_id_suppresses_the_fill_and_nothing_else() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
        let event_tx = Some(event_tx);

        // Partial fill, booked normally.
        ccp.handle_exec_report(
            &exec_report_frame(&[
                (150, "2"), (39, "1"), (32, "1"), (31, "100.00"), (151, "9"), (17, "DUP-1"),
            ]),
            &mut context, &shared, &event_tx, "",
        );
        assert_eq!(shared.orders.drain_fills().len(), 1, "the first delivery books");
        assert!(context.order(42).is_some(), "and the order is still working");

        // The same execution replayed, this time carrying the terminal status.
        ccp.handle_exec_report(
            &exec_report_frame(&[
                (150, "2"), (39, "2"), (32, "1"), (31, "100.00"), (151, "0"), (17, "DUP-1"),
            ]),
            &mut context, &shared, &event_tx, "",
        );

        assert!(
            shared.orders.drain_fills().is_empty(),
            "the fill is not counted twice",
        );
        let position_after = context.position(0);
        assert!(
            context.order(42).is_none(),
            "but the order still reaches its terminal state and is removed",
        );
        let completed = shared.orders.drain_completed_orders();
        assert_eq!(completed.len(), 1, "and is reported completed");
        assert_eq!(completed[0].order_id, 42);
        assert_eq!(completed[0].status, crate::types::OrderStatus::Filled);

        // The terminal status still reaches the application. Treating the
        // duplicate as though it had booked a fill would swallow it, since the
        // status notification is suppressed when a fill was reported instead.
        let updates = shared.orders.drain_order_updates();
        assert_eq!(updates.len(), 1, "exactly one status notification, not none and not two");
        assert_eq!(updates[0].order_id, 42);
        assert_eq!(updates[0].status, crate::types::OrderStatus::Filled);

        // The position is what deduplication exists to protect. One share was
        // filled; the replay must not make it two.
        assert_eq!(position_after, 1.0, "the duplicate must not move the position again");
        assert_eq!(updates[0].filled_qty, 1.0, "nor inflate the filled quantity");
        assert_eq!(completed[0].filled_qty, 1);

        // The event channel is a second delivery path for the same fill, and
        // every other test here passes None for it, so it is checked once.
        let events: Vec<_> = event_rx.try_iter().collect();
        assert_eq!(
            events.iter().filter(|e| matches!(e, Event::Fill(_))).count(), 1,
            "exactly one Fill reaches the channel across both deliveries: {events:?}",
        );
    }

    /// A late duplicate of an earlier partial must not put a finished order
    /// back on the open list. The cache is what `req_open_orders` reads.
    #[test]
    fn a_late_partial_does_not_reopen_a_completed_order() {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let partial = |exec: &str| exec_report_frame(&[
            (150, "2"), (39, "1"), (32, "1"), (31, "100.00"), (151, "9"), (17, exec),
            (6008, "756733"), (55, "SPY"),
        ]);

        ccp.handle_exec_report(&partial("E1"), &mut context, &shared, &None, "");
        ccp.handle_exec_report(
            &exec_report_frame(&[
                (150, "2"), (39, "2"), (32, "9"), (31, "100.00"), (151, "0"), (17, "E2"),
                (6008, "756733"), (55, "SPY"),
            ]),
            &mut context, &shared, &None, "",
        );
        let terminal = shared.orders.get_order_info(42).map(|i| i.order_state.status.clone());

        // The earlier partial arrives again.
        ccp.handle_exec_report(&partial("E1"), &mut context, &shared, &None, "");

        assert_eq!(
            shared.orders.get_order_info(42).map(|i| i.order_state.status.clone()),
            terminal,
            "the completed order stays completed",
        );
    }

    // A fill with no ExecID is deduped on its content instead, and the key
    // includes CumQty, which advances with every execution on an order. Two
    // real fills therefore never collide. The case that asserted the opposite
    // sent one frame twice with no CumQty tag at all, so both read as zero: a
    // shape the gateway does not produce, and treating it as two fills would
    // give back the replay double-booking the content key exists to stop.

    /// The gateway's answer to a symbol it cannot resolve: a `35=d` echoing
    /// the request id with no contract fields (live: "BRK.A").
    fn secdef_no_match(req_id: &str, response_type: &str) -> Vec<u8> {
        crate::protocol::fix::fix_build(&[
            (crate::protocol::fix::TAG_MSG_TYPE, "d"),
            (crate::control::contracts::TAG_SECURITY_REQ_ID, req_id),
            (crate::control::contracts::TAG_SECURITY_RESPONSE_TYPE, response_type),
        ], 1)
    }

    #[test]
    fn secdef_no_match_reports_error_200_not_a_zeroed_row() {
        let (mut ccp, mut context, shared) = u186_test_state();
        // By-symbol lookup: not single-shot.
        ccp.pending_secdef.push((1005, false, Instant::now() + SECDEF_TIMEOUT));

        let msg = secdef_no_match("1005", "6");
        ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

        assert!(shared.reference.drain_contract_details().is_empty(),
            "a contract-less reply must not surface as a ContractDetails row");
        let errors = shared.reference.drain_historical_errors();
        assert_eq!(errors.len(), 1, "the caller must be told the symbol did not resolve");
        assert_eq!(errors[0].0, 1005);
        assert_eq!(errors[0].1, 200);
        assert_eq!(shared.reference.drain_contract_details_end(), vec![1005]);
        assert!(ccp.pending_secdef.is_empty(), "the request must not outlive its answer");
    }

    /// Same reply without the 323 terminator: the by-symbol path reached end
    /// through the fan-out branch instead, and must not fire end twice.
    #[test]
    fn secdef_no_match_without_terminator_ends_once() {
        let (mut ccp, mut context, shared) = u186_test_state();
        ccp.pending_secdef.push((1005, false, Instant::now() + SECDEF_TIMEOUT));

        let msg = secdef_no_match("1005", "4");
        ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

        assert!(shared.reference.drain_contract_details().is_empty());
        assert_eq!(shared.reference.drain_historical_errors().len(), 1);
        assert_eq!(shared.reference.drain_contract_details_end(), vec![1005]);
        assert!(ccp.pending_secdef.is_empty());
    }

    #[test]
    fn secdef_no_match_on_internal_req_id_stays_silent() {
        let (mut ccp, mut context, shared) = u186_test_state();
        ccp.pending_secdef.push((0xF000_0001, true, Instant::now() + SECDEF_TIMEOUT));

        let msg = secdef_no_match("4026531841", "6"); // 0xF0000001
        ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

        assert!(shared.reference.drain_contract_details().is_empty());
        assert!(shared.reference.drain_historical_errors().is_empty());
        assert!(shared.reference.drain_contract_details_end().is_empty());
        assert!(ccp.pending_secdef.is_empty());
    }
}
