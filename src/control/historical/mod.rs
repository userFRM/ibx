//! Historical data queries via the data connection.
//!
//! Responses contain XML ResultSetBar with OHLCV bar data.

use crate::control::xml::tag;
use crate::protocol::fix;

// Tags for historical data
/// FIX tag 6118: the historical xml.
pub const TAG_HISTORICAL_XML: u32 = 6118;

/// Bar data types for historical queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarDataType {
    /// What traded.
    Trades,
    /// The midpoint between bid and ask.
    Midpoint,
    /// The bid.
    Bid,
    /// The ask.
    Ask,
    /// Both sides.
    BidAsk,
    /// Trades, aggregated as the venue aggregates them.
    AggTrades,
    /// The rate charged to borrow the stock.
    FeeRate,
    /// The yield at the bid.
    YieldBid,
    /// The yield at the ask.
    YieldAsk,
    /// The yield at the last trade.
    YieldLast,
    /// The yield at the venue's own mark.
    YieldMark,
    /// A fund's net asset value.
    NavLast,
    /// The volatility the underlying realised.
    HistoricalVolatility,
    /// The volatility its options implied.
    ImpliedVolatility,
    /// The rate the venue's option model carries.
    OptionInterestRate,
}

impl BarDataType {
    /// Read the official API's `what_to_show` string.
    ///
    /// An empty string is the documented TRADES default; anything else must
    /// match exactly, case aside. A value this does not know is an error
    /// rather than a fallback — a misspelled "BID" answered as trade bars
    /// looks like data.
    pub fn from_api_str(s: &str) -> Result<BarDataType, String> {
        Ok(match s.to_uppercase().as_str() {
            "" | "TRADES" => Self::Trades,
            "MIDPOINT" => Self::Midpoint,
            "BID" => Self::Bid,
            "ASK" => Self::Ask,
            "BID_ASK" => Self::BidAsk,
            // Not a name the venue answers to. Asked for it by name the venue
            // states it has no such data, and what it does serve is raw. An
            // adjusted series is built from those raw trades and the
            // contract's own actions, which means holding both before a bar
            // can be handed over — and this call answers on a callback, one
            // bar at a time, with the actions possibly still in flight.
            // `EClient::historical_data` waits, so it can hold both and does
            // serve this. Refused here rather than answered with trade bars
            // under an adjusted name.
            "ADJUSTED_LAST" => return Err(
                "an adjusted series is built from the raw trades and the contract's own \
                 actions, and a call that answers bar by bar on a callback cannot hold \
                 both. Ask `EClient::historical_data` for ADJUSTED_LAST, which waits and \
                 does. To do it by hand, ask here for TRADES and put them on one scale \
                 with `EClient::corporate_actions` and \
                 `control::adjustments::scale_bars`"
                    .to_string(),
            ),
            "AGGTRADES" => Self::AggTrades,
            "FEE_RATE" => Self::FeeRate,
            "YIELD_BID" => Self::YieldBid,
            "YIELD_ASK" => Self::YieldAsk,
            "YIELD_LAST" => Self::YieldLast,
            "YIELD_MARK" => Self::YieldMark,
            "NAV_LAST" => Self::NavLast,
            // Two series the venue carries separately and neither holds both,
            // so answering it means folding one bar out of two. Refused until
            // this client does that, rather than answering with one of them
            // under a name that says both.
            "YIELD_BID_ASK" => return Err(
                "YIELD_BID_ASK is two series, the yield at the bid and the yield at the \
                 ask, and the venue carries no series holding both. Ask for YIELD_BID \
                 and YIELD_ASK"
                    .to_string(),
            ),
            "HISTORICAL_VOLATILITY" => Self::HistoricalVolatility,
            "OPTION_IMPLIED_VOLATILITY" => Self::ImpliedVolatility,
            // The rate the venue prices options at, as a series of its own.
            // Not a name the reference client offers — it is what the
            // protocol's option tools ask for, and the one number an
            // option model needs that no tick states.
            "OPTION_EXERCISE_INTEREST_RATE" => Self::OptionInterestRate,
            other => {
                return Err(format!(
                    "Unsupported what_to_show '{other}': expected TRADES, MIDPOINT, \
                     BID, ASK, BID_ASK, AGGTRADES, FEE_RATE, YIELD_BID, YIELD_ASK, \
                     YIELD_LAST, YIELD_MARK, NAV_LAST, HISTORICAL_VOLATILITY, \
                     OPTION_IMPLIED_VOLATILITY or OPTION_EXERCISE_INTEREST_RATE",
                ));
            }
        })
    }

    /// The name the venue knows this by.
    ///
    /// Not the name the reference client uses, and not always the obvious
    /// casing: the midpoint is `MidPoint`, and sent as `Midpoint` the venue
    /// answers "no historical market data" — which reads as a series that does
    /// not exist rather than a name it does not know. Asked and answered
    /// against a session; see `probe_midpoint`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trades => "Last",
            Self::Midpoint => "MidPoint",
            Self::Bid => "Bid",
            Self::Ask => "Ask",
            Self::BidAsk => "BidAsk",
            Self::AggTrades => "AggLast",
            Self::FeeRate => "FeeRate",
            Self::YieldBid => "BidYield",
            Self::YieldAsk => "AskYield",
            Self::YieldLast => "LastYield",
            Self::YieldMark => "MarkYield",
            Self::NavLast => "NavLast",
            Self::HistoricalVolatility => "HistVol",
            Self::ImpliedVolatility => "OptionImpliedVol",
            Self::OptionInterestRate => "OptExInterestRate",
        }
    }
}

