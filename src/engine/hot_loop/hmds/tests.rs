//! The tests for this module.
//!
//! One file per module, as `api/client` already does it. Each block below
//! reaches the code it tests through `super::super`, which is the module this
//! file belongs to.

mod historical_contract_tests {
    use super::super::{hist_exchange, hist_sec_type};

    /// The substitution the engine applies to whatever the client sent.
    /// Exercised here rather than on the query builder alone, which honours
    /// these fields either way and so cannot show where they are applied.
    #[test]
    fn a_stated_security_type_reaches_the_wire_in_its_own_spelling() {
        assert_eq!(hist_sec_type("FUT"), "FUT");
        assert_eq!(hist_sec_type("OPT"), "OPT");
        assert_eq!(hist_sec_type("CASH"), "CASH");
        // Both vocabularies for a stock land on the wire spelling.
        assert_eq!(hist_sec_type("STK"), "CS");
        assert_eq!(hist_sec_type("CS"), "CS");
        // Absent keeps exactly what every caller got before.
        assert_eq!(hist_sec_type(""), "CS");
        // A valid type the enum does not carry is sent as stated, not
        // narrowed away — the subscribe path does the same.
        assert_eq!(hist_sec_type("FOP"), "FOP");
        assert_eq!(hist_sec_type("CFD"), "CFD");
        // A value shaped like a type but unknown to both the enum and the
        // gateway still reaches it, to be named there rather than silently
        // described as a stock.
        assert_eq!(hist_sec_type("NOPE"), "NOPE");
        // Anything that could break the query document is blanked instead of
        // embedded. The value lands in XML, so this is the difference between
        // a refused query and a malformed one.
        assert_eq!(hist_sec_type("FOP&"), "");
        assert_eq!(hist_sec_type("<x>"), "");
        assert_eq!(hist_sec_type("A B"), "");
        assert_eq!(hist_sec_type("VERYLONGTYPE"), "");
    }

    #[test]
    fn a_stated_venue_reaches_the_wire_and_an_absent_one_defaults() {
        assert_eq!(hist_exchange("CME"), "CME");
        assert_eq!(hist_exchange("IDEALPRO"), "IDEALPRO");
        assert_eq!(hist_exchange(""), "SMART");
    }
}
/// A reconnect resubscribes the tick-by-tick streams. Replacing the socket
/// alone leaves every stream behind on the dead one while the transport
/// reports healthy, so nothing anywhere states that the data has stopped.
#[test]
fn a_reconnect_puts_the_tick_by_tick_streams_back() {
    let mut hmds = HmdsState::new();
    let mut market = crate::engine::market_state::MarketState::new();
    let instrument = market.try_register(756733).expect("slot");
    hmds.tbt_subscriptions.push(TbtSubscription { instrument, query_id: "tbt_0".to_string(), kind: TbtType::Last, caller_req_id: 0, venue_id: 0, min_tick: 0, size_tick: 0.0, running: Default::default() });
    // One with no contract behind it: it must be reported, not resubscribed
    // against a contract id the engine does not have.
    hmds.tbt_subscriptions.push(TbtSubscription { instrument: 7, query_id: "tbt_1".to_string(), kind: TbtType::BidAsk, caller_req_id: 0, venue_id: 0, min_tick: 0, size_tick: 0.0, running: Default::default() });

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let sock = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let mut conn = None;
    let mut hb = HeartbeatState::new();
    hmds.disconnected = true;
    hmds.reconnect(
        crate::protocol::connection::Connection::new_raw(sock).unwrap(),
        &mut conn, &market, &mut hb,
    );

    assert!(!hmds.disconnected, "the transport is live again");
    assert_eq!(hmds.tbt_subscriptions.len(), 1, "the resolvable stream is back");
    assert_eq!(
        hmds.tbt_subscriptions[0].running, Default::default(),
        "the dead session's prices go with it, or its last pair takes the next move",
    );
    assert_eq!(hmds.tbt_subscriptions[0].instrument, instrument);
    assert_ne!(hmds.tbt_subscriptions[0].query_id, "tbt_0", "under a new id, not the dead session's");
}

