use std::time::Instant;

use crate::bridge::{Event, SharedState};
use crate::protocol::datetime::chrono_free_timestamp;
use crate::engine::context::Context;
use crate::protocol::connection::{Connection, Frame};
use crate::protocol::fix;
use crate::protocol::fixcomp;
use crate::protocol::tick_decoder;
use crate::types::{qty_from_counted, InstrumentId};

use super::{HeartbeatState, ReplayPacing, emit, fast_extract_msg_type, find_body_after_tag, EventSink};

/// The venue and security type exactly as a subscription states them on the
/// wire: the caller's blanks filled, and both in the wire's own spelling.
///
/// A withdrawal states an entry the way the subscription stated it, so the
/// one is worked out from the same answer as the other.
fn stated_venue_and_type<'a>(sec_type: &'a str, exchange: &'a str) -> (&'a str, &'a str) {
    let exchange = if exchange.is_empty() { "SMART" } else { exchange };
    (
        crate::control::contracts::exchange_to_fix(exchange),
        crate::control::contracts::sec_type_to_fix(sec_type),
    )
}

/// Build the 35=V subscribe tag list for a contract whose conId is known.
///
/// Kept pure so the wire shape stays unit-testable. SecurityType (167) and
/// Exchange (207) must describe the actual contract: the server routes the
/// subscription by them, and a mismatch is answered with a partial ack rather
/// than an error.
fn build_conid_subscribe_tags(
    realtime: bool,
    regulatory_snapshot: bool,
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
    // the smart-routed stock those callers expect rather than sending an empty
    // SecurityType and Exchange, which the venue answers with a partial ack.
    // A subscription naming only a contract id is not answered; tags 167 and
    // 207 must both be present. Confirmed against a live session: the same
    // contract is answered with them and silent without them.
    //
    // What the contract IS arrives stated: the engine fills a caller's blanks
    // from the venue's own definition and reports the subscription where
    // neither says, because a security type invented here subscribes to some
    // other kind of instrument under this contract's id.
    //
    // Where it is ROUTED is a different question and has a default. The venue
    // requires tag 207 — a subscription omitting it is answered with nothing,
    // measured — and a caller who states a type and no venue means the smart
    // route, which is what every example written against the reference client
    // states for one.
    debug_assert!(!sec_type.is_empty(), "contract {con_id} reached the wire untyped");
    let (fix_exchange, fix_sec_type) = stated_venue_and_type(sec_type, exchange);

    // The chargeable snapshot is a request type of its own and one entry. A
    // stream is the realtime fan-out into BID_ASK and LAST, or the single TOP
    // the delayed and frozen feeds are served on.
    let entries: Vec<(u32, String)> = if regulatory_snapshot {
        vec![(bid_ask_id, REGULATORY_SNAPSHOT_REQUEST_TYPE.to_string())]
    } else if realtime {
        vec![(bid_ask_id, "442".to_string()), (last_id, "443".to_string())]
    } else {
        vec![(bid_ask_id, "1".to_string())]
    };
    // 146 = NoRelatedSym: how many entries follow, counted rather than stated
    // per shape, so a shape added here cannot state the wrong number.
    let mut tags: Vec<(u32, String)> = vec![
        (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
        (fix::TAG_SENDING_TIME, ts.to_string()),
        (263, if regulatory_snapshot { SNAPSHOT_ACTION } else { SUBSCRIBE_ACTION }.to_string()),
        (146, entries.len().to_string()),
    ];

    for (req_id, depth) in &entries {
        tags.push((262, req_id.to_string()));
        tags.push((6008, con_id_str.clone()));
        tags.push((207, fix_exchange.to_string()));
        tags.push((167, fix_sec_type.to_string()));
        tags.push((264, depth.to_string()));
        tags.push((6088, "Socket".to_string()));
        tags.push((9830, "1".to_string()));
        tags.push((9839, "1".to_string()));
        // Named only where it selects between the feeds the venue serves a
        // stream from. The chargeable snapshot is served from none of them and
        // is asked for without it.
        if !realtime && !regulatory_snapshot {
            tags.push((9887, mode_9887.to_string()));
        }
    }
    tags
}

/// Option resub info: (instrument, symbol, exchange, sec_type, last_trade_date, strike,
/// right, multiplier, mode_9887).
type MdResubInfo = (InstrumentId, String, String, String, String, f64, String, String, i32);

/// `MdResubInfo` with the instrument's resolved con_id spliced in behind it.
type MdResubTarget = (InstrumentId, i64, String, String, String, String, f64, String, String, i32);

/// One entry of an L1 subscription as it went out: the number it was asked
/// under, the kind of market data that number carries, and the venue it was
/// asked on.
///
/// Kept so a withdrawal can state each entry the way the subscription stated
/// it. A withdrawal naming a number alone is one the venue leaves being
/// served, and the number then outlives the engine that asked under it: a
/// connection handed on with one still held is answered with silence when
/// the next engine asks under the same number.
pub(crate) struct MdReqEntry {
    pub(crate) req_id: u32,
    pub(crate) request_type: u32,
    pub(crate) venue: String,
}

/// An instrument's L1 subscriptions as they went out, so each can be
/// withdrawn as itself: the contract and venue and type each entry named.
pub(crate) struct MdReqRecord {
    pub(crate) con_id: i64,
    /// The security type in the wire's own spelling, the one the entries
    /// were asked for under.
    pub(crate) sec_type: String,
    /// The feed a delayed or frozen quote was asked for, stated beside the
    /// entry and so beside its withdrawal too. Zero where none was.
    pub(crate) mode_9887: i32,
    pub(crate) entries: Vec<MdReqEntry>,
}

pub(crate) struct FarmState {
    /// Subscriptions a reconnect has still to put back, and the earliest the
    /// next burst may go out.
    ///
    /// A server that has just come back up is the least able to take a whole
    /// book at once, so the replay is paced. Sleeping for that pace would stop
    /// the one thread driving every transport, the heartbeats, the reconnects
    /// and shutdown, so the queue is drained a burst at a time on the passes
    /// the loop is already making.
    pub(crate) replay_queue: std::collections::VecDeque<MdResubTarget>,
    /// When the next burst may go out. `None` means there is nothing waiting.
    pub(crate) replay_not_before: Option<Instant>,
    /// The next id this client asks the venue under.
    ///
    /// Every subscription is asked for under one of these and mapped back to
    /// the caller who wanted it. A caller's own id is never sent: the venue
    /// echoes an id back, and one taken from the caller is indistinguishable
    /// from one of these — a caller's second book was answered under another
    /// subscription's venue because both were numbered 2.
    pub(crate) next_md_req_id: u32,
    pub(crate) md_req_to_instrument: Vec<(u32, InstrumentId)>,
    pub(crate) instrument_md_reqs: Vec<(InstrumentId, MdReqRecord)>,
    /// Venue numbers whose quotes this session has no contract for, each said
    /// once. A quote naming one is dropped, and dropped silently there is no
    /// way to tell a venue that stopped sending from one still sending under a
    /// number nobody here holds.
    quotes_for_no_one: std::collections::HashSet<u32>,
    /// Active depth subscriptions: (req_id, is_smart_depth).
    pub(crate) depth_subs: Vec<(u32, bool)>,
    /// How deep each caller asked its book to be, by the caller's own id.
    ///
    /// The venue sends the levels it has and this number is not on the wire —
    /// the reference client asks for a depth and then shows that many, so a
    /// caller that asked for five and was handed ten was handed a book it did
    /// not ask for.
    depth_rows: Vec<(u32, i32)>,
    /// Which exchange each fanned-out depth subscription is for, so a level
    /// says where it stands. Without it every level of a smart book arrived
    /// unattributed, and a caller was handed one book with no way to tell
    /// which venue any of it was on.
    pub(crate) depth_fanout_exchange: Vec<(u32, String)>,
    /// Maps server_tag → (depth_req_id, is_smart_depth, min_tick) for active depth
    /// subscriptions.
    pub(crate) depth_tag_to_req: Vec<(u32, u32, bool, f64, f64, String)>,
    /// Every book: the id this client asked the venue under, against the id
    /// the caller asked for it under.
    ///
    /// Not only the smart-routed fan-out, whatever it was once for — the
    /// subscribe path fills this for every book, and withdrawing them all at a
    /// stop reads the caller's side of it.
    pub(crate) depth_fanout_map: Vec<(u32, u32)>,
    /// Primary depth subscription params for reconnect: (req_id, con_id, exchange,
    /// sec_type, num_rows, is_smart_depth).
    depth_resub_info: Vec<(u32, i64, String, String, String, i32, bool)>,
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
    /// Which instruments a batch of ticks has already published, so one that
    /// several ticks moved is published once. Kept between batches and cleared
    /// bit by bit as each is published, so the cost is the batch rather than
    /// the size of the table.
    notified: Box<[u64]>,
    /// The instruments those bits stand for, in the order they were touched.
    notified_ids: Vec<crate::types::InstrumentId>,
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

/// The days the venue states its option model's volatility and rate over.
///
/// It counts the life of a contract in days and states both figures over one
/// of them. The reference client states them over a year, which is what a
/// caller reads them as, so this is what carries them across.
const A_YEAR_OF_DAYS: f64 = 365.0;

/// The venue's chargeable one-shot snapshot, on tag 264.
///
/// A request type of its own rather than a mode on an ordinary quote: the
/// venue names it `regsshot` when it refuses one, and an account without the
/// entitlement is refused by name. It is asked for under the snapshot action
/// below and never with a feed named beside it.
const REGULATORY_SNAPSHOT_REQUEST_TYPE: u32 = 624;

/// Deliver the request once, on tag 263, in place of subscribing to it.
const SNAPSHOT_ACTION: &str = "3";

/// Open a subscription, on tag 263.
const SUBSCRIBE_ACTION: &str = "1";

/// The trading-status tick's own number, in place of a request type.
const TRADING_STATUS_REQUEST_TYPE: u32 = 437;

/// The tick that states which venue each bit of a quote's exchange mask means.
///
/// The server sends the map itself — every venue's name and the character it
/// is reported under, in the order the bits refer to. A table written into
/// this client's own source would name nothing for any venue absent from it,
/// and could not be checked against what the server assigns.
const BBO_EXCHANGE_MAP_REQUEST_TYPE: u32 = 626;

/// The kind of market data a subscription asks for, on tag 264.
///
/// A book is `Deep`. A quote is asked for under other numbers, and the two are
/// not interchangeable in either direction: a book asked for as a quote is
/// acknowledged and never sent, and the quote frames a venue does answer with
/// are not a book — read as one they become a bid of 143.87 and an ask of a
/// penny on a share trading at 772.
const DEEP_REQUEST: &str = "0";

/// An increment an acknowledgement states, where it states one that can be
/// counted in.
///
/// Every price and every size on the instrument is a count of these. Zero
/// counts nothing, a negative one turns every figure on the contract upside
/// down, and an infinity — which this parser reads from the word — saturates
/// them all. None of the three is a scale, so an acknowledgement stating one
/// states no increment at all.
fn stated_increment(field: &str) -> Option<f64> {
    let stated: f64 = field.trim().parse().ok()?;
    (stated.is_finite() && stated > 0.0).then_some(stated)
}

/// What the venue counts an instrument's sizes in, as its acknowledgement
/// states it: the last field, after the increment prices move in.
///
/// A size on the wire is a count of these. Whole ones for a share, and
/// hundred-millionths for a crypto — so a count taken as whole ones reports
/// one of the two a hundred million times over. An acknowledgement that
/// states none is counted in whole ones, which is what stating none means.
fn trailing_size_increment(parts: &[&str]) -> Option<f64> {
    // Only on an acknowledgement long enough to be one that carries it.
    // Captured off the wire, the two shapes are five fields for a ticker
    // setup — contract, price increment, server tag, an empty field, the size
    // increment — and nine for a subscription, ending the same way. Every
    // neighbouring field parses as a positive number, so a shorter
    // acknowledgement than either would not fail this: it would quietly hand
    // back the field before it, and a server tag read as a size increment
    // multiplies every size on the contract by five figures.
    const SHORTEST_THAT_CARRIES_ONE: usize = 5;
    if parts.len() < SHORTEST_THAT_CARRIES_ONE {
        return None;
    }
    stated_increment(parts.last()?)
}


/// What the option model goes by where an exchange would be named.
const GREEKS_VENUE: &str = "IBVOL";

/// The venue refusing to serve data this account is not subscribed to.
///
/// The number the venue reports that under. It was this client's own
/// number for a malformed request, which said the caller had asked wrongly
/// when the venue had simply declined to serve it — a caller branching on it
/// the way it would against the reference client read a refusal it could fix
/// as one it could not.
///
/// The refusal itself carries no number: the venue names the request and says
/// why in words, and the number is the one a caller is owed for those words.
pub(super) const DEPTH_VENUE_REFUSED: i32 = 354;

/// The option model's own tick, in place of a request type.
const GREEKS_REQUEST_TYPE: u32 = 732;


/// The news tick's own number, in place of a request type.
const NEWS_REQUEST_TYPE: u32 = 292;

/// How a generic tick states the length of its payload.
///
/// Which of the three a tick uses is not on the wire: it is a property of the
/// tick, held as the protocol states it. Guessing from the frame
/// works until two readings both square with it, and then it misframes every
/// record after the one it got wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayloadLength {
    /// One byte. What most ticks use, the option model and the trading status
    /// among them.
    OneByte,
    /// Two bytes.
    TwoBytes,
    /// None at all: the payload runs to the end of the message, so a message
    /// carries exactly one of these.
    ToTheEnd,
}

/// The ticks that state their payload's length in two bytes.
const TWO_BYTE_LENGTH_TICKS: [u32; 28] = [
    247, 256, 257, 258, 292, 385, 386, 434, 454, 481, 490, 491, 496, 546, 593, 594, 628, 631,
    633, 669, 678, 687, 691, 699, 700, 703, 705, 726,
];

/// The ticks that state no length, whose payload runs to the end.
const NO_LENGTH_TICKS: [u32; 7] = [221, 320, 376, 530, 532, 619, 787];

impl PayloadLength {
    fn of(tick: u32) -> Self {
        if TWO_BYTE_LENGTH_TICKS.contains(&tick) {
            Self::TwoBytes
        } else if NO_LENGTH_TICKS.contains(&tick) {
            Self::ToTheEnd
        } else {
            Self::OneByte
        }
    }

    /// How many bytes stand between the start of a record and its payload.
    fn header(self) -> usize {
        match self {
            Self::ToTheEnd => 4,
            Self::OneByte => 5,
            Self::TwoBytes => 6,
        }
    }
}

/// The length of a generic tick message, in bytes, from the length it states
/// and how much arrived with it.
///
/// The venue states it in bits in two bytes, so it wraps at sixty-five
/// thousand five hundred and thirty-six bits — eight thousand one hundred and
/// ninety-two bytes. What was carried is recovered against how much actually
/// arrived: a message longer than that would otherwise be cut off in the
/// middle with nothing to say it had been.
fn generic_tick_length(stated_bits: u16, arrived: usize) -> Option<usize> {
    let mut bits = stated_bits as usize;
    while bits + 65_536 < arrived * 8 {
        bits += 65_536;
    }
    if !bits.is_multiple_of(8) {
        return None;
    }
    Some(bits / 8)
}

/// One generic tick record: the venue's number for the subscription, and the
/// payload under it.
///
/// A message carries one or more of these, one after another. Nothing on a
/// record says which tick it carries — that is what the subscription said —
/// so reading one takes knowing, from its number, how the tick that was asked
/// for states its length.
struct GenericTickRecord<'a> {
    server_tag: u32,
    payload: &'a [u8],
}