/// Bar size / time step for historical queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarSize {
    /// One bar covers a second.
    Sec1,
    /// One bar covers five seconds.
    Sec5,
    /// One bar covers ten seconds.
    Sec10,
    /// One bar covers fifteen seconds.
    Sec15,
    /// One bar covers thirty seconds.
    Sec30,
    /// One bar covers a minute.
    Min1,
    /// One bar covers two minutes.
    Min2,
    /// One bar covers three minutes.
    Min3,
    /// One bar covers five minutes.
    Min5,
    /// One bar covers ten minutes.
    Min10,
    /// One bar covers fifteen minutes.
    Min15,
    /// One bar covers twenty minutes.
    Min20,
    /// One bar covers half an hour.
    Min30,
    /// One bar covers an hour.
    Hour1,
    /// One bar covers two hours.
    Hour2,
    /// One bar covers three hours.
    Hour3,
    /// One bar covers four hours.
    Hour4,
    /// One bar covers eight hours.
    Hour8,
    /// One bar covers a day.
    Day1,
    /// One bar covers a week.
    Week1,
    /// One bar covers a month.
    Month1,
}

impl BarSize {
    /// Read the official API's bar-size string.
    ///
    /// The one table every request path reads, and case-sensitive on purpose:
    /// the official strings are exact, and a size this does not know is an
    /// error rather than a fallback — plausible, complete candles of the wrong
    /// size are worse than none.
    pub fn from_api_str(s: &str) -> Result<BarSize, String> {
        Ok(match s {
            "1 secs" | "1 sec" => Self::Sec1,
            "5 secs" => Self::Sec5,
            "10 secs" => Self::Sec10,
            "15 secs" => Self::Sec15,
            "30 secs" => Self::Sec30,
            "1 min" => Self::Min1,
            "2 mins" => Self::Min2,
            "3 mins" => Self::Min3,
            "5 mins" => Self::Min5,
            "10 mins" => Self::Min10,
            "15 mins" => Self::Min15,
            "20 mins" => Self::Min20,
            "30 mins" => Self::Min30,
            "1 hour" => Self::Hour1,
            "2 hours" => Self::Hour2,
            "3 hours" => Self::Hour3,
            "4 hours" => Self::Hour4,
            "8 hours" => Self::Hour8,
            "1 day" => Self::Day1,
            "1 week" | "1W" => Self::Week1,
            "1 month" | "1M" => Self::Month1,
            other => {
                return Err(format!(
                    "Unsupported bar_size '{other}': expected one of 1 secs, 5 secs, \
                     10 secs, 15 secs, 30 secs, 1 min, 2 mins, 3 mins, 5 mins, \
                     10 mins, 15 mins, 20 mins, 30 mins, 1 hour, 2 hours, \
                     3 hours, 4 hours, 8 hours, 1 day, 1 week, 1 month \
                     (case-sensitive)",
                ));
            }
        })
    }

    /// Whether a bar this long can be kept up to date.
    ///
    /// What the venue keeps sending after the batch is five-second bars, and
    /// the bar still forming is folded from those. So a size that is a whole
    /// number of them can be formed and one that is not cannot: a second is
    /// shorter than what arrives, and folding into it relabelled each
    /// five-second bar as a one-second one and handed the caller five times
    /// the volume under a size it never traded in.
    ///
    /// This is what this client can form, not what the venue accepts — nothing
    /// on the wire says a size may not be kept up to date. The list it replaces
    /// named five sizes, refusing sixteen that fold exactly and admitting the
    /// one that cannot.
    pub fn supports_keep_up_to_date(&self) -> bool {
        let seconds = self.seconds();
        seconds >= 5 && seconds.is_multiple_of(5)
    }

    /// How long one of these lasts.
    ///
    /// What a bar covers, so a bar still forming can be folded from the
    /// five-second bars the venue streams.
    pub fn seconds(&self) -> u32 {
        match self {
            Self::Sec1 => 1,
            Self::Sec5 => 5,
            Self::Sec10 => 10,
            Self::Sec15 => 15,
            Self::Sec30 => 30,
            Self::Min1 => 60,
            Self::Min2 => 120,
            Self::Min3 => 180,
            Self::Min5 => 300,
            Self::Min10 => 600,
            Self::Min15 => 900,
            Self::Min20 => 1_200,
            Self::Min30 => 1_800,
            Self::Hour1 => 3_600,
            Self::Hour2 => 7_200,
            Self::Hour3 => 10_800,
            Self::Hour4 => 14_400,
            Self::Hour8 => 28_800,
            Self::Day1 => 86_400,
            Self::Week1 => 604_800,
            Self::Month1 => 2_592_000,
        }
    }

