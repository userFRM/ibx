//! Which server serves which market, as the venue states it.
//!
//! A session is routed to three farms at logon — trading, market data and
//! security definitions — and everything else has to be looked up. The venue
//! sends that lookup unprompted after a farm logon: a table of every market it
//! serves, what kind of data it serves there, and the host, port and farm name
//! that serve it.
//!
//! Guessing instead does not work, and fails silently. Farms are spread across
//! the venue's servers, and a farm asked for on the wrong one accepts the
//! connection, says nothing, and closes it about ten seconds later. Nothing in
//! that exchange distinguishes a farm this account cannot use from one that is
//! simply somewhere else.
//!
//! ```text
//! AEB,IOPT,Top|Deep,-1,*,<host>,4000,eufarmnj
//! ```
//!
//! Exchange, security type, the endpoints served there, a depth identifier,
//! a qualifier, then where to ask.

use std::collections::{BTreeSet, HashMap};

/// What the venue calls a book, richest first.
///
/// A market listing more than one is offering the better of them, and this is
/// the order the counterpart asks in.
const BOOK_ENDPOINTS: [&str; 4] = ["AggDeep", "Deep2", "DeepX", "Deep"];

/// One market, and where to ask about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The exchange this row is for, as the venue names it.
    pub exchange: String,
    /// The security type, in the venue's own spelling.
    pub sec_type: String,
    /// What can be asked for here: `Top`, `Deep`, `Frz`, `AggDeep` and the
    /// rest, or `*` for every endpoint.
    pub endpoints: Vec<String>,
    /// Which book this row is about, where a market has more than one. `-1`
    /// for the market's own.
    pub book: i32,
    /// A narrower market within the exchange, or `*` for all of it.
    pub qualifier: String,
    /// The server that serves it.
    pub host: String,
    /// The port on that server.
    pub port: u16,
    /// The farm to log on to there.
    pub farm: String,
}

impl Route {
    /// Whether this row answers for an endpoint.
    ///
    /// A row listing `*` answers for every endpoint, which is what the venue
    /// uses for a market whose data all comes from one place.
    pub fn serves(&self, endpoint: &str) -> bool {
        self.endpoints.iter().any(|e| e == "*" || e == endpoint)
    }
}

/// Every market the venue serves this session, and where.
#[derive(Debug, Clone, Default)]
pub struct RoutingTable {
    rows: Vec<Route>,
}

impl RoutingTable {
    /// Read a table out of the answer to a routing request.
    ///
    /// Rows are separated by `;` and fields by `,`. A row that is not eight
    /// fields is skipped rather than guessed at — the last row of a frame
    /// carries the frame's own trailer behind it, and a row this client cannot
    /// read is one row rather than a reason to discard the table.
    pub fn parse(body: &str) -> Self {
        let rows = body
            .split(';')
            .filter_map(Self::parse_row)
            .collect();
        Self { rows }
    }

    fn parse_row(row: &str) -> Option<Route> {
        // The frame's trailer rides on the last row; the row ends where the
        // FIX field separator begins.
        let row = row.split('\x01').next()?.trim();
        if row.is_empty() {
            return None;
        }
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 8 {
            return None;
        }
        Some(Route {
            exchange: f[0].to_string(),
            sec_type: f[1].to_string(),
            endpoints: f[2].split('|').map(|e| e.to_string()).collect(),
            // Skipped rather than defaulted: -1 is the venue's own value for
            // a market's own book, so a row whose book cannot be read would
            // otherwise be reported as naming one.
            book: f[3].parse().ok()?,
            qualifier: f[4].to_string(),
            host: f[5].to_string(),
            port: f[6].parse().ok()?,
            farm: f[7].to_string(),
        })
    }

    /// Every row read.
    pub fn rows(&self) -> &[Route] {
        &self.rows
    }

    /// Whether anything was read at all.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Where to ask about a contract, for one kind of data.
    ///
    /// The first row that names the exchange, the security type and the
    /// endpoint, and applies to the whole market. Order is the venue's, and a
    /// market served from more than one place lists the one it prefers first.
    ///
    /// A row carrying a qualifier is about one sub-market and is not an answer
    /// for a contract that is not in it. The venue serves an aggregated book
    /// on its smart destination for two such sub-markets and for no other
    /// share; matched without reading the qualifier, those two rows say every
    /// share on that destination has a book, which is the opposite of what the
    /// table says.
    pub fn find(&self, exchange: &str, sec_type: &str, endpoint: &str) -> Option<&Route> {
        self.rows.iter().find(|r| {
            r.exchange == exchange
                && r.sec_type == sec_type
                && r.qualifier == "*"
                && r.serves(endpoint)
        })
    }

    /// The same, for a contract known to be in a sub-market the venue names.
    ///
    /// Only a caller that knows which one can ask this; the qualifier is the
    /// venue's own name for it.
    pub fn find_within(
        &self, exchange: &str, sec_type: &str, endpoint: &str, qualifier: &str,
    ) -> Option<&Route> {
        self.rows.iter().find(|r| {
            r.exchange == exchange
                && r.sec_type == sec_type
                && r.qualifier == qualifier
                && r.serves(endpoint)
        })
    }

