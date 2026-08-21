//! What a subscription says on the wire.
//!
//! A market-data request is an action, a count, and that many request rows.
//! The action and the count come first and each row states the contract it is
//! about and the one thing wanted of it, so asking for two kinds of tick is
//! two rows rather than one row saying both. A withdrawal is the same shape as
//! the subscription it withdraws rather than an id on its own.

use crate::protocol::fix::{self, fix_build};

/// One request row: which request, which contract, and which tick.
fn request_row(
    con_id: u32,
    exchange: &str,
    sec_type: &str,
    md_req_id: &str,
    tick_type: u32,
    top_quote: bool,
) -> Vec<(u32, String)> {
    let exchange_fix = match exchange {
        "SMART" => "BEST",
        e => e,
    };
    let mut row = vec![
        (262, md_req_id.to_string()),
        (6008, con_id.to_string()),
        (207, exchange_fix.to_string()),
        (167, sec_type.to_string()),
        // Which tick this row is for, as the caller asked. Fixed at 442 it
        // asked for a quote and nothing else, so a caller wanting trades got
        // a subscription to something they did not ask for.
        (264, tick_type.to_string()),
    ];
    // Stated only when it is asked for. Written on every subscription, this
    // said something about every request that only some requests say.
    if top_quote {
        row.push((9830, "1".to_string()));
    }
    row
}

fn build_request(
    action: &str,
    con_id: u32,
    exchange: &str,
    sec_type: &str,
    md_req_id: &str,
    now: &str,
    tick_types: &[u32],
    top_quote: bool,
    seq: u32,
) -> Vec<u8> {
    let mut fields: Vec<(u32, String)> = vec![
        (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
        (fix::TAG_SENDING_TIME, now.to_string()),
        (263, action.to_string()),
        (146, tick_types.len().to_string()),
    ];
    for &tick_type in tick_types {
        fields.extend(request_row(con_id, exchange, sec_type, md_req_id, tick_type, top_quote));
    }
    fix_build(
        &fields.iter().map(|(t, v)| (*t, v.as_str())).collect::<Vec<_>>(),
        seq,
    )
}

/// Build a market data subscription request.
///
/// `now` is the sending time, tag 52.
/// `tick_types` are the venue's tick numbers — 442 for a quote, 443 for
/// the last trade, 1 for the top of book, 292 for news — one row each.
/// `top_quote` asks for the top of book alone.
#[allow(clippy::too_many_arguments)]
pub fn build_mktdata_subscribe(
    con_id: u32,
    exchange: &str,
    sec_type: &str,
    md_req_id: &str,
    now: &str,
    tick_types: &[u32],
    top_quote: bool,
    seq: u32,
) -> Vec<u8> {
    build_request("1", con_id, exchange, sec_type, md_req_id, now, tick_types, top_quote, seq)
}

/// Build a market data unsubscribe request.
///
/// The same rows the subscription stated. A withdrawal naming only the request
/// id carries no contract and no count, which is not the shape the protocol
/// defines for one.
#[allow(clippy::too_many_arguments)]
pub fn build_mktdata_unsubscribe(
    con_id: u32,
    exchange: &str,
    sec_type: &str,
    md_req_id: &str,
    now: &str,
    tick_types: &[u32],
    top_quote: bool,
    seq: u32,
) -> Vec<u8> {
    build_request("2", con_id, exchange, sec_type, md_req_id, now, tick_types, top_quote, seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tags in wire order: the action and the count precede the row that
    /// names the contract. A map would hide the ordering.
    fn tags(msg: &[u8]) -> Vec<(u32, String)> {
        String::from_utf8_lossy(msg)
            .split('\u{1}')
            .filter_map(|f| f.split_once('='))
            .map(|(t, v)| (t.parse::<u32>().unwrap_or(0), v.to_string()))
            .collect()
    }

    fn order_of(msg: &[u8], want: &[u32]) -> Vec<u32> {
        tags(msg).into_iter().map(|(t, _)| t).filter(|t| want.contains(t)).collect()
    }

    #[test]
    fn a_subscription_states_its_action_before_its_row() {
        let msg = build_mktdata_subscribe(265598, "SMART", "CS", "REQ1", "20260101-12:00:00", &[442], false, 5);
        assert_eq!(
            order_of(&msg, &[35, 34, 52, 263, 146, 262, 6008, 207, 167, 264]),
            vec![35, 34, 52, 263, 146, 262, 6008, 207, 167, 264],
        );
        let fields = tags(&msg);
        let get = |t: u32| fields.iter().find(|(tag, _)| *tag == t).map(|(_, v)| v.clone());
        assert_eq!(get(207).as_deref(), Some("BEST"), "SMART is named BEST");
        assert_eq!(get(167).as_deref(), Some("CS"));
        assert_eq!(get(146).as_deref(), Some("1"));
    }

    /// A withdrawal is the shape of the subscription it withdraws. Sent as an
    /// id alone it named no contract and no count.
    #[test]
    fn an_unsubscribe_states_the_row_it_withdraws() {
        let msg = build_mktdata_unsubscribe(265598, "SMART", "CS", "REQ1", "20260101-12:00:00", &[442], false, 6);
        assert_eq!(
            order_of(&msg, &[35, 34, 52, 263, 146, 262, 6008, 207, 167, 264]),
            vec![35, 34, 52, 263, 146, 262, 6008, 207, 167, 264],
        );
        let fields = tags(&msg);
        let get = |t: u32| fields.iter().find(|(tag, _)| *tag == t).map(|(_, v)| v.clone());
        assert_eq!(get(263).as_deref(), Some("2"), "withdrawn, not subscribed");
        assert_eq!(get(6008).as_deref(), Some("265598"), "the contract it was about");
        assert_eq!(get(146).as_deref(), Some("1"));
    }

    /// Tag 9830 is stated only when it is asked for. Written on every
    /// subscription, it said something about every request that only some
    /// requests say.
    #[test]
    fn the_top_quote_flag_is_stated_only_when_asked_for() {
        let asked = build_mktdata_subscribe(265598, "SMART", "CS", "REQ1", "20260101-12:00:00", &[442], true, 5);
        assert!(tags(&asked).iter().any(|(t, v)| *t == 9830 && v == "1"));
        let not = build_mktdata_subscribe(265598, "SMART", "CS", "REQ1", "20260101-12:00:00", &[442], false, 5);
        assert!(!tags(&not).iter().any(|(t, _)| *t == 9830));
    }

    /// Two kinds of tick are two rows, and tag 146 states the count. One row
    /// carrying both is not a valid request shape.
    #[test]
    fn asking_for_two_ticks_states_two_rows() {
        let msg = build_mktdata_subscribe(
            265598, "SMART", "CS", "REQ1", "20260101-12:00:00", &[442, 443], false, 5,
        );
        let fields = tags(&msg);
        assert_eq!(
            fields.iter().find(|(t, _)| *t == 146).map(|(_, v)| v.as_str()),
            Some("2"),
        );
        let ticks: Vec<&str> =
            fields.iter().filter(|(t, _)| *t == 264).map(|(_, v)| v.as_str()).collect();
        assert_eq!(ticks, vec!["442", "443"]);
        assert_eq!(fields.iter().filter(|(t, _)| *t == 6008).count(), 2, "one row each");
    }
}