    /// The name the venue knows this by.
    ///
    /// Not the name the reference client uses, and not always the obvious
    /// casing: the midpoint is `MidPoint`, and sent as `Midpoint` the venue
    /// answers "no historical market data" — which reads as a series that does
    /// not exist rather than a name it does not know. Asked and answered
    /// against a session; see `probe_midpoint`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sec1 => "1 secs",
            Self::Sec5 => "5 secs",
            Self::Sec10 => "10 secs",
            Self::Sec15 => "15 secs",
            Self::Sec30 => "30 secs",
            Self::Min1 => "1 min",
            Self::Min2 => "2 mins",
            Self::Min3 => "3 mins",
            Self::Min5 => "5 mins",
            Self::Min10 => "10 mins",
            Self::Min15 => "15 mins",
            Self::Min20 => "20 mins",
            Self::Min30 => "30 mins",
            Self::Hour1 => "1 hour",
            Self::Hour2 => "2 hours",
            Self::Hour3 => "3 hours",
            Self::Hour4 => "4 hours",
            Self::Hour8 => "8 hours",
            Self::Day1 => "1 day",
            Self::Week1 => "1 week",
            Self::Month1 => "1 month",
        }
    }
}

/// Parameters for a historical data request.
#[derive(Debug, Clone)]
pub struct HistoricalRequest {
    /// The name this client gave the query, which the answer echoes.
    pub query_id: String,
    /// The venue's id for the contract.
    pub con_id: u32,
    /// Its ticker.
    pub symbol: String,
    /// Wire security type and exchange for the contract being requested.
    /// Owned rather than static: they come from the caller's `Contract`, and
    /// hardcoding them described a different contract than was asked for.
    pub sec_type: String,
    /// Which venue to answer for.
    pub exchange: String,
    /// Which series is wanted.
    pub data_type: BarDataType,
    /// The end of the window. Empty means now.
    pub end_time: String,
    /// How far back from that end it reaches.
    pub duration: String,
    /// How long one bar covers.
    pub bar_size: BarSize,
    /// Whether to count only regular trading hours.
    pub use_rth: bool,
    /// Whether the venue keeps sending once the window is answered.
    pub keep_up_to_date: bool,
    /// Whether a contract that has already expired is in scope.
    ///
    /// Stated on every query, and stated as the caller set it. Written as a
    /// flat `no`, a request for a settled future asked about a contract that
    /// no longer exists and came back empty.
    pub include_expired: bool,
}

/// A single historical OHLCV bar parsed from XML.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoricalBar {
    /// When the bar opened.
    pub time: String,
    /// Its first price.
    pub open: f64,
    /// Its highest.
    pub high: f64,
    /// Its lowest.
    pub low: f64,
    /// Its last.
    pub close: f64,
    /// How much traded in it.
    pub volume: i64,
    /// The volume-weighted average price.
    pub wap: f64,
    /// How many trades made it.
    pub count: u32,
}

/// Parsed historical data response.
#[derive(Debug, Clone)]
pub struct HistoricalResponse {
    /// The name this client gave the query, which the answer echoes.
    pub query_id: String,
    /// The zone the times are stated in.
    pub timezone: String,
    /// The bars themselves.
    pub bars: Vec<HistoricalBar>,
    /// Whether this is the last part of the answer.
    pub is_complete: bool,
}

/// Build the XML query for a historical bar data request.
/// The spelling of a duration unit this venue accepts.
///
/// It is not one case or the other: seconds and weeks are taken uppercase,
/// days, months and years lowercase, and the wrong case is refused outright
/// with "Invalid time length" rather than corrected. Measured against a live
/// session, every unit, both cases: `S` and `W` are taken and `s`/`w` refused;
/// `d`, `m`, `y` are taken and `D`/`M`/`Y` refused.
///
/// The duration had been lowercased whole, which is right for three of the five
/// and silently breaks the other two: a caller asking for seconds or weeks was
/// refused, while the same span asked for in days was served. Callers state the
/// unit however the reference client documents it, so it is normalised here.
pub fn normalize_duration(duration: &str) -> String {
    let trimmed = duration.trim();
    let Some(unit) = trimmed.chars().last() else {
        return trimmed.to_string();
    };
    let spelled = match unit {
        'S' | 's' => 'S',
        'D' | 'd' => 'd',
        'W' | 'w' => 'W',
        'M' | 'm' => 'm',
        'Y' | 'y' => 'y',
        // Not a unit this venue names. Passed through, because refusing it here
        // would hide the venue's answer about what it accepts.
        other => other,
    };
    let mut out: String = trimmed[..trimmed.len() - unit.len_utf8()].to_string();
    out.push(spelled);
    out
}