    /// The name the venue serves a book under for this market, if it serves one.
    ///
    /// There is more than one. A book is `Deep` on most markets, `Deep2` on
    /// some, `DeepX` on others, and `AggDeep` where the venue aggregates one —
    /// and a market that serves one of them need not serve the others. Two
    /// markets on a live session serve a book under no other name than `Deep2`
    /// and `DeepX`, so asking only about `Deep` says they have none.
    ///
    /// Which name is the venue's to state, so it is read off the endpoints it
    /// listed rather than assumed.
    ///
    /// Where a market serves more than one, the richest is taken first: an
    /// aggregated book, then the second-generation one, then the exchange's
    /// own, then the plain one. That is the order the counterpart offers a
    /// contract, and it asks in that order for the same reason — a market
    /// listing both `Deep2` and `Deep` is offering the better of the two.
    pub fn book_endpoint(&self, exchange: &str, sec_type: &str) -> Option<&str> {
        const RICHEST_FIRST: [&str; 4] = BOOK_ENDPOINTS;
        let served = |name: &str| {
            self.rows.iter().any(|r| {
                r.exchange == exchange
                    && r.sec_type == sec_type
                    && r.qualifier == "*"
                    && r.serves(name)
            })
        };
        RICHEST_FIRST.into_iter().find(|name| served(name))
    }

    /// The farms that serve a book, by name.
    ///
    /// A book is asked for on the connection this session holds, and the table
    /// states which farm serves each one. A book whose farm is not the farm
    /// this session is on is asked of a server that does not serve it.
    pub fn book_farms(&self) -> BTreeSet<&str> {
        self.rows
            .iter()
            .filter(|r| BOOK_ENDPOINTS.iter().any(|name| r.serves(name)))
            .map(|r| r.farm.as_str())
            .collect()
    }

    /// Which server a farm is on, by name.
    ///
    /// A farm reached on any other server accepts the connection and closes it
    /// without a word, so this is the difference between a working logon and
    /// ten seconds of silence.
    pub fn host_of(&self, farm: &str) -> Option<(&str, u16)> {
        self.rows
            .iter()
            .find(|r| r.farm == farm)
            .map(|r| (r.host.as_str(), r.port))
    }

