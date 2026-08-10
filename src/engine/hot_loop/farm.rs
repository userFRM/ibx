use std::time::Instant;

use crate::bridge::{Event, SharedState};
use crate::config::chrono_free_timestamp;
use crate::engine::context::Context;
use crate::protocol::connection::{Connection, Frame};
use crate::protocol::fix;
use crate::protocol::fixcomp;
use crate::protocol::tick_decoder;
use crate::types::{qty_from_counted, InstrumentId};
use std::sync::mpsc::SyncSender;

use super::{HeartbeatState, ReplayPacing, emit, fast_extract_msg_type, find_body_after_tag};

/// Build the 35=V subscribe tag list for a contract whose conId is known.
///
/// Kept pure so the wire shape stays unit-testable. SecurityType (167) and
/// Exchange (207) must describe the actual contract: the server routes the
/// subscription by them, and a mismatch is answered with a partial ack rather
/// than an error.
fn build_conid_subscribe_tags(
    realtime: bool,
    bid_ask_id: u32,
    last_id: u32,
    con_id: i64,
    exchange: &str,
    sec_type: &str,
    mode_9887: i32,
    ts: &str,
) -> Vec<(u32, String)> {
    let con_id_str = (con_id as u32).to_string();
    // Subscribing by conId alone is a supported shape — `Contract` defaults
    // both fields to empty, and the in-tree benchmark does exactly that. Keep
    // the smart-routed stock those callers used to get rather than sending an
    // empty SecurityType and Exchange, which is the silent partial-ack this
    // change exists to remove.
    let exchange = if exchange.is_empty() { "SMART" } else { exchange };
    let sec_type = if sec_type.is_empty() { "STK" } else { sec_type };
    let fix_exchange = crate::control::contracts::exchange_to_fix(exchange);
    let fix_sec_type = crate::control::contracts::sec_type_to_fix(sec_type);

    // 146 = NoRelatedSym count: 2 entries for the realtime fan-out, 1 for TOP.
    let mut tags: Vec<(u32, String)> = vec![
        (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
        (fix::TAG_SENDING_TIME, ts.to_string()),
        (263, "1".to_string()),
        (146, if realtime { "2" } else { "1" }.to_string()),
    ];

    let entries: &[(u32, &str)] = if realtime {
        &[(bid_ask_id, "442"), (last_id, "443")]
    } else {
        &[(bid_ask_id, "1")]
    };
    for (req_id, depth) in entries {
        tags.push((262, req_id.to_string()));
        tags.push((6008, con_id_str.clone()));
        tags.push((207, fix_exchange.to_string()));
        tags.push((167, fix_sec_type.to_string()));
        tags.push((264, depth.to_string()));
        tags.push((6088, "Socket".to_string()));
        tags.push((9830, "1".to_string()));
        tags.push((9839, "1".to_string()));
        if !realtime {
            tags.push((9887, mode_9887.to_string()));
        }
    }
    tags
}

/// Option resub info: (instrument, symbol, exchange, sec_type, last_trade_date, strike, right, multiplier, mode_9887).
type MdResubInfo = (InstrumentId, String, String, String, String, f64, String, String, i32);

/// `MdResubInfo` with the instrument's resolved con_id spliced in behind it.
type MdResubTarget = (InstrumentId, i64, String, String, String, String, f64, String, String, i32);

pub(crate) struct FarmState {
    pub(crate) next_md_req_id: u32,
    pub(crate) md_req_to_instrument: Vec<(u32, InstrumentId)>,
    pub(crate) instrument_md_reqs: Vec<(InstrumentId, Vec<u32>)>,
    /// Active depth subscriptions: (req_id, is_smart_depth).
    pub(crate) depth_subs: Vec<(u32, bool)>,
    /// Maps server_tag → (depth_req_id, is_smart_depth, min_tick) for active depth subscriptions.
    pub(crate) depth_tag_to_req: Vec<(u32, u32, bool, f64, f64)>,
    /// SmartDepth fan-out: maps internal sub_req → user's original req_id.
    depth_fanout_map: Vec<(u32, u32)>,
    /// Primary depth subscription params for reconnect: (req_id, con_id, exchange, sec_type, num_rows, is_smart_depth).
    depth_resub_info: Vec<(u32, i64, String, String, i32, bool)>,
    md_resub_info: Vec<MdResubInfo>,
    /// The option-model subscriptions and what they were taken out on, so one
    /// can be withdrawn the same way it was asked for.
    greeks_subs: Vec<(u32, i64, String)>,
    /// What generic tick each request asked for, as (req_id, request type).
    /// The venue numbers a generic tick separately from the prices and states
    /// nothing on the frames themselves about which tick they carry, so the
    /// only thing that says what a frame holds is what was asked for under
    /// that number.
    generic_tick_reqs: Vec<(u32, u32)>,
    /// The venue's number for a generic-tick subscription, and what it
    /// carries: (server tag, request type, instrument).
    generic_tick_tags: Vec<(u32, u32, InstrumentId)>,
    /// Message types the venue has sent on this connection that nothing reads.
    unread_types: std::collections::HashSet<String>,
    pub(crate) disconnected: bool,
    pub(crate) tick_buf: Vec<tick_decoder::RawTick>,
    pub(crate) farm_msg_buf: Vec<Vec<u8>>,
}

/// The venue's option model, subscribed to by naming the model where a price
/// subscription names an exchange, and the model's own tick where a price
/// subscription names a request type. One subscription per option.
fn build_greeks_subscribe_tags(req_id: u32, con_id: i64, sec_type: &str, ts: &str) -> Vec<(u32, String)> {
    let fix_sec_type = crate::control::contracts::sec_type_to_fix(sec_type);
    vec![
        (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
        (fix::TAG_SENDING_TIME, ts.to_string()),
        (263, "1".to_string()),
        (146, "1".to_string()),
        (262, req_id.to_string()),
        (6008, (con_id as u32).to_string()),
        (207, GREEKS_VENUE.to_string()),
        (167, fix_sec_type.to_string()),
        (264, GREEKS_REQUEST_TYPE.to_string()),
        (6088, "Socket".to_string()),
        (9830, "1".to_string()),
        (9839, "1".to_string()),
    ]
}

/// Whether a venue is trading a contract, subscribed to the same way the option
/// model is — its own tick where a price subscription names a request type.
///
/// Unlike the model, this names the contract's **own** exchange rather than a
/// stand-in: the model and the news feed are the exceptions that go by a name of
/// their own, and everything else is asked for where it trades.
fn build_trading_status_subscribe_tags(
    req_id: u32,
    con_id: i64,
    sec_type: &str,
    exchange: &str,
    ts: &str,
) -> Vec<(u32, String)> {
    let fix_sec_type = crate::control::contracts::sec_type_to_fix(sec_type);
    vec![
        (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
        (fix::TAG_SENDING_TIME, ts.to_string()),
        (263, "1".to_string()),
        (146, "1".to_string()),
        (262, req_id.to_string()),
        (6008, (con_id as u32).to_string()),
        (207, exchange.to_string()),
        (167, fix_sec_type.to_string()),
        (264, TRADING_STATUS_REQUEST_TYPE.to_string()),
        (6088, "Socket".to_string()),
        (9830, "1".to_string()),
        (9839, "1".to_string()),
    ]
}

/// The trading-status tick's own number, in place of a request type.
const TRADING_STATUS_REQUEST_TYPE: u32 = 437;

/// What the venue counts an instrument's sizes in, as its acknowledgement
/// states it: the last field, after the increment prices move in.
///
/// A size on the wire is a count of these. Whole ones for a share, and
/// hundred-millionths for a crypto — so a count taken as whole ones reports
/// one of the two a hundred million times over. An acknowledgement that
/// states none is counted in whole ones, which is what stating none means.
fn trailing_size_increment(parts: &[&str]) -> Option<f64> {
    let stated: f64 = parts.last()?.trim().parse().ok()?;
    (stated > 0.0).then_some(stated)
}


/// What the option model goes by where an exchange would be named.
const GREEKS_VENUE: &str = "IBVOL";

/// One venue refusing to show its book. Not the end of the request: this
/// client asks several venues for one book and the others may answer.
const DEPTH_VENUE_REFUSED: i32 = 321;

/// The option model's own tick, in place of a request type.
const GREEKS_REQUEST_TYPE: u32 = 732;


/// The news tick's own number, in place of a request type.
const NEWS_REQUEST_TYPE: u32 = 292;

/// One generic tick as the venue frames it.
///
/// The envelope opens with the length of everything after it, in bits, then
/// the venue's number for the subscription, then the payload's length in
/// bytes and the payload. Nothing on it says which tick it carries.
///
/// The leading length was read as a kind for a while, which worked only
/// because each tick's payload happened to be a fixed size: an option model
/// is a hundred and twenty-four bytes and so always announced the same
/// number. Any other tick of that size would have been read as an option
/// model, and a model of any other size would not have been read at all.
struct GenericTickFrame<'a> {
    server_tag: u32,
    payload: &'a [u8],
}

impl<'a> GenericTickFrame<'a> {
    /// Read the envelope, or nothing where it does not hold together.
    ///
    /// The frame states its length twice — once for the whole of it and once
    /// for the payload — and the two have to agree. That is also what says
    /// whether the payload's length is stated in one byte or two: the reading
    /// that squares with the whole is the right one.
    fn read(body: &'a [u8]) -> Option<Self> {
        let stated_bits = u16::from_be_bytes([*body.first()?, *body.get(1)?]) as usize;
        if !stated_bits.is_multiple_of(8) {
            return None;
        }
        let stated = stated_bits / 8;
        let frame = body.get(2..2 + stated)?;
        let server_tag = u32::from_be_bytes([
            *frame.first()?,
            *frame.get(1)?,
            *frame.get(2)?,
            *frame.get(3)?,
        ]);
        // One byte of length, then two, and whichever squares with the whole
        // frame is the one the venue wrote.
        let short = *frame.get(4)? as usize;
        if 5 + short == stated {
            return Some(Self { server_tag, payload: frame.get(5..)? });
        }
        let long = u16::from_be_bytes([*frame.get(4)?, *frame.get(5)?]) as usize;
        if 6 + long == stated {
            return Some(Self { server_tag, payload: frame.get(6..)? });
        }
        log::debug!(
            "A generic tick under server tag {server_tag} states {stated} bytes and a payload \
             that fits neither reading; dropping it rather than reading past its end",
        );
        None
    }
}

/// What the venue sent of its option model.
///
/// A set of flags says which fields the payload carries, then one price that
/// is always there, then the stated fields in a fixed order that is not the
/// order of the flags: the time value is flagged first and read last. A field
/// whose flag is clear occupies no bytes at all, so the payload's length
/// varies with what was stated.
fn decode_greeks(payload: &[u8]) -> Option<crate::types::OptionComputation> {
    const VALID: u32 = 1 << 0;
    const TIME_VALUE: u32 = 1 << 13;
    const DELTA: u32 = 1 << 16;
    const GAMMA: u32 = 1 << 17;
    const VEGA: u32 = 1 << 18;
    const RHO: u32 = 1 << 19;
    const THETA: u32 = 1 << 20;
    const FUGIT: u32 = 1 << 21;
    const BOUNDARY: u32 = 1 << 22;
    const FORWARD_COEFF: u32 = 1 << 23;
    const UNDERLYING_PRICE: u32 = 1 << 25;
    const IMPLIED_VOL: u32 = 1 << 26;
    const CALENDAR_DAYS: u32 = 1 << 27;
    const DAILY_RATE: u32 = 1 << 28;
    const MODEL_YIELD: u32 = 1 << 29;
    const BRIDGE_YIELD: u32 = 1 << 30;
    // A value the venue did not state. The reference client's own mark, so a
    // caller can tell an unstated field from a real zero — and zero is a real
    // greek.
    const UNSTATED: f64 = f64::MAX;

    if payload.len() < 12 {
        return None;
    }
    let flags = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    // The first flag gates the whole payload: without it nothing was computed
    // and nothing after it is a number.
    if flags & VALID == 0 {
        return None;
    }
    let mut pos = 4;
    let mut next = |stated: bool| -> f64 {
        if !stated {
            return UNSTATED;
        }
        let Some(bytes) = payload.get(pos..pos + 8) else { return UNSTATED };
        pos += 8;
        f64::from_be_bytes(bytes.try_into().unwrap_or_default())
    };

    // The model's own price for the option, stated whatever else is.
    let opt_price = next(true);
    let delta = next(flags & DELTA != 0);
    let gamma = next(flags & GAMMA != 0);
    let vega = next(flags & VEGA != 0);
    let _rho = next(flags & RHO != 0);
    let theta = next(flags & THETA != 0);
    let _fugit = next(flags & FUGIT != 0);
    let _boundary = next(flags & BOUNDARY != 0);
    let _forward_coeff = next(flags & FORWARD_COEFF != 0);
    let und_price = next(flags & UNDERLYING_PRICE != 0);
    let implied_vol = next(flags & IMPLIED_VOL != 0);
    let _calendar_days = next(flags & CALENDAR_DAYS != 0);
    let _daily_rate = next(flags & DAILY_RATE != 0);
    let _model_yield = next(flags & MODEL_YIELD != 0);
    let _bridge_yield = next(flags & BRIDGE_YIELD != 0);
    let _time_value = next(flags & TIME_VALUE != 0);

    Some(crate::types::OptionComputation {
        instrument: 0,
        implied_vol,
        delta,
        opt_price,
        // The venue states no present value of dividends on this tick.
        pv_dividend: UNSTATED,
        gamma,
        vega,
        theta,
        und_price,
    })
}

impl FarmState {
    /// Whether this instrument still has market-data state that would be
    /// repointed by a slot reuse: a live subscription, or a record kept for the
    /// next reconnect.
    ///
    /// The resubscribe record is the reason this matters. Before it existed a
    /// reclaimed slot replayed nothing; now it replays the old contract's
    /// descriptor against whatever contract holds the id at reconnect (ibx#288).
    ///
    /// `md_req_to_instrument` is deliberately not consulted. A subscribe fills
    /// it and `instrument_md_reqs` together, so it says nothing the first check
    /// does not already cover — and an entry left there after an unsubscribe is
    /// a defect in its own right (ibx#289), not a reason to pin the slot. Held
    /// on that basis, a subscribe/unsubscribe cycle would consume a slot
    /// permanently and the instrument cap would become cumulative per session,
    /// which is the failure ibx#233 exists to prevent.
    pub(crate) fn holds_market_data(&self, instrument: InstrumentId) -> bool {
        self.instrument_md_reqs.iter().any(|(id, _)| *id == instrument)
            || self.md_resub_info.iter().any(|r| r.0 == instrument)
    }

    pub(crate) fn new() -> Self {
        Self {
            next_md_req_id: 1,
            md_req_to_instrument: Vec::new(),
            instrument_md_reqs: Vec::new(),
            depth_subs: Vec::new(),
            depth_tag_to_req: Vec::new(),
            depth_fanout_map: Vec::new(),
            depth_resub_info: Vec::new(),
            md_resub_info: Vec::new(),
            greeks_subs: Vec::new(),
            generic_tick_reqs: Vec::new(),
            generic_tick_tags: Vec::new(),
            unread_types: std::collections::HashSet::new(),
            disconnected: false,
            tick_buf: Vec::with_capacity(16),
            farm_msg_buf: Vec::with_capacity(32),
        }
    }

    pub(crate) fn poll_market_data(
        &mut self,
        farm_conn: &mut Option<Connection>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        hb: &mut HeartbeatState,
    ) {
        if self.disconnected {
            return;
        }
        self.farm_msg_buf.clear();
        {
            let conn = match farm_conn.as_mut() {
                None => return,
                Some(c) => c,
            };
            match conn.try_recv() {
                Ok(0) => return,
                Err(e) => {
                    log::error!("Farm connection lost: {e}");
                    self.handle_disconnect(context, event_tx);
                    return;
                }
                Ok(n) => {
                    log::trace!("Farm recv: {} bytes, buffered: {}", n, conn.buffered());
                    let now = Instant::now();
                    hb.last_farm_recv = now;
                    context.recv_at = now;
                    hb.pending_farm_test = None;
                }
            }
            let frames = conn.extract_frames();
            log::trace!("Farm frames: {}", frames.len());
            for frame in &frames {
                match frame {
                    Frame::FixComp(raw) => {
                        let Some(unsigned) = conn.unsign(raw) else { continue };
                        match fixcomp::fixcomp_decompress(&unsigned) {
                            Ok(inner) => {
                                if log::log_enabled!(log::Level::Trace) {
                                    for m in &inner {
                                        log::trace!("WIRE< farm/comp {}", fix::fmt_pipe(m));
                                    }
                                }
                                self.farm_msg_buf.extend(inner);
                            }
                            Err(e) => {
                                log::warn!(
                                    "Farm: dropping malformed FIXCOMP frame ({} bytes): {}",
                                    unsigned.len(), e,
                                );
                            }
                        }
                    }
                    Frame::Binary(raw) => {
                        let Some(unsigned) = conn.unsign(raw) else { continue };
                        if log::log_enabled!(log::Level::Trace) {
                            log::trace!("WIRE< farm/bin {}", fix::fmt_pipe(&unsigned));
                        }
                        self.farm_msg_buf.push(unsigned);
                    }
                    Frame::Fix(raw) => {
                        let Some(unsigned) = conn.unsign(raw) else { continue };
                        if log::log_enabled!(log::Level::Trace) {
                            log::trace!("WIRE< farm/fix {}", fix::fmt_pipe(&unsigned));
                        }
                        self.farm_msg_buf.push(unsigned);
                    }
                    Frame::Control(_) => {
                        // 8=1 / 8=X control state — not consumed on the farm path (ibx#185).
                    }
                }
            }
        }

        let mut msgs = std::mem::take(&mut self.farm_msg_buf);
        for msg in &msgs {
            self.process_farm_message(msg, farm_conn, context, shared, event_tx, hb);
        }
        msgs.clear();
        self.farm_msg_buf = msgs;
    }

    pub(crate) fn process_farm_message(
        &mut self,
        msg: &[u8],
        farm_conn: &mut Option<Connection>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        hb: &mut HeartbeatState,
    ) {
        let msg_type = match fast_extract_msg_type(msg) {
            Some(t) => t,
            None => return,
        };
        // Every message this connection carries, kept whole when asked. What a
        // subscription answers with is a question the wire answers; a reading
        // of it is not evidence of it.
        if std::env::var("IBX_CAPTURE_WIRE").is_ok() {
            let hex: String = msg.iter().map(|b| format!("{b:02x}")).collect();
            shared.market.note_unread_wire("farm-msg", hex);
        }
        match msg_type {
            b"P" => self.handle_tick_data(msg, context, shared, event_tx),
            b"Q" => {
                log::info!("Farm 35=Q subscription ack received");
                self.handle_subscription_ack(msg, context);
            }
            b"0" => {}
            b"1" => {
                let parsed = fix::fix_parse(msg);
                let test_id = parsed.get(&fix::TAG_TEST_REQ_ID).cloned().unwrap_or_default();
                if let Some(conn) = farm_conn.as_mut() {
                    let ts = chrono_free_timestamp();
                    let result = conn.send_fix(&[
                        (fix::TAG_MSG_TYPE, fix::MSG_HEARTBEAT),
                        (fix::TAG_SENDING_TIME, &ts),
                        (fix::TAG_TEST_REQ_ID, &test_id),
                    ]);
                    log::info!("Farm TestReq '{}' -> heartbeat response seq={} result={:?}",
                        test_id, conn.seq, result);
                    hb.last_farm_sent = Instant::now();
                }
            }
            b"L" => self.handle_ticker_setup(msg, context),
            b"UT" | b"UM" | b"RL" => super::ccp::handle_account_update(msg, context, shared),
            b"UP" => {
                let parsed = fix::fix_parse(msg);
                super::ccp::handle_position_update(&parsed, context, shared, event_tx);
            }
            // The venue refusing a subscription it was asked for, naming the
            // request that asked. Dropped, a caller that asked for depth on a
            // venue this account cannot see waits for data that was refused
            // before it started.
            b"3" => self.handle_subscription_reject(msg, context, shared),
            b"Y" => self.handle_depth_35y(msg, shared),
            b"G" => self.handle_generic_tick(msg, shared, event_tx),
            // Named once, the first time each arrives, the way the trading
            // connection names what it does not read. Reported where nobody
            // looks, a type the venue sends and this client drops is
            // indistinguishable from one it never sends.
            other => {
                let named = String::from_utf8_lossy(other).to_string();
                if self.unread_types.insert(named.clone()) {
                    shared.market.note_unread_wire("market data", format!("type {named}"));
                    log::info!(
                        "Unread on the market data connection: type {named}, {} bytes. Nothing \
                         here reads it, so whatever it carries is being discarded",
                        msg.len(),
                    );
                }
            }
        }
    }

    fn handle_tick_data(&mut self, msg: &[u8], context: &mut Context, shared: &SharedState, event_tx: &Option<SyncSender<Event>>) {
        let body = match find_body_after_tag(msg, b"35=P\x01") {
            Some(b) => b,
            None => return,
        };

        // Depth 35=P entries may be interleaved with L1 tick entries in the same body.
        if !self.depth_tag_to_req.is_empty() {
            let mut has_depth = false;
            let mut off = 0;
            while off + 3 < body.len() {
                if body[off] == 0x00 {
                    let stag = ((body[off+1] as u32) << 16) | ((body[off+2] as u32) << 8) | (body[off+3] as u32);
                    if self.depth_tag_to_req.iter().any(|(s, ..)| *s == stag) {
                        has_depth = true;
                        break;
                    }
                }
                off += 1;
            }
            if has_depth {
                self.handle_depth_35p(body, shared);
                // Don't return — also process L1 ticks from same body below
            }
        }

        let mut ticks = std::mem::take(&mut self.tick_buf);
        tick_decoder::decode_ticks_35p_into(body, &mut ticks);
        let mut notified = [0u64; crate::types::MAX_INSTRUMENTS / 64];

        // Every entry as decoded, before the apply loop below drops the types it
        // does not map. A field that arrives but is unmapped otherwise leaves no
        // trace anywhere, which is what let the identities above go wrong; this
        // is the measurement that settles what a given wire number carries.
        if log::log_enabled!(log::Level::Trace) {
            for tick in &ticks {
                log::trace!("35=P raw: server_tag={} type={} magnitude={}",
                    tick.server_tag, tick.tick_type, tick.magnitude);
            }
        }

        // Phase 1: Apply all ticks to internal quotes before publishing.
        for tick in &ticks {
            let instrument = match context.market.instrument_by_server_tag(tick.server_tag) {
                Some(id) => id,
                None => continue,
            };

            let mts = context.market.min_tick_scaled(instrument);
            // A size is a count of what the venue said sizes move in for this
            // contract, the same way a price is a count of what prices move in.
            let size_tick = context.market.size_tick(instrument);
            let q = context.market.quote_mut(instrument);

            // The extended 35=P format carries a full 8-bit byte width, so a
            // magnitude can reach i64::MAX and the scaling multiply wrapped to
            // an arbitrary price in release builds (ibx#272). Drop the price
            // rather than pinning it: a saturated value is ~92e9 at
            // PRICE_SCALE, which reads downstream as an ordinary quote, and
            // leaving the previous one standing is the honest failure.
            let scaled = |m: i64| m.checked_mul(mts);
            match tick.tick_type {
                // Opcode 18 is deliberately not read as a halt. It was named
                // for one on no evidence, and the venue states a halt
                // elsewhere — as a generic tick carrying a status mask. A
                // wrong halt is worse than none: a caller told a contract is
                // trading when it is not prices against a book that is not
                // there.
                tick_decoder::O_BID_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.bid = v,
                        None => log::warn!("35=P bid price out of range (magnitude={}), tick dropped", tick.magnitude),
                    }
                }
                tick_decoder::O_ASK_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.ask = v,
                        None => log::warn!("35=P ask price out of range (magnitude={}), tick dropped", tick.magnitude),
                    }
                }
                tick_decoder::O_LAST_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.last = v,
                        None => log::warn!("35=P last price out of range (magnitude={}), tick dropped", tick.magnitude),
                    }
                }
                tick_decoder::O_HIGH_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.high = v,
                        None => log::warn!("35=P high price out of range (magnitude={}), tick dropped", tick.magnitude),
                    }
                }
                tick_decoder::O_LOW_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.low = v,
                        None => log::warn!("35=P low price out of range (magnitude={}), tick dropped", tick.magnitude),
                    }
                }
                tick_decoder::O_OPEN_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.open = v,
                        None => log::warn!("35=P open price out of range (magnitude={}), tick dropped", tick.magnitude),
                    }
                }
                tick_decoder::O_CLOSE_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.close = v,
                        None => log::warn!("35=P close price out of range (magnitude={}), tick dropped", tick.magnitude),
                    }
                }
                // Quantities are fixed-point, the same way prices are; every
                // reader divides by `QTY_SCALE` on the way out (ibx#287).
                tick_decoder::O_BID_SIZE => { q.bid_size = qty_from_counted(tick.magnitude, size_tick); }
                tick_decoder::O_ASK_SIZE => { q.ask_size = qty_from_counted(tick.magnitude, size_tick); }
                tick_decoder::O_LAST_SIZE => { q.last_size = qty_from_counted(tick.magnitude, size_tick); }
                tick_decoder::O_VOLUME => { q.volume = qty_from_counted(tick.magnitude, size_tick); }
                // Type 20 carries Unix seconds. Guarded because the same
                // type also carried a `yyyymmdd` value in capture, and a date
                // read as an epoch is worse than no timestamp. Type 21 is a
                // per-second offset against it and is left undecoded until a
                // capture settles how the two combine (ibx#303).
                // Type 23 was previously folded in here and is now dropped:
                // it did not appear once in 733 captured entries on a future,
                // and it was writing a raw magnitude of unknown unit into a
                // nanosecond field. Left unmapped until a capture identifies
                // it rather than guessed at (ibx#303).
                tick_decoder::O_TS_BASE if tick.magnitude > 1_000_000_000 => {
                    q.timestamp_ns = (tick.magnitude as u64).saturating_mul(1_000_000_000);
                }
                tick_decoder::O_BID_EXCH => { q.bid_exch_mask = tick.magnitude; }
                tick_decoder::O_ASK_EXCH => { q.ask_exch_mask = tick.magnitude; }
                tick_decoder::O_LAST_EXCH => { q.last_exch_mask = tick.magnitude; }
                _ => {}
            }

            notified[(instrument >> 6) as usize] |= 1u64 << (instrument & 63);
        }

        // Phase 2: Publish complete quotes after all ticks in the batch are applied.
        for (word_idx, &word) in notified.iter().enumerate() {
            let mut remaining = word;
            while remaining != 0 {
                let instrument = (word_idx as u32) * 64 + remaining.trailing_zeros();
                remaining &= remaining - 1;
                shared.market.push_quote(instrument, context.quote(instrument));
                emit(event_tx, Event::Tick(instrument));
            }
        }
        self.tick_buf = ticks;
    }

    fn handle_subscription_ack(&mut self, msg: &[u8], context: &mut Context) {
        let body = match find_body_after_tag(msg, b"35=Q\x01") {
            Some(b) => b,
            None => return,
        };
        let text = String::from_utf8_lossy(body);
        let text = text.split("\x018349=").next().unwrap_or(&text);
        let parts: Vec<&str> = text.trim().split(',').collect();
        if parts.len() < 3 { return; }
        let server_tag: u32 = match parts[0].parse() { Ok(v) => v, Err(_) => return };
        let req_id: u32 = match parts[1].parse() { Ok(v) => v, Err(_) => return };
        // The venue states the increment; a penny is not stood in where it did
        // not. Every price on this instrument is scaled by it, so a wrong one
        // is wrong prices rather than a wrong field.
        let min_tick: f64 = match parts[2].parse() {
            Ok(v) => v,
            Err(_) => {
                log::warn!(
                    "a subscription was acknowledged with an increment that cannot be \
                     read ({}), so this instrument has none and its prices cannot be \
                     worked out",
                    parts[2],
                );
                return;
            }
        };

        // Depth ack: always map the server_tag if this req_id is a depth subscription,
        // even when depth_levels=0 (book empty now but updates may arrive later).
        let depth_levels: i32 = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        if let Some((_, is_smart)) = self.depth_subs.iter().find(|(id, _)| *id == req_id) {
            let is_smart = *is_smart;
            // For SmartDepth fan-out, map back to the user's original req_id
            let user_req = self.depth_fanout_map.iter()
                .find(|(sub, _)| *sub == req_id)
                .map(|(_, user)| *user)
                .unwrap_or(req_id);
            self.depth_tag_to_req.push((
                server_tag,
                user_req,
                is_smart,
                min_tick,
                trailing_size_increment(&parts).unwrap_or(1.0),
            ));
            log::info!("Depth ack: server_tag {server_tag} -> req_id {user_req} (levels={depth_levels}, smart={is_smart}, min_tick={min_tick})");
            return;
        }

        // L1 ack
        let instrument = match self.md_req_to_instrument.iter()
            .position(|(id, _)| *id == req_id)
        {
            Some(idx) => {
                let (_, instr) = self.md_req_to_instrument.remove(idx);
                instr
            }
            None => return,
        };

        // A generic tick is numbered apart from the prices, and its frames say
        // nothing about which tick they carry. What was asked for under this
        // request is the only thing that does, so the two are recorded
        // together here and read back when a frame arrives.
        if let Some((_, request_type)) =
            self.generic_tick_reqs.iter().find(|(id, _)| *id == req_id).copied()
        {
            self.generic_tick_tags.retain(|(tag, ..)| *tag != server_tag);
            self.generic_tick_tags.push((server_tag, request_type, instrument));
            log::info!(
                "Generic tick {request_type} on instrument {instrument} -> server_tag {server_tag}"
            );
            return;
        }

        context.market.register_server_tag(server_tag, instrument);
        context.market.set_min_tick(instrument, min_tick);
        if let Some(size_tick) = trailing_size_increment(&parts) {
            context.market.set_size_tick(instrument, size_tick);
        }
        log::info!("Subscribed instrument {instrument} -> server_tag {server_tag}, minTick {min_tick}");
    }

    /// The venue refusing something this connection asked for.
    ///
    /// It names the request on tag 262 and says why on tag 58, so unlike the
    /// trading connection's own error channel this one can be handed back to
    /// the caller that asked. A refusal read as nothing leaves that caller
    /// waiting on data the venue already said it would not send.
    fn handle_subscription_reject(&mut self, msg: &[u8], context: &Context, shared: &SharedState) {
        let parsed = fix::fix_parse(msg);
        let said = parsed.get(&58).map(String::as_str).unwrap_or("");
        if said.is_empty() {
            return;
        }
        // The venue writes these as "Error&VENUE/TYPE/WHAT". The lead-in says
        // only that it is one, which the caller already knows from being told.
        let reason = said.strip_prefix("Error&").unwrap_or(said);
        let req_id = parsed.get(&262).and_then(|s| s.parse::<u32>().ok());
        let instrument = req_id.and_then(|rid| {
            self.md_req_to_instrument.iter().find(|(id, _)| *id == rid).map(|(_, i)| *i)
        });
        match instrument {
            Some(instrument) => {
                let named = context.market.symbol(instrument);
                log::warn!("The venue refused a subscription on {named}: {reason}");
                shared.market.push_subscription_failure(
                    instrument, format!("the venue refused this subscription: {reason}"),
                );
            }
            // A depth subscription asks under an id of its own, and one venue's
            // refusal does not end the request: this client asks several venues
            // for one book and the others may answer. So the caller is told
            // which venue refused, and the request stands.
            None => {
                let asked_for = req_id
                    .and_then(|rid| {
                        self.depth_fanout_map.iter().find(|(sub, _)| *sub == rid).map(|(_, u)| *u)
                    })
                    .or(req_id);
                log::warn!("The venue refused a subscription: {reason}");
                if let Some(rid) = asked_for {
                    shared.reference.push_historical_error(
                        rid,
                        DEPTH_VENUE_REFUSED,
                        format!("the venue refused depth here: {reason}"),
                    );
                }
            }
        }
    }

    fn handle_ticker_setup(&mut self, msg: &[u8], context: &mut Context) {
        let body = match find_body_after_tag(msg, b"35=L\x01") {
            Some(b) => b,
            None => return,
        };
        let text = String::from_utf8_lossy(body);
        let text = text.split("\x018349=").next().unwrap_or(&text);
        let parts: Vec<&str> = text.trim().split(',').collect();
        if parts.len() < 3 { return; }
        let con_id: i64 = match parts[0].parse() { Ok(v) => v, Err(_) => return };
        let min_tick: f64 = match parts[1].parse() {
            Ok(v) => v,
            Err(_) => {
                log::warn!(
                    "a depth subscription was acknowledged with an increment that \
                     cannot be read ({}), so this instrument has none",
                    parts[1],
                );
                return;
            }
        };
        let server_tag: u32 = match parts[2].parse() { Ok(v) => v, Err(_) => return };

        if let Some(instrument) = context.market.instrument_by_con_id(con_id) {
            context.market.register_server_tag(server_tag, instrument);
            context.market.set_min_tick(instrument, min_tick);
            if let Some(size_tick) = trailing_size_increment(&parts) {
                context.market.set_size_tick(instrument, size_tick);
            }
            log::info!("Ticker setup: con_id {con_id} -> server_tag {server_tag}, minTick {min_tick}");
        }
    }

    pub(crate) fn send_mktdata_subscribe(
        &mut self,
        con_id: i64,
        symbol: &str,
        exchange: &str,
        sec_type: &str,
        last_trade_date: &str,
        strike: f64,
        right: &str,
        multiplier: &str,
        instrument: InstrumentId,
        mode_9887: i32,
        farm_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        // Realtime fans out into BID_ASK + LAST; frozen/delayed/delayed-frozen
        // collapse to a single 264=1 (TOP) sub with 9887=mode_9887.
        let realtime = mode_9887 == 0;
        let bid_ask_id = self.next_md_req_id;
        let last_id = self.next_md_req_id + 1;
        if realtime {
            self.next_md_req_id += 2;
        } else {
            self.next_md_req_id += 1;
        }

        // An option is worth asking the venue to model. Anything else has no
        // volatility to imply, and the venue answers such a request with
        // nothing at all.
        let models_a_volatility = matches!(
            crate::control::contracts::sec_type_to_fix(sec_type),
            "OPT" | "FOP" | "IOPT" | "WAR",
        );
        let greeks_req_id = if models_a_volatility && con_id > 0 {
            let id = self.next_md_req_id;
            self.next_md_req_id += 1;
            Some(id)
        } else {
            None
        };

        let status_req_id = self.next_md_req_id;
        self.next_md_req_id += 1;

        self.md_req_to_instrument.push((bid_ask_id, instrument));
        self.md_req_to_instrument.push((status_req_id, instrument));
        self.generic_tick_reqs.push((status_req_id, TRADING_STATUS_REQUEST_TYPE));
        if realtime {
            self.md_req_to_instrument.push((last_id, instrument));
        }
        if let Some(id) = greeks_req_id {
            self.md_req_to_instrument.push((id, instrument));
            self.generic_tick_reqs.push((id, GREEKS_REQUEST_TYPE));
            // Recorded whether or not the farm is up: what was asked for is
            // bookkeeping, and a cancel has to find it either way.
            self.greeks_subs.push((id, con_id, sec_type.to_string()));
        }

        match self.instrument_md_reqs.iter_mut().find(|(id, _)| *id == instrument) {
            Some((_, reqs)) => {
                reqs.push(bid_ask_id);
                reqs.push(status_req_id);
                if realtime { reqs.push(last_id); }
                if let Some(id) = greeks_req_id { reqs.push(id); }
            }
            None => {
                let mut reqs = if realtime { vec![bid_ask_id, last_id, status_req_id] } else { vec![bid_ask_id, status_req_id] };
                if let Some(id) = greeks_req_id { reqs.push(id); }
                self.instrument_md_reqs.push((instrument, reqs));
            }
        }
        if self.md_resub_info.iter().all(|(id, ..)| *id != instrument) {
            self.md_resub_info.push((instrument, symbol.to_string(), exchange.to_string(), sec_type.to_string(), last_trade_date.to_string(), strike, right.to_string(), multiplier.to_string(), mode_9887));
        }

        if let Some(conn) = farm_conn.as_mut() {
            let bid_ask_str = bid_ask_id.to_string();
            let last_str = last_id.to_string();
            let mode_str = mode_9887.to_string();
            let ts = chrono_free_timestamp();

            // 146 = NoRelatedSym count: 2 entries for realtime fan-out, 1 for TOP.
            let no_related_sym = if realtime { "2" } else { "1" };

            let fix_exchange = crate::control::contracts::exchange_to_fix(exchange);
            let fix_sec_type = crate::control::contracts::sec_type_to_fix(sec_type);

            // When con_id is known the server still routes the subscription by
            // SecurityType (167) and Exchange (207), so both must describe the
            // actual contract. When con_id is 0, include the descriptive fields
            // as well so the server can resolve by description.
            if con_id > 0 {
                let tags = build_conid_subscribe_tags(
                    realtime, bid_ask_id, last_id, con_id, exchange, sec_type, mode_9887, &ts,
                );
                let refs: Vec<(u32, &str)> =
                    tags.iter().map(|(tag, val)| (*tag, val.as_str())).collect();
                let _ = conn.send_fixcomp(&refs);

                // The venue's option model is a subscription of its own, not a
                // field on the price one: it names the model in place of an
                // exchange and its own tick in place of a request type. A
                // subscription that does not ask for it is sent prices alone.
                if let Some(greeks_id) = greeks_req_id {
                    let tags = build_greeks_subscribe_tags(greeks_id, con_id, sec_type, &ts);
                    let refs: Vec<(u32, &str)> =
                        tags.iter().map(|(tag, val)| (*tag, val.as_str())).collect();
                    let _ = conn.send_fixcomp(&refs);
                }

                // Whether the venue is trading the contract at all. A halt
                // changes what every price on this subscription means — the
                // ones standing are from before it stopped — so it is asked for
                // alongside them rather than left to a caller to think of.
                //
                // Asked for under the same request as the prices, so a caller
                // reading a halt reads it against the quote it belongs to.
                let tags = build_trading_status_subscribe_tags(
                    status_req_id, con_id, sec_type, exchange, &ts,
                );
                let refs: Vec<(u32, &str)> =
                    tags.iter().map(|(tag, val)| (*tag, val.as_str())).collect();
                let _ = conn.send_fixcomp(&refs);
            } else {
                // No con_id — send descriptive fields
                let strike_str = if strike > 0.0 { strike.to_string() } else { String::new() };
                let mut tags: Vec<(u32, &str)> = vec![
                    (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                    (fix::TAG_SENDING_TIME, &ts),
                    (263, "1"),
                    (146, no_related_sym),
                ];
                let entries: &[(&String, &str)] = if realtime {
                    &[(&bid_ask_str, "442"), (&last_str, "443")]
                } else {
                    &[(&bid_ask_str, "1")]
                };
                for (req_str, depth) in entries {
                    tags.push((262, req_str));
                    tags.push((55, symbol));
                    tags.push((207, fix_exchange));
                    tags.push((167, fix_sec_type));
                    if !last_trade_date.is_empty() { tags.push((200, last_trade_date)); }
                    if strike > 0.0 { tags.push((202, &strike_str)); }
                    if !right.is_empty() { tags.push((201, right)); }
                    if !multiplier.is_empty() { tags.push((231, multiplier)); }
                    tags.push((264, depth));
                    tags.push((6088, "Socket"));
                    tags.push((9830, "1"));
                    tags.push((9839, "1"));
                    if !realtime { tags.push((9887, &mode_str)); }
                }
                let _ = conn.send_fixcomp(&tags);
            }
            if realtime {
                log::info!("Sent 35=V subscribe: con_id={} sec_type={} ids={},{} seq={}",
                    con_id, sec_type, bid_ask_id, last_id, conn.seq);
            } else {
                log::info!("Sent 35=V subscribe (9887={}): con_id={} sec_type={} id={} seq={}",
                    mode_9887, con_id, sec_type, bid_ask_id, conn.seq);
            }
            hb.last_farm_sent = Instant::now();
        }
    }

    pub(crate) fn send_mktdata_unsubscribe(
        &mut self,
        instrument: InstrumentId,
        farm_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        // Drop the resubscribe record first. The lookup below early-returns
        // when the instrument has no active requests, which is always the case
        // while the farm is down — `handle_disconnect` cleared that list — so
        // an unsubscribe issued during an outage would otherwise leave the
        // record standing and the reconnect would re-subscribe an instrument
        // the caller explicitly cancelled (ibx#288).
        self.md_resub_info.retain(|(id, ..)| *id != instrument);
        let reqs = match self.instrument_md_reqs.iter()
            .position(|(id, _)| *id == instrument)
        {
            Some(idx) => {
                let (_, reqs) = self.instrument_md_reqs.remove(idx);
                reqs
            }
            None => return,
        };
        self.md_resub_info.retain(|(id, ..)| *id != instrument);
        // Forget the pending acks too. A `35=Q` still in flight when the
        // unsubscribe goes out would otherwise resolve its request id after
        // the slot has been reclaimed and reused, binding this subscription's
        // server_tag AND its minTick onto whatever contract now holds the
        // slot — prices for the new contract then scale by the old one's tick
        // size, which reads as plausible rather than broken (ibx#289).
        self.md_req_to_instrument.retain(|(req_id, _)| !reqs.contains(req_id));

        // Take the option-model records before the connection is checked. With
        // the farm down there is nothing to send, and a record left behind
        // would outlive the subscription it describes.
        let mut withdrawn: Vec<(u32, i64, String)> = Vec::new();
        self.greeks_subs.retain(|entry| {
            if reqs.contains(&entry.0) {
                withdrawn.push(entry.clone());
                false
            } else {
                true
            }
        });

        let conn = match farm_conn.as_mut() {
            Some(c) => c,
            None => return,
        };

        for req_id in reqs {
            let req_id_str = req_id.to_string();
            // The option model is withdrawn the way it was asked for: the venue
            // is told which model, on which contract, not merely which request.
            if let Some(at) = withdrawn.iter().position(|(id, ..)| *id == req_id) {
                let (_, con_id, sec_type) = withdrawn.remove(at);
                let con_id_str = (con_id as u32).to_string();
                let fix_sec_type = crate::control::contracts::sec_type_to_fix(&sec_type);
                let greeks_type = GREEKS_REQUEST_TYPE.to_string();
                let _ = conn.send_fixcomp(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                    (262, &req_id_str),
                    (263, "2"),
                    (6008, &con_id_str),
                    (207, GREEKS_VENUE),
                    (167, fix_sec_type),
                    (264, &greeks_type),
                ]);
                continue;
            }
            let _ = conn.send_fixcomp(&[
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                (262, &req_id_str),
                (263, "2"),
            ]);
        }
        hb.last_farm_sent = Instant::now();
    }

    pub(crate) fn send_depth_subscribe(
        &mut self,
        req_id: u32,
        con_id: i64,
        exchange: &str,
        sec_type: &str,
        _num_rows: i32,
        is_smart_depth: bool,
        farm_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        let fix_sec_type = match sec_type {
            "STK" => "CS", "FUT" => "FUT", "OPT" => "OPT", "IND" => "IND",
            "CASH" => "CASH", other => other,
        };
        self.depth_subs.push((req_id, is_smart_depth));
        self.depth_resub_info.push((req_id, con_id, exchange.to_string(), sec_type.to_string(), _num_rows, is_smart_depth));

        // SmartDepth requires per-exchange fan-out. The server ACKs a BEST/SMART
        // subscribe but never sends data for it. Data only arrives for individual exchanges.
        // Auto-enable fan-out when exchange is SMART/BEST (aggregated routing), since
        // single-exchange depth to SMART returns nothing.
        let needs_fanout = is_smart_depth || matches!(exchange, "SMART" | "BEST" | "");
        let exchanges: &[&str] = if needs_fanout {
            // US equity exchanges that the gateway fans out to
            &["NASDAQ", "IEX", "BATS", "ARCA", "BEX", "NYSE", "BYX", "NYSENAT", "T24X",
              "DRCTEDGE", "MEMX", "PEARL", "AMEX", "CHX", "LTSE", "PSX", "ISE", "EDGEA"]
        } else {
            // Single exchange subscribe
            static SINGLE: [&str; 0] = [];
            &SINGLE
        };

        if let Some(conn) = farm_conn.as_mut() {
            let con_id_str = (con_id as u32).to_string();

            if !exchanges.is_empty() {
                // SmartDepth: fan-out to individual exchanges.
                // Each sub gets a unique req_id tracked as a depth subscription.
                for exch in exchanges {
                    let sub_req = self.next_md_req_id;
                    self.next_md_req_id += 1;
                    self.depth_subs.push((sub_req, true));
                    self.depth_fanout_map.push((sub_req, req_id));
                    let sub_req_str = sub_req.to_string();
                    self.send_depth_one(conn, &sub_req_str, &con_id_str, exch, fix_sec_type);
                }
                log::info!("SmartDepth fan-out: req={} con_id={} -> {} exchanges", req_id, con_id, exchanges.len());
            } else {
                // Single exchange
                let fix_exchange = match exchange {
                    "ISLAND" => "NASDAQ",
                    other => other,
                };
                let req_id_str = req_id.to_string();
                self.send_depth_one(conn, &req_id_str, &con_id_str, fix_exchange, fix_sec_type);
                log::info!("Depth subscribe: req={req_id} con_id={con_id} exchange={fix_exchange}");
            }
            hb.last_farm_sent = Instant::now();
        }
    }

    pub(crate) fn send_depth_unsubscribe(
        &mut self,
        req_id: u32,
        farm_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        let found = match self.depth_subs.iter().position(|(id, _)| *id == req_id) {
            Some(idx) => {
                self.depth_subs.remove(idx);
                true
            }
            None => false,
        };
        if !found { return; }

        // Remove reconnect params
        self.depth_resub_info.retain(|(id, _, _, _, _, _)| *id != req_id);

        // Collect SmartDepth fan-out sub_reqs that map to this user req_id
        let fanout_reqs: Vec<u32> = self.depth_fanout_map.iter()
            .filter(|(_, user)| *user == req_id)
            .map(|(sub, _)| *sub)
            .collect();

        // Remove fan-out entries from depth_subs and depth_fanout_map
        self.depth_subs.retain(|(id, _)| !fanout_reqs.contains(id));
        self.depth_fanout_map.retain(|(_, user)| *user != req_id);

        // Clear server_tag mappings for this req_id
        self.depth_tag_to_req.retain(|(_, rid, ..)| *rid != req_id);

        if let Some(conn) = farm_conn.as_mut() {
            // Send unsub for each fan-out sub_req (SmartDepth per-exchange)
            for sub_req in &fanout_reqs {
                let sub_req_str = sub_req.to_string();
                let _ = conn.send_fixcomp(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                    (262, &sub_req_str),
                    (263, "2"),
                ]);
            }
            // Send unsub for the primary req_id
            let req_id_str = req_id.to_string();
            let _ = conn.send_fixcomp(&[
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                (262, &req_id_str),
                (263, "2"),
            ]);
            hb.last_farm_sent = Instant::now();
            log::info!("Sent depth unsubscribe: req_id={} (+ {} fan-out)", req_id, fanout_reqs.len());
        }
    }

    /// Send a single depth subscribe for one exchange.
    fn send_depth_one(&self, conn: &mut Connection, req_id_str: &str, con_id_str: &str, exchange: &str, sec_type: &str) {
        let is_direct = matches!(exchange, "NASDAQ" | "BATS" | "ARCA" | "BEX" | "NYSE" | "IEX"
            | "BYX" | "NYSENAT" | "T24X");
        if is_direct {
            let _ = conn.send_fixcomp(&[
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                (263, "1"), (146, "1"), (262, req_id_str),
                (6008, con_id_str), (207, exchange), (167, sec_type),
                (264, "0"), (9830, "1"),
            ]);
        } else {
            // Socket exchanges (DRCTEDGE, MEMX, PEARL, AMEX, CHX, LTSE, PSX, ISE, EDGEA, etc.)
            let _ = conn.send_fixcomp(&[
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                (263, "1"), (146, "1"), (262, req_id_str),
                (6008, con_id_str), (207, exchange), (167, sec_type),
                (264, "442"), (6088, "Socket"), (9830, "1"),
            ]);
        }
    }

    /// Parse 35=P depth entries (byte-aligned: [00][3B stag][field tags...][58 terminator]).
    /// SmartDepth entries may contain multiple price+size pairs (bid then ask).
    /// Field tag encoding: bit 5(0x20)=size, bit 3(0x08)=ask, bit 2(0x04)=snapshot, bit 0(0x01)=2-byte.
    fn handle_depth_35p(&self, body: &[u8], shared: &SharedState) {
        use crate::types::DepthUpdate;
        let mut pos = 0;
        let mut bid_position: i32 = 0;
        let mut ask_position: i32 = 0;

        while pos < body.len() {
            if body[pos] != 0x00 { pos += 1; continue; }
            pos += 1;
            if pos + 3 > body.len() { break; }

            let stag = ((body[pos] as u32) << 16) | ((body[pos+1] as u32) << 8) | (body[pos+2] as u32);
            pos += 3;

            let (req_id, is_smart, min_tick, size_tick) = match self.depth_tag_to_req.iter()
                .find(|(s, ..)| *s == stag)
                .map(|(_, r, sm, mt, st)| (*r, *sm, *mt, *st))
            {
                Some(v) => v,
                None => { continue; }
            };

            // What the venue counts this contract's sizes in, the same way
            // min_tick is what it counts its prices in. Stating none means
            // whole ones.
            let counted_in = if size_tick > 0.0 { size_tick } else { 1.0 };
            // Parse field tags, pushing a depth update each time we complete a price+size pair.
            let mut price: f64 = 0.0;
            let mut size: f64 = 0.0;
            let mut side: i32 = 1;
            let mut is_snapshot = false;
            let mut has_price = false;
            let mut has_size = false;

            while pos < body.len() && body[pos] != 0x58 && body[pos] != 0x00 {
                let tag = body[pos];
                // Only recognize tags with known bits (0x20, 0x08, 0x04, 0x01).
                // Bit 7 (0x80) or bit 6 (0x40) set → unknown encoding, stop.
                if tag & 0xC0 != 0 { break; }
                pos += 1;

                let is_size_field = tag & 0x20 != 0;
                let is_ask = tag & 0x08 != 0;
                let snapshot = tag & 0x04 != 0;
                let two_byte = tag & 0x01 != 0;

                let new_side = if is_ask { 0 } else { 1 };
                if snapshot { is_snapshot = true; }

                // If side changes and we have a pending pair, flush it first
                if has_price && has_size && new_side != side {
                    let position = if side == 0 { let p = ask_position; ask_position += 1; p }
                                  else { let p = bid_position; bid_position += 1; p };
                    let operation = if is_snapshot { 0 } else { 1 };
                    shared.market.push_depth_update(DepthUpdate {
                        req_id, position, market_maker: String::new(),
                        operation, side, price, size, is_smart_depth: is_smart,
                    });
                    has_price = false;
                    has_size = false;
                }
                side = new_side;

                if two_byte {
                    if pos + 2 > body.len() { break; }
                    let val = ((body[pos] as u16) << 8) | (body[pos+1] as u16);
                    pos += 2;
                    if is_size_field { size = val as f64 * counted_in; has_size = true; }
                    else { price = val as f64 * min_tick; has_price = true; }
                } else {
                    if pos >= body.len() { break; }
                    let val = body[pos];
                    pos += 1;
                    if is_size_field { size = val as f64 * 100.0 * counted_in; has_size = true; }
                    else { price = val as f64 * min_tick; has_price = true; }
                }

                // Flush complete pair immediately
                if has_price && has_size {
                    let position = if side == 0 { let p = ask_position; ask_position += 1; p }
                                  else { let p = bid_position; bid_position += 1; p };
                    let operation = if is_snapshot { 0 } else { 1 };
                    shared.market.push_depth_update(DepthUpdate {
                        req_id, position, market_maker: String::new(),
                        operation, side, price, size, is_smart_depth: is_smart,
                    });
                    has_price = false;
                    has_size = false;
                }
            }

            if pos < body.len() && body[pos] == 0x58 { pos += 1; }
        }
    }

    /// Parse 35=Y depth entries (NASDAQ TotalView market-maker level).
    /// Wire format (from wire capture):
    ///   Header: [2B misc][2B stag_uint16_be]
    ///   Stag switch sentinel: [80 00][2B stag_uint16_be]
    ///   Snapshot entry: [C4|44][4B market_maker][1B position][field_tags...]
    ///   Compact entry:  [80|00][1B position][field_tags...]
    ///     C4/80 = continuation, 44/00 = terminal (last entry for this stag section).
    /// Field tag encoding: bit 7=size, bit 5=ask, bit 2=snapshot, bits 0-1=value_len (00=1B,01=2B,10=3B).
    fn handle_depth_35y(&self, msg: &[u8], shared: &SharedState) {
        use crate::types::DepthUpdate;
        let body = match find_body_after_tag(msg, b"35=Y\x01") {
            Some(b) => b,
            None => return,
        };

        // Header: 2 bytes misc. The stag is set by the first 80 00 [2B stag] sentinel.
        if body.len() < 4 { return; }

        // Try header stag at body[2..4] (common case).
        let mut req_id: u32 = 0;
        let mut is_smart = false;
        let mut min_tick: f64 = 0.01;
        let mut size_tick: f64 = 1.0;
        let mut pos = 2;

        let hdr_stag = ((body[2] as u32) << 8) | (body[3] as u32);
        if let Some((r, sm, mt, st)) = self.lookup_depth_stag(hdr_stag) {
            req_id = r;
            is_smart = sm;
            min_tick = mt;
            size_tick = st;
            pos = 4;
        }
        // If header stag didn't match, start scanning from pos=2;
        // the first stag switch sentinel will set req_id.

        while pos < body.len() {
            let b = body[pos];

            // Stag switch sentinel: 80 00 [2B stag] — bid_size=0 repurposed.
            // Also detect 00 00 [2B stag] (3-byte stag with high byte 0x00, at message boundaries).
            if (b == 0x80 || b == 0x00) && pos + 4 <= body.len() && body[pos + 1] == 0x00 {
                let candidate = ((body[pos + 2] as u32) << 8) | (body[pos + 3] as u32);
                if let Some((r, sm, mt, st)) = self.lookup_depth_stag(candidate) {
                    req_id = r;
                    is_smart = sm;
                    min_tick = mt;
                    size_tick = st;
                    pos += 4;
                    continue;
                }
            }

            // Snapshot entry: [C4|44][4B market_maker][1B position][field_tags...]
            if b == 0xC4 || b == 0x44 {
                pos += 1;
                if pos + 5 > body.len() { break; }
                let mm = String::from_utf8_lossy(&body[pos..pos + 4]).trim().to_string();
                pos += 4;
                let book_position = body[pos] as i32;
                pos += 1;

                if let Some((price, size, side, is_snapshot)) = self.parse_depth_fields(body, &mut pos, min_tick, size_tick) {
                    shared.market.push_depth_update(DepthUpdate {
                        req_id, position: book_position, market_maker: mm,
                        operation: if is_snapshot { 0 } else { 1 },
                        side, price, size, is_smart_depth: is_smart,
                    });
                }
                continue;
            }

            // Compact entry: [80|00][1B position][field_tags...]  (no market maker)
            // 80 = continuation, 00 = terminal for this stag section.
            // Guard: stag switch sentinel already checked above.
            // Validate: position must be 0-29 and next byte must be a valid field tag.
            if (b == 0x80 || b == 0x00) && pos + 2 < body.len() {
                let candidate_pos = body[pos + 1];
                let candidate_tag = body[pos + 2];
                // Valid field tags: only bits 7,5,2,1,0 set (mask 0xAF). Reject bits 6,4,3.
                if candidate_pos < 30 && candidate_tag & 0x50 == 0 && candidate_tag & 0x08 == 0 {
                    pos += 1;
                    let book_position = body[pos] as i32;
                    pos += 1;

                    if let Some((price, size, side, is_snapshot)) = self.parse_depth_fields(body, &mut pos, min_tick, size_tick) {
                        shared.market.push_depth_update(DepthUpdate {
                            req_id, position: book_position, market_maker: String::new(),
                            operation: if is_snapshot { 0 } else { 1 },
                            side, price, size, is_smart_depth: is_smart,
                        });
                    }
                    continue;
                }
            }

            // Unknown byte — skip
            pos += 1;
        }
    }

    /// Look up a depth server_tag → (req_id, is_smart, min_tick, size_tick).
    fn lookup_depth_stag(&self, stag: u32) -> Option<(u32, bool, f64, f64)> {
        self.depth_tag_to_req.iter()
            .find(|(s, ..)| *s == stag)
            .map(|(_, r, sm, mt, st)| (*r, *sm, *mt, *st))
    }

    /// Parse one price + one size field tag pair. Returns (price, size, side, is_snapshot).
    /// Advances `pos` past consumed bytes.
    fn parse_depth_fields(&self, body: &[u8], pos: &mut usize, min_tick: f64, size_tick: f64) -> Option<(f64, f64, i32, bool)> {
        let mut price: f64 = 0.0;
        let mut size: f64 = 0.0;
        let mut side: i32 = 1; // default bid
        let mut is_snapshot = false;
        let mut has_price = false;
        let mut has_size = false;

        // Parse up to 2 field tags (one price + one size).
        for _ in 0..2 {
            if *pos >= body.len() { break; }
            let tag = body[*pos];
            // Valid field tags use bits 7,5,2,1,0. Reject if bit 6 or bit 4 set.
            if tag & 0x50 != 0 { break; }
            // Reject entry/stag prefixes that would start a new entry.
            if tag == 0xC4 || tag == 0x44 { break; }
            *pos += 1;

            let is_size_field = tag & 0x80 != 0;
            let is_ask = tag & 0x20 != 0;
            if tag & 0x04 != 0 { is_snapshot = true; }
            if is_ask { side = 0; } else { side = 1; }

            let val_len = tag & 0x03;
            let val: u32 = match val_len {
                0 => {
                    if *pos >= body.len() { break; }
                    let v = body[*pos] as u32; *pos += 1; v
                }
                1 => {
                    if *pos + 2 > body.len() { break; }
                    let v = ((body[*pos] as u32) << 8) | (body[*pos + 1] as u32);
                    *pos += 2; v
                }
                _ => {
                    if *pos + 3 > body.len() { break; }
                    let v = ((body[*pos] as u32) << 16) | ((body[*pos + 1] as u32) << 8) | (body[*pos + 2] as u32);
                    *pos += 3; v
                }
            };

            if is_size_field {
                // A size is a count of what the venue said sizes move in for
                // this contract, the same way a price is a count of what
                // prices move in. Counted as whole ones, a crypto's depth
                // reads a hundred million times over.
                size = val as f64 * if size_tick > 0.0 { size_tick } else { 1.0 };
                has_size = true;
            } else {
                price = val as f64 * min_tick;
                has_price = true;
            }
        }

        if has_price || has_size { Some((price, size, side, is_snapshot)) } else { None }
    }

    pub(crate) fn handle_disconnect(&mut self, context: &mut Context, _event_tx: &Option<SyncSender<Event>>) {
        self.disconnected = true;
        self.md_req_to_instrument.clear();
        self.instrument_md_reqs.clear();
        // Clear depth wire-state (server_tags become invalid after disconnect).
        // depth_resub_info is preserved for resubscription on reconnect.
        self.depth_subs.clear();
        self.depth_tag_to_req.clear();
        self.depth_fanout_map.clear();
        context.market.clear_server_tags();
        context.market.zero_all_quotes();
        // Don't emit Event::Disconnected — auto-reconnect handles farm drops transparently.
        // Python is only notified if reconnect exhausts retries.
    }

    /// Test-only: set disconnected without clearing state or emitting events.
    pub fn handle_disconnect_for_test(&mut self) {
        self.disconnected = true;
    }

    /// The L1 subscriptions to re-issue on a new farm connection, drained from
    /// the record that survives a disconnect. `handle_disconnect` clears
    /// `instrument_md_reqs`, so selecting from that list re-subscribes nothing
    /// and the reconnect silently delivers no market data (ibx#288). Skips
    /// instruments whose slot was reclaimed while the farm was down.
    fn take_resub_targets(
        &mut self,
        market: &crate::engine::market_state::MarketState,
    ) -> Vec<MdResubTarget> {
        std::mem::take(&mut self.md_resub_info)
            .into_iter()
            .filter_map(|(id, sym, exch, st, ltd, strike, right, mult, mode)| {
                market.con_id(id)
                    .map(|con_id| (id, con_id, sym, exch, st, ltd, strike, right, mult, mode))
            })
            .collect()
    }

    pub(crate) fn reconnect(
        &mut self,
        conn: Connection,
        farm_conn: &mut Option<Connection>,
        context: &mut Context,
        hb: &mut HeartbeatState,
        replay: ReplayPacing,
    ) {
        *farm_conn = Some(conn);
        self.disconnected = false;
        hb.last_farm_sent = Instant::now();
        hb.last_farm_recv = Instant::now();
        hb.pending_farm_test = None;

        // Re-issue the L1 subscriptions from `md_resub_info`, which survives a
        // disconnect for exactly this purpose — the same shape the depth path
        // below uses. Driving this off `instrument_md_reqs` re-subscribed
        // nothing, because `handle_disconnect` clears that list before the
        // reconnect runs (ibx#288).
        let active = self.take_resub_targets(&context.market);
        self.md_req_to_instrument.clear();
        self.instrument_md_reqs.clear();
        // Paced. A server that has just come back up is the least able to take
        // a whole book of subscriptions at once, and the client that hands it
        // one is the client it throttles.
        for (i, (instrument, con_id, sym, exch, st, ltd, strike, right, mult, mode)) in
            active.into_iter().enumerate()
        {
            if i > 0 && replay.burst > 0 && i.is_multiple_of(replay.burst) {
                std::thread::sleep(replay.pace);
            }
            self.send_mktdata_subscribe(con_id, &sym, &exch, &st, &ltd, strike, &right, &mult, instrument, mode, farm_conn, hb);
        }

        // Re-subscribe depth subscriptions (depth_resub_info survived disconnect)
        let depth_params: Vec<_> = self.depth_resub_info.drain(..).collect();
        let depth_count = depth_params.len();
        for (req_id, con_id, exchange, sec_type, num_rows, is_smart_depth) in depth_params {
            self.send_depth_subscribe(
                req_id, con_id, &exchange, &sec_type, num_rows, is_smart_depth,
                farm_conn, hb,
            );
        }

        log::info!("Farm reconnected, re-subscribed {} instruments + {} depth", self.instrument_md_reqs.len(), depth_count);
    }

    fn handle_generic_tick(&mut self, msg: &[u8], shared: &SharedState, event_tx: &Option<SyncSender<Event>>) {
        let body = match find_body_after_tag(msg, b"35=G\x01") {
            Some(b) => b,
            None => return,
        };

        let Some(frame) = GenericTickFrame::read(body) else { return };

        // Which tick this is is not on the frame. The venue numbers a generic
        // tick apart from the prices and says what it carries once, when it
        // takes the subscription on, so what was asked for under that number
        // is the only thing that says what these bytes are. Read off the
        // frame's own length instead, every tick whose payload happened to be
        // the same size read as the same tick.
        let Some((_, request_type, instrument)) = self
            .generic_tick_tags
            .iter()
            .find(|(tag, ..)| *tag == frame.server_tag)
            .copied()
        else {
            log::debug!(
                "A generic tick arrived under server tag {}, which nothing here asked for",
                frame.server_tag,
            );
            return;
        };

        if request_type == GREEKS_REQUEST_TYPE {
            if let Some(mut comp) = decode_greeks(frame.payload) {
                comp.instrument = instrument;
                shared.market.push_option_computation(comp);
                emit(event_tx, Event::OptionComputation(comp));
            }
            return;
        }
        if request_type != NEWS_REQUEST_TYPE { return; }
        let body = frame.payload;
        if body.len() < 4 { return; }

        let batch_count = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
        let mut pos = 4;

        for _ in 0..batch_count {
            if pos + 4 > body.len() { break; }
            let prov_len = u32::from_be_bytes([body[pos], body[pos+1], body[pos+2], body[pos+3]]) as usize;
            pos += 4;
            if pos + prov_len > body.len() { break; }
            let provider = String::from_utf8_lossy(&body[pos..pos+prov_len]).to_string();
            pos += prov_len;

            if pos + 4 > body.len() { break; }
            pos += 4;

            if pos + 2 > body.len() { break; }
            let aid_len = u16::from_be_bytes([body[pos], body[pos+1]]) as usize;
            pos += 2;
            if pos + aid_len > body.len() { break; }
            let article_id = String::from_utf8_lossy(&body[pos..pos+aid_len]).to_string();
            pos += aid_len;

            if pos + 8 > body.len() { break; }
            pos += 4;
            let timestamp = u32::from_be_bytes([body[pos], body[pos+1], body[pos+2], body[pos+3]]) as u64;
            pos += 4;

            if pos + 4 > body.len() { break; }
            let hl_len = u32::from_be_bytes([body[pos], body[pos+1], body[pos+2], body[pos+3]]) as usize;
            pos += 4;
            if pos + hl_len > body.len() { break; }
            let raw_headline = String::from_utf8_lossy(&body[pos..pos+hl_len]).to_string();
            pos += hl_len;

            let headline = if raw_headline.starts_with('{') {
                match raw_headline.find('}') {
                    Some(i) => raw_headline[i+1..].to_string(),
                    None => raw_headline,
                }
            } else {
                raw_headline
            };

            let news = crate::types::TickNews {
                instrument,
                provider_code: provider,
                article_id,
                headline,
                timestamp,
            };
            shared.market.push_tick_news(news.clone());
            emit(event_tx, Event::News(news));
        }
    }
}

