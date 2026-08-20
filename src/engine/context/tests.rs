//! The tests for this module.
//!
//! One file per module, as `api/client` already does it. Each block below
//! reaches the code it tests through `super::super`, which is the module this
//! file belongs to.

use super::*;
use crate::types::QTY_SCALE;

// --- Order submission & drain ---

#[test]
fn submit_limit_returns_incrementing_ids() {
    let mut ctx = Context::new();
    let id1 = ctx.submit(0, Side::Buy, 100, OrderKind::Limit { price: 150 * PRICE_SCALE }, b'0', OrderAttrs::default());
    let id2 = ctx.submit(0, Side::Sell, 50, OrderKind::Limit { price: 151 * PRICE_SCALE }, b'0', OrderAttrs::default());
    assert_eq!(id2, id1 + 1, "IDs should be sequential");
}

#[test]
fn submit_limit_drains_correctly() {
    let mut ctx = Context::new();
    ctx.submit(0, Side::Buy, 100, OrderKind::Limit { price: 150 * PRICE_SCALE }, b'0', OrderAttrs::default());

    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match orders[0] {
        OrderRequest::SubmitEx {
            instrument, side, qty,
            kind: OrderKind::Limit { price }, ..
        } => {
            assert_eq!(instrument, 0);
            assert_eq!(side, Side::Buy);
            assert_eq!(qty, 100 * crate::types::QTY_SCALE);
            assert_eq!(price, 150 * PRICE_SCALE);
        }
        _ => panic!("expected SubmitLimit"),
    }
}

#[test]
fn submit_market_drains_correctly() {
    let mut ctx = Context::new();
    ctx.submit(1, Side::Sell, 200, OrderKind::Market, b'0', OrderAttrs::default());

    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match orders[0] {
        OrderRequest::SubmitEx {
            instrument, side, qty,
            kind: OrderKind::Market, ..
        } => {
            assert_eq!(instrument, 1);
            assert_eq!(side, Side::Sell);
            assert_eq!(qty, 200 * crate::types::QTY_SCALE);
        }
        _ => panic!("expected SubmitMarket"),
    }
}

#[test]
fn cancel_drains_correctly() {
    let mut ctx = Context::new();
    ctx.cancel(42);

    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    match orders[0] {
        OrderRequest::Cancel { order_id } => assert_eq!(order_id, 42),
        _ => panic!("expected Cancel"),
    }
}

#[test]
fn cancel_all_drains_correctly() {
    let mut ctx = Context::new();
    ctx.cancel_all(5);

    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    match orders[0] {
        OrderRequest::CancelAll { instrument } => assert_eq!(instrument, 5),
        _ => panic!("expected CancelAll"),
    }
}

#[test]
fn modify_drains_correctly() {
    let mut ctx = Context::new();
    ctx.modify(7, 200 * PRICE_SCALE, 50, false);

    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    match orders[0] {
        OrderRequest::Modify {
            order_id,
            price,
            qty,
            ..
        } => {
            assert_eq!(order_id, 7);
            assert_eq!(price, 200 * PRICE_SCALE);
            assert_eq!(qty, 50 * crate::types::QTY_SCALE);
        }
        _ => panic!("expected Modify"),
    }
}

#[test]
fn drain_clears_buffer() {
    let mut ctx = Context::new();
    ctx.submit(0, Side::Buy, 100, OrderKind::Limit { price: 150 * PRICE_SCALE }, b'0', OrderAttrs::default());
    let _: Vec<_> = ctx.drain_pending_orders().collect();
    // Second drain should be empty
    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert!(orders.is_empty());
}

#[test]
fn multiple_orders_per_tick() {
    let mut ctx = Context::new();
    ctx.submit(0, Side::Buy, 100, OrderKind::Limit { price: 150 * PRICE_SCALE }, b'0', OrderAttrs::default());
    ctx.submit(0, Side::Sell, 50, OrderKind::Limit { price: 152 * PRICE_SCALE }, b'0', OrderAttrs::default());
    ctx.cancel(99);

    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 3);
}

// --- Position tracking ---

