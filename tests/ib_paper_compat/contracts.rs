//! Contract detail lookup test phases.

use super::common::*;
use ibx::control::contracts;

pub(super) fn phase_contract_details(conns: Conns) -> Conns {
    println!("--- Phase 12: Contract Details Lookup (SPY, conId=756733) ---");

    // Step 1: Create HotLoop with real connections
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    // Step 2: Send ControlCommand through the channel
    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 756733, symbol: String::new(), sec_type: "STK".into(), exchange: String::new(), currency: String::new(), ..Default::default() }, req_id: 1200, filters: Default::default() }).unwrap();
    // The same contract asked for by name. A definition asked for by id and one
    // asked for by name are answered from the same record, so any field that
    // arrives for one and not the other is this client's reading of the reply
    // rather than the venue withholding it.
    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 0, symbol: "SPY".to_string(), sec_type: "STK".to_string(), exchange: "SMART".to_string(), currency: "USD".to_string(), ..Default::default() }, req_id: 1201, filters: Default::default() }).unwrap();
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
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    // Send by symbol (con_id=0 triggers symbol-based lookup)
    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 0, symbol: "AAPL".into(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "USD".into(), ..Default::default() }, req_id: 7800, filters: Default::default() }).unwrap();
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

/// A caller asks what hours a contract trades.
///
/// The schedule is asked for against the definition and arrives paired with
/// it. Subscribing on the market-data connection and watching the trading
/// connection for a schedule to appear is a guess at a mechanism rather than
/// the one the client uses, so a phase written that way tests a
/// path nothing else takes, found nothing, and skipped — while the hours were
/// reaching callers correctly all along by the path they actually use.
///
/// What a caller can see is what is checked now: ask for the contract, and the
/// hours, the liquid hours and the time zone are on the answer.
pub(super) fn phase_trading_hours(conns: Conns) -> Conns {
    println!("--- Phase 80: Trading Hours (AAPL) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 265598, symbol: String::new(), sec_type: String::new(), exchange: String::new(), currency: String::new(), ..Default::default() }, req_id: 8000, filters: Default::default() }).unwrap();
    let join = run_hot_loop(hot_loop);

    let mut details: Option<contracts::ContractDefinition> = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && details.is_none() {
        if let Ok(Event::ContractDetails { req_id, details: d }) =
            event_rx.recv_timeout(Duration::from_millis(100))
            && req_id == 8000
        {
            details = Some(*d);
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    let Some(def) = details else {
        lookup_returned_nothing("no definition came back for AAPL, so no schedule with it");
    };

    println!(
        "  tz={:?} trading_hours={:?} liquid_hours={:?}",
        def.time_zone_id,
        def.trading_hours.as_deref().map(|h| h.len()),
        def.liquid_hours.as_deref().map(|h| h.len()),
    );

    // A schedule states its own time zone: hours without one cannot be placed
    // against a clock, which is the whole use a caller has for them.
    let zone = def.time_zone_id.as_deref().unwrap_or_default();
    assert!(!zone.is_empty(), "the schedule states no time zone");
    let trading = def.trading_hours.as_deref().unwrap_or_default();
    let liquid = def.liquid_hours.as_deref().unwrap_or_default();
    assert!(!trading.is_empty(), "the schedule states no trading hours");
    assert!(!liquid.is_empty(), "the schedule states no liquid hours");
    println!("  PASS\n");
    conns
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
    // Asking the venue for SPY and being handed a list without SPY in it is the
    // reply having been read wrong, not a quiet day. This warned and passed, so
    // a parser that dropped the symbol, the security type or the currency from
    // every row still reported that matching symbols worked.
    let spy = m.iter()
        .find(|s| s.symbol == "SPY" && s.sec_type == contracts::SecurityType::Stock && s.currency == "USD")
        .unwrap_or_else(|| panic!(
            "the venue matched {} symbols for \"SPY\" and the US-dollar SPY stock was not \
             among them: {:?}",
            m.len(),
            m.iter().map(|s| (&s.symbol, &s.sec_type, &s.currency)).collect::<Vec<_>>(),
        ));
    assert_eq!(spy.con_id, 756733);
    println!("  SPY: conId={} exchange={} desc={}", spy.con_id, spy.primary_exchange, spy.description);
    println!("  PASS\n");
    conns
}

pub(super) fn phase_market_rule_id(conns: Conns) -> Conns {
    println!("--- Phase 84: Market Rule ID (SPY, tag 6031) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 756733, symbol: String::new(), sec_type: "STK".into(), exchange: String::new(), currency: String::new(), ..Default::default() }, req_id: 8400, filters: Default::default() }).unwrap();
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
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
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
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 756733, symbol: String::new(), sec_type: "STK".into(), exchange: String::new(), currency: String::new(), ..Default::default() }, req_id: 1001, filters: Default::default() }).unwrap();
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
                let _ = control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 756733, symbol: String::new(), sec_type: "STK".into(), exchange: String::new(), currency: String::new(), ..Default::default() }, req_id: 1001, filters: Default::default() });
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

    // Nothing answers on a connection that went away, and a definition that did
    // not arrive because there was nothing to arrive on says nothing about
    // whether this client reads one.
    if !got_details && shared.take_connection_lost() {
        super::common::note_lost_session("a contract lookup after the session went away");
        println!("  SKIP: the connection was lost, so nothing could answer\n");
        return conns;
    }
    assert!(got_details, "Event::ContractDetails not received for SPY");
    // Every contract lookup ends with this callback, including one naming a
    // single contract id: the phase in `main.rs` that asks by contract id
    // requires it, on this same venue. Called non-fatal here, a parser that
    // dropped only the completion passed, and a caller waiting for the end of
    // the lookup would wait for ever.
    assert!(got_end, "ContractDetailsEnd never fired for the SPY lookup");
    println!("  ContractDetailsEnd received");
    println!("  PASS\n");
    conns
}