#[cfg(test)]
mod news_tests {
    use super::*;
    use crate::bridge::SharedState;
    use crate::engine::context::Context;

    /// Wrap a news body in the frame the farm connection delivers it in.
    /// One generic tick, framed the way the venue frames it: the length of
    /// everything after it in bits, the venue's number for the subscription,
    /// then the payload's length in bytes and the payload.
    fn framed_generic_tick(server_tag: u32, payload: &[u8]) -> Vec<u8> {
        let mut frame = server_tag.to_be_bytes().to_vec();
        frame.push(payload.len() as u8);
        frame.extend_from_slice(payload);
        let mut msg = b"35=G\x01".to_vec();
        msg.extend_from_slice(&((frame.len() * 8) as u16).to_be_bytes());
        msg.extend_from_slice(&frame);
        msg
    }

    /// One article, laid out as the handler reads it.
    fn one_article() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&1u32.to_be_bytes());
        body.extend_from_slice(&4u32.to_be_bytes());
        body.extend_from_slice(b"BRFG");
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(b"id");
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&1_785_325_554u32.to_be_bytes());
        body.extend_from_slice(&8u32.to_be_bytes());
        body.extend_from_slice(b"headline");
        body
    }

    /// A frame under a number nothing asked a generic tick under says nothing
    /// about which tick it is, so it is dropped rather than guessed at.
    /// Instrument 0 is a real instrument — the first one registered — so a
    /// guess would pin somebody else's article on it.
    #[test]
    fn a_tick_under_an_unasked_number_is_dropped_not_misattributed() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let first = context.market.register(756733);
        assert_eq!(first, 0, "the first instrument really is id 0");
        context.market.register_server_tag(999_999, first);

        let msg = framed_generic_tick(999_999, &one_article());
        farm.handle_generic_tick(&msg, &shared, &None);
        assert!(
            shared.market.drain_tick_news().is_empty(),
            "a number nothing asked a generic tick under delivers nothing",
        );

        // Positive control: the same frame, once this client has said what it
        // asked for under that number.
        farm.generic_tick_tags.push((999_999, NEWS_REQUEST_TYPE, first));
        farm.handle_generic_tick(&msg, &shared, &None);
        assert_eq!(
            shared.market.drain_tick_news().len(), 1,
            "so the drop above is what was asked for, not the frame",
        );
    }

    /// Which tick a frame carries is what was asked for under its number, not
    /// how long the frame is. Read off the length, every tick whose payload
    /// happened to be the size of an option model read as an option model.
    #[test]
    fn two_ticks_of_one_length_are_told_apart() {
        let shared = SharedState::new();
        let article = one_article();

        let mut as_news = FarmState::new();
        as_news.generic_tick_tags.push((7, NEWS_REQUEST_TYPE, 0));
        as_news.handle_generic_tick(&framed_generic_tick(7, &article), &shared, &None);
        assert_eq!(shared.market.drain_tick_news().len(), 1);

        // The same bytes, the same length, asked for as something else.
        let mut as_status = FarmState::new();
        as_status.generic_tick_tags.push((7, TRADING_STATUS_REQUEST_TYPE, 0));
        as_status.handle_generic_tick(&framed_generic_tick(7, &article), &shared, &None);
        assert!(
            shared.market.drain_tick_news().is_empty(),
            "the same bytes under a different tick are not an article",
        );
    }

    /// The frame states its length twice, and a frame whose two lengths
    /// disagree is refused rather than read as far as it goes.
    #[test]
    fn a_frame_that_does_not_hold_together_is_refused() {
        let article = one_article();
        let mut msg = framed_generic_tick(7, &article);
        // Claim one byte more of payload than the frame carries.
        let at = msg.len() - article.len() - 1;
        msg[at] += 1;
        assert!(GenericTickFrame::read(&msg[5..]).is_none(), "the two lengths disagree");
    }

    /// A payload too long to state in one byte states its length in two, and
    /// the reading that squares with the whole frame is the right one.
    #[test]
    fn a_long_payload_states_its_length_in_two_bytes() {
        let payload = vec![7u8; 300];
        let mut frame = 42u32.to_be_bytes().to_vec();
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        frame.extend_from_slice(&payload);
        let mut body = ((frame.len() * 8) as u16).to_be_bytes().to_vec();
        body.extend_from_slice(&frame);

        let read = GenericTickFrame::read(&body).expect("the frame holds together");
        assert_eq!(read.server_tag, 42);
        assert_eq!(read.payload, &payload[..]);
    }

}

