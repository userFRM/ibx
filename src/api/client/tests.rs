use std::sync::Arc;

use super::*;
use crate::api::types::PRICE_SCALE_F;
use crate::api::wrapper::Wrapper;
use crate::api::wrapper::tests::RecordingWrapper;
use crate::bridge::SharedState;
use crate::control::historical::{HistoricalResponse, HistoricalBar, HeadTimestampResponse};
use crate::control::contracts::{ContractDefinition, SecurityType, SymbolMatch};
use crate::control::scanner::{ScannerEntry, ScannerResult};
use crate::control::news::NewsHeadline;
use crate::control::histogram::HistogramEntry;

/// Helper: create a test EClient backed by SharedState + channel.
fn test_client() -> (EClient, crossbeam_channel::Receiver<ControlCommand>, Arc<SharedState>) {
    let shared = Arc::new(SharedState::new());
    let (tx, rx) = crossbeam_channel::unbounded();
    let handle = std::thread::spawn(|| {});
    let client = EClient::from_parts(shared.clone(), tx, handle, "DU123".into());
    // Pre-seed SPY so find_or_register_instrument hits the fast path.
    client.core.con_id_to_instrument.lock().unwrap().insert(756733, 0);
    (client, rx, shared)
}

/// Helper: SPY contract.
fn spy() -> Contract {
    Contract { con_id: 756733, symbol: "SPY".into(), ..Default::default() }
}