/// Read the records in a generic tick message, asking `tick_of` what tick each
/// number was asked for under.
///
/// A number nothing asked for stops the reading rather than skipping the
/// record: without knowing the tick, there is no knowing where the record
/// ends, and carrying on would read the next one from the middle of this one.
fn read_generic_ticks<'a>(
    body: &'a [u8],
    mut tick_of: impl FnMut(u32) -> Option<u32>,
    mut each: impl FnMut(u32, GenericTickRecord<'a>),
) {
    let Some(stated_bits) = body.get(..2) else { return };
    let stated_bits = u16::from_be_bytes([stated_bits[0], stated_bits[1]]);
    let Some(length) = generic_tick_length(stated_bits, body.len()) else {
        return;
    };
    let Some(frame) = body.get(2..2 + length) else { return };

    let mut at = 0usize;
    while at + 4 <= frame.len() {
        let server_tag =
            u32::from_be_bytes([frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]);
        let Some(tick) = tick_of(server_tag) else {
            log::debug!(
                "A generic tick arrived under server tag {server_tag}, which nothing here asked \
                 for. Without the tick there is no knowing where its record ends, so the rest of \
                 the message goes unread",
            );
            return;
        };
        let form = PayloadLength::of(tick);
        let stated = match form {
            PayloadLength::ToTheEnd => frame.len() - at - form.header(),
            PayloadLength::OneByte => match frame.get(at + 4) {
                Some(&n) => n as usize,
                None => return,
            },
            PayloadLength::TwoBytes => match frame.get(at + 4..at + 6) {
                Some(n) => u16::from_be_bytes([n[0], n[1]]) as usize,
                None => return,
            },
        };
        let from = at + form.header();
        let Some(payload) = frame.get(from..from + stated) else {
            log::debug!(
                "A generic tick under server tag {server_tag} states {stated} bytes of payload \
                 and the message does not carry them; the rest goes unread",
            );
            return;
        };
        each(tick, GenericTickRecord { server_tag, payload });
        at = from + stated;
    }
}

/// The text inside a tick payload that states its own length.
///
/// Four bytes of big-endian length, then that many bytes of text, then padding
/// to a four-byte boundary. Read end to end as text, the length bytes join the
/// first name and the padding joins the last.
fn length_prefixed_text(payload: &[u8]) -> Option<std::borrow::Cow<'_, str>> {
    let stated = u32::from_be_bytes(payload.get(..4)?.try_into().ok()?) as usize;
    Some(String::from_utf8_lossy(payload.get(4..4 + stated)?))
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
    // Which volatility the model was given, not a figure: it states no bytes
    // and the walk below steps over nothing for it.
    const PRICE_BASED_VOL: u32 = 1 << 24;
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
    let cal_days = next(flags & CALENDAR_DAYS != 0);
    let daily_rate = next(flags & DAILY_RATE != 0);
    let _model_yield = next(flags & MODEL_YIELD != 0);
    let _bridge_yield = next(flags & BRIDGE_YIELD != 0);
    let _time_value = next(flags & TIME_VALUE != 0);

    // Below the walk, so nothing here can be mistaken for a field and step the
    // read position. Both figures are stated over one of the days counted
    // beside them, and both are handed on over a year: that is the scale the
    // reference client reports them on, and the scale a caller reads every
    // other volatility against. A volatility grows with the root of time, so
    // it carries over by the root of the days in a year; a rate accrues with
    // time itself.
    //
    // The sentinel is left alone. It is not a figure, and scaling it would
    // turn the mark for "not stated" into an infinity.
    let over_a_year = |v: f64, by: f64| if v == UNSTATED { UNSTATED } else { v * by };
    let implied_vol = over_a_year(implied_vol, A_YEAR_OF_DAYS.sqrt());
    let rate = over_a_year(daily_rate, A_YEAR_OF_DAYS);

    Some(crate::types::OptionComputation {
        // The venue's, reported under whichever request subscribed it.
        answers: None,
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
        cal_days,
        rate,
        price_based_vol: flags & PRICE_BASED_VOL != 0,
    })
}

