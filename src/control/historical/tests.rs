//! The tests for this module.
//!
//! One file per module, as `api/client` already does it. Each block below
//! reaches the code it tests through `super::super`, which is the module this
//! file belongs to.

use super::*;

#[test]
fn bar_data_type_strings() {
    assert_eq!(BarDataType::Trades.as_str(), "Last");
    assert_eq!(BarDataType::Midpoint.as_str(), "MidPoint");
    assert_eq!(BarDataType::BidAsk.as_str(), "BidAsk");
}

#[test]
fn bar_size_strings() {
    assert_eq!(BarSize::Min5.as_str(), "5 mins");
    assert_eq!(BarSize::Hour1.as_str(), "1 hour");
        assert_eq!(BarSize::Day1.as_str(), "1 day");
}

// ── single parse table, rejection instead of Min5/TRADES ──

#[test]
fn bar_size_from_api_str_accepts_all_official_strings() {
    let all = [
        "1 secs", "5 secs", "10 secs", "15 secs", "30 secs",
        "1 min", "2 mins", "3 mins", "5 mins", "10 mins", "15 mins",
        "20 mins", "30 mins", "1 hour", "2 hours", "3 hours", "4 hours",
            "8 hours", "1 day", "1 week", "1 month",
    ];
    for s in all {
        assert!(BarSize::from_api_str(s).is_ok(), "'{s}' must parse");
    }
    assert_eq!(BarSize::from_api_str("1 min").unwrap(), BarSize::Min1);
}

#[test]
fn bar_size_from_api_str_rejects_unknown_and_wrong_case() {
    // The issue's exact repro: "1 Min" silently became 5-minute bars.
    for s in ["1 Min", "1min", "1 minute", "7 mins", ""] {
        let err = BarSize::from_api_str(s).unwrap_err();
        assert!(err.contains("bar_size"), "'{s}' -> {err}");
    }
}

#[test]
fn bar_size_keep_up_to_date_support() {
    for s in ["1 secs", "5 secs", "5 mins", "1 hour", "1 day"] {
            assert!(BarSize::from_api_str(s).unwrap().supports_keep_up_to_date(), "{}", s);
    }
    for s in ["10 secs", "1 min", "15 mins", "4 hours", "1 week"] {
        assert!(!BarSize::from_api_str(s).unwrap().supports_keep_up_to_date(), "{}", s);
    }
}

#[test]
fn bar_data_type_from_api_str() {
    assert_eq!(BarDataType::from_api_str("TRADES").unwrap(), BarDataType::Trades);
    assert_eq!(BarDataType::from_api_str("trades").unwrap(), BarDataType::Trades);
    assert_eq!(BarDataType::from_api_str("").unwrap(), BarDataType::Trades);
    assert_eq!(BarDataType::from_api_str("BID_ASK").unwrap(), BarDataType::BidAsk);
    // A misspelled value is refused rather than answered with trade bars.
    assert!(BarDataType::from_api_str("TRADE").is_err());
    assert!(BarDataType::from_api_str("BIDD").is_err());
}

#[test]
fn build_query_xml_structure() {
    let req = HistoricalRequest {
        query_id: "q1".to_string(),
        con_id: 265598,
        symbol: "AAPL".to_string(),
        sec_type: "CS".to_string(),
        exchange: "SMART".to_string(),
        data_type: BarDataType::Trades,
        end_time: "20260228-15:00:00".to_string(),
        duration: "1 d".to_string(),
        bar_size: BarSize::Min5,
        use_rth: true,
        keep_up_to_date: false,
        include_expired: false,
    };
    let xml = build_query_xml(&req);
    assert!(xml.contains("<id>q1</id>"));
    assert!(xml.contains("<contractID>265598</contractID>"));
    assert!(xml.contains("<exchange>BEST</exchange>")); // SMART→BEST
    assert!(xml.contains("<data>Last</data>"));
    assert!(xml.contains("<step>5 mins</step>"));
    assert!(xml.contains("<useRTH>true</useRTH>"));
    assert!(xml.contains("<timeLength>1 d</timeLength>"));
}

#[test]
fn build_fix_request() {
    let req = HistoricalRequest {
        query_id: "q1".to_string(),
        con_id: 265598,
        symbol: "AAPL".to_string(),
        sec_type: "CS".to_string(),
        exchange: "SMART".to_string(),
        data_type: BarDataType::Trades,
        end_time: "20260228-15:00:00".to_string(),
        duration: "1 d".to_string(),
        bar_size: BarSize::Min5,
        use_rth: true,
        keep_up_to_date: false,
        include_expired: false,
    };
    let msg = build_historical_request(&req, 1);
    let tags = fix::fix_parse(&msg);
    assert_eq!(tags[&fix::TAG_MSG_TYPE], "W");
    assert!(tags[&TAG_HISTORICAL_XML].contains("<ListOfQueries>"));
}

