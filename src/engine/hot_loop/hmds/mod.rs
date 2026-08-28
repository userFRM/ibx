use std::time::Instant;

use crate::bridge::{Event, SharedState};
use crate::protocol::datetime::chrono_free_timestamp;
use crate::protocol::connection::{Connection, Frame};
use crate::protocol::fix;
use crate::protocol::fixcomp;
use crate::types::{InstrumentId, TbtType};

use super::{HeartbeatState, emit, clone_for_event, find_body_after_tag, extract_raw_tag, EventSink};

/// One tick-by-tick stream, and everything that is true of it alone.
///
/// Held per subscription rather than per contract. Two streams of different
/// kinds on one contract — every trade, and every quote change — are two
/// subscriptions with two of the venue's numbers, two running prices and two
/// record layouts. Kept per contract, the second overwrote the first's number
/// and its frames were then read under the first's layout: an every-trade
/// stream delivered quotes, and a busy listing's trades came back with a size
/// of nine hundred million and an exchange of three letters that were not one.
pub(crate) struct TbtSubscription {
    pub(crate) instrument: InstrumentId,
    /// What this client called the subscription, which the acknowledgement
    /// echoes.
    pub(crate) query_id: String,
    /// Which of the streams it is, and so which layout its records have.
    pub(crate) kind: TbtType,
    /// What the caller numbered this request. Stamped on every record, since
    /// a contract can carry several streams and the contract alone does not
    /// say which one a record came from.
    pub(crate) caller_req_id: i64,
    /// The venue's number for it, stated on every frame.
    pub(crate) venue_id: u64,
    /// The increment its prices move in.
    pub(crate) min_tick: i64,
    /// The increment its sizes are counted in.
    pub(crate) size_tick: f64,
    /// Where its price has got to. A move is measured from here.
    pub(crate) running: crate::protocol::tbt_stream::RunningPrice,
}

pub(crate) struct HmdsState {
    pub(crate) next_tbt_req_id: u32,
    pub(crate) tbt_subscriptions: Vec<TbtSubscription>,
    /// Streams this session withdrew and the venue kept sending anyway, by the
    /// number the venue names them with. Held so the records that keep arriving
    /// are reported once as what they are rather than once each: a withdrawal
    /// this venue does not act on sends ticks for the rest of the session, and
    /// several hundred identical warnings bury whatever else is in the log.
    pub(crate) tbt_withdrawn: std::collections::HashSet<u64>,
    /// Streams already spoken about, so each is spoken about once.
    pub(crate) tbt_reported: std::collections::HashSet<u64>,
    pub(crate) next_hmds_query_id: u32,
    pub(crate) disconnected: bool,
    /// In-flight historical bar queries: the venue's id for each, and the
    /// request it answers. Waited on for as long as the venue takes, which is
    /// what the reference client does; one whose connection goes away is
    /// failed where that is stated.
    pub(crate) pending_historical: Vec<(String, u32)>,
    pub(crate) pending_head_ts: Vec<(String, u32)>,
    pub(crate) pending_scanner_params: bool,
    pub(crate) pending_scanner: Vec<(String, u32)>,
    pub(crate) next_scanner_id: u32,
    pub(crate) pending_news: Vec<(String, u32)>,
    pub(crate) pending_articles: Vec<(String, u32)>,
    pub(crate) pending_fundamental: Vec<(String, u32)>,
    pub(crate) pending_histogram: Vec<(String, u32)>,
    /// (query name, caller's request id, end date the request stated). The
    /// response carries no end date, so the requested one is reported back.
    pub(crate) pending_schedule: Vec<(String, u32, String)>,
    pub(crate) pending_ticks: Vec<(String, u32, String)>,
    /// (query name, caller's request id, the venue's ticker for the stream,
    /// the increment its prices move in, the increment its sizes move in).
    pub(crate) rtbar_subs: Vec<(String, u32, Option<u32>, f64, f64)>,
    /// req_ids that should keep streaming after initial batch (keepUpToDate=True).
    pub(crate) keep_up_to_date_reqs: std::collections::HashSet<u32>,
    /// The bars still forming, one per request keeping its bars up to date.
    pub(crate) forming_bars: Vec<FormingBar>,
    /// The live five-second bar streams, in the shape their request needs to
    /// go out again. `rtbar_subs` holds the routing for the session that is
    /// running and cannot rebuild a request, so a reconnect had nothing to
    /// send and the bars stopped for good.
    pub(crate) rtbar_resub: Vec<RtBarRequest>,
    /// Scanner results parked for contract-detail enrichment before dispatch.
    /// Drained by the engine top-level after each hmds.poll, then handed to
    /// `CcpState::start_scanner_enrichment`.
    pub(crate) cold_scanner_results: Vec<(u32, crate::control::scanner::ScannerResult)>,
}

/// Wire security type for a historical query. Empty falls back to the stock
/// encoding, which is what every caller got unconditionally before.
///
/// A type the enum does not list is passed through rather than emptied. The
/// enum covers the types the order path understands, and `to_fix` deliberately
/// blanks anything else so an unclassified instrument cannot masquerade as a
/// stock — but that reasoning is about the order path. A historical
/// query for a valid type the enum happens not to carry, `FOP` say, would
/// otherwise be narrowed to nothing. The descriptive branch of the subscribe
/// path passes such types through in the same way.
///
/// The value goes into a query document, so what passes through is restricted
/// to the shape a security type actually has. Anything else is blanked rather
/// than embedded: a caller-supplied `&` or `<` would otherwise produce a
/// malformed query instead of a refused one.
///
/// This helps a caller that states the type itself. It does not recover one the
/// client has already lost: a contract returned by `req_contract_details` for
/// an unlisted type arrives with an empty `sec_type`, because the enum cannot
/// carry it, and an empty type still means stock here. That round trip needs
/// the enum to stop discarding what it cannot name.
fn hist_sec_type(sec_type: &str) -> String {
    if sec_type.is_empty() {
        // The last resort, and a guess: it names a US stock, so anything else
        // reaching here is asked for as one. Every caller that names a
        // contract by id alone is answered by the venue before it gets this
        // far, so what arrives undescribed came in through the engine's own
        // request without one.
        log::warn!("a historical query states no security type; asking as a US stock");
        return "CS".to_string();
    }
    match crate::control::contracts::SecurityType::from_fix(sec_type).to_fix() {
        "" if is_plausible_sec_type(sec_type) => sec_type.to_string(),
        "" => String::new(),
        known => known.to_string(),
    }
}