/// Build the query the venue reads, as the XML it expects.
pub fn build_query_xml(req: &HistoricalRequest) -> String {
    let exchange = match req.exchange.as_str() {
        "SMART" => "BEST",
        e => e,
    };
    let rth = if req.use_rth { "true" } else { "false" };
    let expired = if req.include_expired { "yes" } else { "no" };

    let data_str = req.data_type.as_str();
    // keepUpToDate uses structured ;;-delimited ID required by CCP gateway parser.
    // One-shot uses simple ID (HMDS accepts it fine).
    let query_id = if req.keep_up_to_date {
        let graph_name = format!("{}@{} {}", req.symbol, exchange, data_str);
        format!("{};;{};;1;;true;;0;;I", req.query_id, graph_name)
    } else {
        req.query_id.clone()
    };

    let (end_time_tag, refresh_tag) = if req.keep_up_to_date {
        (String::new(), "<refresh>5 secs</refresh>")
    } else {
        (format!("<endTime>{}</endTime>", req.end_time), "")
    };

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <Query>\
         <id>{id}</id>\
         <useRTH>{rth}</useRTH>\
         <contractID>{con_id}</contractID>\
         <exchange>{exchange}</exchange>\
         <secType>{sec_type}</secType>\
         <expired>{expired}</expired>\
         <type>BarData</type>\
         <data>{data}</data>\
         {end_time}\
         {refresh}\
         <timeLength>{dur}</timeLength>\
         <step>{step}</step>\
         <source>API</source>\
         <needTotalValue>false</needTotalValue>\
         <wholeDays>false</wholeDays>\
         <delay>auto</delay>\
         </Query>\
         </ListOfQueries>",
        id = query_id,
        con_id = req.con_id,
        sec_type = req.sec_type,
        data = data_str,
        end_time = end_time_tag,
        dur = req.duration,
        step = req.bar_size.as_str(),
        refresh = refresh_tag,
    )
}

/// Build a historical data query message.
pub fn build_historical_request(req: &HistoricalRequest, seq: u32) -> Vec<u8> {
    let xml = build_query_xml(req);
    fix::fix_build(
        &[
            (fix::TAG_MSG_TYPE, "W"),
            (TAG_HISTORICAL_XML, &xml),
        ],
        seq,
    )
}

/// Build a cancellation message for a real-time bar subscription.
pub fn build_cancel_request(ticker_id: &str, seq: u32) -> Vec<u8> {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfCancelQueries>\
         <CancelQuery>\
         <id>ticker:{ticker_id}</id>\
         </CancelQuery>\
         </ListOfCancelQueries>",
    );
    fix::fix_build(
        &[
            (fix::TAG_MSG_TYPE, "Z"),
            (TAG_HISTORICAL_XML, &xml),
        ],
        seq,
    )
}

/// The smallest price move the venue states for a contract, where it states one.
///
/// Every price on the contract is decoded against this, so the wrong one does
/// not fail — it scales every price by a constant and reports the result as
/// the venue's. A penny is the right answer for a US share and wrong for a
/// currency pair, a future, and anything quoted in yen — so where the venue
/// states none, this states none, and the bars are left unread.
///
/// The venue states it on the definition, so a definition that does not is the
/// interesting case and says so rather than passing quietly as a share.
pub fn min_tick_of(xml_tag: &str, ticker_id: &str) -> Option<f64> {
    match tag(xml_tag, "minTick").and_then(|s| s.parse::<f64>().ok()) {
        Some(tick) => Some(tick),
        None => {
            // Nothing is decoded without it. Prices in a bar are counted in
            // this unit, so choosing one decides every price in the answer:
            // a penny is right for a US share and wrong for everything that
            // moves in anything else, and the caller cannot tell which they
            // were handed. A bar that cannot be read is told; a bar read at a
            // unit nobody stated is a wrong price presented as a right one.
            log::warn!("no minTick stated for ticker {ticker_id}; its bars cannot be read");
            None
        }
    }
}

/// Parse a ResultSetBar XML response into bars.
pub fn parse_bar_response(xml: &str) -> Option<HistoricalResponse> {
    // Check for ResultSetBar
    if !xml.contains("<ResultSetBar>") {
        return None;
    }

    let query_id = tag(xml, "id").unwrap_or("").to_string();
    let timezone = tag(xml, "tz").unwrap_or("").to_string();
    let is_complete = tag(xml, "eoq").unwrap_or("false") == "true";

    let mut bars = Vec::new();
    let mut search_start = 0;

    while let Some(bar_start) = xml[search_start..].find("<Bar>") {
        let abs_start = search_start + bar_start;
        let bar_end = match xml[abs_start..].find("</Bar>") {
            Some(e) => abs_start + e + 6,
            None => break,
        };
        let bar_xml = &xml[abs_start..bar_end];

        // A price the bar does not state is not nought. Read that way a bar
        // whose close went missing is a crash to zero on a caller's chart, and
        // nothing in it says the number was never sent — which is the same
        // reason a bar whose unit nobody stated is not read either.
        let priced = |name: &str| -> Option<f64> {
            tag(bar_xml, name).and_then(|s| s.parse().ok())
        };
        let (Some(open), Some(high), Some(low), Some(close)) =
            (priced("open"), priced("high"), priced("low"), priced("close"))
        else {
            log::warn!("a bar states no open, high, low or close, so the series is not read");
            return None;
        };
        let bar = HistoricalBar {
            time: tag(bar_xml, "time").unwrap_or("").to_string(),
            open,
            high,
            low,
            close,
            volume: tag(bar_xml, "volume")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            wap: tag(bar_xml, "weightedAvg")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            count: tag(bar_xml, "count")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        };
        bars.push(bar);
        search_start = bar_end;
    }

    Some(HistoricalResponse {
        query_id,
        timezone,
        bars,
        is_complete,
    })
}