#[test]
fn position_starts_at_zero() {
    let ctx = Context::new();
    assert_eq!(ctx.position(0), 0.0);
    assert_eq!(ctx.position(255), 0.0);
}

#[test]
fn update_position_accumulates() {
    let mut ctx = Context::new();
    ctx.update_position(0, 100.0);
    assert_eq!(ctx.position(0), 100.0);
    ctx.update_position(0, -30.0);
    assert_eq!(ctx.position(0), 70.0);
    ctx.update_position(0, -70.0);
    assert_eq!(ctx.position(0), 0.0);
}

#[test]
fn positions_per_instrument() {
    let mut ctx = Context::new();
    ctx.update_position(0, 100.0);
    ctx.update_position(1, -50.0);
    assert_eq!(ctx.position(0), 100.0);
    assert_eq!(ctx.position(1), -50.0);
}

// --- Open orders ---

#[test]
fn insert_and_query_order() {
    let mut ctx = Context::new();
    let order = Order {
        order_id: 1,
        instrument: 0,
        side: Side::Buy,
        price: 150 * PRICE_SCALE,
        qty: 100 * QTY_SCALE,
        filled: 0,
        status: OrderStatus::Submitted,
        ord_type: b'2',
        tif: b'0',
        stop_price: 0,
    };
    ctx.insert_order(order);
    assert!(ctx.order(1).is_some());
    assert_eq!(ctx.order(1).unwrap().qty, 100 * crate::types::QTY_SCALE);
}

#[test]
fn open_orders_for_instrument() {
    let mut ctx = Context::new();
    ctx.insert_order(Order {
        order_id: 1,
        instrument: 0,
        side: Side::Buy,
        price: 150 * PRICE_SCALE,
        qty: 100,
        filled: 0,
        status: OrderStatus::Submitted,
        ord_type: b'2',
        tif: b'0',
        stop_price: 0,
    });
    ctx.insert_order(Order {
        order_id: 2,
        instrument: 1,
        side: Side::Sell,
        price: 400 * PRICE_SCALE,
        qty: 50,
        filled: 0,
        status: OrderStatus::Submitted,
        ord_type: b'2',
        tif: b'0',
        stop_price: 0,
    });

    let inst0_orders = ctx.open_orders_for(0);
    assert_eq!(inst0_orders.len(), 1);
    assert_eq!(inst0_orders[0].order_id, 1);
}

#[test]
fn update_order_status() {
    let mut ctx = Context::new();
    ctx.insert_order(Order {
        order_id: 1,
        instrument: 0,
        side: Side::Buy,
        price: 150 * PRICE_SCALE,
        qty: 100,
        filled: 0,
        status: OrderStatus::Submitted,
        ord_type: b'2',
        tif: b'0',
        stop_price: 0,
    });
    ctx.update_order_status(1, OrderStatus::Cancelled, false);
    assert_eq!(ctx.order(1).unwrap().status, OrderStatus::Cancelled);

    // Cancelled orders not in open_orders_for (filters by Submitted)
    assert!(ctx.open_orders_for(0).is_empty());
}

// ── monotonic status guard ──

fn submitted_order(ctx: &mut Context, oid: u64) {
    ctx.insert_order(Order {
        order_id: oid, instrument: 0, side: Side::Buy, price: 100,
        qty: 100, filled: 0, status: OrderStatus::Submitted,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });
}

#[test]
fn stale_presubmitted_does_not_regress_submitted() {
    let mut ctx = Context::new();
    submitted_order(&mut ctx, 1);
    assert!(!ctx.update_order_status(1, OrderStatus::PreSubmitted, false),
        "regression must be rejected");
    assert_eq!(ctx.order(1).unwrap().status, OrderStatus::Submitted);
}

#[test]
fn terminal_states_are_absorbing() {
    let mut ctx = Context::new();
    submitted_order(&mut ctx, 1);
    assert!(ctx.update_order_status(1, OrderStatus::Filled, false));
    // A late mass-status snapshot must not resurrect the order.
    for stale in [OrderStatus::Submitted, OrderStatus::Cancelled, OrderStatus::PendingCancel] {
        assert!(!ctx.update_order_status(1, stale, false), "{stale:?} must not overwrite Filled");
    }
    assert_eq!(ctx.order(1).unwrap().status, OrderStatus::Filled);
}

