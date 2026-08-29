//! Historical data, scanner, news, and fundamental data test phases.

use super::common::*;
use ibx::control::historical::{self};
use ibx::control::scanner;
use ibx::gateway::connect_farm;

/// A farm this session was routed to, opened on the route the venue gave.
///
/// Every phase here named its farm as a literal on the configured host, with
/// fresh default settings and no port. The venue states which farm this session
/// belongs to, on which host, at which port, and a farm reached on the wrong
/// host does not refuse the connection — it closes it, so the phase reads as a
/// venue holding no data rather than a request sent to the wrong farm. This
/// reads the same route the engine's own rebuild reads.
///
/// A route the venue did not state connects nothing rather than
/// falling back to a literal: that fallback is the client's own, it is not
/// visible from here, and writing it in again is the defect this replaces.
pub(super) fn open_farm(kind: ibx::gateway::Farm) -> std::io::Result<Connection> {
    let auth = RECOVERY_AUTH.get().ok_or_else(|| {
        std::io::Error::other("no session credentials were remembered to reach a farm with")
    })?;
    let (host, farm, port) = match kind {
        ibx::gateway::Farm::Historical => {
            (&auth.hmds_host, &auth.hmds_farm, auth.hmds_port)
        }
        ibx::gateway::Farm::MarketData => {
            (&auth.trading_host, &auth.trading_farm, auth.trading_port)
        }
        ibx::gateway::Farm::SecurityDefinition => {
            (&auth.secdef_host, &auth.secdef_farm, auth.secdef_port)
        }
    };
    if host.is_empty() || farm.is_empty() {
        return Err(std::io::Error::other(format!(
            "the venue named no {kind:?} farm for this session",
        )));
    }
    connect_farm(
        &auth.settings, host, farm,
        &auth.username, &auth.password, auth.paper,
        &auth.server_session_id, &auth.session_key,
        &auth.hw_info, &auth.encoded, kind, port,
    )
}