#[test]
fn cancel_request_structure() {
    let msg = super::build_cancel_request("12345", 1);
    let tags = fix::fix_parse(&msg);
    assert_eq!(tags[&fix::TAG_MSG_TYPE], "Z");
    assert!(tags[&TAG_HISTORICAL_XML].contains("ticker:12345"));
}

#[test]
fn parse_bar_response_basic() {
    let xml = r#"<ResultSetBar>
            <id>q1</id>
        <eoq>true</eoq>
        <tz>US/Eastern</tz>
        <Events>
            <Open><time>20260227-14:30:00</time></Open>
            <Bar>
                <time>20260227-14:30:00</time>
                <open>272.77</open>
                <close>269.47</close>
                <high>272.81</high>
                <low>269.2</low>
                <weightedAvg>270.998</weightedAvg>
                <volume>1411775</volume>
                <count>5165</count>
            </Bar>
            <Bar>
                <time>20260227-14:35:00</time>
                <open>269.48</open>
                <close>270.10</close>
                <high>270.50</high>
                <low>269.30</low>
                <weightedAvg>269.90</weightedAvg>
                <volume>500000</volume>
                <count>2000</count>
            </Bar>
            <Close><time>20260227-21:00:00</time></Close>
        </Events>
    </ResultSetBar>"#;

    let resp = parse_bar_response(xml).unwrap();
    assert_eq!(resp.query_id, "q1");
    assert_eq!(resp.timezone, "US/Eastern");
    assert!(resp.is_complete);
    assert_eq!(resp.bars.len(), 2);

    let bar = &resp.bars[0];
    assert_eq!(bar.time, "20260227-14:30:00");
    assert_eq!(bar.open, 272.77);
    assert_eq!(bar.high, 272.81);
    assert_eq!(bar.low, 269.2);
    assert_eq!(bar.close, 269.47);
    assert_eq!(bar.volume, 1411775);
    assert_eq!(bar.wap, 270.998);
    assert_eq!(bar.count, 5165);

    let bar2 = &resp.bars[1];
    assert_eq!(bar2.time, "20260227-14:35:00");
    assert_eq!(bar2.close, 270.10);
}

#[test]
fn parse_bar_response_incomplete() {
    let xml = r#"<ResultSetBar>
            <id>q2</id>
        <eoq>false</eoq>
        <tz>US/Eastern</tz>
        <Events>
            <Bar>
                <time>20260227-14:30:00</time>
                <open>100.0</open>
                <close>101.0</close>
                <high>102.0</high>
                <low>99.0</low>
                <volume>1000</volume>
                <count>10</count>
            </Bar>
        </Events>
    </ResultSetBar>"#;

    let resp = parse_bar_response(xml).unwrap();
    assert!(!resp.is_complete);
    assert_eq!(resp.bars.len(), 1);
}

#[test]
fn parse_bar_response_rejects_non_bar() {
    assert!(parse_bar_response("<ResultSetTickerId>...").is_none());
    assert!(parse_bar_response("not xml at all").is_none());
}

#[test]
fn parse_ticker_id() {
    let xml = r#"<ResultSetTickerId>
            <id>q1</id>
        <tickerId>42</tickerId>
    </ResultSetTickerId>"#;
    assert_eq!(super::parse_ticker_id(xml), Some("42".to_string()));
}

/// A tick-by-tick assignment names it differently, exactly as it arrives
/// from the server. Reading only `tickerId` left the subscription unbound
/// and no tick could be routed to it.
#[test]
fn parse_ticker_id_reads_the_tick_by_tick_spelling() {
    let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\t<ResultSetTickerId>\n\t\t                   <id>tbt_1</id>\n\t\t<rtTickerId>1</rtTickerId>\n\t\t                   <minTick>0.00005</minTick>\n\t\t<sizeMinTick>1</sizeMinTick>\n\t\t                   <eoq>false</eoq>\n\t</ResultSetTickerId>\n";
    assert_eq!(super::parse_ticker_id(xml), Some("1".to_string()));
}

#[test]
fn parse_ticker_id_rejects_other() {
    assert!(super::parse_ticker_id("<ResultSetBar>...</ResultSetBar>").is_none());
}