#[cfg(test)]
mod decode_publish_tests {
    use super::*;
    use crate::bridge::SharedState;
    use crate::engine::context::Context;
    use crate::protocol::tick_decoder;
    use crate::types::QTY_SCALE;

    fn push_bits(bits: &mut Vec<u8>, val: u64, n: usize) {
        for i in (0..n).rev() {
            bits.push(((val >> i) & 1) as u8);
        }
    }

    /// One 35=P body carrying `ticks` for `server_tag`, framed as the farm
    /// connection delivers it.
    fn framed_35p(server_tag: u32, ticks: &[(u64, u64, u64)]) -> Vec<u8> {
        let mut bits: Vec<u8> = Vec::new();
        push_bits(&mut bits, 0, 1);
        push_bits(&mut bits, server_tag as u64, 31);
        for (i, &(tick_type, width, value)) in ticks.iter().enumerate() {
            push_bits(&mut bits, tick_type, 5);
            push_bits(&mut bits, if i < ticks.len() - 1 { 1 } else { 0 }, 1);
            push_bits(&mut bits, width - 1, 2);
            push_bits(&mut bits, 0, 1); // positive
            push_bits(&mut bits, value, (width * 8 - 1) as usize);
        }
        let byte_count = bits.len().div_ceil(8);
        let mut payload = vec![0u8; byte_count];
        for (i, &b) in bits.iter().enumerate() {
            if b == 1 {
                payload[i >> 3] |= 1 << (7 - (i & 7));
            }
        }
        let mut tick_payload = Vec::with_capacity(2 + byte_count);
        tick_payload.push((bits.len() >> 8) as u8);
        tick_payload.push((bits.len() & 0xFF) as u8);
        tick_payload.extend_from_slice(&payload);

        let body_len = 5 + tick_payload.len() + 15;
        let mut msg = format!("8=O\x019={body_len}\x01").into_bytes();
        msg.extend_from_slice(b"35=P\x01");
        msg.extend_from_slice(&tick_payload);
        msg.extend_from_slice(b"\x018349=AABBCCDD\x01");
        msg
    }