#[test]
fn cancel_and_fill_progressions_still_flow() {
    let mut ctx = Context::new();
    submitted_order(&mut ctx, 1);
    // Cancel of a partially filled order, and a fill landing while the
    // cancel is pending, are both legitimate.
    assert!(ctx.update_order_status(1, OrderStatus::PartiallyFilled, false));
    assert!(ctx.update_order_status(1, OrderStatus::PendingCancel, false));
    assert!(ctx.update_order_status(1, OrderStatus::Filled, false));
}

#[test]
fn modify_ack_returns_to_submitted() {
    let mut ctx = Context::new();
    submitted_order(&mut ctx, 1);
    assert!(ctx.update_order_status(1, OrderStatus::PendingReplace, false));
    assert!(ctx.update_order_status(1, OrderStatus::Submitted, false),
        "modify ack returns the order to working");
}

/// A cancel in flight is not undone by the venue restating the past.
///
/// A report stating the order is working moves it out of PendingCancel. A
/// replayed report does not: recent activity is replayed when a session opens,
/// and a replayed working status predates any cancel sent since.
#[test]
fn a_replay_does_not_undo_a_cancel_in_flight() {
    let mut ctx = Context::new();
    submitted_order(&mut ctx, 1);
    assert!(ctx.update_order_status(1, OrderStatus::PendingCancel, false));

    for replayed in [OrderStatus::PreSubmitted, OrderStatus::Submitted] {
        assert!(
            !ctx.update_order_status(1, replayed, true),
            "{replayed:?} restating history must leave the cancel standing",
        );
        assert_eq!(ctx.order(1).unwrap().status, OrderStatus::PendingCancel);
    }

    // The report that announces something new still gets the order back.
    assert!(ctx.update_order_status(1, OrderStatus::Submitted, false));
}

#[test]
fn forced_setter_bypasses_guard() {
    let mut ctx = Context::new();
    submitted_order(&mut ctx, 1);
    assert!(ctx.update_order_status(1, OrderStatus::PendingCancel, false));
    // PendingCancel does not supersede the working states, so a report
    // stating the order is working moves it out on the ordinary path.
    assert!(ctx.update_order_status(1, OrderStatus::Submitted, false));

    // The guard blocks leaving a terminal state. The forced setter exists for
    // a cancel this session reported and the venue then refused.
    assert!(ctx.update_order_status(1, OrderStatus::Cancelled, false));
    assert!(!ctx.update_order_status(1, OrderStatus::Submitted, false));
    ctx.set_order_status_forced(1, OrderStatus::Submitted);
    assert_eq!(ctx.order(1).unwrap().status, OrderStatus::Submitted);
}

#[test]
fn unchanged_status_reports_no_change() {
    let mut ctx = Context::new();
    submitted_order(&mut ctx, 1);
    assert!(!ctx.update_order_status(1, OrderStatus::Submitted, false));
    assert!(!ctx.update_order_status(999, OrderStatus::Cancelled, false), "unknown order");
    }

#[test]
fn remove_order() {
    let mut ctx = Context::new();
    ctx.insert_order(Order {
        order_id: 1,
        instrument: 0,
        side: Side::Buy,
        price: 150 * PRICE_SCALE,
        qty: 100,
        filled: 0,
        status: OrderStatus::Submitted,
        ord_type: b'2',
        tif: b'0',
        stop_price: 0,
    });
    ctx.last_clord.insert(1, "1.0".to_string());
    ctx.remove_order(1);
    assert!(ctx.order(1).is_none());
    // The chain survives an order that merely stopped being tracked: a
    // replace that failed to send leaves the previous version working, and
    // cancelling it means stating the ClOrdID the broker last recorded.
    assert!(ctx.last_clord.contains_key(&1), "the ClOrdID outlives the tracking");

    // Retiring it is what drops everything keyed to it. Nothing pruned
    // these, so a process left running for weeks held one entry per order
    // it had ever placed, in both maps.
    ctx.retire_order(1);
    assert!(!ctx.modify_versions.contains_key(&1), "the version counter goes with it");
    assert!(!ctx.last_clord.contains_key(&1), "and so does the ClOrdID");
}

