//! The tests for this module.
//!
//! One file per module, as `api/client` already does it. Each block below
//! reaches the code it tests through `super::super`, which is the module this
//! file belongs to.

mod hot_loop_panic_tests {
    use super::super::*;

    /// Frame bodies are decoded with `from_utf8_lossy`, so one invalid byte
    /// becomes a three-byte U+FFFD that can straddle a byte-indexed slice
    /// boundary. These return a value rather than aborting the hot loop.
    #[test]
    fn session_endpoint_survives_a_lossily_decoded_field() {
        // U+FFFD at bytes 12..15: a byte-indexed cut at 12..14 lands inside it.
        let lossy = format!("20260728-09:{}0", '\u{FFFD}');
        assert!(!lossy.is_ascii());
        let _ = trim_session_endpoint(&lossy);

        let lossy2 = format!("2026072{}-09:30", '\u{FFFD}');
        let _ = trim_session_endpoint(&lossy2);

        // The ASCII form still trims exactly as before.
        assert_eq!(trim_session_endpoint("20260728-09:30"), "20260728:0930");
        assert_eq!(trim_session_endpoint("short"), "short");
    }

    /// The closed-session branch is the one that slices trade_date/start, and
    /// it is only taken when start == end. The previous version of this test
    /// set them differently, so it never reached the slice at all and passed
    /// against the unfixed code too.
    #[test]
    fn sessions_string_survives_a_non_ascii_trade_date() {
        let closed = format!("2026072{}", '\u{FFFD}');
        let s = ScheduleSession {
            trade_date: closed.clone(),
            start: closed.clone(),
            end: closed,
        };
        let out = format_sessions_string(&[s]);
        assert!(!out.is_empty(), "a closed session must still render");

        // A short trade_date falls back to start, and a short start to the
        // whole string, without slicing past a boundary.
        let short = ScheduleSession {
            trade_date: "2026".to_string(),
            start: "2026".to_string(),
            end: "2026".to_string(),
        };
        let _ = format_sessions_string(&[short]);
    }
}
use super::*;

#[test]
fn security_type_roundtrip() {
    for st in [
        SecurityType::Stock,
        SecurityType::Option,
        SecurityType::Future,
        SecurityType::Forex,
    ] {
        assert_eq!(SecurityType::from_fix(st.to_fix()), st);
    }
}

#[test]
fn exchange_mapping() {
    assert_eq!(exchange_to_fix("SMART"), "BEST");
    assert_eq!(exchange_to_fix("NYSE"), "NYSE");
    assert_eq!(exchange_to_fix("ISLAND"), "NASDAQ", "legacy spelling must route");
    assert_eq!(exchange_from_fix("BEST"), "SMART");
    assert_eq!(exchange_from_fix("ARCA"), "ARCA");
}

#[test]
fn sec_type_wire_mapping() {
    // Only STK is renamed on the wire; everything else passes through.
    assert_eq!(sec_type_to_fix("STK"), "CS");
    assert_eq!(sec_type_to_fix("FUT"), "FUT");
    assert_eq!(sec_type_to_fix("OPT"), "OPT");
    assert_eq!(sec_type_to_fix("CASH"), "CASH");
}

#[test]
fn build_secdef_by_conid() {
    let msg = build_secdef_request_by_conid("R1", 265598, 1);
    let tags = fix::fix_parse(&msg);
    assert_eq!(tags[&TAG_MSG_TYPE], "c");
    assert_eq!(tags[&TAG_SECURITY_REQ_ID], "R1");
    assert_eq!(tags[&TAG_SECURITY_REQ_TYPE], "2");
    assert_eq!(tags[&TAG_IB_CON_ID], "265598");
    assert_eq!(tags[&TAG_IB_SOURCE], "Socket");
}

#[test]
fn build_secdef_by_symbol() {
    let msg = build_secdef_request_by_symbol("R2", "AAPL", SecurityType::Stock, "SMART", "USD", 2);
    let tags = fix::fix_parse(&msg);
    assert_eq!(tags[&TAG_MSG_TYPE], "c");
    assert_eq!(tags[&TAG_SYMBOL], "AAPL");
    assert_eq!(tags[&TAG_SECURITY_TYPE], "CS");
    assert_eq!(tags[&TAG_EXCHANGE], "BEST"); // SMART→BEST
    assert_eq!(tags[&TAG_CURRENCY], "USD");
}

#[test]
fn parse_secdef_response() {
    // Build a fake security definition response
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_SECURITY_REQ_ID, "R1"),
            (TAG_SECURITY_RESPONSE_TYPE, "4"),
            (TAG_IB_CON_ID, "265598"),
            (TAG_SYMBOL, "AAPL"),
            (TAG_SECURITY_TYPE, "CS"),
            (TAG_SECURITY_EXCHANGE, "NASDAQ"),
            (TAG_CURRENCY, "USD"),
            (TAG_LONG_NAME, "APPLE INC"),
            (TAG_IB_VALID_EXCHANGES, "BEST,NYSE,ARCA"),
            (TAG_IB_PRIMARY_EXCHANGE, "NASDAQ"),
            // Inline price-increment block: min_tick is derived from the
            // smallest increment, not the 6019 rule-start sentinel.
            (TAG_MARKET_RULE_START, "1"),
            (TAG_MARKET_RULE_ID, "26"),
            (TAG_LOW_EDGE, "0"),
            (TAG_INCREMENT, "0.01"),
            (TAG_SIZE_INCREMENT_COUNT, "1"),
        ],
        1,
    );
    let def = super::parse_secdef_response(&msg, true).unwrap();
    assert_eq!(def.con_id, 265598);
    assert_eq!(def.symbol, "AAPL");
    assert_eq!(def.sec_type, SecurityType::Stock);
    // Handed back under the name the counterpart hands it back under. What
    // goes out still routes under the venue's own name.
    assert_eq!(def.exchange, "ISLAND");
    assert_eq!(exchange_to_fix(&def.exchange), "NASDAQ");
    assert_eq!(def.currency, "USD");
    assert_eq!(def.long_name, "APPLE INC");
    assert_eq!(def.min_tick, 0.01);
    assert_eq!(def.valid_exchanges, vec!["SMART", "NYSE", "ARCA"]);
    assert_eq!(def.primary_exchange, "ISLAND");
}

#[test]
fn parse_rejects_non_secdef() {
    let msg = fix::fix_build(&[(TAG_MSG_TYPE, "A")], 1);
    assert!(super::parse_secdef_response(&msg, true).is_none());
}

