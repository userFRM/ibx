//! Historical data queries via the data connection.
//!
//! Responses contain XML ResultSetBar with OHLCV bar data.

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
    /// What traded, adjusted for splits and dividends.
    AdjustedLast,
    /// The volatility the underlying realised.
    HistoricalVolatility,
    /// The volatility its options implied.
    ImpliedVolatility,
    /// The rate the venue's option model carries.
    OptionInterestRate,
}

impl BarDataType {
    /// Parse the official API what_to_show string. Unknown values were
    /// previously coerced to TRADES silently, so a misspelled "BID" quietly
    /// returned trade bars. An empty string is the documented
    /// TRADES default; anything else must match exactly (case-insensitive).
    pub fn from_api_str(s: &str) -> Result<BarDataType, String> {
        Ok(match s.to_uppercase().as_str() {
            "" | "TRADES" => Self::Trades,
            "MIDPOINT" => Self::Midpoint,
            "BID" => Self::Bid,
            "ASK" => Self::Ask,
            "BID_ASK" => Self::BidAsk,
            "ADJUSTED_LAST" => Self::AdjustedLast,
            "HISTORICAL_VOLATILITY" => Self::HistoricalVolatility,
            "OPTION_IMPLIED_VOLATILITY" => Self::ImpliedVolatility,
            // The rate the venue prices options at, as a series of its own.
            // Not a name the reference client offers — it is what the
            // counterpart's own option tools ask for, and the one number an
            // option model needs that no tick states.
            "OPTION_EXERCISE_INTEREST_RATE" => Self::OptionInterestRate,
            other => {
                return Err(format!(
                    "Unsupported what_to_show '{other}': expected TRADES, MIDPOINT, \
                     BID, ASK, BID_ASK, ADJUSTED_LAST, HISTORICAL_VOLATILITY \
                     OPTION_IMPLIED_VOLATILITY or OPTION_EXERCISE_INTEREST_RATE",
                ));
            }
        })
    }

    /// The name the venue knows this by.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trades => "Last",
            Self::Midpoint => "Midpoint",
            Self::Bid => "Bid",
            Self::Ask => "Ask",
            Self::BidAsk => "BidAsk",
            Self::AdjustedLast => "AdjustedLast",
            Self::HistoricalVolatility => "HV",
            Self::ImpliedVolatility => "IV",
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
    /// Parse the official API bar-size string. THE single table for every
    /// request path — two divergent copies previously fell back to Min5
    /// silently, so a typo or an unsupported size returned plausible,
    /// complete, WRONG candles. Case-sensitive on purpose: the
    /// official API strings are exact.
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