    /// The constants table says which wire type is which; this says where each
    /// one lands. Nothing else pins that: swapping the open and close arms with
    /// the table intact passes the whole suite, and that is precisely the
    /// failure this decode change exists to remove — two plausible prices
    /// exchanged, with the P&L path reading the wrong one.
    #[test]
    fn each_price_type_lands_in_its_own_quote_field() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(9, id);
        context.market.set_min_tick(id, 0.01);

        // Distinct magnitudes, so no two fields can be confused.
        let msg = framed_35p(9, &[
            (tick_decoder::O_LAST_PRICE, 2, 501),
            (tick_decoder::O_HIGH_PRICE, 2, 502),
            (tick_decoder::O_LOW_PRICE, 2, 503),
            (tick_decoder::O_OPEN_PRICE, 2, 504),
            (tick_decoder::O_CLOSE_PRICE, 2, 505),
        ]);
        farm.handle_tick_data(&msg, &mut context, &shared, &None);

        let mts = context.market.min_tick_scaled(id);
        let q = context.market.quote(id);
        assert_eq!(q.last, 501 * mts, "last");
        assert_eq!(q.high, 502 * mts, "high");
        assert_eq!(q.low, 503 * mts, "low");
        assert_eq!(q.open, 504 * mts, "open");
        assert_eq!(q.close, 505 * mts, "close");
    }

    /// The timestamp arm carries seconds and is stored in nanoseconds, and the
    /// guard is what keeps a date-shaped value out of the field.
    #[test]
    fn the_timestamp_is_seconds_stored_as_nanoseconds() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(11, id);
        context.market.set_min_tick(id, 0.01);

        farm.handle_tick_data(
            &framed_35p(11, &[(tick_decoder::O_TS_BASE, 4, 1_785_325_554)]),
            &mut context, &shared, &None,
        );
        assert_eq!(
            context.market.quote(id).timestamp_ns, 1_785_325_554_000_000_000,
            "an epoch second is stored as nanoseconds",
        );

        // A yyyymmdd-shaped value is not a timestamp and must not land here.
        let id2 = context.market.register(265598);
        context.market.register_server_tag(12, id2);
        context.market.set_min_tick(id2, 0.01);
        farm.handle_tick_data(
            &framed_35p(12, &[(tick_decoder::O_TS_BASE, 4, 20_260_729)]),
            &mut context, &shared, &None,
        );
        assert_eq!(
            context.market.quote(id2).timestamp_ns, 0,
            "a date-shaped magnitude is dropped rather than stored",
        );
    }

    /// The producer half of the quantity contract. Everything downstream
    /// divides by `QTY_SCALE`, so a decode path that stores the wire magnitude
    /// raw delivers quantities 10_000x too small (ibx#287) — and nothing else
    /// in the suite reaches this function, which is why that shipped.
    #[test]
    fn decoded_quantities_are_stored_as_fixed_point() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(7, id);
        context.market.set_min_tick(id, 0.01);

        let msg = framed_35p(7, &[
            (tick_decoder::O_BID_SIZE, 1, 42),
            (tick_decoder::O_ASK_SIZE, 1, 17),
            (tick_decoder::O_LAST_SIZE, 1, 5),
            (tick_decoder::O_VOLUME, 2, 1234),
        ]);
        farm.handle_tick_data(&msg, &mut context, &shared, &None);

        let q = context.market.quote(id);
        assert_eq!(q.bid_size, 42 * QTY_SCALE, "bid_size must be stored fixed-point");
        assert_eq!(q.ask_size, 17 * QTY_SCALE, "ask_size must be stored fixed-point");
        assert_eq!(q.last_size, 5 * QTY_SCALE, "last_size must be stored fixed-point");
        assert_eq!(q.volume, 1234 * QTY_SCALE, "volume must be stored fixed-point");
    }

    /// Prices were already scaled correctly; pin that the quantity change did
    /// not disturb them.
    #[test]
    fn decoded_prices_are_still_scaled_by_min_tick() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(9, id);
        context.market.set_min_tick(id, 0.01);
        let mts = context.market.min_tick_scaled(id);

        let msg = framed_35p(9, &[(tick_decoder::O_BID_PRICE, 2, 15000)]);
        farm.handle_tick_data(&msg, &mut context, &shared, &None);

        assert_eq!(context.market.quote(id).bid, 15000 * mts);
    }
}