/// The routing for a five-second bar stream is a ticker id the session
/// issued. A reconnect that kept the routing and re-sent nothing left the
/// bars stopped with the connection reporting healthy.
///
/// A keep-up-to-date request needs only this stream restored: its bars are
/// folded from it, and the partial bar survives the reconnect. A second
/// request for the same stream leaves two subscriptions upstream for one
/// caller.
#[test]
fn a_reconnect_asks_for_the_five_second_bars_again() {
    let mut hmds = HmdsState::new();
    let market = crate::engine::market_state::MarketState::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let sock = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let mut conn = Some(crate::protocol::connection::Connection::new_raw(sock).unwrap());
    let mut hb = HeartbeatState::new();
    hmds.send_realtime_bar_subscribe(9, 265598, "", "STK", "SMART", "TRADES", true, &mut conn, &mut hb);
    let first = hmds.rtbar_subs[0].0.clone();
    // State a keep-up-to-date request leaves behind: the stream and the
    // partial bar.
    hmds.keep_up_to_date_reqs.insert(9);
    hmds.forming_bars.push(super::FormingBar {
        req_id: 9,
        seconds: 60,
        opened_at: 0,
        bar: Default::default(),
        weighted: 0.0,
    });

    let sock2 = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer2, _) = listener.accept().unwrap();
    hmds.disconnected = true;
    hmds.reconnect(
        crate::protocol::connection::Connection::new_raw(sock2).unwrap(),
        &mut conn, &market, &mut hb,
    );

    assert_eq!(hmds.rtbar_subs.len(), 1, "the stream is asked for again");
    assert_ne!(hmds.rtbar_subs[0].0, first, "under a new query, not the dead session's");
    assert_eq!(hmds.rtbar_subs[0].1, 9, "still answering the caller's request id");
    assert_eq!(
        hmds.forming_bars.iter().filter(|f| f.req_id == 9).count(), 1,
        "and the bar it was folding is still the one being folded",
    );
    assert!(
        hmds.pending_historical.iter().all(|(_, rid, _)| *rid != 9),
        "nothing else is asked for on its behalf: one request, one stream",
    );
}

use super::*;

/// The diagnostic log byte-sliced a lossily decoded value, so a tag whose
/// two-hundredth byte fell inside a multi-byte character aborted the hot
/// loop — and only when debug logging was on, which is to say only while
/// someone was diagnosing an incident.
#[test]
fn a_non_utf8_xml_tag_does_not_abort_the_hot_loop() {
    // Driven through the handler that performs the slice, so the assertion
    // depends on the code under test.
    //
    // 199 ASCII bytes then one invalid byte. Lossily decoded, that byte becomes
    // a three-byte replacement character, so byte 200 falls inside it.
    let mut payload = b"<ResultSetBar>".to_vec();
    payload.extend(std::iter::repeat_n(b'a', 199 - payload.len()));
    payload.push(0xFF);

    let mut msg = Vec::new();
    msg.extend_from_slice(b"35=W\x016118=");
    msg.extend_from_slice(&payload);
    msg.push(0x01);

    let mut hmds = HmdsState::new();
    let shared = crate::bridge::SharedState::new();
    let mut hb = HeartbeatState::new();
    let mut conn: Option<crate::protocol::connection::Connection> = None;
    // The slice runs only with debug logging enabled.
    log::set_max_level(log::LevelFilter::Debug);
    hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);
}

fn make_query_error_msg(query_id: &str, error: &str) -> Vec<u8> {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<QueryError>\n\t<id>{query_id}</id>\n\t<error>{error}</error>\n</QueryError>\n",
    );
    let mut msg = Vec::new();
    msg.extend_from_slice(b"35=W\x016118=");
    msg.extend_from_slice(xml.as_bytes());
    msg.push(0x01);
    msg
}

fn make_bar_msg(query_id: &str, eoq: bool) -> Vec<u8> {
    let xml = format!(
        "<ResultSetBar><id>{}</id><eoq>{}</eoq><tz>UTC</tz><Events>\
         <Bar><time>20260714-13:30:00</time><open>100.0</open><close>100.5</close>\
         <high>100.7</high><low>99.9</low><weightedAvg>100.2</weightedAvg>\
         <volume>1000</volume><count>10</count></Bar></Events></ResultSetBar>",
        query_id, if eoq { "true" } else { "false" },
    );
    let mut msg = Vec::new();
    msg.extend_from_slice(b"35=W\x016118=");
    msg.extend_from_slice(xml.as_bytes());
    msg.push(0x01);
    msg
}

#[test]
fn segmented_bar_reply_completes_on_eoq_true() {
    // /: a segmented bar reply carries <eoq>false> on
    // early frames and <eoq>true> on the final one. The pending entry must
    // persist through the false frames and be released on the true frame.
    let mut hmds = HmdsState::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    let mut conn: Option<Connection> = None;
    hmds.pending_historical.push(("q7".to_string(), 21, Instant::now() + HISTORICAL_IDLE_TIMEOUT));

    hmds.process_hmds_message(&make_bar_msg("q7", false), &mut conn, &shared, &None, &mut hb);
    assert_eq!(hmds.pending_historical.len(), 1, "entry must persist through eoq=false");

    hmds.process_hmds_message(&make_bar_msg("q7", true), &mut conn, &shared, &None, &mut hb);
    assert!(hmds.pending_historical.is_empty(), "eoq=true must release the pending entry");

    let hist = shared.reference.drain_historical_data();
    assert_eq!(hist.len(), 2);
    assert!(!hist[0].1.is_complete, "first segment incomplete");
    assert!(hist[1].1.is_complete, "final segment complete");
}