    /// Bar sizes the keepUpToDate streaming path supports. The rest are
    /// accepted on the batch path only; sending them with
    /// keep_up_to_date=true previously downgraded to Min5 silently.
    pub fn supports_keep_up_to_date(&self) -> bool {
        matches!(self, Self::Sec1 | Self::Sec5 | Self::Min5 | Self::Hour1 | Self::Day1)
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
    /// hardcoding them described a different contract than was asked for
 ///.
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
        // would hide the venue's own answer about what it accepts.
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
         <expired>no</expired>\
         <type>BarData</type>\
         <data>{data}</data>\
         {end_time}\
         <cutoffDate>20090224</cutoffDate>\
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
/// the venue's own. A penny is the right answer for a US share and wrong for a
/// currency pair, a future, and anything quoted in yen — so where the venue
/// states none, this states none, and the bars are left unread.
///
/// The venue states it on the definition, so a definition that does not is the
/// interesting case and says so rather than passing quietly as a share.
pub fn min_tick_of(xml_tag: &str, ticker_id: &str) -> Option<f64> {
    match extract_xml_tag(xml_tag, "minTick").and_then(|s| s.parse::<f64>().ok()) {
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

/// Extract a simple XML tag value: `<tag>value</tag>` → `value`.
pub fn extract_xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(&xml[start..end])
}

/// Parse a ResultSetBar XML response into bars.
pub fn parse_bar_response(xml: &str) -> Option<HistoricalResponse> {
    // Check for ResultSetBar
    if !xml.contains("<ResultSetBar>") {
        return None;
    }

    let query_id = extract_xml_tag(xml, "id").unwrap_or("").to_string();
    let timezone = extract_xml_tag(xml, "tz").unwrap_or("").to_string();
    let is_complete = extract_xml_tag(xml, "eoq").unwrap_or("false") == "true";

    let mut bars = Vec::new();
    let mut search_start = 0;

    while let Some(bar_start) = xml[search_start..].find("<Bar>") {
        let abs_start = search_start + bar_start;
        let bar_end = match xml[abs_start..].find("</Bar>") {
            Some(e) => abs_start + e + 6,
            None => break,
        };
        let bar_xml = &xml[abs_start..bar_end];

        let bar = HistoricalBar {
            time: extract_xml_tag(bar_xml, "time").unwrap_or("").to_string(),
            open: extract_xml_tag(bar_xml, "open")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            high: extract_xml_tag(bar_xml, "high")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            low: extract_xml_tag(bar_xml, "low")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            close: extract_xml_tag(bar_xml, "close")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            volume: extract_xml_tag(bar_xml, "volume")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            wap: extract_xml_tag(bar_xml, "weightedAvg")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            count: extract_xml_tag(bar_xml, "count")
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

/// Extract the ticker ID from a ResultSetTickerId response (for real-time bar subscriptions).
pub fn parse_ticker_id(xml: &str) -> Option<String> {
    if !xml.contains("<ResultSetTickerId>") {
        return None;
    }
    // The assignment comes back under either name: a bar subscription is
    // answered with `tickerId` and a tick-by-tick one with `rtTickerId`.
    // Reading only the first left every tick-by-tick assignment unmatched, so
    // the ticker was never bound to the subscription and every tick that
    // followed had nowhere to go.
    extract_xml_tag(xml, "tickerId")
        .or_else(|| extract_xml_tag(xml, "rtTickerId"))
        .map(|s| s.to_string())
}

/// Parameters for a head timestamp request.
#[derive(Debug, Clone)]
pub struct HeadTimestampRequest {
    /// The venue's id for the contract.
    pub con_id: u32,
    /// What kind of contract it is.
    pub sec_type: &'static str,
    /// Which venue to answer for.
    pub exchange: &'static str,
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

/// Build the XML query for a head timestamp request.
pub fn build_head_timestamp_xml(req: &HeadTimestampRequest) -> String {
    let exchange = match req.exchange {
        "SMART" => "BEST",
        e => e,
    };
    let rth = if req.use_rth { "true" } else { "false" };
    let id = format!("TickHeadClient1;;{}@{} {};;0;;{};;0;;U",
        req.con_id, exchange, req.data_type.as_str(), rth);

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
/// A name this does not know is refused rather than turned into trades. The
/// bar path stopped doing that when a misspelled `BID` quietly returned trade
/// bars; this path went on doing it, so a caller asking for anything
/// else was answered with trades and told nothing — which is exactly how a
/// request for the venue's own interest-rate series came back as a list of
/// option prints.
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

/// Build the XML query for a historical ticks request.
///
/// Uses `<type>TickData</type>`, `<step>ticks</step>`, `<timeLength>{N} t</timeLength>`.
#[allow(clippy::too_many_arguments)]
pub fn build_tick_query_xml(
    query_id: &str, con_id: i64, start_date_time: &str, end_date_time: &str,
    number_of_ticks: u32, what_to_show: &str, use_rth: bool,
    sec_type: &str, exchange: &str,
) -> String {
    // Stated from the contract. Assuming a US stock routed BEST left every
    // other kind asking for ticks under a description that is not its own.
    let exchange = if exchange.is_empty() { "BEST" } else { exchange };
    let sec_type = if sec_type.is_empty() { "CS" } else { sec_type };
    let rth = if use_rth { "true" } else { "false" };
    let data = tick_data_type(what_to_show).unwrap_or("AllLast");

    // Use endTime if provided, otherwise startTime
    let time_tag = if !end_date_time.is_empty() {
        format!("<endTime>{end_date_time}</endTime>")
    } else {
        format!("<endTime>{start_date_time}</endTime>")
    };

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <Query>\
         <id>{query_id}</id>\
         <useRTH>{rth}</useRTH>\
         <contractID>{con_id}</contractID>\
         <exchange>{exchange}</exchange>\
         <secType>{sec_type}</secType>\
         <expired>no</expired>\
         <type>TickData</type>\
         <data>{data}</data>\
         {time_tag}\
         <timeLength>{number_of_ticks} t</timeLength>\
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

    let query_id = extract_xml_tag(xml, "id").unwrap_or("").to_string();
    let is_complete = extract_xml_tag(xml, "eoq").unwrap_or("false") == "true";

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
                    time: extract_xml_tag(t, "time").unwrap_or("").to_string(),
                    bid_price: extract_xml_tag(t, "priceBid").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    ask_price: extract_xml_tag(t, "priceAsk").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    bid_size: extract_xml_tag(t, "sizeBid").and_then(|s| s.parse().ok()).unwrap_or(0),
                    ask_size: extract_xml_tag(t, "sizeAsk").and_then(|s| s.parse().ok()).unwrap_or(0),
                });
                search_start = end;
            }
            Some((query_id, crate::types::HistoricalTickData::BidAsk(ticks), is_complete))
        }
        "MIDPOINT" => {
            let mut ticks = Vec::new();
            while let Some(tick_pos) = xml[search_start..].find("<Tick>") {
                let abs = search_start + tick_pos;
                let end = match xml[abs..].find("</Tick>") {
                    Some(e) => abs + e + 7,
                    None => break,
                };
                let t = &xml[abs..end];
                ticks.push(crate::types::HistoricalTickMidpoint {
                    time: extract_xml_tag(t, "time").unwrap_or("").to_string(),
                    price: extract_xml_tag(t, "price").and_then(|s| s.parse().ok()).unwrap_or(0.0),
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
                    time: extract_xml_tag(t, "time").unwrap_or("").to_string(),
                    price: extract_xml_tag(t, "price").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    size: extract_xml_tag(t, "size").and_then(|s| s.parse().ok()).unwrap_or(0),
                    exchange: extract_xml_tag(t, "exchange").unwrap_or("").to_string(),
                    special_conditions: extract_xml_tag(t, "specialConditions").unwrap_or("").to_string(),
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
    let exchange = if exchange.is_empty() { "BEST" } else { exchange };
    let sec_type = if sec_type.is_empty() { "CS" } else { sec_type };
    let rth = if use_rth { "true" } else { "false" };
    let data = match what_to_show.to_uppercase().as_str() {
        "MIDPOINT" => "Midpoint",
        "BID" => "Bid",
        "ASK" => "Ask",
        _ => "Last",
    };

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
pub fn decode_bar_payload(payload: &[u8], min_tick: f64) -> Option<crate::types::RealTimeBar> {
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

    let read_bits = |pos: &mut usize, n: usize| -> u32 {
        let mut val: u32 = 0;
        for i in 0..n {
            let byte_idx = *pos / 8;
            let bit_idx = *pos % 8;
            if byte_idx < data.len() {
                val |= (((data[byte_idx] >> bit_idx) & 1) as u32) << i;
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

    // Volume: 1-bit flag selects width
    let volume = if read_bits(&mut pos, 1) == 1 {
        read_bits(&mut pos, 16) as f64
    } else {
        read_bits(&mut pos, 32) as f64
    };

    let wap = if count > 1 && volume > 0.0 {
        low + wap_sum * min_tick / volume
    } else {
        low
    };

    Some(crate::types::RealTimeBar {
        timestamp: 0, // filled by caller from message header
        open, high, low, close, volume, wap, count,
    })
}

/// Build the XML query for a historical schedule request.
///
/// Schedule requests use `<data>Schedule</data>` and `<scheduleOnly>true</scheduleOnly>`
/// with `<type>BarData</type>`. Response is `<ResultSetSchedule>`.
pub fn build_schedule_xml(
    query_id: &str, con_id: i64, end_time: &str, duration: &str, use_rth: bool,
    sec_type: &str, exchange: &str,
) -> String {
    // The last of the query builders that described a US stock routed BEST
    // whatever contract it was asked about.
    let exchange = if exchange.is_empty() { "BEST" } else { exchange };
    let sec_type = if sec_type.is_empty() { "CS" } else { sec_type };
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

    let query_id = extract_xml_tag(xml, "id").unwrap_or("").to_string();
    let timezone = extract_xml_tag(xml, "tz").unwrap_or("").to_string();
    let start_date_time = extract_xml_tag(xml, "derivedStart").unwrap_or("").to_string();

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

        let open_time = extract_xml_tag(open_xml, "time").unwrap_or("").to_string();
        let ref_date = extract_xml_tag(open_xml, "refDate").unwrap_or("").to_string();

        // Find the matching Close
        let close_time = if let Some(close_pos) = xml[open_end..].find("<Close>") {
            let abs_close = open_end + close_pos;
            let close_end = match xml[abs_close..].find("</Close>") {
                Some(e) => abs_close + e + 8,
                None => break,
            };
            let close_xml = &xml[abs_close..close_end];
            search_start = close_end;
            extract_xml_tag(close_xml, "time").unwrap_or("").to_string()
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
        end_date_time: String::new(), // filled by caller from request context
        sessions,
    })
}

/// Parse a ResultSetHeadTimeStamp XML response.
pub fn parse_head_timestamp_response(xml: &str) -> Option<HeadTimestampResponse> {
    if !xml.contains("<ResultSetHeadTimeStamp>") {
        return None;
    }
    let head_timestamp = extract_xml_tag(xml, "headTS")?.to_string();
    let timezone = extract_xml_tag(xml, "tz").unwrap_or("").to_string();
    Some(HeadTimestampResponse { head_timestamp, timezone })
}

#[cfg(test)]
mod tests;