#[cfg(test)]
mod resub_tests {
    use super::*;
    use crate::engine::market_state::MarketState;

    /// A disconnect clears `instrument_md_reqs` and keeps `md_resub_info`.
    /// Selecting the reconnect's work from the cleared list re-subscribed
    /// nothing, so the farm came back healthy and delivered no ticks for the
    /// rest of the session (ibx#288).
    ///
    /// Drives the real `handle_disconnect` rather than simulating what it does
    /// — the test-only hook that skips the clearing is what let this survive,
    /// and a hand-written stand-in can drift from the real one the same way.
    #[test]
    fn resub_targets_survive_a_real_disconnect() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let mut hb = HeartbeatState::new();
        let instrument = context.market.register(756733);

        farm.send_mktdata_subscribe(
            756733, "SPY", "SMART", "STK", "", 0.0, "", "", instrument, 0,
            &mut None, &mut hb,
        );
        farm.handle_disconnect(&mut context, &None);
        assert!(farm.instrument_md_reqs.is_empty(), "the disconnect clears the request list");

        let targets = farm.take_resub_targets(&context.market);
        assert_eq!(targets.len(), 1, "the subscription must survive the disconnect");
        assert_eq!(targets[0].0, instrument);
        assert_eq!(targets[0].1, 756733, "con_id must be resolved for the re-issue");
        assert_eq!(targets[0].2, "SPY");

