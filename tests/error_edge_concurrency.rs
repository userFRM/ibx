//! Error path, edge case, and concurrency tests for ibx.
//!
//! Validates that the library handles bad inputs, boundary conditions,
//! And concurrent access without panics or data races.

use std::sync::Arc;
use std::thread;

use ibx::api::client::{EClient, Contract, Order};
use ibx::api::wrapper::tests::RecordingWrapper;
use ibx::bridge::SharedState;
use ibx::control::historical::HistoricalResponse;
use ibx::engine::hot_loop::HotLoop;
use ibx::protocol::fix;
use ibx::types::*;

fn test_client() -> (EClient, std::sync::mpsc::Receiver<ControlCommand>, Arc<SharedState>) {
    let shared = Arc::new(SharedState::new());
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let handle = thread::spawn(|| {});
    let client = EClient::from_parts(shared.clone(), tx, handle, "DU123".into());
    // Pre-seed instrument mappings so tests don't need a running hot loop.
    client.seed_instrument(756733, 0);
    client.seed_instrument(0, 1);
    (client, rx, shared)
}

fn spy() -> Contract {
    Contract {
        con_id: 756733, symbol: "SPY".into(), exchange: "SMART".into(),
        sec_type: "STK".into(),
        ..Default::default()
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  ERROR PATHS — place_order
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn place_order_invalid_action_returns_error() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "INVALID".into(), total_quantity: 100.0,
        order_type: "MKT".into(), ..Default::default()
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("Invalid action"));
}

#[test]
fn place_order_empty_action_returns_error() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: String::new(), total_quantity: 100.0,
        order_type: "MKT".into(), ..Default::default()
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
}

#[test]
fn place_order_unsupported_order_type_returns_error() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "NONSENSE".into(), ..Default::default()
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("Unsupported order type"));
}

/// An algorithm this client does not model is carried to the venue, not
/// refused here.
///
/// This asserted the opposite, and the opposite was wrong: a caller could use
/// only the handful of strategies this client parses, while the venue states
/// which ones the account may use and refuses the rest itself. The reference
/// client forwards these without reading them.
#[test]
fn place_order_with_an_unmodelled_algo_is_sent() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 150.0,
        algo_strategy: "Accumulate/Distribute".into(),
        ..Default::default()
    };
    assert!(client.place_order(1, &spy(), &order).is_ok(), "carried, not refused");

    // An order this client does find wrong is still refused: the algorithm is
    // one it models and the parameter is not a number.
    let bad = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 150.0,
        algo_strategy: "vwap".into(),
        algo_params: vec![ibx::api::types::TagValue {
            tag: "maxPctVol".into(), value: "not a number".into(),
        }],
        ..Default::default()
    };
    assert!(client.place_order(2, &spy(), &bad).is_err(), "read, and found wrong");
}

#[test]
fn place_order_zero_con_id_asks_the_venue_to_name_it() {
    // An order names its contract by the venue's id. A contract stating only a
    // symbol is looked up first, and this test holds that the lookup goes out.
    // It cannot hold that the order is placed, because no venue is answering.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let contract = Contract {
        con_id: 0, symbol: "TEST".into(), exchange: "SMART".into(),
        ..Default::default()
    };
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "MKT".into(), ..Default::default()
    };
    let refused = client.place_order(1, &contract, &order)
        .expect_err("with nothing to answer it, the caller is told so");
    // Nothing answered, which is not the same as the venue answering that it
    // has no definition. The refusal says so under its own number rather than
    // borrowing one the venue never sent.
    assert_eq!(refused.code, ibx::api::error_codes::Refusal::NO_ANSWER);
    let asked = rx.try_iter().any(|cmd| matches!(
        cmd, ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { ref symbol, .. }, .. } if symbol == "TEST"
    ));
    assert!(asked, "the venue was asked to name the contract");
}

// ═══════════════════════════════════════════════════════════════════════
//  ERROR PATHS — cancel operations on non-existent targets
// ═══════════════════════════════════════════════════════════════════════