/// The contract's own security type and exchange have to reach the query.
/// Hardcoding them described a stock on SMART whatever was asked for, and
/// anything venue-specific is rejected with error 162.
#[test]
fn a_futures_query_carries_its_own_sec_type_and_exchange() {
    let req = HistoricalRequest {
        query_id: "hist_1".into(),
        con_id: 793356225,
        symbol: "MNQ".into(),
        sec_type: "FUT".into(),
        exchange: "CME".into(),
        data_type: BarDataType::Trades,
        end_time: String::new(),
        duration: "2 D".into(),
        bar_size: BarSize::Day1,
        use_rth: false,
        keep_up_to_date: false,
        include_expired: false,
    };
    let xml = build_query_xml(&req);
    assert!(xml.contains("<secType>FUT</secType>"), "got: {xml}");
    assert!(xml.contains("<exchange>CME</exchange>"), "got: {xml}");
    assert!(!xml.contains("BEST"), "a futures query must not be routed to BEST: {xml}");
}

/// SMART still maps to BEST, which is what stock callers relied on.
#[test]
fn a_smart_query_still_routes_to_best() {
    let req = HistoricalRequest {
        query_id: "hist_2".into(),
        con_id: 756733,
        symbol: "SPY".into(),
        sec_type: "CS".into(),
        exchange: "SMART".into(),
        data_type: BarDataType::Trades,
        end_time: String::new(),
        duration: "1 D".into(),
        bar_size: BarSize::Day1,
        use_rth: true,
        keep_up_to_date: false,
        include_expired: false,
    };
    let xml = build_query_xml(&req);
    assert!(xml.contains("<exchange>BEST</exchange>"), "got: {xml}");
    assert!(xml.contains("<secType>CS</secType>"), "got: {xml}");
}

#[test]
fn head_timestamp_xml_structure() {
    let req = HeadTimestampRequest {
        con_id: 756733,
        sec_type: "STK".to_string(),
        exchange: "SMART".to_string(),
        data_type: BarDataType::Trades,
        use_rth: true,
    };
    let xml = build_head_timestamp_xml(&req);
    assert!(xml.contains("<type>TickHeadTimeStamp</type>"));
    assert!(xml.contains("<contractID>756733</contractID>"));
    assert!(xml.contains("<exchange>BEST</exchange>")); // SMART→BEST
    assert!(xml.contains("<data>Last</data>"));
    assert!(xml.contains("<step>-1</step>"));
    assert!(xml.contains("<useRTH>true</useRTH>"));
    assert!(xml.contains("TickHeadClient1;;756733@BEST Last;;0;;true;;0;;U"));
}

#[test]
fn parse_head_timestamp_response_basic() {
    let xml = r#"<ResultSetHeadTimeStamp>
            <id>TickHeadClient1;;756733@BEST Last;;0;;true;;0;;U</id>
        <eoq>true</eoq>
        <headTS>19930129-09:00:00</headTS>
        <tz>US/Eastern</tz>
        <Events>
            <Open><time>19930129-14:30:00</time><refDate>19930129</refDate></Open>
            <Close><time>19930129-21:15:00</time></Close>
        </Events>
    </ResultSetHeadTimeStamp>"#;
    let resp = parse_head_timestamp_response(xml).unwrap();
    assert_eq!(resp.head_timestamp, "19930129-09:00:00");
    assert_eq!(resp.timezone, "US/Eastern");
}

#[test]
fn parse_head_timestamp_rejects_other() {
    assert!(parse_head_timestamp_response("<ResultSetBar>...</ResultSetBar>").is_none());
    assert!(parse_head_timestamp_response("not xml").is_none());
}

#[test]
fn build_schedule_xml_structure() {
    let xml = build_schedule_xml("sched_1", 756733, "20260312-19:34:06", "5 d", true, "CS", "BEST");
    assert!(xml.contains("<id>sched_1</id>"));
    assert!(xml.contains("<contractID>756733</contractID>"));
    assert!(xml.contains("<data>Schedule</data>"));
    assert!(xml.contains("<scheduleOnly>true</scheduleOnly>"));
    assert!(xml.contains("<step>1 day</step>"));
    assert!(xml.contains("<useRTH>true</useRTH>"));
    assert!(xml.contains("<timeLength>5 d</timeLength>"));
}