        // Re-issuing with no connection must still leave the record standing,
        // so a later reconnect can retry rather than losing the subscription.
        let (id, con_id, sym, exch, st, ltd, k, r, m, mode) = targets.into_iter().next().unwrap();
        farm.send_mktdata_subscribe(
            con_id, &sym, &exch, &st, &ltd, k, &r, &m, id, mode, &mut None, &mut hb,
        );
        assert_eq!(farm.md_resub_info.len(), 1, "the record must survive an absent connection");
    }

    /// An unsubscribe issued while the farm is down must still cancel. The
    /// lookup it does first early-returns during an outage, so a record left
    /// standing would be replayed on reconnect as a subscription the caller
    /// had explicitly cancelled.
    #[test]
    fn unsubscribing_while_down_does_not_leave_a_resubscribe_record() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let mut hb = HeartbeatState::new();
        let instrument = context.market.register(756733);

        farm.send_mktdata_subscribe(
            756733, "SPY", "SMART", "STK", "", 0.0, "", "", instrument, 0,
            &mut None, &mut hb,
        );
        farm.handle_disconnect(&mut context, &None);
        farm.send_mktdata_unsubscribe(instrument, &mut None, &mut hb);

        assert!(
            farm.take_resub_targets(&context.market).is_empty(),
            "a cancelled subscription must not come back on reconnect",
        );
    }

    /// The other side of keeping a slot resident: it has to become releasable
    /// again, or the guard turns a bounded pool into a leak and the instrument
    /// cap becomes cumulative-per-session — the failure ibx#233 exists to
    /// prevent. Every route out of a subscription has to clear all three
    /// references, whether the farm is up or down.
    #[test]
    fn a_slot_becomes_reclaimable_again_once_the_subscription_ends() {
        for down in [false, true] {
            let mut farm = FarmState::new();
            let mut context = Context::new();
            let mut hb = HeartbeatState::new();
            let instrument = context.market.register(756733);

            farm.send_mktdata_subscribe(
                756733, "SPY", "SMART", "STK", "", 0.0, "", "", instrument, 0,
                &mut None, &mut hb,
            );
            assert!(farm.holds_market_data(instrument), "subscribed: held");

            if down {
                farm.handle_disconnect(&mut context, &None);
                // The record deliberately survives a disconnect, so the slot
                // stays held — that is what makes the resubscribe possible.
                assert!(farm.holds_market_data(instrument), "disconnected: still held");
            }

            farm.send_mktdata_unsubscribe(instrument, &mut None, &mut hb);
            assert!(
                !farm.holds_market_data(instrument),
                "unsubscribed (farm down: {down}): the slot must be releasable",
            );
        }
    }

    /// A slot reclaimed while the farm was down has no con_id to subscribe.
    #[test]
    fn resub_targets_skip_an_instrument_reclaimed_while_down() {
        let mut farm = FarmState::new();
        let mut market = MarketState::new();
        let instrument = market.register(756733);
        farm.md_resub_info.push((
            instrument, "SPY".into(), "SMART".into(), "STK".into(), String::new(),
            0.0, String::new(), String::new(), 0,
        ));
        market.unregister(instrument);

        assert!(farm.take_resub_targets(&market).is_empty());
    }

    /// The case the test above does not reach: the slot is not merely freed but
    /// handed to another contract before the reconnect. `md_resub_info` holds
    /// no con_id of its own, so the record is combined with whatever con_id the
    /// id now resolves to — the old contract's descriptor subscribing the new
    /// contract's instrument. The guard is that a slot holding market-data
    /// state is not reclaimable in the first place.
    #[test]
    fn an_instrument_holding_a_resubscribe_record_is_not_reclaimable() {
        let mut farm = FarmState::new();
        let mut market = MarketState::new();
        let instrument = market.register(756733);
        farm.md_resub_info.push((
            instrument, "SPY".into(), "SMART".into(), "STK".into(), String::new(),
            0.0, String::new(), String::new(), 0,
        ));

        assert!(
            farm.holds_market_data(instrument),
            "the record alone must keep the slot resident",
        );

        // And a live subscription does the same on its own.
        let mut farm = FarmState::new();
        farm.instrument_md_reqs.push((instrument, vec![7]));
        assert!(farm.holds_market_data(instrument), "a live subscription");

        // An instrument with none of the three is free to go.
        assert!(!FarmState::new().holds_market_data(instrument));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tag_values(tags: &[(u32, String)], tag: u32) -> Vec<&str> {
        tags.iter().filter(|(t, _)| *t == tag).map(|(_, v)| v.as_str()).collect()
    }

    /// The server routes a market-data subscription by SecurityType and
    /// Exchange even when a conId is supplied. Describing every contract as a
    /// SMART-routed common stock makes the server ack only the trade leg of a
    /// futures subscription, so bid/ask never arrives.
    #[test]
    fn conid_subscribe_describes_the_actual_contract() {
        let fut = build_conid_subscribe_tags(true, 1, 2, 793356225, "CME", "FUT", 0, "T");
        assert_eq!(tag_values(&fut, 167), ["FUT", "FUT"], "SecurityType must say FUT");
        assert_eq!(tag_values(&fut, 207), ["CME", "CME"], "Exchange must say CME");

        // Both legs of the realtime fan-out are requested: 442 bid/ask, 443 last.
        assert_eq!(tag_values(&fut, 264), ["442", "443"]);
        assert_eq!(tag_values(&fut, 262), ["1", "2"]);
        assert_eq!(tag_values(&fut, 146), ["2"]);
    }

    /// Stocks keep the exact wire shape they had before: SMART maps to BEST and
    /// STK to CS, so this path is unchanged for equities. Pinned as the whole
    /// ordered tag list rather than the two mapped tags, so a reordering or a
    /// dropped field is caught here too.
    #[test]
    fn conid_subscribe_is_unchanged_for_stocks() {
        let stk = build_conid_subscribe_tags(true, 1, 2, 265598, "SMART", "STK", 0, "T");
        assert_eq!(
            stk,
            vec![
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
                (fix::TAG_SENDING_TIME, "T".to_string()),
                (263, "1".to_string()),
                (146, "2".to_string()),
                (262, "1".to_string()),
                (6008, "265598".to_string()),
                (207, "BEST".to_string()),
                (167, "CS".to_string()),
                (264, "442".to_string()),
                (6088, "Socket".to_string()),
                (9830, "1".to_string()),
                (9839, "1".to_string()),
                (262, "2".to_string()),
                (6008, "265598".to_string()),
                (207, "BEST".to_string()),
                (167, "CS".to_string()),
                (264, "443".to_string()),
                (6088, "Socket".to_string()),
                (9830, "1".to_string()),
                (9839, "1".to_string()),
            ],
        );

        let delayed = build_conid_subscribe_tags(false, 1, 2, 265598, "SMART", "STK", 3, "T");
        assert_eq!(
            delayed,
            vec![
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
                (fix::TAG_SENDING_TIME, "T".to_string()),
                (263, "1".to_string()),
                (146, "1".to_string()),
                (262, "1".to_string()),
                (6008, "265598".to_string()),
                (207, "BEST".to_string()),
                (167, "CS".to_string()),
                (264, "1".to_string()),
                (6088, "Socket".to_string()),
                (9830, "1".to_string()),
                (9839, "1".to_string()),
                (9887, "3".to_string()),
            ],
        );
    }

    /// Subscribing by conId alone is a supported shape: `Contract` defaults
    /// both descriptive fields to empty and the in-tree benchmark relies on it.
    /// Those callers used to get a smart-routed stock from the two literals;
    /// sending an empty SecurityType and Exchange instead would reintroduce the
    /// silent partial ack from the other side.
    #[test]
    fn conid_subscribe_falls_back_when_the_contract_is_not_described() {
        let bare = build_conid_subscribe_tags(true, 1, 2, 265598, "", "", 0, "T");
        assert_eq!(tag_values(&bare, 167), ["CS", "CS"]);
        assert_eq!(tag_values(&bare, 207), ["BEST", "BEST"]);

        let described = build_conid_subscribe_tags(true, 1, 2, 265598, "SMART", "STK", 0, "T");
        assert_eq!(bare, described, "an undescribed conId keeps the smart-routed stock shape");
    }

    /// Non-realtime modes collapse to a single TOP subscription carrying 9887.
    #[test]
    fn conid_subscribe_collapses_to_one_entry_when_not_realtime() {
        let delayed = build_conid_subscribe_tags(false, 7, 8, 265598, "SMART", "STK", 3, "T");
        assert_eq!(tag_values(&delayed, 262), ["7"], "only the first req id is used");
        assert_eq!(tag_values(&delayed, 264), ["1"]);
        assert_eq!(tag_values(&delayed, 146), ["1"]);
        assert_eq!(tag_values(&delayed, 9887), ["3"], "delayed mode must be carried");

        let realtime = build_conid_subscribe_tags(true, 7, 8, 265598, "SMART", "STK", 0, "T");
        assert!(tag_values(&realtime, 9887).is_empty(), "realtime carries no 9887");
    }

    /// Every entry must be self-contained: the server reads conId per entry.
    #[test]
    fn each_entry_carries_its_own_conid() {
        let fut = build_conid_subscribe_tags(true, 1, 2, 793356225, "CME", "FUT", 0, "T");
        assert_eq!(tag_values(&fut, 6008), ["793356225", "793356225"]);

        let counts: HashMap<u32, usize> =
            fut.iter().fold(HashMap::new(), |mut m, (t, _)| { *m.entry(*t).or_insert(0) += 1; m });
        for tag in [262, 6008, 207, 167, 264, 6088, 9830, 9839] {
            assert_eq!(counts[&tag], 2, "tag {tag} must appear once per entry");
        }
    }
}