/// Withdrawing something this client does not hold is answered, not waved
/// through.
///
/// Said nothing, the withdrawal reads exactly like one that worked, and a
/// caller whose record disagrees with this client's has no way to learn it.
#[test]
fn cancel_mkt_data_under_a_number_that_holds_nothing_says_so() {
    let (client, rx, _shared) = test_client();
    let refused = client.cancel_mkt_data(999);
    assert!(
        refused.as_ref().is_err_and(|why| why.code == 300),
        "nothing is being watched under that number: {refused:?}",
    );
    assert!(rx.try_recv().is_err(), "and nothing was asked of the engine for it");
}

#[test]
fn cancel_tick_by_tick_under_a_number_that_holds_nothing_says_so() {
    let (client, rx, _shared) = test_client();
    let refused = client.cancel_tick_by_tick_data(999);
    assert!(
        refused.as_ref().is_err_and(|why| why.code == 300),
        "nothing is held under that number: {refused:?}",
    );
    assert!(rx.try_recv().is_err(), "and nothing was asked of the engine for it");
}

/// A withdrawal naming an order this client is not working is answered rather
/// than sent under a name this client invented.
///
/// It used to go to the venue regardless, with the order name composed here,
/// so the caller learnt from the venue rather than from the number. Read after
/// the connect-time replay of the account's working set, so an order carried
/// over from a previous session is never refused.
#[test]
fn cancel_order_naming_nothing_is_answered_rather_than_sent() {
    let (client, rx, _shared) = test_client();
    let refused = client.cancel_order(999999, "");
    assert!(
        refused.as_ref().is_err_and(|why| why.code == 135),
        "no order is working under that number: {refused:?}",
    );
    assert!(rx.try_recv().is_err(), "and nothing was sent under it");
}

#[test]
fn req_global_cancel_no_instruments_no_commands() {
    let (client, rx, _shared) = test_client();
    client.req_global_cancel().unwrap();
    assert!(rx.try_recv().is_err());
}

// ═══════════════════════════════════════════════════════════════════════
//  ERROR PATHS — disconnect during activity
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn disconnect_during_active_subscription() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);

    // Subscribe
    let _ = client.req_mkt_data(1, &spy(), "", false, false);
    while rx.try_recv().is_ok() {}

    // Disconnect
    client.disconnect();
    assert!(!client.is_connected());

    // Push quote after disconnect — process_msgs should still work (no panic)
    let q = Quote {
        bid: 150 * PRICE_SCALE,
        ..Quote::default()
    };
    shared.market.push_quote(0, &q);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    // Might or might not dispatch depending on mapping — key is no panic
}

#[test]
fn disconnect_during_pending_order_uncertain_status() {
    let (client, _rx, shared) = test_client();

    // The order was pending at the disconnect
    shared.orders.push_order_update(OrderUpdate {
        order_id: 50, instrument: 0, status: OrderStatus::Uncertain,
        filled_qty: 0.0, remaining_qty: 100.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("order_status:50:Unknown")));
}

// ═══════════════════════════════════════════════════════════════════════
//  ERROR PATHS — fill dedup (engine level)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn fill_dedup_duplicate_exec_id_no_double_position() {
    let shared = Arc::new(SharedState::new());
    let mut engine = HotLoop::new(shared.clone(), None, None);
    engine.context_mut().register_instrument(265598);

    engine.context_mut().insert_order(ibx::types::Order {
        order_id: 70, instrument: 0, side: Side::Buy,
        price: 150 * PRICE_SCALE, qty: 100 * QTY_SCALE * QTY_SCALE, filled: 0,
        status: OrderStatus::Submitted,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });

    // Same ExecID sent twice
    let msg = fix::fix_build(&[
        (35, "8"), (11, "70"), (17, "EXEC_DUP1"),
        (39, "2"), (150, "F"),
        (31, "150.0"), (32, "100"), (151, "0"),
    ], 1);
    engine.inject_ccp_message(&msg);
    engine.inject_ccp_message(&msg);

    // Only one fill
    assert_eq!(shared.orders.drain_fills().len(), 1);
    assert_eq!(engine.context_mut().position(0), 100.0);
}

