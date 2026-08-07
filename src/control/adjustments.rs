//! Corporate actions on a contract — the splits and dividends that make a
//! historical price series adjusted rather than raw.
//!
//! A caller asks once for a date range and the venue answers with every action
//! in it; the subscription is withdrawn by the id it was asked under. Without
//! this a price series crosses a split unadjusted, which is not a missing
//! feature but a wrong number, and nothing in the series says so.

/// What a corporate action did to the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustmentKind {
    CashDividend,
    StockDividend,
    Split,
    SpinOff,
    RightsOffer,
    FutureRollover,
}

impl AdjustmentKind {
    /// The venue's own two-letter name for the action, which is the element it
    /// arrives under.
    pub fn code(self) -> &'static str {
        match self {
            Self::CashDividend => "CD",
            Self::StockDividend => "SD",
            Self::Split => "SS",
            Self::SpinOff => "SO",
            Self::RightsOffer => "RO",
            Self::FutureRollover => "FR",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "CD" => Self::CashDividend,
            "SD" => Self::StockDividend,
            "SS" => Self::Split,
            "SO" => Self::SpinOff,
            "RO" => Self::RightsOffer,
            "FR" => Self::FutureRollover,
            _ => return None,
        })
    }

    /// How many values the action states. A cash dividend states the most; a
    /// rollover states none at all.
    pub fn field_count(self) -> usize {
        match self {
            Self::CashDividend => 8,
            Self::StockDividend => 6,
            Self::Split | Self::SpinOff | Self::RightsOffer => 4,
            Self::FutureRollover => 0,
        }
    }
}

/// One corporate action.
///
/// The fields an action does not state are left empty rather than invented: a
/// split states no record date, and saying it did would be a date nobody set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Adjustment {
    pub kind: Option<AdjustmentKind>,
    pub date: String,
    pub value: String,
    pub currency: String,
    pub announce_date: String,
    pub record_date: String,
    pub pay_date: String,
    /// Whether a dividend was the regular one or a special.
    pub payment_type: String,
    /// What a dividend was paid out of — income, capital gain, or unstated.
    pub distribution_type: String,
}

/// What a caller asks for.
#[derive(Debug, Clone)]
pub struct AdjustmentRequest {
    pub query_id: String,
    pub con_id: u32,
    pub sec_type: String,
    pub exchange: String,
    /// Both dates as the venue states them back, so a caller can hand back what
    /// it was given.
    pub start_date: String,
    pub end_date: String,
}

/// Ask for every corporate action on a contract in a date range.
///
/// The dividend request type is stated as `T`, which is the whole of them. The
/// venue names two narrower kinds beside it; nothing here asks for a subset, so
/// nothing here sends one.
pub fn build_adjustments_request_xml(req: &AdjustmentRequest) -> String {
    format!(
        "<ListOfQueries>\
         <ConAdjQuery>\
         <id>{id}</id>\
         <contractID>{con_id}</contractID>\
         <exchange>{exchange}</exchange>\
         <secType>{sec_type}</secType>\
         <startDate>{start}</startDate>\
         <endDate>{end}</endDate>\
         <divRequestType>T</divRequestType>\
         </ConAdjQuery>\
         </ListOfQueries>",
        id = req.query_id,
        con_id = req.con_id,
        exchange = req.exchange,
        sec_type = req.sec_type,
        start = req.start_date,
        end = req.end_date,
    )
}

/// Withdraw the subscription, which is named by the id it was asked under and
/// by nothing else — not the contract, not the range.
pub fn build_adjustments_cancel_xml(query_id: &str) -> String {
    format!(
        "<ListOfCancelQueries>\
         <CancelQuery>\
         <id>{query_id}</id>\
         </CancelQuery>\
         </ListOfCancelQueries>",
    )
}

/// The contract the actions belong to, echoed back beside them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdjustedContract {
    pub con_id: String,
    pub symbol: String,
    pub exchange: String,
}

fn element_body<'a>(xml: &'a str, tag: &str, from: usize) -> Option<(&'a str, usize)> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml[from..].find(&open)? + from + open.len();
    let end = xml[start..].find(&close)? + start;
    Some((&xml[start..end], end + close.len()))
}