#[test]
fn parse_schedule_response_basic() {
    let xml = r#"<ResultSetSchedule>
            <id>sched_1</id>
        <eoq>true</eoq>
        <tz>US/Eastern</tz>
        <derivedStart>20260306-14:30:00</derivedStart>
        <Events>
            <Open><time>20260306-14:30:00</time><refDate>20260306</refDate></Open>
            <Close><time>20260306-21:00:00</time></Close>
            <Open><time>20260309-14:30:00</time><refDate>20260309</refDate></Open>
            <Close><time>20260309-21:00:00</time></Close>
        </Events>
    </ResultSetSchedule>"#;

    let resp = parse_schedule_response(xml).unwrap();
    assert_eq!(resp.query_id, "sched_1");
    assert_eq!(resp.timezone, "US/Eastern");
    assert_eq!(resp.start_date_time, "20260306-14:30:00");
    assert_eq!(resp.sessions.len(), 2);
    assert_eq!(resp.sessions[0].ref_date, "20260306");
    assert_eq!(resp.sessions[0].open_time, "20260306-14:30:00");
    assert_eq!(resp.sessions[0].close_time, "20260306-21:00:00");
    assert_eq!(resp.sessions[1].ref_date, "20260309");
}

#[test]
fn parse_schedule_response_rejects_other() {
    assert!(parse_schedule_response("<ResultSetBar>...</ResultSetBar>").is_none());
    assert!(parse_schedule_response("not xml").is_none());
}

#[test]
fn build_tick_query_xml_structure() {
    let xml = build_tick_query_xml("tk_1", 265598, "", "20260312-15:00:00", 100, "TRADES", true, "CS", "BEST", false);
    assert!(xml.contains("<id>tk_1</id>"));
    assert!(xml.contains("<type>TickData</type>"));
    assert!(xml.contains("<data>AllLast</data>"));
    assert!(xml.contains("<step>ticks</step>"));
    assert!(xml.contains("<timeLength>100 t</timeLength>"));
    assert!(xml.contains("<wholeDays>true</wholeDays>"));
}

#[test]
fn build_tick_query_xml_bid_ask() {
    let xml = build_tick_query_xml("tk_2", 265598, "", "20260312-15:00:00", 50, "BID_ASK", false, "CS", "BEST", false);
    assert!(xml.contains("<data>BidAsk</data>"));
    assert!(xml.contains("<useRTH>false</useRTH>"));
}

/// The query counts back from its end. A start written into that same field
/// asked for the ticks before the moment the caller wanted the ticks after, and
/// the answer looked right and covered the wrong side of the clock.
#[test]
fn a_tick_request_names_one_end_and_counts_from_it() {
    use crate::control::historical::{build_tick_query_xml, validate_tick_window};
    let q = |start: &str, end: &str| build_tick_query_xml(
        "tk", 265598, start, end, 100, "TRADES", true, "CS", "BEST", false,
    );
    // Either end is served, and the count says how far it reaches. A start
    // used to be refused before it was ever sent; the venue answers one with
    // a hundred and twenty-five ticks.
    let from_a_start = q("20260312-09:30:00", "");
    assert!(from_a_start.contains("<startTime>20260312-09:30:00</startTime>"));
    assert!(!from_a_start.contains("<endTime>"), "one end, not two");
    assert!(from_a_start.contains("<timeLength>100 t</timeLength>"));

    let to_an_end = q("", "20260312-15:00:00");
    assert!(to_an_end.contains("<endTime>20260312-15:00:00</endTime>"));
    assert!(!to_an_end.contains("<startTime>"));
    assert!(to_an_end.contains("<timeLength>100 t</timeLength>"));

    // The two shapes the venue refuses, refused before they are sent, in its
    // own words: neither end leaves it a parameter short, and both without a
    // count is not served to an API client at all.
    assert!(validate_tick_window("", "").is_err());
    assert!(validate_tick_window("20260312-09:30:00", "20260312-15:00:00").is_err());
    assert!(validate_tick_window("20260312-09:30:00", "").is_ok());
    assert!(validate_tick_window("", "20260312-15:00:00").is_ok());
}

/// A historical size crosses as text because it can be a fraction of a share.
/// Read as a whole number, a fractional print was a print of nothing.
#[test]
fn a_fractional_historical_size_is_read_as_the_fraction_it_states() {
    let xml = r#"<ResultSetTick>
            <id>tk_frac</id>
        <eoq>true</eoq>
        <tz>US/Eastern</tz>
        <Events>
            <Tick><time>20260312-14:30:01</time><price>150.25</price><size>0.5</size><exchange>NASDAQ</exchange></Tick>
        </Events>
    </ResultSetTick>"#;
    let (_qid, data, _done) = parse_tick_response(xml, "TRADES").unwrap();
    match data {
        crate::types::HistoricalTickData::Last(ticks) => {
            assert_eq!(ticks.len(), 1);
            assert_eq!(ticks[0].size, 0.5, "half a share is half a share, not none");
        }
        _ => panic!("Expected Last variant"),
    }
}