// A US equity secdef carries an inline price-increment block whose start
// sentinel is `6019=1`. Tag 6019 is not min_tick — reading it as one
// yields 1.0; min_tick is the smallest increment the block states.
#[test]
fn secdef_min_tick_from_price_increments_not_rule_sentinel() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_IB_CON_ID, "4726868"),
            (TAG_SYMBOL, "AXTI"),
            (TAG_SECURITY_TYPE, "CS"),
            (TAG_CURRENCY, "USD"),
            // Inline market-rule block (6019="1" is the start sentinel).
            (TAG_MARKET_RULE_START, "1"),
            (TAG_MARKET_RULE_ID, "26"),
            (TAG_LOW_EDGE, "0"),
            (TAG_INCREMENT, "0.0001"),
            (TAG_LOW_EDGE, "1"),
            (TAG_INCREMENT, "0.01"),
            (TAG_SIZE_INCREMENT_COUNT, "1"),
        ],
        1,
    );
    let def = super::parse_secdef_response(&msg, true).unwrap();
    // Smallest increment across bands, not the "1" rule sentinel.
    assert_eq!(def.min_tick, 0.0001);
}

/// A definition stating no rule block states no smallest increment, and
/// none is invented for it. A penny would price most futures on a grid
/// they are not traded on, and state it as confidently as a figure the
/// venue gave.
#[test]
fn a_definition_stating_no_rule_states_no_increment() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_IB_CON_ID, "265598"),
            (TAG_SYMBOL, "AAPL"),
            (TAG_SECURITY_TYPE, "CS"),
        ],
        1,
    );
    let def = super::parse_secdef_response(&msg, true).unwrap();
    assert_eq!(def.min_tick, 0.0, "unstated, not a penny");
}

#[test]
fn secdef_response_last_check() {
    let msg5 = fix::fix_build(
        &[(TAG_MSG_TYPE, "d"), (TAG_SECURITY_RESPONSE_TYPE, "5")],
        1,
    );
    let msg4 = fix::fix_build(
        &[(TAG_MSG_TYPE, "d"), (TAG_SECURITY_RESPONSE_TYPE, "4")],
        2,
    );
    assert!(secdef_response_is_last(&msg5));
    assert!(!secdef_response_is_last(&msg4));
}

#[test]
fn contract_store_insert_and_lookup() {
    let mut store = ContractStore::default();
    let def = ContractDefinition {
        con_id: 265598,
        symbol: "AAPL".to_string(),
        sec_type: SecurityType::Stock,
        currency: "USD".to_string(),
        exchange: "NASDAQ".to_string(),
        ..Default::default()
    };
    store.insert(def);

    assert_eq!(store.len(), 1);
    let found = store.get(265598).unwrap();
    assert_eq!(found.symbol, "AAPL");

    let by_sym = store.find("AAPL", SecurityType::Stock, "USD").unwrap();
    assert_eq!(by_sym.con_id, 265598);

    assert!(store.find("MSFT", SecurityType::Stock, "USD").is_none());
}

#[test]
fn contract_store_update_replaces() {
    let mut store = ContractStore::default();
    store.insert(ContractDefinition {
        con_id: 265598,
        symbol: "AAPL".to_string(),
        long_name: "OLD".to_string(),
        ..Default::default()
    });
    store.insert(ContractDefinition {
        con_id: 265598,
        symbol: "AAPL".to_string(),
        long_name: "APPLE INC".to_string(),
        ..Default::default()
    });
    assert_eq!(store.len(), 1);
    assert_eq!(store.get(265598).unwrap().long_name, "APPLE INC");
}

/// A definition that states both keeps the one that names the contract.
/// Handing back the month instead turned a weekly option into something
/// that no longer identifies what it came from.
#[test]
fn the_exact_expiry_wins_over_the_month_it_falls_in() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_IB_CON_ID, "12345"),
            (TAG_SYMBOL, "SPY"),
            (TAG_SECURITY_TYPE, "OPT"),
            (TAG_LAST_TRADE_DATE, "202609"),
            (TAG_MATURITY_DATE, "20260918"),
        ],
        1,
    );
    let def = super::parse_secdef_response(&msg, true).unwrap();
    assert_eq!(def.last_trade_date, "20260918");
}

/// An expiry is a date and a contract month is not, and the two ride
/// different tags. Reading only MaturityMonthYear left the expiry empty on
/// any definition that stated the date.
#[test]
fn an_expiry_stated_as_a_date_is_read_from_its_own_tag() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_IB_CON_ID, "12345"),
            (TAG_SYMBOL, "AAPL"),
            (TAG_SECURITY_TYPE, "OPT"),
            (TAG_MATURITY_DATE, "20260918"),
            (TAG_STRIKE, "200.0"),
            (TAG_RIGHT, "C"),
        ],
        1,
    );
    let def = super::parse_secdef_response(&msg, true).unwrap();
    assert_eq!(def.last_trade_date, "20260918");
}

#[test]
fn option_contract_fields() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_IB_CON_ID, "12345"),
            (TAG_SYMBOL, "AAPL"),
            (TAG_SECURITY_TYPE, "OPT"),
            (TAG_LAST_TRADE_DATE, "20260321"),
            (TAG_STRIKE, "200.0"),
            (TAG_RIGHT, "C"),
            (TAG_MULTIPLIER, "100"),
        ],
        1,
    );
    let def = super::parse_secdef_response(&msg, true).unwrap();
    assert_eq!(def.sec_type, SecurityType::Option);
    assert_eq!(def.last_trade_date, "20260321");
    assert_eq!(def.strike, 200.0);
    assert_eq!(def.right, Some(OptionRight::Call));
    assert_eq!(def.multiplier, 100.0);
}

#[test]
fn parse_schedule_response_basic() {
    // Build a fake schedule response with 2 trading + 2 liquid sessions
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "U"),
            (TAG_SUB_PROTOCOL, "107"),
            (TAG_SCHEDULE_TIMEZONE, "US/Eastern"),
            (TAG_SESSION_COUNT, "4"),
            // Trading session 1
            (TAG_SESSION_START, "20260311-08:00:00"),
            (TAG_SESSION_END, "20260312-00:00:00"),
            (TAG_TRADE_DATE, "20260311"),
            (TAG_IS_TRADING_HOURS, "1"),
            // Liquid session 1
            (TAG_SESSION_START, "20260311-13:30:00"),
            (TAG_SESSION_END, "20260311-20:00:00"),
            (TAG_TRADE_DATE, "20260311"),
            (TAG_IS_LIQUID_HOURS, "1"),
            // Trading session 2
            (TAG_SESSION_START, "20260312-08:00:00"),
            (TAG_SESSION_END, "20260313-00:00:00"),
            (TAG_TRADE_DATE, "20260312"),
            (TAG_IS_TRADING_HOURS, "1"),
            // Liquid session 2
            (TAG_SESSION_START, "20260312-13:30:00"),
            (TAG_SESSION_END, "20260312-20:00:00"),
            (TAG_TRADE_DATE, "20260312"),
            (TAG_IS_LIQUID_HOURS, "1"),
        ],
        1,
    );
    let sched = parse_schedule_response(&msg).unwrap();
    assert_eq!(sched.timezone, "US/Eastern");
    assert_eq!(sched.trading_hours.len(), 2);
    assert_eq!(sched.liquid_hours.len(), 2);

    assert_eq!(sched.trading_hours[0].start, "20260311-08:00:00");
    assert_eq!(sched.trading_hours[0].end, "20260312-00:00:00");
    assert_eq!(sched.trading_hours[0].trade_date, "20260311");

    assert_eq!(sched.liquid_hours[0].start, "20260311-13:30:00");
    assert_eq!(sched.liquid_hours[0].end, "20260311-20:00:00");
}