#[test]
fn conadj_response_frame_is_skipped_without_disturbing_pending() {
    // /: the 6040=10022 ConAdjResponse (corporate
    // actions) is pushed once per contract on the first historical request.
    // It must be recognized and skipped, not treated as bar or completion.
    let mut hmds = HmdsState::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    let mut conn: Option<Connection> = None;
    hmds.pending_historical.push(("q8".to_string(), 22, Instant::now() + HISTORICAL_IDLE_TIMEOUT));

    let mut msg = Vec::new();
    msg.extend_from_slice(b"35=U\x016040=10022\x016118=");
    msg.extend_from_slice(b"<ConAdjResponse><id>ContractAdjustment1</id></ConAdjResponse>");
    msg.push(0x01);
    hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);

    assert_eq!(hmds.pending_historical.len(), 1, "ConAdjResponse must not touch pending historical");
    assert!(shared.reference.drain_historical_data().is_empty());
    assert!(shared.reference.drain_historical_errors().is_empty());
}

#[test]
fn query_error_releases_historical_and_emits_error_and_end_sentinel() {
    let mut hmds = HmdsState::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    let mut conn: Option<Connection> = None;
    hmds.pending_historical.push(("hist_1003".to_string(), 11, Instant::now() + HISTORICAL_IDLE_TIMEOUT));
    hmds.keep_up_to_date_reqs.insert(11);

    let msg = make_query_error_msg("hist_1003", "Invalid time length");
    hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);

    assert!(hmds.pending_historical.is_empty(), "pending entry should be drained");
    assert!(!hmds.keep_up_to_date_reqs.contains(&11), "kut flag should be cleared");

    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 11);
    assert_eq!(errors[0].1, 162);
    assert_eq!(errors[0].2, "Invalid time length");

    let hist = shared.reference.drain_historical_data();
    assert_eq!(hist.len(), 1, "terminal sentinel must be queued for historical req");
    assert_eq!(hist[0].0, 11);
    assert!(hist[0].1.is_complete);
    assert!(hist[0].1.bars.is_empty());
}

#[test]
fn query_error_releases_head_timestamp_without_sentinel() {
    let mut hmds = HmdsState::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    let mut conn: Option<Connection> = None;
    hmds.pending_head_ts.push(("hts_1004".to_string(), 42));

    let msg = make_query_error_msg("hts_1004", "No head timestamp");
    hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);

    assert!(hmds.pending_head_ts.is_empty());
    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors, vec![(42, 162, "No head timestamp".to_string())]);
    // Head-ts is not a bar request — no historical_data sentinel should fire.
    assert!(shared.reference.drain_historical_data().is_empty());
}

// ── unknown bar_size rejects at the engine too (backstop for
// raw control-channel callers; the client validates synchronously) ──

#[test]
fn engine_rejects_unknown_bar_size_with_error_and_sentinel() {
    let mut hmds = HmdsState::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    let mut conn: Option<Connection> = None;

    hmds.send_historical_request_ex(9, 756733, "", "2 d", "1 Min", "TRADES",
        true, false, false, "SPY", "STK", "SMART", &mut conn, &mut hb, &shared);

    assert!(hmds.pending_historical.is_empty(), "rejected request must not go pending");
    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].1, 162);
    assert!(errors[0].2.contains("bar_size"), "got: {}", errors[0].2);
    let hist = shared.reference.drain_historical_data();
    assert_eq!(hist.len(), 1, "terminal sentinel must unblock waiters");
    assert!(hist[0].1.is_complete);
}

// ── idle-deadline sweep ──

#[test]
fn sweep_times_out_idle_historical_with_error_and_end_sentinel() {
    let mut hmds = HmdsState::new();
    let shared = SharedState::new();
    // Deadline already in the past — the gateway went silent.
    hmds.pending_historical.push(("hist_1010".to_string(), 21, Instant::now() - std::time::Duration::from_secs(1)));

    hmds.sweep_pending_historical(&shared);

    assert!(hmds.pending_historical.is_empty(), "expired entry must be reclaimed");
    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 21);
    assert_eq!(errors[0].1, 162);
    let hist = shared.reference.drain_historical_data();
    assert_eq!(hist.len(), 1, "terminal sentinel must unblock historical_data_end waiters");
    assert_eq!(hist[0].0, 21);
    assert!(hist[0].1.is_complete);
    assert!(hist[0].1.bars.is_empty());
}