// --- Market data through context ---

#[test]
fn context_market_data_accessors() {
    let mut ctx = Context::new();
    let id = ctx.market.register(265598);
    let q = ctx.market.quote_mut(id);
    q.bid = 15000 * (PRICE_SCALE / 100);
    q.ask = 15010 * (PRICE_SCALE / 100);

    assert_eq!(ctx.bid(id), 15000 * (PRICE_SCALE / 100));
    assert_eq!(ctx.ask(id), 15010 * (PRICE_SCALE / 100));
    assert_eq!(ctx.spread(id), 10 * (PRICE_SCALE / 100));
    assert_eq!(ctx.mid(id), 15005 * (PRICE_SCALE / 100));
}

// --- Clock ---

#[test]
fn clock_monotonic() {
    let ctx = Context::new();
    let t1 = ctx.now_ns();
    let t2 = ctx.now_ns();
    assert!(t2 >= t1);
}

#[test]
fn clock_utc_reasonable() {
    let ctx = Context::new();
    let ts = ctx.now_utc();
    // Should be after 2025-01-01 (1735689600)
    assert!(ts > 1_735_689_600);
}

// --- submit_limit uses current bid ---

#[test]
fn submit_limit_uses_current_bid() {
    let mut ctx = Context::new();
    ctx.market.register(265598);
    ctx.market.quote_mut(0).bid = 150 * PRICE_SCALE;

    ctx.submit(0, Side::Buy, 100, OrderKind::Limit { price: ctx.bid(0) }, b'0', OrderAttrs::default());

    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match orders[0] {
        OrderRequest::SubmitEx { kind: OrderKind::Limit { price }, .. } => {
            assert_eq!(price, 150 * PRICE_SCALE);
        }
        _ => panic!("expected SubmitLimit"),
    }
}

// --- register_instrument ---

#[test]
fn register_instrument_returns_id() {
    let mut ctx = Context::new();
    let id = ctx.register_instrument(265598);
    assert_eq!(id, 0);
    let id2 = ctx.register_instrument(272093);
    assert_eq!(id2, 1);
}

#[test]
fn register_instrument_idempotent() {
    let mut ctx = Context::new();
    let id1 = ctx.register_instrument(265598);
    let id2 = ctx.register_instrument(265598);
    assert_eq!(id1, id2);
}

// --- set_quote ---

#[test]
fn set_quote_replaces_entire_quote() {
    let mut ctx = Context::new();
    let id = ctx.register_instrument(265598);
    let q = Quote {
        bid: 150 * PRICE_SCALE,
        ask: 151 * PRICE_SCALE,
        last: 15050 * (PRICE_SCALE / 100),
        bid_size: 500,
        ask_size: 300,
        ..Quote::default()
    };
    ctx.set_quote(id, q);
    assert_eq!(ctx.bid(id), 150 * PRICE_SCALE);
    assert_eq!(ctx.ask(id), 151 * PRICE_SCALE);
    assert_eq!(ctx.bid_size(id), 500);
    assert_eq!(ctx.ask_size(id), 300);
}

// --- quote_mut ---

#[test]
fn quote_mut_modifies_in_place() {
    let mut ctx = Context::new();
    let id = ctx.register_instrument(265598);
    ctx.quote_mut(id).bid = 42 * PRICE_SCALE;
    assert_eq!(ctx.bid(id), 42 * PRICE_SCALE);
}

// --- bid_size, ask_size ---

#[test]
fn bid_size_ask_size_delegates() {
    let mut ctx = Context::new();
    let id = ctx.register_instrument(265598);
    ctx.quote_mut(id).bid_size = 123;
    ctx.quote_mut(id).ask_size = 456;
    assert_eq!(ctx.bid_size(id), 123);
    assert_eq!(ctx.ask_size(id), 456);
}