#[test]
fn parse_tick_response_trades() {
    let xml = r#"<ResultSetTick>
            <id>tk_1</id>
        <eoq>true</eoq>
        <tz>US/Eastern</tz>
        <Events>
            <Tick><time>20260312-14:30:01</time><price>150.25</price><size>100</size><exchange>NASDAQ</exchange><specialConditions></specialConditions></Tick>
            <Tick><time>20260312-14:30:02</time><price>150.30</price><size>200</size><exchange>NYSE</exchange><specialConditions>I</specialConditions></Tick>
        </Events>
    </ResultSetTick>"#;
    let (qid, data, done) = parse_tick_response(xml, "TRADES").unwrap();
    assert_eq!(qid, "tk_1");
    assert!(done);
    match data {
        crate::types::HistoricalTickData::Last(ticks) => {
            assert_eq!(ticks.len(), 2);
            assert_eq!(ticks[0].price, 150.25);
            assert_eq!(ticks[0].size, 100.0);
            assert_eq!(ticks[0].exchange, "NASDAQ");
            assert_eq!(ticks[1].special_conditions, "I");
        }
        _ => panic!("Expected Last variant"),
    }
}

#[test]
fn parse_tick_response_bid_ask() {
    let xml = r#"<ResultSetTick>
            <id>tk_2</id>
        <eoq>true</eoq>
        <Events>
            <Tick><time>20260312-14:30:01</time><priceBid>150.24</priceBid><priceAsk>150.26</priceAsk><sizeBid>500</sizeBid><sizeAsk>600</sizeAsk></Tick>
        </Events>
    </ResultSetTick>"#;
    let (_, data, _) = parse_tick_response(xml, "BID_ASK").unwrap();
    match data {
        crate::types::HistoricalTickData::BidAsk(ticks) => {
            assert_eq!(ticks.len(), 1);
            assert_eq!(ticks[0].bid_price, 150.24);
            assert_eq!(ticks[0].ask_price, 150.26);
        }
        _ => panic!("Expected BidAsk variant"),
    }
}

#[test]
fn parse_tick_response_midpoint() {
    let xml = r#"<ResultSetTick>
            <id>tk_3</id>
        <eoq>true</eoq>
        <Events>
            <Tick><time>20260312-14:30:01</time><price>150.25</price></Tick>
        </Events>
    </ResultSetTick>"#;
    let (_, data, _) = parse_tick_response(xml, "MIDPOINT").unwrap();
    match data {
        crate::types::HistoricalTickData::Midpoint(ticks) => {
            assert_eq!(ticks.len(), 1);
            assert_eq!(ticks[0].price, 150.25);
        }
        _ => panic!("Expected Midpoint variant"),
    }
}

/// The venue's interest-rate series is a value with a moment, not a print.
///
/// Read through the trade decoder it arrived as a trade, with a size of zero
/// and no venue — a rate reported as something that changed hands.
#[test]
fn the_interest_rate_series_is_not_read_as_a_trade() {
    let xml = r#"<ResultSetTick>
            <id>tk_4</id>
        <eoq>true</eoq>
        <Events>
            <Tick><time>20260312-14:30:01</time><price>0.0425</price></Tick>
        </Events>
    </ResultSetTick>"#;
    let (_, data, _) = parse_tick_response(xml, "OPTION_EXERCISE_INTEREST_RATE").unwrap();
    match data {
        crate::types::HistoricalTickData::Midpoint(ticks) => {
            assert_eq!(ticks.len(), 1);
            assert_eq!(ticks[0].price, 0.0425);
        }
        other => panic!("a rate is not a trade: {other:?}"),
    }
}

#[test]
fn parse_tick_response_rejects_other() {
    assert!(parse_tick_response("<ResultSetBar>...</ResultSetBar>", "TRADES").is_none());
}

#[test]
fn build_realtime_bar_xml_structure() {
    let xml = build_realtime_bar_xml("rt_1", 265598, "TRADES", true, "CS", "BEST");
    // The contract is stated, not assumed: an FX pair is not a US stock.
    let fx = build_realtime_bar_xml("rt_2", 12087792, "MIDPOINT", false, "CASH", "IDEALPRO");
    assert!(fx.contains("<secType>CASH</secType>"), "{fx}");
    assert!(fx.contains("<exchange>IDEALPRO</exchange>"), "{fx}");
    assert!(fx.contains("<data>MidPoint</data>"), "{fx}");
    assert!(xml.contains("<id>rt_1</id>"));
    assert!(xml.contains("<type>BarData</type>"));
    assert!(xml.contains("<data>Last</data>"));
    assert!(xml.contains("<refresh>5 secs</refresh>"));
    assert!(xml.contains("<step>5 secs</step>"));
}