#[test]
fn fill_dedup_different_exec_ids_both_count() {
    let shared = Arc::new(SharedState::new());
    let mut engine = HotLoop::new(shared.clone(), None, None);
    engine.context_mut().register_instrument(265598);

    engine.context_mut().insert_order(ibx::types::Order {
        order_id: 71, instrument: 0, side: Side::Buy,
        price: 150 * PRICE_SCALE, qty: 200 * QTY_SCALE * QTY_SCALE, filled: 0,
        status: OrderStatus::Submitted,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });

    let msg_a = fix::fix_build(&[
        (35, "8"), (11, "71"), (17, "EXEC_A1"),
        (39, "1"), (150, "1"),
        (31, "150.0"), (32, "100"), (151, "100"),
    ], 1);
    let msg_b = fix::fix_build(&[
        (35, "8"), (11, "71"), (17, "EXEC_B1"),
        (39, "2"), (150, "F"),
        (31, "150.0"), (32, "100"), (151, "0"),
    ], 2);
    engine.inject_ccp_message(&msg_a);
    engine.inject_ccp_message(&msg_b);

    assert_eq!(shared.orders.drain_fills().len(), 2);
    assert_eq!(engine.context_mut().position(0), 200.0);
}

// ═══════════════════════════════════════════════════════════════════════
//  EDGE CASES — the instrument table's edge
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn every_slot_in_the_table_can_be_taken() {
    let shared = Arc::new(SharedState::new());
    let mut engine = HotLoop::new(shared.clone(), None, None);
    for i in 0..ibx::types::MAX_INSTRUMENTS {
        let id = engine.context_mut().register_instrument(i as i64 + 1000);
        assert_eq!(id, i as u32);
    }
}

#[test]
#[should_panic(expected = "too many instruments")]
fn one_past_the_table_is_refused() {
    let shared = Arc::new(SharedState::new());
    let mut engine = HotLoop::new(shared.clone(), None, None);
    for i in 0..ibx::types::MAX_INSTRUMENTS + 1 {
        engine.context_mut().register_instrument(i as i64 + 1000);
    }
}

#[test]
fn register_same_instrument_twice_returns_same_id() {
    let shared = Arc::new(SharedState::new());
    let mut engine = HotLoop::new(shared.clone(), None, None);
    let id1 = engine.context_mut().register_instrument(265598);
    let id2 = engine.context_mut().register_instrument(265598);
    assert_eq!(id1, id2);
}

// ═══════════════════════════════════════════════════════════════════════
//  EDGE CASES — quote values
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn zero_price_quote_dispatches_correctly() {
    let (client, _rx, shared) = test_client();
    client.map_req_instrument(1, 0);

    let q = Quote { bid: 0, ask: 0, last: 0, bid_size: 0, ask_size: 0,
        last_size: 0, high: 0, low: 0, volume: 0, close: 0, open: 0, timestamp_ns: 0,
        bid_exch_mask: 0, ask_exch_mask: 0, last_exch_mask: 0, halted: 0 };
    shared.market.push_quote(0, &q);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    // Zero quotes should not cause panic — they just dispatch as 0.0
    // (Whether they dispatch depends on change detection — default is [0;12])
}

#[test]
fn crossed_market_quote_dispatches() {
    let (client, _rx, shared) = test_client();
    client.map_req_instrument(1, 0);

    // Crossed: ask < bid (happens during fast markets)
    let q = Quote {
        bid: 151 * PRICE_SCALE, ask: 150 * PRICE_SCALE,
        last: 0, bid_size: 0, ask_size: 0, last_size: 0,
        high: 0, low: 0, volume: 0, close: 0, open: 0, timestamp_ns: 0,
        bid_exch_mask: 0, ask_exch_mask: 0, last_exch_mask: 0,
        halted: 0,
    };
    shared.market.push_quote(0, &q);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    // Should dispatch normally — no validation (that's user's job)
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:1:151")));
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:2:150")));
}

