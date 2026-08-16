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
    /// Its headline figures.
    FinancialSummary,
    /// Its full statements.
    FinancialStatements,
}

impl ReportType {
    /// Which provider supplies this report.
    pub fn provider(&self) -> &'static str {
        match self {
            Self::Snapshot => "Fundamentals",
            Self::FinancialSummary => "Morningstar",
            Self::FinancialStatements => "Morningstar",
        }
    }

    /// The name the venue knows the report by.
    pub fn report_type_str(&self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::FinancialSummary => "finsum",
            Self::FinancialStatements => "finstat",
        }
    }
}

/// Parameters for a fundamental data request.
#[derive(Debug, Clone)]
pub struct FundamentalRequest {
    /// The venue's id for the contract.
    pub con_id: u32,
    /// What kind of contract it is.
    pub sec_type: &'static str,
    /// What currency that is in.
    pub currency: &'static str,
    /// Which report is wanted.
    pub report_type: ReportType,
}

/// What a fundamentals request calls itself, and what a cancel names to
/// withdraw it.
pub const FUNDAMENTALS_QUERY_ID: &str = "COMPANY_FUNDAMENTALS";

/// Build the XML query for a fundamental data request.
pub fn build_fundamental_request_xml(req: &FundamentalRequest) -> String {
    format!(
        "<ListOfQueries>\
         <FundamentalsQuery>\
         <id>{FUNDAMENTALS_QUERY_ID}</id>\
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
pub fn decompress_fundamental_data(compressed: &[u8]) -> Option<String> {
    let mut decoder = flate2::read::GzDecoder::new(compressed);
    let mut result = String::new();
    decoder.read_to_string(&mut result).ok()?;
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    #[test]
    fn report_type_mapping() {
        assert_eq!(ReportType::Snapshot.provider(), "Fundamentals");
        assert_eq!(ReportType::Snapshot.report_type_str(), "snapshot");

        assert_eq!(ReportType::FinancialSummary.provider(), "Morningstar");
        assert_eq!(ReportType::FinancialSummary.report_type_str(), "finsum");

        assert_eq!(ReportType::FinancialStatements.provider(), "Morningstar");
        assert_eq!(ReportType::FinancialStatements.report_type_str(), "finstat");
    }

    #[test]
    fn fundamental_request_xml_structure() {
        let req = FundamentalRequest {
            con_id: 265598,
            sec_type: "STK",
            currency: "USD",
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

    #[test]
    fn parse_response_rejects_other() {
        assert!(parse_fundamental_response_id("<ResultSetBar>...</ResultSetBar>").is_none());
        assert!(parse_fundamental_response_id("not xml").is_none());
    }

    /// The shape the venue expects, which is a list even when it withdraws one
    /// thing, and names the request by the id the request gave itself.
    #[test]
    fn a_withdrawal_names_the_request_it_withdraws() {
        let xml = crate::control::xml::cancel_query(FUNDAMENTALS_QUERY_ID);
        assert_eq!(
            xml,
            "<ListOfCancelQueries><CancelQuery><id>COMPANY_FUNDAMENTALS</id>\
             </CancelQuery></ListOfCancelQueries>".replace(' ', ""),
        );
        assert!(
            build_fundamental_request_xml(&FundamentalRequest {
                con_id: 265598, sec_type: "STK", currency: "USD",
                report_type: ReportType::Snapshot,
            }).contains(&format!("<id>{FUNDAMENTALS_QUERY_ID}</id>")),
            "the request and the withdrawal name the same thing",
        );
    }
}
