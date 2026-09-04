//! Scanner subscriptions via the data connection.

use crate::protocol::fix;

/// FIX tag 6040: the sub protocol.
pub const TAG_SUB_PROTOCOL: u32 = 6040;

/// Parameters for a scanner subscription request.
#[derive(Debug, Clone)]
pub struct ScannerSubscription {
    /// Which kind of instrument the scan runs over.
    pub instrument: String,
    /// Which market.
    pub location_code: String,
    /// Which scan.
    pub scan_code: String,
    /// The most rows wanted.
    pub max_items: u32,
    /// Filter code / value pairs, named exactly as `req_scanner_parameters` names them
    /// (`priceAbove`, `usdMarketCapAbove`, `stkTypes`, …).
    pub filters: Vec<(String, String)>,
}

/// One entry from a scanner result.
#[derive(Debug, Clone, Default)]
pub struct ScannerEntry {
    /// The venue's id for the contract.
    ///
    /// A row is this and the time the contract entered the scan, and nothing
    /// else — the response carries no ticker, kind, venue or currency, so
    /// everything else a caller reads about the contract is resolved against
    /// the trading connection before the row is handed over.
    pub con_id: u32,
}

/// Parsed scanner subscription response.
#[derive(Debug, Clone)]
pub struct ScannerResult {
    /// The contracts the scan returned.
    pub con_ids: Vec<u32>,
    /// The rows themselves.
    pub entries: Vec<ScannerEntry>,
    /// When the venue ran it.
    pub scan_time: String,
    /// Why the venue would not run it, in its own words, and empty when it
    /// ran.
    ///
    /// A refused scan answers with the same element as a successful one and
    /// says what went wrong inside it. Read for its rows alone, a refusal was
    /// a scan that found nothing — the caller was told the market held no
    /// matches where the venue declined the question. The text is handed to
    /// the caller instead of the contracts alongside it.
    pub error_text: String,
}

/// Build a scanner parameters request (no XML payload).
pub fn build_scanner_params_request(seq: u32) -> Vec<u8> {
    fix::fix_build(
        &[
            (fix::TAG_MSG_TYPE, "U"),
            (TAG_SUB_PROTOCOL, "10001"),
        ],
        seq,
    )
}

/// Build the XML payload for a scanner subscription request.
pub fn build_scanner_subscribe_xml(sub: &ScannerSubscription, scan_id: &str) -> String {
    let mut filter = String::new();
    if !sub.filters.is_empty() {
        filter.push_str("<Filter varName=\"filter\">");
        for (code, value) in &sub.filters {
            filter.push_str(&format!("<{code}>{value}</{code}>"));
        }
        filter.push_str("</Filter>");
    }
    format!(
        "<ScanSubscription>\
         <id>{id}</id>\
         <instrument>{instrument}</instrument>\
         <locations>{locations}</locations>\
         <scanCode>{scan_code}</scanCode>\
         <source>API</source>\
         <maxItems>{max_items}</maxItems>\
         {filter}\
         <suspend>no</suspend>\
         <inclRestrictedLocations>yes</inclRestrictedLocations>\
         <aggGroup>-1</aggGroup>\
         </ScanSubscription>",
        id = scan_id,
        instrument = sub.instrument,
        locations = sub.location_code,
        scan_code = sub.scan_code,
        max_items = sub.max_items,
    )
}

/// Build the XML payload for cancelling a scanner subscription.
pub fn build_scanner_cancel_xml(scan_id: &str) -> String {
    format!(
        "<ScanDesubscription>\
         <id>{scan_id}</id>\
         </ScanDesubscription>",
    )
}