#[test]
fn parse_schedule_dual_flag_appends_to_both() {
    // 24h venues (FOREX) emit sessions with both 6843=1 AND 6844=1.
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "U"),
            (TAG_SUB_PROTOCOL, "107"),
            (TAG_SCHEDULE_TIMEZONE, "US/Eastern"),
            (TAG_SESSION_COUNT, "1"),
            (TAG_SESSION_START, "20260427-22:15:00"),
            (TAG_SESSION_END, "20260428-22:00:00"),
            (TAG_TRADE_DATE, "20260427"),
            (TAG_IS_TRADING_HOURS, "1"),
            (TAG_IS_LIQUID_HOURS, "1"),
        ],
        1,
    );
    let sched = parse_schedule_response(&msg).unwrap();
    assert_eq!(sched.trading_hours.len(), 1);
    assert_eq!(sched.liquid_hours.len(), 1);
    assert_eq!(sched.trading_hours[0].start, sched.liquid_hours[0].start);
}

#[test]
fn format_sessions_string_basic() {
    let sessions = vec![
        ScheduleSession {
            start: "20260427-13:30:00".into(),
            end: "20260427-20:00:00".into(),
            trade_date: "20260427".into(),
        },
        ScheduleSession {
            start: "20260428-13:30:00".into(),
            end: "20260428-20:00:00".into(),
            trade_date: "20260428".into(),
        },
    ];
    let s = format_sessions_string(&sessions);
    assert_eq!(s, "20260427:1330-20260427:2000;20260428:1330-20260428:2000");
}

#[test]
fn format_sessions_string_empty() {
    assert_eq!(format_sessions_string(&[]), "");
}

#[test]
fn parse_schedule_rejects_non_schedule() {
    let msg = fix::fix_build(&[(TAG_MSG_TYPE, "d")], 1);
    assert!(parse_schedule_response(&msg).is_none());

    // Wrong sub-protocol
    let msg = fix::fix_build(
        &[(TAG_MSG_TYPE, "U"), (TAG_SUB_PROTOCOL, "100")],
        1,
    );
    assert!(parse_schedule_response(&msg).is_none());
}

#[test]
fn market_rule_id_parsed() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_IB_CON_ID, "756733"),
            (TAG_SYMBOL, "SPY"),
            (TAG_SECURITY_TYPE, "CS"),
            (TAG_IB_MARKET_RULE_ID, "4563"),
        ],
        1,
    );
    let def = super::parse_secdef_response(&msg, true).unwrap();
    assert_eq!(def.market_rule_id, Some(4563));
}

#[test]
fn market_rule_id_absent() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_IB_CON_ID, "756733"),
            (TAG_SYMBOL, "SPY"),
        ],
        1,
    );
    let def = super::parse_secdef_response(&msg, true).unwrap();
    assert_eq!(def.market_rule_id, None);
}

#[test]
fn build_matching_symbols_request_structure() {
    let msg = build_matching_symbols_request("APP", "R1", 1);
    let tags = fix::fix_parse(&msg);
    assert_eq!(tags[&fix::TAG_MSG_TYPE], "U");
    assert_eq!(tags[&TAG_SUB_PROTOCOL], "185");
    assert_eq!(tags[&TAG_SECURITY_REQ_ID], "R1");
    assert_eq!(tags[&TAG_MATCH_PATTERN], "APP");
}

#[test]
fn parse_matching_symbols_response_basic() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "U"),
            (TAG_SUB_PROTOCOL, "186"),
            (TAG_SECURITY_REQ_ID, "R1"),
            (TAG_MATCH_COUNT, "2"),
            // Match 1
            (TAG_SYMBOL, "AAPL"),
            (TAG_SECURITY_TYPE, "CS"),
            (TAG_CURRENCY, "USD"),
            (TAG_IB_CON_ID, "265598"),
            (TAG_MATCH_PRIMARY_EXCHANGE, "NASDAQ"),
            (TAG_MATCH_DESCRIPTION, "APPLE INC"),
            (TAG_MATCH_DERIVATIVE_TYPES, "OPT,WAR"),
            // Match 2
            (TAG_SYMBOL, "APP"),
            (TAG_SECURITY_TYPE, "CS"),
            (TAG_CURRENCY, "USD"),
            (TAG_IB_CON_ID, "481863646"),
            (TAG_MATCH_PRIMARY_EXCHANGE, "NASDAQ"),
            (TAG_MATCH_DESCRIPTION, "APPLOVIN CORP"),
            (TAG_MATCH_DERIVATIVE_TYPES, "OPT"),
        ],
        1,
    );
    let matches = parse_matching_symbols_response(&msg).unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].symbol, "AAPL");
    assert_eq!(matches[0].con_id, 265598);
    assert_eq!(matches[0].primary_exchange, "NASDAQ");
    assert_eq!(matches[0].description, "APPLE INC");
    assert_eq!(matches[0].derivative_types, vec!["OPT", "WAR"]);
    assert_eq!(matches[1].symbol, "APP");
    assert_eq!(matches[1].con_id, 481863646);
}