/// A payload the decoder can read: 4 bits of padding, a 1-bit flag and an
/// 8-bit count, a 31-bit low in ticks, then a 1-bit flag and a 16-bit volume.
/// Sixty-one bits, in eight bytes, written the way the reader takes them —
/// least significant bit first, with each four-byte group reversed, which is
/// its own inverse over two whole groups.
fn single_tick_payload(low_ticks: u32, volume: u32) -> Vec<u8> {
    let mut bits: Vec<u8> = Vec::new();
    let mut put = |value: u32, width: usize| {
        for i in 0..width {
            bits.push(((value >> i) & 1) as u8);
        }
    };
    put(0, 4);
    put(1, 1);
    put(1, 8);
    put(low_ticks, 31);
    put(1, 1);
    put(volume, 16);
    bits.resize(64, 0);

    let mut stream = [0u8; 8];
    for (at, bit) in bits.iter().enumerate() {
        stream[at / 8] |= bit << (at % 8);
    }
    stream.chunks(4).flat_map(|c| c.iter().rev().copied()).collect()
}

/// A bar's volume is a count of the increment the venue said this
/// contract's sizes move in, the same as a size on the quote and
/// tick-by-tick streams. Counted as whole units, one instrument reported
/// two different volumes depending on which stream it was read from.
#[test]
fn a_bars_volume_counts_the_contracts_size_increment() {
    let payload = single_tick_payload(15_000, 100);
    let whole = decode_bar_payload(&payload, 0.01, 1.0).expect("it decodes");
    let counted = decode_bar_payload(&payload, 0.01, 0.5).expect("it decodes");
    assert_eq!(counted.volume, whole.volume * 0.5, "the count is in the venue's unit");
}


#[test]
fn decode_bar_payload_single_tick() {
    // Count of one, so the bar collapses to a single price: 15000 ticks of a
    // cent is 150.00, and the volume is stated in the narrow field.
    let bar = decode_bar_payload(&single_tick_payload(15_000, 100), 0.01, 1.0)
        .expect("a whole payload decodes");
    assert_eq!(bar.count, 1);
    assert!((bar.low - 150.00).abs() < 1e-9, "{bar:?}");
    assert!((bar.open - 150.00).abs() < 1e-9, "{bar:?}");
    assert!((bar.high - 150.00).abs() < 1e-9, "{bar:?}");
    assert!((bar.close - 150.00).abs() < 1e-9, "{bar:?}");
    assert!((bar.volume - 100.0).abs() < 1e-9, "{bar:?}");

    assert!(decode_bar_payload(&[], 0.01, 1.0).is_none());
}

/// A read past the end of the payload takes zeroes, and so does every field
/// after it. Unrecorded, a payload cut anywhere decodes into a bar of plausible
/// zeroes indistinguishable from one the venue sent.
#[test]
fn a_bar_payload_cut_short_is_not_decoded() {
    let whole = single_tick_payload(15_000, 100);
    for cut in 1..whole.len() {
        assert!(
            decode_bar_payload(&whole[..cut], 0.01, 1.0).is_none(),
            "{cut} of {} bytes decoded into a bar", whole.len(),
        );
    }
}
mod duration_spelling_tests {
    use super::super::normalize_duration;

    /// The spelling each unit is taken in, measured against a live session in
    /// both cases. Getting one wrong is refused outright — "Invalid time
    /// length" — rather than corrected, so a caller asking for seconds or weeks
    /// got nothing while the same span in days was served.
    #[test]
    fn each_unit_is_spelled_the_way_the_venue_takes_it() {
        for (asked, sent) in [
            ("3600 S", "3600 S"), ("3600 s", "3600 S"),
            ("2 D", "2 d"), ("2 d", "2 d"),
            ("2 W", "2 W"), ("2 w", "2 W"),
            ("1 M", "1 m"), ("1 m", "1 m"),
            ("1 Y", "1 y"), ("1 y", "1 y"),
        ] {
            assert_eq!(normalize_duration(asked), sent, "asked for {asked}");
        }
    }

    /// A unit this venue does not name is the venue's to refuse, not this
    /// client's to swallow.
    #[test]
    fn an_unknown_unit_reaches_the_venue_unchanged() {
        assert_eq!(normalize_duration("5 Q"), "5 Q");
        assert_eq!(normalize_duration(""), "");
    }
}
mod tick_data_type_tests {
    use super::super::tick_data_type;

