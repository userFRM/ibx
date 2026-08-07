//! Contract detail lookup test phases.

use super::common::*;
use ibx::control::contracts;
use ibx::protocol::fix;
use ibx::protocol::fixcomp;
use ibx::protocol::connection::Frame;

pub(super) fn phase_contract_details(conns: Conns) -> Conns {
    println!("--- Phase 12: Contract Details Lookup (SPY, conId=756733) ---");

    // Step 1: Create HotLoop with real connections
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(event_tx), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    // Step 2: Send ControlCommand through the channel
    control_tx.send(ControlCommand::FetchContractDetails {
        req_id: 1200, con_id: 756733,
        symbol: String::new(), sec_type: String::new(),
        exchange: String::new(), currency: String::new(),
        filters: Default::default(),
    }).unwrap();
    // The same contract asked for by name. A definition asked for by id and one
    // asked for by name are answered from the same record, so any field that
    // arrives for one and not the other is this client's reading of the reply
    // rather than the venue withholding it.
    control_tx.send(ControlCommand::FetchContractDetails {
        req_id: 1201, con_id: 0,
        symbol: "SPY".to_string(), sec_type: "STK".to_string(),
        exchange: "SMART".to_string(), currency: "USD".to_string(),
        filters: Default::default(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    // Step 3: Wait for real server response via Event channel
    let mut contract: Option<contracts::ContractDefinition> = None;
    let mut by_name: Option<contracts::ContractDefinition> = None;
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline && (contract.is_none() || by_name.is_none()) {
        if let Ok(Event::ContractDetails { req_id, details }) = event_rx.recv_timeout(Duration::from_millis(100)) {
            println!(
                "  <- details req_id={} con_id={} long_name={:?} valid_exchanges={}",
                req_id, details.con_id, details.long_name, details.valid_exchanges.len(),
            );
            match req_id {
                1200 => contract = Some(*details),
                1201 if details.con_id == 756733 => by_name = Some(*details),
                _ => {}
            }
        }
    }

    // Step 4: Verify SPECIFIC VALUES
    let def = contract.expect("No contract details received for SPY (756733)");
    assert_eq!(def.con_id, 756733);
    assert_eq!(def.symbol, "SPY");
    assert_eq!(def.sec_type, contracts::SecurityType::Stock);
    assert_eq!(def.currency, "USD");
    println!(
        "  by id:   long_name={:?} class={:?} primary={:?} valid_exchanges={} min_tick={}",
        def.long_name, def.trading_class, def.primary_exchange,
        def.valid_exchanges.len(), def.min_tick,
    );
    // A contract asked for by id and the same contract asked for by name are
    // the same contract. They were not: one arrived whole and the other lost
    // the fields the message states once, which is invisible unless the two are
    // put side by side.
    let named = by_name.as_ref().expect("the same contract asked for by name did not answer");
    println!(
        "  by name: long_name={:?} class={:?} primary={:?} valid_exchanges={} min_tick={}",
        named.long_name, named.trading_class, named.primary_exchange,
        named.valid_exchanges.len(), named.min_tick,
    );
    assert_eq!(def.long_name, named.long_name, "the same contract, two ways of asking");
    assert_eq!(def.primary_exchange, named.primary_exchange, "the same contract, two ways of asking");
    assert_eq!(
        def.valid_exchanges.len(), named.valid_exchanges.len(),
        "the same contract, two ways of asking",
    );
    assert!(
        !def.long_name.is_empty(),
        "the definition carries no long name: {def:?}",
    );
    assert!(!def.valid_exchanges.is_empty(), "Valid exchanges should not be empty");
    assert!(def.valid_exchanges.contains(&"SMART".to_string()), "SMART should be in valid exchanges");
    assert!(def.min_tick > 0.0, "Min tick should be positive");
    println!("  {} ({}) conId={}", def.symbol, def.long_name, def.con_id);
    println!("  SecType={:?} Currency={} MinTick={}", def.sec_type, def.currency, def.min_tick);

    // Step 5: Clean up
    let conns = shutdown_and_reclaim(&control_tx, join, account_id);
    println!("  PASS\n");
    conns
}

pub(super) fn phase_contract_details_by_symbol(conns: Conns) -> Conns {
    println!("--- Phase 78: Contract Details by Symbol Search (AAPL) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(event_tx), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    // Send by symbol (con_id=0 triggers symbol-based lookup)
    control_tx.send(ControlCommand::FetchContractDetails {
        req_id: 7800, con_id: 0,
        symbol: "AAPL".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), currency: "USD".into(),
        filters: Default::default(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let mut contract: Option<contracts::ContractDefinition> = None;
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline && contract.is_none() {
        if let Ok(Event::ContractDetails { req_id, details }) = event_rx.recv_timeout(Duration::from_millis(100))
            && req_id == 7800 { contract = Some(*details); }
    }

    let def = contract.expect("No contract details received for AAPL by symbol search");
    assert_eq!(def.symbol, "AAPL");
    assert!(def.con_id > 0, "conId should be positive");
    assert_eq!(def.sec_type, contracts::SecurityType::Stock);
    assert_eq!(def.currency, "USD");
    assert!(!def.long_name.is_empty(), "Long name should not be empty");
    assert!(def.min_tick > 0.0, "Min tick should be positive");
    println!("  {} ({}) conId={} MinTick={}", def.symbol, def.long_name, def.con_id, def.min_tick);

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);
    println!("  PASS\n");
    conns
}

pub(super) fn phase_trading_hours(conns: &mut Conns) {
    println!("--- Phase 80: Trading Hours (schedule subscription, AAPL) ---");

    let now = ibx::gateway::chrono_free_timestamp();
    if let Err(e) = conns.farm.send_fixcomp(&[
        (fix::TAG_MSG_TYPE, "V"),
        (fix::TAG_SENDING_TIME, &now),
        (263, "1"), (146, "1"), (262, "sched_test"),
        (6008, "265598"), (207, "BEST"), (167, "CS"),
        (264, "442"), (6088, "Socket"), (9830, "1"), (9839, "1"),
    ]) {
        println!("  SKIP: farm subscribe failed: {e}\n");
        return;
    }
    println!("  Subscribed AAPL on farm, listening on CCP for schedule");

    let mut schedule: Option<contracts::ContractSchedule> = None;
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline && schedule.is_none() {
        if conns.farm.try_recv().is_ok() { conns.farm.extract_frames(); }
        match conns.ccp.try_recv() {
            Ok(0) => { std::thread::sleep(Duration::from_millis(50)); continue; }
            Err(e) => { println!("  CCP recv error: {e}"); break; }
            Ok(_) => {}
        }
        for frame in conns.ccp.extract_frames() {
            let messages = match frame {
                Frame::FixComp(raw) => { let Some(u) = conns.ccp.unsign(&raw) else { continue }; fixcomp::fixcomp_decompress(&u).unwrap_or_default() }
                Frame::Fix(raw) => vec![raw],
                _ => continue,
            };
            for msg in messages {
                if let Some(sched) = contracts::parse_schedule_response(&msg) {
                    println!("  Schedule: tz={} trading={} liquid={}", sched.timezone, sched.trading_hours.len(), sched.liquid_hours.len());
                    schedule = Some(sched);
                }
            }
        }
    }

    let now2 = ibx::gateway::chrono_free_timestamp();
    let _ = conns.farm.send_fixcomp(&[
        (fix::TAG_MSG_TYPE, "V"), (fix::TAG_SENDING_TIME, &now2),
        (263, "2"), (146, "1"), (262, "sched_test"),
        (6008, "265598"), (207, "BEST"), (167, "CS"),
        (264, "442"), (6088, "Socket"), (9830, "1"), (9839, "1"),
    ]);

    if schedule.is_none() {
        lookup_returned_nothing("no trading schedule came back");
    }
    let sched = schedule.unwrap();
    assert!(!sched.timezone.is_empty());
    assert!(!sched.trading_hours.is_empty());
    assert!(!sched.liquid_hours.is_empty());
    assert!(sched.liquid_hours.len() <= sched.trading_hours.len());
    println!("  PASS\n");
}

pub(super) fn phase_matching_symbols(conns: Conns) -> Conns {
    println!("--- Phase 81: Matching Symbols Search (pattern=\"SPY\") ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::FetchMatchingSymbols {
        req_id: 8100, pattern: "SPY".into(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut matches: Option<Vec<contracts::SymbolMatch>> = None;

    while Instant::now() < deadline && matches.is_none() {
        let results = shared.reference.drain_matching_symbols();
        for (req_id, m) in results {
            if req_id == 8100 {
                matches = Some(m);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    let m = matches.expect("No matching symbols response received for 'SPY'");
    assert!(!m.is_empty(), "Should have at least one match for 'SPY'");
    println!("  {} matches found", m.len());
    let spy = m.iter().find(|s| s.symbol == "SPY" && s.sec_type == contracts::SecurityType::Stock && s.currency == "USD");
    if let Some(spy) = spy {
        assert_eq!(spy.con_id, 756733);
        println!("  SPY: conId={} exchange={} desc={}", spy.con_id, spy.primary_exchange, spy.description);
    } else {
        println!("  WARNING: SPY STK not found in matches");
    }
    println!("  PASS\n");
    conns
}

pub(super) fn phase_market_rule_id(conns: Conns) -> Conns {
    println!("--- Phase 84: Market Rule ID (SPY, tag 6031) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(event_tx), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::FetchContractDetails {
        req_id: 8400, con_id: 756733,
        symbol: String::new(), sec_type: String::new(),
        exchange: String::new(), currency: String::new(),
        filters: Default::default(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let mut contract: Option<contracts::ContractDefinition> = None;
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline && contract.is_none() {
        if let Ok(Event::ContractDetails { req_id, details }) = event_rx.recv_timeout(Duration::from_millis(100))
            && req_id == 8400 { contract = Some(*details); }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if contract.is_none() {
        lookup_returned_nothing("no contract details came back");
    }
    let def = contract.unwrap();
    println!("  market_rule_id={:?} min_tick={}", def.market_rule_id, def.min_tick);
    assert!(def.market_rule_id.is_some(), "SPY should have a market rule ID (tag 6031)");
    assert!(def.market_rule_id.unwrap() > 0);
    println!("  PASS\n");
    conns
}

// ─── Phase 125: Matching Symbols via ControlCommand channel ───

pub(super) fn phase_matching_symbols_channel(conns: Conns) -> Conns {
    println!("--- Phase 125: Matching Symbols via Channel (pattern=\"AAPL\") ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, _event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(event_tx), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::FetchMatchingSymbols {
        req_id: 2001, pattern: "AAPL".to_string(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_matches = false;
    let mut match_count = 0usize;

    while Instant::now() < deadline {
        let results = shared.reference.drain_matching_symbols();
        for (req_id, matches) in &results {
            if *req_id == 2001 {
                match_count = matches.len();
                println!("  {match_count} matches for 'AAPL'");
                for m in matches.iter().take(3) {
                    println!("    {} ({:?}) conId={} exchange={}", m.symbol, m.sec_type, m.con_id, m.primary_exchange);
                }
                got_matches = true;
            }
        }
        if got_matches { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if !got_matches {
        lookup_returned_nothing("no matching-symbols answer came back");
    }
    assert!(match_count > 0, "Should have at least one match for 'AAPL'");
    println!("  PASS\n");
    conns
}

pub(super) fn phase_contract_details_channel(conns: Conns) -> Conns {
    println!("--- Phase 86: Contract Details via Event Channel (SPY) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(event_tx), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::FetchContractDetails {
        req_id: 1001, con_id: 756733,
        symbol: String::new(), sec_type: String::new(),
        exchange: String::new(), currency: String::new(),
        filters: Default::default(),
    }).unwrap();
    let join = run_hot_loop(hot_loop);

    // A request in flight when the transport drops is answered by nobody. The
    // engine rebuilds the connection underneath, but the question was asked on
    // the old one, so a client that wants an answer asks again — once, since a
    // connection that keeps dropping is a real fault and should read as one.
    let mut reasked = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut got_details = false;
    let mut got_end = false;

    while Instant::now() < deadline && !got_details {
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::ContractDetails { req_id, details }) => {
                if req_id == 1001 {
                    println!("  ContractDetails: {} ({}) conId={}", details.symbol, details.long_name, details.con_id);
                    assert_eq!(details.con_id, 756733);
                    assert_eq!(details.symbol, "SPY");
                    got_details = true;
                }
            }
            Ok(Event::ContractDetailsEnd(req_id)) => {
                if req_id == 1001 { got_end = true; }
            }
            Ok(Event::Disconnected) if !reasked => {
                reasked = true;
                std::thread::sleep(Duration::from_secs(3));
                let _ = control_tx.send(ControlCommand::FetchContractDetails {
                    req_id: 1001, con_id: 756733,
                    symbol: String::new(), sec_type: String::new(),
                    exchange: String::new(), currency: String::new(),
                    filters: Default::default(),
                });
            }
            _ => {}
        }
    }

    // Wait briefly for ContractDetailsEnd if not yet received
    if got_details && !got_end {
        let end_deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end_deadline {
            if let Ok(Event::ContractDetailsEnd(req_id)) = event_rx.recv_timeout(Duration::from_millis(100))
                && req_id == 1001 { got_end = true; break; }
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    assert!(got_details, "Event::ContractDetails not received for SPY");
    if got_end {
        println!("  ContractDetailsEnd received");
    } else {
        println!("  ContractDetailsEnd not received (single-conId request — non-fatal)");
    }
    println!("  PASS\n");
    conns
}