#[test]
fn negative_price_quote_dispatches() {
    let (client, _rx, shared) = test_client();
    client.map_req_instrument(1, 0);

    // Negative prices (valid for some instruments like spreads)
    let q = Quote {
        bid: -5 * PRICE_SCALE, ask: -4 * PRICE_SCALE,
        last: 0, bid_size: 0, ask_size: 0, last_size: 0,
        high: 0, low: 0, volume: 0, close: 0, open: 0, timestamp_ns: 0,
        bid_exch_mask: 0, ask_exch_mask: 0, last_exch_mask: 0,
        halted: 0,
    };
    shared.market.push_quote(0, &q);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:1:-5")));
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:2:-4")));
}

// ═══════════════════════════════════════════════════════════════════════
//  EDGE CASES — empty responses
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn empty_historical_data_response() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_historical_data(5, HistoricalResponse {
        query_id: String::new(), timezone: String::new(),
        bars: vec![], // empty
        is_complete: true,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    // Should just get historical_data_end, no bars
    assert!(!w.events.iter().any(|e| e.starts_with("historical_data:5:")));
    assert!(w.events.iter().any(|e| e == "historical_data_end:5"));
}

#[test]
fn empty_scanner_results() {
    use ibx::control::scanner::ScannerResult;
    let (client, _rx, shared) = test_client();
    shared.reference.push_scanner_data(3, ScannerResult {
        con_ids: vec![],
        entries: vec![],
        scan_time: "2026-03-13".into(),
        error_text: String::new(),
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(!w.events.iter().any(|e| e.starts_with("scanner_data:3:")));
    assert!(w.events.iter().any(|e| e == "scanner_data_end:3"));
}

#[test]
fn empty_matching_symbols() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_matching_symbols(8, vec![]);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "symbol_samples:8:0"));
}

#[test]
fn empty_historical_news() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_historical_news(4, vec![], true);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "historical_news_end:4:true"));
}

// ═══════════════════════════════════════════════════════════════════════
//  EDGE CASES — process_msgs idempotency
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_multiple_rapid_calls_no_duplicates() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 1, side: Side::Buy,
        price: PRICE_SCALE, qty: QTY_SCALE, remaining: 0,
        commission: 0, timestamp_ns: 0,
        cum_qty: QTY_SCALE, avg_price: PRICE_SCALE,
    });

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    let count1 = w.events.iter().filter(|e| e.starts_with("order_status:1:")).count();
    assert_eq!(count1, 1);

    // Second call — should have no fills
    w.events.clear();
    client.process_msgs(&mut w);
    let count2 = w.events.iter().filter(|e| e.starts_with("order_status:1:")).count();
    assert_eq!(count2, 0);
}

// ═══════════════════════════════════════════════════════════════════════
//  CONCURRENCY — SeqLock quote reads during writes
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn concurrent_seqlock_quote_read_write() {
    let shared = Arc::new(SharedState::new());
    let writer_shared = shared.clone();
    let reader_shared = shared.clone();

    let writer = thread::spawn(move || {
        for i in 0..10_000i64 {
            let q = Quote {
                bid: i * PRICE_SCALE,
                ask: (i + 1) * PRICE_SCALE,
                last: i * PRICE_SCALE,
                ..Quote::default()
            };
            writer_shared.market.push_quote(0, &q);
        }
    });

    let reader = thread::spawn(move || {
        for _ in 0..10_000 {
            let q = reader_shared.market.quote(0);
            // Consistency check: if bid is set, ask should be bid + PRICE_SCALE
            if q.bid > 0 {
                assert_eq!(q.ask, q.bid + PRICE_SCALE,
                    "SeqLock inconsistency: bid={}, ask={}", q.bid, q.ask);
            }
        }
    });

    writer.join().unwrap();
    reader.join().unwrap();
}

