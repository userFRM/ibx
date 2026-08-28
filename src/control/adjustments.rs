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
    /// A dividend paid in cash.
    CashDividend,
    /// A dividend paid in shares.
    StockDividend,
    /// A share split.
    Split,
    /// A business spun out as its own listing.
    SpinOff,
    /// A right to buy new shares.
    RightsOffer,
    /// A future rolling into the next month.
    FutureRollover,
}

impl AdjustmentKind {
    /// The venue's two-letter name for the action, which is the element it
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

    /// Read the kind from the code the venue states it as.
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

}

/// One corporate action.
///
/// The fields an action does not state are left empty rather than invented: a
/// split states no record date, and saying it did would be a date nobody set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Adjustment {
    /// What kind of action it is.
    pub kind: Option<AdjustmentKind>,
    /// When it takes effect.
    pub date: String,
    /// What the venue states for it.
    pub value: String,
    /// What currency that is in.
    pub currency: String,
    /// When it was announced.
    pub announce_date: String,
    /// The day the holder of record is fixed.
    pub record_date: String,
    /// The day it is paid.
    pub pay_date: String,
    /// Whether a dividend was the regular one or a special.
    pub payment_type: String,
    /// What a dividend was paid out of — income, capital gain, or unstated.
    pub distribution_type: String,
}

/// What a caller asks for.
#[derive(Debug, Clone)]
pub struct AdjustmentRequest {
    /// The name this client gave the query, which the answer echoes.
    pub query_id: String,
    /// The venue's id for the contract.
    pub con_id: u32,
    /// What kind of contract it is.
    pub sec_type: String,
    /// Which venue.
    pub exchange: String,
    /// Both dates as the venue states them back, so a caller can hand back what
    /// it was given.
    pub start_date: String,
    /// Its end.
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

/// The contract the actions belong to, echoed back beside them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdjustedContract {
    /// The venue's id for the contract.
    pub con_id: String,
    /// Its ticker.
    pub symbol: String,
    /// Which venue.
    pub exchange: String,
}

