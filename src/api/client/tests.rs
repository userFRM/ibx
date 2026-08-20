use std::sync::Arc;

use super::*;
use crate::types::model::PRICE_SCALE_F;
use crate::api::wrapper::Wrapper;
use crate::api::wrapper::tests::RecordingWrapper;
use crate::bridge::SharedState;
use crate::control::historical::{HistoricalResponse, HistoricalBar, HeadTimestampResponse};
use crate::control::contracts::{ContractDefinition, OptionChainScope, SecurityType, SymbolMatch};
use crate::control::scanner::{ScannerEntry, ScannerResult};
use crate::control::news::NewsHeadline;
use crate::control::histogram::HistogramEntry;

/// Helper: create a test EClient backed by SharedState + channel.
pub(crate) fn test_client() -> (EClient, std::sync::mpsc::Receiver<ControlCommand>, Arc<SharedState>) {
    let shared = Arc::new(SharedState::new());
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let handle = std::thread::spawn(|| {});
    let client = EClient::from_parts(shared.clone(), tx, handle, "DU123".into());
    // Pre-seed SPY so find_or_register_instrument hits the fast path.
    client.core.con_id_to_instrument.lock().unwrap().insert(756733, 0);
    (client, rx, shared)
}

/// A short bracket is a sell, and its exits take the selling orientation:
/// take-profit below the entry, stop-loss above.
#[test]
fn a_short_bracket_reads_its_exits_the_way_a_sell_does() {
    let (client, _rx, _shared) = test_client();
    let c = spy();
    // Selling at 100: take profit below, stop out above.
    assert!(
        client.place_bracket(&c, "SSHORT", 1.0, 100.0, 90.0, 110.0).is_ok(),
        "a short bracket with its exits the right way round is placed",
    );
    assert!(
        client.place_bracket(&c, "SSHORT", 1.0, 100.0, 110.0, 90.0).is_err(),
        "and one with them the wrong way round is refused",
    );
    // The plain sell it must agree with.
    assert!(client.place_bracket(&c, "SELL", 1.0, 100.0, 90.0, 110.0).is_ok());
    assert!(client.place_bracket(&c, "SELL", 1.0, 100.0, 110.0, 90.0).is_err());
}

/// Helper: SPY contract.
fn spy() -> Contract {
    Contract {
        con_id: 756733, symbol: "SPY".into(), exchange: "SMART".into(),
        ..Default::default()
    }
}

/// Case name paired with the setter that gives an order the named attribute.
type OrderCase = (&'static str, fn(&mut Order));

// ═══════════════════════════════════════════════════════════════════
//  Algo parsing
// ═══════════════════════════════════════════════════════════════════

/// Re-placing a tracked id is a modify, and a stop order's price lives in
/// `aux_price`. Reading only `lmt_price` sent a limit price of zero for an
/// order that has no limit leg, which the venue rejects outright.
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
        filled_qty: 0.0, remaining_qty: 1.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
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
        filled_qty: 0.0, remaining_qty: 1.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    let mut w2 = RecordingWrapper::default();
    client.process_msgs(&mut w2);
    assert_eq!(w2.parent_ids.last().copied(), Some(0), "no parent is invented");
}

