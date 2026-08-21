//! Histogram data queries via the data connection.
//!
//! Responses contain XML with Tick entries (price, size pairs).

use crate::protocol::fix;

use super::historical::TAG_HISTORICAL_XML;

/// Parameters for a histogram data request.
#[derive(Debug, Clone)]
pub struct HistogramRequest {
    /// The name this client gave the query, which the answer echoes.
    pub query_id: String,
    /// The venue's id for the contract.
    pub con_id: u32,
    /// What kind of contract it is.
    pub sec_type: String,
    /// Where it is routed.
    pub exchange: String,
    /// Whether to count only regular trading hours.
    pub use_rth: bool,
    /// Time period, e.g. "1 week", "3 days", "1 month".
    pub period: String,
    /// End time for the histogram query (HMDS requires 2 of
    /// startTime/endTime/timeLength).
    pub end_time: String,
}

/// A single histogram entry (price level and count at that level).
#[derive(Debug, Clone, PartialEq)]
pub struct HistogramEntry {
    /// The price, in the units the record carries.
    pub price: f64,
    /// How much traded there.
    pub count: i64,
}

/// The route the query names. HMDS spells the smart-routed choice BEST, which
/// is what an unnamed exchange means and what the head timestamp query sends
/// for it; anything else is the venue the caller named.
fn query_exchange(req: &HistogramRequest) -> &str {
    match req.exchange.as_str() {
        // SMART and BEST map to each other, and no other pair does. Any
        // other exchange passes through unchanged: naming one here states a
        // venue the caller did not, and the contract id already identifies
        // the contract.
        "SMART" => "BEST",
        e => e,
    }
}

/// The id a histogram query goes out under, which is what its answer comes
/// back naming. Built from the request so the caller that sent it can be found
/// from the reply rather than from the order the replies happen to arrive in,
/// and led by the query's own name so two histograms in flight at once are
/// told apart.
pub fn histogram_query_id(req: &HistogramRequest) -> String {
    let rth = if req.use_rth { "true" } else { "false" };
    format!(
        "{};;{}@{} Histogram;;0;;{rth};;0;;U",
        req.query_id, req.con_id, query_exchange(req),
    )
}

/// Build the XML query for a histogram data request.
pub fn build_histogram_request_xml(req: &HistogramRequest) -> String {
    let rth = if req.use_rth { "true" } else { "false" };

    // Already in the protocol's spelling, whose case is part of the
    // unit: lowercasing it turned a second into a unit that is not in the
    // table.
    let time_length = convert_period(&req.period);

    // Stated from the contract, as the bar and tick queries state theirs.
    // Fixing it to a BEST-routed stock describes a future or an FX pair as a
    // US stock, and the answer comes back for one.
    let exchange = query_exchange(req);
    // Left empty where the caller stated none, rather than described as a US
    // stock. The contract id identifies the contract exactly, and a type
    // stated here is one the venue reads.
    let sec_type = req.sec_type.as_str();

    let id = histogram_query_id(req);

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <Query>\
         <id>{id}</id>\
         <useRTH>{rth}</useRTH>\
         <contractID>{con_id}</contractID>\
         <exchange>{exchange}</exchange>\
         <secType>{sec_type}</secType>\
         <type>HistogramData</type>\
         <data>Last</data>\
         <endTime>{end_time}</endTime>\
         <timeLength>{time_length}</timeLength>\
         <source>API</source>\
         <needTotalValue>false</needTotalValue>\
         <wholeDays>false</wholeDays>\
         <delay>auto</delay>\
         </Query>\
         </ListOfQueries>",
        con_id = req.con_id,
        end_time = req.end_time,
    )
}

/// Build a histogram query message.
pub fn build_histogram_fix_request(req: &HistogramRequest, seq: u32) -> Vec<u8> {
    let xml = build_histogram_request_xml(req);
    fix::fix_build(
        &[
            (fix::TAG_MSG_TYPE, "W"),
            (TAG_HISTORICAL_XML, &xml),
        ],
        seq,
    )
}