#[test]
fn sweep_spares_keep_up_to_date_and_live_entries() {
    let mut hmds = HmdsState::new();
    let shared = SharedState::new();
    // keepUpToDate entry: resident by design, even past its deadline.
    hmds.pending_historical.push(("hist_kut".to_string(), 30, Instant::now() - std::time::Duration::from_secs(1)));
    hmds.keep_up_to_date_reqs.insert(30);
    // Live entry: deadline in the future.
    hmds.pending_historical.push(("hist_live".to_string(), 31, Instant::now() + HISTORICAL_IDLE_TIMEOUT));

    hmds.sweep_pending_historical(&shared);

    assert_eq!(hmds.pending_historical.len(), 2, "neither entry may be swept");
    assert!(shared.reference.drain_historical_errors().is_empty());
    assert!(shared.reference.drain_historical_data().is_empty());
}

#[test]
fn query_error_for_unknown_query_id_drops_nothing_and_emits_no_error() {
    let mut hmds = HmdsState::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    let mut conn: Option<Connection> = None;
    hmds.pending_historical.push(("hist_1003".to_string(), 11, Instant::now() + HISTORICAL_IDLE_TIMEOUT));

    let msg = make_query_error_msg("hist_9999", "Boom");
    hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);

    assert_eq!(hmds.pending_historical.len(), 1, "unrelated entry must stay");
    assert!(shared.reference.drain_historical_errors().is_empty());
    assert!(shared.reference.drain_historical_data().is_empty());
}
/// The helpers above are only worth anything if the request that reaches
/// the wire uses them. Reinstating the old `CS`/`SMART` constants in the
/// builder passes every test that checks the helpers or the query encoder
/// in isolation, so this drives the real function and reads the socket.
#[test]
fn the_query_on_the_wire_carries_the_contract_s_own_type_and_venue() {
    use crate::protocol::connection::Connection;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let client = std::net::TcpStream::connect(addr).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut conn = Some(Connection::new_raw(client).unwrap());

    let mut hmds = super::HmdsState::new();
    let mut hb = crate::engine::hot_loop::HeartbeatState::new();
    let shared = crate::bridge::SharedState::new();

    hmds.send_historical_request_ex(
        1, 495512563, "20260101 16:00:00", "1 D", "1 hour", "TRADES",
            true, false, false, "ES", "FUT", "CME", &mut conn, &mut hb, &shared,
    );

    let sent = String::from_utf8_lossy(&read_frame(&mut peer)).to_string();

    assert!(sent.contains("FUT"), "the contract's security type: {sent}");
    assert!(sent.contains("CME"), "the contract's venue: {sent}");
    assert!(!sent.contains("SMART"), "and not the old constant: {sent}");
}