/// A status arriving on the heels of a fill reported an average of zero, so
/// the last thing a caller heard about a filled order was that it had filled
/// at no price at all.
#[test]
fn a_status_states_what_the_order_paid() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_order_update(OrderUpdate {
        order_id: 9403, instrument: 0, status: OrderStatus::Filled,
        filled_qty: 100.0, remaining_qty: 0.0,
        avg_price: 13 * crate::types::PRICE_SCALE + crate::types::PRICE_SCALE / 2,
        perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    let status = w.events.iter().find(|e| e.starts_with("order_status:9403:"))
        .expect("the status was dispatched");
    assert!(status.ends_with(":13.5"), "the average the report stated: {status}");
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
        order_id: 9402, instrument: 0, side: Side::Sell, qty: crate::types::QTY_SCALE, remaining: 0,
        price: 110 * crate::types::PRICE_SCALE, commission: 0, timestamp_ns: 0,
        cum_qty: crate::types::QTY_SCALE, avg_price: 110 * crate::types::PRICE_SCALE,
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
        min_commission: 0,
        max_commission: 0,
        commission_currency: String::new(),
        warning_text: String::new(),
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert_eq!(
        w.parent_ids.first().copied(), Some(4242),
        "the preview carries the recorded parent: {:?}", w.events,
    );
}

/// A quantity that is not a number, or is negative, or overflows the
/// fixed-point form, all become a size the caller did not ask for.
#[test]
fn an_unusable_quantity_is_refused() {
    for (qty, expect) in [
        (f64::NAN, "finite"),
        (f64::INFINITY, "finite"),
        (-5.0, "negative"),
        (1e11, "too large"),
    ] {
        let (client, _rx, _shared) = test_client();
        let order = Order {
            action: "BUY".into(), total_quantity: qty, order_type: "MKT".into(),
            tif: "DAY".into(), ..Default::default()
        };
        let err = client.place_order(9102, &spy(), &order)
            .expect_err("must be refused");
        assert!(err.message.contains(expect), "quantity {qty}: expected {expect:?}, got: {err}");
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

    let largest = crate::types::MAX_EXACT_QTY_SHARES;
    assert!(place(largest).is_ok(), "the largest carryable quantity still places");
    assert!(place(largest + 1.0).is_err(), "one past it does not");
    assert!(place(-1.0).is_err(), "a small negative is refused, not just a large one");
    assert!(place(1.25).is_ok(), "a fraction is carried, not refused");
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
/// trailing stop it describes a pegged order with no offset, which the venue
/// rejects, leaving the caller with no stop. Refusing keeps the resting order.
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
        assert!(err.message.contains("cannot be modified"), "{order_type}: {err}");
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
        // modify would state the order without it. The bracket links
        // are the costly pair: a child sent without its parent or OCA group
        // rests alone, and a fill on the sibling no longer cancels it.
        ("bracket child", |o| o.parent_id = 4242),
        ("OCA member", |o| o.oca_group = "bracket_1".into()),
        ("good-till expiry", |o| o.good_till_date = "20260311 16:00:00".into()),
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
        assert!(err.message.contains("cannot be modified"), "{name}: {err}");
        assert!(rx.try_recv().is_err(), "{name}: nothing reaches the wire");
    }
}

/// An order that cannot be placed the way it was asked for is refused, rather
/// than placed a different way.
///
/// Each of these used to go out transformed: a delayed order placed at once, a
/// misspelled time in force placed as DAY and gone at the close, an unreadable
/// expiry placed with none. The order reached the venue every time and nothing
/// said what had changed.
#[test]
fn an_order_that_cannot_be_placed_as_asked_is_refused() {
    /// What the case does to an order, and the field its refusal must name.
    type Refusal = (&'static str, fn(&mut Order), &'static str);
    let cases: &[Refusal] = &[
        ("a delayed activation that cannot be read",
         |o| o.good_after_time = "next tuesday".into(), "good_after_time"),
        ("a time in force spelled the wrong way",
         |o| o.tif = "gtc".into(), "tif"),
        ("a time in force that is not one",
         |o| o.tif = "FOREVER".into(), "tif"),
        ("an expiry that cannot be read",
         |o| o.good_till_date = "next tuesday".into(), "good_till_date"),
        ("a hedge of a kind this venue does not carry",
         |o| o.hedge_type = "X".into(), "hedge_type"),
        ("a beta hedge struck at something that is not a number",
         |o| { o.hedge_type = "B".into(); o.hedge_param = "market".into() },
         "hedge_param"),
        ("a pair hedge with no ratio stated",
         |o| o.hedge_type = "P".into(), "hedge_param"),
        ("a trigger this venue does not carry",
         |o| o.trigger_method = 9, "trigger_method"),
        ("a one-cancels-all rule this venue does not carry",
         |o| o.oca_type = 7, "oca_type"),
        ("a display size that is not a quantity",
         |o| o.display_size = -5, "display_size"),
        ("a borrow slot that is not one",
         |o| o.short_sale_slot = -1, "short_sale_slot"),
        ("a minimum trade quantity below nothing",
         |o| o.min_trade_qty = -10, "min_trade_qty"),
        // One of the twenty-nine this protocol has no field for. Stated by a
        // caller, the order would otherwise be placed with the instruction
        // missing and nothing to say it had been.
        ("a routing preference this protocol cannot express",
         |o| o.opt_out_smart_routing = true, "opt_out_smart_routing"),
        ("an order origin other than the account's own",
         |o| o.origin = 1, "origin"),
        ("a scale table this protocol has no field for",
         |o| o.scale_table = "SCALE".into(), "scale_table"),
    ];
    for (what, set, names) in cases {
        let (client, rx, _shared) = test_client();
        let mut order = Order {
            action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
            lmt_price: 100.0, tif: "DAY".into(), ..Default::default()
        };
        set(&mut order);
        let err = client.place_order(9401, &spy(), &order)
            .expect_err(&format!("{what} must be refused"));
        assert!(err.message.contains(names), "{what}: {err}");
        assert!(rx.try_recv().is_err(), "{what}: nothing reaches the wire");
        assert!(!client.core.is_order_tracked(9401), "{what}: nothing is tracked");
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
    assert!(err.message.contains("cannot be modified"), "{err}");
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
        assert!(err.message.contains("cannot be modified"), "{name}: {err}");
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
    assert!(err.message.contains("cannot be modified"), "{err}");
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

/// An algorithm this client does not model is carried, not refused.
///
/// Which algorithms an account may use is stated at logon — thirteen keys on an
/// ordinary session — and enforced by the venue. Refusing on the five this
/// client happens to name is a narrower answer than the venue's, and it stops a
/// caller using one the venue would have taken. The reference client does not
/// interpret these either.
#[test]
fn an_algorithm_this_client_does_not_model_is_carried_through() {
    let params = vec![
        TagValue { tag: "componentSize".into(), value: "100".into() },
        TagValue { tag: "timeBetweenOrders".into(), value: "60".into() },
    ];
    match parse_algo_params("Accumulate/Distribute", &params).expect("named, not refused") {
        AlgoParams::Named { strategy, params } => {
            assert_eq!(strategy, "Accumulate/Distribute", "as the caller wrote it");
            assert_eq!(
                params,
                vec!["componentSize", "100", "timeBetweenOrders", "60"],
                "name then value, in the order given",
            );
        }
        other => panic!("carried through as {other:?}"),
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

/// An algorithm this client does not model is not an algorithm the venue does
/// not offer, and is not refused as though it were.
///
/// Which strategies an account may use is stated at logon and enforced by the
/// venue, so refusing here would be narrower than the venue's answer. The
/// reference client forwards these without reading them.
#[test]
fn parse_algo_unsupported() {
    let carried = parse_algo_params("unknown", &[]).expect("carried, not refused");
    assert!(matches!(carried, AlgoParams::Named { .. }));

    // A malformed parameter on a strategy this client does model is still
    // refused: that is this client reading something it understands and
    // finding it wrong, which is a different thing.
    let bad = vec![TagValue { tag: "maxPctVol".into(), value: "not a number".into() }];
    assert!(parse_algo_params("vwap", &bad).is_err());
}

// ── malformed / non-finite algo params must be rejected, not
// silently coerced into a valid-looking default ──

#[test]
fn parse_algo_vwap_rejects_malformed_max_pct_vol() {
    let params = vec![TagValue { tag: "maxPctVol".into(), value: "abc".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.message.contains("maxPctVol"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_nan_max_pct_vol() {
    let params = vec![TagValue { tag: "maxPctVol".into(), value: "NaN".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.message.contains("maxPctVol"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_infinite_max_pct_vol() {
    let params = vec![TagValue { tag: "maxPctVol".into(), value: "inf".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.message.contains("maxPctVol"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_malformed_bool() {
    let params = vec![TagValue { tag: "noTakeLiq".into(), value: "yes".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.message.contains("noTakeLiq"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_empty_max_pct_vol() {
    // A present-but-empty value is a caller who set the tag, not one who
    // never set it — it must be refused like any other malformed value,
    // not silently coerced into the "absent" default of 0.0.
    let params = vec![TagValue { tag: "maxPctVol".into(), value: "".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.message.contains("maxPctVol"), "got: {err}");
}

#[test]
fn parse_algo_vwap_rejects_empty_bool() {
    let params = vec![TagValue { tag: "noTakeLiq".into(), value: "".into() }];
    let err = parse_algo_params("vwap", &params).unwrap_err();
    assert!(err.message.contains("noTakeLiq"), "got: {err}");
}

#[test]
fn parse_algo_arrival_price_rejects_unknown_risk_aversion() {
    // The issue's own repro: a typo must be refused, not silently sent as Neutral.
    let params = vec![TagValue { tag: "riskAversion".into(), value: "Aggresive".into() }];
    let err = parse_algo_params("arrivalpx", &params).unwrap_err();
    assert!(err.message.contains("riskAversion"), "got: {err}");
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
    assert!(err.message.contains("riskAversion"), "got: {err}");
}

#[test]
fn parse_algo_dark_ice_rejects_malformed_display_size() {
    let params = vec![TagValue { tag: "displaySize".into(), value: "abc".into() }];
    let err = parse_algo_params("darkice", &params).unwrap_err();
    assert!(err.message.contains("displaySize"), "got: {err}");
}

#[test]
fn parse_algo_dark_ice_rejects_negative_display_size() {
    let params = vec![TagValue { tag: "displaySize".into(), value: "-5".into() }];
    let err = parse_algo_params("darkice", &params).unwrap_err();
    assert!(err.message.contains("displaySize"), "got: {err}");
}

/// Tag 111 display size is how much of the order the book shows. It is required
/// rather than defaulted, since any default publishes a size the caller did not
/// choose.
#[test]
fn parse_algo_dark_ice_needs_a_display_size() {
    let err = parse_algo_params("darkice", &[]).unwrap_err();
    assert!(err.message.contains("displaySize"), "got: {err}");
}

/// A key the strategy does not carry is refused. A modelled strategy is encoded
/// from the fields it names, so any other key would not reach the venue.
#[test]
fn parse_algo_says_when_a_parameter_would_not_be_carried() {
    let params = vec![
        TagValue { tag: "displaySize".into(), value: "100".into() },
        TagValue { tag: "speedUp".into(), value: "1".into() },
    ];
    let err = parse_algo_params("darkice", &params).unwrap_err();
    assert!(err.message.contains("speedUp"), "got: {err}");

    // A strategy this client does not model is handed over as written, so
    // every key the caller set goes with it.
    assert!(parse_algo_params("Balanced", &params).is_ok());
}

#[test]
fn parse_algo_dark_ice_rejects_empty_display_size() {
    let params = vec![TagValue { tag: "displaySize".into(), value: "".into() }];
    let err = parse_algo_params("darkice", &params).unwrap_err();
    assert!(err.message.contains("displaySize"), "got: {err}");
}

// ═══════════════════════════════════════════════════════════════════
//  Connection
// ═══════════════════════════════════════════════════════════════════

#[test]
fn is_connected_after_construction() {
    let (client, _rx, _shared) = test_client();
    assert!(client.is_connected());
}

/// Disconnecting ends the session and then stops the engine, in that order. The
/// venue is told the session is going while there is still a connection to tell
/// it on; stopping the engine first would leave it to notice.
#[test]
fn disconnect_ends_the_session_then_stops_the_engine() {
    let (client, rx, _shared) = test_client();
    client.disconnect();
    assert!(!client.is_connected());
    assert!(matches!(rx.try_recv().unwrap(), ControlCommand::Logout));
    assert!(matches!(rx.try_recv().unwrap(), ControlCommand::Shutdown));
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
    assert!(matches!(cmd1, ControlCommand::RegisterInstrument { contract: ContractRef { con_id: 756733, .. }, .. }));
    let cmd2 = rx.try_recv().unwrap();
    match cmd2 {
        ControlCommand::Subscribe { contract: ContractRef { con_id, symbol, .. }, .. } => {
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
            ControlCommand::Subscribe { contract: ContractRef { con_id, .. }, mode_9887, .. } => {
                assert_eq!(mode_9887, mode);
                assert_eq!(con_id, 756733);
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }
}

// A second live subscription on the same contract would clobber
// the first's reverse mapping and orphan it silently. Reject at the call.
#[test]
fn a_second_caller_watches_the_subscription_that_is_up() {
    let (client, rx, _shared) = test_client();
    // Existing live subscription for SPY (instrument 0) under req_id 1.
    client.core.con_id_to_instrument.lock().unwrap().insert(spy().con_id, 0);
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);
    client.core.req_to_instrument.lock().unwrap().insert(1, 0);

    client.req_mkt_data(2, &spy(), "", false, false)
        .expect("a second caller watches it rather than being refused");
    assert!(rx.try_recv().is_err(), "one contract, one subscription on the wire");
    assert_eq!(client.core.followers_of(0), vec![2], "and it hears the quotes");
    assert_eq!(
        client.core.instrument_to_req.lock().unwrap().get(&0).copied(),
        Some(1),
        "the one that holds it still holds it",
    );

    // The holder leaves; the one still watching takes it over rather than
    // losing the feed, and nothing is withdrawn from the venue.
    let (withdraw, _) = client.core.unregister_mkt_data(1);
    assert!(withdraw.is_none(), "nothing is withdrawn while someone is watching");
    assert_eq!(
        client.core.instrument_to_req.lock().unwrap().get(&0).copied(),
        Some(2),
        "handed to the one still watching",
    );

    // And when the last one leaves, it goes.
    let (withdraw, _) = client.core.unregister_mkt_data(2);
    assert_eq!(withdraw, Some(0), "the last one out withdraws it");
}

// A contract given the ordinary ibapi way carries conId 0. Cached as
// an identity it maps every later symbol onto the first one's instrument, and
// the guard above then refuses them all — a symbol-only client could
// hold exactly one subscription.
#[test]
fn a_second_symbol_is_not_a_duplicate_of_the_first_con_id_less_contract() {
    let (client, rx, _shared) = test_client();
    // What a live symbol-only subscription under req_id 1 leaves behind.
    client.core.con_id_to_instrument.lock().unwrap().insert(0, 0);
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);

    let qqq = Contract {
        symbol: "QQQ".into(), sec_type: "STK".into(), exchange: "SMART".into(),
        ..Default::default()
    };
    let err = client.req_mkt_data(2, &qqq, "", false, false).unwrap_err();
    assert!(!err.message.contains("req_id 1"), "QQQ is not the live contract: {err}");
    match rx.try_recv().expect("the registration reaches the engine") {
        ControlCommand::RegisterInstrument { contract: ContractRef { con_id, symbol, .. }, .. } => {
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

/// Nothing about a session reaches the disk unless the caller asks for it. A
/// credential is theirs to place, and a library that writes one somewhere by
/// itself has made that decision for them.
#[test]
fn a_session_is_not_written_anywhere_by_default() {
    let cfg = EClientConfig::default();
    assert!(cfg.session_file.is_none(), "no file unless one is named");
    assert!(cfg.resume.is_none(), "and nothing is resumed unless one is given");
}

/// A session names the account it came from. Handing back one from a different
/// login describes a session this connect has no claim on, and the request that
/// names it is asking the server about somebody else's — so it is not offered,
/// and the login proceeds as if none had been given.
#[test]
fn a_session_from_another_account_is_not_offered() {
    let session = crate::auth::resume::ResumableSession {
        token: vec![1, 2, 3],
        server_session_id: "abc.0001".into(),
        hw_info: "hw".into(),
        encoded: "enc".into(),
        username: "someone-else".into(),
        paper: true,
    };
    let offered = |cfg: &EClientConfig| {
        cfg.resume.as_ref().filter(|r| r.username == cfg.username && r.paper == cfg.paper).is_some()
    };

    let cfg = |username: &str, paper: bool| EClientConfig {
        username: username.into(), paper,
        resume: Some(session.clone()), ..Default::default()
    };
    assert!(offered(&cfg("someone-else", true)), "its own account's session is offered");
    assert!(!offered(&cfg("me", true)), "another account's session is not");
    assert!(!offered(&cfg("someone-else", false)), "nor a session of the other kind");
}

/// Tick-by-tick rides the historical farm this client already reaches, not a
/// service of its own, so the subscription is sent and the venue answers it.
#[test]
fn req_tick_by_tick_data_is_sent_rather_than_refused() {
    let (client, _rx, _shared) = test_client();
    // A kind the venue does not name is still refused, and refused for saying
    // so rather than for the feed being unreachable.
    let err = client
        .req_tick_by_tick_data(10, &spy(), "Sideways", 0, false)
        .expect_err("a kind that is not a kind is refused");
    assert!(err.message.contains("no such kind"), "{err}");
    assert!(
        !err.message.contains("not served to this session"),
        "the old reasoning is gone: {err}"
    );
}

/// The two trade streams are two streams. Asking for one under the other's
/// name asked the venue for someone else's trades: every trade reported away
/// from the exchange arrived on a subscription that wanted the exchange's own.
#[test]
fn the_two_trade_streams_are_asked_for_separately() {
    assert_eq!(TbtType::named("AllLast"), Ok(TbtType::AllLast));
    assert_eq!(TbtType::named("Last"), Ok(TbtType::Last));
    assert_eq!(TbtType::named("BidAsk"), Ok(TbtType::BidAsk));
    assert!(TbtType::named("Sideways").is_err(), "a kind that is not a kind is refused");
}

#[test]
fn cancel_tick_by_tick_data_sends_unsubscribe_tbt() {
    let (client, rx, _shared) = test_client();
    // A trade stream is held in its own table. Held in the quote table, a
    // request for trades was handed the contract's quotes, and withdrawing it
    // took the quotes away from whoever was watching them.
    client.core.tbt_to_instrument.lock().unwrap().insert(10, 3);
    client.core.instrument_to_req.lock().unwrap().insert(3, 99);
    client.cancel_tick_by_tick_data(10).unwrap();
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, ControlCommand::UnsubscribeTbt { instrument: 3, .. }));
    assert_eq!(
        client.core.instrument_to_req.lock().unwrap().get(&3).copied(),
        Some(99),
        "and the caller quoting that contract still is",
    );
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

/// A caller asking for a fraction of a share gets one. The quantity was
/// taken through `as u32`, so `placeOrder` with 0.5 sent an order for none:
/// the fraction was dropped before it reached the wire.
#[test]
fn place_order_carries_a_fractional_quantity() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(),
        total_quantity: 0.5,
        order_type: "LMT".into(),
        lmt_price: 150.0,
        ..Default::default()
    };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { qty, .. }) => assert_eq!(
            qty, crate::types::QTY_SCALE / 2,
            "half a share reaches the engine as half a share",
        ),
        _ => panic!("expected a submitted order, got {cmd:?}"),
    }
}

#[test]
fn place_order_market() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order { action: "BUY".into(), total_quantity: 100.0, order_type: "MKT".into(), ..Default::default() };
    client.place_order(1, &spy(), &order).unwrap();

    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::Order(OrderRequest::SubmitEx { qty, kind: OrderKind::Market, .. }) => assert_eq!(qty, 100 * crate::types::QTY_SCALE),
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
            assert_eq!(qty, 50 * crate::types::QTY_SCALE);
            assert_eq!(price, (150.25 * PRICE_SCALE_F) as i64);
        }
        _ => panic!("expected a Limit order, got {cmd:?}"),
    }
}

#[test]
fn place_order_trailing_stop_carries_initial_trigger() {
    // Part B /: a plain amount trailing stop can carry an
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
    // /: a base STP that converts to a TRAIL must carry
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
    // An adjustable stop used as a bracket child must stay linked to
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
    // The replace asserted 6433=1 unconditionally, so an order placed
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

// ── every order type must carry attrs + tif when set ──

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
            assert_eq!(attrs.oca_type, 2);
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

// ── transmit=false must be rejected, not silently ignored ──

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

// ── FA allocation must be rejected, not silently dropped ──

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
        assert!(err.message.contains(field), "{name}: the message must name the field — {err}");
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

// ── oca_type carried and coerced ──

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
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::SnapMkt { .. }, .. })));
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
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::SnapMid { .. }, .. })));
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
    assert!(matches!(cmd, ControlCommand::Order(OrderRequest::SubmitEx { kind: OrderKind::SnapPri { .. }, .. })));
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
    assert!(result.unwrap_err().message.contains("Unsupported order type"));
}

/// A preview states the order type it asks about. An unrecognised type is
/// refused; encoded as a limit, the answer would describe a different order.
#[test]
fn a_preview_is_refused_for_a_type_this_client_cannot_send() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0,
        order_type: "SOMETHING NEW".into(), what_if: true,
        ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.message.contains("Unsupported order type"), "got: {err}");
}

/// An algorithm rides on a limit order: tag 40 is written once, as `2`. Any
/// other order type carrying an algorithm is refused.
#[test]
fn an_algo_order_states_the_limit_it_is_sent_as() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let market_with_algo = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "MKT".into(),
        algo_strategy: "Adaptive".into(),
        algo_params: vec![TagValue { tag: "adaptivePriority".into(), value: "Normal".into() }],
        ..Default::default()
    };
    let err = client.place_order(1, &spy(), &market_with_algo).unwrap_err();
    assert!(err.message.contains("limit order"), "got: {err}");

    let limit_with_algo = Order {
        order_type: "LMT".into(), lmt_price: 100.0, ..market_with_algo
    };
    client.place_order(2, &spy(), &limit_with_algo).expect("a limit carries the algo");
    assert!(matches!(rx.try_recv().unwrap(), ControlCommand::Order(OrderRequest::SubmitEx {
        kind: OrderKind::Adaptive { .. }, .. })));
}

/// A pegged-to-benchmark order encodes and previews.
#[test]
fn a_pegged_to_benchmark_order_reaches_the_builder() {
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "PEG BENCH".into(),
        lmt_price: 100.0, reference_contract_id: 265598, ..Default::default()
    };
    client.place_order(1, &spy(), &order).expect("PEG BENCH is placeable");
    assert!(matches!(rx.try_recv().unwrap(), ControlCommand::Order(OrderRequest::SubmitEx {
        kind: OrderKind::PegBench { .. }, .. })));
}

#[test]
fn place_order_non_stk_contract_rejected() {
    // An option's symbol names a whole chain. Without an expiry, strike or
    // right the order cannot say which contract it means.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    // No contract id: an id names one contract on its own and the venue takes
    // an order carrying nothing else, so this is the case the guard is for.
    let bare = Contract {
        symbol: "AAPL".into(), sec_type: "OPT".into(), exchange: "SMART".into(),
        ..Default::default()
    };
    let order = Order { action: "BUY".into(), total_quantity: 1.0, order_type: "MKT".into(), ..Default::default() };
    let err = client.place_order(1, &bare, &order).expect_err("a chain is not a contract");
    assert!(err.message.contains("OPT"), "the refusal names the type: {err}");
    assert!(rx.try_recv().is_err(), "and nothing reaches the engine");

    // An id says which contract without any of it.
    let by_id = Contract {
        con_id: 999001, sec_type: "OPT".into(), exchange: "SMART".into(),
        ..Default::default()
    };
    let refusal = client.place_order(2, &by_id, &order).unwrap_err();
    assert!(!refusal.message.contains("names a whole chain"), "an id is not a chain: {refusal}");

    // The named case is not asserted here: this fixture has no engine, so
    // registering an instrument blocks on a reply that never arrives. That an
    // identified option is accepted is pinned by `contract_gate_tests`, and that
    // the identity reaches the wire by `an_option_order_names_its_contract`.
}

/// What an exercise refuses, and it refuses before it builds anything: a
/// caller told the request went out believes the position was dealt with.
///
/// The documented API names a third action, a hold, which is not served here.
/// A quantity that is not a count reaches the wire through `as u32` as a very
/// large one. And the account is not carried on the order at all, so an
/// exercise naming another one would be taken on the connected account.
#[test]
fn an_exercise_it_cannot_serve_is_refused_before_anything_is_sent() {
    let (client, rx, _shared) = test_client();
    let opt = Contract {
        con_id: 999002, symbol: "AAPL".into(), sec_type: "OPT".into(),
        last_trade_date_or_contract_month: "20260619".into(), strike: 230.0,
        right: "C".into(), multiplier: "100".into(), ..Default::default()
    };
    let cases: [(&str, i32, i32, &str); 5] = [
        ("a hold", 3, 1, ""),
        ("no action at all", 0, 1, ""),
        ("no contracts", 1, 0, ""),
        ("a negative count", 1, -1, ""),
        ("another account", 1, 1, "DU999"),
    ];
    for (name, action, qty, account) in cases {
        client.exercise_options(1, &opt, action, qty, account, false).expect_err(name);
        assert!(rx.try_recv().is_err(), "{name} reached the engine");
    }

    // One it can serve gets as far as naming the contract. This fixture has no
    // engine to answer the registration, so the call ends there.
    let _ = client.exercise_options(1, &opt, 1, 1, "DU123", false);
    assert!(
        matches!(rx.try_recv(), Ok(ControlCommand::RegisterInstrument { contract: ContractRef { con_id: 999002, .. }, .. })),
        "a served exercise registers its contract",
    );
}

#[test]
fn an_order_states_where_it_is_to_be_filled() {
    // The venue does not choose a destination. Without this, a contract stating
    // only a symbol was looked up and filled on whichever listing the
    // definition service answered with first.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let nowhere = Contract { con_id: 756733, symbol: "SPY".into(), ..Default::default() };
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "MKT".into(),
        ..Default::default()
    };

    let refused = client.place_order(1, &nowhere, &order).expect_err("no destination");
    assert_eq!(refused.code, crate::error_codes::Refusal::VALIDATION);
    assert!(rx.try_recv().is_err(), "and nothing reaches the engine");

    client.place_order(2, &spy(), &order).expect("a destination is all it lacked");
    assert!(rx.try_recv().is_ok());
}

#[test]
fn place_order_explicit_stk_contract_accepted() {
    // An explicit sec_type="STK" must still be accepted.
    let (client, rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let stk = Contract {
        con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), ..Default::default()
    };
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

/// The executions mutex is not held while user callbacks run. A wrapper that
/// re-enters a path locking `executions` is an ordinary ibapi pattern —
/// re-requesting from `exec_details` — and holding the lock across it
/// deadlocks, in Python with the GIL held, freezing the interpreter.
#[test]
fn req_executions_does_not_hold_the_lock_across_callbacks() {
    struct Reentrant<'a> {
        core: &'a ClientCore,
        observed_locked: bool,
        rows: usize,
    }
    impl Wrapper for Reentrant<'_> {
        fn exec_details(&mut self, _r: i64, _c: &Contract, _e: &crate::types::model::Execution) {
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
        crate::types::model::Contract { symbol: "AAPL".into(), ..Default::default() },
        Default::default(),
        Default::default(),
    );

    let mut w = Reentrant { core: &client.core, observed_locked: false, rows: 0 };
    client.req_executions(1, &crate::types::model::ExecutionFilter::default(), &mut w);
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
        fn exec_details(&mut self, _r: i64, _c: &Contract, e: &crate::types::model::Execution) {
            self.seen.push(e.time.clone());
        }
    }

    let (client, _rx, _shared) = test_client();
    for t in ["20260729-09:00:00", "20260729-11:00:00"] {
        client.core.push_execution(
            1,
            crate::types::model::Contract { symbol: "AAPL".into(), ..Default::default() },
            crate::types::model::Execution { time: t.into(), ..Default::default() },
            Default::default(),
        );
    }

    let mut w = Rows::default();
    client.req_executions(1, &crate::types::model::ExecutionFilter {
        time: "20260729-10:00:00".into(), ..Default::default()
    }, &mut w);
    assert_eq!(w.seen, vec!["20260729-11:00:00"], "only executions at or after the bound");

    // Punctuation differs between the two sides in practice; the comparison is
    // on digits, so a space-separated bound behaves identically.
    let mut w2 = Rows::default();
    client.req_executions(1, &crate::types::model::ExecutionFilter {
        time: "20260729 10:00:00".into(), ..Default::default()
    }, &mut w2);
    assert_eq!(w2.seen, vec!["20260729-11:00:00"], "separator must not change the bound");

    // A date-only bound keeps the whole day rather than dropping it.
    let mut w3 = Rows::default();
    client.req_executions(1, &crate::types::model::ExecutionFilter {
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
//  Order validation — aux_price guards
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
    assert!(result.unwrap_err().message.contains("aux_price"));
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
    assert!(result.unwrap_err().message.contains("aux_price"));
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
    assert!(result.unwrap_err().message.contains("trailing_percent"));
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
    assert!(result.unwrap_err().message.contains("aux_price"));
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
    assert!(result.unwrap_err().message.contains("aux_price"));
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
    assert!(result.unwrap_err().message.contains("aux_price"));
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
    assert!(result.unwrap_err().message.contains("aux_price"));
}

// ═══════════════════════════════════════════════════════════════════
//  Order validation — non-finite and out-of-range numbers
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
    assert!(err.message.contains("lmt_price"), "got: {err}");
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
    assert!(err.message.contains("lmt_price"), "got: {err}");
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
    assert!(err.message.contains("lmt_price"), "got: {err}");
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
    assert!(err.message.contains("lmt_price"), "got: {err}");
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
    assert!(err.message.contains("aux_price"), "got: {err}");
}

#[test]
fn place_order_rejects_negative_quantity() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: -100.0, order_type: "MKT".into(), ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.message.contains("total_quantity"), "got: {err}");
}

#[test]
fn place_order_rejects_nan_quantity() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: f64::NAN, order_type: "MKT".into(), ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.message.contains("total_quantity"), "got: {err}");
}