/// Case name paired with the setter that gives an order the named attribute.
type OrderCase = (&'static str, fn(&mut Order));

// ═══════════════════════════════════════════════════════════════════
//  Algo parsing
// ═══════════════════════════════════════════════════════════════════

/// Re-placing a tracked id is a modify, and a stop order's price lives in
/// `aux_price`. Reading only `lmt_price` sent a limit price of zero for an
/// order that has no limit leg, which the gateway rejects outright.
#[test]
fn modifying_a_stop_carries_the_new_trigger() {
    let (client, rx, _shared) = test_client();
    let stop = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "STP".into(),
        aux_price: 600.0, tif: "DAY".into(), ..Default::default()
    };
    client.place_order(9201, &spy(), &stop).unwrap();
    rx.try_recv().expect("the submit");

    let moved = Order { aux_price: 610.0, ..stop };
    client.place_order(9201, &spy(), &moved).unwrap();

    match rx.try_recv().expect("the modify") {
        ControlCommand::Order(OrderRequest::Modify { stop_price, .. }) => assert_eq!(
            stop_price, (610.0 * PRICE_SCALE_F) as i64,
            "the new trigger must reach the request",
        ),
        other => panic!("expected a Modify, got {other:?}"),
    }
}
/// Nothing on an execution report carries a parent order id, so the engine
/// reports none. This client placed the order and was told the parent, so it
/// can answer where the engine cannot — and an order it did not place keeps
/// the engine's answer rather than borrowing someone else's.
#[test]
fn a_locally_placed_child_reports_the_parent_it_was_given() {
    let (client, rx, shared) = test_client();
    let child = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "LMT".into(),
        lmt_price: 110.0, tif: "DAY".into(), parent_id: 4242, ..Default::default()
    };
    client.place_order(9401, &spy(), &child).unwrap();
    while rx.try_recv().is_ok() {}

    shared.orders.push_order_update(OrderUpdate {
        order_id: 9401, instrument: 0, status: OrderStatus::Submitted,
        filled_qty: 0.0, remaining_qty: 1.0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    let status = w.events.iter().find(|e| e.starts_with("order_status:9401:"))
        .expect("the status was dispatched");
    assert!(status.contains(":9401:"), "{status}");
    assert_eq!(
        w.parent_ids.last().copied(), Some(4242),
        "the parent this client recorded is reported: {:?}", w.events,
    );

    // An order this client never placed keeps the engine's answer.
    shared.orders.push_order_update(OrderUpdate {
        order_id: 9999, instrument: 0, status: OrderStatus::Submitted,
        filled_qty: 0.0, remaining_qty: 1.0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    let mut w2 = RecordingWrapper::default();
    client.process_msgs(&mut w2);
    assert_eq!(w2.parent_ids.last().copied(), Some(0), "no parent is invented");
}

/// A fill emits its own order_status from a different branch, so the parent
/// has to be preferred there as well. Before this it reported zero on every
/// fill of a bracket child — the callback a caller is most likely to act on.
#[test]
fn a_fill_reports_the_parent_the_child_was_given() {
    let (client, rx, shared) = test_client();
    let child = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "LMT".into(),
        lmt_price: 110.0, tif: "DAY".into(), parent_id: 4242, ..Default::default()
    };
    client.place_order(9402, &spy(), &child).unwrap();
    while rx.try_recv().is_ok() {}

    shared.orders.push_fill(Fill {
        order_id: 9402, instrument: 0, side: Side::Sell, qty: 1, remaining: 0,
        price: 110 * crate::types::PRICE_SCALE, commission: 0, timestamp_ns: 0,
        cum_qty: 1, avg_price: 110 * crate::types::PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert_eq!(
        w.parent_ids.first().copied(), Some(4242),
        "the fill's order_status carries the recorded parent: {:?}", w.events,
    );
}

/// The margin preview reports its own status too, and hard-coded a zero
/// parent. A preview of a bracket child that disowns it is the same wrong
/// answer as the other two paths gave.
#[test]
fn a_what_if_preview_reports_the_parent_the_child_was_given() {
    let (client, rx, shared) = test_client();
    let child = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "LMT".into(),
        lmt_price: 110.0, tif: "DAY".into(), parent_id: 4242, what_if: true,
        ..Default::default()
    };
    client.place_order(9403, &spy(), &child).unwrap();
    while rx.try_recv().is_ok() {}

    shared.orders.push_what_if(WhatIfResponse {
        order_id: 9403, instrument: 0,
        init_margin_before: 0, maint_margin_before: 0, equity_with_loan_before: 0,
        init_margin_after: 0, maint_margin_after: 0, equity_with_loan_after: 0,
        commission: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert_eq!(
        w.parent_ids.first().copied(), Some(4242),
        "the preview carries the recorded parent: {:?}", w.events,
    );
}
/// The quantity reaches the wire through `as u32`, which truncates. Asking for
/// 1.5 shares sent an order for 1 and reported nothing — the fill, the status
/// and the position were all consistent with an order that was never placed.
#[test]
fn a_fractional_quantity_is_refused_rather_than_truncated() {
    let (client, rx, _shared) = test_client();
    let order = Order {
        action: "BUY".into(), total_quantity: 1.5, order_type: "MKT".into(),
        tif: "DAY".into(), ..Default::default()
    };
    let err = client.place_order(9101, &spy(), &order).expect_err("must be refused");
    assert!(err.to_string().contains("whole number"), "the error says why: {err}");
    assert!(rx.try_recv().is_err(), "and nothing reaches the wire");
}

/// A quantity that is not a number, or is negative, or overflows the wire
/// type, all reach `as u32` and become something the caller did not ask for.
#[test]
fn an_unusable_quantity_is_refused() {
    for (qty, expect) in [
        (f64::NAN, "finite"),
        (f64::INFINITY, "finite"),
        (-5.0, "negative"),
        (5e9, "too large"),
    ] {
        let (client, _rx, _shared) = test_client();
        let order = Order {
            action: "BUY".into(), total_quantity: qty, order_type: "MKT".into(),
            tif: "DAY".into(), ..Default::default()
        };
        let err = client.place_order(9102, &spy(), &order)
            .expect_err("must be refused").to_string();
        assert!(err.contains(expect), "quantity {qty}: expected {expect:?}, got: {err}");
    }
}

/// The boundaries, exactly. Each of these was reachable by a mutation that the
/// round-number cases above could not see: rejecting the valid maximum,
/// admitting one past it, admitting a small negative, admitting a fraction
/// near a half, and misclassifying negative infinity.
#[test]
fn the_quantity_boundaries_are_exact() {
    let place = |qty: f64| {
        let (client, _rx, _shared) = test_client();
        let order = Order {
            action: "BUY".into(), total_quantity: qty, order_type: "MKT".into(),
            tif: "DAY".into(), ..Default::default()
        };
        client.place_order(9601, &spy(), &order)
    };

    assert!(place(u32::MAX as f64).is_ok(), "the largest carryable quantity still places");
    assert!(place(u32::MAX as f64 + 1.0).is_err(), "one past it does not");
    assert!(place(-1.0).is_err(), "a small negative is refused, not just a large one");
    assert!(place(1.25).is_err(), "a fraction below a half is refused too");
    assert!(place(f64::NEG_INFINITY).is_err(), "negative infinity is not finite either");
}

/// A cash-quantity order states its size in currency and carries no shares, so
/// zero is only wrong when nothing else says how much to buy.
#[test]
fn zero_shares_is_refused_unless_the_order_is_cash_sized() {
    let (client, rx, _shared) = test_client();
    let bare = Order {
        action: "BUY".into(), total_quantity: 0.0, order_type: "MKT".into(),
        tif: "DAY".into(), ..Default::default()
    };
    assert!(
        client.place_order(9602, &spy(), &bare).is_err(),
        "zero shares with no cash quantity is not an order",
    );
    assert!(rx.try_recv().is_err(), "and nothing reaches the wire");

    let cash = Order {
        action: "BUY".into(), total_quantity: 0.0, order_type: "LMT".into(),
        lmt_price: 100.0, cash_qty: 1000.0, tif: "DAY".into(), ..Default::default()
    };
    client.place_order(9603, &spy(), &cash).expect("a cash-sized order still places");
}

/// Whole quantities are unaffected.
#[test]
fn a_whole_quantity_still_places() {
    let (client, rx, _shared) = test_client();
    let order = Order {
        action: "BUY".into(), total_quantity: 200.0, order_type: "MKT".into(),
        tif: "DAY".into(), ..Default::default()
    };
    client.place_order(9103, &spy(), &order).expect("a whole quantity places");
    assert!(rx.try_recv().is_ok(), "and reaches the wire");
}
/// A replace carries the order type, the limit price and the trigger — not the
/// peg offset, the trailing amount or the execution instruction. Sent for a
/// trailing stop it describes a pegged order with no offset, the gateway
/// rejects it, and the caller is left with no stop at all. Refusing keeps the
/// order they already have.
#[test]
fn a_type_the_replace_cannot_restate_is_not_modified() {
    for order_type in ["TRAIL", "TRAIL LIMIT", "REL", "PEG MID", "MIDPX", "SNAP MID"] {
        let (client, rx, _shared) = test_client();
        let submit = Order {
            action: "SELL".into(), total_quantity: 1.0, order_type: order_type.into(),
            aux_price: 1.0, trailing_percent: 0.0, tif: "DAY".into(), ..Default::default()
        };
        // Submitting is fine; it is the replace that cannot express it.
        let _ = client.place_order(9201, &spy(), &submit);
        while rx.try_recv().is_ok() {}

        // Skipping when tracking did not happen would let this pass without
        // testing anything, which is how the modify gate went unnoticed.
        assert!(
            client.core.is_order_tracked(9201),
            "{order_type} must submit and be tracked, or the refusal below proves nothing",
        );
        let err = client.place_order(9201, &spy(), &submit)
            .expect_err("modifying it must be refused");
        assert!(err.contains("cannot be modified"), "{order_type}: {err}");
        assert!(rx.try_recv().is_err(), "{order_type}: nothing reaches the wire");
    }
}

/// The order type alone does not decide this. An adaptive or algo order is an
/// ordinary LMT defined by its algo tags; an adjustable stop is an ordinary STP
/// defined by its conversion; a conditional order rides submit-only tags. A
/// replace states none of those, so each is destroyed by one just as surely as
/// a trailing stop is — and each would have passed a gate that looked only at
/// the type.
#[test]
fn an_order_defined_by_more_than_its_type_is_not_modified() {
    let cases: Vec<OrderCase> = vec![
        ("adaptive", |o| o.algo_strategy = "Adaptive".into()),
        ("algo", |o| o.algo_strategy = "Vwap".into()),
        ("adjustable stop", |o| o.adjusted_order_type = "TRAIL".into()),
        ("conditional", |o| o.conditions.push(
            crate::types::OrderCondition::Time { time: "20260311-09:30:00".into(), is_more: true },
        )),
        // Every attribute below rides a tag the replace does not carry, so a
        // modify would state the order without it (ibx#248). The bracket links
        // are the costly pair: a child sent without its parent or OCA group
        // rests alone, and a fill on the sibling no longer cancels it.
        ("bracket child", |o| o.parent_id = 4242),
        ("OCA member", |o| o.oca_group = "bracket_1".into()),
        ("good-till expiry", |o| o.good_till_date = "20260311 16:00:00".into()),
        ("good-after time", |o| o.good_after_time = "20260311 09:30:00".into()),
        ("iceberg", |o| o.display_size = 100),
        ("minimum quantity", |o| o.min_qty = 50),
        ("discretionary", |o| o.discretionary_amt = 0.05),
        ("sweep to fill", |o| o.sweep_to_fill = true),
        ("trigger method", |o| o.trigger_method = 2),
    ];
    for (name, set) in cases {
        let (client, rx, _shared) = test_client();
        let mut order = Order {
            action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
            lmt_price: 100.0, tif: "DAY".into(), ..Default::default()
        };
        set(&mut order);
        let _ = client.place_order(9301, &spy(), &order);
        while rx.try_recv().is_ok() {}

        assert!(
            client.core.is_order_tracked(9301),
            "{name} must submit and be tracked, or the refusal below proves nothing",
        );
        let err = client.place_order(9301, &spy(), &order)
            .expect_err("modifying it must be refused");
        assert!(err.contains("cannot be modified"), "{name}: {err}");
        assert!(rx.try_recv().is_err(), "{name}: nothing reaches the wire");
    }
}

/// Limit-if-touched is submitted as `LT` but tracked under a byte the replace
/// renders as `K`, which is market-to-limit here — so a replace would describe
/// a different order type entirely.
#[test]
fn a_limit_if_touched_is_not_modified() {
    let (client, rx, _shared) = test_client();
    let order = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "LIT".into(),
        lmt_price: 100.0, aux_price: 101.0, tif: "DAY".into(), ..Default::default()
    };
    let _ = client.place_order(9302, &spy(), &order);
    while rx.try_recv().is_ok() {}

    assert!(
        client.core.is_order_tracked(9302),
        "the LIT must submit and be tracked, or the refusal below proves nothing",
    );
    let err = client.place_order(9302, &spy(), &order)
        .expect_err("a LIT modify must be refused");
    assert!(err.contains("cannot be modified"), "{err}");
}

/// The refusal has to read the order the caller is asking for, not only the one
/// on the book. A modify that *adds* a bracket link or an OCA group states an
/// order that has neither — so a gate looking only at the resting record lets
/// the attribute through on the very message that was supposed to carry it.
#[test]
fn an_attribute_added_by_the_modify_is_refused_too() {
    let cases: Vec<OrderCase> = vec![
        ("bracket child", |o| o.parent_id = 4242),
        ("OCA member", |o| o.oca_group = "bracket_1".into()),
        ("iceberg", |o| o.display_size = 100),
        ("all-or-none", |o| o.all_or_none = true),
    ];
    for (name, set) in cases {
        let (client, rx, _shared) = test_client();
        let plain = Order {
            action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
            lmt_price: 100.0, tif: "DAY".into(), ..Default::default()
        };
        client.place_order(9303, &spy(), &plain).expect("a plain limit submits");
        while rx.try_recv().is_ok() {}

        let mut attributed = plain.clone();
        set(&mut attributed);
        let err = client.place_order(9303, &spy(), &attributed)
            .expect_err("adding it by modify must be refused");
        assert!(err.contains("cannot be modified"), "{name}: {err}");
        assert!(rx.try_recv().is_err(), "{name}: nothing reaches the wire");
    }
}

/// Every allowed type must still modify, not just the one. Excluding any of
/// them costs a working modify, and only `LMT` was covered.
#[test]
fn every_restatable_type_still_modifies() {
    for (order_type, lmt, aux) in [
        ("MKT", 0.0, 0.0),
        ("LMT", 100.0, 0.0),
        ("STP", 0.0, 90.0),
        ("STP LMT", 100.0, 90.0),
        ("MOC", 0.0, 0.0),
        ("LOC", 100.0, 0.0),
        ("MIT", 0.0, 90.0),
        ("STP PRT", 0.0, 90.0),
        // Regression guard: these three were modifiable before the gate and
        // the replace renders the same byte they were submitted under.
        ("MTL", 0.0, 0.0),
        ("BOX TOP", 0.0, 0.0),
        ("MKT PRT", 0.0, 0.0),
    ] {
        let (client, rx, _shared) = test_client();
        let order = Order {
            action: "BUY".into(), total_quantity: 1.0, order_type: order_type.into(),
            lmt_price: lmt, aux_price: aux, tif: "DAY".into(), ..Default::default()
        };
        client.place_order(9701, &spy(), &order)
            .unwrap_or_else(|e| panic!("{order_type} must submit: {e}"));
        while rx.try_recv().is_ok() {}

        client.place_order(9701, &spy(), &order)
            .unwrap_or_else(|e| panic!("{order_type} must still modify: {e}"));
        match rx.try_recv().expect("the modify") {
            ControlCommand::Order(OrderRequest::Modify { .. }) => {}
            other => panic!("{order_type}: expected a Modify, got {other:?}"),
        }
    }
}

/// The decision is read from the order as it was submitted, not from the one
/// handed to the modify — a caller cannot make a trailing stop modifiable by
/// describing it as a limit on the way in.
#[test]
fn the_refusal_reads_the_tracked_order_not_the_incoming_one() {
    let (client, rx, _shared) = test_client();
    let trail = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "TRAIL".into(),
        aux_price: 1.0, tif: "DAY".into(), ..Default::default()
    };
    client.place_order(9702, &spy(), &trail).expect("the trailing stop submits");
    while rx.try_recv().is_ok() {}

    let disguised = Order {
        action: "SELL".into(), total_quantity: 2.0, order_type: "LMT".into(),
        lmt_price: 100.0, tif: "DAY".into(), ..Default::default()
    };
    let err = client.place_order(9702, &spy(), &disguised)
        .expect_err("the tracked type decides, so this is still refused");
    assert!(err.contains("cannot be modified"), "{err}");
    assert!(rx.try_recv().is_err(), "and nothing reaches the wire");
}

/// The ordinary types still modify.
#[test]
fn a_limit_order_still_modifies() {
    let (client, rx, _shared) = test_client();
    let order = Order {
        action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
        lmt_price: 100.0, tif: "DAY".into(), ..Default::default()
    };
    client.place_order(9202, &spy(), &order).unwrap();
    while rx.try_recv().is_ok() {}

    let moved = Order { lmt_price: 101.0, ..order };
    client.place_order(9202, &spy(), &moved).expect("a limit modify still goes through");
    match rx.try_recv().expect("the modify") {
        ControlCommand::Order(OrderRequest::Modify { .. }) => {}
        other => panic!("expected a Modify, got {other:?}"),
    }
}

#[test]
fn parse_algo_vwap() {
    let params = vec![
        TagValue { tag: "maxPctVol".into(), value: "0.1".into() },
        TagValue { tag: "startTime".into(), value: "09:30:00".into() },
        TagValue { tag: "endTime".into(), value: "16:00:00".into() },
    ];
    let algo = parse_algo_params("vwap", &params).unwrap();
    match algo {
        AlgoParams::Vwap { max_pct_vol, start_time, end_time, .. } => {
            assert!((max_pct_vol - 0.1).abs() < 1e-10);
            assert_eq!(start_time, "09:30:00");
            assert_eq!(end_time, "16:00:00");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn parse_algo_twap() {
    let algo = parse_algo_params("twap", &[]).unwrap();
    assert!(matches!(algo, AlgoParams::Twap { .. }));
}

#[test]
fn parse_algo_arrival_price() {
    let params = vec![
        TagValue { tag: "maxPctVol".into(), value: "0.25".into() },
        TagValue { tag: "riskAversion".into(), value: "Aggressive".into() },
    ];
    let algo = parse_algo_params("arrivalpx", &params).unwrap();
    match algo {
        AlgoParams::ArrivalPx { max_pct_vol, risk_aversion, .. } => {
            assert!((max_pct_vol - 0.25).abs() < 1e-10);
            assert!(matches!(risk_aversion, RiskAversion::Aggressive));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn parse_algo_close_price() {
    let algo = parse_algo_params("closepx", &[]).unwrap();
    assert!(matches!(algo, AlgoParams::ClosePx { .. }));
}

#[test]
fn parse_algo_dark_ice() {
    let params = vec![
        TagValue { tag: "displaySize".into(), value: "200".into() },
    ];
    let algo = parse_algo_params("darkice", &params).unwrap();
    match algo {
        AlgoParams::DarkIce { display_size, .. } => assert_eq!(display_size, 200),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn parse_algo_pct_vol() {
    let params = vec![
        TagValue { tag: "pctVol".into(), value: "0.05".into() },
    ];
    let algo = parse_algo_params("pctvol", &params).unwrap();
    match algo {
        AlgoParams::PctVol { pct_vol, .. } => assert!((pct_vol - 0.05).abs() < 1e-10),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn parse_algo_unsupported() {
    assert!(parse_algo_params("unknown", &[]).is_err());
}

// ── ibx#263: malformed / non-finite algo params must be rejected, not
// silently coerced into a valid-looking default ──

#[test]
fn parse_algo_vwap_rejects_malformed_max_pct_vol() {
    let params = vec![TagValue { tag: "maxPctVol".into(), value: "abc".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.contains("maxPctVol"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_nan_max_pct_vol() {
    let params = vec![TagValue { tag: "maxPctVol".into(), value: "NaN".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.contains("maxPctVol"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_infinite_max_pct_vol() {
    let params = vec![TagValue { tag: "maxPctVol".into(), value: "inf".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.contains("maxPctVol"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_malformed_bool() {
    let params = vec![TagValue { tag: "noTakeLiq".into(), value: "yes".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.contains("noTakeLiq"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_empty_max_pct_vol() {
    // A present-but-empty value is a caller who set the tag, not one who
    // never set it — it must be refused like any other malformed value,
    // not silently coerced into the "absent" default of 0.0.
    let params = vec![TagValue { tag: "maxPctVol".into(), value: "".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.contains("maxPctVol"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_empty_bool() {
    let params = vec![TagValue { tag: "noTakeLiq".into(), value: "".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.contains("noTakeLiq"), "got: {err}");
}

#[test]
fn parse_algo_arrival_price_rejects_unknown_risk_aversion() {
    // The issue's own repro: a typo must be refused, not silently sent as Neutral.
    let params = vec![TagValue { tag: "riskAversion".into(), value: "Aggresive".into() }];
    let err = parse_algo_params("arrivalpx", &params).unwrap_err();
    assert!(err.contains("riskAversion"), "got: {err}");
}

#[test]
fn parse_algo_arrival_price_defaults_risk_aversion_when_absent() {
    let algo = parse_algo_params("arrivalpx", &[]).unwrap();
    match algo {
        AlgoParams::ArrivalPx { risk_aversion, .. } => assert!(matches!(risk_aversion, RiskAversion::Neutral)),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn parse_algo_arrival_price_rejects_empty_risk_aversion() {
    // Present-but-empty is not the same as absent: only a tag the caller
    // never set may default to Neutral.
    let params = vec![TagValue { tag: "riskAversion".into(), value: "".into() }];
    let err = parse_algo_params("arrivalpx", &params).unwrap_err();
    assert!(err.contains("riskAversion"), "got: {err}");
}

#[test]
fn parse_algo_dark_ice_rejects_malformed_display_size() {
    let params = vec![TagValue { tag: "displaySize".into(), value: "abc".into() }];
    let err = parse_algo_params("darkice", &params).unwrap_err();
    assert!(err.contains("displaySize"), "got: {err}");
}

#[test]
fn parse_algo_dark_ice_rejects_negative_display_size() {
    let params = vec![TagValue { tag: "displaySize".into(), value: "-5".into() }];
    let err = parse_algo_params("darkice", &params).unwrap_err();
    assert!(err.contains("displaySize"), "got: {err}");
}

#[test]
fn parse_algo_dark_ice_defaults_display_size_when_absent() {
    let algo = parse_algo_params("darkice", &[]).unwrap();
    match algo {
        AlgoParams::DarkIce { display_size, .. } => assert_eq!(display_size, 100),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn parse_algo_dark_ice_rejects_empty_display_size() {
    let params = vec![TagValue { tag: "displaySize".into(), value: "".into() }];
    let err = parse_algo_params("darkice", &params).unwrap_err();
    assert!(err.contains("displaySize"), "got: {err}");
}

// ═══════════════════════════════════════════════════════════════════
//  Connection
// ═══════════════════════════════════════════════════════════════════

#[test]
fn is_connected_after_construction() {
    let (client, _rx, _shared) = test_client();
    assert!(client.is_connected());
}

#[test]
fn disconnect_sends_shutdown_and_clears_connected() {
    let (client, rx, _shared) = test_client();
    client.disconnect();
    assert!(!client.is_connected());
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Shutdown));
}

#[test]
fn disconnect_idempotent() {
    let (client, _rx, _shared) = test_client();
    client.disconnect();
    client.disconnect();
    assert!(!client.is_connected());
}

// ═══════════════════════════════════════════════════════════════════
//  next_order_id / req_ids
// ═══════════════════════════════════════════════════════════════════

#[test]
fn next_order_id_monotonic() {
    let (client, _rx, _shared) = test_client();
    let id1 = client.next_order_id();
    let id2 = client.next_order_id();
    let id3 = client.next_order_id();
    assert!(id2 > id1);
    assert!(id3 > id2);
}

#[test]
fn req_ids_calls_wrapper() {
    let (client, _rx, _shared) = test_client();
    let mut w = RecordingWrapper::default();
    client.req_ids(&mut w);
    assert_eq!(w.events.len(), 1);
    assert!(w.events[0].starts_with("next_valid_id:"));
}

// ═══════════════════════════════════════════════════════════════════
//  Market data requests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_mkt_data_sends_register_and_subscribe() {
    let (client, rx, _shared) = test_client();
    let _ = client.req_mkt_data(1, &spy(), "", false, false);
    let cmd1 = rx.try_recv().unwrap();
    assert!(matches!(cmd1, ControlCommand::RegisterInstrument { con_id: 756733, .. }));
    let cmd2 = rx.try_recv().unwrap();
    match cmd2 {
        ControlCommand::Subscribe { con_id, symbol, .. } => {
            assert_eq!(con_id, 756733);
            assert_eq!(symbol, "SPY");
        }
        _ => panic!("expected Subscribe, got {cmd2:?}"),
    }
}

#[test]
fn req_mkt_data_defaults_to_realtime_mode() {
    let (client, rx, _shared) = test_client();
    let _ = client.req_mkt_data(1, &spy(), "", false, false);
    let _register = rx.try_recv().unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::Subscribe { mode_9887, .. } => assert_eq!(mode_9887, 0),
        other => panic!("expected Subscribe, got {other:?}"),
    }
}

#[test]
fn req_mkt_data_ex_propagates_mode_9887() {
    for mode in [1_i32, 2, 3] {
        let (client, rx, _shared) = test_client();
        let _ = client.req_mkt_data_ex(1, &spy(), "", false, false, mode);
        let _register = rx.try_recv().unwrap();
        match rx.try_recv().unwrap() {
            ControlCommand::Subscribe { mode_9887, con_id, .. } => {
                assert_eq!(mode_9887, mode);
                assert_eq!(con_id, 756733);
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }
}

// ibx#233: a second live subscription on the same contract would clobber
// the first's reverse mapping and orphan it silently. Reject at the call.
#[test]
fn req_mkt_data_duplicate_instrument_is_rejected() {
    let (client, rx, _shared) = test_client();
    // Existing live subscription for SPY (instrument 0) under req_id 1.
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);

    let err = client.req_mkt_data(2, &spy(), "", false, false).unwrap_err();
    assert!(err.contains("req_id 1"), "got: {err}");
    assert!(rx.try_recv().is_err(), "nothing may reach the engine");
}

// ibx#278: a contract given the ordinary ibapi way carries conId 0. Cached as
// an identity it maps every later symbol onto the first one's instrument, and
// the ibx#233 guard above then refuses them all — a symbol-only client could
// hold exactly one subscription.
#[test]
fn a_second_symbol_is_not_a_duplicate_of_the_first_con_id_less_contract() {
    let (client, rx, _shared) = test_client();
    // What a live symbol-only subscription under req_id 1 used to leave behind.
    client.core.con_id_to_instrument.lock().unwrap().insert(0, 0);
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);

    let qqq = Contract {
        symbol: "QQQ".into(), sec_type: "STK".into(), exchange: "SMART".into(),
        ..Default::default()
    };
    let err = client.req_mkt_data(2, &qqq, "", false, false).unwrap_err();
    assert!(!err.contains("req_id 1"), "QQQ is not the live contract: {err}");
    match rx.try_recv().expect("the registration reaches the engine") {
        ControlCommand::RegisterInstrument { con_id, symbol, .. } => {
            assert_eq!((con_id, symbol.as_str()), (0, "QQQ"));
        }
        other => panic!("expected RegisterInstrument, got {other:?}"),
    }
}

#[test]
fn cancel_mkt_data_sends_unsubscribe() {
    let (client, rx, _shared) = test_client();
    // Pre-register mapping
    client.core.req_to_instrument.lock().unwrap().insert(1, 0);
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);
    client.cancel_mkt_data(1).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Unsubscribe { instrument: 0 }));
    // Mapping should be cleared
    assert!(client.core.req_to_instrument.lock().unwrap().get(&1).is_none());
}

#[test]
fn cancel_mkt_data_unknown_req_id_no_panic() {
    let (client, rx, _shared) = test_client();
    client.cancel_mkt_data(999).unwrap();
    assert!(rx.try_recv().is_err()); // no commands sent
}

#[test]
fn req_tick_by_tick_data_sends_subscribe_tbt() {
    let (client, rx, _shared) = test_client();
    let _ = client.req_tick_by_tick_data(10, &spy(), "BidAsk", 0, false);
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::SubscribeTbt { con_id, symbol, tbt_type, .. } => {
            assert_eq!(con_id, 756733);
            assert_eq!(symbol, "SPY");
            assert!(matches!(tbt_type, TbtType::BidAsk));
        }
        _ => panic!("expected SubscribeTbt"),
    }
}

#[test]
fn req_tick_by_tick_data_defaults_to_last() {
    let (client, rx, _shared) = test_client();
    let _ = client.req_tick_by_tick_data(10, &spy(), "AllLast", 0, false);
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::SubscribeTbt { tbt_type, .. } => {
            assert!(matches!(tbt_type, TbtType::Last));
        }
        _ => panic!("expected SubscribeTbt"),
    }
}

#[test]
fn cancel_tick_by_tick_data_sends_unsubscribe_tbt() {
    let (client, rx, _shared) = test_client();
    client.core.req_to_instrument.lock().unwrap().insert(10, 3);
    client.cancel_tick_by_tick_data(10).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::UnsubscribeTbt { instrument: 3 }));
}

#[test]
fn cancel_tick_by_tick_unknown_req_id_no_panic() {
    let (client, rx, _shared) = test_client();
    client.cancel_tick_by_tick_data(999).unwrap();
    assert!(rx.try_recv().is_err());
}

// ═══════════════════════════════════════════════════════════════════
//  Orders — every order type
// ═══════════════════════════════════════════════════════════════════

#[test]
fn place_order_market() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order { action: "BUY".into(), total_quantity: 100.0, order_type: "MKT".into(), ..Default::default() };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { qty, kind: OrderKind::Market, .. }) => assert_eq!(qty, 100),
        _ => panic!("expected a Market order, got {cmd:?}"),
    }
}

#[test]
fn place_order_limit() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 50.0, order_type: "LMT".into(),
        lmt_price: 150.25, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { qty, kind: OrderKind::Limit { price, .. }, .. }) => {
            assert_eq!(qty, 50);
            assert_eq!(price, (150.25 * PRICE_SCALE_F) as i64);
        }
        _ => panic!("expected a Limit order, got {cmd:?}"),
    }
}

#[test]
fn place_order_trailing_stop_carries_initial_trigger() {
    // ibx#225 Part B / ib-agent#173: a plain amount trailing stop can carry an
    // initial stop trigger (trailStopPrice); it must reach the request.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "TRAIL".into(),
        aux_price: 0.50,             // trail amount
        trail_stop_price: 10.00,     // initial stop trigger
        ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::TrailingStop { trail_amt, trail_stop_price, .. }, .. }) => {
            assert_eq!(trail_amt, (0.50 * PRICE_SCALE_F) as i64);
            assert_eq!(trail_stop_price, (10.00 * PRICE_SCALE_F) as i64);
        }
        cmd => panic!("expected a TrailingStop order, got {cmd:?}"),
    }
}

#[test]
fn place_order_trailing_stop_without_trigger_is_unset() {
    // Default (f64::MAX) must encode as 0 (not set), so the tag is omitted.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "TRAIL".into(),
        aux_price: 0.50, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::TrailingStop { trail_stop_price, .. }, .. }) => {
            assert_eq!(trail_stop_price, 0);
        }
        cmd => panic!("expected a TrailingStop order, got {cmd:?}"),
    }
}

#[test]
fn place_order_adjustable_trail_carries_trailing_amount_and_unit() {
    // ibx#225 / ib-agent#167: a base STP that converts to a TRAIL must carry
    // the trailing amount and unit through to the AdjustableStop request.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "STP".into(),
        aux_price: 11.00,                          // base stop price
        adjusted_order_type: "TRAIL".into(),
        trigger_price: 11.00,
        adjusted_stop_price: 10.00,
        adjusted_trailing_amount: 0.50,
        adjustable_trailing_unit: 0,               // amount
        ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { kind: crate::types::OrderKind::AdjustableStop {
            adjusted_order_type, stop_price, trigger_price, adjusted_stop_price,
            adjusted_trailing_amount, adjustable_trailing_unit, .. }, .. }) => {
            assert_eq!(adjusted_order_type, crate::types::AdjustedOrderType::Trail);
            assert_eq!(stop_price, (11.00 * PRICE_SCALE_F) as i64);
            assert_eq!(trigger_price, (11.00 * PRICE_SCALE_F) as i64);
            assert_eq!(adjusted_stop_price, (10.00 * PRICE_SCALE_F) as i64);
            assert_eq!(adjusted_trailing_amount, (0.50 * PRICE_SCALE_F) as i64);
            assert_eq!(adjustable_trailing_unit, 0);
        }
        _ => panic!("expected SubmitEx carrying AdjustableStop, got {cmd:?}"),
    }
}

#[test]
fn place_order_adjustable_stop_carries_bracket_attrs_and_tif() {
    // ibx#240: an adjustable stop used as a bracket child must stay linked to
    // its parent and its OCA group and keep the caller's tif. Routing it around
    // the extended-attrs path shipped the child naked, unlinked and DAY.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "STP".into(),
        aux_price: 11.00,
        adjusted_order_type: "STP".into(),
        trigger_price: 12.00,
        adjusted_stop_price: 11.50,
        parent_id: 42,
        oca_group: "bracket_1".into(),
        oca_type: 1,
        tif: "GTC".into(),
        ..Default::default()
    };
    client.place_order(7, &spy(), &order).unwrap();

    match rx.try_recv().unwrap() {
        ControlCommand::Order(OrderRequest::SubmitEx { kind, tif, attrs, .. }) => {
            assert!(matches!(kind, crate::types::OrderKind::AdjustableStop { .. }),
                "adjustable stop must route through the extended path; got {kind:?}");
            assert_eq!(tif, b'1', "tif must survive as GTC");
            assert_eq!(attrs.parent_id, 42, "bracket child must stay linked to its parent");
            assert_eq!(attrs.oca_group_str, "bracket_1", "OCA group must survive");
        }
        cmd => panic!("expected SubmitEx carrying AdjustableStop, got {cmd:?}"),
    }
}

#[test]
fn modify_carries_outside_rth_from_the_resubmitted_order() {
    // ibx#247: the replace asserted 6433=1 unconditionally, so an order placed
    // with outside_rth=false came back outside-RTH after any modify. The flag
    // has to travel with the modify, since the tracked record has no field for
    // it.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
        lmt_price: 100.0, outside_rth: false, ..Default::default()
    };
    client.place_order(70, &spy(), &order).unwrap();
    let _submit = rx.try_recv().unwrap();

    // Same id -> modify. Caller still says outside_rth=false.
    let reprice = Order { lmt_price: 101.0, ..order.clone() };
    client.place_order(70, &spy(), &reprice).unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::Order(OrderRequest::Modify { outside_rth, .. }) => {
            assert!(!outside_rth, "a modify must not opt the order into the extended session");
        }
        cmd => panic!("expected Modify, got {cmd:?}"),
    }

    // And it survives when the caller does want it.
    let rth_out = Order { lmt_price: 102.0, outside_rth: true, ..order.clone() };
    client.place_order(70, &spy(), &rth_out).unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::Order(OrderRequest::Modify { outside_rth, .. }) => {
            assert!(outside_rth, "an explicit outside_rth=true must reach the replace");
        }
        cmd => panic!("expected Modify, got {cmd:?}"),
    }
}

#[test]
fn place_order_adjustable_trail_percent_unit_passes_through() {
    // Percent unit (100) must survive; the trailing amount is a percent value.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "STP".into(),
        aux_price: 11.00,
        adjusted_order_type: "TRAIL".into(),
        adjusted_trailing_amount: 1.00,            // 1.00%
        adjustable_trailing_unit: 100,             // percent
        ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    match rx.try_recv().unwrap() {
        ControlCommand::Order(OrderRequest::SubmitEx { kind: crate::types::OrderKind::AdjustableStop {
            adjustable_trailing_unit, adjusted_trailing_amount, .. }, .. }) => {
            assert_eq!(adjustable_trailing_unit, 100);
            assert_eq!(adjusted_trailing_amount, (1.00 * PRICE_SCALE_F) as i64);
        }
        cmd => panic!("expected SubmitEx carrying AdjustableStop, got {cmd:?}"),
    }
}

#[test]
fn place_order_limit_gtc_carries_the_tif() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 10.0, order_type: "LMT".into(),
        lmt_price: 100.0, tif: "GTC".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { tif, kind: OrderKind::Limit { .. }, .. }) => {
            assert_eq!(tif, b'1'); // GTC
        }
        _ => panic!("expected a limit order, got {cmd:?}"),
    }
}

#[test]
fn place_order_limit_hidden_carries_the_attribute() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 10.0, order_type: "LMT".into(),
        lmt_price: 100.0, hidden: true, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { attrs, kind: OrderKind::Limit { .. }, .. }) => {
            assert!(attrs.hidden);
        }
        _ => panic!("expected a limit order, got {cmd:?}"),
    }
}

// ── ibx#224: every order type must carry attrs + tif when set ──

#[test]
fn place_order_stop_with_parent_and_gtc_uses_submit_ex() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "STP".into(),
        aux_price: 240.0, tif: "GTC".into(), parent_id: 42,
        oca_group: "77".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { kind, tif, attrs, .. }) => {
            assert!(matches!(kind, crate::types::OrderKind::Stop { stop_price }
                if stop_price == (240.0 * PRICE_SCALE_F) as i64));
            assert_eq!(tif, b'1'); // GTC
            assert_eq!(attrs.parent_id, 42);
            assert_eq!(attrs.oca_group, 77);
        }
        _ => panic!("expected a Ex order, got {cmd:?}"),
    }
}

