//! Fundamental data queries via the data farm connection.

use std::io::Read;

/// FIX tag 6040: the sub protocol.
pub const TAG_SUB_PROTOCOL: u32 = 6040;
/// FIX tag 95: the raw data length.
pub const TAG_RAW_DATA_LENGTH: u32 = 95;
/// FIX tag 96: the raw data.
pub const TAG_RAW_DATA: u32 = 96;

/// Report types for fundamental data queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportType {
    /// A summary of the issuer as it stands.
    Snapshot,
    /// What analysts expect of it.
    Estimates,
    /// What it has coming: earnings dates and the like.
    Calendar,
}

impl ReportType {
    /// Which provider supplies this report.
    pub fn provider(&self) -> &'static str {
        "Fundamentals"
    }

    /// The name the venue knows the report by.
    ///
    /// Three, and these three: the reference client names exactly these and
    /// asks for them under exactly these words. Two others used to be offered
    /// here — a summary and the full statements — under names that appear
    /// nowhere in the vendor build at all, so what went out for them was a
    /// word the venue has never been asked for. The two the reference client
    /// does offer were refused instead.
    pub fn report_type_str(&self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Estimates => "estimates",
            Self::Calendar => "calendar",
        }
    }

    /// The name a caller asks for it by.
    pub fn api_name(&self) -> &'static str {
        match self {
            Self::Snapshot => "ReportSnapshot",
            Self::Estimates => "RESC",
            Self::Calendar => "CalendarReport",
        }
    }
}

/// Parameters for a fundamental data request.
#[derive(Debug, Clone)]
pub struct FundamentalRequest {
    /// The venue's id for the contract.
    pub con_id: u32,
    /// What kind of contract it is, as the venue names it.
    pub sec_type: String,
    /// What currency that is in.
    pub currency: String,
    /// Which report is wanted.
    pub report_type: ReportType,
    /// What this request calls itself, which the venue echoes on the answer.
    pub query_id: String,
}

/// What a fundamentals request calls itself.
///
/// The reference client counts these, and puts the count in the name, so the
/// venue echoes a different one on every answer. One name for all of them left
/// two in flight indistinguishable, and the reply to either was handed to
/// whichever had waited longest.
pub fn fundamentals_query_id(counter: u32) -> String {
    format!("COMPANY_FUNDAMENTALS{counter}")
}

/// The name the venue echoes back on an answer, so it can be matched to the
/// request that asked.
pub fn echoed_query_id(xml: &str) -> String {
    let Some(at) = xml.find("<id>") else { return String::new() };
    let rest = &xml[at + 4..];
    match rest.find("</id>") {
        Some(end) => rest[..end].to_string(),
        None => String::new(),
    }
}

/// Build the XML query for a fundamental data request.
pub fn build_fundamental_request_xml(req: &FundamentalRequest) -> String {
    let query_id = &req.query_id;
    format!(
        "<ListOfQueries>\
         <FundamentalsQuery>\
         <id>{query_id}</id>\
         <contractID>{con_id}</contractID>\
         <exchange>RTRSFND</exchange>\
         <secType>{sec_type}</secType>\
         <source>API</source>\
         <needTotalValue>false</needTotalValue>\
         <wholeDays>false</wholeDays>\
         <delay>auto</delay>\
         <reportType>{report_type}</reportType>\
         <currency>{currency}</currency>\
         </FundamentalsQuery>\
         </ListOfQueries>",
        con_id = req.con_id,
        sec_type = req.sec_type,
        report_type = req.report_type.report_type_str(),
        currency = req.currency,
    )
}

/// Extract the query ID from a `<FundResponse>` XML correlation tag.
pub fn parse_fundamental_response_id(xml: &str) -> Option<String> {
    if !xml.contains("<FundResponse>") {
        return None;
    }
    crate::control::xml::tag(xml, "id").map(|s| s.to_string())
}