/// Whether a string is shaped like a security type: short, and letters or
/// digits only. Deliberately strict — it guards a document, and every type the
/// gateway uses fits inside it.
fn is_plausible_sec_type(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 8
        && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Exchange for a historical query, defaulting to the previous constant.
fn hist_exchange(exchange: &str) -> String {
    if exchange.is_empty() {
        log::warn!("a historical query states no exchange; asking smart-routed");
        return "SMART".to_string();
    }
    // Under the name the venue routes by. A caller passing back a contract it
    // was handed carries the older spelling for a Nasdaq listing, which
    // reaches nothing.
    crate::control::contracts::exchange_to_fix(exchange).to_string()
}

/// Whether a trade belongs on the stream a caller asked for.
///
/// The venue serves one trade stream and marks the prints that were not
/// reported to the tape; the exchange's own trades are that stream without
/// them.
fn belongs_on(asked_for: TbtType, unreported: bool) -> bool {
    !(asked_for == TbtType::Last && unreported)
}

/// A bar still forming, folded from the five-second bars the venue streams.
///
/// The venue answers a request to keep bars up to date with the bars so far
/// and nothing after them: the query is closed as soon as it is answered, on
/// both connections it can be sent over. What it does keep sending is
/// five-second bars, which is what the bar still forming is made of.
#[derive(Clone)]
pub(crate) struct FormingBar {
    /// The caller's request, which its updates are delivered under.
    pub(crate) req_id: u32,
    /// How long the caller's bars are.
    pub(crate) seconds: u32,
    /// The moment the bar being formed opened.
    pub(crate) opened_at: u32,
    pub(crate) bar: crate::types::RealTimeBar,
    /// Volume-weighted price needs the weights kept as they arrive.
    pub(crate) weighted: f64,
}

impl FormingBar {
    /// Fold a five-second bar in, and answer with the bar as it now stands.
    fn fold(&mut self, five: &crate::types::RealTimeBar) -> crate::types::RealTimeBar {
        // Counted from the epoch, so a bar opens on a whole multiple of its
        // own length. Right to the clock for every size up to an hour; for a
        // day it is midnight UTC, which is the trading day of an instrument
        // that trades around the clock and the middle of the evening for one
        // that does not. Folding on the contract's own trading day needs the
        // schedule down here, which this does not have.
        let opened_at = five.timestamp - five.timestamp % self.seconds;
        if opened_at != self.opened_at {
            self.opened_at = opened_at;
            self.bar = *five;
            self.weighted = five.wap * five.volume;
            self.bar.timestamp = opened_at;
            return self.bar;
        }
        self.bar.high = self.bar.high.max(five.high);
        self.bar.low = if self.bar.low == 0.0 { five.low } else { self.bar.low.min(five.low) };
        self.bar.close = five.close;
        self.bar.volume += five.volume;
        self.bar.count += five.count;
        self.weighted += five.wap * five.volume;
        self.bar.wap = if self.bar.volume > 0.0 {
            self.weighted / self.bar.volume
        } else {
            five.wap
        };
        self.bar
    }
}

/// A live five-second bar stream, in the shape its request needs to go again.
#[derive(Clone)]
pub(crate) struct RtBarRequest {
    pub req_id: u32,
    pub con_id: i64,
    pub sec_type: String,
    pub exchange: String,
    pub what_to_show: String,
    pub use_rth: bool,
}

impl HmdsState {
    pub(crate) fn new() -> Self {
        Self {
            next_tbt_req_id: 1,
            tbt_subscriptions: Vec::new(),
            tbt_withdrawn: std::collections::HashSet::new(),
            tbt_reported: std::collections::HashSet::new(),
            next_hmds_query_id: 1000,
            disconnected: false,
            pending_historical: Vec::new(),
            pending_head_ts: Vec::new(),
            pending_scanner_params: false,
            pending_scanner: Vec::new(),
            next_scanner_id: 1,
            pending_news: Vec::new(),
            pending_articles: Vec::new(),
            pending_fundamental: Vec::new(),
            pending_histogram: Vec::new(),
            pending_schedule: Vec::new(),
            pending_ticks: Vec::new(),
            rtbar_subs: Vec::new(),
            keep_up_to_date_reqs: std::collections::HashSet::new(),
            forming_bars: Vec::new(),
            rtbar_resub: Vec::new(),
            cold_scanner_results: Vec::new(),
        }
    }

    /// Give up on the transport.
    ///
    /// The flag and the connection move together. Setting the flag alone left
    /// the dead socket in place, and the reconnect scheduler returns early
    /// while a connection is present — so the transport was stuck for the life
    /// of the process, on both the liveness timeout and the ordinary
    /// receive-error path.
    ///
    /// Unanswered one-shot requests are failed here. Streaming subscriptions
    /// are restored on reconnect; one-shot requests are not, and only
    /// historical bars carry a timeout, so the rest would never complete.
    pub(crate) fn disconnect(
        &mut self, hmds_conn: &mut Option<Connection>, shared: &SharedState,
        event_tx: &Option<crate::engine::hot_loop::EventSink>,
    ) {
        self.disconnected = true;
        *hmds_conn = None;
        self.fail_pending("the historical connection went away before the venue answered", shared);
        // The venue says when this connection breaks, and a caller waiting on
        // history has nothing else to read it from.
        crate::engine::hot_loop::emit(event_tx, crate::bridge::Event::VenueData {
            which: crate::bridge::VenueDataConnection::Historical,
            up: false,
        });
    }

    /// Report every unanswered one-shot request as failed, and forget it.
    fn fail_pending(&mut self, why: &str, shared: &SharedState) {
        let mut stranded: Vec<(u32, bool)> = Vec::new();
        // Bars are failed on the data channel as well as the error channel;
        // a caller blocked on the series needs the empty completion. Other
        // request kinds use the error channel alone.
        stranded.extend(self.pending_historical.drain(..).map(|(_, rid)| (rid, true)));
        stranded.extend(self.pending_head_ts.drain(..).map(|(_, rid)| (rid, false)));
        stranded.extend(self.pending_scanner.drain(..).map(|(_, rid)| (rid, false)));
        stranded.extend(self.pending_news.drain(..).map(|(_, rid)| (rid, false)));
        stranded.extend(self.pending_articles.drain(..).map(|(_, rid)| (rid, false)));
        stranded.extend(self.pending_fundamental.drain(..).map(|(_, rid)| (rid, false)));
        stranded.extend(self.pending_histogram.drain(..).map(|(_, rid)| (rid, false)));
        stranded.extend(self.pending_schedule.drain(..).map(|(_, rid, _)| (rid, false)));
        stranded.extend(self.pending_ticks.drain(..).map(|(_, rid, _)| (rid, false)));
        if stranded.is_empty() {
            return;
        }
        log::warn!(
            "{} historical request(s) were still unanswered when the connection went: {why}",
            stranded.len(),
        );
        for (req_id, from_historical) in stranded {
            super::push_hmds_error(shared, req_id, why.to_string(), from_historical);
        }
    }

    /// Take over a fresh socket and put the streams back on it.
    ///
    /// A reconnect that only replaced the transport left every tick-by-tick
    /// subscription behind on the dead one. Nothing said so — the socket was
    /// healthy, the heartbeats flowed — and the stream simply never resumed.
    pub(crate) fn reconnect(
        &mut self,
        conn: Connection,
        hmds_conn: &mut Option<Connection>,
        market: &crate::engine::market_state::MarketState,
        hb: &mut HeartbeatState,
    ) {
        *hmds_conn = Some(conn);
        self.disconnected = false;

        // The ids belong to the session that died; each subscription is sent
        // again and takes a new one. What was held against the old numbers goes
        // with them — a stream the last connection would not stop is a question
        // the new one answers for itself.
        self.tbt_withdrawn.clear();
        self.tbt_reported.clear();
        let stale = std::mem::take(&mut self.tbt_subscriptions);
        let wanted = stale.len();
        for dead in stale {
            let (instrument, tbt_type) = (dead.instrument, dead.kind);
            // Prices arrive as deltas against the last pair seen. The pair the
            // dead session left would have the new session's first delta added
            // to it, and every price after that would carry the error.
            match market.con_id(instrument) {
                Some(con_id) => {
                    let (stype, venue) = market.order_routing(instrument);
                    let mts = market.min_tick_scaled(instrument);
                    self.send_tbt_subscribe(
                        dead.caller_req_id, con_id, instrument, tbt_type, &stype, &venue,
                        mts, hmds_conn, hb,
                    )
                }
                None => log::warn!(
                    "HMDS reconnect: instrument {instrument} has no contract id, \
                     leaving its tick-by-tick stream unsubscribed",
                ),
            }
        }
        log::info!(
            "HMDS reconnected, re-subscribed {}/{} tick-by-tick streams",
            self.tbt_subscriptions.len(), wanted,
        );

        // Five-second bars are routed by a ticker id the dead session issued.
        // The routing goes with it and the requests are sent again.
        let bars: Vec<_> = self.rtbar_resub.clone();
        self.rtbar_subs.clear();
        self.rtbar_resub.clear();
        for r in &bars {
            self.send_realtime_bar_subscribe(
                r.req_id, r.con_id, "", &r.sec_type, &r.exchange,
                &r.what_to_show, r.use_rth, hmds_conn, hb,
            );
        }
        if !bars.is_empty() {
            log::info!("HMDS reconnected, re-subscribed {} real-time bar streams", bars.len());
        }
    }

    pub(crate) fn poll(
        &mut self,
        hmds_conn: &mut Option<Connection>,
        shared: &SharedState,
        event_tx: &Option<EventSink>,
        hb: &mut HeartbeatState,
    ) {
        if self.disconnected { return; }
        let messages = match hmds_conn.as_mut() {
            None => return,
            Some(conn) => {
                match conn.try_recv() {
                    Ok(0) if !conn.has_buffered_data() => return,
                    Ok(0) => {}
                    Err(e) => {
                        log::error!("HMDS connection lost: {e}");
                        self.disconnect(hmds_conn, shared, event_tx);
                        return;
                    }
                    Ok(n) => {
                        log::debug!("HMDS recv: {n} bytes");
                        hb.last_hmds_recv = Instant::now();
                        hb.pending_hmds_test = None;
                    }
                }
                let frames = conn.extract_frames();
                // Bytes arrived and nothing came out of them: either they are
                // stuck waiting for more, when some are still buffered, or
                // they were dropped outright, when none are. Both are worth
                // saying; a poll that produced frames is not.
                //
                // Said on every poll, this fired about once a second on a
                // healthy session, which is how a warning stops being read —
                // including the one beside it about a tick belonging to no
                // subscription.
                if frames.is_empty() {
                    log::warn!(
                        "HMDS poll: no frames from what arrived, {}B still buffered",
                        conn.buffered(),
                    );
                } else {
                    log::debug!(
                        "HMDS poll: extracted={} frames, buffered_after={}B",
                        frames.len(),
                        conn.buffered(),
                    );
                }
                let mut msgs = Vec::new();
                for frame in &frames {
                    match frame {
                        Frame::FixComp(raw) => {
                            let Some(unsigned) = conn.unsign(raw) else { continue };
                            match fixcomp::fixcomp_decompress(&unsigned) {
                                Ok(inner) => {
                                    if log::log_enabled!(log::Level::Trace) {
                                        for m in &inner {
                                            log::trace!("WIRE< hmds/comp {}", crate::protocol::fix::fmt_pipe(m));
                                        }
                                    }
                                    msgs.extend(inner);
                                }
                                Err(e) => {
                                    log::warn!(
                                        "HMDS: dropping malformed FIXCOMP frame ({} bytes): {}",
                                        unsigned.len(), e,
                                    );
                                }
                            }
                        }
                        Frame::Binary(raw) => {
                            let Some(unsigned) = conn.unsign(raw) else { continue };
                            if log::log_enabled!(log::Level::Trace) {
                                log::trace!("WIRE< hmds/bin {}", crate::protocol::fix::fmt_pipe(&unsigned));
                            }
                            msgs.push(unsigned);
                        }
                        Frame::Fix(raw) => {
                            let Some(unsigned) = conn.unsign(raw) else { continue };
                            if log::log_enabled!(log::Level::Trace) {
                                log::trace!("WIRE< hmds/fix {}", crate::protocol::fix::fmt_pipe(&unsigned));
                            }
                            msgs.push(unsigned);
                        }
                        Frame::Control(_) => {
                        // 8=1 / 8=X control state — not consumed on the data path.
                        }
                    }
                }
                msgs
            }
        };
        for msg in &messages {
            self.process_hmds_message(msg, hmds_conn, shared, event_tx, hb);
        }
    }

    pub(crate) fn process_hmds_message(
        &mut self,
        msg: &[u8],
        hmds_conn: &mut Option<Connection>,
        shared: &SharedState,
        event_tx: &Option<EventSink>,
        hb: &mut HeartbeatState,
    ) {
        let parsed = fix::fix_parse(msg);
        let msg_type = match parsed.get(&fix::TAG_MSG_TYPE) {
            Some(t) => t.as_str(),
            None => return,
        };
        // Every message this connection carries, kept whole when asked. What a
        // subscription actually answers with is a question the answer to which
        // is on the wire, not in anyone's reading of it.
        if *crate::engine::hot_loop::CAPTURE_WIRE {
            let hex: String = msg.iter().map(|b| format!("{b:02x}")).collect();
            shared.market.note_unread_wire("hmds-msg", hex);
        }
        match msg_type {
            "E" => self.handle_tbt_data(msg, shared, event_tx),
            "0" => {}
            "1" => {
                let test_id = parsed.get(&fix::TAG_TEST_REQ_ID).cloned().unwrap_or_default();
                if let Some(conn) = hmds_conn.as_mut() {
                    let ts = chrono_free_timestamp();
                    let _ = conn.send_fix(&[
                        (fix::TAG_MSG_TYPE, fix::MSG_HEARTBEAT),
                        (fix::TAG_SENDING_TIME, &ts),
                        (fix::TAG_TEST_REQ_ID, &test_id),
                    ]);
                    hb.last_hmds_sent = Instant::now();
                }
            }
            "W" => {
                if let Some(xml_tag) = parsed.get(&6118) {
                    // Per-frame XML root tracer (kept at debug: fires on every
                    // W/6118 payload). Unmatched payloads still warn below.
                    log::debug!(
                        "HMDS W xml head (len={}): {:?}",
                        xml_tag.len(),
                        // Byte-slicing a lossily decoded value panics when
                        // byte 200 lands inside a multi-byte character, which
                        // aborts the hot loop exactly when debug logging is on.
                        xml_tag.get(..200).unwrap_or(xml_tag),
                    );
                    // The venue answers a tick subscription by saying what it
                    // has called it and, on the same breath, the increments its
                    // prices and sizes move in. Everything needed to read what
                    // follows is here, and none of it had been read: the
                    // increment was being taken from a market-data
                    // subscription that a caller need not have made, and the
                    // number the venue gave was never learned at all.
                    if let Some(ack) = parse_tick_subscription_ack(xml_tag) {
                        if let Some(pos) = self
                            .tbt_subscriptions
                            .iter()
                            .position(|sub| sub.query_id == ack.query_id)
                        {
                            let sub = &mut self.tbt_subscriptions[pos];
                            sub.venue_id = ack.venue_id;
                            sub.min_tick =
                                (ack.min_tick * crate::types::PRICE_SCALE as f64).round() as i64;
                            sub.size_tick = ack.size_min_tick;
                            sub.running = Default::default();
                            log::info!(
                                "tick subscription {} is number {} to the venue, moving in {} \
                                 with sizes in {}",
                                ack.query_id, ack.venue_id, ack.min_tick, ack.size_min_tick,
                            );
                        }
                        return;
                    }
                    if let Some(resp) = crate::control::historical::parse_bar_response(xml_tag) {
                        // The same rule the other replies are matched by. A
                        // plain prefix test hands `hist_10001`'s bars to
                        // whoever is waiting on `hist_1000`, which a session
                        // long enough to reach five figures of query ids does
                        // hold at the same time.
                        if let Some(pos) = self.pending_historical.iter().position(|(qid, _)| states(&resp.query_id, qid.as_str())) {
                            let (_, req_id) = self.pending_historical[pos];
                            let is_complete = resp.is_complete;
                            // Activity on this query — push the idle deadline out.
                            // Bar completion rides <eoq>true> in the final segmented
                            // ResultSetBar; earlier segments carry <eoq>false>. Kept at
                            // debug: fires per bar batch.
                            log::debug!(
                                "HMDS W matched: req_id={} query_id={:?} eoq={} bars={}",
                                req_id, resp.query_id, is_complete, resp.bars.len()
                            );
                            // Clone only when someone is listening on the event
                            // channel — a bar batch is a deep copy.
                            let for_event = clone_for_event(event_tx, &resp);
                            shared.reference.push_historical_data(req_id, resp);
                            if let Some(data) = for_event {
                                emit(event_tx, Event::HistoricalData { req_id, data });
                            }
                            if is_complete && !self.keep_up_to_date_reqs.contains(&req_id) {
                                self.pending_historical.remove(pos);
                            }
                        } else {
                            // A parsed response whose query_id matches no
                            // in-flight pending_historical is reported rather
                            // than dropped.
                            log::warn!(
                                "HMDS W parsed but no pending_historical match: resp.query_id={:?} eoq={} bars={} pending={:?}",
                                resp.query_id, resp.is_complete, resp.bars.len(), self.pending_historical
                            );
                        }
                    }
                    else if let Some(resp) = crate::control::historical::parse_head_timestamp_response(xml_tag) {
                        // Matched on the query name, which the response
                        // echoes. Two head-timestamp requests can be in flight
                        // at once.
                        if let Some(pos) = self.pending_head_ts.iter()
                            .position(|(qid, _)| answers(xml_tag, qid))
                        {
                            let (_, req_id) = self.pending_head_ts.remove(pos);
                            let for_event = clone_for_event(event_tx, &resp);
                            shared.reference.push_head_timestamp(req_id, resp);
                            if let Some(data) = for_event {
                                emit(event_tx, Event::HeadTimestamp { req_id, data });
                            }
                        }
                    }
                    else if let Some(entries) = crate::control::histogram::parse_histogram_response(xml_tag) {
                        // Matched on the query name, as tick and schedule
                        // responses are.
                        if let Some(pos) = self.pending_histogram
                            .iter()
                            .position(|(qid, _)| answers(xml_tag, qid))
                        {
                            let (_, req_id) = self.pending_histogram.remove(pos);
                            shared.reference.push_histogram_data(req_id, entries);
                        }
                    }
                    else if xml_tag.contains("<ResultSetTick>") {
                        if let Some(pos) = self.pending_ticks.iter().position(|(qid, _, _)| answers(xml_tag, qid)) {
                            let (_, req_id, what_to_show) = self.pending_ticks[pos].clone();
                            match crate::control::historical::parse_tick_response(xml_tag, &what_to_show) {
                                // A tick response may arrive in segments,
                                // each stating whether it is the last. The
                                // route is held until a segment states it is.
                                Some((_, data, done)) => {
                                    if done {
                                        self.pending_ticks.remove(pos);
                                    }
                                    shared.reference.push_historical_ticks(req_id, data, what_to_show, done);
                                }
                                None => log::warn!(
                                    "HMDS tick segment for req_id={req_id} did not read; \
                                     waiting for the rest of the answer",
                                ),
                            }
                        }
                    }
                    else if let Some(resp) = crate::control::historical::parse_schedule_response(xml_tag) {
                        if let Some(pos) = self.pending_schedule.iter().position(|(qid, _, _)| *qid == resp.query_id) {
                            let (_, req_id, asked_to) = self.pending_schedule.remove(pos);
                            // The venue states where its coverage ends. Only
                            // where it says nothing does the request's own end
                            // stand in, which is the caller's timestamp and
                            // not a statement about what is covered.
                            let mut resp = resp;
                            if resp.end_date_time.is_empty() {
                                resp.end_date_time = asked_to;
                            }
                            shared.reference.push_historical_schedule(req_id, resp);
                        }
                    }
                    else if let Some(ticker_id_str) = crate::control::historical::parse_ticker_id(xml_tag) {
                        // No unit, no bars: a price counted in a unit nobody
                        // stated is wrong and looks right.
                        let Some(min_tick) =
                            crate::control::historical::min_tick_of(xml_tag, &ticker_id_str)
                        else {
                            return;
                        };
                        // Stated in the same element as the price increment.
                        // A volume is a count of it, the way a size is on the
                        // quote and tick-by-tick streams; absent, sizes are
                        // already whole and one leaves them alone.
                        let size_tick = crate::control::xml::tag(xml_tag, "sizeMinTick")
                            .and_then(|v| v.parse::<f64>().ok())
                            .filter(|t| *t > 0.0)
                            .unwrap_or(1.0);
                        let ticker_id: u32 = ticker_id_str.parse().unwrap_or(0);
                        let mut matched = false;
                        for sub in &mut self.rtbar_subs {
                            if answers(xml_tag, &sub.0) {
                                sub.2 = Some(ticker_id);
                                sub.3 = min_tick;
                                // And the increment its sizes move in. A
                                // stream asked for directly is written down
                                // before the ack arrives, with a placeholder
                                // for both — updating only the price one left
                                // every ordinary real-time bar counting sizes
                                // as whole units.
                                sub.4 = size_tick;
                                log::info!("HMDS rtbar ticker_id={} min_tick={} for req_id={}", ticker_id, min_tick, sub.1);
                                matched = true;
                                break;
                            }
                        }
                        if !matched {
                            // Check keepUpToDate historical queries
                            for (qid, req_id) in &self.pending_historical {
                                if answers(xml_tag, qid) && self.keep_up_to_date_reqs.contains(req_id) {
                                    // Store as rtbar subscription so 35=G bars get
                                    // dispatched
                                    self.rtbar_subs.push((qid.clone(), *req_id, Some(ticker_id), min_tick, size_tick));
                                    matched = true;
                                    break;
                                }
                            }
                        }
                        if !matched {
                            log::info!("HMDS TBT ticker_id assigned: {ticker_id_str}");
                        }
                    }
                    else if xml_tag.contains("<QueryError>") {
                        // Gateway rejected the query (e.g. "Invalid time length").
                        // Without this branch the pending entry leaks forever and the
                        // consumer sees no completion or error event.
                        let query_id = crate::control::xml::tag(xml_tag, "id")
                            .map(|s| s.to_string());
                        let error_msg = crate::control::xml::tag(xml_tag, "error")
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "unknown".to_string());
                        // IB canonical error code for HMDS-side validation/rejection.
                        const HMDS_ERROR_CODE: i32 = 162;
                        let mut released_req_id: Option<u32> = None;
                        let mut from_historical = false;
                        if let Some(qid) = &query_id {
                            if let Some(pos) = self.pending_historical.iter().position(|(q, _)| q == qid) {
                                let (_, req_id) = self.pending_historical.remove(pos);
                                self.keep_up_to_date_reqs.remove(&req_id);
                                // The reconnect list is what gets asked for
                                // again, so a query the server rejected has to
                                // leave it or it comes straight back. Only the
                                // keepUpToDate list: a real-time bar carries the
                                // same caller id in a list of its own, and the
                                // two id spaces are not shared, so matching on
                                // the number alone tore down a live bar stream
                                // that nothing had rejected.
                                released_req_id = Some(req_id);
                                from_historical = true;
                            } else if let Some(pos) = self.rtbar_subs.iter().position(|(q, ..)| q == qid) {
                                // A rejected bar query is matched on the query
                                // id, which is the one identifier that is
                                // unique across request kinds. Its reconnect
                                // record goes too, or the next reconnect asks
                                // for a stream the server already refused.
                                let (_, req_id, ..) = self.rtbar_subs.remove(pos);
                                self.rtbar_resub.retain(|r| r.req_id != req_id);
                                released_req_id = Some(req_id);
                            } else if let Some(pos) = self.pending_head_ts.iter().position(|(q, _)| q == qid) {
                                let (_, req_id) = self.pending_head_ts.remove(pos);
                                released_req_id = Some(req_id);
                            } else if let Some(pos) = self.pending_histogram.iter().position(|(q, _)| q == qid) {
                                let (_, req_id) = self.pending_histogram.remove(pos);
                                released_req_id = Some(req_id);
                            } else if let Some(pos) = self.pending_ticks.iter().position(|(q, _, _)| q == qid) {
                                let (_, req_id, _) = self.pending_ticks.remove(pos);
                                released_req_id = Some(req_id);
                            } else if let Some(pos) = self.pending_schedule.iter().position(|(q, _, _)| q == qid) {
                                let (_, req_id, _) = self.pending_schedule.remove(pos);
                                released_req_id = Some(req_id);
                            } else if let Some(pos) = self.pending_scanner.iter().position(|(q, _)| q == qid) {
                                let (_, req_id) = self.pending_scanner.remove(pos);
                                released_req_id = Some(req_id);
                            } else if let Some(pos) =
                                self.tbt_subscriptions.iter().position(|sub| &sub.query_id == qid)
                            {
                                // A stream the venue refused. Left out of this
                                // chain, its refusal was logged against a query
                                // nobody had been told about and the caller
                                // waited on a stream that was never coming.
                                let sub = self.tbt_subscriptions.remove(pos);
                                released_req_id = Some(sub.caller_req_id as u32);
                            }
                        }
                        match released_req_id {
                            Some(req_id) => {
                                log::warn!(
                                    "HMDS QueryError req_id={req_id} query_id={query_id:?}: {error_msg}"
                                );
                                shared.reference.push_historical_error(req_id, HMDS_ERROR_CODE, error_msg.clone());
                                // Surface a terminal sentinel for historical-bar
                                // consumers
                                // that wait on historical_data_end. Empty response with
                                // is_complete=true unblocks the existing dispatch path.
                                if from_historical {
                                    shared.reference.push_historical_data(
                                        req_id,
                                        crate::control::historical::HistoricalResponse {
                                            query_id: query_id.clone().unwrap_or_default(),
                                            timezone: String::new(),
                                            is_complete: true,
                                            bars: Vec::new(),
                                        },
                                    );
                                }
                            }
                            None => {
                                log::warn!(
                                    "HMDS QueryError for unknown query_id={query_id:?}: {error_msg}"
                                );
                            }
                        }
                    }
                    else {
                        // Warn, not debug: a drop in the W cascade is visible
                        // to an application logging at info.
                        log::warn!("HMDS unmatched W response (len={}): {:?}", xml_tag.len(), xml_tag);
                    }
                } else {
                    // A W message with no 6118 payload carries no bars; it is
                    // reported rather than dropped silently.
                    log::warn!("HMDS W with no tag 6118 (msg_len={})", msg.len());
                }
            }
            "U" => {
                if let Some(comm) = parsed.get(&6040) {
                    match comm.as_str() {
                        "10002" => {
                            if let Some(xml) = parsed.get(&6118) {
                                self.pending_scanner_params = false;
                                shared.reference.push_scanner_params(xml.clone());
                            }
                        }
                        "10005" => {
                            let payload = parsed.get(&6118);
                            // What the venue actually states per row, which is
                            // the only way to tell a field it does not send
                            // from one this client does not read.
                            if let Some(xml) = payload {
                                log::debug!("scan response payload: {xml}");
                            }
                            if payload.is_none() {
                                log::warn!("scan response carried no payload (msg_len={})", msg.len());
                            }
                            if let Some(xml) = payload
                                && let Some(result) = crate::control::scanner::parse_scanner_response(xml)
                                        .or_else(|| {
                                            log::warn!("scan response payload did not parse ({} bytes)", xml.len());
                                            None
                                        })
                                    && let Some(req_id) = self.scanner_answered(xml) {
                                        // A row is a contract id and the time it
                                        // entered the scan, and nothing else —
                                        // measured on the wire, not assumed — so
                                        // everything a caller reads about the
                                        // contract is resolved on the trading
                                        // connection. Results whose ids are not
                                        // cached are parked for the engine to
                                        // enrich before they are handed over.
                                        let any_cold = result.entries.iter().any(|e| {
                                            e.con_id != 0
                                                && shared.reference.get_contract(e.con_id as i64).is_none()
                                        });
                                        if any_cold {
                                            self.cold_scanner_results.push((req_id, result));
                                        } else {
                                            shared.reference.push_scanner_data(req_id, result);
                                        }
                                    }
                        }
                        "10032" => {
                            let raw_bytes = extract_raw_tag(msg, 96);
                            if let Some(xml) = parsed.get(&6118) {
                                let is_article = xml.contains("article_file");
                                if is_article {
                                    if let Some(pos) = self.pending_articles.iter()
                                        .position(|(qid, _)| answers(xml, qid))
                                        .or_else(|| (!self.pending_articles.is_empty()).then_some(0))
                                    {
                                        let (_, req_id) = self.pending_articles.remove(pos);
                                        match raw_bytes.as_deref()
                                            .and_then(crate::control::news::parse_article_payload)
                                        {
                                            Some((atype, text)) => {
                                                shared.reference.push_news_article(req_id, atype, text)
                                            }
                                            // The response consumes the
                                            // pending request whether or not
                                            // the payload reads, so an
                                            // unreadable one is reported.
                                            None => super::push_hmds_error(
                                                shared,
                                                req_id,
                                                "the news article reply carried no readable article"
                                                    .to_string(),
                                                false,
                                            ),
                                        }
                                    }
                                // Matched on the id the response names, as a
                                // bar response is. Falls back to the oldest
                                // pending request only when the response names
                                // none.
                                } else if let Some(pos) = self.pending_news.iter()
                                    .position(|(qid, _)| answers(xml, qid))
                                    .or_else(|| (!self.pending_news.is_empty()).then_some(0))
                                {
                                    let (_, req_id) = self.pending_news.remove(pos);
                                    if let Some(raw) = &raw_bytes {
                                        let (headlines, has_more) = crate::control::news::parse_news_payload(raw);
                                        shared.reference.push_historical_news(req_id, headlines, has_more);
                                    } else {
                                        shared.reference.push_historical_news(req_id, Vec::new(), false);
                                    }
                                }
                            }
                        }
                        "10012" => {
                            if let Some(xml) = parsed.get(&6118) {
                                // Tag 96 carries gzip bytes, framed by the
                                // length on tag 95. Read from the raw frame:
                                // the parsed field map is UTF-8 lossy, which
                                // replaces every invalid byte and breaks
                                // decompression.
                                let data = if let Some(raw) = extract_raw_tag(msg, 96) {
                                    crate::control::fundamental::decompress_fundamental_data(&raw)
                                        .unwrap_or_else(|| String::from_utf8_lossy(&raw).into_owned())
                                } else {
                                    xml.clone()
                                };
                                // Under the name the venue echoes, which is
                                // this request's own. Taken oldest-first
                                // instead, two in flight were indistinguishable
                                // and either answer went to whichever had
                                // waited longest.
                                let echoed = crate::control::fundamental::echoed_query_id(xml);
                                let at = self.pending_fundamental.iter()
                                    .position(|(qid, _)| *qid == echoed);
                                if let Some(at) = at {
                                    let (_, req_id) = self.pending_fundamental.remove(at);
                                    shared.reference.push_fundamental_data(req_id, data);
                                } else {
                                    shared.market.note_unread_wire(
                                        "historical",
                                        "fundamentals reply with nothing pending".to_string(),
                                    );
                                }
                            }
                        }
                        "10022" => {
                            // A contract's corporate actions. Not a bar frame and
                            // not a completion sentinel: bar completion rides
                            // <eoq>true> in the ResultSetBar.
                            //
                            // The query is echoed back as XML and the actions
                            // arrive beside it on the raw field, a name on its own
                            // line and the rows under it. Read against the contract
                            // they name rather than the request that asked, because
                            // the venue sends one per contract.
                            match parsed.get(&96) {
                                Some(body) => {
                                    let (contract, actions) =
                                        crate::control::adjustments::parse_adjustments(body);
                                    log::debug!(
                                        "corporate actions for {}: {} stated",
                                        contract.con_id, actions.len(),
                                    );
                                    shared.reference.note_adjustments(contract, actions);
                                }
                                // The subtype with nothing on the field it states
                                // its answer on is recorded rather than guessed at.
                                None => shared.market.note_unread_wire(
                                    "historical", "6040=10022 with no body".to_string(),
                                ),
                            }
                        }
                        // An unread subtype is recorded, as an unknown
                        // message type is.
                        other => shared
                            .market
                            .note_unread_wire("historical", format!("6040={other}")),
                    }
                }
            }
            "G" => self.handle_rtbar_data(msg, shared),
            other => {
                // Log unhandled msg_types rather than swallowing them, so a
                // frame bypassing the W cascade — a completion sentinel
                // delivered under another type — is visible.
                // Recorded as well as logged: a claim that this client reads
                // everything the venue sends is only checkable if what it does
                // not read is written down.
                shared.market.note_unread_wire("historical", format!("type {other}"));
                log::warn!("HMDS unhandled msg_type={:?} (msg_len={})", other, msg.len());
            }
        }
    }

    fn handle_tbt_data(&mut self, msg: &[u8], shared: &SharedState, event_tx: &Option<EventSink>) {
        use crate::protocol::tbt_stream::{self, TbtKind, TbtRecord};

        let body = match find_body_after_tag(msg, b"35=E\x01") {
            Some(b) => b,
            None => return,
        };
        if *crate::engine::hot_loop::CAPTURE_WIRE {
            let hex: String = msg.iter().map(|b| format!("{b:02x}")).collect();
            shared.market.note_unread_wire("tbt-frame", hex);
        }

        // Which subscription a frame belongs to is stated on the frame. Taking
        // the first subscription instead attributed every record to whichever
        // was made first, so a second contract's trades were reported under the
        // first contract's name — visibly, once two were running at once.
        let stated = crate::protocol::tbt_stream::frame_ticker_id(body);
        let found = stated.and_then(|id| {
            self.tbt_subscriptions.iter().position(|sub| sub.venue_id == id)
        });
        // A frame naming a subscription this session does not hold is not
        // attributed to another one.
        let Some(at) = found else {
            // Said once per stream rather than once per tick. A withdrawal this
            // venue does not act on goes on delivering for the rest of the
            // session, and several hundred identical lines bury the rest of the
            // log without saying anything the first one did not.
            if let Some(id) = stated
                && self.tbt_reported.insert(id)
            {
                if self.tbt_withdrawn.contains(&id) {
                    log::warn!(
                        "the venue is still sending ticks for stream {id}, which this \
                         session withdrew; they are dropped, and it does not stop until \
                         the session ends",
                    );
                } else {
                    log::warn!(
                        "a tick names stream {id}, which this session does not hold; dropped",
                    );
                }
            }
            return;
        };
        let instrument = self.tbt_subscriptions[at].instrument;
        // Which request this arrived under, as the caller numbered it. A
        // contract can carry several streams, so the contract alone does not
        // say which one a record belongs to.
        let caller_req_id = self.tbt_subscriptions[at].caller_req_id;
        // The layout of a record is the layout of the stream it arrived on,
        // which is a property of the subscription and not of the contract.
        let kind = match self.tbt_subscriptions[at].kind {
            TbtType::BidAsk => TbtKind::BidAsk,
            _ => TbtKind::AllLast,
        };

        // A move is stated in whole increments of the contract's own smallest
        // one, so without that increment a move cannot be turned into a price.
        let mts = self.tbt_subscriptions[at].min_tick;
        if mts <= 0 {
            log::warn!(
                "a tick arrived for an instrument whose smallest increment is not \
                 known, so its price cannot be worked out; dropped rather than guessed"
            );
            return;
        }

        // What sizes move in for this contract. Stated once, when the venue
        // took the subscription on.
        let size_tick = self.tbt_subscriptions[at].size_tick;
        // Whether this subscription wants only what the exchange itself
        // printed. The venue serves one trade stream — a future, which has no
        // off-exchange tape at all, streams on AllLast and stays silent on
        // Last — and marks the prints that were not reported to the tape. So
        // the narrower stream is the wider one without those.
        let kind_asked_for = self.tbt_subscriptions[at].kind;

        // Decoded in whole increments and scaled by whole numbers afterwards,
        // so a session of moves cannot drift the way adding fractions would.
        let running = &mut self.tbt_subscriptions[at].running;
        let Some(frame) = tbt_stream::decode_frame(body, kind, 1.0, running) else {
            return;
        };

        for stamped in &frame.records {
            match &stamped.record {
                TbtRecord::Trade(t) => {
                    if !belongs_on(kind_asked_for, t.unreported) {
                        continue;
                    }
                    let trade = crate::types::TbtTrade {
                        instrument,
                        req_id: caller_req_id,
                        price: (t.price as i64).saturating_mul(mts),
                        // A size is a count of what the venue said sizes move
                        // in for this contract — whole ones for a share,
                        // hundred-millionths for a crypto — and is then held in
                        // the form every reader divides by.
                        size: scaled_size(t.size, size_tick),
                        timestamp: stamped.seconds,
                        exchange: t.exchange.clone(),
                        conditions: t.conditions.clone(),
                        past_limit: t.past_limit,
                        unreported: t.unreported,
                    };
                    shared.market.push_tbt_trade(trade.clone());
                    emit(event_tx, Event::TbtTrade(trade));
                }
                TbtRecord::Quote(q) => {
                    let quote = crate::types::TbtQuote {
                        instrument,
                        req_id: caller_req_id,
                        bid: (q.bid as i64).saturating_mul(mts),
                        ask: (q.ask as i64).saturating_mul(mts),
                        bid_size: scaled_size(q.bid_size, size_tick),
                        ask_size: scaled_size(q.ask_size, size_tick),
                        timestamp: stamped.seconds,
                        bid_past_low: q.bid_past_low,
                        ask_past_high: q.ask_past_high,
                    };
                    shared.market.push_tbt_quote(quote);
                    emit(event_tx, Event::TbtQuote(quote));
                }
                // A midpoint has no place on either of the two shapes a caller
                // reads, so no record is synthesised. Recorded as unread:
                // nothing here subscribes to this stream.
                TbtRecord::MidPoint { .. } => shared
                    .market
                    .note_unread_wire("tbt-frame", "MidPoint record".to_string()),
            }
        }
    }

    fn handle_rtbar_data(&mut self, msg: &[u8], shared: &SharedState) {
        let body = match find_body_after_tag(msg, b"35=G\x01") {
            Some(b) => b,
            None => return,
        };
        let sig_pos = body.windows(6).position(|w| w == b"\x018349=");
        let body = if let Some(pos) = sig_pos { &body[..pos] } else { body };
        if body.len() < 11 { return; }
        let ticker_id = u32::from_be_bytes([body[2], body[3], body[4], body[5]]);
        let timestamp = u32::from_be_bytes([body[6], body[7], body[8], body[9]]);
        let payload_len = body[10] as usize;
        if body.len() < 11 + payload_len { return; }
        let sub = self.rtbar_subs.iter().find(|(_, _, tid, ..)| *tid == Some(ticker_id));
        let (req_id, min_tick, size_tick) = match sub {
            Some((_, rid, _, mt, st)) => (*rid, *mt, *st),
            None => return,
        };
        let payload = &body[11..11 + payload_len];
        if let Some(mut bar) =
            crate::control::historical::decode_bar_payload(payload, min_tick, size_tick)
        {
            bar.timestamp = timestamp;
            // A caller keeping bars up to date asked for its own bar size, so
            // what it hears is the bar it asked for as it stands, not the
            // five-second one this was folded from.
            if let Some(forming) = self.forming_bars.iter_mut().find(|f| f.req_id == req_id) {
                let so_far = forming.fold(&bar);
                shared.market.push_real_time_bar(req_id, so_far);
                return;
            }
            shared.market.push_real_time_bar(req_id, bar);
        }
    }



/// The query that opens one tick stream.
///
/// Every element is named for the field the venue's own query holds it in,
/// without its prefix. It states no filter: the venue carries one and this
/// client has not settled how to make it apply, which is why `ignore_size` is
/// refused rather than sent.
fn build_tbt_query(
    req_id: u32,
    con_id: i64,
    venue: &str,
    stype: &str,
    tbt_type_str: &str,
) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <Query>\
         <id>tbt_{req_id}</id>\
         <contractID>{con_id}</contractID>\
         <exchange>{venue}</exchange>\
         <secType>{stype}</secType>\
         <expired>no</expired>\
         <type>TickData</type>\
         <refresh>ticks</refresh>\
         <data>{tbt_type_str}</data>\
         <source>API</source>\
         </Query>\
         </ListOfQueries>"
    )
}

    pub(crate) fn send_tbt_subscribe(
        &mut self,
        // What the caller numbered this request, which every record it
        // delivers is stamped with.
        caller_req_id: i64,
        con_id: i64,
        instrument: InstrumentId,
        tbt_type: TbtType,
        sec_type: &str,
        exchange: &str,
        // The smallest increment this contract's price moves in, scaled. A
        // move on this stream is stated in whole ones of these.
        min_tick_scaled: i64,
        hmds_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        let req_id = self.next_tbt_req_id;
        self.next_tbt_req_id += 1;
        // KNOWN TO DIVERGE. The vendor build states these apart — `Last`,
        // `AllLast` and `BidAsk` are three distinct values it writes — and
        // both trade streams are asked for here under one of them, with the
        // other made afterwards by dropping the prints the venue marks as not
        // reported to the tape. That rule is this client's reading of what
        // belongs on a tape, not the venue's.
        //
        // The note this replaced said the venue acknowledges the other name
        // and sends nothing. That may still be so — the vendor's own query
        // carries fields this one omits, any of which could be why — but it
        // was not re-checked, and a contract thin enough to trade nothing in
        // twenty seconds cannot check it. Settle it on a liquid name in a
        // session, by asking for `Last` and seeing whether trades arrive.
        let tbt_type_str = match tbt_type {
            TbtType::AllLast | TbtType::Last => "AllLast",
            TbtType::BidAsk => "BidAsk",
        };
        // The contract says what it is. A US stock routed BEST was assumed for
        // every subscription, so an FX pair or a future asked for ticks under a
        // description that was not its own.
        let venue = hist_exchange(exchange);
        let stype = hist_sec_type(sec_type);
        let xml = Self::build_tbt_query(req_id, con_id, &venue, &stype, tbt_type_str);
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "W"),
                (fix::TAG_SENDING_TIME, &ts),
                (6118, &xml),
            ]);
            log::info!("Sent TBT subscribe: con_id={con_id} type={tbt_type_str} req_id={req_id}");
            hb.last_hmds_sent = Instant::now();
        }
        let ticker_id = format!("tbt_{req_id}");
        self.tbt_subscriptions.push(TbtSubscription {
            instrument,
            query_id: ticker_id,
            kind: tbt_type,
            caller_req_id,
            venue_id: 0,
            // A move means nothing without the increment it is counted in.
            // The acknowledgement states the venue's; this stands until it
            // arrives.
            min_tick: min_tick_scaled,
            size_tick: 0.0,
            running: Default::default(),
                });
    }

    pub(crate) fn send_tbt_unsubscribe(
        &mut self,
        // The request that opened the stream being withdrawn. A contract can
        // carry several, and taking "the one on this contract" withdraws
        // whichever was opened first and leaves the caller's own running.
        caller_req_id: i64,
        instrument: InstrumentId,
        hmds_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        let idx = match self
            .tbt_subscriptions
            .iter()
            .position(|sub| sub.caller_req_id == caller_req_id && sub.instrument == instrument)
            // A caller that opened one stream on this contract and names it by
            // something else still gets that one withdrawn.
            .or_else(|| self.tbt_subscriptions.iter().position(|sub| sub.instrument == instrument))
        {
            Some(i) => i,
            None => return,
        };
        let gone = self.tbt_subscriptions.remove(idx);
        // The venue's own number for the stream, which its records are stamped
        // with. Kept so records that keep arriving after this are recognised as
        // the ones this withdrawal was meant to stop.
        if gone.venue_id != 0 {
            self.tbt_withdrawn.insert(gone.venue_id);
        }
        // By the venue's own number for the stream, not the name this client
        // gave it when asking. Named the second way the withdrawal is accepted
        // and does nothing: a live session counted three hundred and forty-four
        // records after one, and four hundred and sixty-nine after another,
        // arriving until the session ended. The bar stream beside this already
        // withdraws by the venue's number. Falls back to the name only where
        // the subscription was never acknowledged and there is no number yet.
        let ticker_id = if gone.venue_id != 0 {
            gone.venue_id.to_string()
        } else {
            gone.query_id
        };
        if let Some(conn) = hmds_conn.as_mut() {
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <ListOfCancelQueries>\
                 <CancelQuery>\
                 <id>ticker:{ticker_id}</id>\
                 </CancelQuery>\
                 </ListOfCancelQueries>",
            );
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "Z"),
                (fix::TAG_SENDING_TIME, &ts),
                (6118, &xml),
            ]);
            log::info!("Sent TBT unsubscribe: instrument={instrument} ticker_id={ticker_id}");
            hb.last_hmds_sent = Instant::now();
        }
    }

    pub(crate) fn send_historical_request_ex(
        &mut self,
        req_id: u32,
        con_id: i64,
        end_date_time: &str,
        duration: &str,
        bar_size: &str,
        what_to_show: &str,
        use_rth: bool,
        keep_up_to_date: bool,
        include_expired: bool,
        symbol: &str,
        sec_type: &str,
        exchange: &str,
        hmds_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
        shared: &SharedState,
    ) {
        let duration = crate::control::historical::normalize_duration(duration);
        let duration = duration.as_str();
        let end_date_time = if end_date_time.is_empty() {
            crate::protocol::datetime::chrono_free_timestamp().to_string()
        } else {
            end_date_time.to_string()
        };
        let end_date_time = end_date_time.as_str();
        let qid = self.next_hmds_query_id;
        self.next_hmds_query_id += 1;

        // One shared table, rejection instead of a silent Min5/TRADES
        // fallback. The client validates synchronously before the
        // command is sent; this is the engine-side backstop for raw
        // control-channel callers.
        let data_type = match crate::control::historical::BarDataType::from_api_str(what_to_show) {
            Ok(dt) => dt,
            Err(e) => {
                log::error!("historical req_id={req_id}: {e}");
                super::push_hmds_error(shared, req_id, e, true);
                return;
            }
        };
        let bs = match crate::control::historical::BarSize::from_api_str(bar_size) {
            Ok(bs) => bs,
            Err(e) => {
                log::error!("historical req_id={req_id}: {e}");
                super::push_hmds_error(shared, req_id, e, true);
                return;
            }
        };

        let query_id = format!("hist_{qid}");
        let req = crate::control::historical::HistoricalRequest {
            query_id: query_id.clone(),
            con_id: con_id as u32,
            symbol: symbol.to_string(),
            sec_type: hist_sec_type(sec_type),
            exchange: hist_exchange(exchange),
            data_type,
            end_time: end_date_time.to_string(),
            duration: duration.to_string(),
            bar_size: bs,
            use_rth,
            keep_up_to_date,
            include_expired,
        };

        let xml = crate::control::historical::build_query_xml(&req);
        // The query as it goes out, when asked for. A request the venue does
        // not answer is only distinguishable from one it never received by
        // what was actually sent.
        if *crate::engine::hot_loop::CAPTURE_WIRE {
            shared.market.note_unread_wire("historical-query", xml.clone());
            log::info!("historical query as sent: {xml}");
        }
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "W"),
                (fix::TAG_SENDING_TIME, &ts),
                (6118, &xml),
            ]);
            log::info!("Sent historical request: req_id={req_id} con_id={con_id} bar_size={bar_size}");
            hb.last_hmds_sent = Instant::now();
        }
        self.pending_historical.push((query_id, req_id));
    }

    /// Tell the venue to stop serving a query.
    pub(crate) fn send_historical_cancel(&mut self, query_id: &str, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        if let Some(conn) = hmds_conn.as_mut() {
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <ListOfCancelQueries>\
                 <CancelQuery>\
                 <id>ticker:{query_id}</id>\
                 </CancelQuery>\
                 </ListOfCancelQueries>",
            );
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "Z"),
                (fix::TAG_SENDING_TIME, &ts),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
        }
    }

    pub(crate) fn send_head_timestamp_request(&mut self, req_id: u32, con_id: i64, what_to_show: &str, use_rth: bool, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState, shared: &SharedState) {
        // Same shared table as the bar paths — this was a third divergent
        // copy with a silent TRADES fallback.
        let data_type = match crate::control::historical::BarDataType::from_api_str(what_to_show) {
            Ok(dt) => dt,
            Err(e) => {
                log::error!("head timestamp req_id={req_id}: {e}");
                super::push_hmds_error(shared, req_id, e, false);
                return;
            }
        };
        // Security type and exchange come from the cached contract
        // definition. A fixed CS/SMART describes a future or currency pair as
        // a US stock, and the request is answered for that instrument.
        //
        // Left empty where no definition is cached. The server reads these
        // fields, so an invented description returns another instrument.
        let Some(described) = shared.reference.get_contract(con_id).filter(|c| {
            !c.sec_type.is_empty() && !c.exchange.is_empty()
        }) else {
            let told = format!(
                "contract {con_id} has no definition here yet, and a head timestamp \
                 states the contract's own type and venue"
            );
            log::warn!("{told}");
            super::push_hmds_error(shared, req_id, told, false);
            return;
        };
        let req = crate::control::historical::HeadTimestampRequest {
            con_id: con_id as u32,
            sec_type: described.sec_type.clone(),
            exchange: described.exchange.clone(),
            data_type,
            use_rth,
        };
        let xml = crate::control::historical::build_head_timestamp_xml(&req);
        // The id the query goes out under, so the response can be matched to
        // the caller. A locally generated id never reaches the wire.
        let query_id = crate::control::historical::head_timestamp_query_id(&req);
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "W"),
                (fix::TAG_SENDING_TIME, &ts),
                (6118, &xml),
            ]);
            log::info!("Sent head timestamp request: req_id={req_id} con_id={con_id}");
            hb.last_hmds_sent = Instant::now();
        }
        self.pending_head_ts.push((query_id, req_id));
    }

    pub(crate) fn send_scanner_params_request(&mut self, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (crate::control::scanner::TAG_SUB_PROTOCOL, "10001"),
            ]);
            self.pending_scanner_params = true;
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent scanner params request");
        }
    }

    pub(crate) fn send_scanner_subscribe(&mut self, req_id: u32, instrument: &str, location_code: &str, scan_code: &str, max_items: u32, filters: Vec<(String, String)>, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        let sub = crate::control::scanner::ScannerSubscription {
            instrument: instrument.to_string(),
            location_code: location_code.to_string(),
            scan_code: scan_code.to_string(),
            max_items,
            filters,
        };
        let scan_id = format!("APISCAN{}:{}", self.next_scanner_id, req_id);
        self.next_scanner_id += 1;
        let xml = crate::control::scanner::build_scanner_subscribe_xml(&sub, &scan_id);
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (6040, "10003"),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent scanner subscribe: req_id={req_id} scan_code={scan_code}");
        }
        self.pending_scanner.push((scan_id, req_id));
    }

    /// Which scan a response answers.
    ///
    /// Every scan response arrives under the same message id, so that id
    /// cannot identify the scan. The payload carries the scan name this client
    /// supplied on subscribe, which does.
    ///
    /// A response naming a scan this client is not running is delivered to the
    /// oldest pending request.
    fn scanner_answered(&self, xml: &str) -> Option<u32> {
        let Some(named) = crate::control::xml::tag(xml, "id") else {
            log::warn!(
                "scan response names no scan — {} pending, so there is nothing \
                 that says whose rows these are",
                self.pending_scanner.len(),
            );
            return None;
        };
        let found = self
            .pending_scanner
            .iter()
            .find(|(scan_id, _)| scan_id == named)
            .map(|(_, req_id)| *req_id);
        if found.is_none() {
            // A scan this session is not running: already withdrawn, or
            // belonging to another session on this login.
            log::warn!("scan response names {named}, which is not a scan this session is running");
        }
        found
    }

    pub(crate) fn send_scanner_cancel(&mut self, scan_id: &str, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        let xml = crate::control::scanner::build_scanner_cancel_xml(scan_id);
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (6040, "10004"),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent scanner cancel: scan_id={scan_id}");
        }
    }

    pub(crate) fn send_historical_news_request(&mut self, req_id: u32, con_id: u32, provider_codes: &str, start_time: &str, end_time: &str, max_results: u32, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        let query_id = format!("news_{}", self.next_hmds_query_id);
        let req = crate::control::news::HistoricalNewsRequest {
            query_id: query_id.clone(),
            con_id,
            provider_codes: provider_codes.to_string(),
            start_time: start_time.to_string(),
            end_time: end_time.to_string(),
            max_results,
        };
        let xml = crate::control::news::build_historical_news_xml(&req);
        self.next_hmds_query_id += 1;
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (6040, "10030"),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent historical news request: req_id={req_id} con_id={con_id}");
        }
        self.pending_news.push((query_id, req_id));
    }

    pub(crate) fn send_news_article_request(&mut self, req_id: u32, provider_code: &str, article_id: &str, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        let query_id = format!("art_{}", self.next_hmds_query_id);
        let req = crate::control::news::NewsArticleRequest {
            query_id: query_id.clone(),
            provider_code: provider_code.to_string(),
            article_id: article_id.to_string(),
        };
        let xml = crate::control::news::build_article_request_xml(&req);
        self.next_hmds_query_id += 1;
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (6040, "10030"),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent news article request: req_id={req_id} article={article_id}");
        }
        self.pending_articles.push((query_id, req_id));
    }

    pub(crate) fn send_fundamental_data_request(&mut self, req_id: u32, con_id: u32, report_type: &str, shared: &SharedState, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        use crate::control::fundamental::ReportType;
        let rt = match report_type {
            "ReportSnapshot" | "snapshot" => ReportType::Snapshot,
            "RESC" | "estimates" => ReportType::Estimates,
            "CalendarReport" | "calendar" => ReportType::Calendar,
            other => {
                // Refused rather than substituted; an unsupported report
                // type is not silently answered with a snapshot.
                let told = format!(
                    "report type {other:?} is not one the venue states: it is \
                     ReportSnapshot, RESC or CalendarReport"
                );
                log::warn!("{told}");
                super::push_hmds_error(shared, req_id, told, false);
                return;
            }
        };
        // As with the head timestamp: the contract's own description, not a US
        // stock's.
        // As with the head timestamp: the cached contract description, empty
        // where none is cached.
        let Some(described) = shared.reference.get_contract(con_id as i64).filter(|c| {
            !c.sec_type.is_empty() && !c.currency.is_empty()
        }) else {
            let told = format!(
                "contract {con_id} has no definition here yet, and a fundamentals \
                 request states the contract's own type and currency"
            );
            log::warn!("{told}");
            super::push_hmds_error(shared, req_id, told, false);
            return;
        };
        // Its own name, which the venue echoes on the answer. The name was
        // worked out here and then not sent: every request went out under one
        // constant, so two in flight could only be told apart by which had
        // waited longer.
        let query_id = crate::control::fundamental::fundamentals_query_id(self.next_hmds_query_id);
        self.next_hmds_query_id += 1;
        let req = crate::control::fundamental::FundamentalRequest {
            con_id,
            sec_type: described.sec_type.clone(),
            currency: described.currency.clone(),
            report_type: rt,
            query_id: query_id.clone(),
        };
        let xml = crate::control::fundamental::build_fundamental_request_xml(&req);
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "U"),
                (fix::TAG_SENDING_TIME, &ts),
                (6040, "10010"),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent fundamental data request: req_id={req_id} con_id={con_id}");
        }
        self.pending_fundamental.push((query_id, req_id));
    }

    /// Tell the venue to stop serving a fundamentals request.
    ///
    /// Withdrawing it here alone left the venue serving a subscription nobody
    /// was reading, for as long as the session lasted.
    pub(crate) fn send_fundamental_cancel(
        &mut self,
        req_id: u32,
        hmds_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        // Sent whether or not this client still has the request on its own
        // list. That list is emptied by the first response the venue sends,
        // which is not the same moment the venue stops serving it — so gating
        // the withdrawal on it sent nothing in the case that actually leaks.
        // Withdrawn under the name it went out with, which is its own.
        let named = self.pending_fundamental.iter()
            .position(|(_, rid)| *rid == req_id)
            .map(|pos| self.pending_fundamental.remove(pos).0);
        let Some(conn) = hmds_conn.as_mut() else { return };
        let Some(query_id) = named else {
            log::debug!("fundamentals withdrawal for req_id={req_id}, which is not waiting");
            return;
        };
        let xml = crate::control::xml::cancel_query(&query_id);
        let ts = chrono_free_timestamp();
        let _ = conn.send_fix(&[
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &ts),
            (6040, "10011"),
            (6118, &xml),
        ]);
        hb.last_hmds_sent = Instant::now();
        log::info!("Sent fundamental data cancel: req_id={req_id}");
    }

    pub(crate) fn send_histogram_request(&mut self, req_id: u32, con_id: u32, sec_type: &str, exchange: &str, use_rth: bool, period: &str, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        let req = crate::control::histogram::HistogramRequest {
            // Its own query name, so two histograms in flight are told apart
            // by the id each goes out under.
            query_id: format!("hg_{}", self.next_hmds_query_id),
            con_id,
            sec_type: hist_sec_type(sec_type),
            exchange: hist_exchange(exchange),
            use_rth,
            period: period.to_string(),
            end_time: chrono_free_timestamp().to_string(),
        };
        self.next_hmds_query_id += 1;
        let xml = crate::control::histogram::build_histogram_request_xml(&req);
        // As with the head timestamp: the id the response will name.
        let query_id = crate::control::histogram::histogram_query_id(&req);
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "W"),
                (fix::TAG_SENDING_TIME, &ts),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent histogram request: req_id={req_id} con_id={con_id}");
        }
        self.pending_histogram.push((query_id, req_id));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn send_historical_ticks_request(&mut self, req_id: u32, con_id: i64, sec_type: &str, exchange: &str, start_date_time: &str, end_date_time: &str, number_of_ticks: u32, what_to_show: &str, use_rth: bool, include_expired: bool, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        let qid = self.next_hmds_query_id;
        self.next_hmds_query_id += 1;
        let query_id = format!("tk_{qid}");
        let xml = crate::control::historical::build_tick_query_xml(
            &query_id, con_id, start_date_time, end_date_time, number_of_ticks, what_to_show, use_rth,
            &hist_sec_type(sec_type), &hist_exchange(exchange), include_expired,
        );
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "W"),
                (fix::TAG_SENDING_TIME, &ts),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent historical ticks request: req_id={req_id} con_id={con_id} what={what_to_show}");
        }
        self.pending_ticks.push((query_id, req_id, what_to_show.to_string()));
    }

    pub(crate) fn send_realtime_bar_subscribe(&mut self, req_id: u32, con_id: i64, _symbol: &str, sec_type: &str, exchange: &str, what_to_show: &str, use_rth: bool, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        let qid = self.next_hmds_query_id;
        self.next_hmds_query_id += 1;
        let query_id = format!("rt_{qid}");
        let xml = crate::control::historical::build_realtime_bar_xml(
            &query_id, con_id, what_to_show, use_rth,
            &hist_sec_type(sec_type), &hist_exchange(exchange),
        );
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "W"),
                (fix::TAG_SENDING_TIME, &ts),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent rtbar subscribe: req_id={req_id} con_id={con_id} what={what_to_show}");
        }
        self.rtbar_subs.push((query_id, req_id, None, 0.01, 1.0));
        self.rtbar_resub.retain(|r| r.req_id != req_id);
        self.rtbar_resub.push(RtBarRequest {
            req_id, con_id,
            sec_type: sec_type.to_string(), exchange: exchange.to_string(),
            what_to_show: what_to_show.to_string(), use_rth,
        });
    }

    pub(crate) fn send_schedule_request(&mut self, req_id: u32, con_id: i64, sec_type: &str, exchange: &str, end_date_time: &str, duration: &str, use_rth: bool, hmds_conn: &mut Option<Connection>, hb: &mut HeartbeatState) {
        let qid = self.next_hmds_query_id;
        self.next_hmds_query_id += 1;
        let duration = crate::control::historical::normalize_duration(duration);
        let end_date_time = if end_date_time.is_empty() {
            chrono_free_timestamp().to_string()
        } else {
            end_date_time.to_string()
        };
        let query_id = format!("sched_{qid}");
        let xml = crate::control::historical::build_schedule_xml(
            &query_id, con_id, &end_date_time, &duration, use_rth,
            &hist_sec_type(sec_type), &hist_exchange(exchange),
        );
        if let Some(conn) = hmds_conn.as_mut() {
            let ts = chrono_free_timestamp();
            let _ = conn.send_fix(&[
                (fix::TAG_MSG_TYPE, "W"),
                (fix::TAG_SENDING_TIME, &ts),
                (6118, &xml),
            ]);
            hb.last_hmds_sent = Instant::now();
            log::info!("Sent schedule request: req_id={req_id} con_id={con_id}");
        }
        self.pending_schedule.push((query_id, req_id, end_date_time));
    }

    /// Every historical query still waiting on the venue.
    ///
    /// There used to be a sweep here that failed one after a minute of quiet,
    /// under the venue's own number for a historical error and with a bar
    /// answer of nothing to unblock whoever was waiting. Neither was the
    /// venue's: the reference client sets no deadline of its own on a
    /// historical query — the budget it carries is the widest a long can hold
    /// — and paces its requests so the limiter is not tripped in the first
    /// place. A minute is short for a deep query, so the sweep manufactured
    /// failures the venue never stated, under a number that made them
    /// indistinguishable from ones it did.
    ///
    /// A query whose connection goes away is still failed, where that is
    /// stated — see `fail_pending`.
    #[cfg(test)]
    pub(crate) fn pending_historical_count(&self) -> usize {
        self.pending_historical.len()
    }
}