/// One reply, two underlyings, and two classes under the first of them.
/// A scope states only what it changes from the one before it, and the
/// expirations belong to the record rather than to either class.
#[test]
fn parse_option_chain_response_reads_every_scope_of_every_record() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "U"),
            (TAG_SUB_PROTOCOL, "139"),
            (8371, "opaque"),
            (6455, "2"),
            // First underlying, listed on two venues.
            (TAG_SYMBOL, "AAPL"),
            (6775, "20260116/20260220/20260320/EXPW=20260109"),
            (6777, "20260320"),
            (6971, "20260109"),
            (6346, "265598"),
            (TAG_EXCHANGE, "SMART"),
            (TAG_IB_TRADING_CLASS, "AAPL"),
            (TAG_MULTIPLIER, "100"),
            (6997, "140.0;145.0;150.0"),
            (TAG_EXCHANGE, "CBOE"),
            (TAG_IB_TRADING_CLASS, "AAPL1"),
            (6997, "145.0;150.0"),
            // Second underlying.
            (TAG_SYMBOL, "SPX"),
            (6778, "20260220"),
            (TAG_EXCHANGE, "CBOE"),
            (TAG_IB_TRADING_CLASS, "SPXW"),
            (TAG_MULTIPLIER, "100"),
            (6997, "5000.0;5100.0"),
        ],
        1,
    );

    let scopes = parse_option_chain_response(&msg).unwrap();

    assert_eq!(scopes.len(), 3, "two classes on AAPL and one on SPX");
    assert_eq!(scopes[0].symbol, "AAPL");
    assert_eq!(scopes[0].exchange, "SMART");
    assert_eq!(scopes[0].trading_class, "AAPL");
    assert_eq!(scopes[0].multiplier, "100");
    assert_eq!(scopes[0].strikes, vec![140.0, 145.0, 150.0]);
    assert_eq!(
        scopes[0].expirations,
        vec!["20260116", "20260220", "20260320", "20260109"],
        "the chain itself, without the keyed bucket, and each date once",
    );
    assert_eq!(scopes[1].exchange, "CBOE");
    assert_eq!(scopes[1].trading_class, "AAPL1");
    assert_eq!(scopes[1].multiplier, "100", "carried over from the class before it");
    assert_eq!(scopes[1].strikes, vec![145.0, 150.0]);
    assert_eq!(scopes[1].expirations, scopes[0].expirations, "both classes of one record");
    assert_eq!(scopes[2].symbol, "SPX");
    assert_eq!(scopes[2].trading_class, "SPXW");
    assert_eq!(scopes[2].strikes, vec![5000.0, 5100.0]);
    assert_eq!(scopes[2].expirations, vec!["20260220"], "and nothing of the record before it");
}

/// A value that does not hold a date in the compound shape is still read
/// for the dates it does hold, rather than answering with no expirations.
#[test]
fn parse_option_chain_expirations_fall_back_to_a_plain_list() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "U"),
            (TAG_SUB_PROTOCOL, "139"),
            (TAG_SYMBOL, "AAPL"),
            (6775, "20260116,20260220"),
            (TAG_EXCHANGE, "SMART"),
            (TAG_IB_TRADING_CLASS, "AAPL"),
            (6997, "140.0"),
        ],
        1,
    );

    let scopes = parse_option_chain_response(&msg).unwrap();

    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].expirations, vec!["20260116", "20260220"]);
}

#[test]
fn parse_option_chain_rejects_non_chain() {
    let msg = fix::fix_build(&[(TAG_MSG_TYPE, "U"), (TAG_SUB_PROTOCOL, "186")], 1);
    assert!(parse_option_chain_response(&msg).is_none());
}

// A closed day (neither hours flag set) must be represented,
// not dropped — "market closed" and "data missing" were previously
// indistinguishable.
#[test]
fn schedule_closed_day_is_kept_and_renders_closed() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "U"),
            (TAG_SUB_PROTOCOL, "107"),
            (TAG_SCHEDULE_TIMEZONE, "US/Eastern"),
            (TAG_SESSION_COUNT, "2"),
            // Saturday: closed — no 6843/6844 flags.
            (TAG_SESSION_START, "20260718-00:00:00"),
            (TAG_SESSION_END, "20260718-00:00:00"),
            (TAG_TRADE_DATE, "20260718"),
            // Monday: normal trading session.
            (TAG_SESSION_START, "20260720-13:30:00"),
            (TAG_SESSION_END, "20260720-20:00:00"),
            (TAG_TRADE_DATE, "20260720"),
            (TAG_IS_TRADING_HOURS, "1"),
            (TAG_IS_LIQUID_HOURS, "1"),
        ],
        1,
    );
    let sched = parse_schedule_response(&msg).unwrap();
    assert_eq!(sched.trading_hours.len(), 2, "closed day must appear");
        assert_eq!(sched.liquid_hours.len(), 2);
    let rendered = format_sessions_string(&sched.trading_hours);
    assert_eq!(rendered, "20260718:CLOSED;20260720:1330-20260720:2000");
}

// An unrecognized security type must not be encoded as a stock.
#[test]
fn to_fix_other_is_not_stock() {
    assert_eq!(SecurityType::Other.to_fix(), "");
    assert_eq!(SecurityType::from_fix(""), SecurityType::Other);
}

// User-visible sec_type must be the official API string, and
// an unclassifiable instrument must not masquerade as a stock.
#[test]
fn sec_type_to_api_str_round_trips_and_other_is_empty() {
    assert_eq!(SecurityType::Stock.to_api_str(), "STK");
    assert_eq!(SecurityType::Forex.to_api_str(), "CASH");
    assert_eq!(SecurityType::Warrant.to_api_str(), "WAR");

    // Every type the terminal names, and each one back again. A type this
    // does not know is sent as an empty security type on purpose, so one
    // that is merely missing from here does not quietly route as a stock —
    // which means a gap shows up as a refused request rather than an order
    // on the wrong contract, and is worth keeping honest.
    for (ty, wire) in [
        (SecurityType::Stock, "STK"), (SecurityType::Option, "OPT"),
        (SecurityType::Future, "FUT"), (SecurityType::Forex, "CASH"),
        (SecurityType::Index, "IND"), (SecurityType::Bond, "BOND"),
        (SecurityType::Warrant, "WAR"), (SecurityType::FutureOption, "FOP"),
        (SecurityType::Cfd, "CFD"), (SecurityType::Commodity, "CMDTY"),
        (SecurityType::Fund, "FUND"), (SecurityType::Forward, "FWD"),
        (SecurityType::Bill, "BILL"), (SecurityType::Combo, "BAG"),
        (SecurityType::Crypto, "CRYPTO"),
        (SecurityType::FixedIncome, "FIXED"),
        (SecurityType::SecuritiesLending, "SLB"),
        (SecurityType::News, "NEWS"),
        (SecurityType::Basket, "BSK"),
        (SecurityType::IndexOption, "IOPT"),
        (SecurityType::IcuContract, "ICU"),
        (SecurityType::IcsContract, "ICS"),
        (SecurityType::PhysicalSettlement, "PHYSS"),
    ] {
        assert_eq!(ty.to_api_str(), wire, "{ty:?} states itself as {wire}");
        assert_eq!(SecurityType::from_fix(wire), ty, "and is read back from {wire}");
    }
    assert_eq!(SecurityType::from_fix("NOPE"), SecurityType::Other);
    assert_eq!(SecurityType::Other.to_fix(), "", "unknown stays unroutable, not a stock");
    assert_eq!(SecurityType::Other.to_api_str(), "");
    // Every non-Other variant survives the round trip back through the
    // inbound parser (which accepts API strings too), so a reported
    // Contract can be fed into another request.
    for st in [SecurityType::Stock, SecurityType::Option, SecurityType::Future,
               SecurityType::Forex, SecurityType::Index, SecurityType::Bond,
               SecurityType::Warrant] {
        assert_eq!(SecurityType::from_fix(st.to_api_str()), st, "{st:?}");
    }
}