#[test]
fn place_order_market_outside_rth_uses_submit_ex() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 1.0, order_type: "MKT".into(),
        outside_rth: true, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { kind, tif, attrs, .. }) => {
            assert!(matches!(kind, crate::types::OrderKind::Market));
            assert_eq!(tif, b'0'); // DAY
            assert!(attrs.outside_rth);
        }
        _ => panic!("expected a Ex order, got {cmd:?}"),
    }
}

#[test]
fn place_order_trailing_amount_with_oca_uses_submit_ex() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "TRAIL".into(),
        aux_price: 2.0, tif: "GTC".into(), oca_group: "exit_9".into(),
        oca_type: 2, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { kind, tif, attrs, .. }) => {
            assert!(matches!(kind, crate::types::OrderKind::TrailingStop { trail_amt, .. }
                if trail_amt == (2.0 * PRICE_SCALE_F) as i64));
            assert_eq!(tif, b'1');
            assert_eq!(attrs.oca_group_str, "exit_9");
            assert_eq!(attrs.oca_type, 2); // ibx#215
        }
        _ => panic!("expected a Ex order, got {cmd:?}"),
    }
}

#[test]
fn place_order_empty_tif_is_day() {
    // An empty tif is DAY, matching the official API default.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 1.0, order_type: "STP".into(),
        aux_price: 240.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::Order(OrderRequest::SubmitEx { tif, kind: OrderKind::Stop { .. }, .. }) => {
            assert_eq!(tif, b'0', "an empty tif is DAY");
        }
        other => panic!("expected a stop order, got {other:?}"),
    }
}

// ── ibx#226: transmit=false must be rejected, not silently ignored ──

#[test]
fn place_order_transmit_false_is_rejected() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
        lmt_price: 100.0, transmit: false, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.to_string().contains("transmit=false"), "got: {err}");
    assert!(rx.try_recv().is_err(), "nothing may reach the engine");
}

// ── ibx#96: FA allocation must be rejected, not silently dropped ──

/// No encoder reads an FA field, so an accepted one fills the whole size on
/// the connected account instead of spreading it across the advisor group.
/// The algo case is the ordering proof: `validate_order` returns Ok early for
/// an algo order, so a guard placed after that check lets FA+Adaptive through.
/// Naming your own connected account is the ordinary single-account pattern and
/// must keep working: the guard rejects a mismatch, not the presence of a value.
#[test]
fn place_order_accepts_the_connected_account_by_name() {
    let (client, rx, _shared) = test_client();
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, account: "DU123".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).expect("the connected account is not a mismatch");
    assert!(rx.try_recv().is_ok(), "and the order reaches the engine");
}

#[test]
fn place_order_fa_allocation_is_rejected() {
    let cases: Vec<OrderCase> = vec![
        ("fa_group", |o| o.fa_group = "AllAccounts".into()),
        ("fa_method", |o| o.fa_method = "EqualQuantity".into()),
        ("fa_percentage", |o| o.fa_percentage = "50".into()),
        ("fa_group with an algo", |o| {
            o.fa_group = "AllAccounts".into();
            o.algo_strategy = "Adaptive".into();
        }),
        // Same class, and sharper: no encoder reads this either, so the order
        // fills on the connected account while the open-order snapshot echoes
        // the caller's value back and confirms the wrong one.
        ("account", |o| o.account = "U9999999".into()),
    ];
    for (name, set) in cases {
        let (client, rx, _shared) = test_client();
        let mut order = Order {
            action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
            lmt_price: 150.0, ..Default::default()
        };
        set(&mut order);
        let Err(err) = client.place_order(1, &spy(), &order) else {
            panic!("{name} must be refused");
        };
        // The field under test, not a fixed one: asserting "fa_group" for every
        // arm meant three of them only proved that some error was returned.
        let field = name.split(' ').next().unwrap();
        assert!(err.contains(field), "{name}: the message must name the field — {err}");
        assert!(rx.try_recv().is_err(), "{name}: nothing reaches the engine");
    }
}

