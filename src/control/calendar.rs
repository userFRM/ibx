//! The corporate-events calendar: what a company is about to do.
//!
//! Earnings dates, splits, dividends, meetings and the rest, from a vendor the
//! venue carries rather than from the venue itself. Two requests: what event
//! types exist, and the events themselves for contracts a caller names.
//!
//! Both ride the same envelope — one message type, one sub-protocol, told
//! apart by a number and carrying a JSON document — and both are answered on
//! that envelope too, with the answer under one number and a refusal under
//! another.
//!
//! Neither depends on the other. An event request that no metadata request
//! preceded is answered — measured on a session, which is why nothing here
//! stands in front of one.

// The query a request is built from sits beside the command that carries
// it. Reachable here because that is the path a program written against this
// client already names.
pub use crate::types::CalendarQuery;

/// The sub-protocol both requests and both answers travel under.
pub const CALENDAR_SUB_PROTOCOL: u32 = 155;

/// The tag saying which of the two requests this is.
pub const TAG_CALENDAR_REQUEST_KIND: u32 = 8081;

/// The tag carrying the request's JSON document.
pub const TAG_CALENDAR_JSON: u32 = 8082;

/// The tag a request states its own name under, and an answer echoes.
pub const TAG_CALENDAR_KEY: u32 = 6556;

/// Asking what event types exist.
pub const CALENDAR_META_DATA: u32 = 100;

/// Asking for the events themselves.
pub const CALENDAR_EVENT_DATA: u32 = 101;

/// The answer.
pub const CALENDAR_ANSWER: &str = "158";

/// A refusal, whose words are on tag 58.
pub const CALENDAR_REFUSAL: &str = "159";

/// What this client can be sent, as the venue states it: three bits, set.
///
/// Sent on both requests. It is the venue's encoding of what a caller can
/// be given, and a request that states less is answered with less.
const CLIENT_CAPABILITY: &str = "Bw==";

/// The vendor whose calendar this is.
const CALENDAR_SOURCE: &str = "WSHE";

/// The JSON asking what event types exist.
///
/// It carries no filters of any kind: the answer is the same for everybody.
pub fn meta_data_request() -> String {
    format!(
        r#"{{"T":{CALENDAR_META_DATA},"V":1,"P":{{"calendar_request":{{"client_capability":"{CLIENT_CAPABILITY}"}}}}}}"#
    )
}

/// The JSON asking for events.
///
/// A caller either writes a filter or names a contract. Naming a contract
/// becomes a watchlist of one, with the contract written as text inside an
/// array, which is how the venue reads it.
///
/// Nothing is stated where a caller stated nothing: a key with an empty value
/// is left out rather than sent empty.
pub fn event_data_request(query: &CalendarQuery) -> Result<String, String> {
    let filter = if !query.filter.trim().is_empty() {
        query.filter.trim().to_string()
    } else if let Some(con_id) = query.con_id {
        format!(r#"{{"watchlist":["{con_id}"]}}"#)
    } else {
        return Err(
            "a calendar request needs either a filter or a contract to fetch events for"
                .to_string(),
        );
    };

    let mut parts = vec![format!(r#""sources":["{CALENDAR_SOURCE}"]"#)];

    let mut dates: Vec<String> = Vec::new();
    if !query.start_date.trim().is_empty() {
        dates.push(format!(r#""start":"{}""#, query.start_date.trim()));
    }
    if !query.end_date.trim().is_empty() {
        dates.push(format!(r#""end":"{}""#, query.end_date.trim()));
    }
    // Always stated, empty where the caller bounded nothing. The window and
    // the account are present on every request whether or not either holds
    // anything; omitting them makes a different document.
    parts.push(format!(r#""date":{{{}}}"#, dates.join(",")));
    parts.push(r#""account":"""#.to_string());

    parts.push(format!(r#""filters":{filter}"#));
    parts.push(r#""api":true"#.to_string());
    parts.push(format!(r#""fill_watchlist":{}"#, query.fill_watchlist));
    parts.push(format!(r#""fill_portfolio":{}"#, query.fill_portfolio));
    parts.push(format!(r#""fill_competitors":{}"#, query.fill_competitors));
    parts.push(r#""mode":"chronological""#.to_string());
    if let Some(limit) = query.total_limit {
        // Stated as text. A bare number is a different document.
        parts.push(format!(r#""total_limit":"{limit}""#));
    }
    parts.push(format!(r#""client_capability":"{CLIENT_CAPABILITY}""#));

    Ok(format!(
        r#"{{"T":{CALENDAR_EVENT_DATA},"V":1,"P":{{"calendar_request":{{{}}}}}}}"#,
        parts.join(","),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The metadata request carries no filters. The answer is the same for
    /// everybody, so there is nothing to narrow.
    #[test]
    fn the_metadata_request_states_only_what_this_client_can_take() {
        let json = meta_data_request();
        assert!(json.contains(r#""T":100"#), "{json}");
        assert!(json.contains(r#""client_capability":"Bw==""#), "{json}");
        assert!(!json.contains("filters"), "nothing to filter: {json}");
    }

    /// Naming a contract becomes a watchlist of one, with the contract written
    /// as text inside an array. Written as a bare number it is a different
    /// document and the venue reads no contract at all.
    #[test]
    fn a_named_contract_becomes_a_watchlist_of_one() {
        let json = event_data_request(&CalendarQuery { con_id: Some(265598), ..Default::default() })
            .expect("a contract is enough to ask with");
        assert!(json.contains(r#""filters":{"watchlist":["265598"]}"#), "{json}");
        assert!(json.contains(r#""T":101"#), "{json}");
        assert!(json.contains(r#""sources":["WSHE"]"#), "{json}");
    }

    /// A caller's own filter goes as written. The venue validates it, not this
    /// client, and rewriting it would change what was asked.
    #[test]
    fn a_callers_filter_goes_as_written() {
        let json = event_data_request(&CalendarQuery {
            filter: r#"{"portfolio":true,"other":false}"#.to_string(),
            ..Default::default()
        })
        .expect("a filter is enough to ask with");
        assert!(json.contains(r#""filters":{"portfolio":true,"other":false}"#), "{json}");
    }

    /// The window and the account are stated on every request, empty where
    /// the caller bounded nothing. A limit the caller did not set is left
    /// out.
    #[test]
    fn nothing_stated_is_nothing_sent() {
        let json = event_data_request(&CalendarQuery { con_id: Some(1), ..Default::default() })
            .expect("asks");
        assert!(json.contains(r#""date":{}"#), "an unbounded window is still stated: {json}");
        assert!(json.contains(r#""account":"""#), "the account is stated: {json}");
        assert!(!json.contains("total_limit"), "an unset limit was stated: {json}");

        let bounded = event_data_request(&CalendarQuery {
            con_id: Some(1),
            start_date: "20260810".into(),
            end_date: "20260910".into(),
            total_limit: Some(50),
            ..Default::default()
        })
        .expect("asks");
        assert!(bounded.contains(r#""date":{"start":"20260810","end":"20260910"}"#), "{bounded}");
        // Text, not a bare number.
        assert!(bounded.contains(r#""total_limit":"50""#), "{bounded}");
    }

    /// A request with neither a filter nor a contract is refused here rather
    /// than sent. The venue would answer it with everything or with nothing,
    /// and neither is what the caller meant.
    #[test]
    fn asking_for_nothing_in_particular_is_refused() {
        assert!(event_data_request(&CalendarQuery::default()).is_err());
    }
}