#[test]
fn parse_matching_symbols_rejects_non_match() {
    let msg = fix::fix_build(&[(TAG_MSG_TYPE, "d")], 1);
    assert!(parse_matching_symbols_response(&msg).is_none());

    let msg = fix::fix_build(
        &[(TAG_MSG_TYPE, "U"), (TAG_SUB_PROTOCOL, "107")],
        1,
    );
    assert!(parse_matching_symbols_response(&msg).is_none());
}

#[test]
fn parse_market_rules_single_rule() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_IB_CON_ID, "265598"),
            (TAG_SYMBOL, "AAPL"),
            // Market rule block
            (TAG_MARKET_RULE_START, "1"),
            (TAG_MARKET_RULE_ID, "26"),
            (TAG_LOW_EDGE, "0"),
            (TAG_INCREMENT, "0.01"),
            (TAG_LOW_EDGE, "1"),
            (TAG_INCREMENT, "0.01"),
            (TAG_SIZE_INCREMENT_COUNT, "1"),
        ],
        1,
    );
    let rules = parse_market_rules(&msg);
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].rule_id, 26);
    assert_eq!(rules[0].price_increments.len(), 2);
    assert_eq!(rules[0].price_increments[0].low_edge, 0.0);
    assert_eq!(rules[0].price_increments[0].increment, 0.01);
    assert_eq!(rules[0].price_increments[1].low_edge, 1.0);
    assert_eq!(rules[0].price_increments[1].increment, 0.01);
}

#[test]
fn parse_market_rules_multiple_rules() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            // Rule 1: penny increments
            (TAG_MARKET_RULE_START, "1"),
            (TAG_MARKET_RULE_ID, "26"),
            (TAG_LOW_EDGE, "0"),
            (TAG_INCREMENT, "0.01"),
            (TAG_SIZE_INCREMENT_COUNT, "1"),
            // Rule 2: nickel increments above $1
            (TAG_MARKET_RULE_START, "1"),
            (TAG_MARKET_RULE_ID, "42"),
            (TAG_LOW_EDGE, "0"),
            (TAG_INCREMENT, "0.01"),
            (TAG_LOW_EDGE, "1"),
            (TAG_INCREMENT, "0.05"),
            (TAG_SIZE_INCREMENT_COUNT, "1"),
        ],
        1,
    );
    let rules = parse_market_rules(&msg);
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].rule_id, 26);
    assert_eq!(rules[0].price_increments.len(), 1);
    assert_eq!(rules[1].rule_id, 42);
    assert_eq!(rules[1].price_increments.len(), 2);
    assert_eq!(rules[1].price_increments[1].low_edge, 1.0);
    assert_eq!(rules[1].price_increments[1].increment, 0.05);
}

#[test]
fn parse_market_rules_empty_when_none() {
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_IB_CON_ID, "265598"),
            (TAG_SYMBOL, "AAPL"),
        ],
        1,
    );
    let rules = parse_market_rules(&msg);
    assert!(rules.is_empty());
}

#[test]
fn parse_market_rules_no_end_marker() {
    // Rule without explicit 6030 end marker -- should still be collected
    let msg = fix::fix_build(
        &[
            (TAG_MSG_TYPE, "d"),
            (TAG_MARKET_RULE_START, "1"),
            (TAG_MARKET_RULE_ID, "10"),
            (TAG_LOW_EDGE, "0"),
            (TAG_INCREMENT, "0.005"),
        ],
        1,
    );
    let rules = parse_market_rules(&msg);
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].rule_id, 10);
    assert_eq!(rules[0].price_increments.len(), 1);
    assert_eq!(rules[0].price_increments[0].increment, 0.005);
}

/// The character `from_utf8_lossy` substitutes for an invalid byte.
const REPLACEMENT: &str = "\u{FFFD}";

/// Schedule strings come off the wire and are decoded with
/// `from_utf8_lossy`, so one invalid byte becomes a three-byte replacement
/// character and every byte position after it shifts. Validating byte
/// positions and then slicing the `&str` panics off a character boundary,
/// and this runs on the hot loop, so that is an engine-down on malformed
/// input rather than a dropped message.
#[test]
fn a_schedule_that_is_not_ascii_does_not_panic() {
    // The shapes a lossy decode produces: a replacement character sitting
    // where the parser expects an ASCII digit, at each boundary it cuts on.
    let hostile = [
        "2026010@-09:30:00",
        "20260101-0@:30:00",
        "20260101-09:3@:00",
        "20260101-09:30:0@",
        "@",
        "",
        "short",
        "@@@@@",
    ];

    for h in hostile {
        let h = h.replace('@', REPLACEMENT);
        let sessions = [ScheduleSession {
            start: h.clone(),
            end: "20260101-16:00:00".to_string(),
            trade_date: h.clone(),
        }];
        let _ = format_sessions_string(&sessions);

        // And the closed-session branch, which takes the other two slices.
        let closed = [ScheduleSession {
            start: h.clone(),
            end: h.clone(),
            trade_date: h,
        }];
        let _ = format_sessions_string(&closed);
    }
}

/// The positive control: a well-formed schedule still gets trimmed to the
/// compact form, so the guard above is not simply refusing everything.
#[test]
fn a_well_formed_schedule_is_still_trimmed() {
    let sessions = [
        ScheduleSession {
            start: "20260101-09:30:00".to_string(),
            end: "20260101-16:00:00".to_string(),
            trade_date: "20260101".to_string(),
        },
        ScheduleSession {
            start: "20260102-00:00:00".to_string(),
            end: "20260102-00:00:00".to_string(),
            trade_date: "20260102".to_string(),
        },
    ];
    assert_eq!(
        format_sessions_string(&sessions),
        "20260101:0930-20260101:1600;20260102:CLOSED",
    );
}