/// Extract the ticker ID from a ResultSetTickerId response (for real-time bar
/// subscriptions).
pub fn parse_ticker_id(xml: &str) -> Option<String> {
    if !xml.contains("<ResultSetTickerId>") {
        return None;
    }
    // The assignment comes back under either name: a bar subscription is
    // answered with `tickerId` and a tick-by-tick one with `rtTickerId`. The
    // second name is a fallback only — a tick-by-tick acknowledgement is taken
    // and answered before this is reached — but it costs nothing and the two
    // shapes are not otherwise told apart here.
    tag(xml, "tickerId")
        .or_else(|| tag(xml, "rtTickerId"))
        .map(|s| s.to_string())
}

/// Parameters for a head timestamp request.
#[derive(Debug, Clone)]
pub struct HeadTimestampRequest {
    /// The venue's id for the contract.
    pub con_id: u32,
    /// What kind of contract it is, as the venue names it.
    pub sec_type: String,
    /// Which venue to answer for.
    pub exchange: String,
    /// Which series is wanted.
    pub data_type: BarDataType,
    /// Whether to count only regular trading hours.
    pub use_rth: bool,
}

/// Parsed head timestamp response.
#[derive(Debug, Clone)]
pub struct HeadTimestampResponse {
    /// The earliest moment the venue holds data for.
    pub head_timestamp: String,
    /// The zone the times are stated in.
    pub timezone: String,
}

/// The id a head-timestamp query goes out under, which is what its answer
/// comes back naming.
///
/// Built from the request itself, so the caller that sent it can be found from
/// the reply rather than from the order the replies happen to arrive in.
pub fn head_timestamp_query_id(req: &HeadTimestampRequest) -> String {
    let exchange = match req.exchange.as_str() {
        "SMART" => "BEST",
        e => e,
    };
    let rth = if req.use_rth { "true" } else { "false" };
    format!("TickHeadClient1;;{}@{} {};;0;;{};;0;;U",
        req.con_id, exchange, req.data_type.as_str(), rth)
}

/// Build the XML query for a head timestamp request.
pub fn build_head_timestamp_xml(req: &HeadTimestampRequest) -> String {
    let exchange = match req.exchange.as_str() {
        "SMART" => "BEST",
        e => e,
    };
    let rth = if req.use_rth { "true" } else { "false" };
    let id = head_timestamp_query_id(req);

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <Query>\
         <id>{id}</id>\
         <useRTH>{rth}</useRTH>\
         <contractID>{con_id}</contractID>\
         <exchange>{exchange}</exchange>\
         <secType>{sec_type}</secType>\
         <type>TickHeadTimeStamp</type>\
         <data>{data}</data>\
         <step>-1</step>\
         <source>API</source>\
         <needTotalValue>false</needTotalValue>\
         <wholeDays>false</wholeDays>\
         <delay>auto</delay>\
         </Query>\
         </ListOfQueries>",
        con_id = req.con_id,
        sec_type = req.sec_type,
        data = req.data_type.as_str(),
    )
}

/// Map whatToShow to data type.
/// What the venue calls a tick series, from what a caller calls it.
///
/// A name this does not know is refused rather than turned into trades.
/// Falling back to trades answers a misspelled `BID`, or the venue's
/// interest-rate series, with option prints and reports nothing.
pub fn tick_data_type(what_to_show: &str) -> Result<&'static str, String> {
    Ok(match what_to_show.to_uppercase().as_str() {
        "" | "TRADES" => "AllLast",
        "MIDPOINT" => "MidPoint",
        "BID_ASK" => "BidAsk",
        // The rate the venue prices options at, which it serves as a series of
        // its own rather than on any tick.
        "OPTION_EXERCISE_INTEREST_RATE" => "OptExInterestRate",
        other => {
            return Err(format!(
                "Unsupported what_to_show '{other}' for historical ticks: expected TRADES, \
                 MIDPOINT, BID_ASK or OPTION_EXERCISE_INTEREST_RATE",
            ));
        }
    })
}

/// What a historical-tick window has to state to be askable.
///
/// The query this client sends is bounded at its end and counts back from
/// there. Writing a start into that same field returns the ticks before the
/// moment rather than after it: the right number of records, off the wrong
/// side of the clock, with nothing in them to say so.
pub fn validate_tick_window(start_date_time: &str, end_date_time: &str) -> Result<(), String> {
    match (start_date_time.is_empty(), end_date_time.is_empty()) {
        // One end and a count is what the venue serves an API client, and it
        // is the venue that says so — asked with neither it answers "2 out of
        // startTime/endTime/timeLength parameters have to be specified", and
        // with both and no count "Times and Sales queries not length based not
        // allowed from API".
        (true, true) => Err(
            "historical ticks are asked for from one end and counted from there, and this \
             request names neither. Give start_date_time for the ticks after a moment, or \
             end_date_time for the ones before it."
                .to_string(),
        ),
        (false, false) => Err(format!(
            "historical ticks are asked for from one end and counted from there, and this \
             request names both ({start_date_time} and {end_date_time}). Give one of them \
             and the count says how far it reaches.",
        )),
        _ => Ok(()),
    }
}