impl FarmState {
    /// Whether this instrument still has market-data state that would be
    /// repointed by a slot reuse: a live subscription, or a record kept for the
    /// next reconnect.
    ///
    /// The resubscribe record is the reason this matters. Before it existed a
    /// reclaimed slot replayed nothing; now it replays the old contract's
    /// descriptor against whatever contract holds the id at reconnect.
    ///
    /// `md_req_to_instrument` is deliberately not consulted. A subscribe fills
    /// it and `instrument_md_reqs` together, so it says nothing the first check
    /// does not already cover — and an entry left there after an unsubscribe is
    /// a defect in its own right, not a reason to pin the slot. Held
    /// on that basis, a subscribe/unsubscribe cycle would consume a slot
    /// permanently and the instrument cap would become cumulative per session,
    /// which is the failure exists to prevent.
    pub(crate) fn holds_market_data(&self, instrument: InstrumentId) -> bool {
        self.instrument_md_reqs.iter().any(|(id, _)| *id == instrument)
            || self.md_resub_info.iter().any(|r| r.0 == instrument)
            // And the queue the reconnect replays from. Between the reconnect
            // and the last burst, `md_resub_info` has been taken and
            // `instrument_md_reqs` is not refilled until each subscription is
            // sent, so an instrument waiting its turn is held here and nowhere
            // else. Missing it, the slot reads as free while a subscription for
            // it is still to go out, and the replay binds this contract's
            // server tag and minimum tick onto whatever contract took the slot.
            || self.replay_queue.iter().any(|r| r.0 == instrument)
    }

    pub(crate) fn new() -> Self {
        Self {
            replay_queue: Default::default(),
            replay_not_before: None,
            next_md_req_id: 1,
            md_req_to_instrument: Vec::new(),
            instrument_md_reqs: Vec::new(),
            quotes_for_no_one: std::collections::HashSet::new(),
            depth_subs: Vec::new(),
            depth_rows: Vec::new(),
            depth_tag_to_req: Vec::new(),
            depth_fanout_map: Vec::new(),
            depth_fanout_exchange: Vec::new(),
            depth_resub_info: Vec::new(),
            md_resub_info: Vec::new(),
            greeks_subs: Vec::new(),
            generic_tick_reqs: Vec::new(),
            generic_tick_tags: Vec::new(),
            unread_types: std::collections::HashSet::new(),
            disconnected: false,
            tick_buf: Vec::with_capacity(16),
            notified: vec![0u64; crate::types::MAX_INSTRUMENTS / 64].into(),
            notified_ids: Vec::with_capacity(16),
            farm_msg_buf: Vec::with_capacity(32),
        }
    }