#[cfg(test)]
mod stale_ack_tests {
    use super::*;
    use crate::engine::context::Context;

    /// A `35=Q` in flight when the unsubscribe goes out used to resolve its
    /// request id afterwards. If the slot had been reclaimed and reused by
    /// then, the ack bound its server_tag and minTick onto the new contract,
    /// whose prices then scaled by the previous contract's tick size — a
    /// plausible wrong price rather than an obvious fault (ibx#289).
    #[test]
    fn a_late_ack_for_an_unsubscribed_request_is_ignored() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let mut hb = HeartbeatState::new();
        let instrument = context.market.register(756733);

        farm.send_mktdata_subscribe(
            756733, "SPY", "SMART", "STK", "", 0.0, "", "", instrument, 0,
            &mut None, &mut hb,
        );
        let pending: Vec<u32> = farm.md_req_to_instrument.iter().map(|(r, _)| *r).collect();
        assert!(!pending.is_empty(), "the subscribe must register at least one request");

        farm.send_mktdata_unsubscribe(instrument, &mut None, &mut hb);

        for req_id in pending {
            assert!(
                !farm.md_req_to_instrument.iter().any(|(r, _)| *r == req_id),
                "request {req_id} must not resolve after its unsubscribe",
            );
        }
    }
}

#[cfg(test)]
mod price_scaling_tests {
    use super::*;
    use crate::bridge::SharedState;
    use crate::engine::context::Context;
    use crate::protocol::tick_decoder;