#[test]
fn place_order_unknown_tif_is_rejected() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
        lmt_price: 100.0, tif: "GTX".into(), ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.to_string().contains("tif"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn place_order_all_or_none_trail_is_rejected() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 1.0, order_type: "TRAIL".into(),
        aux_price: 2.0, all_or_none: true, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.to_string().contains("all_or_none"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

// ── ibx#215: oca_type carried and coerced ──

#[test]
fn attrs_oca_type_coerces_out_of_range_to_unset() {
    let order = Order { oca_type: 9, ..Default::default() };
    assert_eq!(order.attrs().oca_type, 0);
    let order = Order { oca_type: 4, ..Default::default() };
    assert_eq!(order.attrs().oca_type, 4);
    let order = Order { oca_type: -1, ..Default::default() };
    assert_eq!(order.attrs().oca_type, 0);
}

#[test]
fn place_order_stop() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "STP".into(),
        aux_price: 145.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { side, kind: OrderKind::Stop { stop_price, .. }, .. }) => {
            assert!(matches!(side, Side::Sell));
            assert_eq!(stop_price, (145.0 * PRICE_SCALE_F) as i64);
        }
        _ => panic!("expected a Stop order, got {cmd:?}"),
    }
}

#[test]
fn place_order_stop_limit() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "STP LMT".into(),
        lmt_price: 144.0, aux_price: 145.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::StopLimit { price, stop_price, .. }, .. }) => {
            assert_eq!(price, (144.0 * PRICE_SCALE_F) as i64);
            assert_eq!(stop_price, (145.0 * PRICE_SCALE_F) as i64);
        }
        _ => panic!("expected a StopLimit order, got {cmd:?}"),
    }
}

#[test]
fn place_order_trailing_stop_amount() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "TRAIL".into(),
        aux_price: 2.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::TrailingStop { trail_amt, .. }, .. }) => {
            assert_eq!(trail_amt, (2.0 * PRICE_SCALE_F) as i64);
        }
        _ => panic!("expected a TrailingStop order, got {cmd:?}"),
    }
}

#[test]
fn place_order_trailing_stop_percent() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "TRAIL".into(),
        trailing_percent: 5.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::TrailPct { trail_pct, .. }, .. }) => {
            assert_eq!(trail_pct, 500); // 5.0 * 100
        }
        _ => panic!("expected a TrailingStopPct order, got {cmd:?}"),
    }
}

#[test]
fn place_order_trailing_stop_limit() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "TRAIL LIMIT".into(),
        lmt_price: 148.0, aux_price: 2.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::TrailingStopLimit { .. }, .. })));
}

#[test]
fn place_order_moc() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "MOC".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::Moc, .. })));
}

#[test]
fn place_order_loc() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LOC".into(),
        lmt_price: 150.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::Loc { .. }, .. })));
}

#[test]
fn place_order_mit() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "MIT".into(),
        aux_price: 148.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::Mit { .. }, .. })));
}

#[test]
fn place_order_lit() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LIT".into(),
        lmt_price: 150.0, aux_price: 148.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::Lit { .. }, .. })));
}

#[test]
fn place_order_mtl() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "MTL".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::Mtl, .. })));
}

#[test]
fn place_order_mkt_prt() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "MKT PRT".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::MktPrt, .. })));
}

#[test]
fn place_order_stp_prt() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "STP PRT".into(),
        aux_price: 145.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::StpPrt { .. }, .. })));
}

#[test]
fn place_order_rel() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "REL".into(),
        aux_price: 0.10, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::Rel { .. }, .. })));
}

#[test]
fn place_order_peg_mkt() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "PEG MKT".into(),
        aux_price: 0.05, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::PegMkt { .. }, .. })));
}

#[test]
fn place_order_peg_mid() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "PEG MID".into(),
        aux_price: 0.02, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::PegMid { .. }, .. })));
}

#[test]
fn place_order_midprice() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "MIDPRICE".into(),
        lmt_price: 150.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::MidPrice { .. }, .. })));
}

#[test]
fn place_order_snap_mkt() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "SNAP MKT".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::SnapMkt, .. })));
}

#[test]
fn place_order_snap_mid() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "SNAP MID".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::SnapMid, .. })));
}

#[test]
fn place_order_snap_pri() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "SNAP PRI".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::SnapPri, .. })));
}

#[test]
fn place_order_box_top() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "BOX TOP".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::Mtl, .. })));
}

#[test]
fn place_order_sell_side() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 50.0, order_type: "MKT".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { side, kind: OrderKind::Market, .. }) => {
            assert!(matches!(side, Side::Sell));
        }
        _ => panic!("expected SubmitMarket"),
    }
}

#[test]
fn place_order_short_sell_side() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SSHORT".into(), total_quantity: 50.0, order_type: "MKT".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { side, kind: OrderKind::Market, .. }) => {
            assert!(matches!(side, Side::ShortSell));
        }
        _ => panic!("expected SubmitMarket"),
    }
}

#[test]
fn place_order_algo_vwap() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 1000.0, order_type: "LMT".into(),
        lmt_price: 150.0, algo_strategy: "vwap".into(),
        algo_params: vec![TagValue { tag: "maxPctVol".into(), value: "0.1".into() }],
        ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx {
        kind: OrderKind::Algo { .. }, .. })));
}

#[test]
fn place_order_what_if() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, what_if: true, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx {
        kind: OrderKind::WhatIf { .. }, .. })));
}

#[test]
fn place_order_unsupported_type_returns_error() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "FANTASY".into(), ..Default::default()
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unsupported order type"));
}

#[test]
fn place_order_non_stk_contract_rejected() {
    // A non-STK contract must be rejected, not silently sent as a stock order
    // on the underlying. See: https://github.com/deepentropy/ibx/issues/202
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let opt = Contract { con_id: 999001, symbol: "AAPL".into(), sec_type: "OPT".into(), ..Default::default() };
    let order = Order { action: "BUY".into(), total_quantity: 1.0, order_type: "MKT".into(), ..Default::default() };
    let result = client.place_order(1, &opt, &order);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("OPT"));
    assert!(err.contains("STK"));
    // No order must have been queued to the engine.
    assert!(rx.try_recv().is_err());
}

#[test]
fn place_order_explicit_stk_contract_accepted() {
    // An explicit sec_type="STK" must still be accepted.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let stk = Contract { con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(), ..Default::default() };
    let order = Order { action: "BUY".into(), total_quantity: 100.0, order_type: "MKT".into(), ..Default::default() };
    client.place_order(1, &stk, &order).unwrap();
    assert!(rx.try_recv().is_ok());
}

#[test]
fn place_order_invalid_action_returns_error() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "INVALID".into(), total_quantity: 100.0, order_type: "MKT".into(), ..Default::default()
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
}

#[test]
fn place_order_auto_assigns_id_when_zero() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "MKT".into(), ..Default::default()
    };
    // order_id = 0 → auto-assign
    client.place_order(0, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { order_id, kind: OrderKind::Market, .. }) => {
            assert!(order_id > 0);
        }
        _ => panic!("expected SubmitMarket"),
    }
}

#[test]
fn cancel_order_sends_cancel_command() {
    let (client, rx, _shared) = test_client();
    client.cancel_order(42, "").unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::Cancel { order_id }) => assert_eq!(order_id, 42),
        _ => panic!("expected Cancel"),
    }
}

/// ibx#265: the executions mutex must not be held while user callbacks run.
/// A wrapper that re-enters a path locking `executions` is an ordinary ibapi
/// pattern (re-requesting from `exec_details`), and holding the lock across it
/// deadlocks — in Python with the GIL held, freezing the interpreter.
#[test]
fn req_executions_does_not_hold_the_lock_across_callbacks() {
    struct Reentrant<'a> {
        core: &'a ClientCore,
        observed_locked: bool,
        rows: usize,
    }
    impl Wrapper for Reentrant<'_> {
        fn exec_details(&mut self, _r: i64, _c: &Contract, _e: &crate::api::types::Execution) {
            self.rows += 1;
            // Re-entering while the lock is held is exactly the deadlock.
            if self.core.executions.try_lock().is_err() {
                self.observed_locked = true;
            }
        }
    }

    let (client, _rx, _shared) = test_client();
    client.core.push_execution(
        1,
        crate::api::types::Contract { symbol: "AAPL".into(), ..Default::default() },
        Default::default(),
        Default::default(),
    );

    let mut w = Reentrant { core: &client.core, observed_locked: false, rows: 0 };
    client.req_executions(1, &crate::api::types::ExecutionFilter::default(), &mut w);
    assert_eq!(w.rows, 1, "the execution must still be replayed");
    assert!(!w.observed_locked,
        "executions lock must be released before the callback runs");
}

/// `ExecutionFilter.time` is a lower bound in ibapi. It was parsed and then
/// ignored, so a caller asking for today's fills got the whole history.
#[test]
fn execution_filter_time_is_a_lower_bound() {
    #[derive(Default)]
    struct Rows { seen: Vec<String> }
    impl Wrapper for Rows {
        fn exec_details(&mut self, _r: i64, _c: &Contract, e: &crate::api::types::Execution) {
            self.seen.push(e.time.clone());
        }
    }

    let (client, _rx, _shared) = test_client();
    for t in ["20260729-09:00:00", "20260729-11:00:00"] {
        client.core.push_execution(
            1,
            crate::api::types::Contract { symbol: "AAPL".into(), ..Default::default() },
            crate::api::types::Execution { time: t.into(), ..Default::default() },
            Default::default(),
        );
    }

    let mut w = Rows::default();
    client.req_executions(1, &crate::api::types::ExecutionFilter {
        time: "20260729-10:00:00".into(), ..Default::default()
    }, &mut w);
    assert_eq!(w.seen, vec!["20260729-11:00:00"], "only executions at or after the bound");

    // Punctuation differs between the two sides in practice; the comparison is
    // on digits, so a space-separated bound behaves identically.
    let mut w2 = Rows::default();
    client.req_executions(1, &crate::api::types::ExecutionFilter {
        time: "20260729 10:00:00".into(), ..Default::default()
    }, &mut w2);
    assert_eq!(w2.seen, vec!["20260729-11:00:00"], "separator must not change the bound");

    // A date-only bound keeps the whole day rather than dropping it.
    let mut w3 = Rows::default();
    client.req_executions(1, &crate::api::types::ExecutionFilter {
        time: "20260729".into(), ..Default::default()
    }, &mut w3);
    assert_eq!(w3.seen.len(), 2, "a date-only bound keeps that day");
}

#[test]
fn req_global_cancel_sends_cancel_all_for_each_instrument() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(2);
    client.req_global_cancel().unwrap();
    let mut cancel_instruments = vec![];
    while let Ok(cmd) = rx.try_recv() {
        if let ControlCommand::Order(OrderRequest::CancelAll { instrument }) = cmd {
            cancel_instruments.push(instrument);
        }
    }
    assert_eq!(cancel_instruments.len(), 2);
    cancel_instruments.sort();
    assert_eq!(cancel_instruments, vec![0, 1]);
}

#[test]
fn req_global_cancel_no_instruments_no_commands() {
    let (client, rx, _shared) = test_client();
    client.req_global_cancel().unwrap();
    assert!(rx.try_recv().is_err());
}

// ═══════════════════════════════════════════════════════════════════
//  Order validation — aux_price guards (issue #115)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn stp_order_with_zero_aux_price_is_rejected() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "STP".into(),
        lmt_price: 145.0, // common mistake: setting lmt_price instead of aux_price
        ..Default::default()
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("aux_price"));
}

#[test]
fn stp_order_with_valid_aux_price_succeeds() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "STP".into(),
        aux_price: 145.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::Stop { .. }, .. })));
}

#[test]
fn stp_lmt_order_with_zero_aux_price_is_rejected() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "STP LMT".into(),
        lmt_price: 144.0, ..Default::default() // aux_price missing
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("aux_price"));
}

#[test]
fn trail_order_with_zero_amount_and_zero_percent_is_rejected() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "TRAIL".into(),
        ..Default::default() // neither trailing_percent nor aux_price
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("trailing_percent"));
}

#[test]
fn trail_order_with_trailing_percent_succeeds() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "TRAIL".into(),
        trailing_percent: 5.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::TrailPct { .. }, .. })));
}

#[test]
fn trail_limit_order_with_zero_aux_price_is_rejected() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "TRAIL LIMIT".into(),
        lmt_price: 148.0, ..Default::default() // aux_price missing
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("aux_price"));
}

#[test]
fn mit_order_with_zero_aux_price_is_rejected() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "MIT".into(),
        ..Default::default()
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("aux_price"));
}

#[test]
fn stp_prt_order_with_zero_aux_price_is_rejected() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "STP PRT".into(),
        ..Default::default()
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("aux_price"));
}

#[test]
fn lit_order_with_zero_aux_price_is_rejected() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LIT".into(),
        lmt_price: 150.0, ..Default::default() // aux_price missing
    };
    let result = client.place_order(1, &spy(), &order);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("aux_price"));
}

// ═══════════════════════════════════════════════════════════════════
//  Order validation — non-finite / out-of-range numerics (issue #263)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn place_order_rejects_nan_lmt_price() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: f64::NAN, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("lmt_price"), "got: {err}");
}

#[test]
fn place_order_rejects_infinite_lmt_price() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: f64::INFINITY, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("lmt_price"), "got: {err}");
}

#[test]
fn place_order_rejects_lmt_price_that_overflows_the_wire() {
    // Finite, but scaling by PRICE_SCALE_F (1e8) overflows the wire's i64 —
    // the old code let this saturate to i64::MAX instead of refusing it.
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 1.0e12, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("lmt_price"), "got: {err}");
}

#[test]
fn place_order_rejects_lmt_price_at_the_exact_wire_boundary() {
    // `i64::MAX as f64` rounds up to 2^63, so this value scales back to
    // exactly 2^63 in `require_finite_price` — a `>` comparison against
    // that rounded boundary let it through and the cast saturated to
    // i64::MAX instead of refusing it.
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: i64::MAX as f64 / PRICE_SCALE_F, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("lmt_price"), "got: {err}");
}