#[test]
fn concurrent_seqlock_multiple_readers() {
    let shared = Arc::new(SharedState::new());
    // Pre-write a known quote
    let q = Quote {
        bid: 100 * PRICE_SCALE,
        ask: 101 * PRICE_SCALE,
        ..Quote::default()
    };
    shared.market.push_quote(0, &q);

    let handles: Vec<_> = (0..4).map(|_| {
        let s = shared.clone();
        thread::spawn(move || {
            for _ in 0..5_000 {
                let q = s.market.quote(0);
                assert!(q.bid == 0 || q.bid == 100 * PRICE_SCALE);
            }
        })
    }).collect();

    for h in handles {
        h.join().unwrap();
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  CONCURRENCY — quote_by_instrument from multiple threads
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn concurrent_quote_by_instrument() {
    let shared = Arc::new(SharedState::new());
    let (tx, _rx) = std::sync::mpsc::sync_channel(4096);
    let handle = thread::spawn(|| {});
    let client = Arc::new(EClient::from_parts(shared.clone(), tx, handle, "DU123".into()));

    // Write quote
    let q = Quote {
        bid: 200 * PRICE_SCALE,
        ..Quote::default()
    };
    shared.market.push_quote(0, &q);

    let handles: Vec<_> = (0..4).map(|_| {
        let c = client.clone();
        thread::spawn(move || {
            for _ in 0..5_000 {
                let q = c.quote_by_instrument(0).expect("in-range id");
                assert!(q.bid == 0 || q.bid == 200 * PRICE_SCALE);
            }
        })
    }).collect();

    for h in handles {
        h.join().unwrap();
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  CONCURRENCY — disconnect while process_msgs runs
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn concurrent_disconnect_during_process_msgs() {
    let shared = Arc::new(SharedState::new());
    let (tx, _rx) = std::sync::mpsc::sync_channel(4096);
    let handle = thread::spawn(|| {});
    let client = Arc::new(EClient::from_parts(shared.clone(), tx, handle, "DU123".into()));

    // Push lots of data
    for i in 0..100 {
        shared.orders.push_fill(Fill {
            instrument: 0, order_id: i, side: Side::Buy,
            price: PRICE_SCALE, qty: QTY_SCALE, remaining: 0,
            commission: 0, timestamp_ns: 0,
            cum_qty: QTY_SCALE, avg_price: PRICE_SCALE,
        });
    }

    let client_process = client.clone();
    let process_thread = thread::spawn(move || {
        let mut w = RecordingWrapper::default();
        client_process.process_msgs(&mut w);
        w.events.len()
    });

    // Disconnect from main thread
    client.disconnect();

    // Should not panic
    let count = process_thread.join().unwrap();
    assert!(count > 0);
}

// ═══════════════════════════════════════════════════════════════════════
//  CONCURRENCY — rapid subscribe/unsubscribe
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn rapid_subscribe_unsubscribe_no_stale_state() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);

    // Answers registrations the way the engine does, and keeps what it was
    // sent. A client assembled from parts has no engine behind it, so without
    // this every subscribe waits out the registration timeout and fails, no
    // request is ever mapped to an instrument, and the stale-state assertion
    // at the end holds before the loop has run once.
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let engine = {
        let sent = Arc::clone(&sent);
        thread::spawn(move || {
            while let Ok(command) = rx.recv() {
                if let ControlCommand::Subscribe { reply_tx: Some(reply), .. } = &command {
                    let _ = reply.send(Ok(0));
                }
                sent.lock().unwrap().push(command);
            }
        })
    };

    const CYCLES: usize = 100;
    for _ in 0..CYCLES {
        client.req_mkt_data(1, &spy(), "", false, false).expect("the subscription was refused");
        client.cancel_mkt_data(1).unwrap();
    }


    // After all subscribe/unsubscribe cycles, mapping should be cleared
    let mut w = RecordingWrapper::default();
    let q = Quote {
        bid: 999 * PRICE_SCALE,
        ..Quote::default()
    };
    shared.market.push_quote(0, &q);
    client.process_msgs(&mut w);
    // No ticks should arrive since all subscriptions were cancelled
    let ticks: Vec<_> = w.events.iter().filter(|e| e.starts_with("tick_price:1:")).collect();
    assert!(ticks.is_empty(), "no ticks after final unsubscribe");

    // Counted once the channel is closed and the stub has drained it: read
    // before that, the last command of the loop is still in flight and the
    // count is one short of what was sent.
    drop(client);
    engine.join().expect("the engine stub panicked");

    let sent = sent.lock().unwrap();
    let subscribed = sent.iter().filter(|c| matches!(c, ControlCommand::Subscribe { .. })).count();
    let withdrawn = sent.iter().filter(|c| matches!(c, ControlCommand::Unsubscribe { .. })).count();
    assert_eq!(subscribed, CYCLES, "not every cycle subscribed");
    assert_eq!(withdrawn, CYCLES, "not every cycle withdrew what it subscribed");
}

// ═══════════════════════════════════════════════════════════════════════
//  CONCURRENCY — concurrent place_order and process_msgs
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn concurrent_place_order_and_process_msgs() {
    let shared = Arc::new(SharedState::new());
    shared.market.set_instrument_count(1);
    let (tx, _rx) = std::sync::mpsc::sync_channel(4096);
    let handle = thread::spawn(|| {});
    let client = Arc::new(EClient::from_parts(shared.clone(), tx, handle, "DU123".into()));

    // Thread A: process_msgs
    let client_a = client.clone();
    let shared_a = shared.clone();
    let process_handle = thread::spawn(move || {
        for i in 0..50 {
            shared_a.orders.push_fill(Fill {
                instrument: 0, order_id: i, side: Side::Buy,
                price: PRICE_SCALE, qty: QTY_SCALE, remaining: 0,
                commission: 0, timestamp_ns: 0,
                cum_qty: QTY_SCALE, avg_price: PRICE_SCALE,
            });
            let mut w = RecordingWrapper::default();
            client_a.process_msgs(&mut w);
        }
    });

    // Thread B: place_order
    let client_b = client.clone();
    let order_handle = thread::spawn(move || {
        for _ in 0..50 {
            let order = Order {
                action: "BUY".into(), total_quantity: 1.0,
                order_type: "MKT".into(), ..Default::default()
            };
            let _ = client_b.place_order(0, &spy(), &order);
        }
    });

    // Both should complete without panic or deadlock
    process_handle.join().unwrap();
    order_handle.join().unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
//  CONCURRENCY — concurrent account reads
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn concurrent_account_read_write() {
    let shared = Arc::new(SharedState::new());
    let writer_shared = shared.clone();

    let writer = thread::spawn(move || {
        for i in 0..5_000i64 {
            let a = AccountState {
                net_liquidation: i * PRICE_SCALE,
                buying_power: i * 2 * PRICE_SCALE,
                ..AccountState::default()
            };
            writer_shared.portfolio.set_account(&a);
        }
    });

    let reader = thread::spawn(move || {
        for _ in 0..5_000 {
            let a = shared.portfolio.account();
            // buying_power should always be 2x net_liquidation
            if a.net_liquidation > 0 {
                assert_eq!(a.buying_power, a.net_liquidation * 2,
                    "Account inconsistency: net_liq={}, buying_power={}",
                    a.net_liquidation, a.buying_power);
            }
        }
    });

    writer.join().unwrap();
    reader.join().unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
//  EDGE CASES — OrderBuffer
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn order_buffer_push_drain_cycle() {
    let mut buf = OrderBuffer::new();
    for i in 0..64 {
        buf.push(OrderRequest::SubmitEx { con_id: 0, order_id: i, instrument: 0, side: Side::Buy, qty: QTY_SCALE, kind: OrderKind::Market, tif: b'0', attrs: OrderAttrs::default() });
    }
    let drained: Vec<_> = buf.drain().collect();
    assert_eq!(drained.len(), 64);
    // After drain, buffer should be empty for reuse
    assert_eq!(buf.drain().count(), 0);
}

#[test]
fn order_buffer_multiple_drain_cycles() {
    let mut buf = OrderBuffer::new();
    for cycle in 0..5 {
        for i in 0..10 {
            buf.push(OrderRequest::SubmitEx { con_id: 0, order_id: cycle * 10 + i, instrument: 0, side: Side::Buy, qty: QTY_SCALE, kind: OrderKind::Market, tif: b'0', attrs: OrderAttrs::default() });
        }
        let drained: Vec<_> = buf.drain().collect();
        assert_eq!(drained.len(), 10);
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  EDGE CASES — SharedState drain idempotency
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn shared_state_all_drains_empty_after_first_call() {
    let ss = SharedState::new();

    // Push one item to each queue
    ss.orders.push_fill(Fill { instrument: 0, order_id: 1, side: Side::Buy,
        cum_qty: 0, avg_price: 0,
        price: PRICE_SCALE, qty: QTY_SCALE, remaining: 0, commission: 0, timestamp_ns: 0 });
    ss.orders.push_order_update(OrderUpdate { order_id: 1, instrument: 0,
        status: OrderStatus::Filled, filled_qty: 1.0, remaining_qty: 0.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0 });
    ss.orders.push_cancel_reject(CancelReject { order_id: 1, instrument: 0,
        reject_type: 1, reason_code: 0, answers_a_live_change: true, still_working: None, timestamp_ns: 0 });
    ss.market.push_tbt_trade(TbtTrade { req_id: 1, instrument: 0, price: PRICE_SCALE,
        size: 1, timestamp: 0, exchange: String::new(), conditions: String::new(),
        past_limit: false, unreported: false });
    ss.market.push_tbt_quote(TbtQuote { req_id: 1, instrument: 0, bid: PRICE_SCALE, ask: PRICE_SCALE,
        bid_size: 1, ask_size: 1, timestamp: 0,
        bid_past_low: false, ask_past_high: false });

    // First drain
    assert_eq!(ss.orders.drain_fills().len(), 1);
    assert_eq!(ss.orders.drain_order_updates().len(), 1);
    assert_eq!(ss.orders.drain_cancel_rejects().len(), 1);
    assert_eq!(ss.market.drain_tbt_trades().len(), 1);
    assert_eq!(ss.market.drain_tbt_quotes().len(), 1);

    // Second drain — all empty
    assert!(ss.orders.drain_fills().is_empty());
    assert!(ss.orders.drain_order_updates().is_empty());
    assert!(ss.orders.drain_cancel_rejects().is_empty());
    assert!(ss.market.drain_tbt_trades().is_empty());
    assert!(ss.market.drain_tbt_quotes().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
//  CONCURRENCY — concurrent drain from multiple threads
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn concurrent_drain_fills_no_duplicates() {
    let shared = Arc::new(SharedState::new());

    // Push 100 fills
    for i in 0..100 {
        shared.orders.push_fill(Fill {
            instrument: 0, order_id: i, side: Side::Buy,
            price: PRICE_SCALE, qty: QTY_SCALE, remaining: 0,
            commission: 0, timestamp_ns: 0,
            cum_qty: QTY_SCALE, avg_price: PRICE_SCALE,
        });
    }

    // Two threads race to drain
    let s1 = shared.clone();
    let s2 = shared.clone();

    let h1 = thread::spawn(move || s1.orders.drain_fills().len());
    let h2 = thread::spawn(move || s2.orders.drain_fills().len());

    let count1 = h1.join().unwrap();
    let count2 = h2.join().unwrap();

    // Total should be exactly 100 — no duplicates, no lost fills
    assert_eq!(count1 + count2, 100, "Total fills should be 100, got {count1} + {count2}");
}