/// Parse a ScanResponse XML into contract IDs and scan time.
pub fn parse_scanner_response(xml: &str) -> Option<ScannerResult> {
    if !xml.contains("<ScanResponse>") {
        return None;
    }

    let scan_time = crate::control::xml::tag(xml, "scanTime").unwrap_or("").to_string();
    let error_text = crate::control::xml::tag(xml, "errorText").unwrap_or("").to_string();

    let mut con_ids = Vec::new();
    let mut entries = Vec::new();
    let mut search_start = 0;

    while let Some(c_start) = xml[search_start..].find("<Contract>") {
        let abs_start = search_start + c_start;
        let c_end = match xml[abs_start..].find("</Contract>") {
            Some(e) => abs_start + e + 11,
            None => break,
        };
        let contract_xml = &xml[abs_start..c_end];

        // A row is its contract id and nothing else, so a row whose id did
        // not arrive readable states nothing at all. Handed over as zero, it
        // named a contract the venue never stated — the same reason a bar
        // whose prices did not arrive is not read either.
        let Some(con_id) = crate::control::xml::tag(contract_xml, "contractID")
            .and_then(|s| s.parse::<u32>().ok())
        else {
            log::warn!("a scan row states no readable contract id, so the scan is not read");
            return None;
        };
        con_ids.push(con_id);
        entries.push(ScannerEntry { con_id });
        search_start = c_end;
    }

    Some(ScannerResult { con_ids, entries, scan_time, error_text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_params_request_structure() {
        let msg = build_scanner_params_request(1);
        let tags = fix::fix_parse(&msg);
        assert_eq!(tags[&fix::TAG_MSG_TYPE], "U");
        assert_eq!(tags[&TAG_SUB_PROTOCOL], "10001");
    }

    #[test]
    fn scanner_subscribe_xml_structure() {
        let sub = ScannerSubscription {
            instrument: "STK".to_string(),
            location_code: "STK.US.MAJOR".to_string(),
            scan_code: "TOP_PERC_GAIN".to_string(),
            max_items: 50,
            filters: Vec::new(),
        };
        let xml = build_scanner_subscribe_xml(&sub, "APISCAN1:1");
        assert!(!xml.contains("<Filter"), "no filters means no filter element: {xml}");
        assert!(xml.contains("<id>APISCAN1:1</id>"));
        assert!(xml.contains("<instrument>STK</instrument>"));
        assert!(xml.contains("<locations>STK.US.MAJOR</locations>"));
        assert!(xml.contains("<scanCode>TOP_PERC_GAIN</scanCode>"));
        assert!(xml.contains("<maxItems>50</maxItems>"));
        assert!(xml.contains("<source>API</source>"));
        assert!(xml.contains("<aggGroup>-1</aggGroup>"));
    }

    /// Filters are what a scan code is worth: `TOP_PERC_GAIN` unfiltered is
    /// a penny-stock
    /// list. They ride as one element per filter code inside the subscription's filter.
    #[test]
    fn scanner_subscribe_xml_carries_filters() {
        let sub = ScannerSubscription {
            instrument: "STK".to_string(),
            location_code: "STK.US.MAJOR".to_string(),
            scan_code: "TOP_PERC_GAIN".to_string(),
            max_items: 50,
            filters: vec![
                ("priceAbove".to_string(), "10".to_string()),
                ("stkTypes".to_string(), "inc:ETF".to_string()),
            ],
        };
        let xml = build_scanner_subscribe_xml(&sub, "APISCAN1:1");
        assert!(xml.contains("<Filter varName=\"filter\">"), "{xml}");
        assert!(xml.contains("<priceAbove>10</priceAbove>"), "{xml}");
        assert!(xml.contains("<stkTypes>inc:ETF</stkTypes>"), "{xml}");
        assert!(xml.contains("</Filter>"), "{xml}");
        // The filter sits between maxItems and suspend, where the subscription declares
        // it.
        let filter_at = xml.find("<Filter").unwrap();
        assert!(xml.find("<maxItems>").unwrap() < filter_at, "{xml}");
        assert!(filter_at < xml.find("<suspend>").unwrap(), "{xml}");
    }

    #[test]
    fn scanner_cancel_xml_structure() {
        let xml = build_scanner_cancel_xml("APISCAN31:3");
        assert!(xml.contains("<ScanDesubscription>"));
        assert!(xml.contains("<id>APISCAN31:3</id>"));
    }

    #[test]
    fn parse_scanner_response_basic() {
        let xml = r#"<ScanResponse>
            <id>APISCAN31:3</id>
            <scanTime>20260311-11:08:43</scanTime>
            <Contracts>
                <Contract>
                    <contractID>592977497</contractID>
                    <inScanTime>20260311-11:08:43</inScanTime>
                </Contract>
                <Contract>
                    <contractID>265598</contractID>
                    <inScanTime>20260311-11:08:43</inScanTime>
                </Contract>
            </Contracts>
        </ScanResponse>"#;

        let result = parse_scanner_response(xml).unwrap();
        assert_eq!(result.scan_time, "20260311-11:08:43");
        assert_eq!(result.con_ids.len(), 2);
        assert_eq!(result.con_ids[0], 592977497);
        assert_eq!(result.con_ids[1], 265598);
    }

    /// A refused scan says so inside the same element a successful one uses.
    /// Reading it for rows alone reports a scan that matched nothing, which is
    /// a different answer from the one the venue gave.
    #[test]
    fn a_refused_scan_carries_the_refusal() {
        let xml = r#"<ScanResponse>
            <id>APISCAN31:3</id>
            <errorText>Scanner subscription not allowed</errorText>
        </ScanResponse>"#;
        let result = parse_scanner_response(xml).expect("the response parses");
        assert_eq!(result.error_text, "Scanner subscription not allowed");
        assert!(result.con_ids.is_empty(), "and it found nothing to report");
    }

    /// A row is its contract id and nothing else. One whose id did not
    /// arrive readable states nothing, and delivering it named contract zero
    /// — a contract the venue never stated.
    #[test]
    fn a_scan_row_with_no_readable_contract_is_not_read() {
        let xml = r#"<ScanResponse>
            <id>APISCAN31:3</id>
            <Contracts>
                <Contract>
                    <contractID>265598</contractID>
                </Contract>
                <Contract>
                    <contractID>not-a-number</contractID>
                </Contract>
            </Contracts>
        </ScanResponse>"#;
        assert!(parse_scanner_response(xml).is_none());
    }

    #[test]
    fn parse_scanner_response_rejects_other() {
        assert!(parse_scanner_response("<ResultSetBar>...</ResultSetBar>").is_none());
        assert!(parse_scanner_response("not xml at all").is_none());
    }
}