#[test]
fn place_order_rejects_nan_aux_price() {
    // NaN != 0.0, so the pre-existing "aux_price required" check (which only
    // compares against == 0.0) never catches this on its own.
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "STP".into(),
        aux_price: f64::NAN, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("aux_price"), "got: {err}");
}

#[test]
fn place_order_rejects_negative_quantity() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: -100.0, order_type: "MKT".into(), ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("total_quantity"), "got: {err}");
}

#[test]
fn place_order_rejects_nan_quantity() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: f64::NAN, order_type: "MKT".into(), ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("total_quantity"), "got: {err}");
}

#[test]
fn place_order_rejects_infinite_quantity() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: f64::INFINITY, order_type: "MKT".into(), ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("total_quantity"), "got: {err}");
}

#[test]
fn place_order_rejects_negative_display_size() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, display_size: -5, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("display_size"), "got: {err}");
}

#[test]
fn place_order_rejects_negative_min_qty() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, min_qty: -5, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("min_qty"), "got: {err}");
}

#[test]
fn place_order_rejects_negative_parent_id() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, parent_id: -5, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("parent_id"), "got: {err}");
}

#[test]
fn place_order_rejects_negative_trailing_percent() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "SELL".into(), total_quantity: 100.0, order_type: "TRAIL".into(),
        trailing_percent: -5.0, ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("trailing_percent"), "got: {err}");
}

#[test]
fn place_order_adaptive_rejects_unknown_priority() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, algo_strategy: "Adaptive".into(),
        algo_params: vec![TagValue { tag: "adaptivePriority".into(), value: "Aggressive".into() }],
        ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.contains("adaptivePriority"), "got: {err}");
}

#[test]
fn place_order_adaptive_defaults_priority_when_absent() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, algo_strategy: "Adaptive".into(), ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::Order(OrderRequest::SubmitEx {
            kind: OrderKind::Adaptive { priority, .. }, ..
        }) => {
            assert_eq!(priority, crate::types::AdaptivePriority::Normal);
        }
        cmd => panic!("expected an adaptive order, got {cmd:?}"),
    }
}

// place_order validates before building: the two tests above go through
// place_order, so either function's check alone makes them pass and neither
// pins down which one is doing the rejecting. validate_order is also the
// only check an order Modify (place_order on an already-tracked order_id)
// runs, since that path never calls build_order_request. These call each
// function directly to prove its own guard independently of the other.

#[test]
fn validate_order_adaptive_rejects_unknown_priority() {
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, algo_strategy: "Adaptive".into(),
        algo_params: vec![TagValue { tag: "adaptivePriority".into(), value: "Aggressive".into() }],
        ..Default::default()
    };
    let err = crate::client_core::ClientCore::validate_order(&order, "DU123").unwrap_err();
    assert!(err.contains("adaptivePriority"), "got: {err}");
}

#[test]
fn build_order_request_adaptive_rejects_unknown_priority() {
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, algo_strategy: "Adaptive".into(),
        algo_params: vec![TagValue { tag: "adaptivePriority".into(), value: "Aggressive".into() }],
        ..Default::default()
    };
    let err = crate::client_core::ClientCore::build_order_request(&order, 1, 0).unwrap_err();
    assert!(err.contains("adaptivePriority"), "got: {err}");
}

// ═══════════════════════════════════════════════════════════════════
//  Historical data requests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_historical_data_sends_fetch_historical() {
    let (client, rx, _shared) = test_client();
    client.req_historical_data(5, &spy(), "20260101 16:00:00", "1 D", "1 hour", "TRADES", true, 1, false).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchHistorical {
            req_id, con_id, duration, bar_size, what_to_show, use_rth, sec_type, exchange, ..
        } => {
            assert_eq!(req_id, 5);
            assert_eq!(con_id, 756733);
            assert_eq!(duration, "1 D");
            assert_eq!(bar_size, "1 hour");
            assert_eq!(what_to_show, "TRADES");
            assert!(use_rth);
            // The contract's own fields have to leave the client, or the
            // engine has nothing but the old constants to fall back on
            // (ibx#305). `spy()` states neither, so both arrive empty and the
            // engine substitutes — that substitution is tested at its source.
            assert_eq!(sec_type, "");
            assert_eq!(exchange, "");
        }
        _ => panic!("expected FetchHistorical"),
    }
}

/// A contract that does state its own type and venue must carry both, which is
/// the whole of what this fixes: every historical query described itself as a
/// SMART-routed stock regardless of the contract asked for.
#[test]
fn req_historical_data_carries_the_contract_s_own_type_and_venue() {
    let (client, rx, _shared) = test_client();
    let es = Contract {
        con_id: 495512563, symbol: "ES".into(),
        sec_type: "FUT".into(), exchange: "CME".into(), ..Default::default()
    };
    client.req_historical_data(6, &es, "20260101 16:00:00", "1 D", "1 hour", "TRADES", true, 1, false).unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::FetchHistorical { sec_type, exchange, .. } => {
            assert_eq!(sec_type, "FUT");
            assert_eq!(exchange, "CME");
        }
        _ => panic!("expected FetchHistorical"),
    }
}

// ── ibx#232: unknown bar_size / what_to_show reject instead of silently
// falling back to 5-minute / TRADES bars ──

#[test]
fn req_historical_data_rejects_unknown_bar_size() {
    let (client, rx, _shared) = test_client();
    // The issue's exact repro: "1 Min" (wrong case) used to return 5-minute
    // candles with no error.
    let err = client.req_historical_data(5, &spy(), "", "2 D", "1 Min", "TRADES", true, 1, false).unwrap_err();
    assert!(err.contains("bar_size"), "got: {err}");
    assert!(rx.try_recv().is_err(), "nothing may reach the engine");
}

#[test]
fn req_historical_data_rejects_unknown_what_to_show() {
    let (client, rx, _shared) = test_client();
    let err = client.req_historical_data(5, &spy(), "", "2 D", "1 min", "TRADE", true, 1, false).unwrap_err();
    assert!(err.contains("what_to_show"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn req_historical_data_rejects_unsupported_keep_up_to_date_size() {
    let (client, rx, _shared) = test_client();
    // "1 min" is valid on the batch path but not supported for streaming —
    // it used to silently downgrade to 5-minute bars on this path only.
    let err = client.req_historical_data(5, &spy(), "", "1 D", "1 min", "TRADES", true, 1, true).unwrap_err();
    assert!(err.contains("keep_up_to_date"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn req_historical_data_accepts_streamable_keep_up_to_date_size() {
    let (client, rx, _shared) = test_client();
    client.req_historical_data(5, &spy(), "", "1 D", "5 mins", "TRADES", true, 1, true).unwrap();
    assert!(matches!(rx.try_recv().unwrap(), ControlCommand::FetchHistorical { keep_up_to_date: true, .. }));
}

/// A req_id reaches these requests' wire form as u32. `next_order_id()` hands
/// out ids near 1.7e12, so a caller running one counter for orders and
/// requests — the ibapi idiom — had every one of these wrap: the gateway saw
/// an id nobody chose, and the callback came back tagged with that id instead
/// of the one the caller asked under.
#[test]
fn an_unwireable_req_id_is_refused() {
    type Call = fn(&EClient, i64) -> Result<(), String>;
    let calls: &[(&str, Call)] = &[
        ("req_historical_data", |c, id| c.req_historical_data(id, &spy(), "", "1 D", "1 min", "TRADES", true, 1, false)),
        ("cancel_historical_data", |c, id| c.cancel_historical_data(id)),
        ("req_head_time_stamp", |c, id| c.req_head_time_stamp(id, &spy(), "TRADES", true, 1)),
        ("cancel_head_time_stamp", |c, id| c.cancel_head_time_stamp(id)),
        ("req_contract_details", |c, id| c.req_contract_details(id, &spy())),
        ("req_matching_symbols", |c, id| c.req_matching_symbols(id, "SP")),
        ("req_scanner_subscription", |c, id| c.req_scanner_subscription(id, "STK", "STK.US", "TOP_PERC_GAIN", 10)),
        ("cancel_scanner_subscription", |c, id| c.cancel_scanner_subscription(id)),
        ("req_historical_news", |c, id| c.req_historical_news(id, 756733, "BRFG", "", "", 10)),
        ("req_news_article", |c, id| c.req_news_article(id, "BRFG", "BRFG$1")),
        ("req_fundamental_data", |c, id| c.req_fundamental_data(id, &spy(), "ReportSnapshot")),
        ("cancel_fundamental_data", |c, id| c.cancel_fundamental_data(id)),
        ("req_histogram_data", |c, id| c.req_histogram_data(id, &spy(), true, "3 days")),
        ("cancel_histogram_data", |c, id| c.cancel_histogram_data(id)),
        ("req_historical_ticks", |c, id| c.req_historical_ticks(id, &spy(), "", "", 100, "TRADES", true)),
        ("req_historical_schedule", |c, id| c.req_historical_schedule(id, &spy(), "", "1 D", true)),
        ("req_mkt_depth", |c, id| c.req_mkt_depth(id, &spy(), 5, false)),
        ("cancel_mkt_depth", |c, id| c.cancel_mkt_depth(id)),
        ("req_real_time_bars", |c, id| c.req_real_time_bars(id, &spy(), 5, "TRADES", true)),
        ("cancel_real_time_bars", |c, id| c.cancel_real_time_bars(id)),
    ];
    for (name, call) in calls {
        for bad in [u32::MAX as i64 + 1, -1] {
            let (client, rx, _shared) = test_client();
            let err = match call(&client, bad) {
                Err(e) => e,
                Ok(()) => panic!("{name}({bad}) must be refused"),
            };
            assert!(err.contains("req_id"), "{name}: the error names the field: {err}");
            assert!(rx.try_recv().is_err(), "{name}: and nothing reaches the wire");
        }
        // The largest id the wire can carry is still a request, not an error.
        let (client, rx, _shared) = test_client();
        if let Err(e) = call(&client, u32::MAX as i64) {
            panic!("{name}: the largest carryable id must still request: {e}");
        }
        assert!(rx.try_recv().is_ok(), "{name}: and it reaches the wire");
    }
}

#[test]
fn cancel_historical_data_sends_cancel() {
    let (client, rx, _shared) = test_client();
    client.cancel_historical_data(5).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::CancelHistorical { req_id: 5 }));
}

#[test]
fn req_head_time_stamp_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_head_time_stamp(10, &spy(), "TRADES", true, 1).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchHeadTimestamp { req_id, con_id, what_to_show, use_rth } => {
            assert_eq!(req_id, 10);
            assert_eq!(con_id, 756733);
            assert_eq!(what_to_show, "TRADES");
            assert!(use_rth);
        }
        _ => panic!("expected FetchHeadTimestamp"),
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Contract details
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_contract_details_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_contract_details(7, &spy()).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchContractDetails { req_id, con_id, .. } => {
            assert_eq!(req_id, 7);
            assert_eq!(con_id, 756733);
        }
        _ => panic!("expected FetchContractDetails"),
    }
}

#[test]
fn req_contract_details_forwards_filter_fields() {
    // ibx#229 / ib-agent#171: a by-symbol lookup must carry the disambiguation
    // filters (primary exchange, local symbol, expiry/strike/right, multiplier,
    // trading class) instead of dropping them.
    let (client, rx, _shared) = test_client();
    let contract = Contract {
        con_id: 0, symbol: "AAPL".into(), sec_type: "OPT".into(),
        exchange: "SMART".into(), currency: "USD".into(),
        primary_exchange: "NASDAQ".into(),
        local_symbol: "AAPL  260808C00250000".into(),
        last_trade_date_or_contract_month: "202608".into(),
        strike: 250.0,
        right: "C".into(),
        multiplier: "100".into(),
        trading_class: "AAPL".into(),
        ..Default::default()
    };
    client.req_contract_details(9, &contract).unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::FetchContractDetails { req_id, con_id, filters, .. } => {
            assert_eq!(req_id, 9);
            assert_eq!(con_id, 0);
            assert_eq!(filters.primary_exchange, "NASDAQ");
            assert_eq!(filters.local_symbol, "AAPL  260808C00250000");
            assert_eq!(filters.last_trade_date_or_contract_month, "202608");
            assert_eq!(filters.strike, 250.0);
            assert_eq!(filters.right, "C");
            assert_eq!(filters.multiplier, "100");
            assert_eq!(filters.trading_class, "AAPL");
        }
        cmd => panic!("expected FetchContractDetails, got {cmd:?}"),
    }
}

#[test]
fn req_contract_details_forwards_identifier_lookup() {
    // ibx#229 / ib-agent#174: an identifier lookup (ISIN) must carry secId and
    // secIdType through to the fetch command.
    let (client, rx, _shared) = test_client();
    let contract = Contract {
        con_id: 0, sec_type: "STK".into(), exchange: "SMART".into(), currency: "USD".into(),
        sec_id: "US0378331005".into(), sec_id_type: "ISIN".into(),
        ..Default::default()
    };
    client.req_contract_details(11, &contract).unwrap();
    match rx.try_recv().unwrap() {
        ControlCommand::FetchContractDetails { filters, .. } => {
            assert_eq!(filters.sec_id, "US0378331005");
            assert_eq!(filters.sec_id_type, "ISIN");
        }
        cmd => panic!("expected FetchContractDetails, got {cmd:?}"),
    }
}

#[test]
fn req_matching_symbols_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_matching_symbols(8, "AAPL").unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchMatchingSymbols { req_id, pattern } => {
            assert_eq!(req_id, 8);
            assert_eq!(pattern, "AAPL");
        }
        _ => panic!("expected FetchMatchingSymbols"),
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Positions
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_positions_delivers_via_wrapper() {
    let (client, _rx, shared) = test_client();
    shared.portfolio.set_position_info(PositionInfo { con_id: 265598, position: 100, avg_cost: 150 * PRICE_SCALE, ..Default::default() });
    shared.portfolio.set_position_info(PositionInfo { con_id: 756733, position: -50, avg_cost: 400 * PRICE_SCALE, ..Default::default() });
    let mut w = RecordingWrapper::default();
    client.req_positions(&mut w);
    let positions: Vec<_> = w.events.iter().filter(|e| e.starts_with("position:")).collect();
    assert_eq!(positions.len(), 2);
    assert!(w.events.last().unwrap() == "position_end");
}

#[test]
fn req_positions_empty_still_calls_position_end() {
    let (client, _rx, _shared) = test_client();
    let mut w = RecordingWrapper::default();
    client.req_positions(&mut w);
    assert_eq!(w.events, vec!["position_end"]);
}

// ═══════════════════════════════════════════════════════════════════
//  Scanner
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_scanner_parameters_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_scanner_parameters().unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::FetchScannerParams));
}