/// Build the XML query for a historical ticks request.
///
/// Uses `<type>TickData</type>`, `<step>ticks</step>`, `<timeLength>{N}
/// t</timeLength>`.
#[allow(clippy::too_many_arguments)]
pub fn build_tick_query_xml(
    query_id: &str, con_id: i64, start_date_time: &str, end_date_time: &str,
    number_of_ticks: u32, what_to_show: &str, use_rth: bool,
    sec_type: &str, exchange: &str, include_expired: bool,
) -> String {
    let expired = if include_expired { "yes" } else { "no" };
    // Stated from the contract, and left unstated where the contract does
    // not state it. Assuming a BEST-routed US stock describes every other
    // kind of contract wrongly, and a description stated here is one the
    // venue reads. The contract id identifies the contract exactly, so an
    // empty field asks about it rather than about something else.
    let rth = if use_rth { "true" } else { "false" };
    let data = tick_data_type(what_to_show).unwrap_or("AllLast");

    // Both bounds, each in the field that carries it. The venue holds a start
    // and an end apart and the reference client passes both, so a request
    // naming only a start is one it serves; this used to refuse that, having
    // once written the start into the end's field and asked for the ticks
    // before the moment the caller wanted the ticks after — the answer looked
    // right and covered the wrong side of the clock. The field, not the
    // request, was the trouble.
    //
    // One end and a count, which is what the venue serves an API client. It
    // says so itself when asked otherwise: naming neither end is answered
    // "2 out of startTime/endTime/timeLength parameters have to be specified",
    // and naming both without a count "Times and Sales queries not length
    // based not allowed from API". Either end will do — a start was refused
    // here before it was ever sent, and the venue answers one with the ticks
    // after it.
    let time_tag = if start_date_time.is_empty() {
        format!("<endTime>{end_date_time}</endTime>")
    } else {
        format!("<startTime>{start_date_time}</startTime>")
    };
    let length_tag = format!("<timeLength>{number_of_ticks} t</timeLength>");

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <Query>\
         <id>{query_id}</id>\
         <useRTH>{rth}</useRTH>\
         <contractID>{con_id}</contractID>\
         <exchange>{exchange}</exchange>\
         <secType>{sec_type}</secType>\
         <expired>{expired}</expired>\
         <type>TickData</type>\
         <data>{data}</data>\
         {time_tag}\
         {length_tag}\
         <step>ticks</step>\
         <source>API</source>\
         <wholeDays>true</wholeDays>\
         <delay>auto</delay>\
         </Query>\
         </ListOfQueries>",
    )
}

/// Parse a ResultSetTick XML response into historical tick data.
pub fn parse_tick_response(xml: &str, what_to_show: &str) -> Option<(String, crate::types::HistoricalTickData, bool)> {
    if !xml.contains("<ResultSetTick>") {
        return None;
    }

    let query_id = tag(xml, "id").unwrap_or("").to_string();
    let is_complete = tag(xml, "eoq").unwrap_or("false") == "true";

    let upper = what_to_show.to_uppercase();
    let mut search_start = 0;

    match upper.as_str() {
        "BID_ASK" => {
            let mut ticks = Vec::new();
            while let Some(tick_pos) = xml[search_start..].find("<Tick>") {
                let abs = search_start + tick_pos;
                let end = match xml[abs..].find("</Tick>") {
                    Some(e) => abs + e + 7,
                    None => break,
                };
                let t = &xml[abs..end];
                ticks.push(crate::types::HistoricalTickBidAsk {
                    time: tag(t, "time").unwrap_or("").to_string(),
                    bid_price: tag(t, "priceBid").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    ask_price: tag(t, "priceAsk").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    bid_size: tag(t, "sizeBid").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    ask_size: tag(t, "sizeAsk").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                });
                search_start = end;
            }
            Some((query_id, crate::types::HistoricalTickData::BidAsk(ticks), is_complete))
        }
        // A rate is a value with a moment, and nothing else. Read through
        // the trade decoder it arrives as a print, with a size and a venue it
        // never had. No trade is involved in this series.
        "MIDPOINT" | "OPTION_EXERCISE_INTEREST_RATE" => {
            let mut ticks = Vec::new();
            while let Some(tick_pos) = xml[search_start..].find("<Tick>") {
                let abs = search_start + tick_pos;
                let end = match xml[abs..].find("</Tick>") {
                    Some(e) => abs + e + 7,
                    None => break,
                };
                let t = &xml[abs..end];
                ticks.push(crate::types::HistoricalTickMidpoint {
                    time: tag(t, "time").unwrap_or("").to_string(),
                    price: tag(t, "price").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                });
                search_start = end;
            }
            Some((query_id, crate::types::HistoricalTickData::Midpoint(ticks), is_complete))
        }
        _ => {
            // TRADES / AllLast
            let mut ticks = Vec::new();
            while let Some(tick_pos) = xml[search_start..].find("<Tick>") {
                let abs = search_start + tick_pos;
                let end = match xml[abs..].find("</Tick>") {
                    Some(e) => abs + e + 7,
                    None => break,
                };
                let t = &xml[abs..end];
                ticks.push(crate::types::HistoricalTickLast {
                    time: tag(t, "time").unwrap_or("").to_string(),
                    price: tag(t, "price").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    size: tag(t, "size").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    exchange: tag(t, "exchange").unwrap_or("").to_string(),
                    special_conditions: tag(t, "specialConditions").unwrap_or("").to_string(),
                });
                search_start = end;
            }
            Some((query_id, crate::types::HistoricalTickData::Last(ticks), is_complete))
        }
    }
}