// --- account ---

#[test]
fn account_default_zeros() {
    let ctx = Context::new();
    let a = ctx.account();
    assert_eq!(a.net_liquidation, 0);
    assert_eq!(a.buying_power, 0);
}

#[test]
fn account_writable() {
    let mut ctx = Context::new();
    ctx.account.net_liquidation = 100_000 * PRICE_SCALE;
    assert_eq!(ctx.account().net_liquidation, 100_000 * PRICE_SCALE);
}

// --- Timing ---

#[test]
fn now_ns_monotonic() {
    let ctx = Context::new();
    let t1 = ctx.now_ns();
    let t2 = ctx.now_ns();
    assert!(t2 >= t1);
}

#[test]
fn now_utc_positive() {
    let ctx = Context::new();
    let ts = ctx.now_utc();
    // Should be after 2024-01-01 in seconds since epoch
    assert!(ts > 1704067200);
}

// --- Multiple orders per instrument ---

#[test]
fn multiple_orders_same_instrument() {
    let mut ctx = Context::new();
    ctx.register_instrument(265598);

    ctx.insert_order(Order {
        order_id: 1, instrument: 0, side: Side::Buy,
        price: 150 * PRICE_SCALE, qty: 100, filled: 0,
        status: OrderStatus::Submitted,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });
    ctx.insert_order(Order {
        order_id: 2, instrument: 0, side: Side::Sell,
        price: 155 * PRICE_SCALE, qty: 50, filled: 0,
        status: OrderStatus::Submitted,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });
    ctx.insert_order(Order {
        order_id: 3, instrument: 0, side: Side::Buy,
        price: 149 * PRICE_SCALE, qty: 200, filled: 0,
        status: OrderStatus::Filled,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });

    // open_orders_for only returns Submitted
    let open = ctx.open_orders_for(0);
    assert_eq!(open.len(), 2);
}

// --- Update order status edge case ---

#[test]
fn update_order_status_nonexistent_no_panic() {
    let mut ctx = Context::new();
    // Should not panic when order doesn't exist
    ctx.update_order_status(999, OrderStatus::Cancelled, false);
}

#[test]
fn remove_order_nonexistent_no_panic() {
    let mut ctx = Context::new();
    ctx.remove_order(999); // should not panic
}

#[test]
fn submit_stop_returns_id_and_drains() {
    let mut ctx = Context::new();
    let id = ctx.submit(0, Side::Sell, 50, OrderKind::Stop { stop_price: 140 * PRICE_SCALE }, b'0', OrderAttrs::default());

    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match orders[0] {
        OrderRequest::SubmitEx {
            order_id, instrument, side, qty,
            kind: OrderKind::Stop { stop_price }, ..
        } => {
            assert_eq!(order_id, id);
            assert_eq!(instrument, 0);
            assert_eq!(side, Side::Sell);
            assert_eq!(qty, 50 * crate::types::QTY_SCALE);
            assert_eq!(stop_price, 140 * PRICE_SCALE);
        }
        _ => panic!("Expected SubmitStop"),
    }
}

#[test]
fn adjust_order_filled_accumulates() {
    let mut ctx = Context::new();
    ctx.insert_order(Order {
        order_id: 1, instrument: 0, side: Side::Buy,
        price: PRICE_SCALE, qty: 100 * QTY_SCALE, filled: 0,
        status: OrderStatus::PendingSubmit,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });
    ctx.adjust_order_filled(1, 30 * QTY_SCALE);
    assert_eq!(ctx.order(1).unwrap().filled, 30 * QTY_SCALE);
    ctx.adjust_order_filled(1, 50 * QTY_SCALE);
    assert_eq!(ctx.order(1).unwrap().filled, 80 * QTY_SCALE);
}