    pub(crate) fn poll_market_data(
        &mut self,
        farm_conn: &mut Option<Connection>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<EventSink>,
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
                // Nothing new on the socket, but a frame may already be whole
                // in the buffer — the other three pollers make that
                // distinction and this one did not.
                Ok(0) if !conn.has_buffered_data() => return,
                Ok(0) => {}
                Err(e) => {
                    log::error!("Farm connection lost: {e}");
                    self.handle_disconnect(farm_conn, context, event_tx);
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
                    // 8=1 / 8=X control state — not consumed on the farm path.
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
        event_tx: &Option<EventSink>,
        hb: &mut HeartbeatState,
    ) {
        let msg_type = match fast_extract_msg_type(msg) {
            Some(t) => t,
            None => return,
        };
        // Every message this connection carries, kept whole when asked. What a
        // subscription answers with is a question the wire answers; a reading
        // of it is not evidence of it.
        if *crate::engine::hot_loop::CAPTURE_WIRE {
            let hex: String = msg.iter().map(|b| format!("{b:02x}")).collect();
            shared.market.note_unread_wire("farm-msg", hex);
        }
        match msg_type {
            b"P" => self.handle_tick_data(msg, context, shared, event_tx),
            b"Q" => {
                if std::env::var("IBX_TRACE_Q").is_ok() {
                    log::info!("35=Q body: {}", String::from_utf8_lossy(msg).replace('\x01', "|"));
                }
                log::info!("Farm 35=Q subscription ack received");
                self.handle_subscription_ack(msg, context, shared);
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
            b"L" => self.handle_ticker_setup(msg, context, shared),
            b"UT" | b"UM" | b"RL" => super::ccp::positions::handle_account_update(msg, context, shared),
            b"UP" => {
                // One frame names several holdings. A flat parse keeps only
                // the last value of each tag.
                for parsed in super::ccp::positions::split_position_entries(msg) {
                    super::ccp::positions::handle_position_update(&parsed, context, shared, event_tx);
                }
            }
            // The venue refusing a subscription it was asked for, naming the
            // request that asked. Dropped, a caller that asked for depth on a
            // venue this account cannot see waits for data that was refused
            // before it started.
            b"3" => self.handle_subscription_reject(msg, context, shared),
            b"Y" => self.handle_depth_35y(msg, shared),
            b"G" => self.handle_generic_tick(msg, context, shared, event_tx),
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

    fn handle_tick_data(&mut self, msg: &[u8], context: &mut Context, shared: &SharedState, event_tx: &Option<EventSink>) {
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
        // Which instruments this batch touched, and each of them once. The
        // membership set is kept rather than rebuilt so nothing is zeroed per
        // batch, and the list beside it is what the publish loop walks — so
        // the work is the number of instruments in the batch rather than the
        // size of the table.
        let mut notified = std::mem::take(&mut self.notified);
        let mut notified_ids = std::mem::take(&mut self.notified_ids);

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
                None => {
                    // Said once for the number rather than once for the quote:
                    // a stream sends as fast held or not, and a line each would
                    // be the whole log. A number this session gave up is the
                    // ordinary case — what was in flight when the withdrawal
                    // went out — and is not worth saying at all.
                    if self.quotes_for_no_one.insert(tick.server_tag)
                        && !context.market.retired_server_tags().contains(&tick.server_tag)
                    {
                        log::warn!(
                            "quotes are arriving under venue number {}, which no contract \
                             in this session holds and which it never gave up; dropped",
                            tick.server_tag,
                        );
                    }
                    continue;
                }
            };

            let mts = context.market.min_tick_scaled(instrument);
            // A size is a count of what the venue said sizes move in for this
            // contract, the same way a price is a count of what prices move in.
            let size_tick = context.market.size_tick(instrument);
            let (q, clock) = context.market.quote_and_clock_mut(instrument);

            // The extended 35=P format carries a full 8-bit byte width, so a
            // magnitude can reach i64::MAX and the scaling multiply wrapped to
            // an arbitrary price in release builds. Drop the price
            // rather than pinning it: a saturated value is ~92e9 at
            // PRICE_SCALE, which reads downstream as an ordinary quote, and
            // leaving the previous one standing is the honest failure.
            // And an increment of nothing scales every price to nothing,
            // which reads downstream as a real quote of zero rather than as
            // the absent increment it is. The venue states one per contract
            // and this is what happens when it has not, or when it states one
            // finer than the scale can hold. The tick-by-tick path already
            // refuses on the same test.
            let scaled = |m: i64| if mts > 0 { m.checked_mul(mts) } else { None };
            // Whether this entry changed the quote. An unmapped opcode does
            // not mark the instrument notified; it is visible in the raw trace
            // above instead.
            //
            // A price too large to scale is refused above, leaving the quote
            // unchanged, so it does not mark the instrument notified either.
            let mut applied = true;
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
                        None => {
                            log::warn!("35=P bid price out of range (magnitude={}), tick dropped", tick.magnitude);
                            applied = false;
                        }
                    }
                }
                tick_decoder::O_ASK_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.ask = v,
                        None => {
                            log::warn!("35=P ask price out of range (magnitude={}), tick dropped", tick.magnitude);
                            applied = false;
                        }
                    }
                }
                tick_decoder::O_LAST_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.last = v,
                        None => {
                            log::warn!("35=P last price out of range (magnitude={}), tick dropped", tick.magnitude);
                            applied = false;
                        }
                    }
                }
                tick_decoder::O_HIGH_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.high = v,
                        None => {
                            log::warn!("35=P high price out of range (magnitude={}), tick dropped", tick.magnitude);
                            applied = false;
                        }
                    }
                }
                tick_decoder::O_LOW_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.low = v,
                        None => {
                            log::warn!("35=P low price out of range (magnitude={}), tick dropped", tick.magnitude);
                            applied = false;
                        }
                    }
                }
                tick_decoder::O_OPEN_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.open = v,
                        None => {
                            log::warn!("35=P open price out of range (magnitude={}), tick dropped", tick.magnitude);
                            applied = false;
                        }
                    }
                }
                tick_decoder::O_CLOSE_PRICE => {
                    match scaled(tick.magnitude) {
                        Some(v) => q.close = v,
                        None => {
                            log::warn!("35=P close price out of range (magnitude={}), tick dropped", tick.magnitude);
                            applied = false;
                        }
                    }
                }
                // Quantities are fixed-point, the same way prices are; every
                // reader divides by `QTY_SCALE` on the way out.
                tick_decoder::O_BID_SIZE => { q.bid_size = qty_from_counted(tick.magnitude, size_tick); }
                tick_decoder::O_ASK_SIZE => { q.ask_size = qty_from_counted(tick.magnitude, size_tick); }
                tick_decoder::O_LAST_SIZE => { q.last_size = qty_from_counted(tick.magnitude, size_tick); }
                tick_decoder::O_VOLUME => { q.volume = qty_from_counted(tick.magnitude, size_tick); }
                // Type 20 carries Unix seconds. Guarded because the same
                // type also carried a `yyyymmdd` value in capture, and a date
                // read as an epoch is worse than no timestamp.
                //
                // Type 21 is the seconds since that base, and the two add.
                // The reference client keeps them as a pair, per stream, and
                // settles it the same way: a base is stored and clears the
                // offset, and an offset only ever moves forward. Left out, a quote carried
                // the time of the last base for as long as one stood — every
                // print in between stamped with the same second.
                // Type 23 was previously folded in here and is now dropped:
                // it did not appear once in 733 captured entries on a future,
                // and it was writing a raw magnitude of unknown unit into a
                // nanosecond field. Left unmapped until a capture identifies
                // it rather than guessed at.
                tick_decoder::O_TS_BASE if tick.magnitude > 1_000_000_000 => {
                    clock.base_secs = tick.magnitude as u64;
                    clock.offset_secs = 0;
                    q.timestamp_ns = clock.base_secs.saturating_mul(1_000_000_000);
                }
                // Only ever forward, as the reference client keeps it: an
                // offset behind the one in hand belongs to a base that has
                // been replaced.
                tick_decoder::O_TS_OFFSET if clock.base_secs > 0 => {
                    let offset = tick.magnitude.max(0) as u64;
                    if offset >= clock.offset_secs {
                        clock.offset_secs = offset;
                        q.timestamp_ns = clock.base_secs
                            .saturating_add(offset)
                            .saturating_mul(1_000_000_000);
                    } else {
                        applied = false;
                    }
                }
                tick_decoder::O_BID_EXCH => { q.bid_exch_mask = tick.magnitude; }
                tick_decoder::O_ASK_EXCH => { q.ask_exch_mask = tick.magnitude; }
                tick_decoder::O_LAST_EXCH => { q.last_exch_mask = tick.magnitude; }
                _ => applied = false,
            }

            if applied {
                let word = &mut notified[(instrument >> 6) as usize];
                let bit = 1u64 << (instrument & 63);
                if *word & bit == 0 {
                    *word |= bit;
                    notified_ids.push(instrument);
                }
            }
        }

        // Phase 2: Publish complete quotes after all ticks in the batch are applied.
        for &instrument in &notified_ids {
            shared.market.push_quote(instrument, context.quote(instrument));
            emit(event_tx, Event::Tick(instrument));
            notified[(instrument >> 6) as usize] &= !(1u64 << (instrument & 63));
        }
        notified_ids.clear();
        self.notified = notified;
        self.notified_ids = notified_ids;
        self.tick_buf = ticks;
    }

    fn handle_subscription_ack(&mut self, msg: &[u8], context: &mut Context, shared: &SharedState) {
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
        let Some(min_tick) = stated_increment(parts[2]) else {
            let told = format!(
                "this subscription was acknowledged with an increment prices cannot be \
                 counted in ({}), so none of its prices can be worked out",
                parts[2],
            );
            log::warn!("{told}");
            // Told to whoever asked. Left as a log line, the subscription
            // stands with no scale and every price on it is dropped, which
            // reads as a contract nobody is quoting.
            if let Some((_, instrument)) =
                self.md_req_to_instrument.iter().find(|(id, _)| *id == req_id)
            {
                shared.market.push_subscription_failure(*instrument, told);
            } else if let Some((_, asked_for)) =
                self.depth_fanout_map.iter().find(|(sub, _)| *sub == req_id)
            {
                shared.reference.push_historical_error(*asked_for, DEPTH_VENUE_REFUSED, told);
            }
            return;
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
            // Which venue this subscription's levels stand on. Every
            // subscription is asked for at one named venue, so every level has
            // one to be placed on.
            let venue = self.depth_fanout_exchange.iter()
                .find(|(sub, _)| *sub == req_id)
                .map(|(_, exch)| exch.clone())
                .unwrap_or_default();
            // The venue answers a second subscription on a contract and venue
            // it is already streaming with the tag it is already using, so two
            // acks can name the same one. Two records then match every update
            // and the book applies each level twice. The generic-tick branch
            // below already keeps one record per tag.
            self.depth_tag_to_req.retain(|(tag, id, ..)| {
                !(*tag == server_tag && *id == user_req)
            });
            self.depth_tag_to_req.push((
                server_tag,
                user_req,
                is_smart,
                min_tick,
                trailing_size_increment(&parts).unwrap_or(1.0),
                venue.clone(),
            ));
            log::info!("Depth ack: server_tag {server_tag} -> req_id {user_req} venue={venue:?} (levels={depth_levels}, smart={is_smart}, min_tick={min_tick})");
            return;
        }

        // L1 ack. A number this session has given up names a subscription that
        // is over, whatever request id the answer carries: the caller's ids are
        // its own and it may ask again under one it used before, and the first
        // request's answer arriving second would point the second at a number
        // nothing comes on.
        if context.market.retired_server_tags().contains(&server_tag) {
            log::warn!(
                "an answer names venue number {server_tag}, which this session has given \
                 up; it belongs to a subscription that is over and is not taken as the \
                 answer to a later one",
            );
            return;
        }
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
            // refusal does not end the caller's request: one book is asked for
            // at several venues and the others may answer. The caller asked
            // once, so they are told once — when the last venue has refused
            // and there is no book coming.
            None => {
                log::warn!("The venue refused a subscription: {reason}");
                let Some(rid) = req_id else { return };
                let fanned_out = self.depth_fanout_map.iter()
                    .find(|(sub, _)| *sub == rid)
                    .map(|(_, user)| *user);
                // Naming nothing this client still asks under, the book was
                // withdrawn or refused before this arrived, and there is no
                // caller to tell. Handed on as it stood, the wire number was
                // published as though it were a caller's request number, and
                // whoever held that number was told a book had been refused.
                let Some(asked_for) = fanned_out else {
                    log::info!("the refusal names {rid}, which no book here asks under any more");
                    return;
                };
                self.depth_fanout_map.retain(|(sub, _)| *sub != rid);
                if self.depth_fanout_map.iter().any(|(_, u)| *u == asked_for) {
                    return;
                }
                shared.reference.push_historical_error(
                    asked_for,
                    DEPTH_VENUE_REFUSED,
                    format!("the venue refused depth here: {reason}"),
                );
            }
        }
    }

    fn handle_ticker_setup(&mut self, msg: &[u8], context: &mut Context, shared: &SharedState) {
        let body = match find_body_after_tag(msg, b"35=L\x01") {
            Some(b) => b,
            None => return,
        };
        let text = String::from_utf8_lossy(body);
        let text = text.split("\x018349=").next().unwrap_or(&text);
        let parts: Vec<&str> = text.trim().split(',').collect();
        if parts.len() < 3 { return; }
        let con_id: i64 = match parts[0].parse() { Ok(v) => v, Err(_) => return };
        let Some(min_tick) = stated_increment(parts[1]) else {
            let told = format!(
                "a depth subscription was acknowledged with an increment prices cannot \
                 be counted in ({}), so none of its levels can be worked out",
                parts[1],
            );
            log::warn!("{told}");
            if let Some(instrument) = context.market.instrument_by_con_id(con_id) {
                shared.market.push_subscription_failure(instrument, told);
            }
            return;
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
        regulatory_snapshot: bool,
        farm_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
    ) {
        // Realtime fans out into BID_ASK + LAST; frozen/delayed/delayed-frozen
        // collapse to a single 264=1 (TOP) sub with 9887=mode_9887. The
        // chargeable snapshot is one entry whatever the feed, so it takes one
        // id and has no second leg to route.
        let realtime = mode_9887 == 0 && !regulatory_snapshot;
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

        let venue_map_req_id = self.next_md_req_id;
        self.next_md_req_id += 1;
        self.md_req_to_instrument.push((venue_map_req_id, instrument));
        self.generic_tick_reqs.push((venue_map_req_id, BBO_EXCHANGE_MAP_REQUEST_TYPE));
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

        // Each entry as it goes onto the wire: the number it is asked under,
        // the kind of market data that number carries, and the venue it is
        // asked on. Kept so the withdrawal can state each one the way the
        // subscription stated it — named by number alone, the venue leaves it
        // being served.
        let (venue, wire_sec_type) = stated_venue_and_type(sec_type, exchange);
        let venue = venue.to_string();
        let mut entries = if realtime {
            vec![
                MdReqEntry { req_id: bid_ask_id, request_type: 442, venue: venue.clone() },
                MdReqEntry { req_id: last_id, request_type: 443, venue: venue.clone() },
            ]
        } else if regulatory_snapshot {
            vec![MdReqEntry { req_id: bid_ask_id, request_type: REGULATORY_SNAPSHOT_REQUEST_TYPE, venue: venue.clone() }]
        } else {
            vec![MdReqEntry { req_id: bid_ask_id, request_type: 1, venue: venue.clone() }]
        };
        entries.push(MdReqEntry { req_id: status_req_id, request_type: TRADING_STATUS_REQUEST_TYPE, venue: venue.clone() });
        entries.push(MdReqEntry { req_id: venue_map_req_id, request_type: BBO_EXCHANGE_MAP_REQUEST_TYPE, venue: venue.clone() });
        if let Some(id) = greeks_req_id {
            entries.push(MdReqEntry { req_id: id, request_type: GREEKS_REQUEST_TYPE, venue: GREEKS_VENUE.to_string() });
        }
        match self.instrument_md_reqs.iter_mut().find(|(id, _)| *id == instrument) {
            Some((_, record)) => record.entries.extend(entries),
            None => {
                self.instrument_md_reqs.push((instrument, MdReqRecord {
                    con_id,
                    sec_type: wire_sec_type.to_string(),
                    mode_9887,
                    entries,
                }));
            }
        }
        // A chargeable snapshot is not recorded for replay: the caller asked
        // for the contract once, and a reconnect that re-sent it would deliver
        // — and bill for — a second burst nobody asked for.
        if !regulatory_snapshot && self.md_resub_info.iter().all(|(id, ..)| *id != instrument) {
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
                    realtime, regulatory_snapshot, bid_ask_id, last_id, con_id, exchange, sec_type,
                    mode_9887, &ts,
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

                // And which venue each bit of the exchange mask means, which
                // the server states rather than this client assuming.
                let mut venue_map = build_trading_status_subscribe_tags(
                    venue_map_req_id, con_id, sec_type, exchange, &ts,
                );
                for (tag, value) in venue_map.iter_mut() {
                    if *tag == 264 {
                        *value = BBO_EXCHANGE_MAP_REQUEST_TYPE.to_string();
                    }
                }
                let refs: Vec<(u32, &str)> =
                    venue_map.iter().map(|(tag, val)| (*tag, val.as_str())).collect();
                let _ = conn.send_fixcomp(&refs);
            } else {
                // No con_id — send descriptive fields
                let strike_str = if strike > 0.0 { strike.to_string() } else { String::new() };
                let mut tags: Vec<(u32, &str)> = vec![
                    (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                    (fix::TAG_SENDING_TIME, &ts),
                    (263, if regulatory_snapshot { SNAPSHOT_ACTION } else { SUBSCRIBE_ACTION }),
                    (146, no_related_sym),
                ];
                let snapshot_type = REGULATORY_SNAPSHOT_REQUEST_TYPE.to_string();
                let entries: &[(&String, &str)] = if regulatory_snapshot {
                    &[(&bid_ask_str, &snapshot_type)]
                } else if realtime {
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
                    if !realtime && !regulatory_snapshot { tags.push((9887, &mode_str)); }
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
        // the caller explicitly cancelled.
        self.md_resub_info.retain(|(id, ..)| *id != instrument);
        // And out of the replay queue, for the same reason. A reconnect moves
        // the records here and empties the list above, so between the reconnect
        // and the last burst this is the only place the subscription is
        // written down — left standing, the replay re-sends a subscription the
        // caller has just cancelled.
        self.replay_queue.retain(|r| r.0 != instrument);
        let record = match self.instrument_md_reqs.iter()
            .position(|(id, _)| *id == instrument)
        {
            Some(idx) => self.instrument_md_reqs.remove(idx).1,
            None => return,
        };
        let reqs: Vec<u32> = record.entries.iter().map(|e| e.req_id).collect();
        // Forget the pending acks too. A `35=Q` still in flight when the
        // unsubscribe goes out would otherwise resolve its request id after
        // the slot has been reclaimed and reused, binding this subscription's
        // server_tag AND its minTick onto whatever contract now holds the
        // slot — prices for the new contract then scale by the old one's tick
        // size, which reads as plausible rather than broken.
        self.md_req_to_instrument.retain(|(req_id, _)| !reqs.contains(req_id));
        // And what was asked for under those requests, for the same reason: a
        // number the venue hands to the next subscription would otherwise
        // still be read as the tick this one asked for.
        self.generic_tick_reqs.retain(|(req_id, _)| !reqs.contains(req_id));
        self.generic_tick_tags.retain(|(_, _, held)| *held != instrument);
        // The option-model records go with the withdrawal whether or not the
        // farm is up: left behind, they outlive the subscription they
        // describe.
        self.greeks_subs.retain(|(id, ..)| !reqs.contains(id));

        let conn = match farm_conn.as_mut() {
            Some(c) => c,
            None => return,
        };

        let asked = record.entries.len();
        let con_id_str = (record.con_id as u32).to_string();
        for entry in &record.entries {
            // Withdrawn the way it was asked for: the venue is told which
            // tick, on which contract, on which venue, not merely which
            // request. Named by the number alone the venue leaves it being
            // served, and the number stays held against whoever asks under it
            // next on this connection.
            let req_id_str = entry.req_id.to_string();
            let request_type_str = entry.request_type.to_string();
            let mut tags: Vec<(u32, &str)> = vec![
                (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                (263, "2"),
                (146, "1"),
                (262, &req_id_str),
                (6008, &con_id_str),
                (207, entry.venue.as_str()),
                (167, record.sec_type.as_str()),
                (264, &request_type_str),
            ];
            let mode_str = record.mode_9887.to_string();
            if record.mode_9887 != 0 && entry.request_type == 1 {
                tags.push((9887, &mode_str));
            }
            let _ = conn.send_fixcomp(&tags);
        }
        // Said, because the venue serves a limited number of these at once and
        // a withdrawal that leaves no trace cannot be told apart from one that
        // never went. The subscribe on the way in is logged; this is the other
        // half of the pair, and without it a session that runs out of room
        // looks like a venue that stopped sending for no reason.
        log::info!("Sent 35=V unsubscribe: instrument={instrument} requests={asked}");
        hb.last_farm_sent = Instant::now();
    }

    /// Ask for a book, on the venue the caller named or on none.
    ///
    /// The venue is passed through as the caller stated it: named, and the book
    /// is that venue's; empty or smart-routed, and the venue aggregates one.
    /// Either way it is one subscription, asked for under an id of this
    /// client's own and mapped back to the caller's.
    pub(crate) fn send_depth_subscribe(
        &mut self,
        req_id: u32,
        con_id: i64,
        exchange: &str,
        primary_exchange: &str,
        sec_type: &str,
        num_rows: i32,
        is_smart_depth: bool,
        farm_conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
        shared: &SharedState,
    ) {
        // A book the venue does not serve where this is routed.
        //
        // The routing answer names, per market and security type, which kinds
        // of data are served. For the smart destination and a stock it names
        // one — the top of the book — and never a deep one, in any region. So
        // a book asked for there is asked of somewhere that has none, and the
        // answer is nothing at all: no refusal, no book, no way to tell which.
        //
        // Said as a refusal instead. Only where the table was read and names
        // the market: a table this client does not have is not evidence that
        // the venue serves nothing.
        // Under the name the venue routes by, not the one a caller was handed.
        // The smart destination is remapped before sending and this
        // client already does it for quotes and for orders; a book asked for
        // under the caller's spelling was the one request still going out
        // under a name the venue does not route by.
        let destination = match exchange {
            "" | "SMART" | "IBKRATS" | "BEST" => "BEST",
            other => other,
        };

        if let Some(conn) = farm_conn.as_ref()
            && !conn.routing.is_empty()
            && !sec_type.is_empty()
        {
            // Whichever name the venue serves a book under here. Asked only
            // about one of them, two markets that serve a book under another
            // read as serving none, and a request the venue would have
            // answered is refused instead.
            let book = conn.routing.book_endpoint(destination, sec_type);
            // Which server the table says serves it. A book asked for on a
            // connection to another farm is acknowledged and then silent,
            // which is indistinguishable from a market with nothing to send.
            if let Some(endpoint) = book
                && let Some(route) = conn.routing.find(destination, sec_type, endpoint)
            {
                log::info!(
                    "book for {sec_type} {con_id} on {destination} is {endpoint}, \
                     served by {} on {} ({num_rows} rows asked)",
                    route.farm, route.host,
                );
            }
            let serves_a_book = book.is_some();
            let named_at_all = conn.routing.find(destination, sec_type, "Top").is_some();
            if named_at_all && !serves_a_book {
                shared.reference.push_historical_error(
                    req_id,
                    crate::error_codes::Refusal::VALIDATION,
                    format!(
                        "the venue serves no book for a {sec_type} on {destination}, \
                         only the top of one — ask on the exchange the contract \
                         trades on instead",
                    ),
                );
                return;
            }
        }

        let fix_sec_type = match sec_type {
            "STK" => "CS", "FUT" => "FUT", "OPT" => "OPT", "IND" => "IND",
            "CASH" => "CASH", other => other,
        };
        // The caller's request, kept so a reconnect can ask for it again. What
        // is registered as a subscription is the id this client asks under,
        // one per venue, below.
        // One record per request, not one per time it was asked for. A caller
        // that asks twice under the same number without withdrawing gets two
        // otherwise, and a reconnect then asks the venue for the book twice.
        self.depth_resub_info.retain(|(id, ..)| *id != req_id);
        self.depth_resub_info.push((
            req_id, con_id, exchange.to_string(), primary_exchange.to_string(),
            sec_type.to_string(), num_rows, is_smart_depth,
        ));

        // A book on no particular venue is asked for once, on no particular
        // venue. The venue aggregates it.
        //
        // Asking each venue the contract is routed to instead — which is what
        // names the venue on every level — costs one subscription per venue,
        // and all but one of them is refused by name. Four contracts cycled
        // that way put seventy-odd subscribes and as many withdrawals on the
        // connection every minute, and the venue stopped answering it
        // altogether: no error, no loss, quotes and books both silent. One
        // request is what the protocol defines and what the connection can
        // carry.
        // A book on one named venue is a book gathered from one venue. Asking
        // under an id of this client's own either way is what keeps the two
        // apart: the venue echoes the id back, and a caller's id sent as it
        // stands is indistinguishable from one of these. A caller's second
        // book was answered under another subscription's venue because both
        // were numbered 2.
        let venues: Vec<String> = vec![destination.to_string()];
        // What the caller asked for is recorded whether or not the socket is
        // up. Recorded only when it was, a book asked for while the farm was
        // down could not be withdrawn, and the reconnect asked for it again.
        self.depth_rows.retain(|(id, _)| *id != req_id);
        if num_rows > 0 {
            self.depth_rows.push((req_id, num_rows));
        }

        // At most one live wire subscription per caller, whatever put a
        // previous one there.
        //
        // The three records below are what a row is routed by, and nothing
        // deduped them on the caller's number. Two ways in: a caller asking
        // twice without withdrawing, which left two contracts' rows arriving
        // interleaved under one number with nothing to tell them apart and a
        // withdrawal that named only the later contract; and a book asked for
        // while this connection was down, which recorded a wire id nothing
        // ever sent -- so when the reconnect asked properly and the venue
        // refused, the refusal saw the phantom still asking and was swallowed,
        // and the caller waited for ever.
        //
        // Cleared here rather than refused, because the reconnect below asks
        // again under the same number by design and must not be refused for
        // it. A caller asking twice is refused at the surface, before this.
        self.depth_subs.retain(|(under, _)| {
            !self.depth_fanout_map.iter().any(|(u, user)| u == under && *user == req_id)
        });
        self.depth_fanout_exchange.retain(|(under, _)| {
            !self.depth_fanout_map.iter().any(|(u, user)| u == under && *user == req_id)
        });
        self.depth_fanout_map.retain(|(_, user)| *user != req_id);

        let mut asked_under = Vec::with_capacity(venues.len());
        for venue in &venues {
            let under = self.next_md_req_id;
            self.next_md_req_id += 1;
            self.depth_subs.push((under, is_smart_depth));
            self.depth_fanout_map.push((under, req_id));
            self.depth_fanout_exchange.push((under, venue.clone()));
            asked_under.push(under);
        }
        log::info!(
            "Book for req={req_id} con_id={con_id} gathered from {}",
            venues.join(", "),
        );

        if let Some(conn) = farm_conn.as_mut() {
            let con_id_str = (con_id as u32).to_string();
            for (under, venue) in asked_under.iter().zip(&venues) {
                self.send_depth_one(
                    conn, &under.to_string(), &con_id_str, venue, fix_sec_type,
                );
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
        // What this client asked under for the caller's book — one venue or
        // several. The caller's own id never went to the venue, so it is not
        // what is withdrawn.
        let asked_under: Vec<u32> = self.depth_fanout_map.iter()
            .filter(|(_, user)| *user == req_id)
            .map(|(sub, _)| *sub)
            .collect();
        // Each entry as it was asked for, gathered before the records go:
        // withdrawn the way it was subscribed, by contract and venue and type
        // as well as number, or the venue leaves it being served.
        let mut entries: Vec<(u32, i64, String, String)> = Vec::new();
        if let Some((_, con_id, _, _, sec_type, ..)) =
            self.depth_resub_info.iter().find(|(id, ..)| *id == req_id)
        {
            let con_id = *con_id;
            let fix_sec_type = crate::control::contracts::sec_type_to_fix(sec_type).to_string();
            for sub in &asked_under {
                let venue = self.depth_fanout_exchange.iter()
                    .find(|(s, _)| s == sub)
                    .map(|(_, venue)| venue.clone())
                    .unwrap_or_default();
                entries.push((*sub, con_id, venue, fix_sec_type.clone()));
            }
        }
        // Cleared whether or not anything was asked yet: left behind, a book
        // the caller withdrew was asked for again by the next reconnect.
        self.depth_resub_info.retain(|(id, ..)| *id != req_id);
        // And the routing, whether or not anything was asked: a tag record
        // left behind after the venue refused the book mid-stream was
        // inherited by the next contract asked for under this number, which
        // then read the old book as its own.
        self.depth_subs.retain(|(id, _)| !asked_under.contains(id));
        self.depth_fanout_map.retain(|(_, user)| *user != req_id);
        self.depth_fanout_exchange.retain(|(sub, _)| !asked_under.contains(sub));
        self.depth_tag_to_req.retain(|(_, rid, ..)| *rid != req_id);
        self.depth_rows.retain(|(id, _)| *id != req_id);
        if asked_under.is_empty() {
            return;
        }

        if let Some(conn) = farm_conn.as_mut() {
            for (sub_req, con_id, venue, fix_sec_type) in &entries {
                let sub_req_str = sub_req.to_string();
                let con_id_str = (*con_id as u32).to_string();
                let _ = conn.send_fixcomp(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
                    (263, "2"),
                    (146, "1"),
                    (262, &sub_req_str),
                    (6008, &con_id_str),
                    (207, venue.as_str()),
                    (167, fix_sec_type.as_str()),
                    (264, DEEP_REQUEST),
                ]);
            }
            hb.last_farm_sent = Instant::now();
            log::info!(
                "Sent depth unsubscribe for req_id={req_id}: {} venue(s)",
                entries.len(),
            );
        }
    }

    /// Ask for one contract's book.
    ///
    /// The same request whether the caller named a venue or left it to the
    /// venue to aggregate one. Asked for as a quote
    /// instead, the venue answers with quote frames, which this client read
    /// with its book reader: a bid of 143.87 and an ask of a penny on a share
    /// trading at 772, a book made of misread quotes, which is worse than no
    /// book. Under the request type that means a book the venue answers —
    /// with the book, or by refusing the entitlement, and either reaches the
    /// caller.
    fn send_depth_one(
        &self, conn: &mut Connection, req_id_str: &str, con_id_str: &str,
        exchange: &str, sec_type: &str,
    ) {
        let _ = conn.send_fixcomp(&[
            (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
            (263, "1"), (146, "1"), (262, req_id_str),
            (6008, con_id_str), (207, exchange), (167, sec_type),
            (264, DEEP_REQUEST), (6088, "Socket"), (9830, "1"),
        ]);
    }

    /// Keep a depth frame exactly as the venue sent it, when asked to.
    ///
    /// A reading checked only against frames this client made up says nothing
    /// about the ones that arrive.
    fn note_depth_wire(&self, kind: &'static str, body: &[u8], shared: &SharedState) {
        if !*crate::engine::hot_loop::CAPTURE_WIRE {
            return;
        }
        let hex: String = body.iter().map(|b| format!("{b:02x}")).collect();
        shared.market.note_unread_wire(kind, hex);
    }

    /// Parse 35=P depth entries (byte-aligned: [00][3B stag][field tags...][58
    /// terminator]).
    /// SmartDepth entries may contain multiple price+size pairs (bid then ask).
    /// Field tag encoding: bit 5(0x20)=size, bit 3(0x08)=ask, bit 2(0x04)=snapshot, bit
    /// 0(0x01)=2-byte.
    fn handle_depth_35p(&self, body: &[u8], shared: &SharedState) {
        self.note_depth_wire("depth-35p", body, shared);
        log::debug!("book frame, {} bytes", body.len());
        use crate::types::DepthUpdate;
        let mut pos = 0;
        let mut bid_position: i32 = 0;
        let mut ask_position: i32 = 0;
        // Which stream the level counters above belong to. One frame can
        // carry sections for more than one stream, and each book's levels are
        // numbered from zero.
        let mut counting_for: Option<u32> = None;

        while pos < body.len() {
            if body[pos] != 0x00 { pos += 1; continue; }
            pos += 1;
            if pos + 3 > body.len() { break; }

            let stag = ((body[pos] as u32) << 16) | ((body[pos+1] as u32) << 8) | (body[pos+2] as u32);
            pos += 3;
            if counting_for != Some(stag) {
                counting_for = Some(stag);
                bid_position = 0;
                ask_position = 0;
            }

            let Some((_, _, min_tick, size_tick, _)) = self.lookup_depth_stag(stag) else {
                // A book frame for a stream this session does not hold. Silent
                // otherwise, and silence here is indistinguishable from a
                // market with nothing to send.
                log::debug!("book frame for an unknown stream, tag {stag}");
                continue;
            };
            // One venue's stream can belong to several requests.
            let subscribers = self.depth_subscribers_of(stag);
            log::debug!("book frame on tag {stag}: {} subscriber(s)", subscribers.len());

            // What the venue counts this contract's sizes in, the same way
            // min_tick is what it counts its prices in. Stating none means
            // whole ones.
            //
            // KNOWN TO DIVERGE, and not changed without a book to check it
            // against. The reference client's multiplier belongs to the
            // CONTRACT and is the same whatever the venue packed the number
            // into: one for anything that is not a share, and for a share the
            // size table's smallest step, or a hundred where the venue stated
            // no rule. What happens below instead is a hundred on the
            // one-byte form and nothing on the two-byte one — a multiplier
            // keyed on the width of the encoding, which the reference client
            // has nothing like.
            //
            // The two agree for exactly one case: a share the venue states no
            // size rule for, packed into one byte. Everything else is out by a
            // hundred one way or the other, and which way cannot be settled
            // from here — the login this was written on is refused a book on
            // every venue it asked (354), so there is nothing to read. A
            // hundredfold error in a number people size trades on is not a
            // thing to fix on reasoning alone.
            let counted_in = if size_tick > 0.0 { size_tick } else { 1.0 };
            // Parse field tags, pushing a depth update on each complete price+size
            // pair.
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

                // A side change with a pending pair flushes it first
                if has_price && has_size && new_side != side {
                    let position = if side == 0 { let p = ask_position; ask_position += 1; p }
                                  else { let p = bid_position; bid_position += 1; p };
                    let operation = if is_snapshot { 0 } else { 1 };
                    for (req_id, is_smart, venue) in &subscribers {
                        if !self.within_asked_depth(*req_id, position) { continue; }
                        shared.market.push_depth_update(DepthUpdate {
                            req_id: *req_id, position, market_maker: venue.clone(),
                            operation, side, price, size, is_smart_depth: *is_smart,
                        });
                    }
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
                    for (req_id, is_smart, venue) in &subscribers {
                        if !self.within_asked_depth(*req_id, position) { continue; }
                        shared.market.push_depth_update(DepthUpdate {
                            req_id: *req_id, position, market_maker: venue.clone(),
                            operation, side, price, size, is_smart_depth: *is_smart,
                        });
                    }
                    has_price = false;
                    has_size = false;
                }
            }

            if pos < body.len() && body[pos] == 0x58 { pos += 1; }
        }
    }

    /// Whether a level is inside the depth its caller asked for.
    ///
    /// The venue sends what it has. A caller that asked for five levels and is
    /// handed every one the venue sends was handed a different book from the
    /// one it asked for, and the reference client it was written against would
    /// have shown five.
    fn within_asked_depth(&self, req_id: u32, position: i32) -> bool {
        match self.depth_rows.iter().find(|(id, _)| *id == req_id) {
            Some((_, rows)) => position < *rows,
            // Nothing asked for in particular, so nothing is too deep.
            None => true,
        }
    }

    /// The subscription a 35=Y frame's opening section belongs to.
    ///
    /// Two bytes, a marker, then the tag in three — the width the venue assigns
    /// it and the width the book's other shape already reads.
    ///
    /// Read from the marker instead and the tag is a byte early: it matches no
    /// subscription, so the section names none and every level it carries waits
    /// for a sentinel further in to name one. Captured frames put the same
    /// three bytes at the same place in every one of them, whichever marker
    /// precedes them.
    fn header_stag(body: &[u8]) -> Option<u32> {
        let tag = body.get(3..6)?;
        Some(((tag[0] as u32) << 16) | ((tag[1] as u32) << 8) | (tag[2] as u32))
    }

    /// Parse 35=Y depth entries (NASDAQ TotalView market-maker level).
    /// Wire format (from wire capture):
    ///   Header: [2B misc][1B marker][3B stag_be]
    ///   Stag switch sentinel: [80|00][00][3B stag_be]
    ///   Snapshot entry: [C4|44][4B market_maker][1B position][field_tags...]
    ///   Compact entry:  [80|00][1B position][field_tags...]
    ///     C4/80 = continuation, 44/00 = terminal (last entry for this stag section).
    /// Field tag encoding: bit 7=size, bit 5=ask, bit 2=snapshot, bits 0-1=value_len
    /// (00=1B,01=2B,10=3B).
    fn handle_depth_35y(&self, msg: &[u8], shared: &SharedState) {
        self.note_depth_wire("depth-35y", msg, shared);
        use crate::types::DepthUpdate;
        let body = match find_body_after_tag(msg, b"35=Y\x01") {
            Some(b) => b,
            None => return,
        };

        // Two bytes of header, then a marker and the tag in three. A frame
        // short of that whole opening names no subscription, and the sentinels
        // below name one for the levels that follow them.
        if body.len() < 4 { return; }

        // Nothing is delivered until a section names the subscription it
        // belongs to. Starting at zero and pushing regardless handed every
        // level of a book to request zero, which no caller ever asked for and
        // no caller could cancel.
        //
        // Each subscriber carries the venue this section of the book is from:
        // the entries below carry no market maker of their own, and a level
        // with no venue on it is a level a caller cannot place.
        let mut subscribers: Vec<(u32, bool, String)> = Vec::new();
        let mut min_tick: f64 = 0.01;
        let mut size_tick: f64 = 1.0;
        let mut pos = 2;

        let hdr_stag = Self::header_stag(body).unwrap_or(u32::MAX);
        if let Some((_, _, mt, st, _)) = self.lookup_depth_stag(hdr_stag) {
            subscribers = self.depth_subscribers_of(hdr_stag);
            min_tick = mt;
            size_tick = st;
            pos = 6;
        }
        log::debug!(
            "book frame, other shape, tag {hdr_stag}, {} subscriber(s)",
            subscribers.len(),
        );
        // If the header named no subscription, scanning starts at 2 and the
        // first sentinel names the ones its levels belong to.

        while pos < body.len() {
            let b = body[pos];

            // Stag switch sentinel: [80|00][00][3B stag_be] — bid_size=0
            // repurposed. Both markers carry the tag in the same three bytes;
            // 0x00 is the one that turns up at a message boundary.
            if (b == 0x80 || b == 0x00) && pos + 5 <= body.len() && body[pos + 1] == 0x00 {
                let candidate = ((body[pos + 2] as u32) << 16)
                    | ((body[pos + 3] as u32) << 8)
                    | (body[pos + 4] as u32);
                if let Some((_, _, mt, st, _)) = self.lookup_depth_stag(candidate) {
                    // Every request this section's levels belong to: the venue
                    // answers a second subscription on a contract and venue it
                    // already streams with the tag it already uses.
                    subscribers = self.depth_subscribers_of(candidate);
                    min_tick = mt;
                    size_tick = st;
                    pos += 5;
                    continue;
                }
                // A switch into a stream this session does not hold. The
                // withdrawal of a book is on its way to the venue while the
                // frames it already sent are still arriving, so a section can
                // name a tag whose requests are gone. Its levels belong to
                // that stream and to nobody here: nothing is delivered until
                // a section names one this session holds. Left to the entry
                // reading below, the switch's own bytes were pushed to the
                // previous section's subscribers as a level of their book.
                subscribers.clear();
                pos += 5;
                continue;
            }

            // Snapshot entry: [C4|44][4B market_maker][1B position][field_tags...]
            if b == 0xC4 || b == 0x44 {
                pos += 1;
                if pos + 5 > body.len() { break; }
                let mm = String::from_utf8_lossy(&body[pos..pos + 4]).trim().to_string();
                pos += 4;
                let book_position = body[pos] as i32;
                pos += 1;

                if let Some((price, size, side, is_snapshot)) =
                    self.parse_depth_fields(body, &mut pos, min_tick, size_tick)
                {
                    for (req_id, is_smart, venue) in &subscribers {
                        if !self.within_asked_depth(*req_id, book_position) { continue; }
                        // The venue's name for the maker where it states
                        // one, and otherwise the exchange this section of the
                        // book is from: a level with neither is a level a
                        // caller cannot place.
                        let named = if mm.is_empty() { venue.clone() } else { mm.clone() };
                        shared.market.push_depth_update(DepthUpdate {
                            req_id: *req_id, position: book_position, market_maker: named,
                            operation: if is_snapshot { 0 } else { 1 },
                            side, price, size, is_smart_depth: *is_smart,
                        });
                    }
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
                // Valid field tags: only bits 7,5,2,1,0 set (mask 0xAF). Reject bits
                // 6,4,3.
                if candidate_pos < 30 && candidate_tag & 0x50 == 0 && candidate_tag & 0x08 == 0 {
                    pos += 1;
                    let book_position = body[pos] as i32;
                    pos += 1;

                    if let Some((price, size, side, is_snapshot)) =
                        self.parse_depth_fields(body, &mut pos, min_tick, size_tick)
                    {
                        for (req_id, is_smart, venue) in &subscribers {
                            if !self.within_asked_depth(*req_id, book_position) { continue; }
                            shared.market.push_depth_update(DepthUpdate {
                                req_id: *req_id, position: book_position,
                                market_maker: venue.clone(),
                                operation: if is_snapshot { 0 } else { 1 },
                                side, price, size, is_smart_depth: *is_smart,
                            });
                        }
                    }
                    continue;
                }
            }

            // Unknown byte — skip
            pos += 1;
        }
    }

    /// Every request this tag's levels belong to, as (req_id, is_smart, venue).
    ///
    /// The venue answers a second subscription on a contract and venue it is
    /// already streaming with the tag it is already using, so a level can
    /// belong to more than one request. Delivering to the first alone left
    /// every later caller with a book that never arrived and no word of why.
    fn depth_subscribers_of(&self, stag: u32) -> Vec<(u32, bool, String)> {
        self.depth_tag_to_req.iter()
            .filter(|(s, ..)| *s == stag)
            .map(|(_, r, sm, _, _, ex)| (*r, *sm, ex.clone()))
            .collect()
    }

    /// Look up a depth server_tag → (req_id, is_smart, min_tick, size_tick, venue).
    fn lookup_depth_stag(&self, stag: u32) -> Option<(u32, bool, f64, f64, String)> {
        self.depth_tag_to_req.iter()
            .find(|(s, ..)| *s == stag)
            .map(|(_, r, sm, mt, st, ex)| (*r, *sm, *mt, *st, ex.clone()))
    }

    /// Parse one price + one size field tag pair. Returns (price, size, side,
    /// is_snapshot).
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

        // A level requires both a price and a size. One alone would report
        // the other as zero, which is not a quoted level.
        if has_price && has_size {
            Some((price, size, side, is_snapshot))
        } else {
            if has_price || has_size {
                log::debug!(
                    "book entry states {} alone; a level is a price and a size, so it is left out",
                    if has_price { "a price" } else { "a size" },
                );
            }
            None
        }
    }

    /// Give this farm up, and the socket it was carried on with it.
    ///
    /// The connection goes here rather than at the reconnect that replaces it.
    /// A liveness timeout says the venue has stopped answering, not that the
    /// socket is closed, so one kept through the outage is a session this
    /// client still holds while it dials another — and on a hard error it is a
    /// descriptor held for as long as the outage lasts. The historical and
    /// calendar farms clear theirs on the same transition.
    pub(crate) fn handle_disconnect(
        &mut self,
        farm_conn: &mut Option<Connection>,
        context: &mut Context,
        event_tx: &Option<EventSink>,
    ) {
        self.disconnected = true;
        *farm_conn = None;
        // Anything the replay had not reached goes back where the next
        // reconnect looks for it. A subscription that was sent is recorded
        // again as it goes out; one still waiting was never sent, so dropping
        // the queue here loses it — and the reconnect that follows rebuilds
        // the queue from the record, which would no longer name it.
        for (instrument, _con_id, symbol, exchange, sec_type, ltd, strike, right, mult, mode)
            in self.replay_queue.drain(..)
        {
            if self.md_resub_info.iter().all(|(id, ..)| *id != instrument) {
                self.md_resub_info.push((
                    instrument, symbol, exchange, sec_type, ltd, strike, right, mult, mode,
                ));
            }
        }
        self.replay_not_before = None;
        self.md_req_to_instrument.clear();
        self.instrument_md_reqs.clear();
        // Clear depth wire-state (server_tags become invalid after disconnect).
        // depth_resub_info is preserved for resubscription on reconnect.
        self.depth_subs.clear();
        self.depth_tag_to_req.clear();
        self.depth_fanout_map.clear();
        // The venue's numbers do not survive the connection that issued them,
        // and one left behind would read the next subscription's frames as the
        // tick the last one asked for.
        self.generic_tick_reqs.clear();
        self.generic_tick_tags.clear();
        // Keyed the same way, and left behind they are never reachable again:
        // what removes an entry looks it up by an id the reconnect has already
        // replaced, so nothing afterwards names the old one. Both are scanned
        // in full on a path that runs per acknowledgement and per withdrawal,
        // so a session that reconnects through a night grows its own latency.
        self.depth_fanout_exchange.clear();
        self.greeks_subs.clear();
        // Server tags are the venue's and start again with the connection, so
        // one already warned about would otherwise silence the warning for a
        // different tick that happened to be given the same number.
        self.quotes_for_no_one.clear();
        context.market.clear_server_tags();
        context.market.zero_all_quotes();
        // Not Event::Disconnected — that is the session going, and a rebuild
        // usually takes this back without one. But the venue says when this
        // connection breaks and a caller stands down on being told: the
        // quotes it can read do not go anywhere when the connection carrying
        // them does, so without this it goes on reading the last price before
        // the drop as though it were still a price.
        emit(event_tx, Event::VenueData {
            which: crate::bridge::VenueDataConnection::MarketData,
            up: false,
        });
    }

    /// Test-only: set disconnected without clearing state or emitting events.
    pub fn handle_disconnect_for_test(&mut self) {
        self.disconnected = true;
    }

    /// The L1 subscriptions to re-issue on a new farm connection, drained from
    /// the record that survives a disconnect. `handle_disconnect` clears
    /// `instrument_md_reqs`, so selecting from that list re-subscribes nothing
    /// and the reconnect silently delivers no market data. Skips
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
        shared: &SharedState,
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
        // reconnect runs.
        let active = self.take_resub_targets(&context.market);
        self.md_req_to_instrument.clear();
        self.instrument_md_reqs.clear();
        // Queued rather than sent here. The first burst goes out on this pass
        // and the rest on the passes that follow, so the pacing costs the
        // venue nothing and the engine nothing.
        self.replay_queue = active.into_iter().collect();
        self.replay_not_before = None;
        self.drive_replay(replay, farm_conn, hb);

        // Re-subscribe depth subscriptions (depth_resub_info survived disconnect)
        let depth_params = std::mem::take(&mut self.depth_resub_info);
        let depth_count = depth_params.len();
        for (req_id, con_id, exchange, listed_on, sec_type, num_rows, is_smart_depth)
            in depth_params
        {
            // A book given up on for running away is refused until it is asked
            // for again, and this is asking for it again: the venue restarts it
            // from the top on the new connection, so what was held against the
            // old one goes with the old one. Without this the reconnect brings
            // the book back on the wire and every update is discarded at the
            // guard, silently and for the life of the session.
            shared.market.purge_depth_updates(req_id);
            // The venue restarts the book from the top on the new connection,
            // and every level of that restart is delivered as a level the
            // caller does not already hold. Told nothing, a caller keyed on
            // position refreshed the levels the new book reaches and kept
            // every level the old one held below them, for the life of the
            // session; a caller that inserts got the new book stacked on top
            // of the old. This says to empty it first, which is the only way a
            // book that shrank can shrink on the caller's side -- nothing on
            // this wire states a level has gone.
            //
            // The queue purge above clears what is queued here, not what the
            // caller holds.
            shared.reference.push_historical_error(
                req_id,
                crate::error_codes::DEPTH_BOOK_RESET,
                "Market depth data has been RESET. Please empty deep book contents \
                 before applying any new entries.".to_string(),
            );
            self.send_depth_subscribe(
                req_id, con_id, &exchange, &listed_on, &sec_type, num_rows, is_smart_depth,
                farm_conn, hb, shared,
            );
        }

        log::info!(
            "Farm reconnected, re-subscribing {} instruments + {} depth",
            self.instrument_md_reqs.len() + self.replay_queue.len(), depth_count,
        );
    }

    /// Put back as much of the reconnect's book as the pacing allows.
    ///
    /// Called on every pass of the loop. Returns immediately when there is
    /// nothing waiting or the pace has not elapsed, so the cost of an idle
    /// engine is one comparison.
    pub(crate) fn drive_replay(
        &mut self,
        replay: crate::engine::hot_loop::ReplayPacing,
        farm_conn: &mut Option<Connection>,
        hb: &mut crate::engine::hot_loop::HeartbeatState,
    ) {
        if self.replay_queue.is_empty() || self.disconnected {
            return;
        }
        let now = Instant::now();
        if let Some(not_before) = self.replay_not_before
            && now < not_before
        {
            return;
        }
        let burst = if replay.burst == 0 { self.replay_queue.len() } else { replay.burst };
        for _ in 0..burst {
            let Some((instrument, con_id, sym, exch, st, ltd, strike, right, mult, mode)) =
                self.replay_queue.pop_front()
            else {
                break;
            };
            self.send_mktdata_subscribe(
                con_id, &sym, &exch, &st, &ltd, strike, &right, &mult, instrument, mode,
                // Nothing replayed here is a snapshot: a one-shot is never
                // written down for replay in the first place.
                false, farm_conn, hb,
            );
        }
        self.replay_not_before =
            (!self.replay_queue.is_empty()).then(|| now + replay.pace);
    }

    fn handle_generic_tick(&mut self, msg: &[u8], context: &mut Context, shared: &SharedState, event_tx: &Option<EventSink>) {
        let body = match find_body_after_tag(msg, b"35=G\x01") {
            Some(b) => b,
            None => return,
        };

        // Which tick a record carries is not on it. The venue numbers a
        // generic tick apart from the prices and says what it carries once,
        // when it takes the subscription on, so what was asked for under that
        // number is the only thing that says what these bytes are — and, since
        // a tick's payload states its length in a way particular to that tick,
        // the only thing that says where the record ends.
        let asked = &self.generic_tick_tags;
        let mut delivered: Vec<(u32, u32, &[u8])> = Vec::new();
        read_generic_ticks(
            body,
            |server_tag| asked.iter().find(|(tag, ..)| *tag == server_tag).map(|(_, tick, _)| *tick),
            |tick, record| delivered.push((tick, record.server_tag, record.payload)),
        );

        for (tick, server_tag, payload) in delivered {
            let Some(instrument) = asked
                .iter()
                .find(|(tag, ..)| *tag == server_tag)
                .map(|(_, _, instrument)| *instrument)
            else {
                continue;
            };
            match tick {
                GREEKS_REQUEST_TYPE => {
                    if let Some(mut comp) = decode_greeks(payload) {
                        comp.instrument = instrument;
                        shared.market.push_option_computation(comp);
                        emit(event_tx, Event::OptionComputation(comp));
                    }
                }
                BBO_EXCHANGE_MAP_REQUEST_TYPE => {
                    // Every venue, in the order the mask's bits refer to:
                    // `NAME/LETTER` per venue, one after another.
                    let Some(stated) = length_prefixed_text(payload) else {
                        // An empty payload states no length at all, which is
                        // the venue naming no venues rather than a message
                        // that lost its tail — reported as the same thing, it
                        // sends the next reader looking for a truncation.
                        if payload.is_empty() {
                            log::debug!("the exchange map named no venues");
                        } else {
                            log::warn!(
                                "the exchange map states a length its {} bytes do not carry",
                                payload.len(),
                            );
                        }
                        continue;
                    };
                    let venues: Vec<crate::types::SmartComponent> = stated
                        .split(';')
                        .filter(|entry| !entry.trim().is_empty())
                        .enumerate()
                        .map(|(bit, entry)| {
                            let (exchange, letter) =
                                entry.split_once('/').unwrap_or((entry, ""));
                            crate::types::SmartComponent {
                                bit_number: bit as i32,
                                exchange: exchange.trim().to_string(),
                                exchange_letter: letter.trim().to_string(),
                            }
                        })
                        .collect();
                    if !venues.is_empty() {
                        log::info!("the server names {} venues for the exchange mask", venues.len());
                        shared.reference.set_smart_components(venues);
                        shared.reference.note_smart_components_provisional(false);
                    }
                }
                TRADING_STATUS_REQUEST_TYPE => {
                    let Some(status) =
                        crate::protocol::trading_status::parse_trading_status(payload)
                    else {
                        continue;
                    };
                    // Published with the quote it belongs to, so a caller
                    // reads the halt against the prices it applies to rather
                    // than a moment either side of them.
                    context.quote_mut(instrument).halted = i64::from(status.is_halted());
                    shared.market.push_quote(instrument, context.quote(instrument));
                    emit(event_tx, Event::Tick(instrument));
                }
                NEWS_REQUEST_TYPE => self.deliver_news(instrument, payload, shared, event_tx),
                other => log::debug!("Generic tick {other} arrives and nothing here reads it"),
            }
        }
    }

    /// The articles in one news tick.
    fn deliver_news(
        &self,
        instrument: InstrumentId,
        body: &[u8],
        shared: &SharedState,
        event_tx: &Option<EventSink>,
    ) {
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
            // Seconds on the wire, and the reference client hands a caller
            // milliseconds: it builds a date out of this and passes what that
            // date reads as. Handed on as it arrived, every stamp a caller
            // read was a thousandth of the moment it meant.
            let stated_secs =
                u32::from_be_bytes([body[pos], body[pos+1], body[pos+2], body[pos+3]]) as u64;
            let timestamp = stated_secs.saturating_mul(1_000);
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
            // With the characters the venue escaped put back, the same as on a
            // headline out of the archive: it writes anything outside plain
            // ASCII as `&#xNN;`, and the reference client reads those back
            // here too.
            let headline = crate::control::news::unescape_venue_characters(&headline);

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
mod tests;