/// Read until the query XML has closed. A single `read` can legally return
/// a partial frame, which would make these tests fail intermittently while
/// production is correct.
fn read_frame(peer: &mut std::net::TcpStream) -> Vec<u8> {
    use std::io::Read;
    let mut acc = Vec::new();
    let mut buf = vec![0u8; 8192];
    for _ in 0..64 {
        match peer.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                acc.extend_from_slice(&buf[..n]);
                // Stop on a complete frame, not on a field in the middle
                // of one: a split right after the query would otherwise
                // return a truncated read that happens to satisfy the
                // assertions above it.
                // A plain FIX frame ends with its checksum field; a
                // compressed one states its own length.
                let ends_with_trailer = acc.len() >= 7
                    && acc[acc.len() - 1] == 0x01
                    && acc[acc.len() - 7..acc.len() - 4] == *b"\x0110=";
                let complete = ends_with_trailer
                    || crate::protocol::fixcomp::fixcomp_length(&acc)
                        .is_some_and(|len| acc.len() >= len);
                if complete {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    acc
}
mod tick_ack_tests {
    use super::super::{parse_tick_subscription_ack, scaled_size};

    /// The venue's answer, taken from a live session. It names the
    /// subscription back, states the number it will use on every frame, and
    /// states the increments prices and sizes move in — none of which is
    /// stated anywhere else on this connection.
    #[test]
    fn the_acknowledgement_states_the_number_and_both_increments() {
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\t<ResultSetTickerId>\n\
                   \t\t<id>tbt_1</id>\n\t\t<rtTickerId>1</rtTickerId>\n\
                   \t\t<minTick>0.00005</minTick>\n\t\t<sizeMinTick>1</sizeMinTick>\n\
                   \t\t<eoq>false</eoq>\n\t</ResultSetTickerId>";
        let ack = parse_tick_subscription_ack(xml).expect("the acknowledgement reads");
        assert_eq!(ack.query_id, "tbt_1");
        assert_eq!(ack.venue_id, 1);
        assert_eq!(ack.min_tick, 0.00005);
        assert_eq!(ack.size_min_tick, 1.0);
    }

    /// A crypto deals in hundred-millionths, and says so.
    #[test]
    fn a_contract_dealt_in_fractions_states_a_fractional_size_increment() {
        let xml = "<ResultSetTickerId><id>tbt_2</id><rtTickerId>2</rtTickerId>\
                   <minTick>0.25</minTick><sizeMinTick>0.00000001</sizeMinTick></ResultSetTickerId>";
        let ack = parse_tick_subscription_ack(xml).expect("the acknowledgement reads");
        assert_eq!(ack.venue_id, 2, "and its number is its own, not the one asked under");
        assert_eq!(ack.size_min_tick, 0.00000001);
    }

    /// Some other reply on the same connection is not an acknowledgement.
    #[test]
    fn another_reply_is_not_read_as_an_acknowledgement() {
        assert!(parse_tick_subscription_ack("<QueryError><id>tbt_1</id></QueryError>").is_none());
        assert!(parse_tick_subscription_ack("<ResultSetBar><id>h_1</id></ResultSetBar>").is_none());
    }

    /// A size is a count of what the venue said sizes move in. Counting a
    /// crypto's size in whole ones reports it a hundred million times too large.
    #[test]
    fn a_size_is_counted_in_what_the_venue_said_it_moves_in() {
        // A share: whole ones, held in the form readers divide by.
        assert_eq!(scaled_size(100, 1.0), 100 * crate::types::QTY_SCALE);
        // A crypto: a hundred million counts is one whole unit.
        assert_eq!(scaled_size(100_000_000, 0.00000001), crate::types::QTY_SCALE);
        // Stating no increment means whole ones.
        assert_eq!(scaled_size(5, 0.0), 5 * crate::types::QTY_SCALE);
    }
}
mod counted_size_ceiling_tests {
    use super::super::scaled_size;

    /// A count past what a quantity can hold is held at the largest one. Cast
    /// straight through it comes out negative, and a size that reads as
    /// negative is a sell where there was a buy.
    #[test]
    fn a_count_past_the_ceiling_does_not_come_back_negative() {
        assert!(scaled_size(u64::MAX, 1.0) > 0, "a size came back negative");
        assert!(scaled_size(u64::MAX, 1e-8) > 0);
    }

    /// An ordinary count is untouched.
    #[test]
    fn an_ordinary_count_is_untouched() {
        assert_eq!(scaled_size(100, 1.0), 100 * crate::types::QTY_SCALE);
    }
}
mod withdrawing_one_stream_tests {
    use super::super::*;

    fn stream(caller_req_id: i64, instrument: InstrumentId, kind: TbtType) -> TbtSubscription {
        TbtSubscription {
            instrument,
            query_id: format!("tbt_{caller_req_id}"),
            kind,
            caller_req_id,
            venue_id: caller_req_id as u64,
            min_tick: 1,
            size_tick: 1.0,
            running: Default::default(),
        }
    }

    /// A contract can carry two streams — every trade, and every quote change.
    /// Withdrawing one by naming the contract took whichever was opened first
    /// and left the caller's own running.
    #[test]
    fn withdrawing_one_stream_leaves_the_other() {
        let mut hmds = HmdsState::new();
        hmds.tbt_subscriptions.push(stream(1, 7, TbtType::Last));
        hmds.tbt_subscriptions.push(stream(2, 7, TbtType::BidAsk));

        hmds.send_tbt_unsubscribe(2, 7, &mut None, &mut HeartbeatState::new());

        assert_eq!(hmds.tbt_subscriptions.len(), 1, "one stream was withdrawn");
        assert_eq!(
            hmds.tbt_subscriptions[0].caller_req_id, 1,
            "the wrong stream was withdrawn",
        );
    }

    /// Every trade includes the prints that never reached the tape; the
    /// exchange's own trades are the same stream without them. Sent as two
    /// requests, the venue acknowledged the second and answered it with
    /// silence — on a future, which has no off-exchange tape at all.
    #[test]
    fn the_narrower_stream_is_the_wider_one_without_the_unreported() {
        assert!(belongs_on(TbtType::AllLast, true), "every trade means every trade");
        assert!(belongs_on(TbtType::AllLast, false));
        assert!(!belongs_on(TbtType::Last, true), "that one never reached the tape");
        assert!(belongs_on(TbtType::Last, false));
    }

    /// A caller that opened one stream and names it by something else still
    /// gets that one withdrawn, rather than nothing happening.
    #[test]
    fn one_stream_named_loosely_is_still_withdrawn() {
        let mut hmds = HmdsState::new();
        hmds.tbt_subscriptions.push(stream(1, 7, TbtType::Last));
        hmds.send_tbt_unsubscribe(99, 7, &mut None, &mut HeartbeatState::new());
        assert!(hmds.tbt_subscriptions.is_empty());
    }
}
mod counted_size_range_tests {
    use super::super::scaled_size;

    /// A count too large to hold as sent is still held when the increment
    /// shrinks it back into range. Cut down to fit before scaling, it came
    /// back as a number that was plausible and was not the venue's.
    #[test]
    fn an_increment_that_shrinks_a_count_keeps_it() {
        let counted = u64::MAX / 4;
        let held = scaled_size(counted, 1e-8);
        let expected = (counted as f64 * 1e-8 * crate::types::QTY_SCALE as f64).round() as i64;
        assert_eq!(held, expected, "the increment was applied to a count that had been cut");
    }
}
mod forming_bar_tests {
    use super::super::*;

    fn five(timestamp: u32, open: f64, high: f64, low: f64, close: f64, volume: f64) -> crate::types::RealTimeBar {
        crate::types::RealTimeBar {
            timestamp, open, high, low, close, volume, wap: close, count: 1,
        }
    }

    /// A caller keeping five-minute bars up to date hears its own bar as it
    /// forms, not the five-second bars it is made of.
    #[test]
    fn the_forming_bar_is_folded_from_what_the_venue_streams() {
        let mut forming = FormingBar {
            req_id: 1, seconds: 300, opened_at: 0,
            bar: Default::default(), weighted: 0.0,
        };
        // 14:35:00, then two more within the same five minutes.
        let first = forming.fold(&five(1_786_456_500, 10.0, 10.5, 9.5, 10.2, 100.0));
        assert_eq!(first.timestamp, 1_786_456_500, "the bar opens on its own boundary");

        forming.fold(&five(1_786_456_505, 10.2, 11.0, 10.1, 10.8, 50.0));
        let so_far = forming.fold(&five(1_786_456_510, 10.8, 10.9, 9.0, 9.4, 50.0));
        assert_eq!(so_far.timestamp, 1_786_456_500, "still the same bar");
        assert_eq!(so_far.open, 10.0, "opened where the first one did");
        assert_eq!(so_far.high, 11.0, "the highest of them");
        assert_eq!(so_far.low, 9.0, "the lowest of them");
        assert_eq!(so_far.close, 9.4, "the latest of them");
        assert_eq!(so_far.volume, 200.0, "all of it");
        assert_eq!(so_far.count, 3);
        assert!((so_far.wap - (10.2 * 100.0 + 10.8 * 50.0 + 9.4 * 50.0) / 200.0).abs() < 1e-9);

        // And the next five minutes start a bar of their own.
        let next = forming.fold(&five(1_786_456_800, 9.4, 9.6, 9.3, 9.5, 10.0));
        assert_eq!(next.timestamp, 1_786_456_800);
        assert_eq!(next.volume, 10.0, "nothing carried over");
    }
}

/// A scan response is delivered to the scan that answered.
///
/// Every response arrives under one message id, so the id cannot identify the
/// scan. The payload names the scan, and that is what routes the rows.
#[test]
fn each_scan_gets_its_own_answer() {
    let mut hmds = super::HmdsState::new();
    hmds.pending_scanner.push(("APISCAN1:10001".to_string(), 10001));
    hmds.pending_scanner.push(("APISCAN2:10002".to_string(), 10002));
    hmds.pending_scanner.push(("APISCAN3:10003".to_string(), 10003));

    for (named, expected) in [
        ("APISCAN1:10001", 10001),
        ("APISCAN2:10002", 10002),
        ("APISCAN3:10003", 10003),
    ] {
        let xml = format!("<ScanResponse>\n\t<id>{named}</id>\n</ScanResponse>");
        assert_eq!(
            hmds.scanner_answered(&xml),
            Some(expected),
            "{named} answered, so its rows belong to the request that asked for it",
        );
    }
}

/// A response naming a scan this session is not running belongs to nobody here.
///
/// A withdrawn scan can answer once more. The scan name identifies the owner,
/// so a name matching no running scan has no owner and is not delivered.
#[test]
fn an_answer_naming_no_running_scan_is_not_handed_to_another() {
    let mut hmds = super::HmdsState::new();
    hmds.pending_scanner.push(("APISCAN1:10001".to_string(), 10001));
    assert_eq!(hmds.scanner_answered("<ScanResponse></ScanResponse>"), None);
    assert_eq!(hmds.scanner_answered("<ScanResponse><id>APISCAN9:99</id></ScanResponse>"), None);
    // The one it does name still answers.
    assert_eq!(
        hmds.scanner_answered("<ScanResponse><id>APISCAN1:10001</id></ScanResponse>"),
        Some(10001),
    );
}

mod hmds_correlation_tests {
    use super::super::*;
    use crate::bridge::SharedState;
    use crate::protocol::connection::Connection;

    /// Query names are numbered, so one is a prefix of another as soon as the
    /// count reaches ten. Searching the payload for the name handed the answer
    /// for `tk_12` to whichever of `tk_1` and `tk_12` was waiting first, and
    /// the other was never answered.
    ///
    /// A news reply states what the query asked for after its name, separated
    /// from it, and is still that query's answer.
    #[test]
    fn a_query_name_that_prefixes_another_is_not_answered_by_it() {
        let answer_for = |id: &str| {
            format!("<ResultSetTick><id>{id}</id><eoq>true</eoq></ResultSetTick>")
        };

        assert!(answers(&answer_for("tk_1"), "tk_1"), "its own answer");
        assert!(
            !answers(&answer_for("tk_12"), "tk_1"),
            "tk_1 took the answer meant for tk_12",
        );
        assert!(answers(&answer_for("tk_12"), "tk_12"), "which tk_12 needs itself");
        assert!(!answers(&answer_for("tk_2"), "tk_1"), "a different query entirely");

        // The decorated form a news reply states.
        let news = "<NewsResponse><id>news_2-headlines;;NewsQuery;;0;;true;;0;;U</id></NewsResponse>";
        assert!(answers(news, "news_2"), "the reply names the query it answers");
        assert!(!answers(news, "news_2x"), "and not one whose name merely resembles it");
    }

    fn tick_msg(query_id: &str, done: bool) -> Vec<u8> {
        let xml = format!(
            "<ResultSetTick><id>{}</id><eoq>{}</eoq><tz>UTC</tz><Events>\
             <Tick><time>20260714-13:30:00</time><price>100.0</price><size>1</size></Tick>\
             </Events></ResultSetTick>",
            query_id, if done { "true" } else { "false" },
        );
        let mut msg = Vec::new();
        msg.extend_from_slice(b"35=W\x016118=");
        msg.extend_from_slice(xml.as_bytes());
        msg.push(0x01);
        msg
    }

    /// A tick query is answered in segments, each stating whether it is the
    /// last. The route is held until a segment states it is.
    #[test]
    fn a_segmented_tick_reply_keeps_its_route_until_the_venue_is_done() {
        let mut hmds = HmdsState::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let mut conn: Option<Connection> = None;
        hmds.pending_ticks.push(("tk_1".to_string(), 31, "TRADES".to_string()));

        hmds.process_hmds_message(&tick_msg("tk_1", false), &mut conn, &shared, &None, &mut hb);
        assert_eq!(hmds.pending_ticks.len(), 1, "more is coming, so the route stays");

        hmds.process_hmds_message(&tick_msg("tk_1", true), &mut conn, &shared, &None, &mut hb);
        assert!(hmds.pending_ticks.is_empty(), "the last segment releases it");
        assert_eq!(
            shared.reference.drain_historical_ticks().len(), 2,
            "both segments reached the caller",
        );
    }

    /// Tag 96 carries gzip bytes. The parsed field map is UTF-8 lossy, which
    /// replaces every invalid byte, so the payload is read from the raw
    /// frame.
    #[test]
    fn a_compressed_fundamental_report_survives_being_read() {
        use flate2::write::GzEncoder;
        use std::io::Write;

        let report = "<ReportSnapshot><Issuer>ACME</Issuer></ReportSnapshot>";
        let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(report.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(
            String::from_utf8(compressed.clone()).is_err(),
            "the payload really is not text",
        );

        let mut hmds = HmdsState::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let mut conn: Option<Connection> = None;
        hmds.pending_fundamental.push(("fund_1".to_string(), 51));

        // Tag 95 states the length, which frames a payload containing SOH
        // bytes.
        let mut msg = Vec::new();
        msg.extend_from_slice(b"35=U\x016040=10012\x016118=<FundamentalsResponse/>\x0195=");
        msg.extend_from_slice(compressed.len().to_string().as_bytes());
        msg.extend_from_slice(b"\x0196=");
        msg.extend_from_slice(&compressed);
        msg.push(0x01);
        hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);

        let answered = shared.reference.drain_fundamental_data();
        assert_eq!(answered.len(), 1, "the report reached the caller");
        assert_eq!(answered[0].1, report, "and it is the report the venue sent");
    }

    /// An article response consumes the pending request whether or not its
    /// payload reads, so an unreadable one is reported to the caller.
    #[test]
    fn an_unreadable_article_is_reported_rather_than_swallowed() {
        let mut hmds = HmdsState::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let mut conn: Option<Connection> = None;
        hmds.pending_articles.push(("art_1".to_string(), 61));

        let xml = "<NewsResponse><id>art_1-article_file;;NewsQuery;;0;;true;;0;;U</id></NewsResponse>";
        let mut msg = Vec::new();
        msg.extend_from_slice(b"35=U\x016040=10032\x016118=");
        msg.extend_from_slice(xml.as_bytes());
        msg.push(0x01);
        hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);

        assert!(hmds.pending_articles.is_empty(), "the request is spent either way");
        assert!(
            shared.reference.drain_news_articles().is_empty(),
            "there was no article to deliver",
        );
        assert!(
            shared.reference.drain_historical_errors_for_dispatch().iter().any(|(id, _, _)| *id == 61),
            "and the caller is told, rather than left waiting",
        );
    }

    /// A news response names the query it answers, and is matched on that
    /// name. Two searches can be in flight at once.
    #[test]
    fn a_news_reply_answers_the_request_it_names() {
        let mut hmds = HmdsState::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let mut conn: Option<Connection> = None;
        hmds.pending_news.push(("news_1".to_string(), 51));
        hmds.pending_news.push(("news_2".to_string(), 52));

        // The second request's response arrives first, under the id its own
        // query went out with.
        let xml = "<NewsResponse><id>news_2-headlines;;NewsQuery;;0;;true;;0;;U</id></NewsResponse>";
        let mut msg = Vec::new();
        msg.extend_from_slice(b"35=U\x016040=10032\x016118=");
        msg.extend_from_slice(xml.as_bytes());
        msg.push(0x01);
        hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);

        let answered = shared.reference.drain_historical_news();
        assert_eq!(answered.len(), 1, "one answer reached a caller");
        assert_eq!(answered[0].0, 52, "and it is the caller the reply names");
        assert_eq!(
            hmds.pending_news.iter().map(|(q, _)| q.as_str()).collect::<Vec<_>>(),
            vec!["news_1"],
            "the other request is still outstanding",
        );
    }

    /// A head timestamp goes out under an id its response names, and is
    /// matched on it. Two can be in flight at once.
    #[test]
    fn a_head_timestamp_answers_the_request_the_reply_names() {
        let mut hmds = HmdsState::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let mut conn: Option<Connection> = None;
        let id_of = |con_id: u32| {
            crate::control::historical::head_timestamp_query_id(
                &crate::control::historical::HeadTimestampRequest {
                    con_id,
                    sec_type: "CS".into(),
                    exchange: "SMART".into(),
                    data_type: crate::control::historical::BarDataType::Trades,
                    use_rth: true,
                },
            )
        };
        hmds.pending_head_ts.push((id_of(1), 41));
        hmds.pending_head_ts.push((id_of(2), 42));

        let xml = format!(
            "<ResultSetHeadTimeStamp><id>{}</id><eoq>true</eoq>\
             <headTS>19930129-09:00:00</headTS><tz>US/Eastern</tz></ResultSetHeadTimeStamp>",
            id_of(2),
        );
        let mut msg = Vec::new();
        msg.extend_from_slice(b"35=W\x016118=");
        msg.extend_from_slice(xml.as_bytes());
        msg.push(0x01);
        hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);

        let answered = shared.reference.drain_head_timestamps();
        assert_eq!(answered.len(), 1);
        assert_eq!(answered[0].0, 42, "the reply named the second request");
    }

    /// A response naming no pending query is not delivered to the oldest
    /// outstanding request.
    #[test]
    fn an_answer_naming_no_pending_query_is_not_handed_to_another() {
        let mut hmds = HmdsState::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let mut conn: Option<Connection> = None;
        hmds.pending_head_ts.push(("hts_of_another_query".to_string(), 71));
        hmds.pending_histogram.push(("hg_of_another_query".to_string(), 72));

        for xml in [
            "<ResultSetHeadTimeStamp><id>nobody</id><eoq>true</eoq>\
             <headTS>19930129-09:00:00</headTS><tz>US/Eastern</tz></ResultSetHeadTimeStamp>",
            "<ResultSetHistogram><id>nobody</id><eoq>true</eoq>\
             <Events><Tick><price>100.0</price><size>5</size></Tick></Events></ResultSetHistogram>",
        ] {
            let mut msg = Vec::new();
            msg.extend_from_slice(b"35=W\x016118=");
            msg.extend_from_slice(xml.as_bytes());
            msg.push(0x01);
            hmds.process_hmds_message(&msg, &mut conn, &shared, &None, &mut hb);
        }

        assert!(shared.reference.drain_head_timestamps().is_empty());
        assert!(shared.reference.drain_histogram_data().is_empty());
        assert_eq!(hmds.pending_head_ts.len(), 1, "and both are still waiting");
        assert_eq!(hmds.pending_histogram.len(), 1);
    }
}

mod hmds_transport_tests {
    use super::super::*;
    use crate::bridge::SharedState;
    use crate::protocol::connection::Connection;

    /// One-shot requests are failed when the connection is lost. Only
    /// historical bars carry a timeout, so the rest would never complete.
    #[test]
    fn a_lost_connection_answers_the_requests_it_took_with_it() {
        let mut hmds = HmdsState::new();
        let shared = SharedState::new();
        let mut conn: Option<Connection> = None;
        hmds.pending_head_ts.push(("hts".to_string(), 61));
        hmds.pending_fundamental.push(("fund".to_string(), 62));
        hmds.pending_ticks.push(("tk".to_string(), 63, "TRADES".to_string()));

        hmds.disconnect(&mut conn, &shared);

        let errors = shared.reference.drain_historical_errors();
        for req_id in [61, 62, 63] {
            assert!(
                errors.iter().any(|(rid, ..)| *rid == req_id),
                "request {req_id} was told the connection went: {errors:?}",
            );
        }
        assert!(hmds.pending_head_ts.is_empty(), "and nothing is left waiting");
    }
}