    fn push(bits: &mut Vec<u8>, val: u64, n: usize) {
        for i in (0..n).rev() {
            bits.push(((val >> i) & 1) as u8);
        }
    }

    /// One 35=P body carrying a single extended entry, framed as the farm
    /// connection delivers it. The extended header carries a full byte width,
    /// which is how a magnitude large enough to overflow the price scaling
    /// arrives from the wire.
    fn framed_extended(server_tag: u32, tick_type: u64, byte_width: u64, value: u64) -> Vec<u8> {
        let mut bits: Vec<u8> = Vec::new();
        push(&mut bits, 0, 1);
        push(&mut bits, server_tag as u64, 31);
        push(&mut bits, 31, 5); // extended sentinel
        push(&mut bits, 0, 1);  // has_more
        push(&mut bits, 0, 2);  // raw width, ignored for extended
        push(&mut bits, tick_type, 8);
        push(&mut bits, byte_width, 8);
        push(&mut bits, 0, 1);  // sign
        push(&mut bits, value, (byte_width * 8 - 1) as usize);

        let byte_count = bits.len().div_ceil(8);
        let mut payload = vec![0u8; byte_count];
        for (i, &b) in bits.iter().enumerate() {
            if b == 1 {
                payload[i >> 3] |= 1 << (7 - (i & 7));
            }
        }
        let mut tick_payload = Vec::with_capacity(2 + byte_count);
        tick_payload.push((bits.len() >> 8) as u8);
        tick_payload.push((bits.len() & 0xFF) as u8);
        tick_payload.extend_from_slice(&payload);

        let body_len = 5 + tick_payload.len() + 15;
        let mut msg = format!("8=O\x019={body_len}\x01").into_bytes();
        msg.extend_from_slice(b"35=P\x01");
        msg.extend_from_slice(&tick_payload);
        msg.extend_from_slice(b"\x018349=AABBCCDD\x01");
        msg
    }

    /// A magnitude the price scaling cannot represent must leave the previous
    /// quote standing. Wrapping it publishes an arbitrary price — the probe
    /// for this test produces -1000000, a negative price indistinguishable
    /// downstream from a real quote (ibx#272).
    #[test]
    fn a_price_that_cannot_be_scaled_does_not_replace_the_quote() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(7, id);
        context.market.set_min_tick(id, 0.01);

        farm.handle_tick_data(
            &framed_extended(7, tick_decoder::O_LAST_PRICE, 2, 15_000),
            &mut context, &shared, &None,
        );
        let good = context.market.quote(id).last;
        assert!(good > 0, "the ordinary tick must land");

        farm.handle_tick_data(
            &framed_extended(7, tick_decoder::O_LAST_PRICE, 8, u64::MAX >> 1),
            &mut context, &shared, &None,
        );
        assert_eq!(
            context.market.quote(id).last, good,
            "an unrepresentable price must be dropped, leaving the last good quote",
        );
    }

    /// A frame the venue sent for a deep in-the-money call, byte for byte.
    /// Nothing here is constructed: a wrong alignment does not produce a price
    /// that decomposes into the other two fields by accident.
    #[test]
    fn the_venue_states_an_option_model() {
        const FRAME: &[u8] = &[0x7e, 0xf7, 0x20, 0x01, 0x40, 0x57, 0x04, 0x41, 0xc8, 0xf2, 0xf3, 0x45, 0x3f, 0xef, 0xfc, 0x3a, 0xab, 0x98, 0x37, 0xb3, 0x3f, 0x12, 0xf3, 0x0c, 0x1b, 0xcf, 0xac, 0xe7, 0x3f, 0x53, 0x13, 0xaf, 0x03, 0xfc, 0x00, 0x00, 0xbf, 0xa0, 0x60, 0x85, 0xf4, 0x8d, 0x38, 0x00, 0x40, 0x0d, 0x23, 0xdb, 0x03, 0xb8, 0xf5, 0x14, 0x40, 0x71, 0x7c, 0xb2, 0x05, 0x82, 0x74, 0xf0, 0x3f, 0xf0, 0x07, 0x27, 0xcf, 0x01, 0x13, 0xef, 0x40, 0x73, 0x7f, 0x52, 0x20, 0x00, 0x00, 0x00, 0x3f, 0x9f, 0xf2, 0x61, 0x35, 0xdd, 0x42, 0xd9, 0x40, 0x2e, 0x2b, 0xd8, 0x8e, 0x99, 0xfa, 0xb0, 0x3f, 0x1e, 0x54, 0x91, 0xb1, 0x1c, 0x9a, 0x6c, 0xbe, 0xf5, 0x34, 0xf6, 0xa2, 0xc8, 0x61, 0xb4, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3f, 0xb5, 0x32, 0x2a, 0x5c, 0xf4, 0xd4];
        let c = super::decode_greeks(FRAME).expect("the payload is stated valid");
        assert!((c.opt_price - 92.066_515_195_137_14).abs() < 1e-9, "{c:?}");
        assert!((c.delta - 0.999_539_694_925_024_9).abs() < 1e-12, "deep in the money: {c:?}");
        assert!((c.gamma - 0.000_072_286_237_766_827_99).abs() < 1e-15, "{c:?}");
        assert!((c.vega - 0.001_164_360_917_698_559_2).abs() < 1e-15, "{c:?}");
        assert!((c.theta - -0.031_986_414_053_434_94).abs() < 1e-12, "{c:?}");
        assert!((c.und_price - 311.957_550_048_828_1).abs() < 1e-9, "{c:?}");
        assert!((c.implied_vol - 0.031_198_042_786_232_037).abs() < 1e-12, "{c:?}");
        // The strike was 220, so the model price sits just above the
        // intrinsic. A mis-read of the layout does not land there.
        let intrinsic = c.und_price - 220.0;
        assert!(c.opt_price > intrinsic, "worth at least its intrinsic: {c:?}");
        assert!(c.opt_price - intrinsic < 1.0, "and barely more, this close to expiry: {c:?}");
        assert_eq!(c.pv_dividend, f64::MAX, "not stated on this tick");
    }

    /// A payload the venue did not mark valid carries no numbers.
    #[test]
    fn an_invalid_option_model_states_nothing() {
        assert!(super::decode_greeks(&[0u8; 32]).is_none());
        assert!(super::decode_greeks(&[0xff, 0xff, 0xff, 0xfe]).is_none(), "too short to hold one");
    }

    /// A subscription that asks for the option model has to withdraw it too.
    /// Left behind, the venue keeps sending a model for a contract the caller
    /// stopped watching, and nothing holds a request id to stop it by.
    #[test]
    fn cancelling_an_option_withdraws_its_model() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let instrument = context.market
            .try_register_contract(805711629, "AAPL", "OPT", "SMART", "20260821|220|C|100")
            .unwrap();
        let mut conn = None;
        let mut hb = HeartbeatState::new();
        farm.send_mktdata_subscribe(
            805711629, "AAPL", "SMART", "OPT", "20260821", 220.0, "C", "100",
            instrument, 0, &mut conn, &mut hb,
        );
        assert_eq!(farm.greeks_subs.len(), 1, "an option is worth modelling");
        let reqs = &farm.instrument_md_reqs.iter()
            .find(|(id, _)| *id == instrument).expect("its requests").1;
        assert!(reqs.contains(&farm.greeks_subs[0].0), "and the model is one of them, so a cancel finds it");

        farm.send_mktdata_unsubscribe(instrument, &mut conn, &mut hb);
        assert!(farm.greeks_subs.is_empty(), "withdrawn with the rest");
        assert!(farm.instrument_md_reqs.iter().all(|(id, _)| *id != instrument));
    }

    /// Anything without a volatility to imply is not asked to be modelled: the
    /// venue answers such a request with nothing at all.
    #[test]
    fn a_stock_is_not_asked_for_an_option_model() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let instrument = context.market
            .try_register_contract(756733, "SPY", "STK", "SMART", "").unwrap();
        farm.send_mktdata_subscribe(
            756733, "SPY", "SMART", "STK", "", 0.0, "", "",
            instrument, 0, &mut None, &mut HeartbeatState::new(),
        );
        assert!(farm.greeks_subs.is_empty());
    }
}

#[cfg(test)]
mod trading_status_subscribe_tests {
    use super::build_trading_status_subscribe_tags;

    /// The trading status is its own subscription, named by its own tick where
    /// a price subscription names a request type.
    #[test]
    fn the_status_is_asked_for_by_its_own_tick() {
        let tags = build_trading_status_subscribe_tags(7, 756733, "STK", "SMART", "20260810-12:00:00");
        let get = |t: u32| tags.iter().find(|(k, _)| *k == t).map(|(_, v)| v.as_str());
        assert_eq!(get(264), Some("437"), "its own tick, not a request type");
        assert_eq!(get(262), Some("7"), "under the request the prices came under");
        assert_eq!(get(6008), Some("756733"));
    }

    /// It names the contract's own exchange. The option model and the news feed
    /// go by names of their own; everything else is asked for where it trades,
    /// and naming a stand-in here asks a venue that does not list the contract.
    #[test]
    fn it_names_the_exchange_the_contract_trades_on() {
        let tags = build_trading_status_subscribe_tags(1, 1, "STK", "ARCA", "t");
        let venue = tags.iter().find(|(k, _)| *k == 207).map(|(_, v)| v.as_str());
        assert_eq!(venue, Some("ARCA"), "not a stand-in");
    }
}