/// A reply naming two listings of one symbol is two contracts, not the
/// last one. Read as a single contract it resolves to whichever came last,
/// which is how asking for SPY yields the Australian dollar listing.
#[test]
fn a_reply_naming_two_listings_is_two_contracts() {
    let msg = crate::protocol::fix::fix_build(&[
        (crate::protocol::fix::TAG_MSG_TYPE, "d"),
        (TAG_SECURITY_REQ_ID, "7"),
        (TAG_SECURITY_RESPONSE_TYPE, "4"),
        (TAG_SYMBOL, "SPY"),
        (TAG_SECURITY_TYPE, "CS"),
        (207, "BEST"),
        (TAG_IB_CON_ID, "756733"),
        (TAG_CURRENCY, "USD"),
        (TAG_SYMBOL, "SPY"),
        (TAG_SECURITY_TYPE, "CS"),
        (207, "ASX"),
        (TAG_IB_CON_ID, "237937002"),
        (TAG_CURRENCY, "AUD"),
    ], 1);

    let defs = parse_secdef_responses(&msg, true);
    assert_eq!(defs.len(), 2, "both listings: {defs:?}");
    assert_eq!(defs[0].con_id, 756733);
    assert_eq!(defs[0].currency, "USD");
    assert_eq!(defs[1].con_id, 237937002);
    assert_eq!(defs[1].currency, "AUD", "and neither borrowed the other's fields");

    // The identifier block reuses the symbol tag without naming a
    // contract, so it starts no record.
    let with_ids = crate::protocol::fix::fix_build(&[
        (crate::protocol::fix::TAG_MSG_TYPE, "d"),
        (TAG_SECURITY_REQ_ID, "9"),
        (TAG_SYMBOL, "SPY"),
        (TAG_SECURITY_TYPE, "CS"),
        (TAG_IB_CON_ID, "756733"),
        (TAG_CURRENCY, "USD"),
        (454, "2"),
        (TAG_SYMBOL, "BBG"),
        (455, "BBG000BDTBL9"),
        (TAG_SYMBOL, "US"),
        (455, "US78462F1030"),
    ], 1);
    let defs = parse_secdef_responses(&with_ids, true);
    assert_eq!(defs.len(), 1, "one contract and two identifiers: {defs:?}");
    assert_eq!(defs[0].con_id, 756733);

    // One contract still reads as one.
    let single = crate::protocol::fix::fix_build(&[
        (crate::protocol::fix::TAG_MSG_TYPE, "d"),
        (TAG_SECURITY_REQ_ID, "8"),
        (TAG_SYMBOL, "AAPL"),
        (TAG_IB_CON_ID, "265598"),
        (TAG_CURRENCY, "USD"),
    ], 1);
    let defs = parse_secdef_responses(&single, true);
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].con_id, 265598);
}
mod industry_tests {
    use super::super::*;

    fn secdef(extra: &str) -> Vec<u8> {
        format!("35=d\u{1}320=R1\u{1}6008=756733\u{1}55=SPY\u{1}167=CS\u{1}\
                 207=SMART\u{1}15=USD\u{1}{extra}")
            .into_bytes()
    }

