//! Shared dispatch core for Rust and Python EClient implementations.
//!
//! `ClientCore` owns all subscription tracking state (reqId maps, change-detection
//! snapshots, PnL/account subscriptions) and exposes "prepare" methods that return
//! intermediate structs. Language-specific EClient adapters convert these into their
//! respective callback formats (Rust `Wrapper` trait calls or PyO3 `call_method`).

// The order-status vocabulary moved to the types it describes. Public here
// because that is the path a program written against this client already
// names, and used here for the same reason it was written.
pub use crate::types::order_status::{is_open_or_reactivatable, is_open_status, order_status_str};
use std::collections::{HashMap, HashSet};
use crate::error_codes::Refusal;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Mutex;
use std::sync::LazyLock;

use std::sync::mpsc::SyncSender;

use crate::types::model::{
    Contract as ApiContract, CommissionAndFeesReport as ApiCommissionAndFeesReport,
    Execution as ApiExecution, ExecutionFilter,
    Order as ApiOrder, TagValue,
    PRICE_SCALE_F,
};
use crate::bridge::SharedState;
use crate::types::*;

/// The only market data type the engine delivers (1 = realtime).
const MDT_REALTIME: i32 = 1;
const MDT_FROZEN: i32 = 2;
const MDT_DELAYED: i32 = 3;
const MDT_DELAYED_FROZEN: i32 = 4;

// ── Tick type constants matching ibapi ──

/// Tick type 1: the bid.
pub const TICK_BID: i32 = 1;
/// Tick type 2: the ask.
pub const TICK_ASK: i32 = 2;
/// Tick type 4: the last.
pub const TICK_LAST: i32 = 4;
/// Tick type 6: the high.
pub const TICK_HIGH: i32 = 6;
/// Tick type 7: the low.
pub const TICK_LOW: i32 = 7;
/// Tick type 9: the close.
pub const TICK_CLOSE: i32 = 9;
/// Tick type 14: the open.
pub const TICK_OPEN: i32 = 14;
/// Tick type 0: the bid size.
pub const TICK_BID_SIZE: i32 = 0;
/// Tick type 3: the ask size.
pub const TICK_ASK_SIZE: i32 = 3;
/// Tick type 5: the last size.
pub const TICK_LAST_SIZE: i32 = 5;
/// Tick type 8: the volume.
pub const TICK_VOLUME: i32 = 8;
/// Tick type 45: the last timestamp.
pub const TICK_LAST_TIMESTAMP: i32 = 45;
/// Whether the venue has halted trading. Stated by the venue and delivered as
/// a generic tick, which is where the reference client puts it.
pub const TICK_HALTED: i32 = 49;
/// Tick type 32: the bid exchange.
pub const TICK_BID_EXCHANGE: i32 = 32;
/// Tick type 33: the ask exchange.
pub const TICK_ASK_EXCHANGE: i32 = 33;
/// Tick type 84: the last exchange.
pub const TICK_LAST_EXCHANGE: i32 = 84;

/// Whether one contract of this holding is worth more than one unit of the
/// price the venue quotes it at.
///
/// A quote is per unit, so an option or a future priced from one alone is
/// valued at a fraction of what it is worth — a hundredth, for the commonest
/// option multiplier. The venue states its own figure for such a position, and
/// that is what is reported rather than a price this arithmetic cannot use.
fn position_is_multiplied(pi: &PositionInfo) -> bool {
    match pi.multiplier.trim() {
        "" => false,
        stated => stated.parse::<f64>().is_ok_and(|m| m != 1.0),
    }
}

/// Render an exchange-code bitmask to a letter string using the smart components
/// table. Each set bit at position N picks `smart_components[N].exchange_letter`.
///
/// Bit ordering and width follow the TWS-API convention; the dispatch path
/// tolerates an empty result where the mask layout differs.
pub fn render_exchange_mask(mask: i64, shared: &SharedState) -> String {
    if mask == 0 {
        return String::new();
    }
    let components = shared.reference.smart_components();
    let mut out = String::with_capacity(8);
    let mut bits = mask as u64;
    while bits != 0 {
        let bit = bits.trailing_zeros() as i32;
        bits &= bits - 1;
        if let Some(c) = components.iter().find(|c| c.bit_number == bit) {
            out.push_str(&c.exchange_letter);
        }
    }
    out
}

// ── Intermediate dispatch structs ──

/// A single tick event produced by quote change detection.
pub struct TickEvent {
    /// The request this answers.
    pub req_id: i64,
    /// Which tick this is.
    pub tick_type: i32,
    /// What it is.
    pub value: f64,
    /// true = tick_price, false = tick_size
    pub is_price: bool,
}

/// Timestamp tick from quote polling.
pub struct TimestampTick {
    /// The request this answers.
    pub req_id: i64,
    /// When, in nanoseconds since the epoch.
    pub timestamp_ns: i64,
}

/// String-valued tick (e.g. exchange-code letters for tick_types 32/33/84).
pub struct StringTickEvent {
    /// The request this answers.
    pub req_id: i64,
    /// Which tick this is.
    pub tick_type: i32,
    /// What it is.
    pub value: String,
}

/// Result of polling quotes for one instrument.
pub struct QuotePollResult {
    /// Numeric ticks that arrived.
    pub ticks: Vec<TickEvent>,
    /// Ticks the venue states under a number of its own rather than as a price
    /// or a size, delivered on `tick_generic`.
    pub generic_ticks: Vec<TickEvent>,
    /// Ticks whose value is text.
    pub string_ticks: Vec<StringTickEvent>,
    /// The moment the venue stamped the quote with, if it stated one.
    pub timestamp: Option<TimestampTick>,
    /// true if any tick was delivered (for snapshot detection).
    pub delivered: bool,
}

/// PnL update (account-level).
pub struct PnlUpdate {
    /// The request this answers.
    pub req_id: i64,
    /// What the account has made today.
    pub daily_pnl: f64,
    /// What its positions have made and not realised.
    pub unrealized_pnl: f64,
    /// What it has realised.
    pub realized_pnl: f64,
}

/// PnL single update (per-position).
pub struct PnlSingleUpdate {
    /// The request this answers.
    pub req_id: i64,
    /// How much is held.
    pub pos: f64,
    /// What the account has made today.
    pub daily_pnl: f64,
    /// What its positions have made and not realised.
    pub unrealized_pnl: f64,
    /// What it has realised.
    pub realized_pnl: f64,
    /// What it is.
    pub value: f64,
}

/// A single changed account field.
pub struct AccountFieldUpdate {
    /// Which figure.
    pub key: String,
    /// What it is.
    pub value: String,
    /// What currency it is stated in.
    pub currency: String,
}

/// Batch of account update results.
pub struct AccountUpdateBatch {
    /// Each figure that changed.
    pub fields: Vec<AccountFieldUpdate>,
    /// Whether any field was delivered.
    pub delivered: bool,
    /// Whether the account is now taken to have been fully stated, said once.
    ///
    /// Read from the field stream going quiet, not from a signal: the venue
    /// marks the end of its account request on its own message, and the same
    /// mark is set by the first rows of the burst as well, so it cannot tell
    /// the end of the description from its start. What this waits for instead
    /// is a stretch of silence after the last field — not the first field of
    /// any kind, since the figures that matter arrive seconds after it.
    ///
    /// A download that stalls for longer than that stretch and then resumes is
    /// therefore called finished early, and a caller released on it reads the
    /// account as it stood mid-burst.
    pub finished: bool,
}

/// Prepared account summary response.
pub struct AccountSummaryBatch {
    /// The request this answers.
    pub req_id: i64,
    /// Each figure answering the request.
    pub entries: Vec<AccountSummaryEntry>,
}

/// One figure answering a summary request.
pub struct AccountSummaryEntry {
    /// Which figure this is, under the venue's name for it. Owned for the
    /// same reason the currency is: the set is the venue's, not a fixed list
    /// known here, and a summary built from such a list reported nothing for
    /// every figure that was not on it.
    pub tag: String,
    /// What it is.
    pub value: String,
    /// As the venue stated it for this figure. Owned rather than borrowed
    /// because it is the venue's word, not one of a fixed set known here.
    pub currency: String,
}

/// A single portfolio position update.
pub struct PortfolioUpdateEntry {
    /// The contract.
    pub con_id: i64,
    /// How much is held.
    pub position: f64,
    /// What it cost on average.
    pub avg_cost: f64,
    /// What it is worth now, each.
    pub market_price: f64,
    /// What the holding is worth.
    pub market_value: f64,
    /// What its positions have made and not realised.
    pub unrealized_pnl: f64,
    /// What it has realised.
    pub realized_pnl: f64,
}

/// Parse a `riskAversion` tag value (used by ArrivalPx and ClosePx). A
/// missing tag defaults to Neutral, matching IB's own algo default; a
/// present value — including an empty string — that isn't a recognized
/// member is refused rather than silently defaulting to Neutral.
fn parse_risk_aversion(raw: Option<&str>) -> Result<RiskAversion, Refusal> {
    let raw = match raw {
        None => return Ok(RiskAversion::Neutral),
        Some(raw) => raw,
    };
    match raw.to_lowercase().as_str() {
        "neutral" => Ok(RiskAversion::Neutral),
        "get_done" | "getdone" => Ok(RiskAversion::GetDone),
        "aggressive" => Ok(RiskAversion::Aggressive),
        "passive" => Ok(RiskAversion::Passive),
        _ => Err(Refusal::validation(
            "Unknown riskAversion '{raw}': expected Get_Done, Aggressive, Neutral or Passive"
                .replace("{raw}", raw),
        )),
    }
}

/// The parameters a strategy this client models reads off the caller's list.
///
/// A strategy that is not here is handed to the venue with the caller's list
/// as written, so it has no set to state.
fn algo_param_names(strategy: &str) -> Option<&'static [&'static str]> {
    Some(match strategy {
        "vwap" => &["maxPctVol", "noTakeLiq", "allowPastEndTime", "startTime", "endTime"],
        "twap" => &["allowPastEndTime", "startTime", "endTime"],
        "arrivalpx" | "arrival_price" => &[
            "maxPctVol", "riskAversion", "allowPastEndTime", "forceCompletion",
            "startTime", "endTime",
        ],
        "closepx" | "close_price" => {
            &["maxPctVol", "riskAversion", "forceCompletion", "startTime"]
        }
        "darkice" | "dark_ice" => {
            &["allowPastEndTime", "displaySize", "startTime", "endTime"]
        }
        "pctvol" | "pct_vol" => &["pctVol", "noTakeLiq", "startTime", "endTime"],
        _ => return None,
    })
}

/// Parse algo strategy and TagValue params into internal AlgoParams.
///
/// A key the caller never set defaults the way IB's own algos do (0.0,
/// false, or the documented default enum value). A key the caller *did*
/// set — even to an empty string — is refused if it does not parse, rather
/// than taking that same default: `riskAversion="Aggresive"` would otherwise
/// submit a Neutral algo with no error, and `maxPctVol=""` would submit 0.0.
///
/// A strategy modelled here is re-encoded from the fields it names rather than
/// forwarded as the caller wrote it, so a key it does not name would go no
/// further. Said rather than dropped: the caller had set it for a reason and
/// the order that reached the venue would not have carried it.
pub fn parse_algo_params(strategy: &str, params: &[TagValue]) -> Result<AlgoParams, Refusal> {
    let folded = strategy.to_lowercase();
    if let Some(known) = algo_param_names(&folded)
        && let Some(stray) = params.iter().find(|tv| !known.contains(&tv.tag.as_str()))
    {
        return Err(Refusal::validation(format!(
            "algo parameter '{}' is not one {strategy} carries here, so it would \
             not reach the venue. This strategy reads {}.",
            stray.tag,
            known.join(", "),
        )));
    }
    let get = |key: &str| -> Option<String> {
        params.iter().find(|tv| tv.tag == key).map(|tv| tv.value.clone())
    };
    let get_str = |key: &str| -> String { get(key).unwrap_or_default() };
    let get_f64 = |key: &str| -> Result<f64, Refusal> {
        let raw = match get(key) {
            None => return Ok(0.0),
            Some(raw) => raw,
        };
        let v: f64 = raw.parse()
            .map_err(|_| Refusal::validation(format!("Invalid {key} '{raw}': expected a number")))?;
        if !v.is_finite() {
            return Err(Refusal::validation(
                format!("Invalid {key} '{raw}': must be a finite number"),
            ));
        }
        Ok(v)
    };
    let get_bool = |key: &str| -> Result<bool, Refusal> {
        let raw = match get(key) {
            None => return Ok(false),
            Some(raw) => raw,
        };
        match raw.to_lowercase().as_str() {
            "0" | "false" => Ok(false),
            "1" | "true" => Ok(true),
            _ => Err(Refusal::validation(
                format!("Invalid {key} '{raw}': expected true/false or 1/0"),
            )),
        }
    };

    match folded.as_str() {
        "vwap" => Ok(AlgoParams::Vwap {
            max_pct_vol: get_f64("maxPctVol")?,
            no_take_liq: get_bool("noTakeLiq")?,
            allow_past_end_time: get_bool("allowPastEndTime")?,
            start_time: get_str("startTime"),
            end_time: get_str("endTime"),
        }),
        "twap" => Ok(AlgoParams::Twap {
            allow_past_end_time: get_bool("allowPastEndTime")?,
            start_time: get_str("startTime"),
            end_time: get_str("endTime"),
        }),
        "arrivalpx" | "arrival_price" => Ok(AlgoParams::ArrivalPx {
            max_pct_vol: get_f64("maxPctVol")?,
            risk_aversion: parse_risk_aversion(get("riskAversion").as_deref())?,
            allow_past_end_time: get_bool("allowPastEndTime")?,
            force_completion: get_bool("forceCompletion")?,
            start_time: get_str("startTime"),
            end_time: get_str("endTime"),
        }),
        "closepx" | "close_price" => Ok(AlgoParams::ClosePx {
            max_pct_vol: get_f64("maxPctVol")?,
            risk_aversion: parse_risk_aversion(get("riskAversion").as_deref())?,
            force_completion: get_bool("forceCompletion")?,
            start_time: get_str("startTime"),
        }),
        "darkice" | "dark_ice" => {
            // Stated, not chosen here. A display size is how much of the
            // order the book shows; a default would show a size the caller
            // never asked to show.
            let display_size = match get("displaySize") {
                None => return Err(Refusal::validation(
                    "DarkIce needs a displaySize: it is how much of the order the \
                     book shows, and this client will not choose it for you",
                )),
                Some(raw) => raw.parse().map_err(|_| format!("Invalid displaySize '{raw}': expected a non-negative integer"))?,
            };
            Ok(AlgoParams::DarkIce {
                allow_past_end_time: get_bool("allowPastEndTime")?,
                display_size,
                start_time: get_str("startTime"),
                end_time: get_str("endTime"),
            })
        }
        "pctvol" | "pct_vol" => Ok(AlgoParams::PctVol {
            pct_vol: get_f64("pctVol")?,
            no_take_liq: get_bool("noTakeLiq")?,
            start_time: get_str("startTime"),
            end_time: get_str("endTime"),
        }),
        // Anything else goes as the caller wrote it.
        //
        // Refused here instead, a caller could use only the algorithms this
        // match happens to name — five of the thirteen an ordinary session is
        // offered. Which ones an account may use is the venue's answer, stated
        // at logon and enforced by it, and the reference client does not
        // interpret these either.
        // The caller's own spelling, not the one folded for matching: the
        // venue is handed this name and does not know a lower-cased one.
        _ => Ok(AlgoParams::Named {
            strategy: strategy.to_string(),
            params: params
                .iter()
                .flat_map(|tv| [tv.tag.clone(), tv.value.clone()])
                .collect(),
        }),
    }
}

// ── Order field validation ──

/// Reject a price/amount field that a saturating float-to-int cast would
/// otherwise turn into a different, valid-looking number: NaN becomes 0,
/// +/-Infinity becomes i64::MAX/MIN, and a finite value whose fixed-point
/// form overflows i64 saturates the same way.
pub(crate) fn require_finite_price(field: &str, v: f64) -> Result<(), String> {
    // `i64::MAX as f64` itself rounds up to 2^63 (f64 cannot represent
    // i64::MAX exactly), so a strict `>` lets a scaled value of exactly
    // 2^63 through and the subsequent `as i64` cast saturates to i64::MAX
    // instead of being refused. `>=` excludes that boundary.
    if !v.is_finite() || (v * PRICE_SCALE_F).abs() >= i64::MAX as f64 {
        return Err(format!(
            "{field} must be a finite number representable on the wire, got {v}"
        ));
    }
    Ok(())
}

/// Parse the Adaptive algo's `adaptivePriority` tag. A missing tag defaults
/// to Normal (IB's own default); a present-but-unrecognized value is
/// refused instead of silently defaulting to Normal.
fn adaptive_priority(params: &[TagValue]) -> Result<AdaptivePriority, String> {
    match params.iter().find(|tv| tv.tag == "adaptivePriority") {
        None => Ok(AdaptivePriority::Normal),
        Some(tv) => match tv.value.as_str() {
            "Patient" => Ok(AdaptivePriority::Patient),
            "Normal" => Ok(AdaptivePriority::Normal),
            "Urgent" => Ok(AdaptivePriority::Urgent),
            other => Err(format!(
                "Unknown adaptivePriority '{other}': expected Patient, Normal or Urgent"
            )),
        },
    }
}

// ── Execution storage ──

/// Does a stored execution satisfy an `ExecutionFilter`? Shared by the index
/// form and the snapshot form so the two cannot disagree about what matches.
fn execution_matches(se: &StoredExecution, filter: &ExecutionFilter) -> bool {
    if !filter.symbol.is_empty() && !se.contract.symbol.eq_ignore_ascii_case(&filter.symbol) {
        return false;
    }
    if !filter.sec_type.is_empty() && !se.contract.sec_type.eq_ignore_ascii_case(&filter.sec_type) {
        return false;
    }
    if !filter.exchange.is_empty() && !se.execution.exchange.eq_ignore_ascii_case(&filter.exchange) {
        return false;
    }
    if !filter.side.is_empty() && !se.execution.side.eq_ignore_ascii_case(&filter.side) {
        return false;
    }
    if !filter.acct_code.is_empty() && !se.execution.acct_number.eq_ignore_ascii_case(&filter.acct_code) {
        return false;
    }
    if filter.client_id != 0 && se.execution.client_id != filter.client_id {
        return false;
    }
    // ibapi treats `time` as a lower bound — executions at or after it. The
    // two sides can be punctuated differently ("20260729-10:00:00" against
    // "20260729 10:00:00"), so compare on digits alone; both are yyyyMMdd
    // first, so that ordering is chronological. A bound carrying less
    // precision than the timestamp compares against the same prefix, so a
    // date-only filter keeps that whole day rather than dropping it.
    if !filter.time.is_empty() {
        let digits = |s: &str| s.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
        let lo = digits(&filter.time);
        let at = digits(&se.execution.time);
        let n = lo.len().min(at.len());
        if at.get(..n).unwrap_or("") < lo.get(..n).unwrap_or("") {
            return false;
        }
    }
    true
}