    /// Each name a caller can use names a series the venue serves.
    #[test]
    fn each_known_name_maps_to_the_venues_own() {
        assert_eq!(tick_data_type(""), Ok("AllLast"));
        assert_eq!(tick_data_type("TRADES"), Ok("AllLast"));
        assert_eq!(tick_data_type("MIDPOINT"), Ok("MidPoint"));
        assert_eq!(tick_data_type("BID_ASK"), Ok("BidAsk"));
        assert_eq!(
            tick_data_type("OPTION_EXERCISE_INTEREST_RATE"),
            Ok("OptExInterestRate"),
        );
    }

    /// One it does not know is refused rather than turned into trades. Turned
    /// into trades, a caller asking for the venue's interest-rate series was
    /// answered with a list of option prints and told nothing.
    #[test]
    fn a_name_it_does_not_know_is_refused() {
        for unknown in ["MIDPONT", "BID", "OptExInterestRate ", "anything"] {
            assert!(tick_data_type(unknown).is_err(), "{unknown} was taken for trades");
        }
    }
    // ── decode_bar_payload ───────────────────────────────────────────────
    //
    // The bit layout, stated as the wire states it. These were written against
    // a second reading of this format, which is gone: two implementations of
    // one decoder is how a sign-extension defect survived in the one the
    // engine calls while the one nothing called was right.

    #[test]
    fn decode_bar_single_trade() {
        // count=1: only low is meaningful; open=high=close=low, volume encoded
        // Build LSB-first bit stream, then reverse within 4-byte groups.
        //
        // Layout (LSB first within the reordered buffer):
        //   4 bits padding (0)
        //   1 bit count_flag = 1 (short count)
        //   8 bits count = 1
        //   31 bits low_ticks = 1000 (positive)
        //   (no delta fields when count==1)
        //   1 bit vol_flag = 1 (short volume)
        //   16 bits volume = 500
        //
        // Total: 4+1+8+31+1+16 = 61 bits, 8 bytes
        let min_tick = 0.01;
        let mut bits_lsb: Vec<u8> = Vec::new();

        // helper: push n bits from val LSB-first
        let push_lsb = |bits: &mut Vec<u8>, val: u64, n: usize| {
            for i in 0..n {
                bits.push(((val >> i) & 1) as u8);
            }
        };

        push_lsb(&mut bits_lsb, 0, 4);     // padding
        push_lsb(&mut bits_lsb, 1, 1);      // count_flag=1 (8-bit)
        push_lsb(&mut bits_lsb, 1, 8);      // count=1
        push_lsb(&mut bits_lsb, 1000, 31);  // low_ticks=1000
        // count==1: no delta/wap fields
        push_lsb(&mut bits_lsb, 1, 1);      // vol_flag=1 (16-bit)
        push_lsb(&mut bits_lsb, 500, 16);   // volume=500

        // Convert bit stream to bytes (LSB first)
        let byte_count = bits_lsb.len().div_ceil(8);
        let mut reordered = vec![0u8; byte_count];
        for (i, &b) in bits_lsb.iter().enumerate() {
            if b == 1 {
                reordered[i / 8] |= 1 << (i % 8);
            }
        }

        // Reverse within 4-byte groups to produce the wire payload
        let mut payload = Vec::new();
        for chunk in reordered.chunks(4) {
            let mut c = chunk.to_vec();
            c.reverse();
            payload.extend_from_slice(&c);
        }

        let bar = super::super::decode_bar_payload(&payload, min_tick, 1.0).unwrap();
        assert_eq!(bar.count, 1);
        assert!((bar.low - 10.0).abs() < 1e-9);    // 1000 * 0.01
        assert!((bar.open - bar.low).abs() < 1e-9);
        assert!((bar.high - bar.low).abs() < 1e-9);
        assert!((bar.close - bar.low).abs() < 1e-9);
        assert_eq!(bar.volume, 500.0);
    }