    /// A contract whose economic value follows something other than its own
    /// price says so on its definition. The field was arriving and going
    /// nowhere, so a caller pricing such a contract had nothing to price it by.
    #[test]
    fn the_economic_value_rule_is_read_from_the_definition() {
        let def = parse_secdef_response(&secdef("6858=IND-FUT-CASH\u{1}6859=0.25\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.ev_rule, "IND-FUT-CASH");
        assert_eq!(def.ev_multiplier, 0.25, "a rule without its multiplier values the contract wrongly");
    }

    /// The venue states what the issuer does as one field with bars between,
    /// broadest first. Kept whole, a caller asking for the category was handed
    /// all three of them with bars in the middle.
    #[test]
    fn what_the_issuer_does_arrives_as_three_things() {
        let def = parse_secdef_response(&secdef("6624=Technology|Computers|Computers\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.industry, "Technology");
        assert_eq!(def.category, "Computers");
        assert_eq!(def.subcategory, "Computers");
    }

    /// A contract's CUSIP has no field of its own on this wire: it is one of
    /// the identifiers the contract is known by, picked out by its kind. Only
    /// the ISIN was picked out, so a caller asking for a CUSIP got nothing
    /// while the CUSIP sat in a list that was thrown away.
    #[test]
    fn the_identifiers_are_kept_and_the_cusip_picked_out_of_them() {
        let def = parse_secdef_response(&secdef(
            "455=US0378331005\u{1}456=4\u{1}455=037833100\u{1}456=1\u{1}",
        ), true).expect("the definition parses");
        assert_eq!(def.isin, "US0378331005");
        assert_eq!(def.cusip, "037833100", "the CUSIP is in the list, by its kind");
        assert_eq!(def.sec_id_list.len(), 2, "and every identifier is kept");
    }

    /// A bond is its terms: what it pays, when it can be called, what it is
    /// rated. Read from nothing, a caller asking about a bond received a
    /// contract with none of what makes it one.
    #[test]
    fn a_bond_carries_its_terms() {
        let def = parse_secdef_response(&secdef(
            "223=4.25\u{1}6495=CORP\u{1}6496=FIXED\u{1}6497=1\u{1}6498=0\u{1}\
             6499=0\u{1}6501=20270115\u{1}6502=CALL\u{1}6720=A+\u{1}6493=notes\u{1}",
        ), true).expect("the definition parses");
        assert_eq!(def.coupon, 4.25);
        assert_eq!(def.bond_type, "CORP");
        assert_eq!(def.coupon_type, "FIXED");
        assert!(def.callable, "this one can be called");
        assert!(!def.puttable, "and cannot be put");
        assert!(!def.convertible);
        assert_eq!(def.next_option_date, "20270115");
        assert_eq!(def.next_option_type, "CALL");
        assert_eq!(def.ratings, "A+");
        assert_eq!(def.bond_notes, "notes");
    }

    /// A fund is what it charges and what it is closed to. Without those it is
    /// a symbol.
    #[test]
    fn a_fund_carries_what_it_charges_and_what_it_is_closed_to() {
        let def = parse_secdef_response(&secdef(
            "6481=Some Fund\u{1}6472=Some Family\u{1}6473=Bond\u{1}6474=1.5\u{1}\
             6475=0.5\u{1}6476=0.75\u{1}6477=0\u{1}6511=1\u{1}6512=1\u{1}\
             8150=NY,CA\u{1}8503=Fixed Income\u{1}",
        ), true).expect("the definition parses");
        assert_eq!(def.fund_name, "Some Fund");
        assert_eq!(def.fund_family, "Some Family");
        assert_eq!(def.fund_type, "Bond");
        assert_eq!(def.fund_front_load, "1.5");
        assert_eq!(def.fund_management_fee, "0.75");
        assert!(!def.fund_closed, "open to existing holders");
        assert!(def.fund_closed_for_new_investors, "and closed to new ones");
        assert!(def.fund_closed_for_new_money);
        assert_eq!(def.fund_blue_sky_states, "NY,CA");
        assert_eq!(def.fund_asset_type, "Fixed Income");
    }

    /// The venue states a short description under its own field where it has
    /// one, and under a shorter field where that is all it gives.
    #[test]
    fn a_description_falls_back_to_the_shorter_field() {
        let def = parse_secdef_response(&secdef("6853=short form\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.desc_append, "short form");
    }

    /// A price quoted in a fraction of the currency is out by that fraction
    /// unless the multiplier comes with it.
    #[test]
    fn a_price_carries_what_it_must_be_multiplied_by() {
        let def = parse_secdef_response(&secdef("6021=100\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.price_magnifier, 100);
    }

    /// A value stating one thing is the category, which is what a caller asking
    /// for a category means by it.
    #[test]
    fn one_thing_stated_is_the_category() {
        let def = parse_secdef_response(&secdef("6624=Financial\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.category, "Financial");
        assert!(def.industry.is_empty() && def.subcategory.is_empty());
    }
}
mod unread_tag_tests {
    use super::super::*;

    /// The list of tags the parser reads is derived from the parser itself, so
    /// it cannot fall behind as fields are added.
    #[test]
    fn the_tags_read_include_the_ones_known_to_be_read() {
        let read = tags_read_from_a_definition();
        for known in [TAG_IB_CON_ID, TAG_EV_RULE, 6577, 6624] {
            assert!(read.contains(&known), "{known} is read but not reported as read");
        }
        assert!(read.len() > 40, "only {} tags reported as read", read.len());
    }

    /// A tag the venue sends that nothing reads is named, so the gap is
    /// measurable rather than suspected.
    #[test]
    fn a_tag_that_arrives_and_is_dropped_is_named() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x019999=something\x01";
        let unread = unread_definition_tags(frame);
        assert!(unread.contains(&9999), "an unread tag was not reported");
        assert!(!unread.contains(&6008), "a tag that is read was reported unread");
    }
}
mod unnamed_field_tests {
    use super::super::*;

    fn frame(extra: &str) -> Vec<u8> {
        format!("35=d\u{1}320=R1\u{1}6008=756733\u{1}55=SPY\u{1}167=CS\u{1}\
                 207=SMART\u{1}15=USD\u{1}{extra}").into_bytes()
    }

    /// A field the venue stated and this client does not name is still a fact
    /// about the contract. Dropping it put it beyond reach with nothing to say
    /// it had ever arrived.
    #[test]
    fn a_field_this_client_does_not_name_is_kept_under_its_number() {
        // Numbers this parser names none of, so the test does not go stale the
        // day one of them is given a name.
        let def = parse_secdef_response(&frame("9998=something\u{1}9997=42\u{1}"), true)
            .expect("the definition parses");
        let kept: Vec<u32> = def.unnamed_fields.iter().map(|(t, _)| *t).collect();
        assert!(kept.contains(&9998));
        assert!(kept.contains(&9997));
        let value = def.unnamed_fields.iter().find(|(t, _)| *t == 9998).unwrap();
        assert_eq!(value.1, "something");
    }

    /// The message envelope is not part of the contract. Counting those among
    /// its fields overstated what was unread, by ten.
    #[test]
    fn the_messages_own_fields_are_not_counted_as_the_contracts() {
        let def = parse_secdef_response(&frame(""), true).expect("the definition parses");
        let kept: Vec<u32> = def.unnamed_fields.iter().map(|(t, _)| *t).collect();
        for envelope in [8, 9, 10, 34, 35, 52, 320] {
            assert!(!kept.contains(&envelope), "{envelope} is the message's, not the contract's");
        }
    }

    /// A field that is named is read into its own place, not left as a number.
    #[test]
    fn a_field_this_client_names_does_not_also_appear_unnamed() {
        let def = parse_secdef_response(&frame("6858=IND-FUT-CASH\u{1}6859=0.25\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.ev_rule, "IND-FUT-CASH");
        assert_eq!(def.ev_multiplier, 0.25, "a rule without its multiplier values the contract wrongly");
        for named in [TAG_EV_RULE, TAG_EV_MULTIPLIER] {
            assert!(!def.unnamed_fields.iter().any(|(t, _)| *t == named), "tag {named} is named");
        }
    }
}
mod smart_venue_tests {
    use super::super::*;

    /// The venue states which venues SMART routes a contract to, and the order
    /// it states them in is the order a quote's exchange bitmask refers to.
    /// This client had its own list in its own order, which bore no
    /// resemblance, so every quote's bid, ask and last named the wrong venue.
    #[test]
    fn the_venues_own_routing_list_is_read_in_the_order_it_states() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01\
                      6177=AMEX,NYSE,CHX,ARCA,NASDAQ,DRCTEDGE,BEX,BATS,EDGE\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(
            def.smart_venues,
            vec!["AMEX", "NYSE", "CHX", "ARCA", "NASDAQ", "DRCTEDGE", "BEX", "BATS", "EDGE"],
        );
    }

    /// A contract listed on one venue is routed nowhere, and the venue sends
    /// no list for it. That is empty, not missing.
    #[test]
    fn a_contract_that_is_not_smart_routed_lists_no_venues() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=VOD\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert!(def.smart_venues.is_empty());
    }

    /// A venue's letter is the server's to state, and nothing is stated for
    /// one it has not named.
    ///
    /// A table of venues written into this client's own source would name
    /// nothing for every venue absent from it — most of the United States, and
    /// all of everywhere else — with nothing to check
    /// it against what the server assigns. The counterpart carries no such
    /// table either: it reads the map off the wire.
    #[test]
    fn a_venues_letter_is_not_this_clients_to_invent() {
        use crate::types::exchange_letter;
        assert_eq!(exchange_letter("NASDAQ"), "");
        assert_eq!(exchange_letter("SOMEWHERE"), "");
    }
}
mod underlying_tests {
    use super::super::*;

    /// An option is written on something, and its definition says what. These
    /// were settled from a real reply rather than inferred: the id the venue
    /// sent was the id of the share the option is written on, and the symbol
    /// beside it was that share's symbol.
    #[test]
    fn a_derivative_names_the_contract_it_is_written_on() {
        let frame = b"35=d\x01320=R1\x016008=36233584\x0155=SPY\x01167=OPT\x01\
                      6346=756733\x016855=SPY\x01310=STK\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.under_con_id, 756733);
        assert_eq!(def.under_symbol, "SPY");
        assert_eq!(def.under_sec_type, "STK");
        // And it is not the contract's own id.
        assert_ne!(def.under_con_id, def.con_id);
    }

    /// Trading stops at a time of day, not only on a date. A caller holding
    /// the date alone knows which day a contract stops and not when, and an
    /// option that stops at three in the afternoon is not one that stops at
    /// the close.
    #[test]
    fn the_time_trading_stops_is_read_as_well_as_the_date() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=SPY\x018583=20260918\x018584=150000\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.last_trade_time, "150000");
    }

    /// A share is written on nothing, and says so by sending none of them.
    #[test]
    fn a_contract_written_on_nothing_names_nothing() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01167=CS\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.under_con_id, 0);
        assert!(def.under_symbol.is_empty());
    }
}
mod issue_date_tests {
    use super::super::*;

    /// A bond was issued on a day, and its definition states which. Only a
    /// contract with an issuer states it, which is why asking about shares
    /// never saw the field and it looked absent from the wire.
    #[test]
    fn a_bond_states_the_day_it_was_issued() {
        let frame = b"35=d\x01320=R1\x016008=851160433\x0155=IBM\x01167=CORP\x01225=20260203\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.issue_date, "20260203");
    }

    /// A share has no issuer in this sense and states none, which is empty
    /// rather than missing.
    #[test]
    fn a_share_states_no_issue_date() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01167=CS\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert!(def.issue_date.is_empty());
    }
}
mod repeated_field_tests {
    use super::super::*;

    /// A definition repeats tags: a market rule states an increment per price
    /// band. Keeping only the last of each turns a schedule of tick sizes into
    /// a single number with no way to tell it was ever a schedule.
    #[test]
    fn a_tag_stated_more_than_once_is_kept_more_than_once() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=SPY\x01\
                      9001=alpha\x019001=beta\x019001=gamma\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        let values: Vec<&str> = def
            .unnamed_fields
            .iter()
            .filter(|(t, _)| *t == 9001)
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(values, vec!["alpha", "beta", "gamma"]);
    }
}
mod record_boundary_tests {
    use super::super::*;

    /// A contract's fields run until the next contract is named. The symbol tag
    /// is reused inside the identifier block, and treating that as the start of
    /// a new contract cut every contract short: in a reply naming fifty bonds,
    /// forty-nine came back holding almost nothing while the last held
    /// everything, because only the last had nothing after it to be cut by.
    #[test]
    fn a_contract_keeps_what_it_states_after_its_own_identifier_block() {
        let data = b"35=d\x01320=R1\x01\
                     55=AAA\x01167=CORP\x016008=111\x01\
                     55=BBG\x01456=A\x01\
                     225=20260101\x01\
                     55=ZZZ\x01167=CORP\x016008=222\x01\
                     225=20270202\x01";
        let defs = parse_secdef_responses(data, true);
        assert_eq!(defs.len(), 2, "two contracts are named");
        assert_eq!(defs[0].con_id, 111);
        assert_eq!(defs[1].con_id, 222);
        // The field stated after the first contract's identifier block belongs
        // to that contract.
        assert_eq!(defs[0].issue_date, "20260101");
        assert_eq!(defs[1].issue_date, "20270202");
    }

    /// A field stated after the last contract still belongs to it.
    #[test]
    fn the_last_contract_keeps_what_follows_it() {
        let data = b"35=d\x01320=R1\x01\
                     55=AAA\x01167=CORP\x016008=111\x01\
                     55=BBG\x01456=A\x01225=20260101\x01";
        let defs = parse_secdef_responses(data, true);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].issue_date, "20260101");
    }
}
mod size_and_precision_tests {
    use super::super::*;