/// Every corporate action in a reply, and the contract they belong to.
///
/// Each action is its own element named for the kind, and its body is the
/// values in one comma-separated run. An action stating fewer values than its
/// kind names is kept with the rest empty rather than dropped: a partial answer
/// from the venue is still the venue's answer, and discarding it would leave a
/// series looking unadjusted for a reason nobody could see.
pub fn parse_adjustments(xml: &str) -> (AdjustedContract, Vec<Adjustment>) {
    let mut contract = AdjustedContract::default();
    if let Some((v, _)) = element_body(xml, "conc", 0) {
        contract.con_id = v.trim().to_string();
    }
    if let Some((v, _)) = element_body(xml, "consym", 0) {
        contract.symbol = v.trim().to_string();
    }
    if let Some((v, _)) = element_body(xml, "conexch", 0) {
        contract.exchange = v.trim().to_string();
    }

    let mut out = Vec::new();
    for code in ["CD", "SD", "SS", "SO", "RO", "FR"] {
        let kind = AdjustmentKind::from_code(code);
        let mut at = 0usize;
        while let Some((body, next)) = element_body(xml, code, at) {
            at = next;
            let mut a = Adjustment { kind, ..Default::default() };
            let values: Vec<&str> = if body.trim().is_empty() {
                Vec::new()
            } else {
                body.split(',').collect()
            };
            // Stated in one fixed order, so a value is read by where it sits.
            for (i, v) in values.iter().enumerate() {
                let v = v.trim().to_string();
                match i {
                    0 => a.date = v,
                    1 => a.value = v,
                    2 => a.currency = v,
                    3 => a.announce_date = v,
                    4 => a.record_date = v,
                    5 => a.pay_date = v,
                    6 => a.payment_type = v,
                    7 => a.distribution_type = v,
                    _ => {}
                }
            }
            out.push(a);
        }
    }
    out
        .sort_by(|a, b| a.date.cmp(&b.date));
    (contract, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_names_the_contract_and_the_range() {
        let xml = build_adjustments_request_xml(&AdjustmentRequest {
            query_id: "ADJ.1".into(),
            con_id: 756733,
            sec_type: "STK".into(),
            exchange: "SMART".into(),
            start_date: "20240101".into(),
            end_date: "20241231".into(),
        });
        assert!(xml.contains("<id>ADJ.1</id>"));
        assert!(xml.contains("<contractID>756733</contractID>"));
        assert!(xml.contains("<secType>STK</secType>"));
        assert!(xml.contains("<startDate>20240101</startDate>"));
        assert!(xml.contains("<endDate>20241231</endDate>"));
        assert!(xml.contains("<divRequestType>T</divRequestType>"));
    }

    /// The withdrawal names the request and nothing else.
    #[test]
    fn a_cancel_carries_only_the_id_it_was_asked_under() {
        let xml = build_adjustments_cancel_xml("ADJ.1");
        assert!(xml.contains("<id>ADJ.1</id>"));
        assert!(!xml.contains("contractID"));
        assert!(!xml.contains("startDate"));
    }

    /// Each kind states a different number of values, in one order, and the
    /// ones it does not state stay empty rather than shifting the rest along.
    #[test]
    fn each_kind_reads_the_values_it_states() {
        let xml = "<conc>756733</conc><consym>SPY</consym><conexch>ARCA</conexch>\
                   <CD>20240315,1.59,USD,20240301,20240315,20240425,REGULAR,INCOME</CD>\
                   <SS>20240610,4.0,USD,20240520</SS>\
                   <FR></FR>";
        let (contract, adj) = parse_adjustments(xml);
        assert_eq!(contract.symbol, "SPY");
        assert_eq!(contract.con_id, "756733");

        let cd = adj.iter().find(|a| a.kind == Some(AdjustmentKind::CashDividend)).unwrap();
        assert_eq!(cd.date, "20240315");
        assert_eq!(cd.value, "1.59");
        assert_eq!(cd.currency, "USD");
        assert_eq!(cd.record_date, "20240315");
        assert_eq!(cd.pay_date, "20240425");
        assert_eq!(cd.payment_type, "REGULAR");
        assert_eq!(cd.distribution_type, "INCOME");

        let ss = adj.iter().find(|a| a.kind == Some(AdjustmentKind::Split)).unwrap();
        assert_eq!(ss.value, "4.0");
        assert_eq!(ss.announce_date, "20240520");
        // A split states no record or pay date, and none is invented for it.
        assert!(ss.record_date.is_empty() && ss.pay_date.is_empty());

        assert!(adj.iter().any(|a| a.kind == Some(AdjustmentKind::FutureRollover)));
    }

    /// A dividend paid out of something the venue does not name arrives with
    /// that value empty, which is a value, not a short record.
    #[test]
    fn an_unstated_distribution_is_empty_rather_than_missing() {
        let (_, adj) = parse_adjustments("<CD>20240315,1.59,USD,20240301,20240315,20240425,SPECIAL,</CD>");
        assert_eq!(adj.len(), 1);
        assert_eq!(adj[0].payment_type, "SPECIAL");
        assert!(adj[0].distribution_type.is_empty());
    }

    /// Several actions of the same kind are several records.
    #[test]
    fn repeated_actions_are_all_kept() {
        let (_, adj) = parse_adjustments(
            "<CD>20240315,1.59,USD,20240301,20240315,20240425,REGULAR,INCOME</CD>\
             <CD>20240615,1.61,USD,20240601,20240615,20240725,REGULAR,INCOME</CD>",
        );
        assert_eq!(adj.len(), 2);
        assert_eq!(adj[0].date, "20240315");
        assert_eq!(adj[1].date, "20240615");
    }
}