/// A stored execution + commission_and_fees pair for `req_executions` replay.
/// Shared between Rust and Python adapters via `ClientCore`.
#[derive(Clone)]
pub struct StoredExecution {
    /// The request this answers.
    pub req_id: i64,
    /// The contract it is on.
    pub contract: ApiContract,
    /// The fill itself.
    pub execution: ApiExecution,
    /// What the fill cost.
    pub commission_and_fees: ApiCommissionAndFeesReport,
}

// ── Order tracking ──

/// A locally tracked order for `req_open_orders` / dispatch status updates.
#[derive(Clone)]
pub struct TrackedOrder {
    /// The contract it is on.
    pub contract: ApiContract,
    /// The order as this client sent it.
    pub order: ApiOrder,
    /// Where it stands.
    pub status: String,
    /// How much has filled.
    pub filled: f64,
    /// How much has not.
    pub remaining: f64,
    /// The engine's own slot for the contract.
    pub instrument: InstrumentId,
    /// True once this order's last transition was a genuine Rejected (FIX
    /// 39=8). Rejected and Inactive both stringify to `status == "Inactive"`
    /// (ibapi has no Rejected string), so that string alone cannot tell a
    /// dead order from a parked, reactivatable one — `collect_open_orders`
    /// uses this flag as the discriminator instead of widening
    /// `is_open_status`.
    pub rejected: bool,
}

// ── ClientCore ──

/// An answer owed to a caller about a display group.
#[derive(Debug, Clone)]
pub enum GroupEvent {
    /// The groups on offer, `|`-separated.
    List(i64, String),
    /// What a group now holds.
    Updated(i64, String),
}

/// What both client surfaces share: which request is on which
/// contract, what the venue last said, and what is still subscribed.
pub struct ClientCore {
    /// How long a caller waits for the engine to name an instrument.
    ///
    /// Stated by the session. It was read from the process once and cached for
    /// the life of it, so the first session to ask fixed the wait for every
    /// session after it.
    pub registration_timeout: std::sync::Mutex<std::time::Duration>,
    /// Whether this session refuses to send anything that changes a position.
    ///
    /// The reference API carries the same control. A research or reporting
    /// program gets the guarantee at the client rather than by discipline.
    /// Set once when the session opens.
    pub readonly: std::sync::atomic::AtomicBool,
    // reqId <-> InstrumentId mapping
    /// Which contract each quote request is on.
    pub req_to_instrument: Mutex<HashMap<i64, InstrumentId>>,
    /// Which request owns each contract's quotes. One per contract:
    /// later callers follow it rather than opening a second.
    pub instrument_to_req: Mutex<HashMap<InstrumentId, i64>>,
    /// Which contract each tick-by-tick request is on.
    ///
    /// Its own, because a trade stream is not a quote subscription. Kept in
    /// the quote maps, a request for trades was handed the contract's quotes,
    /// and withdrawing it took away the quotes another caller was watching.
    pub tbt_to_instrument: Mutex<HashMap<i64, InstrumentId>>,
    /// The other requests watching a contract that is already subscribed.
    ///
    /// One contract holds one subscription on the wire, and the same quote is
    /// handed to every caller that asked for it. Two parts of one program may
    /// watch the same contract.
    pub instrument_followers: Mutex<HashMap<InstrumentId, Vec<i64>>>,
    // con_id → InstrumentId for find_or_register_instrument lookup
    /// The engine slot each contract id was given.
    pub con_id_to_instrument: Mutex<HashMap<i64, InstrumentId>>,
    /// What each display group currently holds. The venue knows nothing of
    /// these: they are a way for several callers on one session to agree on a
    /// contract, and the vendor's own client keeps them the same way, serving
    /// them to its callers out of its own state.
    display_groups: Mutex<HashMap<i32, String>>,
    /// Which group each subscribing request follows.
    group_subscriptions: Mutex<HashMap<i64, i32>>,
    /// Group answers waiting to be delivered on the next dispatch, so a caller
    /// hears them where it hears everything else.
    pending_group_events: Mutex<Vec<GroupEvent>>,
    // Change detection for quote polling
    /// The most recent quote per contract, so a caller asking twice is
    /// answered the same way twice.
    pub last_quotes: Mutex<HashMap<InstrumentId, [i64; 16]>>,
    /// Requests that asked for a snapshot rather than a stream, and when each
    /// last heard something.
    ///
    /// A snapshot ends when the venue has finished sending it. There is no
    /// marker for that on this protocol, so it ends when the ticks stop:
    /// ending at the first one cancelled the subscription on whatever arrived
    /// first — often the previous close — and the bid and ask that the caller
    /// asked for never came.
    /// Snapshots being waited on: when each was asked for, and which of the
    /// kinds one is made of the venue has stated so far.
    pub snapshot_reqs: Mutex<HashMap<i64, (std::time::Instant, u8)>>,

    // PnL subscription state
    /// The request a running profit is reported under.
    pub pnl_req_id: Mutex<Option<i64>>,
    /// Which contract each single-position profit request is on.
    pub pnl_single_reqs: Mutex<HashMap<i64, i64>>, // req_id → con_id
    /// The last running profit stated: daily, unrealised, realised.
    pub last_pnl: Mutex<[i64; 3]>, // [daily, unrealized, realized]
    // Per-req_id change detection for pnl_single: [pos, daily, unrealized, realized,
    // value] scaled.
    /// The same per position.
    pub last_pnl_single: Mutex<HashMap<i64, [i64; 5]>>,

    // Account summary subscription state (req_id, tags)
    /// The summary request waiting to be answered, and the tags it
    /// asked for.
    pub account_summary_req: Mutex<Option<(i64, Vec<String>)>>,

    // News bulletin subscription
    /// Whether broadcast notices were asked for.
    pub bulletin_subscribed: AtomicBool,

    // Account updates subscription
    /// Whether the account's own figures were asked for.
    pub account_updates_subscribed: AtomicBool,
    /// The account as last stated.
    pub last_account: Mutex<Option<AccountState>>,
    /// What has already been delivered of what the venue stated, by figure and
    /// currency, so each is delivered once and again when it changes.
    pub last_stated_account: Mutex<HashMap<(String, String), String>>,
    /// Whether the caller has been told the account is fully stated.
    pub account_end_sent: AtomicBool,
    /// Its positions as last stated.
    pub last_portfolio: Mutex<Option<Vec<PositionInfo>>>,

    // Execution replay store
    /// Fills held for a caller who asks for them again.
    pub executions: Mutex<Vec<StoredExecution>>,

    // Open order tracking
    /// Every order this client placed and the venue has not finished.
    pub open_orders: Mutex<HashMap<u64, TrackedOrder>>,

    // Market data type callback tracking
    /// Which feed subscriptions default to.
    pub market_data_type: AtomicI32,
    /// Which requests have already been told which feed they are on.
    pub mdt_sent: Mutex<HashSet<i64>>,
    /// The market-data type each subscription was made under. A request that
    /// names its own mode is not described by the type set for everything
    /// else, and that request's callback has to say what it asked for.
    mdt_by_req: Mutex<HashMap<i64, i32>>,
    /// Which requests asked for their bar times as seconds since the epoch.
    ///
    /// The venue states a time in one form and the client formats it for the
    /// caller. A caller handed the other form is reading a date as a number or
    /// a number as a date. Only the request that asked is affected, so this is
    /// kept per request rather than for the session.
    epoch_dates_by_req: Mutex<HashSet<i64>>,

    // Historical data keepUpToDate: req_ids that have completed initial batch.
    // Subsequent bars for these req_ids dispatch as historical_data_update.
    // Cleared when a request is made under the id again
    // `historical_request_is_new`.
    /// Which historical requests have finished their first batch, so
    /// a later bar under the same id is a continuation rather than a new answer.
    pub hist_initial_complete: Mutex<HashSet<u32>>,

    // News subscription state
    /// Every provider this account may read.
    pub news_providers: Mutex<String>,
    /// Which contracts news was asked for on.
    /// Which requests asked for the headlines on a contract.
    ///
    /// Held by request, because the headlines stop when the last caller that
    /// asked for them goes — not when the first one does, and not when the
    /// quotes happen to end. Keyed by the contract rather than by the
    /// instrument, because the venue is asked by contract and the decision to
    /// ask is made before the instrument is known: keyed by instrument, two
    /// requests racing for a contract neither had registered yet both found
    /// nobody had asked, and both asked.
    pub news_askers: Mutex<HashMap<i64, HashSet<i64>>>,

    // Contract cache for enrichment
    /// What the venue has said about each contract, kept so a second
    /// request need not ask again.
    pub contract_cache: Mutex<HashMap<i64, ApiContract>>,
    /// Contracts the venue has named, under the description that was asked
    /// about rather than the one it answered with.
    ///
    /// An order may name its contract by description, and the venue only takes
    /// orders that name it by id, so the description has to be looked up. Asked
    /// again for every order, a program that places a hundred on one contract
    /// sends a hundred lookups for a name that has not changed since the first.
    named_by_description: Mutex<HashMap<String, ApiContract>>,
}

impl Default for ClientCore {
    fn default() -> Self {
        Self::new()
    }
}

/// Why neither question can be answered without the venue having spoken.
pub(crate) const OPTION_MODEL_UNSTATED: &str =
    "the venue has not stated its own model for this contract on this session. Ask for the \
     option's model first — a market-data subscription on the option carries it — and both \
     questions can then be answered against what it said";

/// Years between now and a stated expiry, as `yyyymmdd`.
pub(crate) fn years_to_expiry(expiry: &str) -> Option<f64> {
    let digits: String = expiry.chars().filter(|c| c.is_ascii_digit()).take(8).collect();
    if digits.len() != 8 {
        return None;
    }
    let year: i64 = digits[0..4].parse().ok()?;
    let month: i64 = digits[4..6].parse().ok()?;
    let day: i64 = digits[6..8].parse().ok()?;
    let expiry_day = days_from_civil(year, month, day);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let today = now / 86_400;
    let days = expiry_day - today;
    (days > 0).then(|| days as f64 / 365.0)
}

/// Days since the epoch for a civil date. Written out rather than pulled in:
/// one date, once, and a dependency for it would be a dependency for good.
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

impl ClientCore {
    /// An empty one.
    pub fn new() -> Self {
        Self {
            // What a session that never states one waits. The library's own
            // tests state a millisecond; see `set_registration_timeout`.
            registration_timeout: Mutex::new(if cfg!(test) {
                std::time::Duration::from_millis(1)
            } else {
                std::time::Duration::from_secs(5)
            }),
            readonly: std::sync::atomic::AtomicBool::new(false),
            req_to_instrument: Mutex::new(HashMap::new()),
            instrument_to_req: Mutex::new(HashMap::new()),
            tbt_to_instrument: Mutex::new(HashMap::new()),
            instrument_followers: Mutex::new(HashMap::new()),
            con_id_to_instrument: Mutex::new(HashMap::new()),
            display_groups: Mutex::new(HashMap::new()),
            group_subscriptions: Mutex::new(HashMap::new()),
            pending_group_events: Mutex::new(Vec::new()),
            last_quotes: Mutex::new(HashMap::new()),
            snapshot_reqs: Mutex::new(HashMap::new()),
            pnl_req_id: Mutex::new(None),
            pnl_single_reqs: Mutex::new(HashMap::new()),
            last_pnl: Mutex::new([0; 3]),
            last_pnl_single: Mutex::new(HashMap::new()),
            account_summary_req: Mutex::new(None),
            bulletin_subscribed: AtomicBool::new(false),
            account_updates_subscribed: AtomicBool::new(false),
            last_account: Mutex::new(None),
            last_stated_account: Mutex::new(HashMap::new()),
            account_end_sent: AtomicBool::new(false),
            last_portfolio: Mutex::new(None),
            executions: Mutex::new(Vec::new()),
            open_orders: Mutex::new(HashMap::new()),
            market_data_type: AtomicI32::new(1),
            mdt_sent: Mutex::new(HashSet::new()),
            mdt_by_req: Mutex::new(HashMap::new()),
            epoch_dates_by_req: Mutex::new(HashSet::new()),
            hist_initial_complete: Mutex::new(HashSet::new()),
            // Empty until something states them. Which providers an account
            // may read is the venue's answer, given at logon; a pair of codes
            // standing in for it asked for news from providers the account
            // may not be entitled to and left out the ones it is.
            news_providers: Mutex::new(String::new()),
            news_askers: Mutex::new(HashMap::new()),
            contract_cache: Mutex::new(HashMap::new()),
            named_by_description: Mutex::new(HashMap::new()),
        }
    }