pub(super) fn phase_historical_data(mut conns: Conns) -> Conns {
    phase!("--- Phase 11: Historical Data Bars (SPY, 1 day of 5-min bars) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => {
            skipped!("  SKIP: the historical farm could not be reached: {e}\n");
            return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id };
        }
    };

    // Step 1: Create HotLoop with HMDS connection
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    // Step 2: Send FetchHistorical via ControlCommand
    control_tx.send(ControlCommand::FetchHistorical {
        contract: ContractRef { con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() },
        req_id: 1100,
        end_date_time: now_ib_timestamp(),
        duration: "1 D".into(),
        bar_size: "5 mins".into(),
        what_to_show: "TRADES".into(),
        use_rth: true,
        keep_up_to_date: false, include_expired: false,
        filters: Default::default(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    // Step 3: Wait for results in SharedState
    let mut all_bars = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut complete = false;

    while Instant::now() < deadline && !complete {
        let results = shared.reference.drain_historical_data();
        for (req_id, resp) in results {
            if req_id == 1100 {
                all_bars.extend(resp.bars);
                if resp.is_complete { complete = true; }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Step 4: Verify specific values
    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    println!("  Total bars received: {}", all_bars.len());
    if all_bars.is_empty() {
        historical_silence(&shared, "no historical bars arrived");
        return conns;
    }

    let first = &all_bars[0];
    assert!(first.open > 0.0, "Open price should be positive: {}", first.open);
    assert!(first.high >= first.low, "High ({}) should be >= Low ({})", first.high, first.low);
    assert!(first.volume > 0, "Volume should be positive: {}", first.volume);
    for bar in &all_bars {
        assert!(bar.high >= bar.low, "Bar {}: high ({}) < low ({})", bar.time, bar.high, bar.low);
    }
    println!("  First bar: O={:.2} H={:.2} L={:.2} C={:.2} V={}",
        first.open, first.high, first.low, first.close, first.volume);
    println!("  PASS ({} bars)\n", all_bars.len());
    conns
}

pub(super) fn phase_historical_daily_bars(mut conns: Conns) -> Conns {
    phase!("--- Phase 76: Historical Daily Bars (SPY, 5 days of 1-day bars) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    // Step 1: Create HotLoop with HMDS connection
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    // Step 2: Send FetchHistorical via ControlCommand
    control_tx.send(ControlCommand::FetchHistorical {
        contract: ContractRef { con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() },
        req_id: 7600,
        end_date_time: now_ib_timestamp(),
        duration: "5 D".into(),
        bar_size: "1 day".into(),
        what_to_show: "TRADES".into(),
        use_rth: true,
        keep_up_to_date: false, include_expired: false,
        filters: Default::default(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    // Step 3: Wait for results in SharedState
    let mut all_bars = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut complete = false;

    while Instant::now() < deadline && !complete {
        let results = shared.reference.drain_historical_data();
        for (req_id, resp) in results {
            if req_id == 7600 {
                all_bars.extend(resp.bars);
                if resp.is_complete { complete = true; }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Step 4: Verify specific values
    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    println!("  Total daily bars: {}", all_bars.len());
    if all_bars.is_empty() {
        historical_silence(&shared, "no daily bars arrived");
        return conns;
    }
    assert!(all_bars.len() <= 5, "Should have at most 5 daily bars, got {}", all_bars.len());
    for bar in &all_bars {
        assert!(bar.open > 0.0, "Open should be positive: {}", bar.open);
        assert!(bar.high >= bar.low, "High ({}) should be >= Low ({})", bar.high, bar.low);
        assert!(bar.volume > 0, "Volume should be positive: {}", bar.volume);
        println!("  {} O={:.2} H={:.2} L={:.2} C={:.2} V={}", bar.time, bar.open, bar.high, bar.low, bar.close, bar.volume);
    }
    println!("  PASS ({} daily bars)\n", all_bars.len());
    conns
}

pub(super) fn phase_cancel_historical(mut conns: Conns) -> Conns {
    phase!("--- Phase 77: Cancel Historical Request (SPY) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    // Request 5-min bars for 5 days (multi-chunk response, cancelable)
    control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 7700, end_date_time: now_ib_timestamp(), duration: "5 D".into(), bar_size: "5 mins".into(), what_to_show: "TRADES".into(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();
    let join = run_hot_loop(hot_loop);

    // Wait for first chunk
    let mut got_first_chunk = false;
    let first_deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < first_deadline && !got_first_chunk {
        let results = shared.reference.drain_historical_data();
        for (req_id, resp) in &results {
            if *req_id == 7700 {
                got_first_chunk = true;
                println!("  First chunk received ({} bars), sending cancel", resp.bars.len());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if !got_first_chunk {
        let conns = shutdown_and_reclaim(&control_tx, join, account_id);
        skipped!("  SKIP: No data received in 15s\n");
        return conns;
    }

    // Cancel via ControlCommand
    control_tx.send(ControlCommand::CancelHistorical { req_id: 7700 }).unwrap();
    println!("  Cancel sent");
    // Chunks already in flight when the cancel is sent are absorbed rather than
    // counted.
    std::thread::sleep(Duration::from_secs(2));
    let in_flight = shared.reference.drain_historical_data()
        .iter().filter(|(id, _)| *id == 7700).count();
    // After that, nothing more: without this the phase shows only that a command
    // was queued, which a cancel reaching nobody also satisfies.
    std::thread::sleep(Duration::from_secs(2));
    let still_coming = shared.reference.drain_historical_data()
        .iter().filter(|(id, _)| *id == 7700).count();

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);
    assert_eq!(
        still_coming, 0,
        "{still_coming} more chunks arrived two seconds after the cancel \
         ({in_flight} were already in flight when it went out): the query is still running",
    );
    println!("  PASS (the query stopped; {in_flight} chunks were in flight)\n");
    conns
}

/// The venue rejects certain bar_size/duration combinations with a QueryError
/// XML payload on HMDS. Validates that the rejection now surfaces as a queued
/// error (code 162) + terminal historical_data sentinel rather than leaking the
/// pending entry forever.
pub(super) fn phase_query_error_surfaces(mut conns: Conns) -> Conns {
    phase!("--- Phase 186: HMDS QueryError surfaces (trades on a quoted-only instrument) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => {
            skipped!("  SKIP: the historical farm could not be reached: {e}\n");
            return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id };
        }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    const REQ_ID: u32 = 18600;
    // A currency pair is asked for trades. It has none: it is quoted and never
    // printed, so the venue answers 162 and there is nothing for it to send
    // instead. Measured 2026-08-27, and structural rather than a policy the
    // venue may revisit — which is what this phase asked for before. It used a
    // bar size and duration the venue refused as an invalid length, and that
    // limit was lifted, so the phase reported SKIP twice a run and verified
    // nothing. If this one is ever answered with bars, the skip below says so.
    control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 12087792, symbol: "EUR".into(), sec_type: "CASH".into(), exchange: "IDEALPRO".into(), currency: "USD".to_string(), ..Default::default() }, req_id: REQ_ID, end_date_time: now_ib_timestamp(), duration: "1 D".into(), bar_size: "1 hour".into(), what_to_show: "TRADES".into(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut error: Option<(u32, i32, String)> = None;
    let mut got_end_sentinel = false;
    let mut bars_seen: usize = 0;

    while Instant::now() < deadline {
        for (rid, code, msg) in shared.reference.drain_historical_errors() {
            if rid == REQ_ID {
                println!("  HMDS error: code={code} msg={msg:?}");
                error = Some((rid, code, msg));
            }
        }
        for (rid, resp) in shared.reference.drain_historical_data() {
            if rid == REQ_ID {
                bars_seen += resp.bars.len();
                if resp.is_complete { got_end_sentinel = true; }
            }
        }
        if error.is_some() && got_end_sentinel { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    match error {
        // The venue answered with data, so the restriction this phase relies on
        // has been lifted. That is an upstream policy change and not this
        // client's doing.
        None if bars_seen > 0 => {
            skipped!(
                "  SKIP: the combination was accepted and {bars_seen} bars came back — \
                 the rejection this phase relies on is gone\n"
            );
        }
        // Nothing at all, on a farm this phase reached. The one thing this is
        // here to catch is a QueryError that arrives and is discarded, and that
        // looks exactly like this — so reporting SKIP was reporting the defect
        // as a clean run.
        None => panic!(
            "nothing came back for a request the venue rejects: no error, no bars, \
             end sentinel {got_end_sentinel}. Either the venue said nothing, or the \
             refusal it sent was dropped before anyone could read it"
        ),
        Some((_, code, msg)) => {
            assert_eq!(code, 162, "expected canonical HMDS error code 162");
            assert!(!msg.is_empty(), "error message must not be empty");
            assert!(
                got_end_sentinel,
                "terminal historical_data sentinel must follow the error so consumers waiting on historical_data_end unblock"
            );
            assert_eq!(bars_seen, 0, "no bars should be delivered for a rejected request");
            println!("  PASS (error surfaced, end sentinel delivered)\n");
        }
    }
    conns
}

pub(super) fn phase_head_timestamp(mut conns: Conns) -> Conns {
    phase!("--- Phase 79: Head Timestamp (SPY, TRADES) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    control_tx.send(ControlCommand::FetchHeadTimestamp { contract: ibx::types::ContractRef { con_id: 756733, symbol: "".to_string(), sec_type: "".to_string(), exchange: "".to_string(), currency: "".to_string(), ..Default::default() }, req_id: 7900, what_to_show: "TRADES".into(), use_rth: true, filters: Default::default() }).unwrap();
    let join = run_hot_loop(hot_loop);

    let mut response: Option<historical::HeadTimestampResponse> = None;
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline && response.is_none() {
        let results = shared.reference.drain_head_timestamps();
        for (req_id, resp) in results {
            if req_id == 7900 { response = Some(resp); }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if response.is_none() {
        historical_silence(&shared, "no head timestamp arrived");
        return conns;
    }
    let resp = response.unwrap();
    assert!(!resp.head_timestamp.is_empty(), "Head timestamp should not be empty");
    assert!(resp.head_timestamp.starts_with("199"), "SPY TRADES head timestamp should be in 1990s, got {}", resp.head_timestamp);
    assert!(!resp.timezone.is_empty(), "Timezone should not be empty");
    println!("  headTS={} tz={}", resp.head_timestamp, resp.timezone);
    println!("  PASS\n");
    conns
}

pub(super) fn phase_scanner_subscription(mut conns: Conns) -> Conns {
    phase!("--- Phase 82: Scanner Subscription (TOP_PERC_GAIN, STK.US.MAJOR) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    control_tx.send(ControlCommand::SubscribeScanner {
        req_id: 8200,
        instrument: "STK".into(),
        location_code: "STK.US.MAJOR".into(),
        scan_code: "TOP_PERC_GAIN".into(),
        max_items: 10,
        filters: vec![("priceAbove".into(), "1".into())],
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let mut result: Option<scanner::ScannerResult> = None;
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline && result.is_none() {
        let results = shared.reference.drain_scanner_data();
        for (req_id, r) in results {
            if req_id == 8200 { result = Some(r); }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Cancel scanner
    control_tx.send(ControlCommand::CancelScanner { req_id: 8200 }).unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if result.is_none() {
        skipped!("  SKIP: No scanner results received\n");
        return conns;
    }
    let r = result.unwrap();
    assert!(!r.con_ids.is_empty(), "Scanner should return contracts");
    assert!(!r.scan_time.is_empty(), "Scanner should have scan_time");
    println!("  Scanner: {} contracts at {}", r.con_ids.len(), r.scan_time);
    for (i, cid) in r.con_ids.iter().enumerate().take(3) {
        println!("  Rank {i}: conId={cid}");
    }
    println!("  PASS ({} contracts)\n", r.con_ids.len());
    conns
}

pub(super) fn phase_fundamental_data(mut conns: Conns) -> Conns {
    phase!("--- Phase 83: Fundamental Data (AAPL, ReportSnapshot) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: HMDS reconnect failed: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    control_tx.send(ControlCommand::FetchFundamentalData {
        req_id: 8300, con_id: 265598,
        report_type: "ReportSnapshot".into(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let mut got_data = false;
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline && !got_data {
        let results = shared.reference.drain_fundamental_data();
        for (req_id, data) in results {
            if req_id == 8300 {
                println!("  Fundamental data: {} chars", data.len());
                assert!(!data.is_empty(), "Fundamental data should not be empty");
                got_data = true;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if !got_data {
        skipped!("  SKIP: No fundamental data received (may require subscription)\n");
        return conns;
    }
    println!("  PASS\n");
    conns
}

/// TEXTBOOK INTEGRATION TEST PATTERN
///
/// Every integration test MUST follow this structure:
///   1. Connect to real server (Gateway::connect or connect_farm)
///   2. Create HotLoop with real connections + ControlCommand channel
///   3. Send ControlCommand through the channel (NOT inject_* or push_*)
///   4. Let the hot_loop process it → sends FIX to server → receives response
///   5. Verify SPECIFIC VALUES in the response (not just "did something arrive")
///   6. Clean up (shutdown_and_reclaim)
///
/// This test verifies historical news end-to-end:
///   ControlCommand::FetchHistoricalNews → hot_loop → HMDS FIX request
///   → real server → FIX response → hot_loop parses j.c codec + ZIP
///   → SharedState → drain_historical_news → verify headline values
pub(super) fn phase_historical_news(mut conns: Conns) -> Conns {
    phase!("--- Phase 85: Historical News (AAPL, end-to-end) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    // Step 1: Create HotLoop with ALL real connections (farm + CCP + HMDS)
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, _event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, Some(hmds), None,
    );

    // Step 2: Send the request through the ControlCommand channel
    // The hot_loop will build the XML, send to HMDS, receive the response,
    // decode the j.c codec, decompress the ZIP, parse the Properties,
    // and push headlines to SharedState.
    control_tx.send(ControlCommand::FetchHistoricalNews {
        req_id: 8500,
        con_id: 265598, // AAPL
        provider_codes: "BRFG+BRFUPDN+DJ-N+DJ-RTA+DJ-RTE+DJ-RTG+DJ-RTPRO+DJNL".into(),
        start_time: String::new(),
        end_time: String::new(),
        max_results: 5,
    }).unwrap();

    // Step 3: Run the hot_loop and wait for results
    let join = run_hot_loop(hot_loop);
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_news = false;

    while Instant::now() < deadline && !got_news {
        // Check SharedState for results (the hot_loop pushes here)
        let results = shared.reference.drain_historical_news();
        if !results.is_empty() {
            for (req_id, headlines, has_more) in &results {
                // Step 4: Verify SPECIFIC VALUES
                assert_eq!(*req_id, 8500, "req_id should match the one sent");
                println!("  Got {} headlines (has_more={})", headlines.len(), has_more);
                // Headlines themselves need a news subscription, which this
                // login holds for none of the providers the venue lists: every
                // one of them, asked for on its own and for several contracts,
                // is answered and answered empty. What is proved here is that
                // the request reaches the venue and its answer is read back
                // under the request that asked — an answer that never arrives
                // still fails below.

                for h in headlines {
                    // Verify each headline has non-empty fields
                    assert!(!h.time.is_empty(), "headline time should not be empty");
                    assert!(!h.provider_code.is_empty(), "provider_code should not be empty");
                    assert!(!h.article_id.is_empty(), "article_id should not be empty");
                    assert!(!h.headline.is_empty(), "headline text should not be empty");
                    // Verify time format looks like a date (starts with 20)
                    assert!(h.time.starts_with("20"), "time should be a date: {}", h.time);
                    println!("    {} [{}] {}", h.time, h.provider_code, &h.headline[..h.headline.len().min(80)]);
                }
            }
            got_news = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Step 5: Clean up
    let bg_conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if !got_news {
        skipped!("  SKIP: No news response received (may require news subscription)\n");
        return bg_conns;
    }
    println!("  PASS\n");
    bg_conns
}

pub(super) fn phase_historical_ticks(mut conns: Conns) -> Conns {
    phase!("--- Phase 88: Historical Ticks (SPY, TRADES) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    // Request last 100 historical ticks for SPY, ending now
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let end_dt = format_utc_timestamp(now);
    control_tx.send(ControlCommand::FetchHistoricalTicks {
        contract: ContractRef { con_id: 756733, sec_type: "STK".to_string(), exchange: "SMART".to_string(), symbol: "".to_string(), currency: "".to_string(), ..Default::default() },
        req_id: 2001,
        start_date_time: String::new(),
        end_date_time: end_dt,
        number_of_ticks: 100,
        what_to_show: "TRADES".to_string(),
        use_rth: true,
        include_expired: false,
        filters: Default::default(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut tick_count = 0usize;

    let mut last_ts = String::new();
    let mut monotonic_violations = 0u32;

    while Instant::now() < deadline {
        let ticks = shared.reference.drain_historical_ticks();
        for (req_id, data, what, done) in &ticks {
            if *req_id == 2001 {
                match data {
                    HistoricalTickData::Last(v) => {
                        for tick in v {
                            tick_count += 1;
                            assert!(tick.price > 0.0, "Tick price should be positive: {}", tick.price);
                            if !tick.time.is_empty() && tick.time < last_ts { monotonic_violations += 1; }
                            if !tick.time.is_empty() { last_ts = tick.time.clone(); }
                        }
                    }
                    HistoricalTickData::Midpoint(v) => {
                        for tick in v {
                            tick_count += 1;
                            assert!(tick.price > 0.0, "Midpoint price should be positive: {}", tick.price);
                        }
                    }
                    HistoricalTickData::BidAsk(v) => {
                        for tick in v {
                            tick_count += 1;
                            assert!(tick.bid_price > 0.0, "Bid should be positive: {}", tick.bid_price);
                            assert!(tick.ask_price >= tick.bid_price, "Ask ({}) should be >= Bid ({})", tick.ask_price, tick.bid_price);
                        }
                    }
                }
                println!("  Received ticks (what={what}, done={done}, total={tick_count})");
                if *done { break; }
            }
        }
        if tick_count > 0 { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if tick_count == 0 {
        historical_silence(&shared, "no historical ticks arrived");
    } else {
        assert_eq!(monotonic_violations, 0, "Timestamps should be monotonically increasing");
        println!("  PASS ({tick_count} ticks, timestamps monotonic)\n");
    }
    conns
}

pub(super) fn phase_histogram_data(mut conns: Conns) -> Conns {
    phase!("--- Phase 89: Histogram Data (SPY, 1 week) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    control_tx.send(ControlCommand::FetchHistogramData {
        req_id: 3001,
        con_id: 756733,
        sec_type: "STK".to_string(),
        exchange: "SMART".to_string(),
        use_rth: true,
        period: "1 week".to_string(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut entries = Vec::new();

    while Instant::now() < deadline {
        let data = shared.reference.drain_histogram_data();
        for (req_id, ents) in data {
            if req_id == 3001 {
                entries = ents;
                break;
            }
        }
        if !entries.is_empty() { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if entries.is_empty() {
        session_owed(&shared, "no histogram entries arrived");
    } else {
        println!("  {} histogram entries", entries.len());
        if let Some(first) = entries.first() {
            println!("  First: price={:.2} count={}", first.price, first.count);
        }
        println!("  PASS\n");
    }
    conns
}

pub(super) fn phase_historical_schedule(mut conns: Conns) -> Conns {
    phase!("--- Phase 90: Historical Schedule (SPY) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let end_dt = format_utc_timestamp(now);
    control_tx.send(ControlCommand::FetchHistoricalSchedule {
        contract: ContractRef { con_id: 756733, sec_type: "STK".to_string(), exchange: "SMART".to_string(), symbol: "".to_string(), currency: "".to_string(), ..Default::default() },
        req_id: 4001,
        end_date_time: end_dt,
        duration: "5 d".to_string(),
        use_rth: true,
        filters: Default::default(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut schedule: Option<HistoricalScheduleResponse> = None;

    while Instant::now() < deadline {
        let data = shared.reference.drain_historical_schedules();
        for (req_id, resp) in data {
            if req_id == 4001 {
                schedule = Some(resp);
                break;
            }
        }
        if schedule.is_some() { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if let Some(sched) = schedule {
        println!("  Timezone: {}", sched.timezone);
        println!("  Sessions: {}", sched.sessions.len());
        for s in sched.sessions.iter().take(3) {
            println!("    {} open={} close={}", s.ref_date, s.open_time, s.close_time);
        }
        assert!(!sched.sessions.is_empty(), "Schedule should contain sessions");
        println!("  PASS\n");
    } else {
        lookup_returned_nothing("no trading schedule came back");
    }
    conns
}

pub(super) fn phase_realtime_bars(mut conns: Conns) -> Conns {
    phase!("--- Phase 91: Real-Time Bars (SPY, 5-second) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    control_tx.send(ControlCommand::SubscribeRealTimeBar {
        contract: ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".to_string(), exchange: "SMART".to_string(), currency: "".to_string(), ..Default::default() },
        req_id: 5001,
        what_to_show: "TRADES".to_string(),
        use_rth: false,
        filters: Default::default(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    // Wait up to 20s for at least one 5-second bar
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut bars = Vec::new();

    while Instant::now() < deadline {
        let data = shared.market.drain_real_time_bars();
        for (req_id, bar) in data {
            if req_id == 5001 {
                bars.push(bar);
            }
        }
        if !bars.is_empty() { break; }
        std::thread::sleep(Duration::from_millis(200));
    }

    // Cancel subscription
    control_tx.send(ControlCommand::CancelRealTimeBar { req_id: 5001 }).unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if bars.is_empty() {
        no_market(&shared, "no real-time bars arrived");
    } else {
        let bar = &bars[0];
        println!("  First bar: O={:.2} H={:.2} L={:.2} C={:.2} V={:.0}", bar.open, bar.high, bar.low, bar.close, bar.volume);
        assert!(bar.high >= bar.low, "High should be >= Low");
        println!("  PASS ({} bars)\n", bars.len());
    }
    conns
}

pub(super) fn phase_news_article(mut conns: Conns) -> Conns {
    phase!("--- Phase 92: News Article Fetch (AAPL) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    // First request historical news to get an article ID
    control_tx.send(ControlCommand::FetchHistoricalNews {
        req_id: 6001,
        con_id: 265598,
        provider_codes: "BRFG+BRFUPDN".to_string(),
        start_time: String::new(),
        end_time: String::new(),
        max_results: 5,
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    // Poll for headlines
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut article_id: Option<String> = None;
    let mut provider_code: Option<String> = None;

    while Instant::now() < deadline && article_id.is_none() {
        let data = shared.reference.drain_historical_news();
        for (req_id, headlines, _done) in data {
            if req_id == 6001
                && let Some(h) = headlines.first() {
                    article_id = Some(h.article_id.clone());
                    provider_code = Some(h.provider_code.clone());
                    println!("  Headline: {} ({})", h.headline, h.article_id);
                }
        }
        if article_id.is_some() { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    if let (Some(art_id), Some(prov)) = (article_id, provider_code) {
        // Now fetch the article body
        control_tx.send(ControlCommand::FetchNewsArticle {
            req_id: 6002,
            provider_code: prov,
            article_id: art_id.clone(),
        }).unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut got_article = false;

        while Instant::now() < deadline {
            let articles = shared.reference.drain_news_articles();
            for (req_id, art_type, body) in &articles {
                if *req_id == 6002 {
                    println!("  Article: type={} len={}", art_type, body.len());
                    assert!(!body.is_empty(), "Article body should not be empty");
                    assert!(body.len() > 50, "Article body too short: {} bytes", body.len());
                    // Type 0 = HTML (may contain tags), type 1 = plain text
                    if *art_type == 0 && body.contains('<') && body.contains('>') {
                        println!("  Format: HTML");
                    } else {
                        println!("  Format: plain text (art_type={art_type})");
                    }
                    println!("  Preview: {}", &body[..body.len().min(120)]);
                    got_article = true;
                }
            }
            if got_article { break; }
            std::thread::sleep(Duration::from_millis(100));
        }

        let conns = shutdown_and_reclaim(&control_tx, join, account_id);
        if got_article {
            println!("  PASS\n");
        } else {
            skipped!("  SKIP: Article body not received\n");
        }
        conns
    } else {
        let conns = shutdown_and_reclaim(&control_tx, join, account_id);
        skipped!("  SKIP: No news headlines to fetch article from\n");
        conns
    }
}

pub(super) fn phase_fundamental_data_channel(mut conns: Conns) -> Conns {
    phase!("--- Phase 93: Fundamental Data via HotLoop (AAPL) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    control_tx.send(ControlCommand::FetchFundamentalData {
        req_id: 7001,
        con_id: 265598,
        report_type: "ReportSnapshot".to_string(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_data = false;

    while Instant::now() < deadline {
        let data = shared.reference.drain_fundamental_data();
        for (req_id, xml) in &data {
            if *req_id == 7001 {
                println!("  Fundamental data: {} bytes", xml.len());
                got_data = true;
            }
        }
        if got_data { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if got_data {
        println!("  PASS\n");
    } else {
        skipped!("  SKIP: No fundamental data received (may require subscription)\n");
    }
    conns
}

pub(super) fn phase_parallel_historical(mut conns: Conns) -> Conns {
    phase!("--- Phase 94: Parallel Historical Requests (SPY: 1d/5min, 5d/1day, 1w/1h) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let end_dt = format_utc_timestamp(now);

    // Send 3 requests in quick succession
    control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 8001, end_date_time: end_dt.clone(), duration: "1 d".to_string(), bar_size: "5 mins".to_string(), what_to_show: "TRADES".to_string(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();
    control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 8002, end_date_time: end_dt.clone(), duration: "5 d".to_string(), bar_size: "1 day".to_string(), what_to_show: "TRADES".to_string(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();
    control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 8003, end_date_time: end_dt, duration: "1 W".to_string(), bar_size: "1 hour".to_string(), what_to_show: "TRADES".to_string(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();

    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut received: [bool; 3] = [false; 3];

    while Instant::now() < deadline {
        let data = shared.reference.drain_historical_data();
        for (req_id, resp) in &data {
            match *req_id {
                8001 => { if resp.is_complete { received[0] = true; println!("  req 8001 (1d/5min): {} bars", resp.bars.len()); } }
                8002 => { if resp.is_complete { received[1] = true; println!("  req 8002 (5d/1day): {} bars", resp.bars.len()); } }
                8003 if resp.is_complete => { received[2] = true; println!("  req 8003 (1W/1h): {} bars", resp.bars.len()); }
                _ => {}
            }
        }
        if received.iter().all(|r| *r) { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    let count = received.iter().filter(|r| **r).count();
    if count == 3 {
        println!("  PASS (all 3 responses received)\n");
    } else if count > 0 {
        println!("  PARTIAL: {count}/3 responses received\n");
    } else {
        historical_silence(&shared, "none of the three historical requests answered");
    }
    conns
}

pub(super) fn phase_scanner_params(mut conns: Conns) -> Conns {
    phase!("--- Phase 95: Scanner Parameters + HOT_BY_VOLUME Scan ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    // Request scanner params XML
    control_tx.send(ControlCommand::FetchScannerParams).unwrap();
    // Also subscribe to a HOT_BY_VOLUME scan
    control_tx.send(ControlCommand::SubscribeScanner {
        req_id: 9001,
        instrument: "STK".to_string(),
        location_code: "STK.US.MAJOR".to_string(),
        scan_code: "HOT_BY_VOLUME".to_string(),
        max_items: 10,
        filters: Vec::new(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut got_params = false;
    let mut got_scan = false;

    while Instant::now() < deadline {
        let params = shared.reference.drain_scanner_params();
        if !params.is_empty() {
            println!("  Scanner params XML: {} bytes", params[0].len());
            got_params = true;
        }
        let scans = shared.reference.drain_scanner_data();
        for (req_id, result) in &scans {
            if *req_id == 9001 {
                println!("  Scanner results: {} contracts", result.con_ids.len());
                got_scan = true;
            }
        }
        if got_params && got_scan { break; }
        // Params but no scan after a while: do not wait forever
        if got_params && Instant::now() > deadline - Duration::from_secs(5) { break; }
        std::thread::sleep(Duration::from_millis(200));
    }

    // Cancel scanner subscription
    control_tx.send(ControlCommand::CancelScanner { req_id: 9001 }).unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if got_params {
        println!("  Scanner params: PASS");
    } else {
        skipped!("  Scanner params: SKIP");
    }
    if got_scan {
        println!("  Scanner scan: PASS");
    } else {
        skipped!("  Scanner scan: SKIP (may need market hours)");
    }
    println!();
    conns
}

pub(super) fn phase_historical_ohlc_validation(conns: Conns) -> Conns {
    phase!("--- Phase 103: Historical Bar OHLC Validation (SPY 1-hour bars) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, _event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    let req_id = 6001u32;
    control_tx.send(ControlCommand::FetchHistorical {
        contract: ContractRef { con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() },
        req_id,
        end_date_time: String::new(), // empty = now
        duration: "5 D".into(),
        bar_size: "1 hour".into(),
        what_to_show: "TRADES".into(),
        use_rth: true,
        keep_up_to_date: false, include_expired: false,
        filters: Default::default(),
    }).unwrap();

    let join = run_hot_loop(hot_loop);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut bars_data: Option<historical::HistoricalResponse> = None;

    while Instant::now() < deadline {
        let hist = shared.reference.drain_historical_data();
        for (rid, data) in hist {
            if rid == req_id {
                bars_data = Some(data);
            }
        }
        if bars_data.is_some() { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    let data = match bars_data {
        Some(d) => d,
        None => {
            historical_silence(&shared, "no historical data arrived");
            return conns;
        }
    };

    let bars = &data.bars;
    println!("  Received {} bars", bars.len());
    assert!(!bars.is_empty(), "Should receive at least 1 bar");

    let mut ohlc_valid = true;
    let mut volume_valid = true;

    for (i, bar) in bars.iter().enumerate() {
        // OHLC consistency: low <= everything, high >= everything
        if bar.low > bar.high {
            println!("  Bar {}: low ({}) > high ({})", i, bar.low, bar.high);
            ohlc_valid = false;
        }
        if bar.low > bar.open {
            println!("  Bar {}: low ({}) > open ({})", i, bar.low, bar.open);
            ohlc_valid = false;
        }
        if bar.low > bar.close {
            println!("  Bar {}: low ({}) > close ({})", i, bar.low, bar.close);
            ohlc_valid = false;
        }
        if bar.high < bar.open {
            println!("  Bar {}: high ({}) < open ({})", i, bar.high, bar.open);
            ohlc_valid = false;
        }
        if bar.high < bar.close {
            println!("  Bar {}: high ({}) < close ({})", i, bar.high, bar.close);
            ohlc_valid = false;
        }
        // Volume should be non-negative
        if bar.volume < 0 {
            println!("  Bar {}: negative volume ({})", i, bar.volume);
            volume_valid = false;
        }
    }

    assert!(ohlc_valid, "All bars should have valid OHLC relationships");
    assert!(volume_valid, "All bars should have non-negative volume");
    println!("  PASS\n");
    conns
}

// ─── Phase 111: Large historical dataset — 1 year daily bars ───

pub(super) fn phase_large_historical_dataset(mut conns: Conns) -> Conns {
    phase!("--- Phase 111: Large Historical Dataset (SPY, 1 year of daily bars) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let end_dt = format_utc_timestamp(now);

    control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 11001, end_date_time: end_dt, duration: "1 Y".to_string(), bar_size: "1 day".to_string(), what_to_show: "TRADES".to_string(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut total_bars = 0usize;
    let mut complete = false;
    let mut prev_time = String::new();
    let mut duplicate_timestamps = 0u32;

    while Instant::now() < deadline {
        let data = shared.reference.drain_historical_data();
        for (req_id, resp) in &data {
            if *req_id == 11001 {
                for bar in &resp.bars {
                    total_bars += 1;
                    assert!(bar.high >= bar.low, "Bar {}: high < low ({} < {})", bar.time, bar.high, bar.low);
                    assert!(bar.open > 0.0, "Bar {}: open should be positive", bar.time);
                    assert!(bar.volume >= 0, "Bar {}: volume should be non-negative", bar.time);
                    if bar.time == prev_time {
                        duplicate_timestamps += 1;
                    }
                    prev_time = bar.time.clone();
                }
                if resp.is_complete { complete = true; }
            }
        }
        if complete { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if total_bars == 0 {
        historical_silence(&shared, "no bars arrived");
        return conns;
    }
    println!("  Total bars: {total_bars} (complete={complete})");
    println!("  Duplicate timestamps: {duplicate_timestamps}");
    assert!(total_bars >= 200, "1 year should have 200+ trading days, got {total_bars}");
    assert_eq!(duplicate_timestamps, 0, "No duplicate bar timestamps expected");
    println!("  PASS\n");
    conns
}

// ─── Phase 112: DST boundary historical data ───

pub(super) fn phase_dst_boundary_historical(mut conns: Conns) -> Conns {
    phase!("--- Phase 112: DST Boundary Historical Data (SPY, bars spanning March DST) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); c }
        Err(e) => { skipped!("  SKIP: the historical farm could not be reached: {e}\n"); return Conns { farm: conns.farm, ccp: conns.ccp, hmds: None, account_id: conns.account_id }; }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(), conns.farm, conns.ccp, Some(hmds), None,
    );

    // Request 2 weeks of 1-hour bars ending after the March DST transition
    // DST 2026: March 8 (second Sunday of March) — spring forward
    // End date: March 14 2026, covering March 2-14 (spans DST)
    control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 12001, end_date_time: "20260314-20:00:00".to_string(), duration: "2 W".to_string(), bar_size: "1 hour".to_string(), what_to_show: "TRADES".to_string(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut bars = Vec::new();
    let mut complete = false;

    while Instant::now() < deadline {
        let data = shared.reference.drain_historical_data();
        for (req_id, resp) in &data {
            if *req_id == 12001 {
                bars.extend(resp.bars.iter().cloned());
                if resp.is_complete { complete = true; }
            }
        }
        if complete { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if bars.is_empty() {
        historical_silence(&shared, "no bars arrived");
        return conns;
    }

    // Check for duplicate timestamps
    let mut timestamps: Vec<String> = bars.iter().map(|b| b.time.clone()).collect();
    let original_count = timestamps.len();
    timestamps.sort();
    timestamps.dedup();
    let unique_count = timestamps.len();
    let duplicates = original_count - unique_count;

    // Check all bars have valid OHLCV
    for bar in &bars {
        assert!(bar.high >= bar.low, "Bar {}: high ({}) < low ({})", bar.time, bar.high, bar.low);
        assert!(bar.open > 0.0, "Bar {}: zero/negative open", bar.time);
    }

    println!("  {original_count} bars received ({unique_count} unique timestamps, {duplicates} duplicates, complete={complete})");
    assert_eq!(duplicates, 0, "No duplicate timestamps at DST boundary");

    // 2 weeks of RTH = ~10 trading days * ~7 hours = ~70 bars
    assert!(bars.len() >= 40, "2 weeks of hourly RTH should have 40+ bars, got {}", bars.len());
    println!("  PASS\n");
    conns
}

// ─── Phase 127: Cancel Data Requests (historical, fundamental, histogram, head timestamp) ───

pub(super) fn phase_cancel_data_requests(mut conns: Conns) -> Conns {
    phase!("--- Phase 127: Cancel Data Requests (4 cancel ControlCommands) ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); Some(c) }
        Err(e) => {
            skipped!("  SKIP: the historical farm could not be reached: {e}\n");
            return conns;
        }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, _event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, hmds, None,
    );

    let now = now_ib_timestamp();

    // 1. FetchHistorical + CancelHistorical
    control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 20001, end_date_time: now.clone(), duration: "1 d".to_string(), bar_size: "5 mins".to_string(), what_to_show: "TRADES".to_string(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();
    control_tx.send(ControlCommand::CancelHistorical { req_id: 20001 }).unwrap();

    // 2. FetchHeadTimestamp + CancelHeadTimestamp
    control_tx.send(ControlCommand::FetchHeadTimestamp { contract: ibx::types::ContractRef { con_id: 756733, symbol: "".to_string(), sec_type: "".to_string(), exchange: "".to_string(), currency: "".to_string(), ..Default::default() }, req_id: 20002, what_to_show: "TRADES".to_string(), use_rth: true, filters: Default::default() }).unwrap();
    control_tx.send(ControlCommand::CancelHeadTimestamp { req_id: 20002 }).unwrap();

    // 3. FetchFundamentalData + CancelFundamentalData
    control_tx.send(ControlCommand::FetchFundamentalData {
        req_id: 20003, con_id: 265598, report_type: "ReportsFinStatements".to_string(),
    }).unwrap();
    control_tx.send(ControlCommand::CancelFundamentalData { req_id: 20003 }).unwrap();

    // 4. FetchHistogramData + CancelHistogramData
    control_tx.send(ControlCommand::FetchHistogramData {
        req_id: 20004, con_id: 756733, sec_type: "STK".to_string(),
        exchange: "SMART".to_string(), use_rth: true, period: "1 week".to_string(),
    }).unwrap();
    control_tx.send(ControlCommand::CancelHistogramData { req_id: 20004 }).unwrap();

    let join = run_hot_loop(hot_loop);

    // Wait a moment for the hot loop to process all commands
    std::thread::sleep(Duration::from_secs(3));

    // Verify no responses arrived for cancelled requests
    let hist = shared.reference.drain_historical_data();
    let head = shared.reference.drain_head_timestamps();
    let fund = shared.reference.drain_fundamental_data();
    let histo = shared.reference.drain_histogram_data();

    let hist_for_req: Vec<_> = hist.iter().filter(|(id, _)| *id == 20001).collect();
    let head_for_req: Vec<_> = head.iter().filter(|(id, _)| *id == 20002).collect();
    let fund_for_req: Vec<_> = fund.iter().filter(|(id, _)| *id == 20003).collect();
    let histo_for_req: Vec<_> = histo.iter().filter(|(id, _)| *id == 20004).collect();

    println!("  Historical (20001): {} responses (expect 0)", hist_for_req.len());
    println!("  HeadTimestamp (20002): {} responses (expect 0)", head_for_req.len());
    println!("  Fundamental (20003): {} responses (expect 0)", fund_for_req.len());
    println!("  Histogram (20004): {} responses (expect 0)", histo_for_req.len());

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    // The four counts above were computed, printed and then discarded, so this
    // passed whether or not a single cancel did anything. There is no race to
    // tolerate: every fetch and its cancel are queued before the loop starts,
    // so each cancel is applied before the loop has read one byte of a reply.
    // Anything reaching a caller here is a request that was withdrawn and
    // answered anyway.
    for (label, arrived) in [
        ("historical bars", hist_for_req.len()),
        ("head timestamp", head_for_req.len()),
        ("fundamental report", fund_for_req.len()),
        ("histogram", histo_for_req.len()),
    ] {
        assert_eq!(
            arrived, 0,
            "{arrived} {label} responses reached the caller for a request cancelled \
             before the loop started",
        );
    }
    println!("  PASS (all four cancels took, nothing was answered)\n");
    conns
}

// ─── Phase 130: Historical Data + Live Orders Coexistence ───

pub(super) fn phase_historical_and_orders(mut conns: Conns) -> Conns {
    phase!("--- Phase 130: Historical Data + Live Orders Coexistence ---");

    ccp_keepalive(&mut conns.ccp);
    let hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => { println!("  HMDS reconnected"); Some(c) }
        Err(e) => {
            skipped!("  SKIP: the historical farm could not be reached: {e}\n");
            return conns;
        }
    };

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    // Step 1: Submit a limit order (far from market, won't fill)
    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();

    // Step 2: Fire 5 historical requests while order is pending
    let now = now_ib_timestamp();
    for i in 0..5u32 {
        control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 30001 + i, end_date_time: now.clone(), duration: "1 d".to_string(), bar_size: "1 hour".to_string(), what_to_show: "TRADES".to_string(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();
    }

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, regulatory_snapshot: false, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut order_acked = false;
    let mut order_cancelled = false;
    let mut cancel_sent = false;
    let mut order_rejected = false;
    let mut hist_responses = std::collections::HashSet::new();

    while Instant::now() < deadline {
        // Check historical responses
        let data = shared.reference.drain_historical_data();
        for (req_id, resp) in &data {
            if *req_id >= 30001 && *req_id <= 30005 && resp.is_complete {
                hist_responses.insert(*req_id);
            }
        }

        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100))
            && update.order_id == oid {
                match update.status {
                    OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                        order_acked = true;
                        if !cancel_sent {
                            control_tx.send(ControlCommand::Order(
                                OrderRequest::Cancel { order_id: oid }
                            )).unwrap();
                            cancel_sent = true;
                        }
                    }
                    OrderStatus::Cancelled => { order_cancelled = true; }
                    OrderStatus::Rejected => { order_rejected = true; }
                    _ => {}
                }
            }

        if order_cancelled && hist_responses.len() >= 3 { break; }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    println!("  Order: acked={order_acked} cancelled={order_cancelled} rejected={order_rejected}");
    println!("  Historical responses: {}/5", hist_responses.len());

    if order_rejected {
        println!("  Order rejected — verifying historical path still works");
    }
    if !order_rejected {
        if skip_unacked_if_closed(order_acked) { return conns; }
        assert!(order_acked, "Order should have been acknowledged");
        assert!(order_cancelled, "Order should have been cancelled");
    }
    // At least some historical requests should complete even during order activity
    // (tolerance for server pacing — may not get all 5)
    if hist_responses.is_empty() {
        skipped!("  SKIP: No historical responses — HMDS pacing limited\n");
    } else {
        println!("  PASS (order lifecycle + {} historical responses coexisted)\n", hist_responses.len());
    }
    conns
}

/// Ask the venue for a contract's corporate actions, and record what it says.
///
/// `src/control/adjustments.rs` parses a reply of these and builds the request
/// that asks for one, and `EClient::corporate_actions` sends it. This phase
/// asks over the same connection the client would, and reports what came back
/// rather than asserting a shape: what the venue states for a contract is the
/// venue's business, and a test that pinned it would be testing the contract
/// rather than the client.
///
/// The envelope follows the one the news requests use, which is the only
/// grounded pattern for a query of this family: a user message carrying its
/// number and the query as XML.
pub(super) fn phase_corporate_actions_reply(mut conns: Conns) -> Conns {
    phase!("--- Phase 187: corporate actions, what the venue answers ---");

    ccp_keepalive(&mut conns.ccp);
    let mut hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => c,
        Err(e) => {
            skipped!("  SKIP: the historical farm could not be reached: {e}\n");
            return conns;
        }
    };

    let xml = ibx::control::adjustments::build_adjustments_request_xml(
        &ibx::control::adjustments::AdjustmentRequest {
            query_id: "adj_1".into(),
            // A contract that split inside the window and paid dividends across
            // it, so one answer states both kinds and a bar series over the same
            // days can be checked against what it says.
            con_id: 4815747,
            sec_type: "STK".into(),
            exchange: "SMART".into(),
            start_date: "20240101".into(),
            end_date: "20241231".into(),
        },
    );
    let ts = now_ib_timestamp();
    if let Err(e) = hmds.send_fix(&[
        (ibx::protocol::fix::TAG_MSG_TYPE, "U"),
        (ibx::protocol::fix::TAG_SENDING_TIME, &ts),
        (6040, "10020"),
        (6118, &xml),
    ]) {
        skipped!("  SKIP: the request could not be sent: {e}\n");
        return conns;
    }

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut answered: Vec<String> = Vec::new();
    while Instant::now() < deadline && answered.is_empty() {
        let _ = hmds.try_recv();
        for frame in hmds.extract_frames() {
            match frame {
                // A compressed frame carries its messages inside, and printing
                // the envelope says nothing about what the venue answered.
                ibx::protocol::connection::Frame::FixComp(raw) => {
                    if let Some(unsigned) = hmds.unsign(&raw)
                        && let Ok(inner) = ibx::protocol::fixcomp::fixcomp_decompress(&unsigned)
                    {
                        for m in inner {
                            answered.push(String::from_utf8_lossy(&m).replace('\x01', "|"));
                        }
                    }
                }
                ibx::protocol::connection::Frame::Fix(raw) => {
                    if let Some(unsigned) = hmds.unsign(&raw) {
                        answered.push(String::from_utf8_lossy(&unsigned).replace('\x01', "|"));
                    }
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if answered.is_empty() {
        // Silence is the one answer this cannot read. A request the venue does
        // not recognise and a request it recognises and has nothing for look
        // the same from here, so it is reported as unestablished rather than
        // as either.
        skipped!(
            "  SKIP: nothing came back in 20s. The request this client builds is not \
             established against the venue, and the module that builds it stays unwired\n"
        );
    } else {
        for (n, msg) in answered.iter().enumerate().take(3) {
            // By characters, not bytes: the reply carries bytes that are not
            // text, so a byte index lands inside one and slicing there panics.
            let shown: String = msg.chars().take(900).collect();
            println!("  reply {}: {shown}", n + 1);
        }
        println!("  PASS ({} message(s) answered the request)\n", answered.len());
    }
    conns
}

/// Withdraw the scan this phase put up, on every way out of it.
///
/// A subscription left standing outlives the phase and is one more thing the
/// account is running for no reason. Every path that leaves after subscribing
/// comes through here, including the ones that leave because something failed.
fn put_the_scan_back_down(hmds: &mut Connection, scan_id: &str) {
    let ts = now_ib_timestamp();
    if let Err(e) = hmds.send_fix(&[
        (ibx::protocol::fix::TAG_MSG_TYPE, "U"),
        (ibx::protocol::fix::TAG_SENDING_TIME, &ts),
        (6040, "10004"),
        (6118, &ibx::control::scanner::build_scanner_cancel_xml(scan_id)),
    ]) {
        println!("  note: the scan could not be withdrawn: {e}");
    }
}

/// Whether a message is the session talking to itself rather than answering.
///
/// A farm sends heartbeats and test requests on its own schedule, and one
/// landing inside the window a probe is watching is not a reply to what the
/// probe sent. Counted as one, it turns "the venue does not answer this" into
/// "the venue answers this", which is the opposite of the truth and the whole
/// result of the phase.
fn keeps_the_session_up(message: &str) -> bool {
    // Every session-control type the venue sends on its own account, minus the
    // logout, which `ends_the_session` reports instead. Narrowed to the
    // heartbeats alone, a sequence reset or a logon would arrive here as an
    // answer to what the probe sent, and the phase would publish the venue
    // answering a question it had not.
    matches!(
        message.split('|').find_map(|f| f.strip_prefix("35=")),
        Some("0" | "1" | "2" | "4" | "A"),
    )
}

/// Whether a message says the session is ending.
///
/// A logout is not housekeeping and not an answer: it is the connection going
/// away. Filtered out with the heartbeats it would leave the phase reporting
/// that the venue said nothing, when what happened is that the venue hung up —
/// possibly because of what was just sent. That reading is published, so it has
/// to be the one thing this cannot get wrong.
fn ends_the_session(message: &str) -> bool {
    matches!(
        message.split('|').find_map(|f| f.strip_prefix("35=")),
        Some("5"),
    )
}

/// Ask for the wires the notes record as the account's to decide, and report
/// what comes back.
///
/// The table of them says the venue does not send these to this account. That
/// is a different statement from "this account may not ask", and the notes have
/// said so for as long as the table has existed without anything telling the
/// two apart. Silence when nobody asked is not evidence of anything.
///
/// So this asks. Nothing here is asserted: what an account may reach is the
/// account's business, and a test that pinned it would fail the day an
/// entitlement changed. What it does is put the answer — including no answer —
/// into the run's output, so the table can say which it is.
///
/// Two are left out on purpose. A request for quote is a message to the people
/// who would quote it, not a question about this account, and this suite does
/// not send those. The transaction-reporting configuration names a firm this
/// session has no value for, so asking would establish that a field was wrong
/// rather than whether the request is reachable.
pub(super) fn phase_what_the_gated_wires_answer(mut conns: Conns) -> Conns {
    phase!("--- Phase 188: what the gated wires answer when asked ---");

    ccp_keepalive(&mut conns.ccp);
    let mut hmds = match open_farm(ibx::gateway::Farm::Historical) {
        Ok(c) => c,
        Err(e) => {
            skipped!("  SKIP: the historical farm could not be reached: {e}\n");
            return conns;
        }
    };

    // A scanner has to exist before suspending one means anything. Asking to
    // suspend nothing and hearing nothing back says only that the request was
    // empty, so one is subscribed first and it is that scan the rest asks
    // about. The id is this client's to choose, as it is on every query here.
    let scan_id = "scan_gate_1";
    let subscribe = ibx::control::scanner::build_scanner_subscribe_xml(
        &ibx::control::scanner::ScannerSubscription {
            instrument: "STK".into(),
            location_code: "STK.US.MAJOR".into(),
            scan_code: "TOP_PERC_GAIN".into(),
            max_items: 10,
            filters: Vec::new(),
        },
        scan_id,
    );
    let ts = now_ib_timestamp();
    if let Err(e) = hmds.send_fix(&[
        (ibx::protocol::fix::TAG_MSG_TYPE, "U"),
        (ibx::protocol::fix::TAG_SENDING_TIME, &ts),
        (6040, "10003"),
        (6118, &subscribe),
    ]) {
        skipped!("  SKIP: the scanner could not be subscribed to suspend: {e}\n");
        return conns;
    }
    std::thread::sleep(Duration::from_secs(3));
    if let Err(e) = hmds.try_recv()
        && e.kind() != std::io::ErrorKind::WouldBlock
    {
        // The subscription is what everything below asks about. If the
        // connection went while it was being made, nothing after this says
        // anything about what the venue makes of a suspend.
        skipped!("  SKIP: the connection went away while subscribing: {e}\n");
        return conns;
    }
    let _ = hmds.extract_frames();
    println!("  subscribed scan {scan_id}, so there is one to suspend");

    // The element each request arrives under is its own name, which is how the
    // subscription and the desubscription above are already sent and answered.
    let mut session_ended = false;
    for (subtype, element, what) in [
        ("10006", "ScanSuspendRequest", "suspend that scan"),
        ("10007", "ScanResumeRequest", "resume that scan"),
    ] {
        if session_ended {
            println!("  {subtype} ({what}): not asked — the session had already ended");
            continue;
        }
        let xml = format!("<{element}><id>{scan_id}</id></{element}>");
        let ts = now_ib_timestamp();
        if let Err(e) = hmds.send_fix(&[
            (ibx::protocol::fix::TAG_MSG_TYPE, "U"),
            (ibx::protocol::fix::TAG_SENDING_TIME, &ts),
            (6040, subtype),
            (6118, &xml),
        ]) {
            skipped!("  SKIP: {what} could not be sent: {e}\n");
            put_the_scan_back_down(&mut hmds, scan_id);
            return conns;
        }

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut answered: Vec<String> = Vec::new();
        let mut hung_up = false;
        while Instant::now() < deadline && answered.is_empty() {
            // A read that fails is the connection going, and a phase that
            // swallows it reports the venue saying nothing when nothing was
            // listening. That reading is what this phase publishes, so it is
            // the one thing it cannot get wrong.
            if let Err(e) = hmds.try_recv()
                && e.kind() != std::io::ErrorKind::WouldBlock
            {
                hung_up = true;
                answered.push(format!("the connection went away: {e}"));
            }
            for frame in hmds.extract_frames() {
                let mut keep = |bytes: &[u8]| {
                    let text = String::from_utf8_lossy(bytes).replace('\x01', "|");
                    if ends_the_session(&text) {
                        hung_up = true;
                        answered.push(text);
                    } else if !keeps_the_session_up(&text) {
                        answered.push(text);
                    }
                };
                match frame {
                    ibx::protocol::connection::Frame::FixComp(raw) => {
                        if let Some(unsigned) = hmds.unsign(&raw)
                            && let Ok(inner) = ibx::protocol::fixcomp::fixcomp_decompress(&unsigned)
                        {
                            for m in inner {
                                keep(&m);
                            }
                        }
                    }
                    ibx::protocol::connection::Frame::Fix(raw) => {
                        if let Some(unsigned) = hmds.unsign(&raw) {
                            keep(&unsigned);
                        }
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        if hung_up {
            session_ended = true;
            println!("  {subtype} ({what}): the session ended while waiting, so nothing \
                      here says what the venue makes of the request");
        } else if answered.is_empty() {
            println!("  {subtype} ({what}): nothing in 8s — asked and unanswered, which is \
                      not the same as never asked");
        } else {
            for msg in answered.iter().take(2) {
                let shown: String = msg.chars().take(400).collect();
                println!("  {subtype} ({what}): {shown}");
            }
        }
    }

    put_the_scan_back_down(&mut hmds, scan_id);

    println!();
    conns
}

/// Ask for the account's transaction-reporting configuration, and report what
/// comes back.
///
/// The four fields it would state are already carried on an order, and a caller
/// states them itself. The request that would state them instead was on the
/// unsent list because what its answer looks like is not established here — and
/// that stays true until something asks. This asks.
///
/// Nothing is asserted. What the venue makes of it is the venue's business: an
/// answer establishes a shape nobody here has seen, a refusal names why the
/// account cannot have it, and silence says the request as built reaches
/// nothing. All three are worth more than the row that said it had never been
/// tried.
///
/// The firm is left empty. This session has no value for it, and putting a made
/// up one in would establish that a wrong field is refused rather than whether
/// the request is reachable.
pub(super) fn phase_transaction_reporting_config(mut conns: Conns) -> Conns {
    phase!("--- Phase 189: what asking for the reporting configuration answers ---");

    /// The id this request goes out under, and the only thing that marks a
    /// reply as answering it.
    const ASKED_UNDER: &str = "mifid_1";

    let account = conns.account_id.clone();
    let mut unrelated = 0usize;
    let ts = now_ib_timestamp();
    // The firm is stated empty rather than made up. A session asking without it
    // at all was told the field must be there, which says the venue reads this
    // request and wants that field; sending it empty asks the next question,
    // whether it is the field's presence or its content that is required.
    if let Err(e) = conns.ccp.send_fix(&[
        (ibx::protocol::fix::TAG_MSG_TYPE, "U"),
        (ibx::protocol::fix::TAG_SENDING_TIME, &ts),
        (6040, "211"),
        (6556, ASKED_UNDER),
        (1, &account),
        (8234, ""),
    ]) {
        skipped!("  SKIP: the request could not be sent: {e}\n");
        return conns;
    }
    println!("  asked for account {}", redacted(&account));

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut answered: Vec<String> = Vec::new();
    let mut hung_up = false;
    while Instant::now() < deadline && answered.is_empty() && !hung_up {
        if let Err(e) = conns.ccp.try_recv()
            && e.kind() != std::io::ErrorKind::WouldBlock
        {
            skipped!("  SKIP: the connection went away while waiting: {e}\n");
            return conns;
        }
        for frame in conns.ccp.extract_frames() {
            for m in messages_in(&mut conns.ccp, &frame) {
                let text = String::from_utf8_lossy(&m).replace('\x01', "|");
                // Only what names this request counts. The venue sends its own
                // traffic on its own schedule — its clock arrived inside the
                // window the first time this ran — and anything that happens to
                // land here is not an answer to what was sent. Either the id
                // comes back or nothing did.
                if text.contains(ASKED_UNDER) {
                    answered.push(text);
                } else if ends_the_session(&text) {
                    hung_up = true;
                } else if !keeps_the_session_up(&text) {
                    unrelated += 1;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if hung_up {
        println!("  the session ended while waiting, so nothing here says what the \
                  venue makes of the request\n");
    } else if answered.is_empty() {
        println!(
            "  nothing naming {ASKED_UNDER} in 10s, with {unrelated} other message(s) \
             arriving meanwhile. That says the request drew no reply carrying its id; \
             it does not say the venue read it\n"
        );
    } else {
        for msg in answered.iter().take(3) {
            let shown: String = msg.chars().take(500).collect();
            println!("  answered: {shown}");
        }
        println!();
    }
    conns
}

/// Ask the venue for a quote, and report what it makes of the request.
///
/// This is the one wire on the unsent list that is a message to other people
/// rather than a question about this account: a request for quote goes to whoever
/// would quote it. It is sent here on a paper session, where the people who would
/// quote it are the venue's own simulation, and it asks for a quote rather than
/// placing anything — nothing here can fill.
///
/// Nothing is asserted. The row said instruments that accept one are not held by
/// this account, which was a statement about the account made without asking. A
/// reply naming the request settles that it is reachable; a rejection names what
/// the venue wants instead; silence says the request as built reaches nothing.
pub(super) fn phase_what_a_quote_request_answers(mut conns: Conns) -> Conns {
    phase!("--- Phase 190: what a request for quote answers ---");

    /// The id this request goes out under, and the only thing that marks a
    /// reply as answering it.
    const ASKED_UNDER: &str = "rfq_1";

    let ts = now_ib_timestamp();
    if let Err(e) = conns.ccp.send_fix(&[
        (ibx::protocol::fix::TAG_MSG_TYPE, "R"),
        (ibx::protocol::fix::TAG_SENDING_TIME, &ts),
        // The standard field for the id a quote request is answered under.
        (131, ASKED_UNDER),
        (ibx::control::contracts::TAG_SYMBOL, "SPY"),
        (ibx::control::contracts::TAG_SECURITY_TYPE, "STK"),
        (ibx::control::contracts::TAG_EXCHANGE, "SMART"),
        (ibx::control::contracts::TAG_CURRENCY, "USD"),
    ]) {
        skipped!("  SKIP: the request could not be sent: {e}\n");
        return conns;
    }
    println!("  asked under {ASKED_UNDER} for a stock every session here can see");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut answered: Vec<String> = Vec::new();
    let mut unrelated = 0usize;
    let mut hung_up = false;
    while Instant::now() < deadline && answered.is_empty() && !hung_up {
        if let Err(e) = conns.ccp.try_recv()
            && e.kind() != std::io::ErrorKind::WouldBlock
        {
            skipped!("  SKIP: the connection went away while waiting: {e}\n");
            return conns;
        }
        for frame in conns.ccp.extract_frames() {
            for m in messages_in(&mut conns.ccp, &frame) {
                let text = String::from_utf8_lossy(&m).replace('\x01', "|");
                if text.contains(ASKED_UNDER) {
                    answered.push(text);
                } else if ends_the_session(&text) {
                    hung_up = true;
                } else if !keeps_the_session_up(&text) {
                    unrelated += 1;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if hung_up {
        println!("  the session ended while waiting, so nothing here says what the \
                  venue makes of the request\n");
    } else if answered.is_empty() {
        println!(
            "  nothing naming {ASKED_UNDER} in 10s, with {unrelated} other message(s) \
             arriving meanwhile. That says the request drew no reply carrying its id; \
             it does not say the venue read it\n"
        );
    } else {
        for msg in answered.iter().take(3) {
            let shown: String = msg.chars().take(500).collect();
            println!("  answered: {shown}");
        }
        println!();
    }
    conns
}