/// A gateway figure large enough to overflow the counter must not wrap the
/// order's filled quantity round to nothing.
#[test]
fn adjust_order_filled_saturates() {
    let mut ctx = Context::new();
    ctx.insert_order(Order {
        order_id: 1, instrument: 0, side: Side::Buy,
        price: PRICE_SCALE, qty: crate::types::Qty::MAX, filled: crate::types::Qty::MAX - 1,
        status: OrderStatus::PartiallyFilled,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });
    ctx.adjust_order_filled(1, 10 * QTY_SCALE);
    assert_eq!(ctx.order(1).unwrap().filled, crate::types::Qty::MAX);
}

#[test]
fn open_orders_for_includes_pending_and_partial() {
    let mut ctx = Context::new();
    ctx.insert_order(Order {
        order_id: 1, instrument: 0, side: Side::Buy,
        price: PRICE_SCALE, qty: 100, filled: 0,
        status: OrderStatus::PendingSubmit,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });
    ctx.insert_order(Order {
        order_id: 2, instrument: 0, side: Side::Buy,
        price: PRICE_SCALE, qty: 100, filled: 50,
        status: OrderStatus::PartiallyFilled,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });
    ctx.insert_order(Order {
        order_id: 3, instrument: 0, side: Side::Buy,
        price: PRICE_SCALE, qty: 100, filled: 100,
        status: OrderStatus::Filled,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });
    let open = ctx.open_orders_for(0);
    // PendingSubmit and PartiallyFilled count as open; Filled does not
    assert_eq!(open.len(), 2);
}


#[test]
fn submit_limit_auc_drains_correctly() {
    let mut ctx = Context::new();
    let id = ctx.submit(0, Side::Buy, 100, OrderKind::Limit { price: 150 * PRICE_SCALE }, b'8',
        OrderAttrs { outside_rth: false, ..Default::default() });
    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match &orders[0] {
        OrderRequest::SubmitEx { order_id, instrument, side, qty, kind: OrderKind::Limit { price }, tif: b'8', .. } => {
            assert_eq!(*order_id, id);
            assert_eq!(*instrument, 0);
            assert_eq!(*side, Side::Buy);
            assert_eq!(*qty, 100 * crate::types::QTY_SCALE);
            assert_eq!(*price, 150 * PRICE_SCALE);
        }
        _ => panic!("expected SubmitLimitAuc"),
    }
}

#[test]
fn submit_mtl_auc_drains_correctly() {
    let mut ctx = Context::new();
    let id = ctx.submit(0, Side::Buy, 100, OrderKind::Mtl, b'8',
        OrderAttrs { outside_rth: false, ..Default::default() });
    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match &orders[0] {
        OrderRequest::SubmitEx { order_id, instrument, side, qty, kind: OrderKind::Mtl, tif: b'8', .. } => {
            assert_eq!(*order_id, id);
            assert_eq!(*instrument, 0);
            assert_eq!(*side, Side::Buy);
            assert_eq!(*qty, 100 * crate::types::QTY_SCALE);
        }
        _ => panic!("expected SubmitMtlAuc"),
    }
}

#[test]
fn submit_box_top_reuses_mtl() {
    let mut ctx = Context::new();
    let id = ctx.submit(0, Side::Buy, 100, OrderKind::Mtl, b'0', OrderAttrs::default());
    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match &orders[0] {
        OrderRequest::SubmitEx {
            order_id, instrument, side, qty,
            kind: OrderKind::Mtl, ..
        } => {
            assert_eq!(*order_id, id);
            assert_eq!(*instrument, 0);
            assert_eq!(*side, Side::Buy);
            assert_eq!(*qty, 100 * crate::types::QTY_SCALE);
        }
        _ => panic!("expected SubmitMtl from box_top"),
    }
}

#[test]
fn submit_what_if_drains_correctly() {
    let mut ctx = Context::new();
    let id = ctx.submit(0, Side::Buy, 100, OrderKind::WhatIf { price: 25620 * (PRICE_SCALE / 100), aux: 0, ord_type: b'2' },
        b'0', OrderAttrs::default());
    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match &orders[0] {
        OrderRequest::SubmitEx {
            order_id, instrument, side, qty, kind: OrderKind::WhatIf { price, .. }, ..
        } => {
            assert_eq!(*order_id, id);
            assert_eq!(*instrument, 0);
            assert_eq!(*side, Side::Buy);
            assert_eq!(*qty, 100 * crate::types::QTY_SCALE);
            assert_eq!(*price, 25620 * (PRICE_SCALE / 100));
        }
        _ => panic!("expected a what-if"),
    }
}