#[test]
fn place_order_rejects_infinite_quantity() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let order = Order {
        action: "BUY".into(), total_quantity: f64::INFINITY, order_type: "MKT".into(), ..Default::default()
    };
    let err = client.place_order(1, &spy(), &order).unwrap_err();
    assert!(err.message.contains("total_quantity"), "got: {err}");
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
    assert!(err.message.contains("display_size"), "got: {err}");
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
    assert!(err.message.contains("min_qty"), "got: {err}");
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
    assert!(err.message.contains("parent_id"), "got: {err}");
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
    assert!(err.message.contains("trailing_percent"), "got: {err}");
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
    assert!(err.message.contains("adaptivePriority"), "got: {err}");
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
    let err = crate::client_core::ClientCore::build_order_request(&order, 1, 0, None).unwrap_err();
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
        ControlCommand::FetchHistorical { contract: ContractRef { con_id, sec_type, exchange, .. }, req_id, duration, bar_size, what_to_show, use_rth, .. } => {
            assert_eq!(req_id, 5);
            assert_eq!(con_id, 756733);
            assert_eq!(duration, "1 D");
            assert_eq!(bar_size, "1 hour");
            assert_eq!(what_to_show, "TRADES");
            assert!(use_rth);
            // The contract's own fields have to leave the client, or the
            // engine has nothing but the old constants to fall back on. `spy()` states
            // a destination and no security type, so
            // the destination arrives as given and the type arrives empty for
            // the engine to substitute — tested at its source.
            assert_eq!(sec_type, "");
            assert_eq!(exchange, "SMART");
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
        ControlCommand::FetchHistorical { contract: ContractRef { sec_type, exchange, .. }, .. } => {
            assert_eq!(sec_type, "FUT");
            assert_eq!(exchange, "CME");
        }
        _ => panic!("expected FetchHistorical"),
    }
}

// ── unknown bar_size / what_to_show reject instead of silently
// falling back to 5-minute / TRADES bars ──

#[test]
fn req_historical_data_rejects_unknown_bar_size() {
    let (client, rx, _shared) = test_client();
    // A bar size in the wrong case is refused rather than answered with
    // five-minute candles.
    let err = client.req_historical_data(5, &spy(), "", "2 D", "1 Min", "TRADES", true, 1, false).unwrap_err();
    assert!(err.message.contains("bar_size"), "got: {err}");
    assert!(rx.try_recv().is_err(), "nothing may reach the engine");
}