    /// Clear all per-session state so the owning client can reconnect.
    /// Refuse this session anything that changes a position.
    pub fn set_readonly(&self, on: bool) {
        self.readonly.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether this client refuses anything that would trade.
    pub fn is_readonly(&self) -> bool {
        self.readonly.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The refusal a read-only session gives, naming the call that was made.
    ///
    /// Loud rather than silent: a program that believes it placed an order and
    /// did not is worse off than one that stops.
    pub fn refuse_if_readonly(&self, what: &str) -> Result<(), String> {
        if self.is_readonly() {
            return Err(format!("this session is read-only; {what} was not sent"));
        }
        Ok(())
    }

    /// Forget everything this session held, so the next one starts clean.
    pub fn reset(&self) {
        self.req_to_instrument.lock().unwrap().clear();
        self.instrument_to_req.lock().unwrap().clear();
        self.tbt_to_instrument.lock().unwrap().clear();
        self.instrument_followers.lock().unwrap().clear();
        self.con_id_to_instrument.lock().unwrap().clear();
        self.last_quotes.lock().unwrap().clear();
        self.snapshot_reqs.lock().unwrap().clear();
        *self.pnl_req_id.lock().unwrap() = None;
        self.pnl_single_reqs.lock().unwrap().clear();
        *self.last_pnl.lock().unwrap() = [0; 3];
        self.last_pnl_single.lock().unwrap().clear();
        *self.account_summary_req.lock().unwrap() = None;
        self.bulletin_subscribed.store(false, Ordering::Relaxed);
        self.account_updates_subscribed.store(false, Ordering::Relaxed);
        *self.last_account.lock().unwrap() = None;
        self.last_stated_account.lock().unwrap().clear();
        self.account_end_sent.store(false, Ordering::Release);
        *self.last_portfolio.lock().unwrap() = None;
        self.executions.lock().unwrap().clear();
        self.open_orders.lock().unwrap().clear();
        self.market_data_type.store(1, Ordering::Relaxed);
        self.mdt_sent.lock().unwrap().clear();
        self.mdt_by_req.lock().unwrap().clear();
        self.epoch_dates_by_req.lock().unwrap().clear();
        self.hist_initial_complete.lock().unwrap().clear();
        self.news_providers.lock().unwrap().clear();
        self.news_askers.lock().unwrap().clear();
        self.contract_cache.lock().unwrap().clear();
        // What the venue named for a description belongs to the session that
        // asked. Kept across a reconnect — or a login as somebody else — the
        // next order goes out under an id this session was never given.
        self.named_by_description.lock().unwrap().clear();
        // A group this session joined, and what it was told about it. Kept,
        // the next session is called back about a group under a request id it
        // never subscribed with.
        self.display_groups.lock().unwrap().clear();
        self.group_subscriptions.lock().unwrap().clear();
        self.pending_group_events.lock().unwrap().clear();
    }

    // ── Registration helpers ──

    /// What this session waits, stated when it opened.
    ///
    /// The library's own tests wait a millisecond: a test with no engine to
    /// answer would otherwise wait the full default on every call.
    pub fn set_registration_timeout(&self, waiting: std::time::Duration) {
        *self.registration_timeout.lock().unwrap() = waiting;
    }

    fn registration_timeout(&self) -> std::time::Duration {
        *self.registration_timeout.lock().unwrap()
    }

    /// Wait for the hot loop to register a contract, and answer with the slot
    /// it gave. A full instrument table comes back as an `Err` for this
    /// request alone; the engine keeps running.
    fn recv_registration(
        &self, reply_rx: std::sync::mpsc::Receiver<Result<InstrumentId, String>>,
    ) -> Result<InstrumentId, Refusal> {
        use std::sync::mpsc::RecvTimeoutError;
        reply_rx.recv_timeout(self.registration_timeout())
            .map_err(|why| match why {
                // The engine took the command and then went. That is a session
                // to reopen, not a venue that stayed silent, and a caller
                // branching on the code has to be able to tell them apart.
                RecvTimeoutError::Disconnected => {
                    Refusal::not_connected("Engine stopped before it answered")
                }
                RecvTimeoutError::Timeout => Refusal::no_answer("Registration timed out"),
            })?
            // A full instrument table is the caller asking for more than this
            // session can hold, which is theirs to fix.
            .map_err(Refusal::validation)
    }

    /// The instrument this conId is already known to hold. `0` means the
    /// contract carries no conId and answers for no one: the engine
    /// resolves those by descriptor, so only it can say which slot they got.
    pub(crate) fn cached_instrument(&self, con_id: i64) -> Option<InstrumentId> {
        if con_id == 0 {
            return None;
        }
        self.con_id_to_instrument.lock().unwrap().get(&con_id).copied()
    }

    /// Cache the engine's answer for later lookups. Caching it under `0` would
    /// point every conId-less contract at the first one's slot.
    fn cache_instrument(&self, con_id: i64, instrument: InstrumentId) {
        if con_id != 0 {
            self.con_id_to_instrument.lock().unwrap().insert(con_id, instrument);
        }
    }

    /// Whether somebody already holds this contract, in which case this
    /// request watches theirs.
    ///
    /// Asks only. A contract nobody holds is not taken here: this runs as a
    /// question about a cached contract, and a subscription that goes on to
    /// fail would leave a holder recorded for a request that never started —
    /// which nothing then cancels. [`take_or_follow`](Self::take_or_follow) is
    /// where it is taken.
    pub(crate) fn follows_existing_subscription(&self, instrument: InstrumentId, req_id: i64) -> bool {
        let held = self.instrument_to_req.lock().unwrap();
        match held.get(&instrument) {
            Some(&existing) if existing != req_id => {
                self.follow_under_holder_lock(instrument, req_id);
                drop(held);
                // Outside both, because a request pointing at the instrument it
                // follows is not what a withdrawal races against.
                self.req_to_instrument.lock().unwrap().insert(req_id, instrument);
                true
            }
            _ => false,
        }
    }

    /// Watch a contract somebody else holds, with the holder map already held.
    ///
    /// The follower is written down before the holder can be released. Recorded
    /// after instead, a withdrawal of the holder running in between finds
    /// nobody watching, takes the subscription down, and the follower is left
    /// registered against a feed that has gone — told nothing, and with nothing
    /// on the wire. The holder map is what decides, so the follower is recorded
    /// under it, the same way taking one is.
    ///
    /// `instrument_followers` is taken under `instrument_to_req` here and
    /// nowhere the other way round: the withdrawal path releases the followers
    /// before it touches the holder map.
    fn follow_under_holder_lock(&self, instrument: InstrumentId, req_id: i64) {
        let mut following = self.instrument_followers.lock().unwrap();
        let watchers = following.entry(instrument).or_default();
        if !watchers.contains(&req_id) {
            watchers.push(req_id);
        }
    }

    /// Hold this contract, or follow whoever took it first.
    ///
    /// `instrument_to_req` maps one request per instrument: a second holder
    /// would clobber the first's reverse mapping and orphan it silently — no
    /// ticks, no error. Deciding and taking happen under one acquisition, so
    /// two callers subscribing the same unheld contract cannot both take it
    /// and leave the loser cancelling the winner's feed.
    ///
    /// Answers whether this request ended up a follower.
    pub(crate) fn take_or_follow(&self, instrument: InstrumentId, req_id: i64) -> bool {
        let mut held = self.instrument_to_req.lock().unwrap();
        match held.get(&instrument) {
            Some(&existing) if existing != req_id => {
                self.follow_under_holder_lock(instrument, req_id);
                drop(held);
                // Outside both, because a request pointing at the instrument it
                // follows is not what a withdrawal races against.
                self.req_to_instrument.lock().unwrap().insert(req_id, instrument);
                true
            }
            _ => {
                held.insert(instrument, req_id);
                false
            }
        }
    }

    /// Every other request watching a contract, so one quote reaches them all.
    pub fn followers_of(&self, instrument: InstrumentId) -> Vec<i64> {
        self.instrument_followers
            .lock()
            .unwrap()
            .get(&instrument)
            .cloned()
            .unwrap_or_default()
    }

    /// Find instrument ID for a contract, registering if needed.
    /// Returns `Err` if the control channel is closed.
    pub fn find_or_register_instrument(
        &self,
        control_tx: &SyncSender<ControlCommand>,
        con_id: i64,
        symbol: &str,
        exchange: &str,
        sec_type: &str,
        identity: &str,
    ) -> Result<InstrumentId, Refusal> {
        // The cache is skipped when the caller states an identity, because the
        // slot may have been allocated by a market-data subscription that had
        // none — and the engine is where the identity is stored. Short-circuiting
        // here sent the order with a correct security type and destination but no
        // expiry, so a future named its exchange and not its month. Registration
        // is idempotent: the engine returns the same slot and adopts the identity.
        if identity.is_empty()
            && let Some(iid) = self.cached_instrument(con_id) {
                return Ok(iid);
            }

        // Register new — only allocates an InstrumentId slot, does not subscribe to
        // market data.
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        control_tx.send(ControlCommand::RegisterInstrument {
            contract: ContractRef { con_id, symbol: symbol.to_string(), sec_type: sec_type.to_string(), exchange: exchange.to_string(), ..Default::default() },
            identity: identity.to_string(),
            reply_tx: Some(reply_tx),
        }).map_err(|e| Refusal::not_connected(format!("Engine stopped: {e}")))?;

        let id = self.recv_registration(reply_rx)?;
        self.cache_instrument(con_id, id);
        Ok(id)
    }

    /// A request is being made under this id, so whatever a request under it
    /// finished before is over.
    ///
    /// Bars answering a fresh request were delivered as though they continued
    /// the last one — as updates, with no completion — because the id had
    /// been marked finished and nothing unmarked it. A caller looping over
    /// contracts under one id was answered once and never again.
    pub fn historical_request_is_new(&self, req_id: u32) {
        self.hist_initial_complete.lock().unwrap().remove(&req_id);
    }

    // ── Subscription management ──

    /// Record that this request asked for the headlines on an instrument.
    ///
    /// Called on every way out of a registration. Registration leaves by more
    /// than one door — the quotes may already be up, and may come up while
    /// this request was being made — and the subscription is sent before any
    /// of them. A door that does not record it sends headlines nothing will
    /// ever withdraw.
    /// Record that this request wants the headlines on a contract, and say
    /// whether it is the first to ask.
    ///
    /// Decided and recorded together, so two requests racing for one contract
    /// cannot both find that nobody has asked. The venue is asked by contract
    /// and withdrawn by contract, so asking twice leaves a subscription the
    /// one withdrawal cannot match.
    pub(crate) fn first_to_ask_for_news(&self, con_id: i64, req_id: i64) -> bool {
        let mut news = self.news_askers.lock().unwrap();
        let askers = news.entry(con_id).or_default();
        let first = askers.is_empty();
        askers.insert(req_id);
        first
    }

    /// Register a market data subscription mapping.
    /// If `generic_tick_list` contains "292", also subscribes to per-contract news.
    pub fn register_mkt_data(
        &self,
        shared: &SharedState,
        control_tx: &SyncSender<ControlCommand>,
        req_id: i64,
        con_id: i64,
        symbol: &str,
        exchange: &str,
        sec_type: &str,
        currency: &str,
        last_trade_date: &str,
        strike: f64,
        right: &str,
        multiplier: &str,
        snapshot: bool,
        regulatory_snapshot: bool,
        generic_tick_list: &str,
        mode_9887: i32,
    ) -> Result<InstrumentId, Refusal> {
        // The chargeable snapshot is one burst by construction, so it ends the
        // way an ordinary snapshot does and the caller hears the same end.
        let snapshot = snapshot || regulatory_snapshot;
        // News subscription if generic_tick_list names 292. The whole entry,
        // not its last three characters: "1292" is not 292, and matching on a
        // suffix subscribes to news the caller did not ask for. The list is
        // split on commas first, so a comma-joined entry never matches.
        let wants_news = generic_tick_list.split(',').any(|t| t.trim() == "292");
        // And nothing else is served. Not because the protocol cannot carry
        // it: a tick is asked for as a subscription of its own, under the
        // venue's number for it in the request type, which is how the option
        // model, the trading status and the venue map are already asked for
        // here. What is missing is the venue's number for each of the numbers
        // a caller names, and a reader for what each one answers with.
        //
        // So every other number is reported rather than accepted in silence —
        // option volume, shortable shares. A caller hears that it will not
        // arrive instead of watching for a tick that never comes.
        let unsent: Vec<&str> = generic_tick_list
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty() && *t != "292" && *t != "mdoff")
            .collect();
        if !unsent.is_empty() {
            log::warn!(
                "generic tick(s) {} were asked for and are not served by this \
                 client, so no tick of those kinds will arrive",
                unsent.join(", "),
            );
        }
        // Asked for once per contract, whoever asks. Recorded as the decision
        // is made, so two callers racing for one contract cannot both find
        // that nobody has asked.
        if wants_news && self.first_to_ask_for_news(con_id, req_id) {
            // What the logon said this account may read, unless a caller has
            // named its own set. The venue separates codes with a star.
            let named = self.news_providers.lock().unwrap().clone();
            let providers = if named.is_empty() {
                shared.reference.news_providers()
                    .iter()
                    .map(|p| p.code.as_str())
                    .collect::<Vec<_>>()
                    .join("*")
            } else {
                named
            };
            let _ = control_tx.send(ControlCommand::SubscribeNews {
                con_id,
                symbol: symbol.to_string(),
                sec_type: sec_type.to_string(),
                providers,
                reply_tx: None,
            });
        }

        // A contract already being watched needs no second subscription: this
        // caller watches the one that is up, and hears the same quotes under
        // its own request. Nothing goes to the engine.
        //
        // Except a chargeable snapshot, which is a request of its own and not
        // a share of somebody's stream. Following one instead sends nothing,
        // bills nothing, and lets an account with no entitlement hear an end
        // it was never refused — off a stream it did not ask for.
        if !regulatory_snapshot
            && let Some(instrument) = self.cached_instrument(con_id)
            && self.follows_existing_subscription(instrument, req_id)
        {
            self.mdt_by_req.lock().unwrap().insert(
                req_id,
                match mode_9887 {
                    1 => MDT_DELAYED,
                    2 => MDT_FROZEN,
                    3 => MDT_DELAYED_FROZEN,
                    _ => self.market_data_type.load(Ordering::Relaxed),
                },
            );
            if snapshot {
                self.snapshot_reqs.lock().unwrap().insert(req_id, (std::time::Instant::now(), 0));
            }
            // The news subscription was sent above whether or not the quotes
            // were already up, so it is recorded here as well. Recorded only
            // on the path that also opened the quotes, it was never withdrawn:
            // the caller stopped watching and the headlines kept coming.
            return Ok(instrument);
        }

        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        control_tx.send(ControlCommand::RegisterInstrument {
            contract: ContractRef { con_id, symbol: symbol.to_string(), sec_type: sec_type.to_string(), exchange: exchange.to_string(), ..Default::default() },
            identity: String::new(),
            reply_tx: None,
        }).map_err(|e| Refusal::not_connected(format!("Engine stopped: {e}")))?;
        control_tx.send(ControlCommand::Subscribe {
            contract: ContractRef { con_id, symbol: symbol.to_string(), exchange: exchange.to_string(), sec_type: sec_type.to_string(), currency: currency.to_string(), last_trade_date: last_trade_date.to_string(), strike, right: right.to_string(), multiplier: multiplier.to_string() },
            mode_9887,
            regulatory_snapshot,
            reply_tx: Some(reply_tx),
        }).map_err(|e| Refusal::not_connected(format!("Engine stopped: {e}")))?;

        // The engine answers this one. A conId-less contract has no client-side
        // identity, so a duplicate can only be settled against the slot the
        // engine resolved — and refusing here, after `Subscribe` had already
        // gone out, left a live subscription the caller was told did not happen
        // and held no req_id to cancel by. The engine now refuses
        // before the subscribe reaches the wire and that refusal arrives here.
        let instrument_id = match self.recv_registration(reply_rx) {
            Ok(id) => id,
            Err(refused) => {
                // The headlines were asked for before this could fail, and the
                // record of who asked is what decides whether the next caller
                // sends the request at all. Left standing for a request that
                // never started, this contract's headlines are never asked for
                // again — the next caller reads somebody as already watching —
                // and the subscription that did go out cannot be withdrawn,
                // because the path that withdraws it needs a request this one
                // no longer has.
                if wants_news
                    && let Some(instrument) = self.release_news(req_id)
                {
                    let _ = control_tx.send(ControlCommand::UnsubscribeNews { instrument });
                }
                return Err(refused);
            }
        };
        self.cache_instrument(con_id, instrument_id);
        // The contract may have been named only by symbol, in which case the
        // engine is the first to know which slot it holds — and it may already
        // be watched. This caller watches it too rather than taking it over —
        // unless it asked for the chargeable snapshot, which is its own
        // request and was already sent above.
        if !regulatory_snapshot && self.follows_existing_subscription(instrument_id, req_id) {
            self.mdt_by_req.lock().unwrap().insert(
                req_id,
                match mode_9887 {
                    1 => MDT_DELAYED,
                    2 => MDT_FROZEN,
                    3 => MDT_DELAYED_FROZEN,
                    _ => self.market_data_type.load(Ordering::Relaxed),
                },
            );
            if snapshot {
                self.snapshot_reqs.lock().unwrap().insert(req_id, (std::time::Instant::now(), 0));
            }
            return Ok(instrument_id);
        }
        // Somebody may have taken this contract while this request was being
        // registered, in which case this one watches theirs.
        let _ = self.take_or_follow(instrument_id, req_id);
        self.req_to_instrument.lock().unwrap().insert(req_id, instrument_id);
        // What this request asked for, so its own callback says so rather than
        // reporting the type set for everything else.
        self.mdt_by_req.lock().unwrap().insert(
            req_id,
            match mode_9887 {
                1 => MDT_DELAYED,
                2 => MDT_FROZEN,
                3 => MDT_DELAYED_FROZEN,
                _ => self.market_data_type.load(Ordering::Relaxed),
            },
        );
        if snapshot {
            self.snapshot_reqs.lock().unwrap().insert(req_id, (std::time::Instant::now(), 0));
        }
        Ok(instrument_id)
    }

    /// Drop this request's claim on the headlines, and say whether that was
    /// the last one. Called on every path out of a withdrawal: the quotes may
    /// stay up for another caller while the headlines this one asked for stop.
    pub(crate) fn release_news(&self, req_id: i64) -> Option<InstrumentId> {
        let emptied = {
            let mut news = self.news_askers.lock().unwrap();
            let mut done: Option<i64> = None;
            for (con_id, askers) in news.iter_mut() {
                if askers.remove(&req_id) {
                    if askers.is_empty() {
                        done = Some(*con_id);
                    }
                    break;
                }
            }
            let con_id = done?;
            news.remove(&con_id);
            con_id
        };
        // Named by the instrument, which is what a withdrawal states. Known by
        // now: nothing is withdrawn that was never registered.
        self.cached_instrument(emptied)
    }

    /// Unregister a market data subscription.
    ///
    /// Answers with the subscription to withdraw, and separately with the
    /// instrument whose headlines stop. They are not the same question: the
    /// quotes stay up for another caller while the headlines this one asked
    /// for end, and a caller told only that the quotes stay up sent nothing
    /// and left the headlines running.
    pub fn unregister_mkt_data(
        &self, req_id: i64,
    ) -> (Option<InstrumentId>, Option<InstrumentId>) {
        // Whatever this id was waiting to finish, it is not waiting any
        // more. Left behind, the same id handed out again for an ordinary
        // stream reads as a snapshot and is withdrawn as soon as it has both
        // sides of a quote.
        self.snapshot_reqs.lock().unwrap().remove(&req_id);
        if let Some(instrument) = self.req_to_instrument.lock().unwrap().remove(&req_id) {
            // A caller that was watching someone else's subscription stops
            // watching it, and the subscription stays up for the rest. A
            // caller that held it hands it to the next one watching rather
            // than taking the quotes away from them.
            {
                let mut following = self.instrument_followers.lock().unwrap();
                let watching = following.get_mut(&instrument);
                if let Some(watchers) = watching {
                    let was_following = watchers.contains(&req_id);
                    watchers.retain(|&id| id != req_id);
                    let next = if was_following { None } else { watchers.first().copied() };
                    if let Some(next) = next {
                        watchers.retain(|&id| id != next);
                    }
                    if watchers.is_empty() {
                        following.remove(&instrument);
                    }
                    drop(following);
                    self.mdt_sent.lock().unwrap().remove(&req_id);
                    self.mdt_by_req.lock().unwrap().remove(&req_id);
                    if was_following {
                        return (None, self.release_news(req_id));
                    }
                    if let Some(next) = next {
                        self.instrument_to_req.lock().unwrap().insert(instrument, next);
                        return (None, self.release_news(req_id));
                    }
                }
            }
            self.instrument_to_req.lock().unwrap().remove(&instrument);
            self.last_quotes.lock().unwrap().remove(&instrument);
            self.mdt_sent.lock().unwrap().remove(&req_id);
            self.mdt_by_req.lock().unwrap().remove(&req_id);
            let stop_news = self.release_news(req_id);
            self.forget_instrument(instrument);
            (Some(instrument), stop_news)
        } else {
            (None, None)
        }
    }

    /// Drop the client-side conId cache entries for an instrument id. The
    /// engine may reclaim and reuse the slot after an unsubscribe;
    /// a stale cache entry would silently point the old conId at whatever
    /// contract inherits the id. A later request for that conId simply
    /// re-registers.
    pub fn forget_instrument(&self, instrument: InstrumentId) {
        self.con_id_to_instrument.lock().unwrap().retain(|_, iid| *iid != instrument);
    }

    /// Name the providers to ask for news from, overriding what the logon
    /// said this account may read. An empty string returns to that.
    pub fn set_news_providers(&self, providers: &str) {
        *self.news_providers.lock().unwrap() = providers.to_string();
    }

    // ── Contract cache ──

    /// How a contract was asked about, as one string.
    ///
    /// Built from what the caller wrote, not from what the venue answered: a
    /// caller who says `SMART` gets `SMART` back on the next order, and looking
    /// the answer up under the exchange the venue routed it to would never
    /// match.
    pub fn description_key(c: &ApiContract) -> String {
        Self::description_key_of(
            &c.symbol, &c.sec_type, &c.exchange,
            &crate::types::model::contract_identity(
                &c.last_trade_date_or_contract_month, c.strike, &c.right,
                &c.multiplier, &c.currency,
            ),
            &c.primary_exchange, &c.local_symbol, &c.trading_class,
            &c.sec_id_type, &c.sec_id, &c.currency,
        )
    }

    /// The same key from the parts, for surfaces that carry their own contract
    /// type rather than this one.
    ///
    /// Everything the lookup narrows on is in here. Two descriptions that
    /// differ only in a field the key leaves out are one key, and the second
    /// order goes out under the first one's contract.
    pub fn description_key_of(
        symbol: &str, sec_type: &str, exchange: &str, identity: &str,
        primary_exchange: &str, local_symbol: &str, trading_class: &str,
        sec_id_type: &str, sec_id: &str, currency: &str,
    ) -> String {
        // The currency verbatim, beside the identity that has already folded
        // it. A contract's identity treats saying nothing and saying USD as
        // the same thing, which is right for the slot an order is placed
        // through and wrong here: the lookup sends the currency as a filter,
        // so a description that stated none can be answered with a listing in
        // another one — and under a shared key, the next order that does say
        // USD would be placed on it.
        format!(
            "{symbol}|{sec_type}|{exchange}|{identity}|{primary_exchange}|\
             {local_symbol}|{trading_class}|{sec_id_type}|{sec_id}|{currency}"
        )
    }

    /// The contract the venue named for a description, if it has named one.
    pub fn named_for(&self, key: &str) -> Option<ApiContract> {
        self.named_by_description.lock().unwrap().get(key).cloned()
    }

    /// Remember what the venue named a description, for the next order on it.
    pub fn remember_named(&self, key: String, contract: ApiContract) {
        self.named_by_description.lock().unwrap().insert(key, contract);
    }

    /// Cache a contract for later enrichment.
    pub fn cache_contract(&self, con_id: i64, contract: ApiContract) {
        self.contract_cache.lock().unwrap().insert(con_id, contract);
    }

    /// Look up a contract: merge local cache with shared reference for richest data.
    pub fn get_contract(&self, con_id: i64, shared: &SharedState) -> Option<ApiContract> {
        let local = self.contract_cache.lock().unwrap().get(&con_id).cloned();
        let shared_ref = shared.reference.get_contract(con_id);
        match (local, shared_ref) {
            (Some(mut l), Some(s)) => {
                // Enrich local with shared reference fields (secdef has richer data)
                if l.local_symbol.is_empty() { l.local_symbol = s.local_symbol; }
                if l.trading_class.is_empty() { l.trading_class = s.trading_class; }
                if l.primary_exchange.is_empty() { l.primary_exchange = s.primary_exchange; }
                Some(l)
            }
            (Some(l), None) => Some(l),
            (None, Some(s)) => Some(s),
            (None, None) => None,
        }
    }

    /// Register a TBT subscription mapping.
    pub fn register_tbt(
        &self,
        _shared: &SharedState,
        control_tx: &SyncSender<ControlCommand>,
        req_id: i64,
        con_id: i64,
        symbol: &str,
        sec_type: &str,
        exchange: &str,
        tbt_type: TbtType,
    ) -> Result<InstrumentId, Refusal> {
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        control_tx.send(ControlCommand::SubscribeTbt {
            contract: ContractRef { con_id, symbol: symbol.to_string(), sec_type: sec_type.to_string(), exchange: exchange.to_string(), ..Default::default() },
            req_id,
            tbt_type,
            reply_tx: Some(reply_tx),
        }).map_err(|e| Refusal::not_connected(format!("Engine stopped: {e}")))?;

        let instrument_id = self.recv_registration(reply_rx)?;
        self.cache_instrument(con_id, instrument_id);
        self.tbt_to_instrument.lock().unwrap().insert(req_id, instrument_id);
        Ok(instrument_id)
    }

    /// Look up req_id for an instrument.
    pub fn req_id_for_instrument(&self, instrument: InstrumentId) -> i64 {
        self.instrument_to_req.lock().unwrap()
            .get(&instrument).copied().unwrap_or(-1)
    }

    // ── Display groups ──

    /// The groups this client offers. Seven, matching what the vendor's client
    /// presents, numbered from one.
    const DISPLAY_GROUPS: i32 = 7;

    /// What a group holds when nothing has been put in it.
    const NO_CONTRACT: &'static str = "none";

    /// Ask which display groups exist.
    pub fn query_display_groups(&self, req_id: i64) {
        let groups = (1..=Self::DISPLAY_GROUPS)
            .map(|g| g.to_string())
            .collect::<Vec<_>>()
            .join("|");
        self.pending_group_events.lock().unwrap().push(GroupEvent::List(req_id, groups));
    }

    /// Follow a group. The caller is told what the group holds now, not only
    /// what it changes to, or a caller that subscribes to a settled group
    /// hears nothing at all.
    pub fn subscribe_to_group_events(&self, req_id: i64, group_id: i32) {
        self.group_subscriptions.lock().unwrap().insert(req_id, group_id);
        let held = self.display_groups.lock().unwrap()
            .get(&group_id)
            .cloned()
            .unwrap_or_else(|| Self::NO_CONTRACT.to_string());
        self.pending_group_events.lock().unwrap().push(GroupEvent::Updated(req_id, held));
    }

    /// Stop the from group events.
    pub fn unsubscribe_from_group_events(&self, req_id: i64) {
        self.group_subscriptions.lock().unwrap().remove(&req_id);
    }

    /// Put a contract in the group the request follows, and tell everyone else
    /// following it. The caller that made the change is told too, which is what
    /// keeps two callers holding the same group in step.
    pub fn update_display_group(&self, req_id: i64, contract_info: &str) -> Result<(), String> {
        let subs = self.group_subscriptions.lock().unwrap();
        let Some(&group_id) = subs.get(&req_id) else {
            return Err(format!(
                "request {req_id} follows no display group, so there is none to put a \
                 contract in: subscribe to a group first"
            ));
        };
        let followers: Vec<i64> = subs.iter()
            .filter(|(_, g)| **g == group_id)
            .map(|(r, _)| *r)
            .collect();
        drop(subs);
        let value = if contract_info.is_empty() { Self::NO_CONTRACT } else { contract_info };
        self.display_groups.lock().unwrap().insert(group_id, value.to_string());
        let mut pending = self.pending_group_events.lock().unwrap();
        for r in followers {
            pending.push(GroupEvent::Updated(r, value.to_string()));
        }
        Ok(())
    }

    /// Take every group events waiting, leaving none.
    pub fn drain_group_events(&self) -> Vec<GroupEvent> {
        self.pending_group_events.lock().unwrap().drain(..).collect()
    }

    // ── PnL subscription management ──

    /// Ask for the pnl.
    pub fn subscribe_pnl(&self, req_id: i64) {
        *self.pnl_req_id.lock().unwrap() = Some(req_id);
        // Nothing has been reported to this subscription yet. Without a value
        // that no account can hold, an account whose P&L is genuinely zero
        // matched the initial state and the caller was told nothing at all.
        *self.last_pnl.lock().unwrap() = [i64::MIN; 3];
    }

    /// Stop the pnl.
    pub fn unsubscribe_pnl(&self, req_id: i64) {
        let mut pnl = self.pnl_req_id.lock().unwrap();
        if *pnl == Some(req_id) {
            *pnl = None;
        }
    }

    /// Ask for the pnl single.
    pub fn subscribe_pnl_single(&self, req_id: i64, con_id: i64) {
        self.pnl_single_reqs.lock().unwrap().insert(req_id, con_id);
    }

    /// Stop the pnl single.
    pub fn unsubscribe_pnl_single(&self, req_id: i64) {
        self.pnl_single_reqs.lock().unwrap().remove(&req_id);
        self.last_pnl_single.lock().unwrap().remove(&req_id);
    }

    // ── Account summary subscription management ──

    /// Ask for the account summary.
    pub fn subscribe_account_summary(&self, req_id: i64, tags: &str) {
        let tag_list: Vec<String> = tags.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        *self.account_summary_req.lock().unwrap() = Some((req_id, tag_list));
    }

    /// Stop the account summary.
    pub fn unsubscribe_account_summary(&self, req_id: i64) {
        let mut req = self.account_summary_req.lock().unwrap();
        if req.as_ref().map(|(r, _)| *r) == Some(req_id) {
            *req = None;
        }
    }

    // ── Account updates subscription management ──

    /// Ask for the account updates.
    pub fn subscribe_account_updates(&self, subscribe: bool) {
        self.account_updates_subscribed.store(subscribe, Ordering::Release);
        if !subscribe {
            *self.last_account.lock().unwrap() = None;
        self.last_stated_account.lock().unwrap().clear();
        self.account_end_sent.store(false, Ordering::Release);
            *self.last_portfolio.lock().unwrap() = None;
        }
    }

    // ── Market data type tracking ──

    /// Store the requested market data type.
    ///
    /// The caller names the type once; the wire names it per subscription, on
    /// field 9887. Subscriptions made after this carry the mode it implies, so
    /// a client that asks for delayed data receives delayed data.
    pub fn set_market_data_type(&self, mdt: i32) {
        if !matches!(mdt, MDT_REALTIME | MDT_FROZEN | MDT_DELAYED | MDT_DELAYED_FROZEN) {
            // Kept out rather than kept: subscriptions stay realtime whatever
            // this names, and the callback that reports a subscription's type
            // reads what is stored — so a number nobody recognises, stored,
            // reaches the caller as the venue's word for data that is not on
            // it.
            log::warn!("req_market_data_type({mdt}) names no known type; subscriptions stay realtime");
            return;
        }
        self.market_data_type.store(mdt, Ordering::Relaxed);
    }

    /// The per-subscription mode the requested type implies. Zero is realtime,
    /// which is the shape a subscription has when nothing asked otherwise.
    pub fn subscription_mode(&self) -> i32 {
        match self.market_data_type.load(Ordering::Relaxed) {
            MDT_DELAYED => 1,
            MDT_FROZEN => 2,
            MDT_DELAYED_FROZEN => 3,
            _ => 0,
        }
    }

    /// Check if the `market_data_type` callback should fire for this req_id.
    /// Returns `Some(type)` on the first call per req_id that has data, `None`
    /// thereafter. Always reports realtime — the DELIVERED type — rather than
    /// echoing a type the engine did not transmit, which would confirm a state
    /// the session is not in.
    pub fn check_mdt_needed(&self, req_id: i64, has_data: bool) -> Option<i32> {
        if has_data && self.mdt_sent.lock().unwrap().insert(req_id) {
            // The type this subscription was made under, which is the type
            // transmitted with it and therefore the type of the data.
            Some(
                self.mdt_by_req
                    .lock()
                    .unwrap()
                    .get(&req_id)
                    .copied()
                    .unwrap_or_else(|| self.market_data_type.load(Ordering::Relaxed)),
            )
        } else {
            None
        }
    }

    // ── Bulletin subscription management ──

    /// Ask for the bulletins.
    pub fn subscribe_bulletins(&self) {
        self.bulletin_subscribed.store(true, Ordering::Release);
    }

    /// Stop the bulletins.
    pub fn unsubscribe_bulletins(&self) {
        self.bulletin_subscribed.store(false, Ordering::Release);
    }

    /// Whether broadcast notices are being received.
    pub fn bulletins_subscribed(&self) -> bool {
        self.bulletin_subscribed.load(Ordering::Acquire)
    }

    // ── Execution replay store ──

    /// Store an execution for later replay via `req_executions`.
    pub fn push_execution(&self, req_id: i64, contract: ApiContract, execution: ApiExecution, commission_and_fees: ApiCommissionAndFeesReport) {
        self.executions.lock().unwrap().push(StoredExecution {
            req_id, contract, execution, commission_and_fees,
        });
    }

    /// Note what an execution cost against the execution itself, so a replay
    /// of it carries the charge the venue stated rather than the nothing it
    /// was stored with.
    pub fn record_charge(&self, charge: &ApiCommissionAndFeesReport) {
        let mut execs = self.executions.lock().unwrap();
        for stored in execs.iter_mut() {
            if stored.execution.exec_id == charge.exec_id {
                stored.commission_and_fees = charge.clone();
            }
        }
    }

    /// Executions matching `filter`, cloned out under one short lock.
    ///
    /// Callers replay these into user callbacks, and a callback may re-enter
    /// any path that locks `executions` — re-requesting from `exec_details` is
    /// an ordinary ibapi pattern, and the dispatch thread pushes fills through
    /// the same mutex. Handing back indices to be dereferenced later also
    /// raced `reset()`, which clears the vector. Snapshotting closes both.
    pub fn snapshot_executions(&self, filter: &ExecutionFilter) -> Vec<StoredExecution> {
        let execs = self.executions.lock().unwrap();
        execs.iter().filter(|se| execution_matches(se, filter)).cloned().collect()
    }

    // ── Open order tracking ──

    /// The parent this client recorded when it placed the order, if any.
    ///
    /// The engine reads no parent from an execution report, so for an order
    /// this client placed its own record is the only source. An order placed
    /// elsewhere keeps whatever the engine reports.
    pub(crate) fn tracked_parent_id(&self, order_id: u64) -> Option<i64> {
        self.open_orders.lock().unwrap()
            .get(&order_id)
            .map(|t| t.order.parent_id)
            .filter(|p| *p > 0)
    }

    /// Check if an order with this ID is currently tracked (for modify detection).
    pub fn is_order_tracked(&self, order_id: u64) -> bool {
        self.open_orders.lock().unwrap().contains_key(&order_id)
    }

    /// The order a tracked id was submitted with, if it is tracked.
    pub fn tracked_order(&self, order_id: u64) -> Option<ApiOrder> {
        self.open_orders.lock().unwrap().get(&order_id).map(|t| t.order.clone())
    }

    /// Why a replace cannot restate this order, or `None` if it can.
    ///
    /// The replace states the order type and the prices that type carries, plus
    /// the companions the type needs, restated from the record of the order as
    /// it was placed. What it does not state is the peg offset, the execution
    /// instruction or the algo block. For an order defined by any of those, the
    /// replace describes something other than the order being replaced, and the
    /// venue rejects it — leaving the caller with nothing resting.
    ///
    /// `restating_itself` says the replace keeps the order's type. Only then
    /// can a type be restated from the record of the order that was placed: a
    /// conversion into a trailing stop has no such record, so its trail goes
    /// unstated and the venue refuses the replace naming tag 211.
    ///
    /// The order type alone does not decide this. An adaptive or algo order is
    /// an ordinary `LMT` that is defined by its algo tags, and an adjustable
    /// stop is an ordinary `STP` that is defined by its conversion — both are
    /// destroyed by a replace that states only the type.
    pub fn replace_cannot_restate(order: &ApiOrder, restating_itself: bool) -> Option<String> {
        if !order.algo_strategy.is_empty() {
            return Some(format!("an order running the {} algo", order.algo_strategy));
        }
        if !order.adjusted_order_type.is_empty() {
            return Some(format!("an order that adjusts to {}", order.adjusted_order_type));
        }
        if !order.conditions.is_empty() {
            return Some("a conditional order".to_string());
        }
        // These ride tags the replace does not carry either — hidden on 6135,
        // all-or-none as an execution instruction, and the cash quantity on
        // 5920 — so a replace states an order without them.
        if order.hidden {
            return Some("a hidden order".to_string());
        }
        if order.all_or_none {
            return Some("an all-or-none order".to_string());
        }
        if order.cash_qty > 0.0 {
            return Some("a cash-quantity order".to_string());
        }
        // A what-if is a margin preview, not a resting order, so there is
        // nothing on the book for a replace to act on.
        if order.what_if {
            return Some("a what-if order".to_string());
        }
        // The replace is rebuilt from the tracked record, which holds side,
        // price, quantity, order type, time-in-force and trigger — and nothing
        // else. Every attribute below rides a tag the replace does not carry,
        // so a modify would state the order without it.
        //
        // The bracket links are the costly pair. A replace that omits the
        // parent link or the OCA group leaves a child resting alone: a fill on
        // one leg no longer cancels the other, and the position is left with a
        // naked order against it. Whether the venue reads an omitted 583 or
        // 6107 as unchanged or as cleared is not established here, and the
        // difference between "latent" and "detached" is the whole risk — so
        // the modify is refused rather than sent and hoped for.
        if !order.oca_group.is_empty() {
            return Some("an order in an OCA group".to_string());
        }
        if order.parent_id != 0 {
            return Some("a bracket child".to_string());
        }
        if !order.good_till_date.is_empty() {
            return Some("an order with a good-till expiry".to_string());
        }
        if !order.good_after_time.is_empty() {
            return Some("an order with a good-after time".to_string());
        }
        if order.display_size > 0 {
            return Some("an iceberg order".to_string());
        }
        if order.min_qty > 0 {
            return Some("an order with a minimum quantity".to_string());
        }
        if order.discretionary_amt > 0.0 {
            return Some("a discretionary order".to_string());
        }
        if order.sweep_to_fill {
            return Some("a sweep-to-fill order".to_string());
        }
        if order.trigger_method != 0 {
            return Some("an order with a non-default trigger method".to_string());
        }
        let ty = order.order_type.to_uppercase();
        // `LIT` is submitted as `LT` but tracked under a byte the replace
        // renders as `K`, which is market-to-limit in this dialect — so a
        // replace would describe a different order type entirely.
        //
        // `MTL`, `BOX TOP` and `MKT PRT` are here because the replace renders
        // the same byte they were submitted under, so it restates them as
        // themselves — which is the whole test for membership.
        //
        // `TRAIL` restates itself on a session's answer rather than a reading:
        // the replace states the trail on tags 99 and 211 as the submit does,
        // the venue takes it, and the order goes on working.
        if matches!(
            ty.as_str(),
            "MKT" | "LMT" | "STP" | "STP LMT" | "MOC" | "LOC" | "MIT" | "STP PRT"
                | "MTL" | "BOX TOP" | "MKT PRT"
        ) || (restating_itself && ty == "TRAIL")
        {
            return None;
        }
        Some(format!("a {ty} order"))
    }

    /// Why a modify of `order_id` cannot be sent, if it cannot.
    ///
    /// Both sides are checked. The resting order is the one the replace has to
    /// restate, and the incoming order is what the caller is asking it to
    /// become — a modify that *adds* a bracket link or an OCA group states an
    /// order that has neither, so examining only the record on the book lets
    /// the attribute through on the very message that was supposed to carry it.
    ///
    /// One place, so the two bindings cannot diverge on either the rule or the
    /// wording.
    pub fn modify_refusal(&self, order_id: u64, incoming: &ApiOrder) -> Option<String> {
        let tracked = self.tracked_order(order_id);
        // Whether the replace leaves the order the type it already is.
        let restating_itself = tracked
            .as_ref()
            .is_some_and(|t| t.order_type.eq_ignore_ascii_case(&incoming.order_type));
        let why = tracked
            .and_then(|tracked| Self::replace_cannot_restate(&tracked, restating_itself))
            .or_else(|| Self::replace_cannot_restate(incoming, restating_itself))?;
        Some(format!(
            "{why} cannot be modified: the replace does not carry the fields that \
             define it, and sending one would cancel the order"
        ))
    }

    /// Track a newly placed order.
    pub fn track_order(&self, order_id: u64, contract: ApiContract, order: ApiOrder, instrument: InstrumentId) {
        let remaining = order.total_quantity;
        self.open_orders.lock().unwrap().insert(order_id, TrackedOrder {
            contract, order, status: "PendingSubmit".into(), filled: 0.0, remaining, instrument,
            rejected: false,
        });
    }

    /// Update a tracked order after a fill. Removes the order if fully filled.
    pub fn update_order_fill(&self, order_id: u64, status: &str, filled: f64, remaining: f64) {
        let mut orders = self.open_orders.lock().unwrap();
        if remaining == 0.0 {
            orders.remove(&order_id);
        } else if let Some(o) = orders.get_mut(&order_id) {
            o.status = status.into();
            o.filled = filled;
            o.remaining = remaining;
        }
    }

    /// Stop tracking an order the venue has said does not exist.
    ///
    /// A cancel rejected as UnknownOrder retires the engine's record, and the
    /// client's own record has to go with it — the open-order snapshot unions
    /// the two, so leaving this one behind kept reporting the order the
    /// rejection was about.
    pub fn untrack_order(&self, order_id: u64) {
        self.open_orders.lock().unwrap().remove(&order_id);
    }

    /// An order's permanent id and its parent.
    ///
    /// The parent is the one this client recorded where it placed the order:
    /// the engine reads no parent from a report, but a client that placed the
    /// order was told. An order it did not place keeps the engine's answer.
    pub(crate) fn perm_and_parent(&self, shared: &SharedState, order_id: u64) -> (i64, i64) {
        let (perm_id, engine_parent) = shared.orders.get_order_info(order_id)
            .map(|info| (info.order.perm_id, info.order.parent_id))
            .unwrap_or((0, 0));
        (perm_id, self.tracked_parent_id(order_id).unwrap_or(engine_parent))
    }

    /// Which client placed an order, as the venue states it on tag 109.
    ///
    /// Zero where this session has no record of the order, which is also what
    /// the venue states for an order it names no client for.
    pub(crate) fn placing_client(&self, shared: &SharedState, order_id: u64) -> i32 {
        shared.orders.get_order_info(order_id).map_or(0, |info| info.order.client_id)
    }

    /// What tells two contracts on one underlying apart.
    ///
    /// Written on the model now, since it is derived from a contract and needs
    /// nothing of this client's. Kept here because it is the name a program
    /// written against this client already calls.
    pub fn contract_identity(
        last_trade_date: &str, strike: f64, right: &str, multiplier: &str, currency: &str,
    ) -> String {
        crate::types::model::contract_identity(
            last_trade_date, strike, right, multiplier, currency,
        )
    }

    /// Retire the client's record of an order the venue rejected as unknown,
    /// and say what that rejection reports.
    ///
    /// Reason 1 is UnknownOrder: the venue has said the order does not exist,
    /// and the engine has already retired its record. The client's own record
    /// has to go with it, or the open-order snapshot keeps reporting the order
    /// the rejection was about.
    ///
    /// The code says the cancel was refused, and which of the two ways. 202
    /// means an order that was cancelled, so reporting it for a refused cancel
    /// states the opposite and invites a replacement against an order still
    /// working. 10147 means "not found", which is one reason among several.
    pub(crate) fn retire_rejected(&self, reject: &CancelReject) -> (i64, String) {
        if reject.reason_code == 1 {
            self.untrack_order(reject.order_id);
        }
        // 10147 is the order the venue could not find; 10148 is the order it
        // found and would not act on. The reason it stated picks between them.
        let code = if reject.reason_code == 1 { 10147 } else { 10148 };
        let what = if reject.reject_type == 1 { "cancel" } else { "modify" };
        (
            code,
            format!(
                "Order {} {what} rejected by the venue (reason: {})",
                reject.order_id, reject.reason_code,
            ),
        )
    }

    /// Update a tracked order status from an order update event.
    ///
    /// Takes the pre-stringification `OrderStatus` rather than the ibapi string
    /// so a Rejected transition stays distinct from a genuinely-Inactive one:
    /// both stringify to "Inactive", and only a genuine Inactive is
    /// reactivatable and belongs back in the open-order snapshot.
    ///
    /// Upserts, because an order recovered from an earlier session was never
    /// submitted by this client and so has no entry here. Doing nothing for it
    /// left `collect_open_orders` unable to tell it had just been withdrawn. A fresh
    /// entry seeds contract and order from the same enriched
    /// cache `collect_open_orders` reads, rather than leaving them blank.
    pub fn update_order_status(&self, shared: &SharedState, order_id: u64, status: OrderStatus, filled: f64, remaining: f64) {
        let mut orders = self.open_orders.lock().unwrap();
        let o = orders.entry(order_id).or_insert_with(|| {
            let (contract, order) = match shared.orders.get_order_info(order_id) {
                Some(info) => (info.contract, info.order),
                None => (ApiContract::default(), ApiOrder::default()),
            };
            TrackedOrder { contract, order, status: String::new(), rejected: false, filled: 0.0, remaining: 0.0, instrument: 0 }
        });
        o.status = order_status_str(status).into();
        o.rejected = status == OrderStatus::Rejected;
        o.filled = filled;
        o.remaining = remaining;
        drop(orders);
        // An order that is done is dropped, the way one that fills already is.
        // Only a fill removed it before, and a fill is not how most orders end:
        // a cancelled or rejected one reports a quantity still outstanding and
        // produces none, so it stayed for the life of the session and the cost
        // of listing what is open grew with every cancel. Nothing reads it
        // after this — what is open is answered by status, and these are not
        // open. `Inactive` is not among them: it returns to working when
        // whatever holds the order clears.
        if matches!(status, OrderStatus::Cancelled | OrderStatus::Rejected) {
            self.untrack_order(order_id);
        }
    }

    /// Collect open orders: merge local tracking with shared state.
    /// Returns (order_id, contract, order, status, filled, remaining) for non-terminal
    /// orders.
    pub fn collect_open_orders(&self, shared: &SharedState) -> Vec<(u64, TrackedOrder)> {
        let mut result: Vec<(u64, TrackedOrder)> = Vec::new();

        // Drain shared order cache first to enrich local tracking
        let shared_orders = shared.orders.drain_open_orders();
        {
            let mut orders = self.open_orders.lock().unwrap();
            for (oid, info) in &shared_orders {
                if let Some(o) = orders.get_mut(oid) {
                    if o.order.account.is_empty() {
                        o.order.account = info.order.account.clone();
                    }
                    if o.order.perm_id == 0 {
                        o.order.perm_id = info.order.perm_id;
                    }
                }
            }
        }

        // The orders whose status this client has withdrawn. A disconnect marks
        // a tracked order unknown without touching the cached view, so the union
        // below re-imported it as working for the whole outage, contradicting
        // the callback the caller had already been given. Scoped to
        // withdrawn statuses so the cache can still carry a genuinely newer one
        // — a terminal local status the cache has since superseded still wins.
        let status_withdrawn: std::collections::HashSet<u64> = self.open_orders.lock().unwrap()
            .iter()
            .filter(|(_, o)| o.status == "Unknown")
            .map(|(&id, _)| id)
            .collect();

        // Local tracked orders (non-terminal, or genuinely-Inactive and
        // still reactivatable —), enriched from secdef cache
        {
            let orders = self.open_orders.lock().unwrap();
            for (&oid, o) in orders.iter() {
                if is_open_status(&o.status) || (o.status == "Inactive" && !o.rejected) {
                    let contract = if o.contract.con_id != 0 {
                        self.get_contract(o.contract.con_id, shared).unwrap_or_else(|| o.contract.clone())
                    } else {
                        o.contract.clone()
                    };
                    result.push((oid, TrackedOrder {
                        contract,
                        order: o.order.clone(),
                        status: o.status.clone(),
                        filled: o.filled,
                        remaining: o.remaining,
                        instrument: o.instrument,
                        rejected: o.rejected,
                    }));
                }
            }
        }

        // Add shared-only entries not already present from local
        for (oid, info) in shared_orders {
            if !is_open_or_reactivatable(&info.order_state.status, &info.order_state.completed_status) {
                continue;
            }
            if status_withdrawn.contains(&oid) {
                continue;
            }
            if !result.iter().any(|(id, _)| *id == oid) {
                let contract = if info.contract.con_id != 0 {
                    shared.reference.get_contract(info.contract.con_id).unwrap_or(info.contract)
                } else {
                    info.contract
                };
                // The order carries its own filled quantity; reporting zero
                // here made a partially filled order that this client did not
                // place read as untouched.
                let filled = info.order.filled_quantity;
                let remaining = (info.order.total_quantity - filled).max(0.0);
                result.push((oid, TrackedOrder {
                    contract,
                    order: info.order,
                    status: info.order_state.status.clone(),
                    filled,
                    remaining,
                    instrument: 0,
                    rejected: false,
                }));
            }
        }

        result
    }

    // ── Dispatch preparation methods ──

    /// Poll quotes for a single instrument and return tick events.
    /// Updates last_quotes internally.
    pub fn poll_instrument_ticks(
        &self,
        shared: &SharedState,
        iid: InstrumentId,
        req_id: i64,
    ) -> QuotePollResult {
        let q = shared.market.quote(iid);
        let fields = [
            q.bid, q.ask, q.last, q.bid_size, q.ask_size, q.last_size,
            q.high, q.low, q.volume, q.close, q.open, q.timestamp_ns as i64,
            q.bid_exch_mask, q.ask_exch_mask, q.last_exch_mask, q.halted,
        ];

        // Single lock acquisition for both read and write of last_quotes.
        let mut map = self.last_quotes.lock().unwrap();
        let last = map.get(&iid).copied().unwrap_or([0i64; 16]);

        let mut ticks = Vec::new();
        let mut delivered = false;

        // Price ticks: (field_index, tick_type)
        const PRICE_TICKS: &[(usize, i32)] = &[
            (0, TICK_BID), (1, TICK_ASK), (2, TICK_LAST),
            (6, TICK_HIGH), (7, TICK_LOW), (9, TICK_CLOSE), (10, TICK_OPEN),
        ];
        for &(idx, tt) in PRICE_TICKS {
            if fields[idx] != last[idx] {
                ticks.push(TickEvent {
                    req_id, tick_type: tt,
                    value: fields[idx] as f64 / PRICE_SCALE_F,
                    is_price: true,
                });
                delivered = true;
            }
        }

        // Size ticks: (field_index, tick_type)
        const SIZE_TICKS: &[(usize, i32)] = &[
            (3, TICK_BID_SIZE), (4, TICK_ASK_SIZE), (5, TICK_LAST_SIZE), (8, TICK_VOLUME),
        ];
        for &(idx, tt) in SIZE_TICKS {
            if fields[idx] != last[idx] {
                ticks.push(TickEvent {
                    req_id, tick_type: tt,
                    value: fields[idx] as f64 / QTY_SCALE as f64,
                    is_price: false,
                });
                delivered = true;
            }
        }

        // Timestamp tick
        let timestamp = if fields[11] != last[11] && fields[11] != 0 {
            Some(TimestampTick { req_id, timestamp_ns: fields[11] })
        } else {
            None
        };

        // Exchange-code string ticks: rendering is left to dispatch since it
        // depends on shared.reference.smart_components. Emit a delta record
        // when the bitmask changes; dispatch resolves the letter string.
        let mut string_ticks = Vec::new();
        // What is cached for this instrument, which is the quote as it stands
        // except where a field could not be rendered yet.
        let mut cached = fields;
        const EXCH_TICKS: &[(usize, i32)] = &[
            (12, TICK_BID_EXCHANGE), (13, TICK_ASK_EXCHANGE), (14, TICK_LAST_EXCHANGE),
        ];
        for &(idx, tt) in EXCH_TICKS {
            if fields[idx] != last[idx] {
                let letters = render_exchange_mask(fields[idx], shared);
                // A mask with bits set and no letters to show for them is
                // one the venue has not named its exchanges for yet. Caching
                // it as delivered leaves it equal to the next mask, so it is
                // never rendered again once the names arrive and the quote's
                // exchange is lost for the life of the subscription.
                if letters.is_empty() && fields[idx] != 0 {
                    cached[idx] = last[idx];
                    continue;
                }
                string_ticks.push(StringTickEvent {
                    req_id, tick_type: tt, value: letters,
                });
                delivered = true;
            }
        }

        // A halt changes what every other tick in this quote means: the prices
        // standing are the ones from before the venue stopped, not a market
        // anyone can deal on. It arrives on the trading-status tick and was
        // written into the quote, compared here, and then cached without being
        // sent anywhere — so the one transition worth hearing about was
        // consumed and could never be delivered again.
        //
        // The venue states it under a number of its own, so it goes out on
        // `tick_generic` as the reference client delivers it.
        let mut generic_ticks = Vec::new();
        if fields[15] != last[15] {
            generic_ticks.push(TickEvent {
                req_id, tick_type: TICK_HALTED,
                value: fields[15] as f64,
                is_price: false,
            });
            delivered = true;
        }

        map.insert(iid, cached);

        QuotePollResult { ticks, generic_ticks, string_ticks, timestamp, delivered }
    }

    /// Whether a snapshot has just finished arriving.
    ///
    /// Answers true once, for the pump that sees the snapshot complete. A
    /// request that is not a snapshot is never one of these.
    ///
    /// Note that the venue has stated one of the kinds a snapshot is made of.
    ///
    /// What counts is that a tick of the kind ARRIVED, not what it carried: a
    /// currency pair states its last as minus one and a contract that has not
    /// opened states its open as nothing, and both of those are the venue
    /// answering. Waiting for a figure above zero instead waits out the clock
    /// on every one of them.
    pub fn note_snapshot_tick(&self, req_id: i64, tick_type: i32) {
        let bit = match tick_type {
            1 => 1u8,   // bid
            2 => 2,     // ask
            4 => 4,     // last
            14 => 8,    // open
            9 => 16,    // close
            _ => return,
        };
        if let Some((_, stated)) = self.snapshot_reqs.lock().unwrap().get_mut(&req_id) {
            *stated |= bit;
        }
    }

    /// A snapshot ends when the venue has stated every kind one is made of, or
    /// when long enough has passed since it was asked for.
    ///
    /// Both are the reference client's: it holds a snapshot until the bid, the
    /// ask, the last, the open and the close have each been delivered, and
    /// sweeps anything still waiting eleven seconds after the REQUEST — not
    /// eleven since the last thing heard.
    ///
    /// Waiting on the quiet instead, as this did, ends a snapshot on a pause
    /// rather than on an answer, and a contract the venue never says anything
    /// about was never swept at all: the clock only started on the first
    /// delivery, so one that got none waited for ever.
    pub fn check_snapshot_done(&self, req_id: i64) -> bool {
        /// How long after asking the reference client gives up waiting for the
        /// rest of a snapshot.
        const GIVE_UP_AFTER: std::time::Duration = std::time::Duration::from_secs(11);
        /// Every kind: bid, ask, last, open, close.
        const WHOLE: u8 = 1 | 2 | 4 | 8 | 16;

        let mut waiting = self.snapshot_reqs.lock().unwrap();
        let Some((asked_at, stated)) = waiting.get(&req_id).copied() else {
            return false;
        };
        if stated == WHOLE || asked_at.elapsed() >= GIVE_UP_AFTER {
            waiting.remove(&req_id);
            return true;
        }
        false
    }

    /// Snapshot the current instrument→req_id mapping.
    pub fn snapshot_instruments(&self) -> Vec<(InstrumentId, i64)> {
        let map = self.instrument_to_req.lock().unwrap();
        map.iter().map(|(&iid, &req_id)| (iid, req_id)).collect()
    }

    /// What the venue last marked a contract at, which is its price at
    /// midnight rather than its price now. Read at the point of use; text that
    /// is not a usable price leaves the contract unmarked, rather than valuing
    /// it at whatever the characters happened to come to.
    fn midnight_price(shared: &SharedState, con_id: i64) -> Option<Price> {
        let raw = shared.portfolio.venue_price(con_id)?;
        let price = raw.trim().parse::<f64>().ok().filter(|p| p.is_finite())?;
        Some(crate::types::price_from_f64(price)).filter(|&p| p != 0)
    }

    /// Poll PnL and return update if values changed.
    /// Formula: dailyPnL = Σ(qtyNow × priceNow - valueAtMidnight + moneyTraded),
    /// where the venue states the midnight value and the price it last marked
    /// the contract at, and the client falls back to qtyMidnight × prevClose
    /// and a live quote for whatever the venue did not state.
    /// For positions opened intraday (no seed), synthesizes
    /// moneyTraded = -qtyNow × avgCost so the formula collapses to unrealized P&L.
    pub fn poll_pnl(&self, shared: &SharedState) -> Option<PnlUpdate> {
        let req_id = (*self.pnl_req_id.lock().unwrap())?;

        let seeds: HashMap<i64, MidnightSeed> = shared.portfolio.midnight_seeds()
            .into_iter().map(|s| (s.con_id, s)).collect();
        let positions = shared.portfolio.position_infos();

        let mut con_ids: HashSet<i64> = seeds.keys().copied().collect();
        for pi in &positions {
            con_ids.insert(pi.con_id);
        }
        // An account holding nothing still has a P&L: what it realised today
        // is already in the venue's figures. Returning here reported
        // nothing at all to a caller that had asked to be told.

        let con_id_map = self.con_id_to_instrument.lock().unwrap();
        let mut total_daily: f64 = 0.0;
        let mut total_unrealized: f64 = 0.0;
        let mut total_realized: f64 = 0.0;
        let mut priced = 0usize;
        let mut unpriceable = 0usize;

        for con_id in con_ids {
            let seed = seeds.get(&con_id);
            let pi = shared.portfolio.position_info(con_id);

            // Realized P&L is stated outright by the row and does not depend on
            // knowing either quantity, so it accrues before the guards below.
            total_realized += seed.map(|s| s.realized_pnl).unwrap_or(0.0);

            // A position held at midnight and not currently sizeable is not
            // a flat one: pricing the absence as zero reports the whole
            // overnight holding as sold. It is counted as unpriceable rather
            // than merely skipped, because a total missing one position is not
            // a smaller correct answer.
            if seed.is_some() && pi.is_none() {
                unpriceable += 1;
                continue;
            }
            let qty_now = pi.as_ref().map(|p| p.position).unwrap_or(0.0);
            let avg_cost = pi.as_ref().map(|p| p.avg_cost).unwrap_or(0);
            // Likewise for the overnight leg: a seed row whose quantity did not
            // parse says the position is not intraday, only that its size is
            // unknown, so there is nothing to price it against.
            let Some(qty_midnight) = seed.map_or(Some(0.0), |s| s.qty_midnight) else {
                unpriceable += 1;
                continue;
            };

            // A price is per unit and a contract may be worth many of them.
            // Multiplied by nothing, an option holding was valued at a
            // hundredth of what it is worth and the account total with it, so
            // such a position is left to the venue's figures rather than
            // valued from a price this arithmetic cannot use.
            let carries_multiplier = pi.as_ref().is_some_and(position_is_multiplied);
            let has_size = qty_now != 0.0 || qty_midnight != 0.0;
            if carries_multiplier && has_size {
                unpriceable += 1;
                continue;
            }

            let quote = con_id_map.get(&con_id).map(|&iid| shared.market.quote(iid));
            let prev_close = quote.map_or(0, |q| q.close);
            let Some(price_now) = quote.map(|q| q.last).filter(|&p| p != 0) else {
                // A position with size and no live price is one this total
                // is missing, which is what `unpriceable` counts. Skipping it
                // without counting reports the rest of the account as the
                // whole of it.
                if has_size {
                    unpriceable += 1;
                }
                continue;
            };
            // What the position was worth at midnight. The venue states the
            // mark it closed the contract at, and that is what the overnight
            // leg is valued against; a locally derived previous close is used
            // only where the venue said nothing.
            let prev_close = Self::midnight_price(shared, con_id).unwrap_or(prev_close);
            if seed.and_then(|s| s.cost_midnight).is_none() && prev_close == 0 && qty_midnight != 0.0 {
                // Nothing to value the overnight leg against, so this position
                // is missing from the total too.
                unpriceable += 1;
                continue;
            }

            // moneyTradedSinceMidnight (wire 6822) is signed net cash: SELL
            // positive, BUY negative. An intraday-only position
            // has no seed row, so synthesize the opening trade's net cash:
            // -qty*avgCost (cash paid to open a long, received to open a short).
            let money_traded = match seed {
                Some(s) => s.money_traded,
                None => -(qty_now * avg_cost as f64 / PRICE_SCALE_F),
            };

            let mv_now = qty_now * price_now as f64 / PRICE_SCALE_F;
            // The venue states what the position was worth at midnight. That
            // beats sizing the overnight leg against a previous close the
            // client has to find for itself, which it has for no contract it
            // never quoted.
            let mv_midnight = seed.and_then(|s| s.cost_midnight)
                .unwrap_or_else(|| qty_midnight * prev_close as f64 / PRICE_SCALE_F);
            // Daily P&L = value change since midnight plus today's net cash.
            total_daily += mv_now - mv_midnight + money_traded;

            if avg_cost != 0 {
                total_unrealized += qty_now * (price_now - avg_cost) as f64 / PRICE_SCALE_F;
            }
            priced += 1;
        }

        // No position carried a live quote (a req_pnl-only client never populates
        // con_id_to_instrument, so every position above hits `continue`). Fall back
        // to the venue's account-level P&L, which the venue pushes independently
        // of any market-data subscription. Without this the quote-derived totals stay
        // [0,0,0] and no callback ever fires.
        // A position that could not be priced makes the client-side sum an
        // incomplete account total, not a smaller correct one — and the realized
        // figure has already accrued for it, so the three would not even agree
        // with each other. The venue's account-level numbers are complete
        // by construction, so one unpriceable position sends the whole account
        // to them rather than reporting a partial sum as if it were the total.
        if priced == 0 || unpriceable > 0 {
            let acct = shared.portfolio.account();
            total_daily = acct.daily_pnl as f64 / PRICE_SCALE_F;
            total_unrealized = acct.unrealized_pnl as f64 / PRICE_SCALE_F;
            total_realized = acct.realized_pnl as f64 / PRICE_SCALE_F;
        }

        let pnl = [
            crate::types::price_from_f64(total_daily),
            crate::types::price_from_f64(total_unrealized),
            crate::types::price_from_f64(total_realized),
        ];
        let mut last = self.last_pnl.lock().unwrap();
        if pnl == *last {
            return None;
        }
        *last = pnl;
        Some(PnlUpdate {
            req_id,
            daily_pnl: total_daily,
            unrealized_pnl: total_unrealized,
            realized_pnl: total_realized,
        })
    }

    /// Poll per-position PnL and return updates whose values changed.
    /// Routes the quote lookup by con_id (not first-non-zero across all subscriptions),
    /// computes daily/realized from the matching midnight seed, and synthesizes
    /// money_traded = qty_now × avg_cost for intraday-opened positions.
    pub fn poll_pnl_single(&self, shared: &SharedState) -> Vec<PnlSingleUpdate> {
        let reqs: Vec<(i64, i64)> = self.pnl_single_reqs.lock().unwrap()
            .iter().map(|(&r, &c)| (r, c)).collect();
        if reqs.is_empty() {
            return Vec::new();
        }

        let seeds: HashMap<i64, MidnightSeed> = shared.portfolio.midnight_seeds()
            .into_iter().map(|s| (s.con_id, s)).collect();
        let con_id_map = self.con_id_to_instrument.lock().unwrap();
        let mut last_cache = self.last_pnl_single.lock().unwrap();
        let mut results = Vec::new();

        for (req_id, con_id) in reqs {
            let Some(pi) = shared.portfolio.position_info(con_id) else { continue; };
            let qty_now = pi.position;
            let avg_cost = pi.avg_cost;

            let quote = con_id_map.get(&con_id).map(|&iid| shared.market.quote(iid));
            // The venue's mark for the position, stated whether or not
            // anything here subscribed to the contract. A quote is per unit
            // and a contract may be worth many of them, so an option or a
            // future is valued from the venue's figure rather than from a
            // price this arithmetic cannot use. This subscription does not
            // depend on a market-data subscription.
            let stated_mark = (pi.market_price != 0).then_some(pi.market_price);
            let price_now = match quote.map(|q| q.last).filter(|&p| p != 0) {
                Some(live) if !position_is_multiplied(&pi) => live,
                _ => match stated_mark {
                    Some(mark) => mark,
                    None => continue,
                },
            };
            let unit_value = if pi.market_value != 0 {
                pi.market_value as f64 / PRICE_SCALE_F
            } else if qty_now != 0.0 && position_is_multiplied(&pi) {
                // The venue states a value that already carries the contract's
                // multiplier and a price that does not. With no value stated,
                // multiplying the quantity by the price alone values a
                // multiplied contract at a hundredth of what it is worth — and
                // that is the branch an update carrying only the price takes,
                // because an absent value reads as zero. Both neighbours here
                // test for this; this one did not.
                //
                // Only where something is held. A position closed today keeps
                // its row, and its value really is nothing — skipped for
                // carrying a multiplier, the caller would never hear the close
                // or the realized figure that came with it.
                continue;
            } else {
                qty_now * price_now as f64 / PRICE_SCALE_F
            };
            let seed = seeds.get(&con_id);
            // An unparseable overnight quantity leaves nothing to price the
            // day's change against. Unlike the whole-account total, that costs
            // this callback only its daily figure: the position, its value, the
            // unrealized and the realized are all still known, and suppressing
            // the callback would leave every one of them stale on the caller's
            // side rather than reporting one it cannot compute.
            let qty_midnight = seed.map_or(Some(0.0), |s| s.qty_midnight);
            // As in poll_pnl, the venue's mark is what the overnight leg is
            // valued against, so the two callbacks value the same position from
            // the same figures.
            let prev_close = Self::midnight_price(shared, con_id)
                .unwrap_or_else(|| quote.map_or(0, |q| q.close));
            // What the venue says the position was worth at midnight, which the
            // client otherwise has to size from the overnight quantity and a
            // previous close it may not hold.
            let stated_midnight = seed.and_then(|s| s.cost_midnight);
            if stated_midnight.is_none() && prev_close == 0 && qty_midnight.unwrap_or(0.0) != 0.0 {
                continue;
            }

            // moneyTradedSinceMidnight (wire 6822) is signed net cash: SELL
            // positive, BUY negative. Synthesize the opening
            // trade's net cash for an intraday-only position (no seed row).
            let money_traded = match seed {
                Some(s) => s.money_traded,
                None => -(qty_now * avg_cost as f64 / PRICE_SCALE_F),
            };

            let mv_now = unit_value;
            // Held at the value last reported when the overnight size is
            // unknown, rather than recomputed from an assumption that would be
            // wrong in a specific direction: treating the absence as flat
            // reports the whole holding as sold, and treating it as no seed at
            // all reports the day's move as the position's entire unrealized.
            let midnight_value = stated_midnight
                .or_else(|| qty_midnight.map(|q| q * prev_close as f64 / PRICE_SCALE_F));
            let daily = match midnight_value {
                Some(mv_midnight) => mv_now - mv_midnight + money_traded,
                None => last_cache.get(&req_id)
                    .map_or(0.0, |prev| prev[1] as f64 / PRICE_SCALE_F),
            };
            // The venue states what the position has made and not realised,
            // and it is the only figure that is right for a contract worth
            // more than one unit of its own price.
            let unrealized = if pi.unrealized_pnl != 0 {
                pi.unrealized_pnl as f64 / PRICE_SCALE_F
            } else if avg_cost != 0 && !position_is_multiplied(&pi) {
                qty_now * (price_now - avg_cost) as f64 / PRICE_SCALE_F
            } else { 0.0 };
            let realized = seed.map(|s| s.realized_pnl).unwrap_or(0.0);
            let value = mv_now;

            let snapshot: [i64; 5] = [
                qty_now as i64,
                crate::types::price_from_f64(daily),
                crate::types::price_from_f64(unrealized),
                crate::types::price_from_f64(realized),
                crate::types::price_from_f64(value),
            ];
            if last_cache.get(&req_id) == Some(&snapshot) {
                continue;
            }
            last_cache.insert(req_id, snapshot);

            results.push(PnlSingleUpdate {
                req_id,
                pos: qty_now,
                daily_pnl: daily,
                unrealized_pnl: unrealized,
                realized_pnl: realized,
                value,
            });
        }
        results
    }

    /// The account figures to deliver, as the venue stated them.
    ///
    /// Built from the venue's statements rather than from this client's
    /// typed copy, which exists before the venue has stated anything and would
    /// otherwise report every figure as zero in no currency.
    ///
    /// Each figure is delivered once and again whenever it changes, per
    /// currency: a figure stated in two currencies is two figures.
    pub fn prepare_account_updates(&self, shared: &SharedState) -> Option<AccountUpdateBatch> {
        if !self.account_updates_subscribed.load(Ordering::Acquire) {
            return None;
        }
        let stated = shared.portfolio.stated_account_values();
        if stated.is_empty() {
            return None;
        }

        let mut already = self.last_stated_account.lock().unwrap();
        let mut fields = Vec::new();
        for (key, value, currency) in stated {
            let held = already.get(&(key.clone(), currency.clone()));
            if held.map(String::as_str) == Some(value.as_str()) {
                continue;
            }
            already.insert((key.clone(), currency.clone()), value.clone());
            fields.push(AccountFieldUpdate { key, value, currency });
        }

        let delivered = !fields.is_empty();
        // Said once, when the venue ends the batch it was sending. Timed out
        // of a quiet spell instead, an account still arriving was called fully
        // stated because it paused, and one that finished early waited on a
        // clock for permission to say so.
        let finished = shared.portfolio.account_download_complete()
            && !self.account_end_sent.swap(true, Ordering::AcqRel);
        Some(AccountUpdateBatch { fields, delivered, finished })
    }

    /// Prepare portfolio updates (position entries) for account streaming.
    /// Returns changed/new position infos when account updates are subscribed.
    pub fn prepare_portfolio_updates(&self, shared: &SharedState) -> Vec<PortfolioUpdateEntry> {
        if !self.account_updates_subscribed.load(Ordering::Acquire) {
            return Vec::new();
        }
        if !shared.portfolio.account_data_received() {
            return Vec::new();
        }

        let current = shared.portfolio.position_infos();
        let mut prev_guard = self.last_portfolio.lock().unwrap();
        let is_first = prev_guard.is_none();

        let to_entry = |pi: &PositionInfo| PortfolioUpdateEntry {
            con_id: pi.con_id,
            position: pi.position,
            avg_cost: pi.avg_cost as f64 / PRICE_SCALE_F,
            market_price: pi.market_price as f64 / PRICE_SCALE_F,
            market_value: pi.market_value as f64 / PRICE_SCALE_F,
            unrealized_pnl: pi.unrealized_pnl as f64 / PRICE_SCALE_F,
            realized_pnl: pi.realized_pnl as f64 / PRICE_SCALE_F,
        };

        let changed = if is_first {
            current.iter().map(&to_entry).collect()
        } else {
            let prev = prev_guard.as_ref().unwrap();
            // Marks are part of the row: a mark move (each account-updates
            // snapshot) is a genuine update, so compare them too.
            current.iter().filter(|pi| {
                !prev.iter().any(|pp| pp.con_id == pi.con_id
                    && pp.position == pi.position
                    && pp.avg_cost == pi.avg_cost
                    && pp.market_price == pi.market_price
                    && pp.market_value == pi.market_value
                    && pp.unrealized_pnl == pi.unrealized_pnl
                    && pp.realized_pnl == pi.realized_pnl)
            }).map(&to_entry).collect()
        };

        *prev_guard = Some(current);
        changed
    }

    /// Prepare account summary response (one-shot, consumes the request).
    pub fn prepare_account_summary(&self, shared: &SharedState, _account_id: &str) -> Option<AccountSummaryBatch> {
        // Wait for gateway account data before delivering summary.
        if !shared.portfolio.account_data_received() {
            return None;
        }
        let req = self.account_summary_req.lock().unwrap().take();
        let (req_id, tags) = req?;

        // What the venue said, in the currency it said it in. Built from this
        // client's typed copy instead, an account held in a currency the venue
        // states its figures in came back as zero: the copy is filled from the
        // rows the venue sends, and a summary asked for before they arrive
        // reported an empty account rather than nothing.
        let stated = shared.portfolio.stated_account_values();
        // "All" is the venue's word for every figure it holds, and a request
        // naming no tag at all means the same. Matched against a local list of
        // names instead, "All" matches none of them and returns empty, and any
        // figure absent from that list is dropped with it: accrued cash, SMA,
        // look-ahead margin, per-currency ledger rows.
        let wants_all = tags.is_empty() || tags.iter().any(|t| t == "All");
        let entries = stated
            .iter()
            .filter(|(key, ..)| wants_all || tags.iter().any(|t| t == key))
            .map(|(key, value, currency)| AccountSummaryEntry {
                tag: key.clone(),
                value: value.clone(),
                currency: currency.clone(),
            })
            .collect();

        Some(AccountSummaryBatch { req_id, entries })
    }

    // ── Order routing ──

    /// Pre-validate order fields that don't depend on instrument ID.
    /// Call this before `find_or_register_instrument` to fail fast.
    pub fn validate_order(order: &ApiOrder, connected_account: &str) -> Result<(), String> {
        order.side()?;

        // An execution condition names a symbol, an exchange and a security
        // type, and the venue wants all three. Left short, it accepts the
        // order and holds it Inactive with "Invalid value in field # 6246",
        // which names a tag no caller of this client has heard of.
        for condition in &order.conditions {
            if let crate::types::OrderCondition::Execution { symbol, exchange, sec_type } = condition {
                for (what, value) in
                    [("symbol", symbol), ("exchange", exchange), ("security type", sec_type)]
                {
                    if value.trim().is_empty() {
                        return Err(format!(
                            "an execution condition needs a {what}; the venue refuses one \
                             that leaves any of symbol, exchange or security type out",
                        ));
                    }
                }
            }
        }

        // Reject non-finite and out-of-range numerics up front, before any
        // caller-visible order gets built from a NaN, an Infinity, or a
        // magnitude the wire's fixed-point i64 can't hold.
        require_finite_price("lmt_price", order.lmt_price)?;
        require_finite_price("aux_price", order.aux_price)?;
        require_finite_price("discretionary_amt", order.discretionary_amt)?;
        require_finite_price("cash_qty", order.cash_qty)?;
        require_finite_price("trigger_price", order.trigger_price)?;
        require_finite_price("adjusted_stop_price", order.adjusted_stop_price)?;
        require_finite_price("adjusted_stop_limit_price", order.adjusted_stop_limit_price)?;
        // f64::MAX is the sentinel for "not set" on these three; any other
        // value must be finite and representable.
        if order.trail_stop_price != f64::MAX {
            require_finite_price("trail_stop_price", order.trail_stop_price)?;
        }
        if order.lmt_price_offset != f64::MAX {
            require_finite_price("lmt_price_offset", order.lmt_price_offset)?;
        }
        if order.adjusted_trailing_amount != f64::MAX {
            require_finite_price("adjusted_trailing_amount", order.adjusted_trailing_amount)?;
        }
        // Every other field a saturating cast turns into a different,
        // valid-looking number on its way to the wire. Guarding only the
        // handful above lets a NaN ladder step reach the venue as an increment
        // of zero, and a benchmark reference or hedging leg stated as an
        // infinity reach it as the largest price there is.
        for (field, value) in [
            ("scale_price_increment", order.scale_price_increment),
            ("scale_profit_offset", order.scale_profit_offset),
            ("scale_price_adjust_value", order.scale_price_adjust_value),
            ("delta_neutral_aux_price", order.delta_neutral_aux_price),
            ("pegged_change_amount", order.pegged_change_amount),
            ("reference_change_amount", order.reference_change_amount),
            ("starting_price", order.starting_price),
            ("stock_ref_price", order.stock_ref_price),
            ("stock_range_lower", order.stock_range_lower),
            ("stock_range_upper", order.stock_range_upper),
            ("percent_offset", order.percent_offset),
            ("volatility", order.volatility),
        ] {
            // f64::MAX is this API's "not set" and states nothing.
            if value != f64::MAX {
                require_finite_price(field, value)?;
            }
        }
        for (at, leg) in order.order_combo_legs.iter().enumerate() {
            if *leg != f64::MAX {
                require_finite_price(&format!("order_combo_legs[{at}]"), *leg)?;
            }
        }
        if !order.trailing_percent.is_finite()
            || order.trailing_percent < 0.0
            || order.trailing_percent * 100.0 > u32::MAX as f64
        {
            return Err(format!(
                "trailing_percent must be a finite, non-negative number, got {}",
                order.trailing_percent
            ));
        }
        // The quantity reaches the wire as the decimal it was given, so a
        // fraction of a share is carried rather than refused. The bound is
        // where the fixed-point conversion stops being exact: past it the
        // low digits are lost, and the order goes out for a size nobody
        // asked for rather than being refused.
        if !order.total_quantity.is_finite() {
            return Err("total_quantity must be a finite number".to_string());
        }
        if order.total_quantity < 0.0 {
            return Err(format!("total_quantity {} is negative", order.total_quantity));
        }
        if order.total_quantity > crate::types::MAX_EXACT_QTY_SHARES {
            return Err(format!("total_quantity {} is too large", order.total_quantity));
        }
        // A cash-quantity order legitimately carries no shares — the size is
        // stated in currency instead — so zero is only wrong when nothing else
        // says how much to buy.
        if order.total_quantity == 0.0 && order.cash_qty <= 0.0 {
            return Err("total_quantity is zero and no cash_qty was supplied".to_string());
        }
        if order.parent_id < 0 {
            return Err(format!("parent_id must not be negative, got {}", order.parent_id));
        }

        // A time in force this client does not know becomes DAY, and a DAY
        // order dies at the close. A caller who wrote "gtc" and meant GTC gets
        // an order that quietly stops existing, so the spelling is checked
        // rather than fallen back on.
        const TIME_IN_FORCE: [&str; 10] =
            ["DAY", "GTC", "IOC", "FOK", "OPG", "GTD", "GTX", "DTC", "AUC", "NMIN"];
        if !order.tif.is_empty() && !TIME_IN_FORCE.contains(&order.tif.as_str()) {
            return Err(format!(
                "tif '{}' is not one this venue carries. It is one of {}, \
                 spelled exactly — an unrecognised value would otherwise be \
                 sent as DAY and expire at the close.",
                order.tif,
                TIME_IN_FORCE.join(", "),
            ));
        }

        // An expiry this client cannot read used to be logged and dropped, and
        // the order then went out with no expiry at all — which for a GTC-until
        // order is a different order from the one asked for.
        if !order.good_till_date.is_empty()
            && let Err(e) = crate::protocol::datetime::parse_ib_expiry(&order.good_till_date)
        {
            return Err(format!(
                "good_till_date '{}' cannot be read: {e}. State it as \
                 `yyyyMMdd HH:mm:ss` with an optional zone, or `yyyyMMdd` for \
                 a date — sent unread, the order would carry no expiry.",
                order.good_till_date,
            ));
        }

        // Everything this protocol has no field for. Each is documented on the
        // field with what is known about the absence; each is refused here,
        // because a caller that set one and was answered anyway would have an
        // order the venue never saw the instruction on, and nothing to say so.
        //
        // Compared against the default rather than against emptiness: a field
        // left alone is not a field asked for, and only what a caller stated
        // is refused. The list is checked against the documented one by
        // `scripts/gen_order_field_reach.py`, so a field that gains or loses
        // its note here cannot drift from the note itself.
        static UNCARRIED: LazyLock<ApiOrder> = LazyLock::new(ApiOrder::default);
        macro_rules! refuse_if_stated {
            // A field with something of its own to say says it. The rest share
            // the sentence below, and both are one list, so the registry the
            // reach check reads stays whole.
            ($($field:ident $(: $why:expr)?),+ $(,)?) => {
                $(if order.$field != UNCARRIED.$field {
                    let stated: Option<&str> = None $(.or(Some($why)))?;
                    return Err(match stated {
                        Some(why) => why.to_string(),
                        None => format!(
                            "{} is not carried by this protocol: there is no \
                             field to send it under, so the order would go out \
                             without it and do something other than what was \
                             asked. It is documented on the field with what is \
                             known about the absence. Leave it at its default \
                             to place the order without it.",
                            stringify!($field),
                        ),
                    });
                })+
            };
        }
        refuse_if_stated!(
            algo_id, auction_strategy, basis_points, basis_points_type,
            bond_accrued_interest, delta_neutral_clearing_account,
            delta_neutral_clearing_intent, delta_neutral_designated_location,
            delta_neutral_open_close, delta_neutral_settling_firm,
            delta_neutral_short_sale, delta_neutral_short_sale_slot,
            delta, dont_use_auto_price_for_hedge, model_code, opt_out_smart_routing,
            fa_group: "FA allocation is not supported: fa_group, fa_method and \
                       fa_percentage are not carried on the order, so the full \
                       quantity would fill on the connected account instead of \
                       being allocated across the advisor group.",
            fa_method: "FA allocation is not supported: fa_method is not \
                        carried on the order, so the full quantity would fill \
                        on the connected account under no method at all.",
            fa_percentage: "FA allocation is not supported: fa_percentage is \
                            not carried on the order, so the full quantity \
                            would fill on the connected account rather than \
                            the share stated.",
            transmit: "transmit=false is not supported: orders are transmitted \
                       immediately on place_order; there is no staging concept, \
                       so the order would go live despite transmit=false. Place \
                       child orders with parent_id/oca_group set and keep \
                       transmit=true (the engine links them server-side).",
            order_misc_options, origin, override_percentage_constraints,
            parent_perm_id, pt_order_id, pt_order_type, randomize_price,
            scale_init_fill_qty, scale_table, shareholder, sl_order_id,
            sl_order_type, smart_combo_routing_params,
            soft_dollar_tier_display_name, what_if_type,
        );


        // Held until a stated moment. Unreadable, the delay used to be dropped
        // and the order filled at once, which is the opposite of what was asked.
        if !order.good_after_time.is_empty()
            && let Err(e) = crate::protocol::datetime::parse_ib_expiry(&order.good_after_time)
        {
            return Err(format!(
                "good_after_time '{}' cannot be read: {e}. State it as \
                 `yyyyMMdd HH:mm:ss` with an optional zone, or `yyyyMMdd` for \
                 the start of a day — sent unread, the order would go live at \
                 once instead of waiting.",
                order.good_after_time,
            ));
        }

        // A quantity or a slot stated as a negative number is not a smaller
        // one, it is a mistake. The conversion clamps it, so the order goes out
        // asking for none of whatever was asked for — an iceberg with no
        // display size, a minimum of nothing, a leg that borrows from nowhere.
        for (what, stated) in [
            ("display_size", i64::from(order.display_size)),
            ("min_qty", i64::from(order.min_qty)),
            ("scale_init_level_size", i64::from(order.scale_init_level_size)),
            ("scale_subs_level_size", i64::from(order.scale_subs_level_size)),
            ("scale_price_adjust_interval", i64::from(order.scale_price_adjust_interval)),
        ] {
            if stated < 0 {
                return Err(format!(
                    "{what} is {stated}, which is not a quantity. Sent, it goes \
                     out as none at all and the order does something other than \
                     what was asked.",
                ));
            }
        }
        // These two go out in a single byte, so a value above it is not a
        // larger one — it arrives as whatever fits.
        for (what, stated) in [
            ("volatility_type", order.volatility_type),
            ("short_sale_slot", order.short_sale_slot),
        ] {
            if !(0..=255).contains(&stated) {
                return Err(format!(
                    "{what} is {stated}, which does not fit the field it goes \
                     out in. Sent, it would arrive as a different value.",
                ));
            }
        }
        // A cash quantity is what to spend, so a negative one buys nothing: the
        // encoder omits it and the order goes out sized by its quantity alone.
        if order.cash_qty < 0.0 {
            return Err(format!(
                "cash_qty is {}, which is not an amount to spend. Sent, it is \
                 omitted and the order goes out sized by its quantity instead.",
                order.cash_qty,
            ));
        }
        // Both halves or neither: a tier named with nothing against it is not
        // an arrangement, and the encoder writes neither part rather than half.
        if order.soft_dollar_tier_name.is_empty() != order.soft_dollar_tier_val.is_empty() {
            return Err(
                "a soft-dollar arrangement is a tier and what it is worth. \
                 Stated with one of the two, neither goes out and the \
                 commission goes wherever the account's default sends it."
                    .to_string(),
            );
        }
        // These carry a sentinel for "not stated", so only a value that is
        // neither the sentinel nor a quantity is a mistake.
        for (what, stated) in [
            ("min_trade_qty", order.min_trade_qty),
            ("post_to_ats", order.post_to_ats),
            ("min_compete_size", order.min_compete_size),
        ] {
            if stated != i32::MAX && stated < 0 {
                return Err(format!(
                    "{what} is {stated}, which is not a quantity. Sent, it goes \
                     out as none at all. Leave it at its default to state none.",
                ));
            }
        }

        // Two fields the conversion narrows to a set, turning anything else
        // into the default. An unknown value reaching the venue is narrowed
        // there the same way, but a value outside the set is a caller's
        // mistake and is reported rather than silently changed.
        if !matches!(order.trigger_method, 0..=4 | 7 | 8) {
            return Err(format!(
                "trigger_method {} is not one this venue carries. It is 0 to 4, \
                 7 or 8 — anything else becomes 0, which is the default trigger \
                 and not the one asked for.",
                order.trigger_method,
            ));
        }
        if order.oca_type != 0 && !matches!(order.oca_type, 1..=4) {
            return Err(format!(
                "oca_type {} is not one this venue carries. It is 1 to 4, or 0 \
                 to leave it unset — anything else is sent as unset and the \
                 group cancels under the venue's default rather than the rule \
                 asked for.",
                order.oca_type,
            ));
        }

        // A hedge is stated as a kind and a parameter that goes with it. A
        // kind this client does not know reads as no hedge at all, and the
        // order goes out unhedged; a beta or a ratio it cannot read becomes
        // zero, which is omitted, and the order goes out hedged against
        // nothing. Both are a different order from the one asked for.
        const HEDGE: [&str; 5] = ["D", "B", "F", "P", "S"];
        if !order.hedge_type.is_empty() {
            let kind = order.hedge_type.to_ascii_uppercase();
            if !HEDGE.contains(&kind.as_str()) {
                return Err(format!(
                    "hedge_type '{}' is not one this venue carries. It is one of \
                     {} — delta, beta, FX, pair, or the venue's pair. An \
                     unrecognised kind would otherwise be dropped and the order \
                     sent unhedged.",
                    order.hedge_type,
                    HEDGE.join(", "),
                ));
            }
            // Only these two kinds are struck at a number. Delta and FX take
            // no parameter, so one stated with them is not read.
            if matches!(kind.as_str(), "B" | "P")
                && order.hedge_param.parse::<f64>().is_err()
            {
                return Err(format!(
                    "hedge_type '{}' is struck at a number and hedge_param \
                     '{}' is not one. Stated unreadable, the hedge would be \
                     sent as zero and dropped, leaving the order hedged \
                     against nothing.",
                    order.hedge_type, order.hedge_param,
                ));
            }
        }

        // Financial-advisor allocation is not wire-encoded, so an accepted
        // fa_group would put the whole size on the connected account rather
        // than spread it across the group, with nothing to show for it.
        // Same class as the FA fields below, and sharper: no encoder reads
        // `order.account` — every order carries tag 1 from the session account —
        // so the quantity fills on the connected account. The echo then confirms
        // the wrong answer, because the open-order snapshot backfills the account
        // from the report only when the caller left it blank. An FA order at
        // least errors; this one filled elsewhere and reported success.
        if !order.account.is_empty() && order.account != connected_account {
            return Err(format!(
                "order.account {:?} is not carried on the order: the quantity \
                 would fill on the connected account {:?} instead, and the \
                 open-order snapshot would still report {:?}",
                order.account, connected_account, order.account,
            ));
        }


        let order_type = order.order_type.to_uppercase();

        // An order carrying an algorithm is encoded as a limit and nothing
        // else: the strategy rides on an order whose type byte is written
        // once, as `2`. The caller's own type must therefore be a limit.
        // Accepting another type and encoding a limit anyway sends an order
        // the caller did not describe — `MKT` with an algorithm becomes a
        // limit at whatever `lmt_price` holds.
        if !order.algo_strategy.is_empty() {
            if order.algo_strategy.eq_ignore_ascii_case("Adaptive") {
                adaptive_priority(&order.algo_params)?;
            } else {
                crate::client_core::parse_algo_params(&order.algo_strategy, &order.algo_params)?;
            }
            if !order_type.is_empty() && order_type != "LMT" {
                return Err(format!(
                    "algo_strategy '{}' is carried on a limit order, and this one \
                     states order_type '{}'. Sent as it stands the venue would \
                     receive a limit at {}, which is not the order described.",
                    order.algo_strategy, order.order_type, order.lmt_price,
                ));
            }
            return Ok(());
        }
        // A preview asks about an order this client could send, so it answers
        // for the same set of types. Returning before this match sends an
        // unknown type to the wire as a limit, and the venue answers about an
        // order the caller did not ask about.
        match order_type.as_str() {
            "MKT" | "LMT" | "STP" | "STP LMT" | "TRAIL" | "TRAIL LIMIT"
            | "MOC" | "LOC" | "MIT" | "LIT" | "MTL" | "MKT PRT" | "STP PRT"
            | "REL" | "PEG MKT" | "PEG MID" | "PEG MIDPT" | "MIDPX" | "MIDPRICE"
            | "SNAP MKT" | "SNAP MID" | "SNAP MIDPT" | "SNAP PRI" | "SNAP PRIM"
            | "PEG BENCH" | "PEGBENCH" | "BOX TOP" => {}
            _ => return Err(format!("Unsupported order type: '{}'", order.order_type)),
        }
        if order.what_if {
            return Ok(());
        }

        // Reject orders that require aux_price when it is zero — prevents silent no-
        // trigger bugs.
        match order_type.as_str() {
            "STP" | "STP PRT" | "MIT" if order.aux_price == 0.0 => {
                return Err(format!(
                    "{} order requires aux_price (stop/trigger price) but got 0.0 — \
                     set aux_price to the desired trigger price, not lmt_price",
                    order.order_type
                ));
            }
            "STP LMT" | "LIT" if order.aux_price == 0.0 => {
                return Err(format!(
                    "{} order requires aux_price (stop/trigger price) but got 0.0",
                    order.order_type
                ));
            }
            "TRAIL" if order.trailing_percent == 0.0 && order.aux_price == 0.0 => {
                return Err(
                    "TRAIL order requires either trailing_percent or aux_price (trail amount) \
                     but both are 0.0".into()
                );
            }
            "TRAIL LIMIT" if order.aux_price == 0.0 => {
                return Err(
                    "TRAIL LIMIT order requires aux_price (trail amount) but got 0.0".into()
                );
            }
            _ => {}
        }

        Ok(())
    }

    /// Remember how a request asked for its bar times to be written.
    ///
    /// The reference client numbers the two forms: 1 for the venue's
    /// spelling, 2 for seconds since the epoch. Anything else is 1, which is
    /// what that client does with a number it does not know.
    pub fn note_date_format(&self, req_id: i64, format_date: i32) {
        if format_date == 2 {
            self.epoch_dates_by_req.lock().unwrap().insert(req_id);
        } else {
            self.epoch_dates_by_req.lock().unwrap().remove(&req_id);
        }
    }

    /// A bar's time, written the way the request that asked for it wanted.
    ///
    /// Where the stamp cannot be read back to an instant it is handed over as
    /// it came: a time nobody can parse is still what the venue said, and
    /// replacing it with a zero would state an instant in 1970.
    pub fn bar_time_for(&self, req_id: i64, stated: &str) -> String {
        if !self.epoch_dates_by_req.lock().unwrap().contains(&req_id) {
            return stated.to_string();
        }
        crate::protocol::datetime::ib_datetime_to_unix(stated)
            .map(|secs| secs.to_string())
            .unwrap_or_else(|| stated.to_string())
    }

    /// Validate historical-request arguments before anything reaches the
    /// engine: an unrecognized bar_size falls back to 5-minute bars
    /// silently through two divergent tables, and an unrecognized
    /// what_to_show falls back to TRADES. The caller is answered with a
    /// synchronous Err at the call instead of plausible, wrong candles.
    pub fn validate_historical_args(
        bar_size: &str,
        what_to_show: &str,
        keep_up_to_date: bool,
    ) -> Result<(), String> {
        let bs = crate::control::historical::BarSize::from_api_str(bar_size)?;
        crate::control::historical::BarDataType::from_api_str(what_to_show)?;
        if keep_up_to_date && !bs.supports_keep_up_to_date() {
            return Err(format!(
                "bar_size '{bar_size}' is not supported with keep_up_to_date=true: \
                 supported sizes are 1 secs, 5 secs, 5 mins, 1 hour, 1 day",
            ));
        }
        Ok(())
    }

    /// What an order states that this client cannot carry out as stated.
    ///
    /// Two cases, and both would otherwise go out meaning something the caller
    /// did not ask for: a delta-neutral order naming no order type for its
    /// hedging leg, which describes nothing to place, and a hedge parameter
    /// given to a hedge type that takes none.
    ///
    /// Everything else an order can state is encoded. This list was once much
    /// longer — volatility, scale, short-sale slots and the rest were refused
    /// here because no encoder carried them. They are carried now.
    pub fn validate_supported_instructions(o: &ApiOrder) -> Result<(), String> {
        let mut unsent: Vec<&str> = Vec::new();
        // A hedging leg with no order type describes nothing to place.
        if o.delta_neutral_order_type.is_empty()
            && (o.delta_neutral_aux_price != f64::MAX || o.delta_neutral_con_id != 0)
        {
            unsent.push("deltaNeutral without deltaNeutralOrderType");
        }
        // A hedge parameter only means something for the kinds that take one.
        if !o.hedge_param.is_empty()
            && !matches!(o.hedge_type.to_ascii_uppercase().as_str(), "B" | "P")
        {
            unsent.push("hedgeParam for a hedge type that takes none");
        }
        if unsent.is_empty() {
            return Ok(());
        }
        Err(format!(
            "this order sets {}, which is not sent — the order placed would be a \
             different one from the order asked for. Remove it, or place the trade \
             it describes directly.",
            unsent.join(", "),
        ))
    }

    /// A combination states its legs on the order, so an order for one is
    /// placeable. What is refused is a combination that names none: the venue
    /// would be given a security type with nothing to build from.
    pub fn validate_combo_legs(sec_type: &str, leg_count: usize) -> Result<(), String> {
        let names_a_combination = sec_type.eq_ignore_ascii_case("BAG")
            || sec_type.eq_ignore_ascii_case("COMBO");
        if leg_count > 0 || !names_a_combination {
            return Ok(());
        }
        Err("a combination order has no legs: state them on the contract, \
             or use the security type of the thing you mean to trade".to_string())
    }

    /// What each leg of a combination states, before any of it is converted.
    ///
    /// The conversion takes each leg as it finds it: a side it does not
    /// recognise becomes a buy, a negative ratio becomes none, and a slot
    /// outside a byte is clamped to the nearest one that fits. Each of those is
    /// a leg trading the other way, in no size, or borrowing from somewhere
    /// nobody named —
    /// against the rest of a combination that is priced as one thing.
    pub fn validate_leg(at: usize, leg: &crate::types::model::ComboLeg) -> Result<(), String> {
        if !leg.action.eq_ignore_ascii_case("BUY") && !leg.action.eq_ignore_ascii_case("SELL") {
            return Err(format!(
                "leg {at} states side {:?}, which is BUY or SELL. Anything else \
                 is sent as a buy, and the combination trades the wrong way \
                 round on that leg.",
                leg.action,
            ));
        }
        if leg.ratio <= 0 {
            return Err(format!(
                "leg {at} states a ratio of {}, which is not a quantity. Sent, \
                 the leg goes out in no size at all.",
                leg.ratio,
            ));
        }
        for (what, stated) in [("openClose", leg.open_close), ("shortSaleSlot", leg.shorting_policy)] {
            if !(0..=255).contains(&stated) {
                return Err(format!(
                    "leg {at} states {what} as {stated}, which does not fit the \
                     field it goes out in — sent, it would arrive as a \
                     different value.",
                ));
            }
        }
        Ok(())
    }

    /// `con_id` names one contract on its own. Where the caller gave one,
    /// nothing else has to be stated: the venue accepts an order carrying only
    /// the id and the security type, and answers it with a margin preview.
    /// The checks below exist to catch a contract that names a whole chain or
    /// series, which a contract id never does.
    /// Refuse a security type the venue does not permit this account to trade.
    ///
    /// The venue states its permissions at logon and refuses an order on an
    /// unpermitted type by returning it Inactive with no text at all, so
    /// without this the caller is told nothing. Silence here is not
    /// permission: a session that stated none has nothing to enforce.
    ///
    /// Shared, because a guard on one surface and not the other means the
    /// caller it protects depends on which language they wrote in.
    pub fn refuse_unpermitted_sec_type(
        permitted: &std::collections::HashMap<String, Vec<String>>,
        sec_type: &str,
    ) -> Result<(), String> {
        if sec_type.is_empty() || permitted.is_empty() {
            return Ok(());
        }
        let ty = sec_type.to_ascii_uppercase();
        let key = if matches!(ty.as_str(), "BAG" | "COMBO") { "COMB" } else { ty.as_str() };
        if permitted.contains_key(key) {
            return Ok(());
        }
        let mut named: Vec<&str> = permitted.keys().map(String::as_str).collect();
        named.sort_unstable();
        Err(format!(
            "the account is not permitted to trade {ty}. It is permitted: {}",
            named.join(", "),
        ))
    }

    /// An order states where it is to be filled.
    ///
    /// The venue does not choose a destination, and neither does this client:
    /// looking a contract up without one answers with whichever listing came
    /// first, which is how an order reaches a venue the caller never named.
    /// The reference client is refused by the server here, by name.
    pub fn validate_order_destination(exchange: &str) -> Result<(), String> {
        if exchange.trim().is_empty() {
            return Err(
                "an order states the exchange it is to be filled on, and this one \
                 names none".to_string(),
            );
        }
        Ok(())
    }

    /// Refuse an order whose contract does not name one contract:
    /// a symbol alone names a whole option chain.
    pub fn validate_order_contract(con_id: i64, sec_type: &str, identity: &str) -> Result<(), String> {
        if con_id != 0 {
            return Ok(());
        }
        // A currency pair is fully identified by what an order already carries:
        // symbol, currency, security type and destination. There is no expiry,
        // strike, right or multiplier to omit, so the silent mistrade this
        // guard exists to prevent cannot happen for CASH — unlike OPT and FUT,
        // whose orders would go out saying nothing about which strike or which
        // contract month. Verified against a live IDEALPRO book: limit, stop
        // limit, market-if-touched, limit-if-touched, trailing stop limit,
        // relative and hidden all acknowledge and cancel cleanly.
        // A symbol names a stock or a currency pair completely. Anything else
        // needs its expiry, strike, right or multiplier, which an order now
        // restates — so the question is not which type this is but whether the
        // caller said enough to identify one contract.
        let ty = sec_type.to_ascii_uppercase();
        // What identifies one contract differs by kind, and only two kinds need
        // more than the caller has already given.
        //
        // An option and a warrant are one of a chain, so they need the expiry,
        // strike or right that says which one. A future is one of a series and
        // needs its maturity. Everything else is named completely by its symbol
        // and the contract id and local symbol that travel with it, which is
        // how the venue itself names them on an order.
        if matches!(ty.as_str(), "OPT" | "FOP" | "WAR" | "IOPT") {
            if identity.is_empty() {
                return Err(format!(
                    "a {ty} contract needs its expiry, strike or right: the symbol alone \
                     names a whole chain, and an order stating only the symbol would be \
                     filled on whichever contract the gateway picked"
                ));
            }
            return Ok(());
        }
        if matches!(ty.as_str(), "FUT" | "FWD") {
            if identity.is_empty() {
                return Err(format!(
                    "a {ty} contract needs its maturity: the symbol alone names a series, \
                     and an order stating only the symbol would be filled on whichever \
                     contract the gateway picked"
                ));
            }
            return Ok(());
        }
        // A combination states its legs on the order itself, so it needs no
        // identity of its own here. An order that names one and states no legs
        // is refused by `validate_combo_legs` before this.
        Ok(())
    }

    /// What an exercise states, checked before the contract is registered so a
    /// refused one reaches nothing. Returns the action and the quantity the
    /// order carries.
    ///
    /// The documented API names a third action, a hold, which the venue does
    /// not take from a client of this kind. It is refused here rather than sent
    /// and rejected, because the caller who asked for it wants to know that the
    /// position was left alone.
    pub fn validate_exercise(
        exercise_action: i32, exercise_quantity: i32,
        account: &str, connected_account: &str,
    ) -> Result<(u8, u32), String> {
        let action = match exercise_action {
            1 | 2 => exercise_action as u8,
            other => {
                return Err(format!(
                    "exercise_action {other} is not served: 1 exercises, 2 lapses"
                ));
            }
        };
        if exercise_quantity <= 0 {
            return Err(format!(
                "exercise_quantity {exercise_quantity} is not a number of contracts"
            ));
        }
        // Same reason an order's own account field is refused: no encoder reads
        // it, every message carries tag 1 from the session account, so an
        // exercise naming another one would take the position on this account
        // and report the account it was asked for.
        if !account.is_empty() && account != connected_account {
            return Err(format!(
                "account {account:?} is not carried on the order: the exercise \
                 would be taken on the connected account {connected_account:?}"
            ));
        }
        Ok((action, exercise_quantity as u32))
    }

    /// An exercise or a lapse, as the order the venue takes it for: the buy
    /// side, no price, and the action on the attributes so the encoder every
    /// other order goes through emits it.
    ///
    /// The override the documented signature takes is not here. It is a
    /// validation bypass the venue's front end applies while it builds the
    /// order, so no tag carries it and there is nothing to send.
    pub fn build_exercise_request(
        order_id: OrderId, instrument: InstrumentId, action: u8, qty: Qty,
    ) -> OrderRequest {
        OrderRequest::SubmitEx {
            order_id,
            instrument,
            side: Side::Buy,
            qty,
            kind: OrderKind::Limit { price: 0 },
            tif: b'0',
            attrs: OrderAttrs { exercise_action: action, ..Default::default() },
        }
    }

    /// Build an `OrderRequest` from an API `Order`, handling all order types.
    /// This is the shared order-type match block used by both Rust and Python.
    /// A price the caller left alone is `f64::MAX`, which is not a price and
    /// does not survive being scaled into one.
    fn price_or_unset(v: f64) -> i64 {
        if v == f64::MAX { 0 } else { crate::types::price_from_f64(v) }
    }

    /// Turn what a caller set into the request the engine sends.
    pub fn build_order_request(
        order: &ApiOrder,
        order_id: u64,
        instrument: InstrumentId,
        contract: Option<&crate::types::model::Contract>,
    ) -> Result<ControlCommand, String> {
        let side = order.side()?;
        let qty = crate::types::qty_from_f64(order.total_quantity);
        let order_type = order.order_type.to_uppercase();

        // Every order type carries its extended attributes and its time-in-force
        // through one encoder. Choosing per type between an attribute-carrying
        // request and a plain one is how an order type ends up shipping
        // without something the caller set: a bracket child that arrives
        // unlinked and immediate, an adjustable stop that never adjusts, an
        // algo that runs without its parameters.

        // The legs live on the contract, not the order, so they are attached
        // here rather than in `attrs()`.
        // The caller may price the legs separately rather than pricing the
        // combination. The prices are given as their own list, in leg order,
        // so each is put on the leg it belongs to here — kept apart, one would
        // go out against another leg the moment the legs were reordered.
        let leg_prices = order.order_combo_legs.as_slice();
        let leg_specs: Vec<crate::types::ComboLegSpec> =
            contract.map(|c| c.combo_legs.as_slice()).unwrap_or(&[]).iter().enumerate().map(|(at, l)| {
            crate::types::ComboLegSpec {
                con_id: l.con_id,
                ratio: l.ratio.max(0) as u32,
                is_sell: l.action.eq_ignore_ascii_case("SELL"),
                exchange: if l.exchange.eq_ignore_ascii_case("SMART") {
                    String::new()
                } else {
                    l.exchange.clone()
                },
                open_close: l.open_close.clamp(0, 255) as u8,
                short_sale_slot: l.shorting_policy.clamp(0, 255) as u8,
                designated_location: l.designated_location.clone(),
                exempt_code: l.exempt_code,
                price: leg_prices
                    .get(at)
                    .copied()
                    .filter(|p| *p != f64::MAX)
                    .map(crate::types::price_from_f64),
            }
        }).collect();
        let ex = |kind: OrderKind| OrderRequest::SubmitEx {
            order_id, instrument, side, qty,
            kind,
            tif: order.tif_byte(),
            attrs: crate::types::OrderAttrs {
                combo_legs: leg_specs.clone(),
                // The listing exchange and the hedging contract are stated on
                // the contract, not the order, so they are picked up here.
                primary_exchange: contract
                    .map(|c| c.primary_exchange.clone()).unwrap_or_default(),
                delta_neutral_contract: contract
                    .and_then(|c| c.delta_neutral_contract.as_ref())
                    .map(|d| Box::new(crate::types::DeltaNeutralContractSpec {
                        con_id: d.con_id,
                        delta: d.delta,
                        price: d.price,
                    })),
                ..order.attrs()
            },
        };

        // Adaptive orders (special-cased before generic algo)
        if order.algo_strategy.eq_ignore_ascii_case("Adaptive") {
            let price = crate::types::price_from_f64(order.lmt_price);
            let priority = adaptive_priority(&order.algo_params)?;
            return Ok(ControlCommand::Order(ex(OrderKind::Adaptive { price, priority })));
        }

        // Algo orders
        if !order.algo_strategy.is_empty() {
            let algo = crate::client_core::parse_algo_params(&order.algo_strategy, &order.algo_params)?;
            let price = crate::types::price_from_f64(order.lmt_price);
            return Ok(ControlCommand::Order(ex(OrderKind::Algo { price, algo })));
        }

        // What-if orders
        if order.what_if {
            let price = crate::types::price_from_f64(order.lmt_price);
            // A preview states the type of the order being previewed. Sending
            // every preview as a limit made a market-only security answer
            // "The order type Limit is invalid for this combination of
            // exchange and security type" — the venue was refusing an order
            // the caller never asked for.
            //
            // The preview names every type this client sends, which is a wider
            // set than a replace may restate; see `Order::what_if_byte`.
            let ord_type = order.what_if_byte();
            // The price the previewed type triggers at. A stop states it and
            // no limit price at all, so a preview built from the limit price
            // alone asked about a stop at zero.
            let aux = crate::types::price_from_f64(order.aux_price);
            return Ok(ControlCommand::Order(ex(OrderKind::WhatIf { price, aux, ord_type })));
        }

        // Adjustable stop: a base STP that converts to another order type when
        // its trigger is reached. Signalled by a non-empty adjustedOrderType,
        // which is empty on every ordinary order, so this affects nothing else.
        // A Trail/TrailLimit conversion carries the trailing amount + unit
        // (tags 6260/6269).
        if !order.adjusted_order_type.is_empty() {
            let adjusted = match order.adjusted_order_type.to_uppercase().as_str() {
                "STP" => AdjustedOrderType::Stop,
                "STP LMT" => AdjustedOrderType::StopLimit,
                "TRAIL" => AdjustedOrderType::Trail,
                "TRAIL LIMIT" => AdjustedOrderType::TrailLimit,
                other => return Err(format!("unknown adjustedOrderType '{other}'")),
            };
            let scale = |v: f64| crate::types::price_from_f64(v);
            // adjusted_trailing_amount defaults to f64::MAX when unset.
            let adj_trail = if order.adjusted_trailing_amount == f64::MAX {
                0.0
            } else {
                order.adjusted_trailing_amount
            };
            // Through the same construction every other order type uses, so a
            // bracket child keeps its parent link, its OCA group and its tif —
            // and so does everything the contract states rather than the order:
            // its legs, its listing exchange and the contract it hedges
            // against. Built from `order.attrs()` alone, an adjustable stop on
            // a combination reached the encoder with no legs at all.
            return Ok(ControlCommand::Order(ex(OrderKind::AdjustableStop {
                stop_price: scale(order.aux_price),
                trigger_price: scale(order.trigger_price),
                adjusted_order_type: adjusted,
                adjusted_stop_price: scale(order.adjusted_stop_price),
                adjusted_stop_limit_price: scale(order.adjusted_stop_limit_price),
                adjusted_trailing_amount: scale(adj_trail),
                adjustable_trailing_unit: order.adjustable_trailing_unit,
            })));
        }

        let req = match order_type.as_str() {
            "MKT" => {
                ex(OrderKind::Market)
            }
            "LMT" => {
                let price = crate::types::price_from_f64(order.lmt_price);
                ex(OrderKind::Limit { price })
            }
            "STP" => {
                let stop = crate::types::price_from_f64(order.aux_price);
                ex(OrderKind::Stop { stop_price: stop })
            }
            "STP LMT" => {
                let price = crate::types::price_from_f64(order.lmt_price);
                let stop = crate::types::price_from_f64(order.aux_price);
                ex(OrderKind::StopLimit { price, stop_price: stop })
            }
            "TRAIL" => {
                // Optional initial stop trigger (tag 6117); default f64::MAX = unset.
                let trail_stop = if order.trail_stop_price == f64::MAX { 0 } else { crate::types::price_from_f64(order.trail_stop_price) };
                if order.trailing_percent > 0.0 {
                    // Wire granularity is basis points, so a percentage
                    // stated finer than that is put on the nearest one.
                    // Rounded rather than cut: a hundredth of a per cent is
                    // not exactly a double, and 0.29 times a hundred is
                    // 28.999999999999996, so cutting sends 0.28 for a figure
                    // the wire can carry exactly. Five hundred and
                    // seventy-three of the ten thousand basis points went out
                    // a point low. This is the hazard `price_from_f64` names
                    // and rounds for on the price path. validate_order has
                    // already confirmed the value is finite, non-negative and
                    // fits u32 once scaled.
                    let pct = (order.trailing_percent * 100.0).round() as u32;
                    ex(OrderKind::TrailPct { trail_pct: pct, trail_stop_price: trail_stop })
                } else {
                    let trail = crate::types::price_from_f64(order.aux_price);
                    ex(OrderKind::TrailingStop { trail_amt: trail, trail_stop_price: trail_stop })
                }
            }
            "TRAIL LIMIT" => {
                // Wire-side semantic is `LimitPriceOffset` (tag 6370), not an
                // absolute limit price. Prefer `lmt_price_offset`; fall back
                // to `lmt_price` for callers that haven't migrated.
                let offset_f = if order.lmt_price_offset != f64::MAX {
                    order.lmt_price_offset
                } else {
                    order.lmt_price
                };
                let lmt_offset = crate::types::price_from_f64(offset_f);
                let trail = crate::types::price_from_f64(order.aux_price);
                let trail_stop = if order.trail_stop_price == f64::MAX { 0 } else { crate::types::price_from_f64(order.trail_stop_price) };
                ex(OrderKind::TrailingStopLimit { lmt_offset, trail_amt: trail, trail_stop_price: trail_stop })
            }
            "MOC" => {
                ex(OrderKind::Moc)
            }
            "LOC" => {
                let price = crate::types::price_from_f64(order.lmt_price);
                ex(OrderKind::Loc { price })
            }
            "MIT" => {
                let stop = crate::types::price_from_f64(order.aux_price);
                ex(OrderKind::Mit { stop_price: stop })
            }
            "LIT" => {
                let price = crate::types::price_from_f64(order.lmt_price);
                let stop = crate::types::price_from_f64(order.aux_price);
                ex(OrderKind::Lit { price, stop_price: stop })
            }
            "MTL" | "BOX TOP" => {
                ex(OrderKind::Mtl)
            }
            "MKT PRT" => {
                ex(OrderKind::MktPrt)
            }
            "STP PRT" => {
                let stop = crate::types::price_from_f64(order.aux_price);
                ex(OrderKind::StpPrt { stop_price: stop })
            }
            "REL" => {
                let offset = crate::types::price_from_f64(order.aux_price);
                ex(OrderKind::Rel { offset })
            }
            // Every reference field was already carried here and then read by
            // nobody: a caller setting all six got an order that mentioned none
            // of them.
            "PEG BENCH" | "PEGBENCH" => {
                ex(OrderKind::PegBench {
                    price: crate::types::price_from_f64(order.lmt_price),
                    ref_con_id: order.reference_contract_id.max(0) as u32,
                    is_peg_decrease: order.is_pegged_change_amount_decrease,
                    pegged_change_amount: crate::types::price_from_f64(order.pegged_change_amount),
                    ref_change_amount: crate::types::price_from_f64(order.reference_change_amount),
                    starting_price: Self::price_or_unset(order.starting_price),
                    stock_ref_price: Self::price_or_unset(order.stock_ref_price),
                    ref_exchange: order.reference_exchange_id.clone(),
                })
            }
            "PEG MKT" => {
                let offset = crate::types::price_from_f64(order.aux_price);
                let price_cap = crate::types::price_from_f64(order.lmt_price);
                ex(OrderKind::PegMkt { offset, price_cap })
            }
            "PEG MID" | "PEG MIDPT" => {
                let offset = crate::types::price_from_f64(order.aux_price);
                let price_cap = crate::types::price_from_f64(order.lmt_price);
                ex(OrderKind::PegMid { offset, price_cap })
            }
            "MIDPX" | "MIDPRICE" => {
                let cap = crate::types::price_from_f64(order.lmt_price);
                ex(OrderKind::MidPrice { price_cap: cap })
            }
            "SNAP MKT" => {
                let offset = crate::types::price_from_f64(order.aux_price);
                ex(OrderKind::SnapMkt { offset })
            }
            "SNAP MID" | "SNAP MIDPT" => {
                let offset = crate::types::price_from_f64(order.aux_price);
                ex(OrderKind::SnapMid { offset })
            }
            "SNAP PRI" | "SNAP PRIM" => {
                let offset = crate::types::price_from_f64(order.aux_price);
                ex(OrderKind::SnapPri { offset })
            }
            _ => return Err(format!("Unsupported order type: '{}'", order.order_type)),
        };

        Ok(ControlCommand::Order(req))
    }
    /// The contract's terms and the venue's model for it, or why neither
    /// question can be answered.
    pub(crate) fn solve_option(
        &self,
        shared: &SharedState,
        contract: &crate::types::model::Contract,
        solve: impl Fn(
            crate::control::option_model::OptionTerms,
            crate::control::option_model::VenueModel,
        ) -> Option<f64>,
    ) -> Result<f64, crate::error_codes::Refusal> {
        let instrument = self
            .con_id_to_instrument.lock().unwrap().get(&contract.con_id).copied()
            .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?;
        let stated = shared
            .market
            .option_model(instrument)
            .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?;
        // What the venue did not state is not a number. It writes the largest
        // double where it has nothing to say, which this client passes on
        // as-is because the reference client does — so it has to be read back
        // as silence here rather than taken for a value. Taken for one, a
        // contract with no dividend had the largest double in the world
        // subtracted from its underlying.
        let stated_or_none = |v: f64| (v.is_finite() && v != f64::MAX).then_some(v);
        // How long the contract has left, as the venue says it on the same
        // statement as the rest of its model — carried to the fraction, the
        // hours of the last day included. Counted here instead, from whole
        // days off this machine's clock, a contract expiring today has none
        // left and is refused, and so is every contract from the evening
        // before its expiry, because the clock has already turned over in
        // UTC. The venue's own count says 0.73 of a day where this said none.
        //
        // Its basis is 365: the daily rate it states beside this divides the
        // annual one by 365 exactly.
        let years = match stated_or_none(stated.cal_days) {
            Some(days) if days > 0.0 => days / 365.0,
            // Where it stated none, back to counting — a contract with hours
            // left still reads as expired, which is the old behaviour and not
            // worse than refusing outright.
            _ => years_to_expiry(&contract.last_trade_date_or_contract_month)
                .ok_or_else(|| "the contract states no expiry to measure from".to_string())?,
        };
        // The venue states which of two models it priced this contract on, and
        // this library has one of them. Answering a question about the other
        // with this one gives a number, and a number worked out under the
        // wrong distribution is worse than no answer — so it is refused by
        // name rather than quietly produced.
        if stated.price_based_vol {
            return Err(crate::error_codes::Refusal::validation(
                "the venue priced this contract on a volatility stated in its own price \
                 units, which is a different model from the one this client solves with. \
                 Its own figures for the contract are on the model tick and stand as \
                 stated; what cannot be answered is a question about a price or a \
                 volatility other than the ones it published",
            ));
        }
        let terms = crate::control::option_model::OptionTerms {
            strike: contract.strike,
            years_to_expiry: years,
            is_call: contract.right.eq_ignore_ascii_case("C")
                || contract.right.eq_ignore_ascii_case("CALL"),
            // The venue's own calculator tells these apart, and so must this:
            // an option on a future is priced on one that drifts nowhere and
            // settles at expiry.
            on_a_future: contract.sec_type.eq_ignore_ascii_case("FOP"),
        };
        let model = crate::control::option_model::VenueModel {
            volatility: stated_or_none(stated.implied_vol)
                .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?,
            option_price: stated_or_none(stated.opt_price)
                .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?,
            underlying_price: stated_or_none(stated.und_price)
                .ok_or_else(|| OPTION_MODEL_UNSTATED.to_string())?,
            // No dividend stated is no dividend, which is what it means.
            present_value_of_dividends: stated_or_none(stated.pv_dividend).unwrap_or(0.0),
            // No rate stated is no discount, which over the days one of these
            // has left moves the price by less than it is quoted in.
            rate: stated_or_none(stated.rate).unwrap_or(0.0),
        };
        solve(terms, model).ok_or_else(|| {
            crate::error_codes::Refusal::validation(
            "this contract cannot be solved under the venue's model for it. The model is \
             anchored to the price the venue published, so a figure no rate reproduces leaves \
             nothing to solve against — and an option far enough into the money is worth its \
             intrinsic value and little else, its price hardly moving with volatility at all, \
             so no one volatility is implied either. Naming a number for either would be \
             picking one rather than solving for it")
        })
    }

}

#[cfg(test)]
mod tests;

// ── Opening a session ──
//
// Both surfaces open a session the same way and then hold what comes back
// differently. What they do identically is written once here.

/// Remember this session, so the next start does not need a second factor.
///
/// Best effort: a session that cannot be written is a slower start next time,
/// never a failed connect now. Called with the file the caller named, and does
/// nothing when they named none.
pub fn remember_session(
    file: Option<&std::path::Path>,
    password: &str,
    gateway: &crate::gateway::Gateway,
    username: &str,
    paper: bool,
) -> crate::auth::resume::ResumableSession {
    let session = crate::auth::resume::ResumableSession {
        token: crate::auth::crypto::strip_leading_zeros(
            &gateway.session_token.to_bytes_be(),
        ).to_vec(),
        server_session_id: gateway.server_session_id.clone(),
        hw_info: gateway.hw_info.clone(),
        encoded: gateway.encoded.clone(),
        username: username.to_string(),
        paper,
    };
    if let Some(path) = file
        && let Err(e) = crate::auth::resume::save(path, password, &session)
    {
        log::warn!("session not saved to {}: {e}", path.display());
    }
    session
}
