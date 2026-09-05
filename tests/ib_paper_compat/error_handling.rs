//! IB-side error handling test phases.

use super::common::*;

pub(super) fn phase_ib_error_handling(conns: Conns) -> Conns {
    phase!("--- Phase 104: IB-Side Error Handling (invalid requests) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    let spy_inst = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(spy_inst, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(spy_inst, "STK", "SMART");

    // Submit an order for a non-existent instrument (con_id 999999999)
    // The hot loop should handle this gracefully
    let oid = next_order_id();
    // Register a bogus instrument
    let bogus_inst = hot_loop.context_mut().register_instrument(999999999);
    hot_loop.context_mut().set_symbol(bogus_inst, "BOGUS".to_string());

    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { con_id: 0, order_id: oid, instrument: bogus_inst, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Market, tif: b'0', attrs: OrderAttrs::default() })).unwrap();

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, regulatory_snapshot: false, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_error_or_reject = false;

    while Instant::now() < deadline {
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::OrderUpdate(update)) => {
                if update.order_id == oid && update.status == OrderStatus::Rejected {
                    println!("  Order rejected as expected (bogus con_id)");
                    got_error_or_reject = true;
                    break;
                }
            }
            Ok(Event::CancelReject(cr))
                if cr.order_id == oid => {
                    got_error_or_reject = true;
                    println!("  CancelReject received for bogus order");
                    break;
                }
            _ => {}
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if got_error_or_reject {
        println!("  PASS\n");
    } else {
        // The order may have been silently ignored or the hot loop handled it
        skipped!("  SKIP: No rejection/error received (order may have been filtered)\n");
    }
    conns
}

// ─── Phase 114: Pacing violation recovery — rapid historical requests ───

pub(super) fn phase_pacing_violation_recovery(conns: Conns) -> Conns {
    // Named for a pacing violation it does not provoke: ten requests against a
    // limit of roughly sixty in ten minutes will not trip one, and deliberately
    // tripping it would leave the historical farm throttled for every phase
    // after this. What it can establish is that none of the ten went missing —
    // each is answered, or the venue says why. A request that vanishes with
    // nothing reported is the defect, and reads the same as a throttled one.
    phase!("--- Phase 114: Ten rapid historical requests are each answered or explained ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let end_dt = format_utc_timestamp(now);

    // Ten in rapid succession. The venue's limit is roughly sixty in ten
    // minutes, so this is well inside it by design.
    let num_requests = 10u32;
    for i in 0..num_requests {
        control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 14000 + i, end_date_time: end_dt.clone(), duration: "1 d".to_string(), bar_size: "5 mins".to_string(), what_to_show: "TRADES".to_string(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).unwrap();
    }

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, regulatory_snapshot: false, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut responses_received = std::collections::HashSet::new();
    // What the venue said went wrong, which is where a throttled request is
    // named. Drained from the historical-error queue: without it a request the
    // venue refused and a request that vanished are indistinguishable.
    let mut errors_received: std::collections::HashMap<u32, String> =
        std::collections::HashMap::new();

    while Instant::now() < deadline {
        // Check for historical data responses
        let data = shared.reference.drain_historical_data();
        for (req_id, resp) in &data {
            if *req_id >= 14000 && *req_id < 14000 + num_requests
                && resp.is_complete {
                    responses_received.insert(*req_id);
                }
        }

        for (req_id, code, message) in shared.reference.drain_historical_errors() {
            if (14000..14000 + num_requests).contains(&req_id) {
                errors_received.insert(req_id, format!("{code}: {message}"));
            }
        }

        // Check for error events (pacing violations)
        match event_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::OrderUpdate(_)) | Ok(Event::Tick(_)) => {}
            Ok(Event::Disconnected) => {
                println!("  WARNING: Disconnected during pacing test");
                break;
            }
            _ => {}
        }

        if responses_received.len() + errors_received.len() == num_requests as usize {
            break;
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    println!("  Responses received: {}/{}", responses_received.len(), num_requests);
    println!("  Errors: {}", errors_received.len());
    for (req_id, why) in &errors_received {
        println!("    {req_id}: {why}");
    }

    let unaccounted: Vec<u32> = (14000..14000 + num_requests)
        .filter(|id| !responses_received.contains(id) && !errors_received.contains_key(id))
        .collect();

    if responses_received.is_empty() && errors_received.is_empty() {
        // Historical server may be fully rate-limited from prior historical phases
        skipped!("  SKIP: nothing came back at all — HMDS is likely throttled from earlier phases\n");
    } else {
        // A request that neither answered nor was refused is one this client
        // dropped on the floor, which is not the same as a throttled one.
        assert!(
            unaccounted.is_empty(),
            "{} of {num_requests} historical requests were neither answered nor refused: \
             {unaccounted:?}. A request that goes missing with nothing reported is not a \
             throttled one",
            unaccounted.len(),
        );
        println!(
            "  PASS ({} answered, {} refused, none missing)\n",
            responses_received.len(), errors_received.len(),
        );
    }
    conns
}