#[test]
fn req_scanner_subscription_sends_subscribe() {
    let (client, rx, _shared) = test_client();
    client.req_scanner_subscription(3, "STK", "STK.US.MAJOR", "TOP_PERC_GAIN", 25).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::SubscribeScanner { req_id, scan_code, max_items, .. } => {
            assert_eq!(req_id, 3);
            assert_eq!(scan_code, "TOP_PERC_GAIN");
            assert_eq!(max_items, 25);
        }
        _ => panic!("expected SubscribeScanner"),
    }
}

#[test]
fn cancel_scanner_subscription_sends_cancel() {
    let (client, rx, _shared) = test_client();
    client.cancel_scanner_subscription(3).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::CancelScanner { req_id: 3 }));
}

// ═══════════════════════════════════════════════════════════════════
//  News
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_historical_news_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_historical_news(4, 265598, "BRFG", "2026-01-01", "2026-03-01", 10).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchHistoricalNews { req_id, con_id, provider_codes, max_results, .. } => {
            assert_eq!(req_id, 4);
            assert_eq!(con_id, 265598);
            assert_eq!(provider_codes, "BRFG");
            assert_eq!(max_results, 10);
        }
        _ => panic!("expected FetchHistoricalNews"),
    }
}

#[test]
fn req_news_article_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_news_article(5, "BRFG", "BRFG$12345").unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchNewsArticle { req_id, provider_code, article_id } => {
            assert_eq!(req_id, 5);
            assert_eq!(provider_code, "BRFG");
            assert_eq!(article_id, "BRFG$12345");
        }
        _ => panic!("expected FetchNewsArticle"),
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Fundamental data
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_fundamental_data_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_fundamental_data(6, &spy(), "ReportSnapshot").unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchFundamentalData { req_id, report_type, .. } => {
            assert_eq!(req_id, 6);
            assert_eq!(report_type, "ReportSnapshot");
        }
        _ => panic!("expected FetchFundamentalData"),
    }
}

#[test]
fn cancel_fundamental_data_sends_cancel() {
    let (client, rx, _shared) = test_client();
    client.cancel_fundamental_data(6).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::CancelFundamentalData { req_id: 6 }));
}

// ═══════════════════════════════════════════════════════════════════
//  Histogram
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_histogram_data_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_histogram_data(7, &spy(), true, "1 week").unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchHistogramData { req_id, use_rth, period, .. } => {
            assert_eq!(req_id, 7);
            assert!(use_rth);
            assert_eq!(period, "1 week");
        }
        _ => panic!("expected FetchHistogramData"),
    }
}

#[test]
fn cancel_histogram_data_sends_cancel() {
    let (client, rx, _shared) = test_client();
    client.cancel_histogram_data(7).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::CancelHistogramData { req_id: 7 }));
}

// ═══════════════════════════════════════════════════════════════════
//  Historical ticks
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_historical_ticks_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_historical_ticks(8, &spy(), "20260101 09:30:00", "", 1000, "TRADES", true).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchHistoricalTicks { req_id, con_id, number_of_ticks, what_to_show, .. } => {
            assert_eq!(req_id, 8);
            assert_eq!(con_id, 756733);
            assert_eq!(number_of_ticks, 1000);
            assert_eq!(what_to_show, "TRADES");
        }
        _ => panic!("expected FetchHistoricalTicks"),
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Real-time bars
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_real_time_bars_sends_subscribe() {
    let (client, rx, _shared) = test_client();
    client.req_real_time_bars(9, &spy(), 5, "TRADES", true).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::SubscribeRealTimeBar { req_id, con_id, what_to_show, use_rth, .. } => {
            assert_eq!(req_id, 9);
            assert_eq!(con_id, 756733);
            assert_eq!(what_to_show, "TRADES");
            assert!(use_rth);
        }
        _ => panic!("expected SubscribeRealTimeBar"),
    }
}

#[test]
fn cancel_real_time_bars_sends_cancel() {
    let (client, rx, _shared) = test_client();
    client.cancel_real_time_bars(9).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::CancelRealTimeBar { req_id: 9 }));
}

// ═══════════════════════════════════════════════════════════════════
//  Historical schedule
// ═══════════════════════════════════════════════════════════════════

#[test]
fn req_historical_schedule_sends_fetch() {
    let (client, rx, _shared) = test_client();
    client.req_historical_schedule(11, &spy(), "20260101 16:00:00", "1 D", true).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchHistoricalSchedule { req_id, con_id, use_rth, .. } => {
            assert_eq!(req_id, 11);
            assert_eq!(con_id, 756733);
            assert!(use_rth);
        }
        _ => panic!("expected FetchHistoricalSchedule"),
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Quote / Account accessors
// ═══════════════════════════════════════════════════════════════════

#[test]
fn quote_escape_hatch() {
    let shared = Arc::new(SharedState::new());
    let q = Quote { bid: 200 * PRICE_SCALE, ..Default::default() };
    shared.market.push_quote(0, &q);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let handle = std::thread::spawn(|| {});
    let client = EClient::from_parts(shared, tx, handle, "DU123".into());

    client.core.req_to_instrument.lock().unwrap().insert(5, 0);

    let quote = client.quote(5).unwrap();
    assert_eq!(quote.bid, 200 * PRICE_SCALE);
    assert!(client.quote(99).is_none());
}

// ibx#158: RTT is None until measured, then reflects the stored sample;
// req_ping goes out as a Ping command.
#[test]
fn rtt_none_until_measured_and_ping_sends_command() {
    let (client, rx, shared) = test_client();
    assert_eq!(client.last_rtt(), None);
    client.req_ping().unwrap();
    assert!(matches!(rx.try_recv().unwrap(), ControlCommand::Ping));

    shared.set_ccp_rtt(std::time::Duration::from_micros(1234));
    assert_eq!(client.last_rtt(), Some(std::time::Duration::from_micros(1234)));
}

#[test]
fn quote_by_instrument_direct() {
    let shared = Arc::new(SharedState::new());
    let q = Quote { ask: 300 * PRICE_SCALE, ..Default::default() };
    shared.market.push_quote(2, &q);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let handle = std::thread::spawn(|| {});
    let client = EClient::from_parts(shared, tx, handle, "DU123".into());

    let quote = client.quote_by_instrument(2).expect("registered id");
    assert_eq!(quote.ask, 300 * PRICE_SCALE);

    // ibx#234: an out-of-range id is a caller error, not a panic across
    // the language boundary.
    assert!(client.quote_by_instrument(999).is_none());
}

#[test]
fn account_reads_shared_state() {
    let (_client, _rx, shared) = test_client();
    let a = AccountState { net_liquidation: 100_000 * PRICE_SCALE, ..Default::default() };
    shared.portfolio.set_account(&a);
    let (client2, _rx2, _) = {
        let (tx, rx) = crossbeam_channel::unbounded();
        let handle = std::thread::spawn(|| {});
        (EClient::from_parts(shared.clone(), tx, handle, "DU123".into()), rx, shared.clone())
    };
    assert_eq!(client2.account().net_liquidation, 100_000 * PRICE_SCALE);
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — fills, order updates, cancel rejects (existing)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_fill() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 42, side: Side::Buy,
        price: 150 * PRICE_SCALE, qty: 100, remaining: 0,
        commission: PRICE_SCALE, timestamp_ns: 123456789,
        cum_qty: 100, avg_price: 150 * PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("order_status:42:Filled")));
    assert!(w.events.iter().any(|e| e.starts_with("exec_details:-1:BOT:100")));
}

#[test]
fn process_msgs_dispatches_partial_fill() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 42, side: Side::Buy,
        price: 150 * PRICE_SCALE, qty: 50, remaining: 50,
        commission: PRICE_SCALE, timestamp_ns: 123456789,
        cum_qty: 50, avg_price: 150 * PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("order_status:42:PartiallyFilled")));
}

/// IB's `orderStatus` contract: `filled` is cumulative across the order and
/// `avgFillPrice` is volume-weighted across every print, while `lastFillPrice`
/// is this print. Reporting the print's own size and price as the cumulative
/// pair means an order that fills in more than one print never reports its
/// true average, and `filled` never reaches the order quantity.
#[test]
fn order_status_reports_the_order_total_not_the_last_print() {
    let (client, _rx, shared) = test_client();
    // Second print: 100 more at 151, taking the order to 200 filled at an
    // average of 150.50, with 100 still working.
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 42, side: Side::Buy,
        price: 151 * PRICE_SCALE, qty: 100, remaining: 100,
        commission: PRICE_SCALE, timestamp_ns: 0,
        cum_qty: 200, avg_price: 150 * PRICE_SCALE + PRICE_SCALE / 2,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);

    let status = w.events.iter().find(|e| e.starts_with("order_status:42:"))
        .expect("order_status was dispatched");
    assert_eq!(
        status, "order_status:42:PartiallyFilled:200:100:150.5",
        "filled and avgFillPrice must describe the order, not the print",
    );
}

#[test]
fn process_msgs_dispatches_sell_fill() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 43, side: Side::Sell,
        price: 151 * PRICE_SCALE, qty: 100, remaining: 0,
        commission: PRICE_SCALE, timestamp_ns: 0,
        cum_qty: 100, avg_price: 151 * PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("exec_details:-1:SLD:100")));
}

#[test]
fn process_msgs_dispatches_order_updates() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_order_update(OrderUpdate {
        order_id: 43, instrument: 0, status: OrderStatus::Submitted,
        filled_qty: 0.0, remaining_qty: 100.0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    shared.orders.push_order_update(OrderUpdate {
        order_id: 44, instrument: 0, status: OrderStatus::Cancelled,
        filled_qty: 0.0, remaining_qty: 100.0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    shared.orders.push_order_update(OrderUpdate {
        order_id: 45, instrument: 0, status: OrderStatus::Rejected,
        filled_qty: 0.0, remaining_qty: 100.0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("order_status:43:Submitted")));
    assert!(w.events.iter().any(|e| e.starts_with("order_status:44:Cancelled")));
    assert!(w.events.iter().any(|e| e.starts_with("order_status:45:Inactive")));
}

/// ibx#250: a parked (39=I) order's reason reaches the caller through
/// Wrapper::error, on top of the order_status "Inactive" callback above —
/// ibapi has no callback dedicated to "order held with reason".
#[test]
fn process_msgs_dispatches_inactive_reason_as_error() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_order_inactive(46, 399, "Order held pending margin check".into());
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "error:46:399:Order held pending margin check"));
}

/// ibx#250 end-to-end: a genuinely-Inactive order dispatched through the real
/// `process_msgs` path (not a direct `ClientCore` call) stays in the
/// open-order snapshot, while a Rejected one — which stringifies to the same
/// "Inactive" — does not resurrect into it.
#[test]
fn process_msgs_then_open_orders_admits_inactive_excludes_rejected() {
    let (client, _rx, shared) = test_client();
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 150.0, ..Default::default()
    };
    client.place_order(82, &spy(), &order).unwrap();
    client.place_order(83, &spy(), &order).unwrap();

    shared.orders.push_order_update(OrderUpdate {
        order_id: 82, instrument: 0, status: OrderStatus::Inactive,
        filled_qty: 0.0, remaining_qty: 100.0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    shared.orders.push_order_update(OrderUpdate {
        order_id: 83, instrument: 0, status: OrderStatus::Rejected,
        filled_qty: 0.0, remaining_qty: 100.0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);

    w.events.clear();
    client.req_all_open_orders(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("open_order:82:")),
        "genuinely-inactive order must remain in the open-order snapshot after dispatch");
    assert!(!w.events.iter().any(|e| e.starts_with("open_order:83:")),
        "rejected order must not resurrect into the open-order snapshot after dispatch");
}

#[test]
fn process_msgs_dispatches_cancel_reject_type_1() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_cancel_reject(CancelReject {
        order_id: 44, instrument: 0, reject_type: 1, reason_code: 0, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("error:44:202:")));
}

#[test]
fn process_msgs_dispatches_cancel_reject_type_2() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_cancel_reject(CancelReject {
        order_id: 44, instrument: 0, reject_type: 2, reason_code: 5, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("error:44:10147:")));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — quote polling
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_quotes_on_change() {
    let (client, _rx, shared) = test_client();
    let mut q = Quote { bid: 150 * PRICE_SCALE, ask: 151 * PRICE_SCALE, ..Default::default() };
    shared.market.push_quote(0, &q);

    client.core.req_to_instrument.lock().unwrap().insert(1, 0);
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:1:150")));
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:2:151")));

    // Second call — no changes, no events
    w.events.clear();
    client.process_msgs(&mut w);
    assert!(w.events.is_empty(), "no events on unchanged quotes");

    // Now change bid
    q.bid = 149 * PRICE_SCALE;
    shared.market.push_quote(0, &q);
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:1:149")));
}

#[test]
fn process_msgs_dispatches_all_quote_fields() {
    let (client, _rx, shared) = test_client();
    let q = Quote {
        bid: 150 * PRICE_SCALE, ask: 151 * PRICE_SCALE, last: 150_50000000,
        bid_size: 1000 * QTY_SCALE, ask_size: 2000 * QTY_SCALE,
        last_size: 500 * QTY_SCALE,
        high: 155 * PRICE_SCALE, low: 148 * PRICE_SCALE,
        volume: 10_000 * QTY_SCALE,
        close: 149 * PRICE_SCALE, open: 150 * PRICE_SCALE,
        timestamp_ns: 1234567890,
        bid_exch_mask: 0, ask_exch_mask: 0, last_exch_mask: 0,
    };
    shared.market.push_quote(0, &q);

    client.core.req_to_instrument.lock().unwrap().insert(1, 0);
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);

    // Should have tick_price for: bid(1), ask(2), last(4), high(6), low(7), close(9), open(14)
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:1:")));   // bid
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:2:")));   // ask
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:4:")));   // last
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:6:")));   // high
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:7:")));   // low
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:9:")));   // close
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:14:"))); // open
    // tick_size for: bid_size(0), ask_size(3), last_size(5), volume(8).
    // Assert the delivered quantity, not just that a tick appeared — the
    // scaling defect in ibx#287 fired every one of these with a value four
    // orders of magnitude off, and a `starts_with` check passed throughout.
    let delivered = |prefix: &str| -> Option<f64> {
        w.events.iter().find(|e| e.starts_with(prefix))
            .and_then(|e| e.rsplit(':').next())
            .and_then(|v| v.parse().ok())
    };
    assert_eq!(delivered("tick_size:1:0:"), Some(1000.0), "bid_size");
    assert_eq!(delivered("tick_size:1:3:"), Some(2000.0), "ask_size");
    assert_eq!(delivered("tick_size:1:5:"), Some(500.0), "last_size");
    assert_eq!(delivered("tick_size:1:8:"), Some(10_000.0), "volume");
}

