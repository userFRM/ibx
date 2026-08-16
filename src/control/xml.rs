//! Reading and writing the venue's XML.
//!
//! Several of the venue's answers arrive as small XML documents — a
//! fundamentals report, a histogram, a scanner result, a news article, a bar
//! query's own envelope. They are read by name rather than parsed, because
//! what is wanted from each is a handful of known tags and a parser would be a
//! dependency and a shape to keep in step with the venue's. The one document
//! this client writes is here for the same reason.

/// The text between `<tag>` and `</tag>`, or nothing where the pair is absent.
///
/// Borrows from the document rather than copying: these are read once, on the
/// hot loop, and a reply carrying a hundred rows would otherwise allocate a
/// string per field to throw it away again.
///
/// The first pair wins. A document nesting the same name inside itself would
/// need a parser, and none of the replies read here does.
pub fn tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(&xml[start..end])
}

/// Withdraw a query the venue is still answering, by the id it was asked under.
///
/// The venue takes the same document for every kind of query — fundamentals,
/// corporate actions — because what it cancels is the query, not the subject.
pub fn cancel_query(query_id: &str) -> String {
    format!(
        "<ListOfCancelQueries>\
         <CancelQuery>\
         <id>{query_id}</id>\
         </CancelQuery>\
         </ListOfCancelQueries>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What every caller asks of it, and the two ways a document can not
    /// answer: a name that is not there, and one that opens without closing.
    #[test]
    fn a_named_tag_reads_back_and_a_missing_one_reads_as_nothing() {
        let doc = "<HistoricalDataRequest><id>hist_7</id><bar>5 mins</bar></HistoricalDataRequest>";
        assert_eq!(tag(doc, "id"), Some("hist_7"));
        assert_eq!(tag(doc, "bar"), Some("5 mins"));
        assert_eq!(tag(doc, "absent"), None);
        assert_eq!(tag("<id>never closed", "id"), None);
        assert_eq!(tag("<id></id>", "id"), Some(""), "stated and empty is not absent");
    }

    /// The first pair wins, which is what every reply read here needs and all
    /// that this can promise without being a parser.
    #[test]
    fn the_first_pair_is_the_one_read() {
        assert_eq!(tag("<a>one</a><a>two</a>", "a"), Some("one"));
    }
}