/// Every corporate action in a reply, and the contract they belong to.
///
/// The venue answers a name on its own line and the rows under it, until the
/// next name: the query comes back echoed as XML and the actions arrive beside
/// it as text. An action stating fewer values than its kind names is kept with
/// the rest empty rather than dropped, because a partial answer is still the
/// venue's answer, and discarding it would leave a series looking unadjusted
/// for a reason nobody could see.
pub fn parse_adjustments(body: &str) -> (AdjustedContract, Vec<Adjustment>) {
    let mut contract = AdjustedContract::default();
    let mut out = Vec::new();
    // A name on its own line, then the records under it until the next name.
    // The reply carries no markup around them: the query is echoed as XML and
    // the actions arrive beside it as text, which is what a live session was
    // answered with.
    let mut under: Option<&str> = None;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.contains(',') {
            under = Some(line);
            continue;
        }
        let values: Vec<&str> = line.split(',').map(str::trim).collect();
        match under {
            // The contract states its own id first, then what the row is for.
            Some("conc") => contract.con_id = values[0].to_string(),
            Some("consym") => contract.symbol = values.last().unwrap_or(&"").to_string(),
            // Its id, the venue it is listed on, and the date that listing began.
            Some("conexch") => {
                if let Some(exchange) = values.get(1) {
                    contract.exchange = (*exchange).to_string();
                }
            }
            Some(code) => {
                let kind = AdjustmentKind::from_code(code);
                if kind.is_none() {
                    // A name this client does not know: its rows are left
                    // rather than read under the wrong kind.
                    continue;
                }
                let mut a = Adjustment { kind, ..Default::default() };
                // Stated in one fixed order, so a value is read by where it sits.
                for (i, v) in values.iter().enumerate() {
                    let v = (*v).to_string();
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
            None => {}
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date));
    (contract, out)
}

/// What a price before `date` must be multiplied by to sit on the same scale
/// as prices after every action in `actions`.
///
/// A split is the only action here that moves the scale, and its value is the
/// ratio: a share split ten for one states ten, and a price from before that
/// day is a tenth of what it reads once the split has happened. Established
/// against a contract that split ten for one on a stated day, where the close
/// before it was 1208.88 and the close after was 121.79: dividing the first by
/// the stated ten gives 120.89, which is the same scale as the second.
///
/// A dividend does not move the scale. It is a payment out of the price rather
/// than a restatement of it, and how much of one to take off a historical
/// price is a convention this client has not established, so it takes none:
/// a number nobody can check is worse than one nobody applied.
pub fn scale_before(date: &str, actions: &[Adjustment]) -> f64 {
    let mut factor = 1.0;
    for a in actions {
        if a.kind != Some(AdjustmentKind::Split) || a.date.as_str() <= date {
            continue;
        }
        if let Ok(ratio) = a.value.parse::<f64>()
            && ratio > 0.0
        {
            factor /= ratio;
        }
    }
    factor
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A price from before a split reads on the scale after it.
    ///
    /// The venue stated a ten-for-one split on a day, and a bar series across
    /// that day closed at 1208.88 the session before and 121.79 the session
    /// after: a tenfold step with nothing in the series saying so. Scaled by
    /// what the split states, the earlier close reads 120.89, which is the
    /// later one's scale. That is the whole of what an adjusted series is, and
    /// both numbers are ones a session was answered with.
    #[test]
    fn a_price_from_before_a_split_reads_on_the_scale_after_it() {
        let answered = "conc\n4815747,-1,-1\n\
                        CD\n20240305,0.04,USD,20240221,20240306,20240327,R,NA\n\
                        SS\n20240610,10,,20240522\n";
        let (_, actions) = parse_adjustments(answered);
        let before = 1208.88 * scale_before("20240607", &actions);
        assert!((before - 120.888).abs() < 0.001, "scaled to {before}, not 120.888");
        let after = 121.79 * scale_before("20240611", &actions);
        assert!((after - 121.79).abs() < 0.001, "a price after the split is unmoved");
        assert!(
            (before - 121.79).abs() / 121.79 < 0.01,
            "and the sessions either side sit within a day's move of each other",
        );
    }

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

    /// The reply carries its actions in the raw field, and parsing keeps them.
    ///
    /// The query comes back echoed as XML on one tag and the actions arrive on
    /// another, under the length that precedes it. Nothing here would work if
    /// the length-prefixed field did not survive being parsed, so that is
    /// asserted rather than assumed: this is the message a live session was
    /// answered with, put back together and read the way the engine reads one.
    #[test]
    fn the_actions_survive_being_parsed_out_of_the_reply() {
        let payload = "conc\n756733,-1,-1\nconexch\n756733,AMEX,20090223\nCD\n\
20240315,1.594937,USD,20240314,20240318,20240430,R,NA\n";
        let msg = format!(
            "8=FIX.4.1\x019=000355\x0135=U\x016040=10022\x016118=<ConAdjResponse>\
             <id>adj_1</id></ConAdjResponse>\x0195={}\x0196={}\x0110=200\x01",
            payload.len(), payload,
        );
        let parsed = crate::protocol::fix::fix_parse(msg.as_bytes());
        let carried = parsed.get(&96).expect("the actions are on the raw field");
        assert_eq!(carried.len(), payload.len(), "and the whole of it is kept");
        let (contract, adj) = parse_adjustments(carried);
        assert_eq!(contract.con_id, "756733");
        assert_eq!(adj.len(), 1, "the action in it is read");
    }

    /// What a live session was answered with, kept as it arrived.
    ///
    /// Asked for one contract's actions over 2024, the venue answered a name on
    /// its own line and the rows under it. The contract states its id and the
    /// venue it is listed on; each dividend states its date, what it paid, in
    /// what currency, and the three dates around it.
    #[test]
    fn what_the_venue_answered_is_read() {
        let answered = "conc\n756733,-1,-1\n\
                        conexch\n756733,AMEX,20090223\n\
                        CD\n\
                        20240315,1.594937,USD,20240314,20240318,20240430,R,NA\n\
                        20240621,1.759024,USD,20240620,20240621,20240731,R,NA\n\
                        20240920,1.745531,USD,20240919,20240920,20241031,R,NA\n\
                        20241220,1.965548,USD,20241219,20241220,20250131,R,NA\n";
        let (contract, adj) = parse_adjustments(answered);
        assert_eq!(contract.con_id, "756733", "the contract names itself first");
        assert_eq!(contract.exchange, "AMEX", "and the venue it is listed on");
        assert_eq!(adj.len(), 4, "four dividends were paid over the year asked for");
        assert_eq!(adj[0].kind, Some(AdjustmentKind::CashDividend));
        assert_eq!(adj[0].date, "20240315");
        assert_eq!(adj[0].value, "1.594937");
        assert_eq!(adj[0].currency, "USD");
        assert_eq!(adj[0].pay_date, "20240430");
        assert_eq!(adj[3].date, "20241220", "and they come back in date order");
    }
}