#[test]
fn process_msgs_multiple_instruments_independent() {
    let (client, _rx, shared) = test_client();
    let q0 = Quote { bid: 150 * PRICE_SCALE, ..Default::default() };
    shared.market.push_quote(0, &q0);
    let q1 = Quote { bid: 400 * PRICE_SCALE, ..Default::default() };
    shared.market.push_quote(1, &q1);

    client.core.req_to_instrument.lock().unwrap().insert(1, 0);
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);
    client.core.req_to_instrument.lock().unwrap().insert(2, 1);
    client.core.instrument_to_req.lock().unwrap().insert(1, 2);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:1:150")));
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:2:1:400")));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — TBT trades / quotes
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_tbt_trade() {
    let (client, _rx, shared) = test_client();
    client.core.instrument_to_req.lock().unwrap().insert(0, 10);
    shared.market.push_tbt_trade(TbtTrade {
        instrument: 0, price: 150 * PRICE_SCALE, size: 100,
        timestamp: 1700000000, exchange: "ARCA".into(), conditions: "".into(),
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tbt_last:10:1:1700000000:150:100:ARCA")));
}

#[test]
fn process_msgs_dispatches_tbt_quote() {
    let (client, _rx, shared) = test_client();
    client.core.instrument_to_req.lock().unwrap().insert(0, 10);
    shared.market.push_tbt_quote(TbtQuote {
        instrument: 0, bid: 150 * PRICE_SCALE, ask: 151 * PRICE_SCALE,
        bid_size: 1000, ask_size: 2000, timestamp: 1700000000,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tbt_bidask:10:1700000000:150:151:1000:2000")));
}

#[test]
fn process_msgs_tbt_unknown_instrument_uses_neg1() {
    let (client, _rx, shared) = test_client();
    // No mapping for instrument 5
    shared.market.push_tbt_trade(TbtTrade {
        instrument: 5, price: 150 * PRICE_SCALE, size: 100,
        timestamp: 0, exchange: "".into(), conditions: "".into(),
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tbt_last:-1:")));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — tick news
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_tick_news() {
    let (client, _rx, shared) = test_client();
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);
    shared.market.push_tick_news(TickNews {
        instrument: 0,
        provider_code: "BRFG".into(), article_id: "BRFG$123".into(),
        headline: "AAPL beats".into(), timestamp: 1700000000,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "tick_news:BRFG:BRFG$123:AAPL beats"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — news bulletins
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_news_bulletin() {
    let (client, _rx, shared) = test_client();
    client.req_news_bulletins(true);
    shared.market.push_news_bulletin(NewsBulletin {
        msg_id: 1, msg_type: 1,
        message: "Exchange notice".into(), exchange: "NYSE".into(),
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "news_bulletin:1:1:Exchange notice:NYSE"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — what-if
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_what_if() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_what_if(WhatIfResponse {
        order_id: 42, instrument: 0,
        init_margin_before: 0, maint_margin_before: 0,
        equity_with_loan_before: 0,
        init_margin_after: 5000 * PRICE_SCALE,
        maint_margin_after: 3000 * PRICE_SCALE,
        equity_with_loan_after: 0,
        commission: PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("order_status:42:PreSubmitted")));
}

/// Regression: what-if dispatch must populate all 8 OrderState fields and call
/// open_order BEFORE order_status, matching official ibapi contract.
#[test]
fn process_msgs_what_if_emits_full_order_state() {
    let (client, _rx, shared) = test_client();
    // Distinct values per field so any swap/typo is detectable.
    shared.orders.push_what_if(WhatIfResponse {
        order_id: 7, instrument: 0,
        init_margin_before:    100 * PRICE_SCALE,
        maint_margin_before:   200 * PRICE_SCALE,
        equity_with_loan_before: 300 * PRICE_SCALE,
        init_margin_after:     400 * PRICE_SCALE,
        maint_margin_after:    500 * PRICE_SCALE,
        equity_with_loan_after: 600 * PRICE_SCALE,
        commission:            7 * PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);

    let open_idx = w.events.iter().position(|e| e.starts_with("open_order:7:"))
        .expect("open_order callback missing for what-if");
    let status_idx = w.events.iter().position(|e| e.starts_with("order_status:7:PreSubmitted"))
        .expect("order_status callback missing for what-if");
    assert!(open_idx < status_idx, "open_order must be emitted before order_status");

    let evt = &w.events[open_idx];
    // status, all 9 margin fields (before/change/after × init/maint/eql), commission.
    assert!(evt.contains(":PreSubmitted:"), "status field missing: {evt}");
    assert!(evt.contains("initB=100.00:initC=300.00:initA=400.00"), "init margin wrong: {evt}");
    assert!(evt.contains("maintB=200.00:maintC=300.00:maintA=500.00"), "maint margin wrong: {evt}");
    assert!(evt.contains("eqlB=300.00:eqlC=300.00:eqlA=600.00"), "equity-with-loan wrong: {evt}");
    assert!(evt.contains("comm=7"), "commission wrong: {evt}");
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — historical data
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_historical_data() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_historical_data(5, HistoricalResponse {
        query_id: String::new(), timezone: String::new(),
        bars: vec![
            HistoricalBar { time: "20260101".into(), open: 100.0, high: 105.0, low: 99.0, close: 103.0, volume: 1000, wap: 102.0, count: 50 },
            HistoricalBar { time: "20260102".into(), open: 103.0, high: 108.0, low: 102.0, close: 107.0, volume: 1200, wap: 105.0, count: 60 },
        ],
        is_complete: true,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "historical_data:5:20260101"));
    assert!(w.events.iter().any(|e| e == "historical_data:5:20260102"));
    assert!(w.events.iter().any(|e| e == "historical_data_end:5"));
}

#[test]
fn process_msgs_historical_data_incomplete_no_end() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_historical_data(5, HistoricalResponse {
        query_id: String::new(), timezone: String::new(),
        bars: vec![
            HistoricalBar { time: "20260101".into(), open: 100.0, high: 105.0, low: 99.0, close: 103.0, volume: 1000, wap: 102.0, count: 50 },
        ],
        is_complete: false,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "historical_data:5:20260101"));
    assert!(!w.events.iter().any(|e| e == "historical_data_end:5"), "no end for incomplete");
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — head timestamps
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_head_timestamp() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_head_timestamp(10, HeadTimestampResponse { head_timestamp: "20200101".into(), timezone: String::new() });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "head_timestamp:10:20200101"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — contract details
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_contract_details() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_contract_details(7, ContractDefinition {
        con_id: 265598, symbol: "AAPL".into(), sec_type: SecurityType::Stock,
        exchange: "SMART".into(), primary_exchange: "NASDAQ".into(),
        currency: "USD".into(), local_symbol: "AAPL".into(),
        trading_class: "AAPL".into(), long_name: "Apple Inc".into(),
        min_tick: 0.01, multiplier: 1.0, valid_exchanges: vec!["SMART".into()],
        order_types: vec!["LMT".into()], market_rule_id: Some(26),
        last_trade_date: String::new(), right: None, strike: 0.0,
        ..Default::default()
    });
    shared.reference.push_contract_details_end(7);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "contract_details:7:AAPL"));
    assert!(w.events.iter().any(|e| e == "contract_details_end:7"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — matching symbols
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_symbol_samples() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_matching_symbols(8, vec![
        SymbolMatch {
            con_id: 265598, symbol: "AAPL".into(), sec_type: SecurityType::Stock,
            currency: "USD".into(), primary_exchange: "NASDAQ".into(),
            description: "Apple Inc".into(), derivative_types: vec!["OPT".into()],
        },
    ]);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "symbol_samples:8:1"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — scanner
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_scanner_params() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_scanner_params("<scanner>XML</scanner>".into());
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "scanner_parameters"));
}

#[test]
fn process_msgs_dispatches_scanner_data() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_scanner_data(3, ScannerResult {
        con_ids: vec![265598, 756733],
        entries: vec![
            ScannerEntry { con_id: 265598, ..Default::default() },
            ScannerEntry { con_id: 756733, ..Default::default() },
        ],
        scan_time: "2026-03-13".into(),
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "scanner_data:3:0"));
    assert!(w.events.iter().any(|e| e == "scanner_data:3:1"));
    assert!(w.events.iter().any(|e| e == "scanner_data_end:3"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — news
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_historical_news() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_historical_news(4, vec![
        NewsHeadline {
            time: "2026-01-15".into(), provider_code: "BRFG".into(),
            article_id: "BRFG$100".into(), headline: "Earnings beat".into(),
        },
        NewsHeadline {
            time: "2026-01-16".into(), provider_code: "BRFG".into(),
            article_id: "BRFG$101".into(), headline: "Guidance raised".into(),
        },
    ], false);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "historical_news:4:BRFG:BRFG$100:Earnings beat"));
    assert!(w.events.iter().any(|e| e == "historical_news:4:BRFG:BRFG$101:Guidance raised"));
    assert!(w.events.iter().any(|e| e == "historical_news_end:4:false"));
}

#[test]
fn process_msgs_dispatches_news_article() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_news_article(5, 0, "Full article text here".into());
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "news_article:5:0:Full article text here"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — fundamental data
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_fundamental_data() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_fundamental_data(6, "<report>data</report>".into());
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "fundamental_data:6"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — histogram data
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_histogram_data() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_histogram_data(7, vec![
        HistogramEntry { price: 150.0, count: 500 },
        HistogramEntry { price: 151.0, count: 300 },
    ]);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "histogram_data:7:2"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — historical ticks
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_historical_ticks() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_historical_ticks(8, HistoricalTickData::Midpoint(vec![
        HistoricalTickMidpoint { time: "2026-01-15 09:30:00".into(), price: 150.5 },
    ]), "MIDPOINT".into(), true);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "historical_ticks:8:true"));
}

/// Regression: historical-tick variants must route to their variant-specific
/// callback (iso ibapi). Was: all three flowed through historical_ticks().
#[test]
fn process_msgs_routes_historical_tick_variants() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_historical_ticks(10, HistoricalTickData::Last(vec![
        HistoricalTickLast {
            time: "2026-01-15 09:30:00".into(), price: 150.5, size: 100,
            exchange: "ARCA".into(), special_conditions: "".into(),
        },
    ]), "TRADES".into(), true);
    shared.reference.push_historical_ticks(11, HistoricalTickData::BidAsk(vec![
        HistoricalTickBidAsk {
            time: "2026-01-15 09:30:01".into(), bid_price: 150.4, ask_price: 150.6,
            bid_size: 200, ask_size: 300,
        },
    ]), "BID_ASK".into(), true);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);

    assert!(w.events.iter().any(|e| e == "historical_ticks_last:10:true"),
        "Last variant must route to historical_ticks_last; got {:?}", w.events);
    assert!(w.events.iter().any(|e| e == "historical_ticks_bid_ask:11:true"),
        "BidAsk variant must route to historical_ticks_bid_ask; got {:?}", w.events);
    // Generic historical_ticks should NOT fire for Last or BidAsk.
    assert!(!w.events.iter().any(|e| e == "historical_ticks:10:true"));
    assert!(!w.events.iter().any(|e| e == "historical_ticks:11:true"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — real-time bars
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_real_time_bar() {
    let (client, _rx, shared) = test_client();
    shared.market.push_real_time_bar(9, RealTimeBar {
        timestamp: 1700000000, open: 150.0, high: 151.0,
        low: 149.0, close: 150.5, volume: 1000.0, wap: 150.25, count: 50,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("real_time_bar:9:1700000000")));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — historical schedule
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_dispatches_historical_schedule() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_historical_schedule(11, HistoricalScheduleResponse {
        query_id: String::new(),
        timezone: "US/Eastern".into(),
        start_date_time: "20260101".into(),
        end_date_time: "20260102".into(),
        sessions: vec![ScheduleSession {
            ref_date: "20260101".into(),
            open_time: "09:30:00".into(),
            close_time: "16:00:00".into(),
        }],
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "historical_schedule:11:US/Eastern:1"));
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — drain is exhaustive
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_empty_queues_no_events() {
    let (client, _rx, _shared) = test_client();
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.is_empty());
}

#[test]
fn process_msgs_drains_on_first_call_empty_on_second() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 1, side: Side::Buy,
        price: PRICE_SCALE, qty: 1, remaining: 0,
        commission: 0, timestamp_ns: 0,
        cum_qty: 1, avg_price: PRICE_SCALE,
    });
    shared.orders.push_order_update(OrderUpdate {
        order_id: 2, instrument: 0, status: OrderStatus::Submitted,
        filled_qty: 0.0, remaining_qty: 1.0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(!w.events.is_empty());

    w.events.clear();
    client.process_msgs(&mut w);
    // Only quote events might fire (if mapped), but no fills/updates
    let non_tick_events: Vec<_> = w.events.iter()
        .filter(|e| !e.starts_with("tick_price") && !e.starts_with("tick_size"))
        .collect();
    assert!(non_tick_events.is_empty(), "second drain should be empty");
}

// ═══════════════════════════════════════════════════════════════════
//  process_msgs — exec_details uses correct req_id from mapping
// ═══════════════════════════════════════════════════════════════════

#[test]
fn process_msgs_fill_uses_instrument_to_req_mapping() {
    let (client, _rx, shared) = test_client();
    client.core.instrument_to_req.lock().unwrap().insert(0, 42);
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 1, side: Side::Buy,
        price: PRICE_SCALE, qty: 100, remaining: 0,
        commission: 0, timestamp_ns: 0,
        cum_qty: 100, avg_price: PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    // exec_details should use req_id=42 (not -1)
    assert!(w.events.iter().any(|e| e.starts_with("exec_details:42:")));
}

// ── Order modification edge cases ─────────────────────────────────

#[test]
fn modify_limit_order_price_via_resubmit() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 150.0, ..Default::default()
    };
    client.place_order(80, &spy(), &order).unwrap();
    while rx.try_recv().is_ok() {}

    let modified = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 152.0, ..Default::default()
    };
    client.place_order(80, &spy(), &modified).unwrap();

    let mut found = false;
    while let Ok(cmd) = rx.try_recv() {
        if let ControlCommand::Order(OrderRequest::Modify { order_id: 80, price, qty, .. }) = cmd {
            assert_eq!(price, (152.0 * PRICE_SCALE_F) as i64);
            assert_eq!(qty, 100);
            found = true;
        }
    }
    assert!(found, "Resubmit with same orderId should emit Modify");
}

#[test]
fn modify_order_before_ack_no_panic() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    for price in 0..10 {
        let order = Order {
            action: "BUY".into(), total_quantity: 100.0,
            order_type: "LMT".into(), lmt_price: 150.0 + price as f64,
            ..Default::default()
        };
        let _ = client.place_order(42, &spy(), &order);
    }
    let mut count = 0;
    while rx.try_recv().is_ok() { count += 1; }
    assert!(count >= 10, "All modify attempts should send commands, got {count}");
}

#[test]
fn cancel_during_modify_no_panic() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 150.0, ..Default::default()
    };
    client.place_order(99, &spy(), &order).unwrap();
    let modified = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 151.0, ..Default::default()
    };
    client.place_order(99, &spy(), &modified).unwrap();
    client.cancel_order(99, "").unwrap();

    let mut has_cancel = false;
    while let Ok(cmd) = rx.try_recv() {
        if matches!(cmd, ControlCommand::Order(OrderRequest::Cancel { order_id: 99 })) {
            has_cancel = true;
        }
    }
    assert!(has_cancel, "Cancel command should be sent");
}