/// Build the XML subscription for real-time 5-second bars.
pub fn build_realtime_bar_xml(
    query_id: &str, con_id: i64, what_to_show: &str, use_rth: bool,
    sec_type: &str, exchange: &str,
) -> String {
    // Stated from the contract rather than assumed. A stock routed BEST was
    // the only shape this ever described, so a request for anything else — an
    // FX pair on IDEALPRO, a future on its own venue — went out saying it was
    // a US stock and came back untyped.

    let rth = if use_rth { "true" } else { "false" };
    // Through the one place that knows these names. Spelled out again here,
    // the midpoint was "Midpoint" in two of the three and "MidPoint" in the
    // third — and the venue, which only takes the third, answered the other
    // two with "no historical market data", which reads as a series that does
    // not exist rather than a name that is misspelled.
    // Refused at the request, so nothing reaches here that this does not know.
    // Falling back to trades sent a different series than the one asked for,
    // and the bars that came back read as the ones the caller wanted.
    let data = BarDataType::from_api_str(what_to_show)
        .map(|kind| kind.as_str())
        .unwrap_or("Last");

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <Query>\
         <id>{query_id}</id>\
         <useRTH>{rth}</useRTH>\
         <contractID>{con_id}</contractID>\
         <exchange>{exchange}</exchange>\
         <secType>{sec_type}</secType>\
         <type>BarData</type>\
         <data>{data}</data>\
         <refresh>5 secs</refresh>\
         <step>5 secs</step>\
         <source>API</source>\
         <needTotalValue>false</needTotalValue>\
         <wholeDays>false</wholeDays>\
         </Query>\
         </ListOfQueries>",
    )
}

/// Decode a real-time bar binary payload.
///
/// Uses LSB-first bit reader with 4-byte group reversal.
/// Returns (low, open, high, close, volume, wap, count) or None.
pub fn decode_bar_payload(
    payload: &[u8],
    min_tick: f64,
    size_tick: f64,
) -> Option<crate::types::RealTimeBar> {
    if payload.is_empty() {
        return None;
    }

    // Reverse byte order within 4-byte groups
    let mut reordered = Vec::with_capacity(payload.len());
    for chunk in payload.chunks(4) {
        for &b in chunk.iter().rev() {
            reordered.push(b);
        }
    }

    let data = &reordered;
    let mut pos: usize = 0; // bit position

    // A read past the end of the payload takes zeroes, and so does every
    // field after it. Unrecorded, a payload cut anywhere decodes into a bar
    // of plausible zeroes indistinguishable from one the venue sent.
    let overran = std::cell::Cell::new(false);
    let read_bits = |pos: &mut usize, n: usize| -> u32 {
        let mut val: u32 = 0;
        for i in 0..n {
            let byte_idx = *pos / 8;
            let bit_idx = *pos % 8;
            if byte_idx < data.len() {
                val |= (((data[byte_idx] >> bit_idx) & 1) as u32) << i;
            } else {
                overran.set(true);
            }
            *pos += 1;
        }
        val
    };

    // 4 bits padding
    read_bits(&mut pos, 4);

    // Count: 1-bit flag selects width
    let count = if read_bits(&mut pos, 1) == 1 {
        read_bits(&mut pos, 8) as i32
    } else {
        read_bits(&mut pos, 32) as i32
    };

    // Low price in ticks (31-bit signed)
    let low_ticks = read_bits(&mut pos, 31);
    // Sign-extended in a width that holds the intermediate. A 31-bit value with
    // its sign bit set is the raw value less 2^31, and that subtraction does
    // not fit an i32 — a bar with a low below zero, which a spread has, took
    // the process down.
    let low_ticks_signed = if low_ticks & (1 << 30) != 0 {
        (low_ticks as i64 - (1i64 << 31)) as i32
    } else {
        low_ticks as i32
    };
    let low = low_ticks_signed as f64 * min_tick;

    let (open, high, close, wap_sum);
    if count > 1 {
        // Delta width: 1-bit flag
        let width = if read_bits(&mut pos, 1) == 1 { 5 } else { 32 };
        let d_open = read_bits(&mut pos, width);
        let d_high = read_bits(&mut pos, width);
        let d_close = read_bits(&mut pos, width);

        open = low + d_open as f64 * min_tick;
        high = low + d_high as f64 * min_tick;
        close = low + d_close as f64 * min_tick;

        // WAP sum: 1-bit flag selects width
        wap_sum = if read_bits(&mut pos, 1) == 1 {
            read_bits(&mut pos, 18) as f64
        } else {
            read_bits(&mut pos, 32) as f64
        };
    } else {
        open = low;
        high = low;
        close = low;
        wap_sum = 0.0;
    }

    // Volume: 1-bit flag selects width. A count of the increment the venue
    // said this contract's sizes move in, the same as a size on the quote and
    // tick-by-tick streams — where it was left as a whole number, one
    // instrument reported two different volumes.
    let counted = if read_bits(&mut pos, 1) == 1 {
        read_bits(&mut pos, 16) as f64
    } else {
        read_bits(&mut pos, 32) as f64
    };
    // An increment of nothing would zero every volume on the contract, which
    // reads as a bar nobody traded rather than as the missing increment it is.
    // Both writers guarantee a positive one today; this is what happens if a
    // third does not.
    let volume = counted * if size_tick > 0.0 { size_tick } else { 1.0 };

    // Divided by the count, not by the volume above it. The weighted sum is a
    // raw wire figure weighted by those same counts, so the two cancel; put
    // the scaled volume underneath it instead and the offset from the low
    // scales by the reciprocal of the increment — which on a contract counted
    // in hundred-millionths reads a sixty-thousand-dollar bar at fifty
    // million.
    let wap = if count > 1 && counted > 0.0 {
        low + wap_sum * min_tick / counted
    } else {
        low
    };

    if overran.get() {
        return None;
    }

    Some(crate::types::RealTimeBar {
        timestamp: 0, // filled by caller from message header
        open, high, low, close, volume, wap, count,
    })
}