    /// The smallest order a venue will take is stated only where a contract can
    /// be dealt in fractions, and gated by the flag that says so. Read from
    /// the field stating a size, not the one stating how many places a price is
    /// quoted to.
    #[test]
    fn the_smallest_order_is_read_from_the_size_field_not_a_price_precision() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01\
                      8193=1\x018175=0.0001\x018598=0.01\x018599=0.000001\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.min_size, 0.0001);
        assert_eq!(def.last_price_precision, 0.01);
        assert_eq!(def.last_size_precision, 0.000001);
    }

    /// A contract that cannot be dealt in fractions states no smallest order,
    /// and the size is not in force without the flag that admits it.
    #[test]
    fn a_size_stated_without_the_flag_is_not_in_force() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=SPY\x018175=0.0001\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.min_size, 0.0);
    }

    /// How a contract settles, and the day it really stops trading.
    #[test]
    fn how_a_contract_settles_is_read() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=ES\x016660=C\x016614=20260918\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.settlement_method, "C");
        assert_eq!(def.real_expiration_date, "20260918");
    }
}
mod size_table_tests {
    use super::super::*;

    /// A rule states two tables under the same tags: price bands first, then
    /// size bands after a second count. The count opens the second table
    /// rather than ending the rule, so the sizes a contract may be dealt in
    /// are read.
    #[test]
    fn a_rules_size_table_is_read_as_well_as_its_price_table() {
        let data = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01\
                     6019=1\x016031=26\x01\
                     6026=1\x016023=0\x016027=0.01\x01\
                     6030=1\x016023=0\x016027=40\x01";
        let rules = parse_market_rules(data);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].price_increments.len(), 1, "the price band is read");
        assert_eq!(rules[0].price_increments[0].increment, 0.01);
        assert_eq!(rules[0].size_increments.len(), 1, "the size band is read");
        assert_eq!(rules[0].size_increments[0].increment, 40.0);

        let def = parse_secdef_response(data, true).expect("the definition parses");
        assert_eq!(def.min_tick, 0.01, "the price table gives the tick");
        assert_eq!(def.size_increment, 40.0, "the size table gives the size");
    }

    /// A rule stating only price bands leaves the size unset rather than
    /// borrowing the price increment for it.
    #[test]
    fn a_rule_with_no_size_table_states_no_size() {
        let data = b"35=d\x01320=R1\x016008=1\x0155=SPY\x01\
                     6019=1\x016031=26\x016026=1\x016023=0\x016027=0.05\x01";
        let def = parse_secdef_response(data, true).expect("the definition parses");
        assert_eq!(def.min_tick, 0.05);
        assert_eq!(def.size_increment, 0.0);
    }
}
mod delivered_name_tests {
    use super::super::delivered_exchange;

    /// A US stock on Nasdaq is handed back under the older spelling, the way
    /// the counterpart hands it back — so a program written against that one
    /// compares the same here.
    #[test]
    fn a_us_stock_on_nasdaq_is_handed_back_as_island() {
        assert_eq!(delivered_exchange("NASDAQ", "STK", "USD", true), "ISLAND");
    }

    /// Nothing else is. The older name means nothing for a future, and
    /// nothing outside the United States trades under it.
    #[test]
    fn nothing_else_is_renamed() {
        assert_eq!(delivered_exchange("NASDAQ", "FUT", "USD", true), "NASDAQ");
        assert_eq!(delivered_exchange("NASDAQ", "STK", "CAD", true), "NASDAQ");
        assert_eq!(delivered_exchange("ARCA", "STK", "USD", true), "ARCA");
        assert_eq!(delivered_exchange("", "STK", "USD", true), "");
    }

    /// A session that wants the venue's own name says so, and says it for
    /// itself, rather than from the process, so one session stating it does
    /// not state it for every other session running beside it.
    #[test]
    fn the_venues_own_name_can_be_asked_for() {
        assert_eq!(delivered_exchange("NASDAQ", "STK", "USD", false), "NASDAQ");
        assert_eq!(delivered_exchange("NASDAQ", "STK", "USD", true), "ISLAND");
    }

    /// And what goes out still routes under the venue's own name, whatever a
    /// caller was handed: the older spelling reaches nothing.
    #[test]
    fn what_goes_out_still_routes_as_nasdaq() {
        assert_eq!(super::super::exchange_to_fix("ISLAND"), "NASDAQ");
    }
}