#[test]
fn modify_filled_order_receives_cancel_reject() {
    let (client, _rx, shared) = test_client();
    client.map_req_instrument(1, 0);
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 120, side: Side::Buy,
        price: 150 * PRICE_SCALE, qty: 100, remaining: 0,
        commission: 0, timestamp_ns: 1000,
        cum_qty: 100, avg_price: 150 * PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("order_status:120:Filled")));

    shared.orders.push_cancel_reject(CancelReject {
        order_id: 120, instrument: 0, reject_type: 2, reason_code: 0, timestamp_ns: 2000,
    });
    w.events.clear();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("error:120:")),
        "Modify reject should generate error callback, got: {:?}", w.events);
}

#[test]
fn rapid_modify_multiple_prices_no_crash() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    for i in 0..50 {
        let order = Order {
            action: "BUY".into(), total_quantity: 100.0,
            order_type: "LMT".into(), lmt_price: 100.0 + i as f64 * 0.01,
            ..Default::default()
        };
        let _ = client.place_order(77, &spy(), &order);
    }
    let mut order_count = 0;
    while let Ok(cmd) = rx.try_recv() {
        if matches!(cmd, ControlCommand::Order(_)) { order_count += 1; }
    }
    assert_eq!(order_count, 50, "All 50 modify commands should be sent");
}

#[test]
fn modify_tif_day_to_gtc_via_resubmit() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 150.0,
        tif: "DAY".into(), ..Default::default()
    };
    client.place_order(88, &spy(), &order).unwrap();
    while rx.try_recv().is_ok() {}

    let modified = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 150.0,
        tif: "GTC".into(), ..Default::default()
    };
    client.place_order(88, &spy(), &modified).unwrap();

    let mut found_modify = false;
    while let Ok(cmd) = rx.try_recv() {
        if let ControlCommand::Order(OrderRequest::Modify { order_id: 88, price, qty, .. }) = cmd {
            assert_eq!(price, (150.0 * PRICE_SCALE_F) as i64);
            assert_eq!(qty, 100);
            found_modify = true;
        }
    }
    assert!(found_modify, "Resubmit with same orderId should emit Modify");
}

#[test]
fn modify_price_and_qty_simultaneously() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 150.0, ..Default::default()
    };
    client.place_order(55, &spy(), &order).unwrap();
    while rx.try_recv().is_ok() {}

    let modified = Order {
        action: "BUY".into(), total_quantity: 200.0,
        order_type: "LMT".into(), lmt_price: 148.0, ..Default::default()
    };
    client.place_order(55, &spy(), &modified).unwrap();

    let mut found = false;
    while let Ok(cmd) = rx.try_recv() {
        if let ControlCommand::Order(OrderRequest::Modify { order_id: 55, qty, price, .. }) = cmd {
            assert_eq!(qty, 200);
            assert_eq!(price, (148.0 * PRICE_SCALE_F) as i64);
            found = true;
        }
    }
    assert!(found, "Resubmit with same orderId should emit Modify with new price and qty");
}

#[test]
fn modify_order_type_lmt_to_stp() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "LMT".into(), lmt_price: 150.0, ..Default::default()
    };
    client.place_order(66, &spy(), &order).unwrap();
    while rx.try_recv().is_ok() {}

    let modified = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "STP".into(), aux_price: 149.0, ..Default::default()
    };
    client.place_order(66, &spy(), &modified).unwrap();

    let mut found_modify = false;
    while let Ok(cmd) = rx.try_recv() {
        if matches!(cmd, ControlCommand::Order(OrderRequest::Modify { order_id: 66, .. })) {
            found_modify = true;
        }
    }
    assert!(found_modify, "Resubmit with same orderId should emit Modify");
}

// ── Market data type switching ────────────────────────────────────

#[test]
fn market_data_type_callback_compiles_and_dispatches() {
    struct MarketDataTypeRecorder { events: Vec<(i64, i32)> }
    impl crate::api::wrapper::Wrapper for MarketDataTypeRecorder {
        fn market_data_type(&mut self, req_id: i64, market_data_type: i32) {
            self.events.push((req_id, market_data_type));
        }
    }
    let mut w = MarketDataTypeRecorder { events: vec![] };
    w.market_data_type(1, 1); // Live
    w.market_data_type(1, 2); // Frozen
    w.market_data_type(1, 3); // Delayed
    w.market_data_type(1, 4); // Delayed-Frozen
    assert_eq!(w.events.len(), 4);
    assert_eq!(w.events[0], (1, 1));
    assert_eq!(w.events[3], (1, 4));
}

#[test]
fn quote_dispatch_agnostic_to_data_type() {
    let (client, _rx, shared) = test_client();
    client.map_req_instrument(1, 0);
    let q = Quote { bid: 450 * PRICE_SCALE, ask: 451 * PRICE_SCALE, ..Default::default() };
    shared.market.push_quote(0, &q);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:1:450")));
}

#[test]
fn frozen_stale_quote_no_redispatch() {
    let (client, _rx, shared) = test_client();
    client.map_req_instrument(1, 0);
    let q = Quote { bid: 300 * PRICE_SCALE, ask: 301 * PRICE_SCALE, ..Default::default() };
    shared.market.push_quote(0, &q);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:")));

    shared.market.push_quote(0, &q); // same quote
    w.events.clear();
    client.process_msgs(&mut w);
    let second_count = w.events.iter().filter(|e| e.starts_with("tick_price:1:")).count();
    assert_eq!(second_count, 0, "Identical frozen quote should not re-dispatch");
}

#[test]
fn transition_no_data_to_live_fires_callbacks() {
    let (client, _rx, shared) = test_client();
    client.map_req_instrument(1, 0);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert_eq!(w.events.iter().filter(|e| e.starts_with("tick_price:1:")).count(), 0);

    let q = Quote { bid: 500 * PRICE_SCALE, ask: 501 * PRICE_SCALE, ..Default::default() };
    shared.market.push_quote(0, &q);
    w.events.clear();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:1:500")));
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:2:501")));
}

#[test]
fn partial_quote_update_only_changed_fields_dispatch() {
    let (client, _rx, shared) = test_client();
    client.map_req_instrument(1, 0);
    let mut q = Quote { bid: 100 * PRICE_SCALE, ask: 101 * PRICE_SCALE, ..Default::default() };
    shared.market.push_quote(0, &q);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);

    q.bid = 99 * PRICE_SCALE;
    shared.market.push_quote(0, &q);
    w.events.clear();
    client.process_msgs(&mut w);

    let bid_ticks: Vec<_> = w.events.iter().filter(|e| e.starts_with("tick_price:1:1:")).collect();
    let ask_ticks: Vec<_> = w.events.iter().filter(|e| e.starts_with("tick_price:1:2:")).collect();
    assert!(!bid_ticks.is_empty(), "Changed bid should dispatch");
    assert!(ask_ticks.is_empty(), "Unchanged ask should NOT dispatch");
}

// ═══════════════════════════════════════════════════════════════════
//  Thread lifecycle
// ═══════════════════════════════════════════════════════════════════

#[test]
fn disconnect_joins_thread() {
    let (client, _rx, _shared) = test_client();
    // The test_client helper spawns an empty thread (already exited).
    // disconnect() should join it without hanging.
    client.disconnect();
    assert!(!client.is_connected());
}

#[test]
fn drop_without_disconnect_joins_thread() {
    let (client, _rx, _shared) = test_client();
    // Dropping without explicit disconnect — Drop impl should join.
    drop(client);
    // No hang = success.
}

#[test]
fn disconnect_is_idempotent() {
    let (client, _rx, _shared) = test_client();
    client.disconnect();
    // Second disconnect should not panic (thread already joined).
    client.disconnect();
    assert!(!client.is_connected());
}

#[test]
fn ccp_session_id_matches_shared_reference() {
    let (client, _rx, shared) = test_client();
    assert_eq!(client.ccp_session_id(), shared.reference.ccp_session_id());

    shared.reference.set_ccp_session_id("sid.0001".to_string());
    assert_eq!(client.ccp_session_id(), "sid.0001");
    assert_eq!(client.ccp_session_id(), client.shared.reference.ccp_session_id());
}

#[test]
fn misc_url_lookup_delegates_to_shared() {
    let (client, _rx, shared) = test_client();
    assert!(client.misc_url("region_dam").is_none());

    let mut urls = std::collections::HashMap::new();
    urls.insert("region_dam".to_string(), "api.example.com".to_string());
    shared.reference.set_misc_urls(urls);

    assert_eq!(client.misc_url("region_dam").as_deref(), Some("api.example.com"));
    assert!(client.misc_url("missing").is_none());
}

#[test]
fn session_token_bytes_roundtrip_through_biguint() {
    use num_bigint::BigUint;

    let shared = Arc::new(SharedState::new());
    let (tx, _rx) = crossbeam_channel::unbounded();
    let handle = std::thread::spawn(|| {});
    let mut client = EClient::from_parts(shared, tx, handle, "DU123".into());

    let session_token = BigUint::parse_bytes(
        b"fedcba9876543210fedcba9876543210", 16,
    ).unwrap();
    client.session_token_bytes = crate::auth::crypto::strip_leading_zeros(
        &session_token.to_bytes_be(),
    ).to_vec();

    assert_eq!(BigUint::from_bytes_be(client.session_token_bytes()), session_token);
}

#[test]
fn token_type_default_is_empty() {
    let (client, _rx, _shared) = test_client();
    assert_eq!(client.token_type(), "");
}

// ═══════════════════════════════════════════════════════════════════
//  Connection loss (ibx#242)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn engine_connection_loss_fires_connection_closed_once() {
    let (client, _rx, shared) = test_client();
    let mut w = RecordingWrapper::default();

    // Nothing to report while the engine is running.
    client.process_msgs(&mut w);
    assert!(client.is_connected());
    assert!(w.events.is_empty(), "no callbacks before the connection is lost");

    // Engine signals the end of the session.
    shared.set_connection_lost();
    client.process_msgs(&mut w);

    assert_eq!(w.events, vec!["connection_closed"]);
    assert!(!client.is_connected(), "is_connected must turn false");

    // Polling again must not repeat it.
    client.process_msgs(&mut w);
    assert_eq!(w.events, vec!["connection_closed"]);
}

#[test]
fn connection_loss_raises_no_error_callback() {
    // The reference client fires connection_closed with no error code on a
    // lost socket; the connectivity codes are server-pushed, never local.
    let (client, _rx, shared) = test_client();
    let mut w = RecordingWrapper::default();

    shared.set_connection_lost();
    client.process_msgs(&mut w);

    assert!(
        !w.events.iter().any(|e| e.starts_with("error:")),
        "no error callback expected, got: {:?}", w.events,
    );
}

#[test]
fn explicit_disconnect_fires_connection_closed() {
    let (client, _rx, _shared) = test_client();
    let mut w = RecordingWrapper::default();

    client.disconnect();
    client.process_msgs(&mut w);

    assert_eq!(w.events, vec!["connection_closed"]);
    assert!(!client.is_connected());
}

#[test]
fn queued_data_is_dispatched_before_connection_closed() {
    // A caller that stops polling on connection_closed must still have seen
    // whatever the engine had already queued.
    let (client, _rx, shared) = test_client();
    let mut w = RecordingWrapper::default();

    shared.reference.push_contract_details_end(7);
    shared.set_connection_lost();
    client.process_msgs(&mut w);

    assert_eq!(w.events, vec!["contract_details_end:7", "connection_closed"]);
}

/// The provider reaching the gateway is the whole of what makes the
/// authenticator factor usable from this client, and it is one line in a
/// struct literal. Reverting it to `None` broke nothing that was tested.
#[test]
fn the_second_factor_provider_reaches_the_gateway_config() {
    use crate::api::client::gateway_config;

    let base = crate::api::client::EClientConfig {
        username: "u".into(), password: "p".into(), host: "h".into(),
        paper: false, core_id: None, code_provider: None,
    };
    assert!(gateway_config(&base).code_provider.is_none(), "none stays none");

    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = called.clone();
    let with_provider = crate::api::client::EClientConfig {
        code_provider: Some(std::sync::Arc::new(move |_: crate::auth::session::IbKeyChallenge| {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok("12345678".to_string())
        })),
        ..base
    };
    let forwarded = gateway_config(&with_provider).code_provider
        .expect("the provider is forwarded, not dropped");

    // Identity, not just presence: forwarding some other closure would pass a
    // bare `is_some`.
    forwarded(crate::auth::session::IbKeyChallenge {
        factor: crate::auth::session::SecondFactor::AuthenticatorCode,
        display_id: String::new(),
        avth_url: String::new(),
    }).unwrap();
    assert!(called.load(std::sync::atomic::Ordering::SeqCst), "it is the caller's own provider");
}