#[test]
fn req_historical_data_rejects_unknown_what_to_show() {
    let (client, rx, _shared) = test_client();
    let err = client.req_historical_data(5, &spy(), "", "2 D", "1 min", "TRADE", true, 1, false).unwrap_err();
    assert!(err.message.contains("what_to_show"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn req_historical_data_rejects_unsupported_keep_up_to_date_size() {
    let (client, rx, _shared) = test_client();
    // "1 min" is valid on the batch path and not supported for streaming, and
    // is refused here rather than downgraded to five-minute bars.
    let err = client.req_historical_data(5, &spy(), "", "1 D", "1 min", "TRADES", true, 1, true).unwrap_err();
    assert!(err.message.contains("keep_up_to_date"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn req_historical_data_accepts_streamable_keep_up_to_date_size() {
    let (client, rx, _shared) = test_client();
    client.req_historical_data(5, &spy(), "", "1 D", "5 mins", "TRADES", true, 1, true).unwrap();
    assert!(matches!(rx.try_recv().unwrap(), ControlCommand::FetchHistorical { keep_up_to_date: true, .. }));
}

/// An engine that has gone is not a request that was malformed. A caller that
/// branches on the code has to be able to tell a session it can reopen from a
/// request it has to fix.
#[test]
fn a_request_with_no_engine_behind_it_says_so_under_its_own_code() {
    // The engine's end of the channel goes with the receiver.
    let (client, rx, _shared) = test_client();
    drop(rx);

    let refused = client
        .req_contract_details(1, &spy())
        .expect_err("nothing can be sent with no engine to send it");
    assert_eq!(
        refused.code,
        crate::error_codes::Refusal::NOT_CONNECTED,
        "not connected, rather than a request that failed validation: {refused}",
    );
}

/// A req_id reaches these requests' wire form as u32. `next_order_id()` hands
/// out ids near 1.7e12, so a caller running one counter for orders and
/// requests — the ibapi idiom — wraps every one of these: the venue receives an
/// id nobody chose, and the callback carries that id.
#[test]
fn an_unwireable_req_id_is_refused() {
    type Call = fn(&EClient, i64) -> Result<(), Refusal>;
    let calls: &[(&str, Call)] = &[
        ("req_historical_data", |c, id| c.req_historical_data(id, &spy(), "", "1 D", "1 min", "TRADES", true, 1, false)),
        ("cancel_historical_data", |c, id| c.cancel_historical_data(id)),
        ("req_head_time_stamp", |c, id| c.req_head_time_stamp(id, &spy(), "TRADES", true, 1)),
        ("cancel_head_time_stamp", |c, id| c.cancel_head_time_stamp(id)),
        ("req_contract_details", |c, id| c.req_contract_details(id, &spy())),
        ("req_matching_symbols", |c, id| c.req_matching_symbols(id, "SP")),
        ("req_sec_def_opt_params", |c, id| c.req_sec_def_opt_params(id, "SPY", "", "STK", 756733)),
        ("req_scanner_subscription", |c, id| c.req_scanner_subscription(id, "STK", "STK.US", "TOP_PERC_GAIN", 10, &[])),
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
            assert!(err.message.contains("req_id"), "{name}: the error names the field: {err}");
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
        ControlCommand::FetchHeadTimestamp { contract: ContractRef { con_id, .. }, req_id, what_to_show, use_rth, .. } => {
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
        ControlCommand::FetchContractDetails { contract: ContractRef { con_id, .. }, req_id, .. } => {
            assert_eq!(req_id, 7);
            assert_eq!(con_id, 756733);
        }
        _ => panic!("expected FetchContractDetails"),
    }
}

#[test]
fn req_contract_details_forwards_filter_fields() {
    // /: a by-symbol lookup must carry the disambiguation
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
        ControlCommand::FetchContractDetails { contract: ContractRef { con_id, .. }, req_id, filters, .. } => {
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
    // /: an identifier lookup (ISIN) must carry secId and
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
    // The account has stated everything it holds. Without it this waits out
    // the whole ten seconds for a signal no engine is here to send, and comes
    // back through the timeout rather than through delivery.
    shared.portfolio.set_account_download_complete();
    // Named, as a holding off the wire is: nameless, delivery waits three
    // seconds for a definition to arrive from an engine that is not here.
    shared.portfolio.set_position_info(PositionInfo { con_id: 265598, position: 100.0, avg_cost: 150 * PRICE_SCALE, symbol: "AAPL".into(), ..Default::default() });
    shared.portfolio.set_position_info(PositionInfo { con_id: 756733, position: -50.0, avg_cost: 400 * PRICE_SCALE, symbol: "SPY".into(), ..Default::default() });
    let mut w = RecordingWrapper::default();
    client.req_positions(&mut w);
    let positions: Vec<_> = w.events.iter().filter(|e| e.starts_with("position:")).collect();
    assert_eq!(positions.len(), 2);
    assert!(w.events.last().unwrap() == "position_end");
}

/// Account values are reported as the venue states them: every figure it sends,
/// in the currency it names, labelled with the account holding them rather than
/// the account the caller asked about.
#[test]
fn the_account_figures_are_the_ones_the_venue_stated() {
    #[derive(Default)]
    struct Rows(Vec<(String, String, String, String)>);
    impl crate::api::wrapper::Wrapper for Rows {
        fn account_update_multi(
            &mut self, _req_id: i64, account: &str, _model: &str,
            key: &str, value: &str, currency: &str,
        ) {
            self.0.push((
                account.to_string(), key.to_string(), value.to_string(), currency.to_string(),
            ));
        }
    }

    let (client, _rx, shared) = test_client();
    shared.portfolio.note_account_value("NetLiquidation", "12345.678", "CHF");
    shared.portfolio.note_account_value("SettledCash", "42.5", "CHF");

    let mut rows = Rows::default();
    client.req_account_updates_multi(1, "DU999", "", true, &mut rows);

    assert!(
        rows.0.iter().any(|(account, key, value, currency)| {
            account == "DU123" && key == "NetLiquidation" && value == "12345.678"
                && currency == "CHF"
        }),
        "the figure, currency and account as stated: {:?}",
        rows.0,
    );
    assert!(
        rows.0.iter().any(|(_, key, ..)| key == "SettledCash"),
        "a figure the venue states outside the eight that were worked out here",
    );
    assert!(
        rows.0.iter().all(|(account, ..)| account == "DU123"),
        "an account that was asked about is not the account these are for",
    );
}

/// A holding is labelled with the account that holds it.
#[test]
fn holdings_are_labelled_with_the_account_that_holds_them() {
    let (client, _rx, shared) = test_client();
    shared.portfolio.set_position_info(PositionInfo {
        con_id: 756733, position: 100.0, avg_cost: 400 * PRICE_SCALE, ..Default::default()
    });
    let mut w = RecordingWrapper::default();
    client.req_positions_multi(2, "DU999", "", &mut w);
    assert!(
        w.events.iter().any(|e| e.starts_with("position_multi:2:DU123:")),
        "another account's name sat on this account's holdings: {:?}",
        w.events,
    );
}

#[test]
fn req_positions_empty_still_calls_position_end() {
    let (client, _rx, shared) = test_client();
    // As above: the account has spoken, and it holds nothing.
    shared.portfolio.set_account_download_complete();
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
    client.req_scanner_subscription(3, "STK", "STK.US.MAJOR", "TOP_PERC_GAIN", 25,
        &[TagValue { tag: "priceAbove".into(), value: "10".into() }]).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::SubscribeScanner { req_id, scan_code, max_items, filters, .. } => {
            assert_eq!(req_id, 3);
            assert_eq!(scan_code, "TOP_PERC_GAIN");
            assert_eq!(max_items, 25);
            assert_eq!(filters, vec![("priceAbove".to_string(), "10".to_string())]);
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
    // The query carries no time bounds, so a window is refused rather than
    // dropped: the answer is the most recent headlines, not the window's.
    assert!(client.req_historical_news(4, 265598, "BRFG", "2026-01-01", "2026-03-01", 10).is_err());
    client.req_historical_news(4, 265598, "BRFG", "", "", 10).unwrap();
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
    // Bounded at its end. A start alone asked for the ticks before the moment
    // the caller wanted the ticks after.
    assert!(client.req_historical_ticks(8, &spy(), "20260101 09:30:00", "", 1000, "TRADES", true).is_err());
    client.req_historical_ticks(8, &spy(), "", "20260101 16:00:00", 1000, "TRADES", true).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::FetchHistoricalTicks { contract: ContractRef { con_id, .. }, req_id, number_of_ticks, what_to_show, .. } => {
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

/// An unrecognised `what_to_show` is refused rather than encoded as TRADES.
#[test]
fn a_real_time_bar_request_states_a_series_the_venue_serves() {
    let (client, _rx, _shared) = test_client();
    let err = client.req_real_time_bars(9, &spy(), 5, "BDI", true).unwrap_err();
    assert!(err.message.contains("Unsupported what_to_show"), "got: {err}");
}

#[test]
fn req_real_time_bars_sends_subscribe() {
    let (client, rx, _shared) = test_client();
    client.req_real_time_bars(9, &spy(), 5, "TRADES", true).unwrap();
    let cmd = rx.try_recv().unwrap();
    match cmd {
        ControlCommand::SubscribeRealTimeBar { contract: ContractRef { con_id, .. }, req_id, what_to_show, use_rth, .. } => {
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
        ControlCommand::FetchHistoricalSchedule { contract: ContractRef { con_id, .. }, req_id, use_rth, .. } => {
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

    let (tx, _rx) = std::sync::mpsc::sync_channel(4096);
    let handle = std::thread::spawn(|| {});
    let client = EClient::from_parts(shared, tx, handle, "DU123".into());

    client.core.req_to_instrument.lock().unwrap().insert(5, 0);

    let quote = client.quote(5).unwrap();
    assert_eq!(quote.bid, 200 * PRICE_SCALE);
    assert!(client.quote(99).is_none());
}

// RTT is None until measured, then reflects the stored sample;
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

    let (tx, _rx) = std::sync::mpsc::sync_channel(4096);
    let handle = std::thread::spawn(|| {});
    let client = EClient::from_parts(shared, tx, handle, "DU123".into());

    let quote = client.quote_by_instrument(2).expect("registered id");
    assert_eq!(quote.ask, 300 * PRICE_SCALE);

    // An out-of-range id is a caller error, not a panic across
    // the language boundary.
    assert!(client.quote_by_instrument(999).is_none());
}

#[test]
fn account_reads_shared_state() {
    let (_client, _rx, shared) = test_client();
    let a = AccountState { net_liquidation: 100_000 * PRICE_SCALE, ..Default::default() };
    shared.portfolio.set_account(&a);
    let (client2, _rx2, _) = {
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
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
        price: 150 * PRICE_SCALE, qty: 100 * crate::types::QTY_SCALE, remaining: 0,
        commission: PRICE_SCALE, timestamp_ns: 123456789,
        cum_qty: 100 * crate::types::QTY_SCALE, avg_price: 150 * PRICE_SCALE,
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
        price: 150 * PRICE_SCALE, qty: 50 * crate::types::QTY_SCALE, remaining: 50 * crate::types::QTY_SCALE,
        commission: PRICE_SCALE, timestamp_ns: 123456789,
        cum_qty: 50 * crate::types::QTY_SCALE, avg_price: 150 * PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    // A partly filled working order is Submitted: the vocabulary has no
    // status of its own for it, and a program reading one finds it in neither
    // the active set nor the done set.
    assert!(
        w.events.iter().any(|e| e.starts_with("order_status:42:Submitted:")),
        "{:?}", w.events,
    );
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
        price: 151 * PRICE_SCALE, qty: 100 * crate::types::QTY_SCALE, remaining: 100 * crate::types::QTY_SCALE,
        commission: PRICE_SCALE, timestamp_ns: 0,
        cum_qty: 200 * crate::types::QTY_SCALE, avg_price: 150 * PRICE_SCALE + PRICE_SCALE / 2,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);

    let status = w.events.iter().find(|e| e.starts_with("order_status:42:"))
        .expect("order_status was dispatched");
    assert_eq!(
        status, "order_status:42:Submitted:200:100:150.5",
        "filled and avgFillPrice must describe the order, not the print",
    );
}

#[test]
fn process_msgs_dispatches_sell_fill() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 43, side: Side::Sell,
        price: 151 * PRICE_SCALE, qty: 100 * crate::types::QTY_SCALE, remaining: 0,
        commission: PRICE_SCALE, timestamp_ns: 0,
        cum_qty: 100 * crate::types::QTY_SCALE, avg_price: 151 * PRICE_SCALE,
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
        filled_qty: 0.0, remaining_qty: 100.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    shared.orders.push_order_update(OrderUpdate {
        order_id: 44, instrument: 0, status: OrderStatus::Cancelled,
        filled_qty: 0.0, remaining_qty: 100.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    shared.orders.push_order_update(OrderUpdate {
        order_id: 45, instrument: 0, status: OrderStatus::Rejected,
        filled_qty: 0.0, remaining_qty: 100.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("order_status:43:Submitted")));
    assert!(w.events.iter().any(|e| e.starts_with("order_status:44:Cancelled")));
    assert!(w.events.iter().any(|e| e.starts_with("order_status:45:Inactive")));
}

/// A parked (39=I) order's reason reaches the caller through `Wrapper::error`,
/// on top of the order_status "Inactive" callback above: ibapi has no callback
/// dedicated to an order held with a reason.
#[test]
fn process_msgs_dispatches_inactive_reason_as_error() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_order_inactive(46, 399, "Order held pending margin check".into());
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "error:46:399:Order held pending margin check"));
}

/// end-to-end: a genuinely-Inactive order dispatched through the real
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
        filled_qty: 0.0, remaining_qty: 100.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
    });
    shared.orders.push_order_update(OrderUpdate {
        order_id: 83, instrument: 0, status: OrderStatus::Rejected,
        filled_qty: 0.0, remaining_qty: 100.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
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
    // Reason 0 is too-late-to-cancel: the venue found the order and would not
    // act on it. Reported as 202 this read as "Order Cancelled" — the opposite
    // of what happened, and a caller would replace an order still working.
    shared.orders.push_cancel_reject(CancelReject {
        order_id: 44, instrument: 0, reject_type: 1, reason_code: 0, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(
        w.events.iter().any(|e| e.starts_with("error:44:10148:")),
        "{:?}", w.events,
    );
    assert!(!w.events.iter().any(|e| e.starts_with("error:44:202:")));
}

#[test]
fn process_msgs_dispatches_cancel_reject_type_2() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_cancel_reject(CancelReject {
        order_id: 44, instrument: 0, reject_type: 2, reason_code: 5, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(
        w.events.iter().any(|e| e.starts_with("error:44:10148:")),
        "{:?}", w.events,
    );
}

/// Cancel-reject reason 1 is an unknown order. Every other reason describes an
/// order the venue found and declined to act on.
#[test]
fn an_unknown_order_is_the_only_cancel_reject_reported_as_not_found() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_cancel_reject(CancelReject {
        order_id: 44, instrument: 0, reject_type: 1, reason_code: 1, timestamp_ns: 0,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(
        w.events.iter().any(|e| e.starts_with("error:44:10147:")),
        "{:?}", w.events,
    );
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
            halted: 0,
    };
    shared.market.push_quote(0, &q);

    client.core.req_to_instrument.lock().unwrap().insert(1, 0);
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);

    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);

    // Should have tick_price for: bid(1), ask(2), last(4), high(6), low(7), close(9),
    // open(14)
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:1:")));   // bid
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:2:")));   // ask
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:4:")));   // last
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:6:")));   // high
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:7:")));   // low
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:9:")));   // close
    assert!(w.events.iter().any(|e| e.starts_with("tick_price:1:14:"))); // open
    // tick_size for: bid_size(0), ask_size(3), last_size(5), volume(8).
    // Assert the delivered quantity, not just that a tick appeared — the
    // scaling defect in fired every one of these with a value four
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
        req_id: 10,
        // A hundred shares, held the way every quantity is held.
        instrument: 0, price: 150 * PRICE_SCALE, size: 100 * crate::types::QTY_SCALE,
        timestamp: 1700000000, exchange: "ARCA".into(), conditions: "".into(),
        past_limit: false,
        unreported: false,
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
        req_id: 10,
        instrument: 0, bid: 150 * PRICE_SCALE, ask: 151 * PRICE_SCALE,
        bid_size: 1000 * crate::types::QTY_SCALE,
        ask_size: 2000 * crate::types::QTY_SCALE,
        timestamp: 1700000000,
        bid_past_low: false,
        ask_past_high: false,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("tbt_bidask:10:1700000000:150:151:1000:2000")));
}

#[test]
fn process_msgs_tbt_records_carry_the_request_they_arrived_under() {
    let (client, _rx, shared) = test_client();
    // One contract, two streams: every trade, and every quote change. Looked
    // up by contract, both would be handed whichever request was made last.
    client.core.instrument_to_req.lock().unwrap().insert(0, 99);
    shared.market.push_tbt_trade(TbtTrade {
        req_id: 10,
        instrument: 0, price: 150 * PRICE_SCALE, size: 100 * crate::types::QTY_SCALE,
        timestamp: 0, exchange: "".into(), conditions: "".into(),
        past_limit: false,
        unreported: false,
    });
    shared.market.push_tbt_quote(TbtQuote {
        req_id: 11,
        instrument: 0, bid: 150 * PRICE_SCALE, ask: 151 * PRICE_SCALE,
        bid_size: crate::types::QTY_SCALE, ask_size: crate::types::QTY_SCALE,
        timestamp: 0,
        bid_past_low: false,
        ask_past_high: false,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(
        w.events.iter().any(|e| e.starts_with("tbt_last:10:")),
        "the trade did not carry its own request: {:?}", w.events,
    );
    assert!(
        w.events.iter().any(|e| e.starts_with("tbt_bidask:11:")),
        "the quote did not carry its own request: {:?}", w.events,
    );
    assert!(
        !w.events.iter().any(|e| e.contains(":99:")),
        "a record was attributed by contract rather than by request",
    );
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

/// A bulletin subscription starts from the moment it is made unless `all_msgs`
/// is set.
///
/// Bulletins are broadcast at the session whether or not anything is subscribed,
/// so those already queued are discarded on subscribing unless asked for.
#[test]
fn a_bulletin_subscription_starts_where_the_caller_says_it_does() {
    let earlier = || NewsBulletin {
        msg_id: 7, msg_type: 1, message: "Published before anyone asked".into(),
        exchange: "NYSE".into(),
    };

    let (client, _rx, shared) = test_client();
    shared.market.push_news_bulletin(earlier());
    client.req_news_bulletins(false);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(
        !w.events.iter().any(|e| e.starts_with("news_bulletin:7")),
        "a subscription for what follows opened with what came before: {:?}",
        w.events,
    );

    // And what is published after it started arrives.
    shared.market.push_news_bulletin(NewsBulletin {
        msg_id: 8, msg_type: 1, message: "Published after".into(), exchange: "NYSE".into(),
    });
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e.starts_with("news_bulletin:8")));

    // Asking for the day's own is answered with the ones already held.
    let (asks_for_all, _rx, shared) = test_client();
    shared.market.push_news_bulletin(earlier());
    asks_for_all.req_news_bulletins(true);
    let mut w = RecordingWrapper::default();
    asks_for_all.process_msgs(&mut w);
    assert!(
        w.events.iter().any(|e| e.starts_with("news_bulletin:7")),
        "the day's own were asked for and not delivered: {:?}",
        w.events,
    );
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
        min_commission: 0,
        max_commission: 0,
        commission_currency: String::new(),
        warning_text: String::new(),
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
        min_commission: 0,
        max_commission: 0,
        commission_currency: String::new(),
        warning_text: String::new(),
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

/// A head timestamp is returned in the form `format_date` asked for, as bars
/// are. 2 = seconds since the epoch.
#[test]
fn a_head_timestamp_is_written_the_way_it_was_asked_for() {
    let (client, _rx, shared) = test_client();
    client.req_head_time_stamp(11, &spy(), "TRADES", true, 2).expect("the request is sent");
    shared.reference.push_head_timestamp(11, HeadTimestampResponse {
        head_timestamp: "20200101-00:00:00".into(), timezone: String::new(),
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(
        w.events.iter().any(|e| e == "head_timestamp:11:1577836800"),
        "asked for in seconds since the epoch: {:?}",
        w.events,
    );

    // A request asking for format 1 keeps the wire's own spelling.
    client.req_head_time_stamp(12, &spy(), "TRADES", true, 1).expect("the request is sent");
    shared.reference.push_head_timestamp(12, HeadTimestampResponse {
        head_timestamp: "20200101-00:00:00".into(), timezone: String::new(),
    });
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "head_timestamp:12:20200101-00:00:00"));
}

/// A reused request id starts a new request: its completion latch is cleared, so
/// the bars arrive as initial data and `historical_data_end` fires again.
#[test]
fn a_historical_request_under_a_used_id_answers_from_the_beginning() {
    let (client, _rx, shared) = test_client();
    // As the first request left it.
    client.core.hist_initial_complete.lock().unwrap().insert(13);

    client
        .req_historical_data(13, &spy(), "", "1 D", "1 hour", "TRADES", true, 1, false)
        .expect("the request is sent");
    shared.reference.push_historical_data(13, HistoricalResponse {
        query_id: String::new(), timezone: String::new(),
        bars: vec![HistoricalBar {
            time: "20200101-00:00:00".into(), open: 1.0, high: 1.0, low: 1.0, close: 1.0,
            volume: 1, wap: 1.0, count: 1,
        }],
        is_complete: true,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(
        w.events.iter().any(|e| e.starts_with("historical_data:13:")),
        "the bars answering a new request arrived as updates to the old one: {:?}",
        w.events,
    );
    assert!(
        w.events.iter().any(|e| e.starts_with("historical_data_end:13")),
        "and the request never ended: {:?}",
        w.events,
    );
}

/// A trade callback names the stream it carries: tick type 1 = Last,
/// 2 = AllLast. The trade record does not carry it, so the request's kind is.
#[test]
fn a_trade_stream_says_which_of_the_two_it_is() {
    let (client, rx, shared) = test_client();
    // Long enough for the stand-in below to be scheduled. The tests default to
    // a millisecond, which is a real engine answering from another thread and
    // a flake when the machine is busy.
    client.core.set_registration_timeout(std::time::Duration::from_secs(5));
    // Standing in for the engine, which answers a subscription by naming the
    // slot it took. Without an answer the call waits out its registration.
    let engine = std::thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            if let ControlCommand::SubscribeTbt { reply_tx: Some(reply), .. } = cmd {
                let _ = reply.try_send(Ok(0));
            }
        }
    });
    client.req_tick_by_tick_data(20, &spy(), "AllLast", 0, false).expect("subscribed");
    client.req_tick_by_tick_data(21, &spy(), "Last", 0, false).expect("subscribed");

    for req_id in [20, 21] {
        shared.market.push_tbt_trade(crate::types::TbtTrade {
            instrument: 0, req_id, price: PRICE_SCALE, size: 1, timestamp: 0,
            exchange: "NYSE".into(), conditions: String::new(),
            past_limit: false, unreported: false,
        });
    }
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(
        w.events.iter().any(|e| e.starts_with("tbt_last:20:2:")),
        "every trade was reported as the exchange's own: {:?}",
        w.events,
    );
    assert!(w.events.iter().any(|e| e.starts_with("tbt_last:21:1:")));

    drop(client);
    engine.join().expect("the stand-in engine");
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
//  process_msgs — option chain
// ═══════════════════════════════════════════════════════════════════

/// Every class of the underlying is reported, and the request ends once.
#[test]
fn process_msgs_dispatches_option_chain_parameters() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_option_params(9, 265598, vec![
        OptionChainScope {
            symbol: "AAPL".into(), exchange: "SMART".into(), trading_class: "AAPL".into(),
            multiplier: "100".into(), expirations: vec!["20260116".into(), "20260320".into()],
            strikes: vec![140.0, 145.0],
        },
        OptionChainScope {
            symbol: "AAPL".into(), exchange: "CBOE".into(), trading_class: "AAPL1".into(),
            multiplier: "100".into(), expirations: vec!["20260116".into()],
            strikes: vec![145.0],
        },
    ]);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(w.events.iter().any(|e| e == "sec_def_opt_param:9:SMART:265598:AAPL:100:20260116,20260320:140,145"), "{:?}", w.events);
    assert!(w.events.iter().any(|e| e == "sec_def_opt_param:9:CBOE:265598:AAPL1:100:20260116:145"), "{:?}", w.events);
    assert_eq!(w.events.iter().filter(|e| *e == "sec_def_opt_param_end:9").count(), 1);
}

/// A chain the venue lists nothing for is still an answer: the caller is
/// waiting on the end of the request, not on a class that does not exist.
#[test]
fn process_msgs_ends_an_empty_option_chain() {
    let (client, _rx, shared) = test_client();
    shared.reference.push_option_params(9, 265598, Vec::new());
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert_eq!(w.events, vec!["sec_def_opt_param_end:9".to_string()]);
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
        error_text: String::new(),
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

/// Each historical-tick variant routes to its own callback, as in ibapi,
/// rather than all three arriving through `historical_ticks`.
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
        price: PRICE_SCALE, qty: crate::types::QTY_SCALE, remaining: 0,
        commission: 0, timestamp_ns: 0,
        cum_qty: crate::types::QTY_SCALE, avg_price: PRICE_SCALE,
    });
    shared.orders.push_order_update(OrderUpdate {
        order_id: 2, instrument: 0, status: OrderStatus::Submitted,
        filled_qty: 0.0, remaining_qty: 1.0, avg_price: 0, perm_id: 0, parent_id: 0, timestamp_ns: 0,
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
//  process_msgs — a fill answers no request unless one asked for it
// ═══════════════════════════════════════════════════════════════════

/// An unsolicited fill is reported against request id -1. A market-data
/// subscription id does not identify a `reqExecutions` request.
#[test]
fn a_fill_that_answers_no_request_is_reported_against_none() {
    let (client, _rx, shared) = test_client();
    client.core.instrument_to_req.lock().unwrap().insert(0, 42);
    shared.orders.push_fill(Fill {
        instrument: 0, order_id: 1, side: Side::Buy,
        price: PRICE_SCALE, qty: 100 * crate::types::QTY_SCALE, remaining: 0,
        commission: 0, timestamp_ns: 0,
        cum_qty: 100 * crate::types::QTY_SCALE, avg_price: PRICE_SCALE,
    });
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert!(
        w.events.iter().any(|e| e.starts_with("exec_details:-1:")),
        "the fill was numbered after a quote subscription: {:?}",
        w.events.iter().filter(|e| e.starts_with("exec_details")).collect::<Vec<_>>(),
    );
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
            assert_eq!(qty, 100 * crate::types::QTY_SCALE);
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
        price: 150 * PRICE_SCALE, qty: 100 * crate::types::QTY_SCALE, remaining: 0,
        commission: 0, timestamp_ns: 1000,
        cum_qty: 100 * crate::types::QTY_SCALE, avg_price: 150 * PRICE_SCALE,
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
        if let ControlCommand::Order(OrderRequest::Modify { order_id: 88, price, qty, tif, .. }) = cmd {
            assert_eq!(price, (150.0 * PRICE_SCALE_F) as i64);
            assert_eq!(qty, 100 * crate::types::QTY_SCALE);
            // The change the test is named for. Asserting only that a Modify
            // was emitted passed for as long as the time-in-force was dropped.
            assert_eq!(tif, b'1', "the modify must carry GTC, not restate DAY");
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
            assert_eq!(qty, 200 * crate::types::QTY_SCALE);
            assert_eq!(price, (148.0 * PRICE_SCALE_F) as i64);
            found = true;
        }
    }
    assert!(found, "Resubmit with same orderId should emit Modify with new price and qty");
}

/// A zero order-type states nothing and keeps the resting type, so a modify to
/// a type the replace cannot express must be refused rather than reaching the
/// encoder — otherwise the caller's new type is silently restated as the old
/// one and the client caches an order the venue does not have.
#[test]
fn a_modify_to_an_unrepresentable_type_is_refused() {
    for order_type in ["REL", "TRAIL", "LIT", "MIDPX", "SNAP MKT"] {
        let (client, rx, shared) = test_client();
        shared.market.set_instrument_count(1);
        let plain = Order {
            action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
            lmt_price: 100.0, tif: "DAY".into(), ..Default::default()
        };
        client.place_order(9401, &spy(), &plain).expect("a plain limit submits");
        while rx.try_recv().is_ok() {}

        let converted = Order {
            order_type: order_type.into(), aux_price: 99.0, ..plain.clone()
        };
        let err = client.place_order(9401, &spy(), &converted)
            .expect_err("converting to a type the replace cannot express must be refused");
        assert!(err.message.contains("cannot be modified"), "{order_type}: {err}");
        assert!(rx.try_recv().is_err(), "{order_type}: nothing reaches the wire");
    }
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
        if let ControlCommand::Order(OrderRequest::Modify {
            order_id: 66, ord_type, stop_price, ..
        }) = cmd {
            // The change the test is named for. Asserting only that a Modify
            // was emitted passed for as long as the order type was dropped.
            assert_eq!(ord_type, b'3', "the modify must carry STP, not restate LMT");
            assert_eq!(stop_price, (149.0 * PRICE_SCALE_F) as i64,
                "and the trigger the caller set");
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
    let (tx, _rx) = std::sync::mpsc::sync_channel(4096);
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
// Connection loss
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

/// The code provider must reach the session config for the authenticator factor
/// to be usable. It is one field in a struct literal and no other test covers
/// it.
#[test]
fn the_second_factor_provider_reaches_the_gateway_config() {
    use crate::api::client::gateway_config;

    let base = crate::api::client::EClientConfig {
        username: "u".into(), password: "p".into(), host: "h".into(),
        paper: false, core_id: None, code_provider: None,
        ..Default::default()
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


/// A display group is how two callers on one session agree on a contract. The
/// venue is not involved and never was, so the whole behaviour is this
/// client's to reproduce: what the groups are, what each holds, and who is
/// told when one changes.
#[test]
fn a_display_group_keeps_its_followers_in_step() {
    let (client, _rx, _shared) = test_client();

    client.query_display_groups(1);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert_eq!(
        w.events.iter().find(|e| e.starts_with("display_group_list:")).map(String::as_str),
        Some("display_group_list:1:1|2|3|4|5|6|7"),
        "the groups on offer: {:?}", w.events,
    );

    // Two callers follow the same group; a third follows another.
    client.subscribe_to_group_events(10, 3);
    client.subscribe_to_group_events(11, 3);
    client.subscribe_to_group_events(12, 4);
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    assert_eq!(
        w.events.iter().filter(|e| e.ends_with(":none")).count(), 3,
        "each is told what its group holds now, not only what it changes to: {:?}", w.events,
    );

    // One of them puts a contract in it.
    client.update_display_group(10, "756733@SMART").unwrap();
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    let told: Vec<&String> = w.events.iter()
        .filter(|e| e.starts_with("display_group_updated:")).collect();
    assert_eq!(told.len(), 2, "both followers of that group, and only those: {told:?}");
    assert!(told.iter().all(|e| e.ends_with(":756733@SMART")), "{told:?}");
    assert!(told.iter().any(|e| e.contains(":10:")), "including the one that changed it: {told:?}");
    assert!(told.iter().any(|e| e.contains(":11:")), "{told:?}");

    // A caller that follows nothing has no group to put a contract in.
    let refusal = client.update_display_group(99, "1@SMART").unwrap_err();
    assert!(refusal.message.contains("follows no display group"), "{refusal}");

    // Once it stops following, it is no longer told.
    client.unsubscribe_from_group_events(11);
    client.update_display_group(10, "").unwrap();
    let mut w = RecordingWrapper::default();
    client.process_msgs(&mut w);
    let told: Vec<&String> = w.events.iter()
        .filter(|e| e.starts_with("display_group_updated:")).collect();
    assert_eq!(told.len(), 1, "only the one still following: {told:?}");
    assert!(told[0].ends_with(":none"), "and an empty contract empties the group: {told:?}");
}

/// A login holding more than one account is answered with all of them, comma
/// separated, led by the account a caller gets by default. Answering with only
/// the first is how a caller managing linked accounts stops seeing the rest.
#[test]
fn managed_accounts_names_every_account_the_login_holds() {
    #[derive(Default)]
    struct W(Vec<String>);
    impl crate::api::wrapper::Wrapper for W {
        fn managed_accounts(&mut self, accounts: &str) { self.0.push(accounts.to_string()); }
    }

    let (mut client, _rx, _shared) = test_client();
    let mut w = W::default();

    // One account: answered with that account and no comma.
    client.req_managed_accts(&mut w);
    assert_eq!(w.0, vec!["DU123".to_string()]);

    client.accounts = vec!["DU123".into(), "DU456".into(), "DU789".into()];
    client.req_managed_accts(&mut w);
    assert_eq!(w.0[1], "DU123,DU456,DU789");
}

#[cfg(test)]
mod answering_calls_receive_through_dispatch {
    use crate::bridge::SharedState;
    use crate::control::contracts::ContractDefinition;

    /// This shape's answering calls receive through the dispatch loop, so the
    /// drain that feeds it must hand over their replies too.
    ///
    /// Withholding them was a change made for the other shape, whose answering
    /// calls take replies out of the queue by id and so need them left alone.
    /// One drain served both, and the change broke every answering call here
    /// while every offline test kept passing — the queues in those tests are
    /// filled by hand, so nothing depended on the drain being the delivery.
    #[test]
    fn a_reply_to_an_answering_call_is_not_withheld_from_the_dispatch_that_delivers_it() {
        let shared = SharedState::new();
        let ask_id = crate::bridge::ReferenceState::ASK_ID_BASE;
        shared.reference.push_contract_details(
            ask_id,
            ContractDefinition { con_id: 756733, ..Default::default() },
        );

        let delivered = shared.reference.drain_contract_details();
        assert_eq!(
            delivered.len(),
            1,
            "an answering call's reply was withheld from the drain that delivers it"
        );
        assert_eq!(delivered[0].0, ask_id);
    }

    /// The other shape still gets its replies left where it will find them.
    #[test]
    fn the_dispatch_that_does_not_deliver_them_still_leaves_them() {
        let shared = SharedState::new();
        let ask_id = crate::bridge::ReferenceState::ASK_ID_BASE;
        shared.reference.push_contract_details(
            ask_id,
            ContractDefinition { con_id: 756733, ..Default::default() },
        );
        shared.reference.push_contract_details(
            7,
            ContractDefinition { con_id: 111, ..Default::default() },
        );

        let delivered = shared.reference.drain_contract_details_for_dispatch();
        assert_eq!(delivered.len(), 1, "a caller's own reply was withheld");
        assert_eq!(delivered[0].0, 7);
        assert_eq!(shared.reference.take_contract_details_for(ask_id).len(), 1);
    }
}

/// A keep-up-to-date request answers once with its history and then keeps
/// speaking. The reference client separates the two, and this surface reported
/// only the first: a caller that overrode the update callback heard nothing,
/// and the continued bars arrived as real-time bars it never asked for.
#[test]
fn a_kept_up_to_date_request_reports_its_history_then_its_updates() {
    #[derive(Default)]
    struct Heard {
        history: Vec<i64>,
        ended: Vec<i64>,
        updates: Vec<i64>,
        real_time: Vec<i64>,
    }
    impl Wrapper for Heard {
        fn historical_data(&mut self, req_id: i64, _bar: &crate::types::model::BarData) { self.history.push(req_id); }
        fn historical_data_end(&mut self, req_id: i64, _s: &str, _e: &str) { self.ended.push(req_id); }
        fn historical_data_update(&mut self, req_id: i64, _bar: &crate::types::model::BarData) { self.updates.push(req_id); }
        fn real_time_bar(&mut self, req_id: i64, _t: i64, _o: f64, _h: f64, _l: f64,
                         _c: f64, _v: f64, _w: f64, _n: i32) { self.real_time.push(req_id); }
    }

    let (client, _rx, shared) = test_client();
    let mut heard = Heard::default();

    // The initial answer, complete.
    shared.reference.push_historical_data(9, HistoricalResponse {
        query_id: String::new(), timezone: String::new(),
        bars: vec![HistoricalBar { time: "20260101".into(), open: 100.0, high: 105.0, low: 99.0, close: 103.0, volume: 1000, wap: 102.0, count: 50 }],
        is_complete: true,
    });
    client.process_msgs(&mut heard);

    assert_eq!(heard.history, vec![9], "the history is history");
    assert_eq!(heard.ended, vec![9], "and it says when it has finished");
    assert!(heard.updates.is_empty(), "nothing is an update yet");

    // What the venue keeps sending afterwards, on both feeds it can arrive on.
    shared.reference.push_historical_data(9, HistoricalResponse {
        query_id: String::new(), timezone: String::new(),
        bars: vec![HistoricalBar { time: "20260101".into(), open: 100.0, high: 105.0, low: 99.0, close: 103.0, volume: 1000, wap: 102.0, count: 50 }],
        is_complete: false,
    });
    shared.market.push_real_time_bar(9, Default::default());
    client.process_msgs(&mut heard);

    assert_eq!(heard.updates, vec![9, 9], "both continued bars are updates");
    assert_eq!(heard.history, vec![9], "and neither is more history");
    assert_eq!(heard.ended, vec![9], "nor a second end");
    assert!(
        heard.real_time.is_empty(),
        "a request nobody made is not answered with real-time bars",
    );
}

/// The account subscription reports what the account holds as well as what it
/// is worth. A caller watching its positions through it heard only the values.
#[test]
fn subscribing_to_account_updates_reports_the_portfolio() {
    #[derive(Default)]
    struct Heard {
        values: Vec<String>,
        positions: Vec<(i64, f64)>,
    }
    impl Wrapper for Heard {
        fn update_account_value(&mut self, key: &str, _v: &str, _c: &str, _a: &str) {
            self.values.push(key.to_string());
        }
        fn update_portfolio(&mut self, contract: &Contract, position: f64, _mp: f64, _mv: f64,
                            _ac: f64, _up: f64, _rp: f64, _acct: &str) {
            self.positions.push((contract.con_id, position));
        }
    }

    let (client, _rx, shared) = test_client();
    let mut heard = Heard::default();

    client.req_account_updates(true, "DU123");
    shared.portfolio.set_position_info(crate::types::PositionInfo {
        con_id: 756733,
        position: 100.0,
        avg_cost: 490 * crate::types::PRICE_SCALE,
        symbol: "SPY".into(),
        sec_type: "STK".into(),
        ..Default::default()
    });
    shared.portfolio.note_account_value("NetLiquidation", "100000.00", "USD");
    client.process_msgs(&mut heard);

    assert!(heard.values.contains(&"NetLiquidation".to_string()), "the values still arrive");
    assert_eq!(
        heard.positions, vec![(756733, 100.0)],
        "and the holding they describe arrives with them",
    );
}

/// A calculation this client makes answers the call that asked for it.
///
/// The caller's request id was stored in the field naming the option, and the
/// dispatcher then read it as one and mapped it again — so the answer arrived
/// under an unrelated subscription's id, or under none at all.
#[test]
fn a_local_option_calculation_answers_the_request_that_asked() {
    #[derive(Default)]
    struct Heard(Vec<i64>);
    impl Wrapper for Heard {
        fn tick_option_computation(&mut self, req_id: i64, _tick: i32, _attr: i32,
                                   _iv: f64, _d: f64, _op: f64, _pv: f64, _g: f64,
                                   _v: f64, _t: f64, _up: f64) {
            self.0.push(req_id);
        }
    }

    let (client, _rx, shared) = test_client();
    let mut heard = Heard::default();

    // An id far outside the instrument table, so reading it as one cannot
    // accidentally land on the right answer.
    let asked = 4242i64;
    shared.market.push_option_computation(crate::types::OptionComputation {
        answers: Some(asked),
        implied_vol: 0.25,
        ..Default::default()
    });
    client.process_msgs(&mut heard);

    assert_eq!(heard.0, vec![asked], "the answer names the call that asked for it");
}

/// `reqCurrentTime` asks for the venue's clock, not this machine's.
///
/// A caller asks it to learn how far apart the two are, and the local clock is
/// the one number that cannot tell them. The venue stamps every message it
/// sends; the last stamp is the answer.
#[test]
fn the_current_time_is_the_venues_own() {
    #[derive(Default)]
    struct Heard(Vec<i64>);
    impl Wrapper for Heard {
        fn current_time(&mut self, t: i64) { self.0.push(t); }
    }

    let (client, _rx, shared) = test_client();
    let mut heard = Heard::default();

    // Before the venue has said anything, there is nothing but this clock.
    client.req_current_time(&mut heard);
    let local = heard.0[0];
    assert!(local > 1_700_000_000, "a plausible instant");

    // Once it has, its own stamp is what a caller is told.
    shared.market.note_venue_time("20260815-12:00:00");
    client.req_current_time(&mut heard);
    assert_eq!(
        heard.0[1], 1_786_795_200,
        "the venue's stamp, read back to seconds",
    );
    assert_ne!(heard.0[1], local, "and not this machine's clock");
}

/// Arguments this protocol cannot carry are refused rather than dropped.
///
/// A tick-by-tick subscription states the contract and the kind of stream and
/// nothing else. A caller that asked for a prelude of past ticks, or for
/// size-only changes to be suppressed, and was answered anyway would be
/// reading a stream it did not ask for with nothing to say so.
#[test]
fn tick_by_tick_refuses_what_it_cannot_ask_for() {
    let (client, _rx, _shared) = test_client();

    let asked_for_history = client.req_tick_by_tick_data(1, &spy(), "Last", 100, false);
    assert!(asked_for_history.is_err(), "a prelude of past ticks cannot be asked for here");

    let asked_to_drop_sizes = client.req_tick_by_tick_data(2, &spy(), "Last", 0, true);
    assert!(asked_to_drop_sizes.is_err(), "nor can size-only changes be suppressed");

    // And the refusal names the argument rather than the request, so a caller
    // can tell this from a contract or entitlement problem.
    let why = asked_for_history.unwrap_err();
    assert!(why.message.contains("number_of_ticks"), "{}", why.message);
    assert!(asked_to_drop_sizes.unwrap_err().message.contains("ignore_size"));
}

/// A session that came back and went again is not a connected session.
///
/// Loss and recovery were two flags with no order between them. Both raised,
/// the dispatcher applied recovery last whichever way the connection had
/// actually gone — so a client reported itself connected to a socket that had
/// dropped, and nothing was left pending to correct it.
#[test]
fn the_last_thing_the_connection_did_is_what_a_caller_is_told() {
    let (client, _rx, shared) = test_client();
    let mut w = RecordingWrapper::default();

    // Lost, recovered, and lost again before anyone looked.
    shared.set_connection_lost();
    shared.set_connection_restored();
    shared.set_connection_lost();
    client.process_msgs(&mut w);

    assert!(!client.is_connected(), "the connection went and did not come back");
    assert!(
        w.events.iter().any(|e| e == "connection_closed"),
        "and the caller is told once",
    );

    // The other way round: a recovery after a loss stands.
    let (client, _rx, shared) = test_client();
    let mut w = RecordingWrapper::default();
    shared.set_connection_lost();
    shared.set_connection_restored();
    client.process_msgs(&mut w);
    assert!(client.is_connected(), "the connection came back");
}

/// Two callers subscribing one contract at once: one holds it, the other
/// follows. Deciding and taking are one acquisition, so they cannot both read
/// the contract as free and both take it — which left the second write owning
/// the mapping and the first request quiet, with nothing to say why.
#[test]
fn one_contract_has_one_owner_however_many_ask_at_once() {
    use std::sync::Arc;

    let (client, _rx, _shared) = test_client();
    let core = Arc::new(client);
    let instrument = 0u32;

    let barrier = Arc::new(std::sync::Barrier::new(8));
    let claimed: Arc<std::sync::Mutex<Vec<i64>>> = Arc::new(std::sync::Mutex::new(Vec::new()));

    std::thread::scope(|scope| {
        for req_id in 1..=8i64 {
            let core = Arc::clone(&core);
            let barrier = Arc::clone(&barrier);
            let claimed = Arc::clone(&claimed);
            scope.spawn(move || {
                barrier.wait();
                if !core.core.take_or_follow(instrument, req_id) {
                    claimed.lock().unwrap().push(req_id);
                }
            });
        }
    });

    let owners = claimed.lock().unwrap();
    assert_eq!(owners.len(), 1, "exactly one request holds the contract: {owners:?}");
    assert_eq!(
        core.core.instrument_to_req.lock().unwrap().get(&instrument),
        Some(&owners[0]),
        "and the mapping names that one",
    );
    assert_eq!(
        core.core.followers_of(instrument).len(), 7,
        "everybody else follows it rather than being dropped",
    );
}

/// An option solve is computed locally against the model the venue published for
/// that contract. The protocol carries no request for one.
#[test]
fn solving_an_option_answers_against_the_venues_own_model() {
    #[derive(Default)]
    struct Heard {
        computed: Vec<(i64, f64)>,
        greeks: Vec<(f64, f64, f64, f64)>,
        errors: Vec<String>,
    }
    impl Wrapper for Heard {
        fn tick_option_computation(&mut self, req_id: i64, _t: i32, _a: i32, _iv: f64,
                                   delta: f64, opt_price: f64, _pv: f64, gamma: f64,
                                   vega: f64, theta: f64, _up: f64) {
            self.computed.push((req_id, opt_price));
            self.greeks.push((delta, gamma, vega, theta));
        }
        fn error(&mut self, _req_id: i64, _code: i64, msg: &str, _adv: &str) {
            self.errors.push(msg.to_string());
        }
    }

    let (client, _rx, shared) = test_client();
    let mut heard = Heard::default();

    let mut option = spy();
    option.con_id = 756733;
    option.sec_type = "OPT".into();
    option.strike = 500.0;
    option.right = "C".into();
    option.last_trade_date_or_contract_month = "20270115".into();

    // With no published model there is nothing to solve against; the call reports
    // that rather than inventing a rate.
    client.calculate_option_price(5, &option, 0.25, 505.0);
    client.process_msgs(&mut heard);
    assert!(heard.computed.is_empty(), "no model, no answer");
    assert!(!heard.errors.is_empty(), "and the caller is told why");

    // With it, the answer is solved and delivered under the caller's request.
    shared.market.push_option_computation(crate::types::OptionComputation {
        answers: None,
        instrument: 0,
        implied_vol: 0.20,
        opt_price: 30.0,
        und_price: 505.0,
        ..Default::default()
    });
    let _ = shared.market.drain_option_computations();
    heard.errors.clear();

    client.calculate_option_price(6, &option, 0.25, 505.0);
    client.process_msgs(&mut heard);

    assert!(heard.errors.is_empty(), "{:?}", heard.errors);
    assert_eq!(heard.computed.len(), 1, "the price was answered");
    assert_eq!(heard.computed[0].0, 6, "under the request that asked for it");
    assert!(heard.computed[0].1 > 0.0, "and it is a price");

    // The question the other way round — what volatility a price implies —
    // is solved against the same model and answered the same way.
    heard.computed.clear();
    client.calculate_implied_volatility(7, &option, 32.0, 505.0);
    client.process_msgs(&mut heard);

    assert!(heard.errors.is_empty(), "{:?}", heard.errors);
    assert_eq!(heard.computed.len(), 1, "the volatility was answered");
    assert_eq!(heard.computed[0].0, 7);

    // Fields this does not compute carry the unset sentinel. Zero is a valid
    // greek and cannot stand for one.
    let (delta, gamma, vega, theta) = heard.greeks[0];
    for (name, stated) in [("delta", delta), ("gamma", gamma), ("vega", vega), ("theta", theta)] {
        assert_eq!(stated, f64::MAX, "{name} was answered as a number nobody worked out");
    }
}

/// Completed orders are retained: the arrival queue empties on read and the
/// venue does not resend them, so later calls answer from the archive.
#[test]
fn completed_orders_are_still_there_when_they_are_asked_for_again() {
    let (client, _rx, shared) = test_client();
    shared.orders.push_completed_order(crate::types::CompletedOrder {
        order_id: 31, instrument: 0, status: crate::types::OrderStatus::Filled,
        filled_qty: 100, timestamp_ns: 0,
    });

    let mut w = RecordingWrapper::default();
    client.req_completed_orders(false, &mut w);
    assert_eq!(w.events.iter().filter(|e| *e == "completed_order").count(), 1);

    let mut again = RecordingWrapper::default();
    client.req_completed_orders(false, &mut again);
    assert_eq!(
        again.events.iter().filter(|e| *e == "completed_order").count(),
        1,
        "asked a second time, the account read as having completed nothing: {:?}",
        again.events,
    );
}

/// A request that names its contract by contract id refuses one carrying none.
///
/// These carry tag 6008 and nothing else of the contract. Contract id 0 and a
/// negative id are both answered with silence, which reads as no data.
#[test]
fn a_request_named_by_id_refuses_a_contract_that_has_none() {
    let (client, _rx, _shared) = test_client();
    let described = crate::types::model::Contract {
        symbol: "SPY".into(), sec_type: "STK".into(), exchange: "SMART".into(),
        ..Default::default()
    };
    assert!(client.req_fundamental_data(1, &described, "ReportSnapshot").is_err());
    assert!(client.req_histogram_data(2, &described, true, "3 days").is_err());
    assert!(client.req_historical_news(3, -1, "BRFG", "", "", 5).is_err());
    assert!(
        client.req_historical_ticks(4, &spy(), "", "", -1, "TRADES", true).is_err(),
        "a count below zero asked for four billion ticks",
    );

    // And one that carries the id is sent.
    assert!(client.req_fundamental_data(5, &spy(), "ReportSnapshot").is_ok());
    assert!(client.req_histogram_data(6, &spy(), true, "3 days").is_ok());
}

/// A depth request on a contract naming no exchange and no security type is sent
/// as it stands.
///
/// An unnamed exchange is already routed as SMART, and a named security type is
/// checked against the routing table, so writing STK in refuses books that
/// exist for other types.
#[test]
fn a_depth_request_states_the_contract_it_was_given() {
    let (client, rx, _shared) = test_client();
    let by_id = crate::types::model::Contract { con_id: 495512563, ..Default::default() };
    client.req_mkt_depth(1, &by_id, 5, false).expect("the request is sent");
    match rx.try_recv().expect("the subscription") {
        ControlCommand::SubscribeDepth { contract, .. } => {
            assert_eq!(contract.sec_type, "", "a security type nobody stated");
            assert_eq!(contract.exchange, "", "a venue nobody stated");
            assert_eq!(contract.con_id, 495512563);
        }
        other => panic!("expected SubscribeDepth, got {other:?}"),
    }
}

/// A caller chooses how its bar times are written, and the choice is per
/// request. Discarded, a caller that asked for seconds since the epoch is
/// handed the wire's spelling and reads a date where it expects a number.
#[test]
fn a_request_gets_its_bar_times_written_the_way_it_asked() {
    #[derive(Default)]
    struct Heard(Vec<(i64, String)>);
    impl Wrapper for Heard {
        fn historical_data(&mut self, req_id: i64, bar: &crate::types::model::BarData) {
            self.0.push((req_id, bar.date.clone()));
        }
        // A request that has already answered with its history keeps speaking
        // on this one, and its times are written the same way.
        fn historical_data_update(&mut self, req_id: i64, bar: &crate::types::model::BarData) {
            self.0.push((req_id, bar.date.clone()));
        }
    }

    let (client, _rx, shared) = test_client();
    let mut heard = Heard::default();

    let bar = HistoricalBar {
        time: "20260815-12:00:00".into(), open: 1.0, high: 2.0, low: 0.5,
        close: 1.5, volume: 10, wap: 1.2, count: 3,
    };
    // Format 1 is the wire's own spelling.
    let _ = client.req_historical_data(
        1, &spy(), "", "1 D", "1 day", "TRADES", true, 1, false,
    );
    shared.reference.push_historical_data(1, HistoricalResponse {
        query_id: String::new(), timezone: String::new(),
        bars: vec![bar.clone()], is_complete: true,
    });
    client.process_msgs(&mut heard);
    assert_eq!(heard.0[0].1, "20260815-12:00:00", "the wire's own spelling");

    // And seconds since the epoch for the request that asked for them.
    let _ = client.req_historical_data(
        2, &spy(), "", "1 D", "1 day", "TRADES", true, 2, false,
    );
    shared.reference.push_historical_data(2, HistoricalResponse {
        query_id: String::new(), timezone: String::new(),
        bars: vec![bar.clone()], is_complete: true,
    });
    client.process_msgs(&mut heard);
    assert_eq!(heard.0[1].1, "1786795200", "seconds since the epoch");

    // The first request is unaffected: the choice belongs to the request.
    shared.reference.push_historical_data(1, HistoricalResponse {
        query_id: String::new(), timezone: String::new(),
        bars: vec![bar.clone()], is_complete: false,
    });
    client.process_msgs(&mut heard);
    assert_eq!(heard.0[2].1, "20260815-12:00:00", "still the venue's spelling");
}

/// The shorthand states the order a reader would write out, and nothing else.
/// A constructor that quietly set a field a caller had not asked for would put
/// an instruction on the wire that nobody wrote.
#[test]
fn the_shorthand_states_the_order_and_nothing_more() {
    use crate::types::model::Order;
    let plain = Order::default();
    for (what, order, kind, lmt, aux) in [
        ("market", Order::market("BUY", 100.0), "MKT", 0.0, 0.0),
        ("limit", Order::limit("BUY", 100.0, 42.5), "LMT", 42.5, 0.0),
        ("stop", Order::stop("SELL", 100.0, 41.0), "STP", 0.0, 41.0),
        ("stop limit", Order::stop_limit("SELL", 100.0, 41.0, 40.5), "STP LMT", 40.5, 41.0),
    ] {
        assert_eq!(order.order_type, kind, "{what}");
        assert_eq!(order.total_quantity, 100.0, "{what}");
        assert_eq!(order.lmt_price, lmt, "{what}");
        assert_eq!(order.aux_price, aux, "{what}");
        assert_eq!(order.tif, "DAY", "{what}: expires at the close unless said otherwise");
        // Everything this shorthand does not name is left where it was.
        assert_eq!(order.hedge_type, plain.hedge_type, "{what}");
        assert_eq!(order.good_after_time, plain.good_after_time, "{what}");
        assert_eq!(order.origin, plain.origin, "{what}");
        assert_eq!(order.transmit, plain.transmit, "{what}");
    }
    assert_eq!(Order::limit("BUY", 1.0, 10.0).good_till_cancelled().tif, "GTC");
    assert!(Order::market("BUY", 1.0).outside_regular_hours().outside_rth);
}

/// A contract the shorthand names is the one a request would carry. Each of
/// these was read back off a live definition, so a default that drifted from
/// what the venue lists would be a lookup answering about something else.
#[test]
fn the_shorthand_names_the_contract_a_request_carries() {
    use crate::types::model::Contract;
    let spy = Contract::stock("SPY");
    assert_eq!((spy.sec_type.as_str(), spy.exchange.as_str(), spy.currency.as_str()),
               ("STK", "SMART", "USD"));

    let call = Contract::call("AAPL", 150.0, "20261218");
    assert_eq!(call.sec_type, "OPT");
    assert_eq!((call.right.as_str(), call.strike), ("C", 150.0));
    assert_eq!(call.last_trade_date_or_contract_month, "20261218");
    assert!(
        call.multiplier.is_empty(),
        "an option's multiplier identifies the listing and is left to the venue",
    );
    assert_eq!(Contract::put("AAPL", 150.0, "20261218").right, "P");

    let es = Contract::future("ES", "202612", "CME");
    assert_eq!((es.sec_type.as_str(), es.exchange.as_str()), ("FUT", "CME"));
    // A future is quoted in whatever its venue quotes in, so nothing here
    // assumes dollars — assumed, a Eurex contract would be asked about in a
    // currency it is not listed in.
    assert!(es.currency.is_empty(), "a future's currency is the venue's");
    assert!(Contract::index("SPX", "CBOE").currency.is_empty());

    let eurusd = Contract::forex("EUR", "USD");
    assert_eq!((eurusd.sec_type.as_str(), eurusd.symbol.as_str(),
                eurusd.currency.as_str(), eurusd.exchange.as_str()),
               ("CASH", "EUR", "USD", "IDEALPRO"));

    // A contract stated by id carries nothing else: a symbol beside an id that
    // disagreed with it is a description of two different contracts.
    let by_id = Contract::by_id(756733);
    assert_eq!(by_id.con_id, 756733);
    assert!(by_id.symbol.is_empty() && by_id.sec_type.is_empty());

    let toyota = Contract::stock("7203").on_exchange("TSEJ").in_currency("JPY");
    assert_eq!((toyota.exchange.as_str(), toyota.currency.as_str()), ("TSEJ", "JPY"));
}

/// A field left alone is not a field asked for. Several of the twenty-nine
/// carry a non-zero default — `what_if_type` is `i32::MAX`, `exempt_code` is
/// `-1` — so a refusal written against emptiness rather than against the
/// default would reject every order anyone ever placed.
#[test]
fn an_order_nobody_touched_is_not_refused_for_what_it_does_not_carry() {
    let (client, _rx, _shared) = test_client();
    let order = Order {
        action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
        lmt_price: 100.0, tif: "DAY".into(), ..Default::default()
    };
    client.place_order(9501, &spy(), &order).expect("a plain order is placed");

    // And the same order built by the shorthand, which fills no more than it
    // names.
    let (client, _rx, _shared) = test_client();
    client
        .place_order(9502, &spy(), &Order::limit("BUY", 1.0, 100.0))
        .expect("the shorthand's order is placed");
}

/// Two questions asked at once each get their own answer.
///
/// A question drives the message pump itself, and the pump hands everything it
/// drains to whichever collector is running — which keeps what carries its own
/// request id and discards the rest. Asked concurrently, the first question
/// read the second's answer, threw it away, and the second waited out its
/// timeout for a reply that had already arrived. With no engine to answer,
/// what this holds is the ordering: neither question is on the wire while the
/// other is listening, so neither can be handed the other's messages.
#[test]
fn two_questions_asked_at_once_do_not_consume_each_other() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let (client, _rx, _shared) = test_client();
    let client = Arc::new(client);
    let overlapping = Arc::new(AtomicUsize::new(0));
    let inside = Arc::new(AtomicUsize::new(0));

    let threads: Vec<_> = (0..4)
        .map(|_| {
            let (client, overlapping, inside) = (
                Arc::clone(&client), Arc::clone(&overlapping), Arc::clone(&inside),
            );
            std::thread::spawn(move || {
                let _turn = client.asking.lock().unwrap_or_else(|e| e.into_inner());
                let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                if now > 1 {
                    overlapping.fetch_add(1, Ordering::SeqCst);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                inside.fetch_sub(1, Ordering::SeqCst);
            })
        })
        .collect();
    for t in threads {
        t.join().expect("a question finishes");
    }
    assert_eq!(
        overlapping.load(Ordering::SeqCst), 0,
        "a second question ran while the first was still listening",
    );
}

/// Every question takes its turn before it sends.
///
/// The test above holds the lock itself, so it stays green if the lock is taken
/// nowhere in the code that ships. This reads the questions instead: one that
/// waits for an answer and does not take a turn is one that can be handed
/// another question's reply, and there is no way to observe that from a test
/// with no session to answer it.
#[test]
fn a_question_takes_its_turn_before_it_sends() {
    let source = include_str!("ask.rs");
    let waits_for_an_answer: Vec<&str> = source
        .split("\n    pub fn ")
        .skip(1)
        .filter(|body| body.contains("self.wait_for(") || body.contains("holding_the_turn("))
        .collect();
    assert!(waits_for_an_answer.len() >= 10, "the reader found the questions");
    let without: Vec<&str> = waits_for_an_answer
        .iter()
        .filter(|body| !body.contains("self.asking.lock()"))
        .map(|body| body.split('(').next().unwrap_or(body))
        .collect();
    assert!(without.is_empty(), "asks without taking a turn: {without:?}");

    // And the one place that sends before it waits holds the turn across both.
    let placing = include_str!("simple.rs");
    let place = placing.split("pub fn place(").nth(1).expect("place is there");
    let body = place.split("\n    }").next().unwrap_or(place);
    let turn = body.find("self.asking.lock()").expect("place takes a turn");
    let send = body.find("self.place_order(").expect("place sends the order");
    assert!(turn < send, "the order is sent before the turn is taken");
}

/// Placing an order for a contract the venue has not named yet does not wait
/// on itself.
///
/// The order is sent under a turn, so that nothing else pumps its reply away.
/// A contract with no id is looked up before it is sent, and a lookup is a
/// question that takes a turn of its own — asked from inside the placing turn,
/// it waits on a turn that is not going to be given up, and the order is never
/// sent at all. Run on a thread so a regression fails the suite instead of
/// hanging it.
#[test]
fn placing_an_unnamed_contract_does_not_wait_on_itself() {
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&done);
    std::thread::spawn(move || {
        let (client, _rx, _shared) = test_client();
        // No engine answers, so the lookup fails or times out — either way it
        // returns. What must not happen is that it never returns at all.
        let unnamed = Contract {
            symbol: "SPY".into(), sec_type: "STK".into(),
            exchange: "SMART".into(), currency: "USD".into(),
            ..Default::default()
        };
        let _ = client.place(&unnamed, &Order::limit("BUY", 1.0, 1.0));
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if done.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("placing an unnamed contract never returned: it is waiting on its own turn");
}

/// An order that cannot be placed is refused before the venue is asked
/// anything, and a description resolved once is not asked about twice.
///
/// Placing looks a contract up before it takes its turn. Done ahead of the
/// refusals, a caller who wrote an impossible order waits out a lookup for a
/// contract that was never going to be traded, and hears about the lookup
/// rather than about their order.
#[test]
fn an_impossible_order_is_refused_before_the_venue_is_asked() {
    let (client, rx, _shared) = test_client();
    let unnamed = Contract {
        symbol: "SPY".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), currency: "USD".into(),
        ..Default::default()
    };
    let asked = std::time::Instant::now();
    let err = client
        .place(&unnamed, &Order { tif: "FOREVER".into(), ..Order::limit("BUY", 1.0, 1.0) })
        .expect_err("a time in force that is not one is refused");
    assert!(err.message.contains("tif"), "{err}");
    assert!(
        asked.elapsed() < std::time::Duration::from_secs(5),
        "the refusal waited on a lookup: {:?}",
        asked.elapsed(),
    );
    assert!(rx.try_recv().is_err(), "nothing reaches the wire");
}

/// A bracket whose exits sit the wrong side of its entry is refused before it
/// is sent.
///
/// Placed, it opens a position and closes it in the same breath: a take-profit
/// below the entry is already profitable, and a stop above it is already
/// triggered. The venue is not the right place to find that out.
#[test]
fn a_bracket_that_closes_itself_is_refused() {
    let (client, rx, _shared) = test_client();
    for (what, side, entry, take_profit, stop_loss) in [
        ("a buy taking profit below its entry", "BUY", 100.0, 90.0, 95.0),
        ("a buy stopping out above its entry", "BUY", 100.0, 110.0, 105.0),
        ("a sell taking profit above its entry", "SELL", 100.0, 110.0, 105.0),
    ] {
        let err = client
            .place_bracket(&spy(), side, 1.0, entry, take_profit, stop_loss)
            .expect_err(what);
        assert!(err.message.contains("wrong side"), "{what}: {err}");
        assert!(rx.try_recv().is_err(), "{what}: nothing reaches the wire");
    }

    // And one stated the right way round is sent, under three consecutive
    // numbers — the venue reads the children's as the parent's plus one and two.
    let ids = client
        .place_bracket(&spy(), "BUY", 1.0, 100.0, 110.0, 95.0)
        .expect("a bracket the right way round is placed");
    assert_eq!(ids[1], ids[0] + 1);
    assert_eq!(ids[2], ids[0] + 2);
    assert_eq!(client.next_order_id(), ids[0] + 3, "the next order does not reuse a child's");
}

/// Bars are asked for as trades, except where the instrument has none.
///
/// A currency pair does not trade on an exchange, so the venue holds no trade
/// history for one and answers a request for it with "No historical market
/// data" — which is what this call did against a live session until it stopped
/// asking for trades there.
#[test]
fn bars_ask_for_what_the_instrument_has() {
    let (client, rx, _shared) = test_client();
    for (sec_type, wanted) in [("STK", "TRADES"), ("CASH", "MIDPOINT"), ("CFD", "MIDPOINT")] {
        let contract = Contract {
            con_id: 12087792, symbol: "EUR".into(), sec_type: sec_type.into(),
            exchange: "IDEALPRO".into(), currency: "USD".into(), ..Default::default()
        };
        let _ = client.bars(&contract, "1 D", "1 hour");
        let asked = std::iter::from_fn(|| rx.try_recv().ok())
            .find_map(|c| match c {
                crate::types::ControlCommand::FetchHistorical { what_to_show, .. } => Some(what_to_show),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{sec_type}: a request reaches the engine"));
        assert_eq!(asked, wanted, "{sec_type} bars");
    }
}

/// Non-finite prices are refused before scaling. A saturating cast turns NaN
/// into 0 and infinity into the largest representable price, both of which the
/// venue accepts as real values.
#[test]
fn a_number_the_wire_cannot_carry_is_refused_wherever_it_sits() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    let base = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, ..Default::default()
    };

    let cases: Vec<(&str, Order)> = vec![
        ("scale_price_increment", Order { scale_price_increment: f64::NAN, ..base.clone() }),
        ("scale_profit_offset", Order { scale_profit_offset: f64::INFINITY, ..base.clone() }),
        ("stock_range_lower", Order { stock_range_lower: f64::NAN, ..base.clone() }),
        ("volatility", Order { volatility: f64::NAN, ..base.clone() }),
        ("percent_offset", Order { percent_offset: f64::NEG_INFINITY, ..base.clone() }),
        ("starting_price", Order { starting_price: f64::NAN, ..base.clone() }),
        ("delta_neutral_aux_price", Order {
            delta_neutral_order_type: "MKT".into(),
            delta_neutral_aux_price: f64::NAN, ..base.clone()
        }),
        ("order_combo_legs", Order {
            order_combo_legs: vec![f64::NAN], ..base.clone()
        }),
    ];
    for (named, order) in cases {
        let err = match client.place_order(1, &spy(), &order) {
            Err(e) => e,
            Ok(()) => panic!("{named} was accepted"),
        };
        assert!(err.message.contains(named), "{named}: got {err}");
    }
}

/// Two reports on one order in a single pass are two callbacks, in arrival
/// order. Each status change is stated separately on the wire.
#[test]
fn each_report_on_an_order_is_delivered() {
    #[derive(Default)]
    struct Statuses(Vec<String>);
    impl Wrapper for Statuses {
        fn order_status(
            &mut self, _order_id: i64, status: &str, _filled: f64, _remaining: f64,
            _avg: f64, _perm_id: i64, _parent_id: i64, _last: f64, _client_id: i64,
            _why_held: &str, _mkt_cap_price: f64,
        ) {
            self.0.push(status.to_string());
        }
    }

    let (client, _rx, shared) = test_client();
    for status in [OrderStatus::PreSubmitted, OrderStatus::Submitted, OrderStatus::Cancelled] {
        shared.orders.push_order_update(crate::types::OrderUpdate {
            order_id: 4, instrument: 0, status, filled_qty: 0.0, remaining_qty: 100.0,
            avg_price: 0, perm_id: 77, parent_id: 0, timestamp_ns: 0,
        });
    }
    let mut seen = Statuses::default();
    client.process_msgs(&mut seen);
    assert_eq!(
        seen.0, ["PreSubmitted", "Submitted", "Cancelled"],
        "a report was replaced by the one that followed it",
    );
}

/// A replace names the order, so it cannot name another contract.
///
/// The message carries the order id and its fields, not the instrument, so the
/// order stays on the contract it was placed on. A contract naming a different
/// instrument is refused rather than recorded.
#[test]
fn a_replace_does_not_move_an_order_to_another_contract() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(2);
    let order = Order {
        action: "BUY".into(), total_quantity: 100.0, order_type: "LMT".into(),
        lmt_price: 150.0, ..Default::default()
    };
    client.place_order(1, &spy(), &order).expect("the first placement");

    let elsewhere = Contract {
        con_id: 265598, symbol: "AAPL".into(), exchange: "SMART".into(),
        ..Default::default()
    };
    client.core.con_id_to_instrument.lock().unwrap().insert(elsewhere.con_id, 1);
    let err = client
        .place_order(1, &elsewhere, &Order { lmt_price: 151.0, ..order.clone() })
        .expect_err("an order working on one contract is not replaced onto another");
    assert!(err.message.contains("another contract"), "{err}");

    // And the record is the contract the venue is working, not the one refused.
    assert_eq!(
        client.core.open_orders.lock().unwrap()[&1].contract.symbol, "SPY",
        "the refused contract was recorded against the order",
    );

    // The same order on the same contract still replaces.
    client
        .place_order(1, &spy(), &Order { lmt_price: 151.0, ..order })
        .expect("a replace naming the contract it was placed on");
}

/// A bracket is held to the checks a single order is held to.
///
/// An order on a security type the account is not permitted is returned Inactive
/// with tag 58 empty, so the reason is stated here instead.
#[test]
fn a_bracket_is_refused_on_a_security_type_the_venue_does_not_permit() {
    let (client, _rx, shared) = test_client();
    shared.market.set_instrument_count(1);
    shared.reference.set_order_permissions(
        [("STK".to_string(), vec!["LMT".to_string()])].into_iter().collect(),
    );
    let bond = Contract {
        con_id: 15547841, symbol: "IBM".into(), sec_type: "BOND".into(),
        exchange: "SMART".into(), ..Default::default()
    };
    let err = client
        .place_bracket(&bond, "BUY", 1000.0, 100.0, 110.0, 90.0)
        .expect_err("a bracket on an unpermitted security type is refused before it is sent");
    assert!(err.message.to_uppercase().contains("BOND"), "{err}");

    // And a permitted one still goes.
    client
        .place_bracket(&spy(), "BUY", 1.0, 100.0, 110.0, 90.0)
        .expect("a bracket on a permitted security type");
}

/// Asking for the account's P&L sends the subscription.
///
/// The figures on `pnl` are computed against each holding's midnight value and
/// realised amount, which the venue states only in answer to this request.
#[test]
fn asking_for_the_accounts_pnl_asks_the_venue() {
    let (client, rx, _shared) = test_client();
    client.req_pnl(9, "", "");
    let asked = rx.try_iter().find_map(|cmd| match cmd {
        ControlCommand::SubscribePnl { req_id, account } => Some((req_id, account)),
        _ => None,
    });
    assert_eq!(
        asked,
        Some((9, "DU123".to_string())),
        "the venue was not asked for the account's P&L",
    );

    // And under the account named, where one is.
    client.req_pnl(10, "DU999", "");
    assert!(rx.try_iter().any(|cmd| matches!(
        cmd, ControlCommand::SubscribePnl { account, .. } if account == "DU999"
    )));
}

/// `regulatory_snapshot` is refused: the subscription carries no field for it,
/// and an ordinary subscription is a different, unchargeable request.
#[test]
fn a_regulatory_snapshot_is_refused_rather_than_answered_with_an_ordinary_one() {
    let (client, _rx, _shared) = test_client();
    let err = client
        .req_mkt_data(1, &spy(), "", false, true)
        .expect_err("a request this protocol does not carry is refused");
    assert!(err.message.contains("regulatory_snapshot"), "{err}");
    // An ordinary subscription on the same contract still goes.
    client.core.con_id_to_instrument.lock().unwrap().insert(spy().con_id, 0);
    client.core.instrument_to_req.lock().unwrap().insert(0, 1);
    client.core.req_to_instrument.lock().unwrap().insert(1, 0);
    client.req_mkt_data(2, &spy(), "", false, false).expect("an ordinary subscription");
}