/// What the venue says when it takes on a tick subscription.
///
/// It names the subscription back, states the number it will refer to it by on
/// every frame, and states the increments its prices and its sizes move in.
/// Those increments are the contract's own and are not stated anywhere else on
/// this connection.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TickSubscriptionAck {
    /// The name this client asked under.
    pub query_id: String,
    /// The number the venue will use on every frame.
    pub venue_id: u64,
    /// What a price moves in.
    pub min_tick: f64,
    /// What a size moves in. A share deals in whole ones; a crypto deals in
    /// hundred-millionths.
    pub size_min_tick: f64,
}

/// Read that acknowledgement, or nothing if the reply is some other answer.
pub(crate) fn parse_tick_subscription_ack(xml: &str) -> Option<TickSubscriptionAck> {
    if !xml.contains("<ResultSetTickerId>") {
        return None;
    }
    let field = |name: &str| -> Option<&str> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let at = xml.find(&open)? + open.len();
        let end = xml[at..].find(&close)? + at;
        Some(xml[at..end].trim())
    };
    Some(TickSubscriptionAck {
        query_id: field("id")?.to_string(),
        venue_id: field("rtTickerId")?.parse().ok()?,
        min_tick: field("minTick")?.parse().ok()?,
        // A reply stating no size increment states none rather than one.
        size_min_tick: field("sizeMinTick").and_then(|v| v.parse().ok()).unwrap_or(0.0),
    })
}