/// Decompress gzip-compressed fundamental data (FIX tag 96).
///
/// Held to what one inflated frame may become: the frame this came in on is
/// bounded on the wire, and a payload inside it is not, so without a ceiling
/// a small frame could ask this process for every byte it has.
pub fn decompress_fundamental_data(compressed: &[u8]) -> Option<String> {
    let mut decoder = flate2::read::GzDecoder::new(compressed)
        .take(crate::protocol::fixcomp::MAX_INFLATED + 1);
    let mut result = String::new();
    decoder.read_to_string(&mut result).ok()?;
    if result.len() as u64 > crate::protocol::fixcomp::MAX_INFLATED {
        return None;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    /// The three the venue states, under the words it states them by. Two
    /// others used to be offered under words that appear nowhere in the
    /// vendor build, so a caller asking for either sent the venue something
    /// it has never been asked for.
    #[test]
    fn report_type_mapping() {
        for (kind, asked_by, on_the_wire) in [
            (ReportType::Snapshot, "ReportSnapshot", "snapshot"),
            (ReportType::Estimates, "RESC", "estimates"),
            (ReportType::Calendar, "CalendarReport", "calendar"),
        ] {
            assert_eq!(kind.api_name(), asked_by);
            assert_eq!(kind.report_type_str(), on_the_wire);
            assert_eq!(kind.provider(), "Fundamentals");
        }
    }

    #[test]
    fn fundamental_request_xml_structure() {
        let req = FundamentalRequest {
            query_id: fundamentals_query_id(1),
            con_id: 265598,
            sec_type: "STK".to_string(),
            currency: "USD".to_string(),
            report_type: ReportType::Snapshot,
        };
        let xml = build_fundamental_request_xml(&req);
        assert!(xml.contains("<ListOfQueries>"));
        assert!(xml.contains("<FundamentalsQuery>"));
        assert!(xml.contains("<contractID>265598</contractID>"));
        assert!(xml.contains("<exchange>RTRSFND</exchange>"));
        assert!(xml.contains("<secType>STK</secType>"));
        assert!(xml.contains("<reportType>snapshot</reportType>"));
        assert!(xml.contains("<currency>USD</currency>"));
        assert!(xml.contains("<source>API</source>"));
    }

    #[test]
    fn parse_response_id_basic() {
        let xml = "<FundResponse><id>q42</id></FundResponse>";
        assert_eq!(parse_fundamental_response_id(xml), Some("q42".to_string()));
    }

    #[test]
    fn decompress_gzip_data() {
        let original = "<FundamentalData><Revenue>1000000</Revenue></FundamentalData>";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();

        let decompressed = decompress_fundamental_data(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    /// The frame this arrives on is bounded on the wire, and what it inflates
    /// to is not: a payload past the ceiling is refused rather than read.
    #[test]
    fn a_payload_inflating_past_the_ceiling_is_refused() {
        let huge = vec![0u8; (crate::protocol::fixcomp::MAX_INFLATED + 4096) as usize];
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&huge).unwrap();
        let bomb = encoder.finish().unwrap();
        assert!(bomb.len() < 1 << 20, "the frame itself is small: {}", bomb.len());

        assert!(
            decompress_fundamental_data(&bomb).is_none(),
            "what inflates past the ceiling is not read",
        );
    }

    #[test]
    fn parse_response_rejects_other() {
        assert!(parse_fundamental_response_id("<ResultSetBar>...</ResultSetBar>").is_none());
        assert!(parse_fundamental_response_id("not xml").is_none());
    }

    /// The shape the venue expects, which is a list even when it withdraws one
    /// thing, and names the request by the id the request gave itself.
    #[test]
    fn a_withdrawal_names_the_request_it_withdraws() {
        // Each request names itself, as the reference client does — it counts
        // them and puts the count in the name — so the venue echoes a
        // different one on each answer and two in flight are told apart.
        let first = fundamentals_query_id(1);
        let second = fundamentals_query_id(2);
        assert_ne!(first, second, "two requests do not share a name");

        let asked = build_fundamental_request_xml(&FundamentalRequest {
            con_id: 265598, sec_type: "STK".to_string(), currency: "USD".to_string(),
            report_type: ReportType::Snapshot, query_id: first.clone(),
        });
        assert!(asked.contains(&format!("<id>{first}</id>")));
        assert_eq!(echoed_query_id(&asked), first, "and the answer is matched by it");

        assert_eq!(
            crate::control::xml::cancel_query(&first),
            format!("<ListOfCancelQueries><CancelQuery><id>{first}</id>\
                     </CancelQuery></ListOfCancelQueries>").replace(' ', ""),
            "the request and the withdrawal name the same thing",
        );
    }
}