#[test]
fn submit_limit_fractional_drains_correctly() {
    let mut ctx = Context::new();
    let id = ctx.submit_limit_fractional(0, Side::Buy, QTY_SCALE / 2, 150 * PRICE_SCALE);
    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match &orders[0] {
        OrderRequest::SubmitLimitFractional { order_id, instrument, side, qty, price } => {
            assert_eq!(*order_id, id);
            assert_eq!(*instrument, 0);
            assert_eq!(*side, Side::Buy);
            assert_eq!(*qty as f64 / QTY_SCALE as f64, 0.5, "half a share");
            assert_eq!(*price, 150 * PRICE_SCALE);
        }
        _ => panic!("expected SubmitLimitFractional"),
    }
}

#[test]
fn submit_adjustable_stop_drains_correctly() {
    let mut ctx = Context::new();
    let id = ctx.submit(
        0, Side::Sell, 1,
        OrderKind::AdjustableStop {
            stop_price: 25120 * (PRICE_SCALE / 100),
            trigger_price: 25620 * (PRICE_SCALE / 100),
            adjusted_order_type: AdjustedOrderType::StopLimit,
            adjusted_stop_price: 25320 * (PRICE_SCALE / 100),
            adjusted_stop_limit_price: 25220 * (PRICE_SCALE / 100),
            // A stop limit adjusts to a price, not by an amount.
            adjusted_trailing_amount: 0,
            adjustable_trailing_unit: 0,
        },
        b'1',   // GTC
        OrderAttrs { parent_id: 9, ..Default::default() },
    );
    let orders: Vec<_> = ctx.drain_pending_orders().collect();
    assert_eq!(orders.len(), 1);
    match &orders[0] {
        OrderRequest::SubmitEx { order_id, side, qty, kind: OrderKind::AdjustableStop {
            stop_price, trigger_price, adjusted_order_type, adjusted_stop_price,
            adjusted_stop_limit_price, .. }, tif, attrs, .. } => {
            assert_eq!(*order_id, id);
            assert_eq!(*side, Side::Sell);
            assert_eq!(*qty, crate::types::QTY_SCALE);
            assert_eq!(*stop_price, 25120 * (PRICE_SCALE / 100));
            assert_eq!(*trigger_price, 25620 * (PRICE_SCALE / 100));
            assert_eq!(*adjusted_order_type, AdjustedOrderType::StopLimit);
            assert_eq!(*adjusted_stop_price, 25320 * (PRICE_SCALE / 100));
            assert_eq!(*adjusted_stop_limit_price, 25220 * (PRICE_SCALE / 100));
            assert_eq!(*tif, b'1');
            assert_eq!(attrs.parent_id, 9);
        }
        _ => panic!("expected SubmitEx carrying AdjustableStop"),
    }
}

/// A disconnect makes every held order's state unknown, `Inactive` included:
/// the venue holds such an order and it can return to working.
#[test]
fn an_inactive_order_is_one_the_reconnect_has_to_account_for() {
    let mut ctx = Context::new();
    let instrument = ctx.register_instrument(756733);
    let held = crate::types::Order::new(
        7, instrument, Side::Buy, crate::types::QTY_SCALE, 100 * PRICE_SCALE, b'2', b'0', 0,
    );
    ctx.insert_order(held);
    ctx.set_order_status_forced(7, OrderStatus::Inactive);
    assert_eq!(
        ctx.open_orders_for(instrument).len(), 1,
        "a cancel-all already reaches it, so it is an order the venue holds",
    );

    ctx.mark_orders_uncertain();
    assert_eq!(
        ctx.order(7).map(|o| o.status), Some(OrderStatus::Uncertain),
        "so the reconnect has to say what became of it",
    );
    assert_eq!(ctx.uncertain_orders().len(), 1, "and it is reported as one of them");
}