/// Turn a counted size into the form every reader divides by.
///
/// The venue counts a size in whatever it said sizes move in for the contract:
/// whole ones for a share, hundred-millionths for a crypto. Treating every
/// count as whole ones reports a crypto's size a hundred million times too
/// large; treating it as anything fixed is the same mistake in another
/// direction.
///
/// A subscription the venue stated no size increment for is counted in whole
/// ones, which is what it means to state none.
fn scaled_size(counted: u64, size_tick: f64) -> i64 {
    // Scaled from the count as sent, not from a count cut down to fit first.
    // Cutting it down first threw away what the increment would have shrunk
    // back into range, so a count above the ceiling came back as a plausible
    // number that was not the venue's. Scaling first and saturating on the
    // way out keeps every size a quantity can hold, and holds the rest at the
    // ceiling rather than wrapping to a negative — a negative size is a sell
    // where there was a buy.
    let per_unit = if size_tick > 0.0 { size_tick } else { 1.0 };
    (counted as f64 * per_unit * crate::types::QTY_SCALE as f64).round() as i64
}

#[cfg(test)]
mod tests;

/// Whether a response answers the query named by `qid`.
///
/// Read from the name the response states rather than searched for in the
/// payload. Searching matched any query whose name was a prefix of another:
/// with `tk_1` and `tk_12` both in flight, the answer to `tk_12` contains
/// `tk_1`, so it went to whichever of the two was waiting first and the other
/// was never answered at all.
///
/// The stated name is not always the bare one. A news reply carries what the
/// query asked for after it, separated from it — `news_2-headlines;;...` —
/// so a name the reply continues past is still that query's, unless what
/// follows reads as more of the name. `tk_1` and `tk_12` differ by a digit,
/// which is why a digit is not a separator.
fn answers(xml: &str, qid: &str) -> bool {
    let Some(stated) = crate::control::xml::tag(xml, "id") else {
        return false;
    };
    states(stated, qid)
}

/// Whether a name a reply stated is the query named by `qid`.
///
/// The rule [`answers`] applies, for the paths that have already read the name
/// out of the reply and hold it as a string. Kept in one place because a plain
/// prefix test here is the collision the doc above describes.
fn states(stated: &str, qid: &str) -> bool {
    match stated.strip_prefix(qid) {
        Some("") => true,
        Some(rest) => !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'),
        None => false,
    }
}
