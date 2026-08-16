//! What a subscription says on the wire.

use crate::protocol::fix::{self, fix_build};

/// Build market data subscription request.
pub fn build_mktdata_subscribe(
    con_id: u32,
    exchange: &str,
    sec_type: &str,
    md_req_id: &str,
    seq: u32,
) -> Vec<u8> {
    let con_id_str = con_id.to_string();
    let exchange_fix = match exchange {
        "SMART" => "BEST",
        e => e,
    };
    fix_build(
        &[
            (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
            (262, md_req_id),
            (263, "1"), // Subscribe
            (146, "1"), // NumRelatedSym
            (6008, &con_id_str),
            (207, exchange_fix),
            (167, sec_type),
            (264, "442"), // BidAsk
            (9830, "1"),
        ],
        seq,
    )
}

/// Build market data unsubscribe request.
pub fn build_mktdata_unsubscribe(md_req_id: &str, seq: u32) -> Vec<u8> {
    fix_build(
        &[
            (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ),
            (262, md_req_id),
            (263, "2"), // Unsubscribe
        ],
        seq,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::fix::fix_parse;

    #[test]
    fn build_mktdata_subscribe_structure() {
    let msg = build_mktdata_subscribe(265598, "SMART", "CS", "REQ1", 5);
    let fields = fix_parse(&msg);
    assert_eq!(fields[&35], "V");
    assert_eq!(fields[&262], "REQ1");
    assert_eq!(fields[&263], "1");
    assert_eq!(fields[&6008], "265598");
    assert_eq!(fields[&207], "BEST"); // SMART→BEST
    assert_eq!(fields[&167], "CS");
    }

    #[test]
    fn build_mktdata_unsubscribe_structure() {
    let msg = build_mktdata_unsubscribe("REQ1", 6);
    let fields = fix_parse(&msg);
    assert_eq!(fields[&35], "V");
    assert_eq!(fields[&262], "REQ1");
    assert_eq!(fields[&263], "2");
    }

    #[test]
    fn build_mktdata_subscribe_exchange_passthrough() {
    // Non-SMART exchanges should pass through as-is
    let msg = build_mktdata_subscribe(265598, "ARCA", "CS", "REQ2", 3);
    let fields = fix_parse(&msg);
    assert_eq!(fields[&207], "ARCA"); // not mapped to BEST
    }

    #[test]
    fn build_mktdata_subscribe_has_correct_tags() {
    let msg = build_mktdata_subscribe(756733, "SMART", "ETF", "REQ5", 10);
    let fields = fix_parse(&msg);
    assert_eq!(fields[&35], "V");
    assert_eq!(fields[&6008], "756733");
    assert_eq!(fields[&207], "BEST");
    assert_eq!(fields[&167], "ETF");
    assert_eq!(fields[&263], "1"); // subscribe
    assert_eq!(fields[&146], "1"); // NumRelatedSym
    }
}