    /// Every farm named, and the server that serves it.
    pub fn farms(&self) -> HashMap<&str, (&str, u16)> {
        let mut by_farm = HashMap::new();
        for row in &self.rows {
            by_farm
                .entry(row.farm.as_str())
                .or_insert((row.host.as_str(), row.port));
        }
        by_farm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows as the venue sends them, taken from an answer on a live session.
    const SAMPLE: &str = "ADX,STK,Frz,-1,*,zdc1.example,4000,eufarm;\
        AEB,IOPT,Top|Deep,-1,*,zdc1.example,4000,eufarmnj;\
        AMEX,STK,DLDirect,-1,*,ndc1.example,4000,usfarm;\
        TSEJ,STK,Top|Deep,-1,*,hdc1.example,4000,jfarm;\
        PINKSLIPS,STK,Top,20,PINK,ndc1.example,4000,usfarm.nj;\
        EVERYTHING,STK,*,-1,*,cdc1.example,4000,usfuture";

    #[test]
    fn a_routing_table_says_where_each_market_is_served_from() {
        let table = RoutingTable::parse(SAMPLE);
        assert_eq!(table.rows().len(), 6);

        let jp = table.find("TSEJ", "STK", "Deep").expect("Tokyo is served");
        assert_eq!(jp.host, "hdc1.example");
        assert_eq!(jp.farm, "jfarm");
        assert_eq!(jp.port, 4000);

        // The endpoint is part of the key, not decoration: the same market
        // answers for one kind of data and not another.
        assert!(table.find("AMEX", "STK", "DLDirect").is_some());
        assert!(table.find("AMEX", "STK", "Deep").is_none(), "not served here");

        // A row listing every endpoint answers for any of them.
        assert!(table.find("EVERYTHING", "STK", "AggDeep").is_some());

        // Security type is part of the key too.
        assert!(table.find("AEB", "IOPT", "Top").is_some());
        assert!(table.find("AEB", "STK", "Top").is_none());
    }

    /// The farm's server is the whole point: asked for anywhere else, a farm
    /// accepts the connection and closes it without a word.
    #[test]
    fn a_farm_is_looked_up_rather_than_guessed() {
        let table = RoutingTable::parse(SAMPLE);
        assert_eq!(table.host_of("jfarm"), Some(("hdc1.example", 4000)));
        assert_eq!(table.host_of("eufarm"), Some(("zdc1.example", 4000)));
        assert_eq!(table.host_of("usfarm"), Some(("ndc1.example", 4000)));
        assert_eq!(table.host_of("nosuchfarm"), None);

        let farms = table.farms();
        // Six farms across six markets here; the point is that a farm named
        // by several markets is one entry, which the fixture does not yet
        // exercise but the real table does — usfarm alone names dozens.
        assert_eq!(farms.len(), 6, "one entry per farm, not per market");
    }

    /// The smart destination serves the top of a book for a share and never a
    /// deep one, in any region. A named exchange serves both.
    ///
    /// This is what makes a book asked for on no particular venue answerable
    /// before it is sent: the venue has no such book, so the request reaches
    /// nothing and nothing comes back — no refusal, no book, no way to tell
    /// which. The rows below are the shape the venue sends, several regional
    /// books on the smart destination and none of them deep.
    #[test]
    fn the_smart_destination_has_no_book_for_a_share() {
        let table = RoutingTable::parse(
            "BEST,STK,Top,1,*,ndc1.example,4000,usfarm;             BEST,STK,Top,16,*,hdc1.example,4000,jfarm;             BEST,STK,AggDeep,1,PINK,ndc1.example,4000,usfarm;             IEX,STK,Top|Deep,-1,*,ndc1.example,4000,usfarm;             ISLAND,STK,Top|Deep2|Deep,-1,*,ndc1.example,4000,usfarm"
        );

        // Named at all, so the venue does serve this market — just not a book.
        assert!(table.find("BEST", "STK", "Top").is_some());
        assert_eq!(table.book_endpoint("BEST", "STK"), None, "no book on the smart route");

        // A book the venue serves under another name is still a book. Asking
        // only about `Deep` says these markets have none, and they do.
        let others = RoutingTable::parse(
            "IBEFP,COMB,Top|Deep2,-1,*,n.example,4000,usfuture.nj;\
             SEHK,OPT,DeepX,-1,*,h.example,4000,hfarm"
        );
        assert_eq!(others.book_endpoint("IBEFP", "COMB"), Some("Deep2"));
        assert_eq!(others.book_endpoint("SEHK", "OPT"), Some("DeepX"));

        // A market offering more than one is offering the better of them, and
        // the order is the counterpart's rather than the order the row happens
        // to list.
        let several = RoutingTable::parse("X,STK,Deep|Deep2,-1,*,h.example,4000,f");
        assert_eq!(several.book_endpoint("X", "STK"), Some("Deep2"));

        // On the exchange the contract trades on, there is one.
        assert!(table.find("IEX", "STK", "Deep").is_some());
        assert!(table.find("ISLAND", "STK", "Deep2").is_some());

        // The aggregated book exists for one sub-market and is not an answer
        // for a share that is not in it. Matched without reading the
        // qualifier, this row says every share on the destination has a book.
        assert!(
            table.find("BEST", "STK", "AggDeep").is_none(),
            "a qualified row is not an answer for the whole market",
        );
        let agg = table
            .find_within("BEST", "STK", "AggDeep", "PINK")
            .expect("served for the sub-market that names it");
        assert_eq!(agg.qualifier, "PINK");

        // A market the table does not name at all is not evidence of anything,
        // and must not be read as a refusal.
        assert!(table.find("NOSUCH", "STK", "Top").is_none());
    }

    /// The frame's own trailer rides on the last row, and a row that cannot be
    /// read is one row rather than a reason to lose the table.
    #[test]
    fn a_trailing_frame_does_not_take_the_table_with_it() {
        let with_trailer = "ADX,STK,Frz,-1,*,zdc1.example,4000,eufarm\u{1}8349=DD9C5211\u{1}";
        let table = RoutingTable::parse(with_trailer);
        assert_eq!(table.rows().len(), 1);
        assert_eq!(table.rows()[0].farm, "eufarm", "the trailer is not part of the name");

        // Short rows, empty rows, a port that is not a number, and a book that
        // is not one either — the venue writes -1 for a market's own book, so
        // defaulting to it would report a row as naming a book it never named.
        let ragged = "A,B;;;X,STK,Top,-1,*,h,notaport,f;\
             Y,STK,Top,notabook,*,h,4000,f;GOOD,STK,Top,-1,*,h,4000,f";
        let table = RoutingTable::parse(ragged);
        assert_eq!(table.rows().len(), 1, "only the row that reads");
        assert_eq!(table.rows()[0].exchange, "GOOD");

        assert!(RoutingTable::parse("").is_empty());
    }

    /// A book is served by a named farm, and this session is on one farm.
    #[test]
    fn the_farms_that_serve_a_book_are_named() {
        let table = RoutingTable::parse(
            "IEX,CS,Top|Deep,-1,*,ndc1.example,4002,ushmds;\
             BEST,CS,Top,-1,*,ndc1.example,4002,ushmds;\
             XETRA,CS,AggDeep,-1,*,zdc1.example,4002,euhmds",
        );
        let farms = table.book_farms();
        assert!(farms.contains("ushmds"), "IEX serves a book from ushmds");
        assert!(farms.contains("euhmds"), "and XETRA from another farm");
        assert_eq!(farms.len(), 2, "a row serving only the top of a book is not one");
    }
}