/// Build the XML query for a historical schedule request.
///
/// Schedule requests use `<data>Schedule</data>` and
/// `<scheduleOnly>true</scheduleOnly>`
/// with `<type>BarData</type>`. Response is `<ResultSetSchedule>`.
pub fn build_schedule_xml(
    query_id: &str, con_id: i64, end_time: &str, duration: &str, use_rth: bool,
    sec_type: &str, exchange: &str,
) -> String {
    // The last of the query builders that described a US stock routed BEST
    // whatever contract it was asked about.

    let rth = if use_rth { "true" } else { "false" };

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <Query>\
         <id>{query_id}</id>\
         <useRTH>{rth}</useRTH>\
         <contractID>{con_id}</contractID>\
         <exchange>{exchange}</exchange>\
         <secType>{sec_type}</secType>\
         <type>BarData</type>\
         <data>Schedule</data>\
         <endTime>{end_time}</endTime>\
         <timeLength>{duration}</timeLength>\
         <step>1 day</step>\
         <scheduleOnly>true</scheduleOnly>\
         </Query>\
         </ListOfQueries>",
    )
}

/// Parse a ResultSetSchedule XML response into sessions.
pub fn parse_schedule_response(xml: &str) -> Option<crate::types::HistoricalScheduleResponse> {
    if !xml.contains("<ResultSetSchedule>") {
        return None;
    }

    let query_id = tag(xml, "id").unwrap_or("").to_string();
    let timezone = tag(xml, "tz").unwrap_or("").to_string();
    let start_date_time = tag(xml, "derivedStart").unwrap_or("").to_string();

    let mut sessions = Vec::new();
    let mut search_start = 0;

    // Parse Open/Close pairs into sessions
    while let Some(open_pos) = xml[search_start..].find("<Open>") {
        let abs_open = search_start + open_pos;
        let open_end = match xml[abs_open..].find("</Open>") {
            Some(e) => abs_open + e + 7,
            None => break,
        };
        let open_xml = &xml[abs_open..open_end];

        let open_time = tag(open_xml, "time").unwrap_or("").to_string();
        let ref_date = tag(open_xml, "refDate").unwrap_or("").to_string();

        // Find the matching Close
        let close_time = if let Some(close_pos) = xml[open_end..].find("<Close>") {
            let abs_close = open_end + close_pos;
            let close_end = match xml[abs_close..].find("</Close>") {
                Some(e) => abs_close + e + 8,
                None => break,
            };
            let close_xml = &xml[abs_close..close_end];
            search_start = close_end;
            tag(close_xml, "time").unwrap_or("").to_string()
        } else {
            search_start = open_end;
            String::new()
        };

        sessions.push(crate::types::ScheduleSession {
            ref_date,
            open_time,
            close_time,
        });
    }

    Some(crate::types::HistoricalScheduleResponse {
        query_id,
        timezone,
        start_date_time,
        // The venue derives both ends of what it covered and states both.
        // Only the start was read, and the end a caller got back was the one
        // it had asked for — so a request reaching past what the venue holds
        // was answered with its own timestamp as the coverage.
        end_date_time: tag(xml, "derivedEnd").unwrap_or("").to_string(),
        sessions,
    })
}

/// Parse a ResultSetHeadTimeStamp XML response.
pub fn parse_head_timestamp_response(xml: &str) -> Option<HeadTimestampResponse> {
    if !xml.contains("<ResultSetHeadTimeStamp>") {
        return None;
    }
    let head_timestamp = tag(xml, "headTS")?.to_string();
    let timezone = tag(xml, "tz").unwrap_or("").to_string();
    Some(HeadTimestampResponse { head_timestamp, timezone })
}

#[cfg(test)]
mod tests;