/// Parse a histogram XML response into entries.
///
/// The response contains `<Tick>` elements with `<price>` and `<size>` children
/// inside an `<Events>` block.
pub fn parse_histogram_response(xml: &str) -> Option<Vec<HistogramEntry>> {
    if !xml.contains("<ResultSetHistogram>") {
        return None;
    }

    let mut entries = Vec::new();
    let mut search_start = 0;

    while let Some(tick_start) = xml[search_start..].find("<Tick>") {
        let abs_start = search_start + tick_start;
        let tick_end = match xml[abs_start..].find("</Tick>") {
            Some(e) => abs_start + e + 7,
            None => break,
        };
        let tick_xml = &xml[abs_start..tick_end];

        let price = crate::control::xml::tag(tick_xml, "price")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let count = crate::control::xml::tag(tick_xml, "size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        entries.push(HistogramEntry { price, count });
        search_start = tick_end;
    }

    Some(entries)
}

/// State a period the way the protocol's unit table states it.
///
/// The table is `S` seconds, `min` minutes, `h` hours, `d` days, `W` weeks,
/// `m` months, `q` quarters, `y` years. Its case carries meaning —
/// `m` is a month and `min` a minute, `S` a second and `d` a day. Flattening
/// the case sends a second as a unit not in the table. A week is its own unit;
/// rewriting one as seven days asks a different question.
///
/// A period this does not recognise is passed through as the caller wrote it,
/// so the venue refuses it rather than answering something else.
fn convert_period(period: &str) -> String {
    let parts: Vec<&str> = period.split_whitespace().collect();
    if parts.len() != 2 {
        return period.to_string();
    }
    let num: u32 = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => return period.to_string(),
    };
    match parts[1].to_lowercase().as_str() {
        "second" | "seconds" | "secs" | "sec" | "s" => format!("{num} S"),
        "minute" | "minutes" | "mins" | "min" => format!("{num} min"),
        "hour" | "hours" | "h" => format!("{num} h"),
        "day" | "days" | "d" => format!("{num} d"),
        "week" | "weeks" | "w" => format!("{num} W"),
        "month" | "months" | "m" => format!("{num} m"),
        "quarter" | "quarters" | "q" => format!("{num} q"),
        "year" | "years" | "y" => format!("{num} y"),
        _ => period.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Every unit in the table, in its own spelling. Case carries meaning: a
    /// month is `m` and a minute `min`, a second `S` and a day `d`.
    fn convert_period_variants() {
        assert_eq!(convert_period("30 seconds"), "30 S");
        assert_eq!(convert_period("5 minutes"), "5 min");
        assert_eq!(convert_period("4 hours"), "4 h");
        assert_eq!(convert_period("3 days"), "3 d");
        assert_eq!(convert_period("1 week"), "1 W");
        assert_eq!(convert_period("2 weeks"), "2 W");
        assert_eq!(convert_period("2 months"), "2 m");
        assert_eq!(convert_period("1 quarter"), "1 q");
        assert_eq!(convert_period("1 year"), "1 y");
        // passthrough for unknown
        assert_eq!(convert_period("foo"), "foo");
    }

    fn req(con_id: u32, sec_type: &str, exchange: &str, use_rth: bool, period: &str) -> HistogramRequest {
        HistogramRequest {
            query_id: "hg_1".to_string(),
            con_id,
            sec_type: sec_type.to_string(),
            exchange: exchange.to_string(),
            use_rth,
            period: period.to_string(),
            end_time: "20260320-21:00:00".to_string(),
        }
    }

    /// The query describes the contract the caller named. Fixing it to a
    /// BEST-routed stock describes a future or an FX pair as a US stock.
    #[test]
    fn the_query_describes_the_contract_it_was_given() {
        let xml = build_histogram_request_xml(&req(495512563, "FUT", "CME", true, "1 week"));
        assert!(xml.contains("<secType>FUT</secType>"), "{xml}");
        assert!(xml.contains("<exchange>CME</exchange>"), "{xml}");
        assert!(!xml.contains("<secType>CS</secType>"), "{xml}");
    }

    /// A smart-routed contract asks BEST, which is how HMDS spells it. The
    /// engine passes smart routing down under the name SMART, so the request
    /// must translate it rather than carry that name to the wire.
    #[test]
    fn the_venue_s_own_choice_reaches_the_wire_as_best() {
        let xml = build_histogram_request_xml(&req(265598, "CS", "SMART", true, "1 week"));
        assert!(xml.contains("<exchange>BEST</exchange>"), "{xml}");
        assert!(xml.contains("@BEST Histogram"), "{xml}");
    }

    /// And the query's own name leads its id, so the answer says which request
    /// it belongs to. Named only by contract, two histograms in flight had
    /// nothing in the answer to tell them apart.
    #[test]
    fn the_query_carries_the_name_the_answer_is_matched_by() {
        let xml = build_histogram_request_xml(&req(265598, "", "", true, "1 week"));
        assert!(xml.contains("<id>hg_1;;"), "{xml}");
    }

    /// SMART maps to BEST; nothing else is renamed.
    #[test]
    fn smart_is_named_best_and_nothing_else_is_renamed() {
        let smart = build_histogram_request_xml(&req(265598, "STK", "SMART", true, "1 week"));
        assert!(smart.contains("<exchange>BEST</exchange>"), "{smart}");
        let named = build_histogram_request_xml(&req(265598, "FUT", "NYMEX", true, "1 week"));
        assert!(named.contains("<exchange>NYMEX</exchange>"), "{named}");
    }

    #[test]
    fn build_xml_structure() {
        let req = req(265598, "", "", true, "1 week");
        let xml = build_histogram_request_xml(&req);
        assert!(xml.contains("<type>HistogramData</type>"));
        assert!(xml.contains("<contractID>265598</contractID>"));
        assert!(xml.contains("<useRTH>true</useRTH>"));
        assert!(xml.contains("<timeLength>1 W</timeLength>"));
        assert!(xml.contains("<data>Last</data>"));
        // An exchange the caller did not state is not one this states for them.
        // The contract id names the contract exactly.
        assert!(xml.contains("<exchange></exchange>"), "{xml}");
        assert!(xml.contains("<endTime>20260320-21:00:00</endTime>"));
        // No <step> tag
        assert!(!xml.contains("<step>"));
    }

    #[test]
    fn build_xml_rth_false() {
        let req = req(100, "", "", false, "3 days");
        let xml = build_histogram_request_xml(&req);
        assert!(xml.contains("<useRTH>false</useRTH>"));
        assert!(xml.contains("<timeLength>3 d</timeLength>"));
    }

    #[test]
    fn build_fix_request() {
        let msg = build_histogram_fix_request(&req(265598, "", "", true, "1 week"), 1);
        let tags = fix::fix_parse(&msg);
        assert_eq!(tags[&fix::TAG_MSG_TYPE], "W");
        assert!(tags[&TAG_HISTORICAL_XML].contains("<type>HistogramData</type>"));
    }

    #[test]
    fn parse_histogram_basic() {
        let xml = r#"<ResultSetHistogram>
            <id>histogramQuery;;265598@BEST Histogram;;0;;true;;0;;U</id>
            <eoq>true</eoq>
            <Events>
                <Tick><price>270.50</price><size>1500</size></Tick>
                <Tick><price>271.00</price><size>2300</size></Tick>
                <Tick><price>269.75</price><size>800</size></Tick>
            </Events>
        </ResultSetHistogram>"#;

        let entries = parse_histogram_response(xml).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].price, 270.50);
        assert_eq!(entries[0].count, 1500);
        assert_eq!(entries[1].price, 271.00);
        assert_eq!(entries[1].count, 2300);
        assert_eq!(entries[2].price, 269.75);
        assert_eq!(entries[2].count, 800);
    }

    #[test]
    fn parse_histogram_empty() {
        let xml = r#"<ResultSetHistogram>
            <id>test</id>
            <eoq>true</eoq>
            <Events></Events>
        </ResultSetHistogram>"#;
        // Valid histogram response with no data → empty vec
        let entries = parse_histogram_response(xml).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_histogram_rejects_non_histogram() {
        assert!(parse_histogram_response("<ResultSetBar>...</ResultSetBar>").is_none());
        assert!(parse_histogram_response("not xml at all").is_none());
    }
}