    #[test]
    fn decode_bar_multi_trade_short_deltas() {
        // count > 1 with narrow (5-bit) deltas
        let min_tick = 0.01;
        let mut bits_lsb: Vec<u8> = Vec::new();

        let push_lsb = |bits: &mut Vec<u8>, val: u64, n: usize| {
            for i in 0..n {
                bits.push(((val >> i) & 1) as u8);
            }
        };

        push_lsb(&mut bits_lsb, 0, 4);     // padding
        push_lsb(&mut bits_lsb, 1, 1);      // count_flag=1 (8-bit)
        push_lsb(&mut bits_lsb, 5, 8);      // count=5
        push_lsb(&mut bits_lsb, 2000, 31);  // low_ticks=2000

        // count > 1: delta fields
        push_lsb(&mut bits_lsb, 1, 1);      // width_flag=1 → 5-bit deltas
        push_lsb(&mut bits_lsb, 3, 5);      // delta_open=3
        push_lsb(&mut bits_lsb, 7, 5);      // delta_high=7
        push_lsb(&mut bits_lsb, 2, 5);      // delta_close=2

        // wap
        push_lsb(&mut bits_lsb, 1, 1);      // wap_flag=1 → 18-bit
        push_lsb(&mut bits_lsb, 100, 18);   // wap_sum=100

        // volume
        push_lsb(&mut bits_lsb, 1, 1);      // vol_flag=1 → 16-bit
        push_lsb(&mut bits_lsb, 1000, 16);  // volume=1000

        let byte_count = bits_lsb.len().div_ceil(8);
        let mut reordered = vec![0u8; byte_count];
        for (i, &b) in bits_lsb.iter().enumerate() {
            if b == 1 {
                reordered[i / 8] |= 1 << (i % 8);
            }
        }

        let mut payload = Vec::new();
        for chunk in reordered.chunks(4) {
            let mut c = chunk.to_vec();
            c.reverse();
            payload.extend_from_slice(&c);
        }

        let bar = super::super::decode_bar_payload(&payload, min_tick, 1.0).unwrap();
        assert_eq!(bar.count, 5);
        let low = 2000.0 * min_tick; // 20.00
        assert!((bar.low - low).abs() < 1e-9);
        assert!((bar.open - (low + 3.0 * min_tick)).abs() < 1e-9);
        assert!((bar.high - (low + 7.0 * min_tick)).abs() < 1e-9);
        assert!((bar.close - (low + 2.0 * min_tick)).abs() < 1e-9);
        assert_eq!(bar.volume, 1000.0);
        // wap = low + wap_sum * min_tick / volume = 20.0 + 100*0.01/1000
        let expected_wap = low + 100.0 * min_tick / 1000.0;
        assert!((bar.wap - expected_wap).abs() < 1e-9);
    }


    /// The unit prices are counted in is the venue's to state. Chosen here, a
    /// bar in anything that does not move in pennies is decoded wrong and
    /// handed over as though it were right.
    #[test]
    fn a_ticker_with_no_stated_unit_reads_no_bars() {
        let stated = "<ticker id=\"7\"><minTick>0.005</minTick></ticker>";
        assert_eq!(super::super::min_tick_of(stated, "7"), Some(0.005));

        let silent = "<ticker id=\"7\"></ticker>";
        assert_eq!(
            super::super::min_tick_of(silent, "7"), None,
            "no unit stated is no unit, not a penny",
        );
    }
}

/// One name per series, whichever request asks for it.
///
/// The midpoint was spelled three times in this file and two of them were
/// wrong. The venue takes only `MidPoint`, and answered the others with "no
/// historical market data" — which reads as a series that does not exist
/// rather than as a name it does not know, so nobody looked for a typo.
#[test]
fn every_request_asks_for_a_series_by_the_same_name() {
    for name in ["TRADES", "MIDPOINT", "BID", "ASK"] {
        let through_the_type = BarDataType::from_api_str(name)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .as_str();
        let xml = build_realtime_bar_xml("q", 12087792, name, false, "CASH", "IDEALPRO");
        assert!(
            xml.contains(&format!("<data>{through_the_type}</data>")),
            "{name} goes out as something other than {through_the_type}: {xml}",
        );
    }
}

/// A settled contract is asked about as settled, on both query shapes.
///
/// Written as a flat `no`, a request for an expired future asked about a
/// contract that no longer exists and came back empty, whatever the caller's
/// own contract said.
#[test]
fn an_expired_contract_is_asked_about_as_expired() {
    let stated = |include_expired: bool| HistoricalRequest {
        query_id: "h_1".into(),
        con_id: 495512563,
        symbol: "ES".into(),
        sec_type: "FUT".into(),
        exchange: "CME".into(),
        data_type: BarDataType::Trades,
        end_time: "20260101-16:00:00".into(),
        duration: "1 D".into(),
        bar_size: BarSize::Hour1,
        use_rth: true,
        keep_up_to_date: false,
        include_expired,
    };
    assert!(build_query_xml(&stated(true)).contains("<expired>yes</expired>"));
    assert!(build_query_xml(&stated(false)).contains("<expired>no</expired>"));

    let ticks = |include_expired: bool| build_tick_query_xml(
        "tk_1", 495512563, "", "20260101-16:00:00", 100, "TRADES", true, "FUT", "CME", include_expired,
    );
    assert!(ticks(true).contains("<expired>yes</expired>"));
    assert!(ticks(false).contains("<expired>no</expired>"));
}
