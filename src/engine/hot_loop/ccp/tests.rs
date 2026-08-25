//! The tests for this module.
//!
//! One file per module, as `api/client` already does it. Each block below
//! reaches the code it tests through `super::super`, which is the module this
//! file belongs to.

use super::*;
use crate::types::model as api;
use crate::bridge::RichOrderInfo;
use crate::types::{PositionInfo, Price, Side, PRICE_SCALE, QTY_SCALE};

/// `Uncertain` promises the caller a reconciliation when the reconnect
/// completes. Nothing completed it, so an order the recovery push left out
/// waited on a message that was never coming.
#[test]
fn the_recovery_reports_the_orders_it_did_not_account_for() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    let instrument = context.register_instrument(756733);
    context.insert_order(crate::types::Order::new(
        7, instrument, crate::types::Side::Buy, 100 * crate::types::QTY_SCALE,
        150 * crate::types::PRICE_SCALE, b'2', b'0', 0,
    ));
    context.mark_orders_uncertain();

    // Still inside the grace: the push may yet speak for it.
    ccp.recovery_sweep_at = Some(Instant::now() + Duration::from_secs(30));
    ccp.sweep_recovery(&mut context, &shared, &None);
    assert!(shared.orders.drain_order_updates().is_empty(), "nothing is due yet");

    ccp.recovery_sweep_at = Some(Instant::now() - Duration::from_secs(1));
    ccp.sweep_recovery(&mut context, &shared, &None);
    let updates = shared.orders.drain_order_updates();
    assert_eq!(updates.len(), 1, "the stranded order is reported");
    assert_eq!(updates[0].order_id, 7);
    assert_eq!(
        updates[0].status, crate::types::OrderStatus::Uncertain,
        "and reported as what it is — unknown, not a fate the engine invented",
    );

    ccp.sweep_recovery(&mut context, &shared, &None);
    assert!(shared.orders.drain_order_updates().is_empty(), "one report per recovery");
}

fn position_frame(pairs: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
    let mut m = std::collections::HashMap::new();
    m.insert(6008u32, "265598".to_string());
    for (t, v) in pairs { m.insert(*t, v.to_string()); }
    m
}

/// A frame carrying marks but no quantity leaves the position alone.
/// Reading absent as zero reconciles a live position to flat and publishes
/// it to reqPositions and both P&L paths.
///
/// The average cost is written into a row that persists, so a frame that
/// omits the tag must not replace a real one with zero either — the same
/// rule the quantity follows, on the price side.
#[test]
fn a_frame_without_an_average_cost_keeps_the_stored_one() {
    let mut context = Context::new();
    let shared = SharedState::new();
    let frame = |pairs: &[(u32, &str)]| {
        let mut m = std::collections::HashMap::new();
        for (t, v) in pairs { m.insert(*t, v.to_string()); }
        m
    };

    // A frame stating both.
    positions::handle_position_update(
        &frame(&[(6008, "756733"), (6064, "100"), (6101, "150.00"), (6068, "SPY")]),
        &mut context, &shared, &None,
    );
    let stored = shared.portfolio.position_info(756733).expect("row").avg_cost;
    assert_eq!(stored, 150 * PRICE_SCALE);

    // A later frame stating the quantity but not the cost.
    positions::handle_position_update(
        &frame(&[(6008, "756733"), (6064, "120"), (6068, "SPY")]),
        &mut context, &shared, &None,
    );
    let after = shared.portfolio.position_info(756733).expect("row");
    assert_eq!(after.position, 120.0, "the quantity it did state is applied");
    assert_eq!(
        after.avg_cost, 150 * PRICE_SCALE,
        "and the cost it did not state is kept, not zeroed",
    );
}

#[test]
fn marks_only_frame_does_not_flatten_a_live_position() {
    let mut context = Context::new();
    let instrument = context.register_instrument(265598);
    let shared = SharedState::new();

    positions::handle_position_update(&position_frame(&[(6064, "100"), (6101, "150.0")]),
        &mut context, &shared, &None);
    assert_eq!(context.position(instrument), 100.0);

    // Marks move, no 6064 on the frame.
    positions::handle_position_update(&position_frame(&[(6065, "151.0"), (6100, "100.0")]),
        &mut context, &shared, &None);
    assert_eq!(context.position(instrument), 100.0,
        "a marks-only frame must not flatten the position");
    assert_eq!(shared.portfolio.position_infos().iter()
        .find(|p| p.con_id == 265598).map(|p| p.position), Some(100.0),
        "reqPositions must still report the held quantity");

    // The marks from that frame did land on the existing row.
    let row = shared.portfolio.position_infos().into_iter()
        .find(|p| p.con_id == 265598).expect("row still present");
    assert_eq!(row.market_price, (151.0 * PRICE_SCALE as f64) as Price,
        "a marks-only frame must still update the marks");

    // A frame that really does carry a flat quantity still flattens it.
    positions::handle_position_update(&position_frame(&[(6064, "0")]), &mut context, &shared, &None);
    assert_eq!(context.position(instrument), 0.0);
}

/// A marks-only frame for a contract never seen before must not conjure a
/// row: set_position_marks inserts a default PositionInfo, and that row
/// would report position 0 to reqPositions and both P&L paths.
/// Same class as the absent tag: `"NaN".parse::<f64>()` succeeds and
/// `NaN as i64` is 0, so a non-finite value reached the flatten path by
/// exactly the route closed.
#[test]
fn a_non_finite_quantity_is_treated_as_no_quantity() {
    for bad in ["NaN", "inf", "-inf"] {
        let mut context = Context::new();
        let shared = SharedState::new();
        positions::handle_position_update(
            &position_frame(&[(6064, "100"), (6101, "150.0")]), &mut context, &shared, &None);
        assert_eq!(
            shared.portfolio.position_info(265598).map(|p| p.position), Some(100.0),
            "seed must establish a live position");

        positions::handle_position_update(
            &position_frame(&[(6064, bad), (6101, "151.0")]), &mut context, &shared, &None);
        assert_eq!(
            shared.portfolio.position_info(265598).map(|p| p.position), Some(100.0),
            "{bad} must not flatten a live position");
    }
}

#[test]
fn marks_only_frame_for_an_unknown_contract_creates_no_row() {
    let mut context = Context::new();
    let shared = SharedState::new();
    positions::handle_position_update(&position_frame(&[(6065, "151.0"), (6100, "100.0")]),
        &mut context, &shared, &None);
    assert!(shared.portfolio.position_infos().iter().all(|p| p.con_id != 265598),
        "no position row may be fabricated from a marks-only frame");
}

// The fill-dedup set is not wiped wholesale when it reaches its cap: a
// recently-seen ExecID stays deduplicated, so a post-reconnect replay
// cannot double-count the fill.
#[test]
fn record_exec_id_dedupes_within_window() {
    let mut ccp = CcpState::new();
    assert!(ccp.record_exec_id("exec-A"), "first sighting is new");
    assert!(!ccp.record_exec_id("exec-A"), "immediate replay is a duplicate");
}

#[test]
fn record_exec_id_evicts_oldest_not_whole_set() {
    let mut ccp = CcpState::new();
    // The very first ExecID — the one a reconnect is most likely to replay.
    assert!(ccp.record_exec_id("exec-first"));
    // Push the window exactly to its cap. Together with "exec-first" this is
    // EXEC_ID_WINDOW + 1 inserts, which evicts exactly one entry: the oldest
    // ("exec-first"). Every other recent ID must remain deduplicated.
    for i in 0..EXEC_ID_WINDOW {
        assert!(ccp.record_exec_id(&format!("exec-{i}")));
    }
    assert_eq!(ccp.seen_exec_ids.len(), EXEC_ID_WINDOW);
    // Oldest was evicted, so a replay now reads as new (unavoidable past the
    // window) — but the most recent IDs are still caught as duplicates.
    assert!(!ccp.record_exec_id("exec-0"), "recent ID still deduped");
    assert!(!ccp.record_exec_id(&format!("exec-{}", EXEC_ID_WINDOW - 1)),
        "newest ID still deduped");
}

// A wholesale clear() would have made "exec-first" re-insertable as new
// after just one extra fill past the cap; assert the rolling window keeps
// the bound without that cliff.
#[test]
fn record_exec_id_window_is_bounded() {
    let mut ccp = CcpState::new();
    for i in 0..(EXEC_ID_WINDOW * 3) {
        ccp.record_exec_id(&format!("exec-{i}"));
    }
    assert_eq!(ccp.seen_exec_ids.len(), EXEC_ID_WINDOW);
    assert_eq!(ccp.exec_id_order.len(), EXEC_ID_WINDOW);
}

// Build a what-if (6091=1) ExecReport map for order 42. `margin_fields`
// holds (tag, literal wire value) pairs exactly as the gateway puts them
// on the wire.
fn what_if_frame(margin_fields: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
    let mut m = std::collections::HashMap::new();
    m.insert(11u32, "42".to_string()); // ClOrdID
    m.insert(6091u32, "1".to_string()); // what-if marker
    for (tag, val) in margin_fields {
        m.insert(*tag, val.to_string());
    }
    m
}

// The full six margin fields of the captured true-zero close preview
// ( scenario 2b).
const ZERO_CLOSE_FIELDS: [(u32, &str); 6] = [
    (6826, "976.07"), (6827, "887.34"), (6828, "945924.53"),
    (6092, "0"), (6093, "0"), (6094, "945923.47"),
];

fn what_if_test_state() -> (CcpState, Context, SharedState) {
    let mut context = Context::new();
    let instrument = context.register_instrument(756733);
    context.insert_order(crate::types::Order {
        order_id: 42,
        instrument,
        side: Side::Buy,
        price: 0,
        qty: 100 * QTY_SCALE,
        filled: 0,
        status: crate::types::OrderStatus::Submitted,
        ord_type: b'2',
        tif: b'0',
        stop_price: 0,
    });
    (CcpState::new(), context, SharedState::new())
}

/// A replace is acknowledged as 39=5, and the gateway sends 39=6 first.
/// Captured live against a paper account, a modify runs PendingCancel then
/// Replaced. The monotonic guard ranks PendingCancel above the working
/// states, so the acknowledgement looked like a stale frame: the caller was
/// told the order was cancelling and never told the replacement was live.
#[test]
fn a_replace_acknowledgement_is_not_dropped_as_a_stale_frame() {
    let (mut ccp, mut context, shared) = ord_status_test_state();

    // The order is working, then the replace puts a cancel in flight.
    ccp.handle_exec_report(&exec_report_frame(&[(150, "0"), (39, "0")]), b"",
        &mut context, &shared, &None, "");
    ccp.handle_exec_report(&exec_report_frame(&[(150, "6"), (39, "6")]), b"",
        &mut context, &shared, &None, "");
    assert_eq!(context.order(42).map(|o| o.status),
        Some(crate::types::OrderStatus::PendingCancel), "the cancel is in flight");
    let _ = shared.orders.drain_open_orders();

    ccp.handle_exec_report(&exec_report_frame(&[(150, "5"), (39, "5")]), b"",
        &mut context, &shared, &None, "");

    assert_eq!(
        context.order(42).map(|o| o.status),
        Some(crate::types::OrderStatus::Submitted),
        "the replacement is working, not still cancelling",
    );
    assert!(
        shared.orders.drain_open_orders().iter().any(|(id, _)| *id == 42),
        "and the caller is told, rather than the frame being dropped",
    );
}

/// The recovery-push terminator carries `11='*'`, which parses to the
/// reserved order id 0. It is dropped further down the handler, but the
/// recovery insert runs first — so without a guard there it registers the
/// frame's conId and inserts order 0 before the "discard".
#[test]
fn the_recovery_terminator_mutates_no_state_before_it_is_dropped() {
    // A clean context: the shared fixture pre-registers its conId, which
    // would mask exactly what this test is looking for.
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    let frame: std::collections::HashMap<u32, String> = [
        (11u32, "*"), (150, "0"), (39, "0"), (6008, "265598"), (38, "1"), (54, "1"),
    ].iter().map(|(k, v)| (*k, v.to_string())).collect();

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    assert!(context.order(0).is_none(), "the reserved order id must not be inserted");
    assert!(
        context.market.instrument_by_con_id(265598).is_none(),
        "the terminator must not register an instrument",
    );
}
/// LeavesQty is still the remainder everywhere it was already right. The
/// two are complements, so a change that confuses them shows up here as
/// well as on the filled side.
#[test]
fn leaves_qty_is_still_reported_as_the_remainder() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (150, "2"), (39, "1"), (32, "30"), (31, "150.00"),
        (14, "30"), (151, "70"), (6, "150.00"), (17, "E1"),
    ]);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let fills = shared.orders.drain_fills();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].remaining, 70 * QTY_SCALE, "the fill reports what is still working");
}

/// `filled_quantity` was taken from tag 151 (LeavesQty), the *unfilled*
/// remainder, rather than tag 14 (CumQty). The two are complements, so a
/// partially filled order reported the wrong number and a completed one —
/// LeavesQty zero — reported as entirely unfilled.
#[test]
fn filled_quantity_is_the_filled_amount_not_the_remainder() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    let instrument = context.market.register(265598);
    context.insert_order(crate::types::Order {
        order_id: 77, instrument, side: Side::Buy, price: 0, qty: 100 * QTY_SCALE,
        filled: 0, status: crate::types::OrderStatus::Submitted,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });

    // 100 ordered, 30 filled, 70 still working.
    let frame: std::collections::HashMap<u32, String> = [
        (11u32, "77"), (150, "1"), (39, "1"), (6008, "265598"),
        (38, "100"), (14, "30"), (151, "70"), (54, "1"), (6, "150.0"),
    ].iter().map(|(k, v)| (*k, v.to_string())).collect();
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let orders = shared.orders.drain_open_orders();
    let (_, info) = orders.iter().find(|(id, _)| *id == 77)
        .expect("the order must be reported");
    assert_eq!(info.order.filled_quantity, 30.0,
        "filled must be CumQty (30), not LeavesQty (70)");

    // On a consistent frame the complement `total - leaves` gives the same
    // number, so it has to be told apart on a frame without tag 151 —
    // where the complement would report the whole order as filled.
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    let instrument = context.market.register(265598);
    context.insert_order(crate::types::Order {
        order_id: 78, instrument, side: Side::Buy, price: 0, qty: 100 * QTY_SCALE,
        filled: 0, status: crate::types::OrderStatus::Submitted,
        ord_type: b'2', tif: b'0', stop_price: 0,
    });
    let frame: std::collections::HashMap<u32, String> = [
        (11u32, "78"), (150, "1"), (39, "1"), (6008, "265598"),
        (38, "100"), (14, "30"), (54, "1"), (6, "150.0"),
    ].iter().map(|(k, v)| (*k, v.to_string())).collect();
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let orders = shared.orders.drain_open_orders();
    let (_, info) = orders.iter().find(|(id, _)| *id == 78)
        .expect("the order must be reported");
    assert_eq!(
        info.order.filled_quantity, 30.0,
        "still CumQty with no LeavesQty on the frame, not the complement (100)",
    );

    // A later report that omits tag 14 must not wipe what was established.
    // A pending cancel is exactly that shape, and zeroing there would put
    // back the symptom this corrects.
    let later: std::collections::HashMap<u32, String> = [
        (11u32, "78"), (150, "6"), (39, "6"), (6008, "265598"),
        (38, "100"), (151, "70"), (54, "1"),
    ].iter().map(|(k, v)| (*k, v.to_string())).collect();
    ccp.handle_exec_report(&later, b"", &mut context, &shared, &None, "");

    let orders = shared.orders.drain_open_orders();
    let (_, info) = orders.iter().find(|(id, _)| *id == 78)
        .expect("the order must still be reported");
    assert_eq!(
        info.order.filled_quantity, 30.0,
        "a report without tag 14 keeps the filled quantity, it does not zero it",
    );

    // And the remainder is still the remainder, on the same reports.
    assert_eq!(info.order.total_quantity, 100.0);
}
/// The midnight seed carries the same quantity tag and had the same
/// defect: reading an absent one as zero makes the day's P&L look as
/// though the position were opened intraday, when it was held overnight.
///
/// The row is kept with an unknown quantity rather than dropped, because
/// dropping it says the same wrong thing — a position with no seed row *is*
/// the intraday case — and would discard the cash and realized figures the
/// row does state.
#[test]
fn a_midnight_seed_without_a_quantity_is_not_seeded_flat() {
    let shared = SharedState::new();
    // Two entries: one stating its quantity, one omitting it.
    let body = [
        "6008=756733", "6064=100", "6822=-50.0", "6099=7.5",
        "6008=265598", "6822=-10.0", "6099=2.5",
    ].join("\x01");
    positions::handle_pnl_response(body.as_bytes(), &shared);

    let mut seeds = shared.portfolio.midnight_seeds();
    seeds.sort_by_key(|s| s.con_id);
    assert_eq!(seeds.len(), 2, "both entries are seeded");

    let stated = seeds.iter().find(|s| s.con_id == 756733).expect("stated entry");
    assert_eq!(stated.qty_midnight, Some(100.0));

    let silent = seeds.iter().find(|s| s.con_id == 265598).expect("silent entry");
    assert_eq!(silent.qty_midnight, None, "absent is unknown, not flat");
    assert_eq!(silent.money_traded, -10.0, "the figures it did state survive");
    assert_eq!(silent.realized_pnl, 2.5);
}

/// A fractional overnight position is a position. Narrowing the midnight
/// quantity to a whole number reads half a share as flat, and the day's
/// baseline is then sized against nothing.
#[test]
fn a_fractional_midnight_quantity_survives_the_wire() {
    let shared = SharedState::new();
    let body = ["6008=756733", "6064=0.5", "6822=-1.0"].join("\x01");
    positions::handle_pnl_response(body.as_bytes(), &shared);

    let seeds = shared.portfolio.midnight_seeds();
    let seed = seeds.iter().find(|s| s.con_id == 756733).expect("the row");
    assert_eq!(seed.qty_midnight, Some(0.5), "half a share is not flat");
}


/// The venue states what each position was worth at midnight and what has
/// been traded against it since. Those are the figures the day's change is
/// measured from, so they have to arrive intact rather than be recomputed.
///
/// A combo bucket restates the same five fields against a label. Nothing
/// here is keyed by a label, so a bucket's figures must land nowhere at
/// all instead of on whichever contract happened to come before it.
#[test]
fn the_venue_states_what_a_position_was_worth_at_midnight() {
    let shared = SharedState::new();
    let body = [
        "146=2",
        "6008=756733", "6064=100", "8223=25", "8233=44000.5", "6822=-1250.0", "6099=7.5",
        "6008=265598", "6064=-3", "8223=0", "8233=-1200.0", "6822=0", "6099=0",
        "8058=1",
        "8020=SPY 26JUN CALENDAR", "6064=9", "8233=999999.0", "6822=888888.0", "6099=777777.0",
    ].join("\x01");
    positions::handle_pnl_response(body.as_bytes(), &shared);

    let seeds = shared.portfolio.midnight_seeds();
    assert_eq!(seeds.len(), 2, "the combo bucket is not a contract");

    let long = seeds.iter().find(|s| s.con_id == 756733).expect("first contract");
    assert_eq!(long.qty_midnight, Some(100.0));
    assert_eq!(long.qty_traded, Some(25.0));
    assert_eq!(long.cost_midnight, Some(44000.5), "taken as sent, unscaled");
    assert_eq!(long.money_traded, -1250.0);
    assert_eq!(long.realized_pnl, 7.5);

    let short = seeds.iter().find(|s| s.con_id == 265598).expect("second contract");
    assert_eq!(short.qty_midnight, Some(-3.0));
    assert_eq!(short.cost_midnight, Some(-1200.0), "a short is worth a negative amount");
    assert_eq!(
        short.realized_pnl, 0.0,
        "the combo bucket's figures did not fall through onto the last contract",
    );
}

/// The body says whether it is an answer. One that reports a problem is
/// reporting that instead of stating figures, so nothing in it is read.
#[test]
fn a_pnl_body_that_reports_a_problem_states_no_seeds() {
    let shared = SharedState::new();
    let body = [
        "58=No security definition has been found",
        "6008=756733", "6064=100", "8233=44000.5", "6099=7.5",
    ].join("\x01");
    positions::handle_pnl_response(body.as_bytes(), &shared);
    assert!(shared.portfolio.midnight_seeds().is_empty(), "a problem is not a figure");
}

/// The venue answers against the reference it was handed and falls back to
/// its own request id only when it has none.
#[test]
fn the_reference_id_names_the_request_the_seeds_answer() {
    let shared = SharedState::new();
    let both = ["6529=PLR.2", "8292=PLR.1", "6008=756733", "6064=1"].join("\x01");
    positions::handle_pnl_response(both.as_bytes(), &shared);
    assert_eq!(shared.portfolio.pnl_request_key(), "PLR.1");

    let neither = ["6529=PLR.2", "8292=", "6008=756733", "6064=1"].join("\x01");
    positions::handle_pnl_response(neither.as_bytes(), &shared);
    assert_eq!(shared.portfolio.pnl_request_key(), "PLR.2");
}

/// The price table is two lists paired by position. An unreadable contract
/// id has to hold its place, because dropping it slides every price after
/// it onto the wrong contract.
#[test]
fn the_price_table_pairs_each_contract_with_its_own_price() {
    let shared = SharedState::new();
    let body = [
        "146=3",
        "6008=756733", "6008=not-a-contract", "6008=265598",
        "8057=612.34", "8057=9.99", "8057=1.005",
    ].join("\x01");
    handle_pnl_prices(body.as_bytes(), &shared);

    assert_eq!(shared.portfolio.venue_price(756733).as_deref(), Some("612.34"));
    assert_eq!(
        shared.portfolio.venue_price(265598).as_deref(), Some("1.005"),
        "the third price belongs to the third contract",
    );

    // A later table restates what it names and leaves the rest standing.
    handle_pnl_prices(["6008=756733", "8057=615.00"].join("\x01").as_bytes(), &shared);
    assert_eq!(shared.portfolio.venue_price(756733).as_deref(), Some("615.00"));
    assert_eq!(shared.portfolio.venue_price(265598).as_deref(), Some("1.005"));
}

/// One lookup describes one contract. The venues answer separately and
/// each answer is the same contract with a different exchange, so reporting
/// every one of them returned a single stock as twenty-seven listings.
#[test]
fn a_contract_reaches_the_caller_once_per_request() {
    let mut ccp = CcpState::new();
    let mut seen = |req_id: u32, con_id: i64| {
        ccp.details_delivered.entry(req_id).or_default().insert(con_id)
    };
    assert!(seen(9, 756733), "the first answer is the caller's row");
    assert!(!seen(9, 756733), "and every later venue saying the same is not");
    assert!(seen(9, 885901989), "a different contract still comes through");
    assert!(seen(10, 756733), "as does the same one under another request");
}

/// An option is asked for by expiry date and a future by contract month.
/// Both went out on MaturityMonthYear, so the option lookup asked for a
/// month that does not exist and matched nothing.
#[test]
fn a_maturity_rides_the_tag_its_precision_belongs_to() {
    assert_eq!(maturity_tag("202609"), Some(200), "a contract month");
    assert_eq!(maturity_tag("20260918"), Some(541), "a full expiry date");
    assert_eq!(maturity_tag("20260918 14:30:00"), Some(541), "a date with a time on it");
    assert_eq!(maturity_tag(""), None, "nothing to state");
    assert_eq!(maturity_tag("2026"), None, "too short to be either, so it is not guessed");
}

/// A holding arrives as a contract id and a quantity. Reported before its
/// definition lands, it named no instrument at all — a position in a
/// contract the caller cannot identify.
#[test]
fn a_holding_takes_its_contract_from_the_definition_that_follows() {
    use crate::control::contracts::{ContractDefinition, SecurityType};
    let shared = SharedState::new();
    shared.portfolio.set_position_info(PositionInfo {
        con_id: 793356217, position: 1.0, avg_cost: 38270,
        ..Default::default()
    });
    assert_eq!(
        shared.portfolio.position_info(793356217).map(|p| p.symbol.clone()),
        Some(String::new()),
        "the feed states no symbol",
    );

    let def = ContractDefinition {
        con_id: 793356217,
        symbol: "MES".to_string(),
        sec_type: SecurityType::Future,
        currency: "USD".to_string(),
        ..ContractDefinition::default()
    };
    identify_position(&shared, &def);

    let row = shared.portfolio.position_info(793356217).unwrap();
    assert_eq!(row.symbol, "MES", "and the definition names it");
    assert_eq!(row.position, 1.0, "without disturbing the quantity");
    assert_eq!(row.avg_cost, 38270, "or the basis");
}

/// The lean feed states a quantity and often no cost. Reading the absence
/// as a cost of zero erased the basis of a live holding, and the P&L path
/// reads a zero basis as having acquired it for nothing.
#[test]
fn a_row_without_a_cost_keeps_the_one_on_file() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    context.market.register(265598);

    ccp.handle_position_feed(
        "6008=265598\x016064=100\x016101=150.0\x01".as_bytes(),
        &mut None, &mut context, &shared, &None, &mut hb,
    );
    let basis = shared.portfolio.position_info(265598).map(|i| i.avg_cost);
    assert_eq!(basis, Some(150 * crate::types::PRICE_SCALE));

    // Same holding, stated without a cost.
    ccp.handle_position_feed(
        "6008=265598\x016064=100\x01".as_bytes(),
        &mut None, &mut context, &shared, &None, &mut hb,
    );
    assert_eq!(
        shared.portfolio.position_info(265598).map(|i| i.avg_cost), basis,
        "the basis on file stands where the row states none",
    );

    // A row that closes the holding takes the basis with it, whether or not
    // it states one, or the next position in this contract opens against
    // the last one's cost.
    ccp.handle_position_feed(
        "6008=265598\x016064=0\x016101=151.0\x01".as_bytes(),
        &mut None, &mut context, &shared, &None, &mut hb,
    );
    assert_eq!(
        shared.portfolio.position_info(265598).map(|i| i.avg_cost), Some(0),
        "a closed holding keeps no basis, not even one the row states",
    );
    ccp.handle_position_feed(
        "6008=265598\x016064=100\x016101=150.0\x01".as_bytes(),
        &mut None, &mut context, &shared, &None, &mut hb,
    );

    ccp.handle_position_feed(
        "6008=265598\x016064=0\x01".as_bytes(),
        &mut None, &mut context, &shared, &None, &mut hb,
    );
    assert_eq!(
        shared.portfolio.position_info(265598).map(|i| i.avg_cost), Some(0),
        "a closed holding leaves no basis behind",
    );
    ccp.handle_position_feed(
        "6008=265598\x016064=100\x016101=150.0\x01".as_bytes(),
        &mut None, &mut context, &shared, &None, &mut hb,
    );

    // Stated as zero, which is the broker saying zero.
    ccp.handle_position_feed(
        "6008=265598\x016064=100\x016101=0\x01".as_bytes(),
        &mut None, &mut context, &shared, &None, &mut hb,
    );
    assert_eq!(
        shared.portfolio.position_info(265598).map(|i| i.avg_cost), Some(0),
        "a stated zero is a value, not an absence",
    );
}

/// The feed is the account's own statement of what it holds. It reached the
/// portfolio and the event, and not the table the callback side reads — so
/// a process that restarted holding stock ran its first decisions against
/// flat, and a strategy sizing from `position()` bought what it already had.
#[test]
fn a_position_feed_is_adopted_by_the_engine_not_only_published() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    let instrument = context.market.register(265598);
    assert_eq!(context.position(instrument), 0.0, "the engine starts knowing nothing");

    ccp.handle_position_feed(
        "6008=265598\x016064=500\x016101=151.0\x01".as_bytes(),
        &mut None, &mut context, &shared, &None, &mut hb,
    );

    assert_eq!(context.position(instrument), 500.0, "the account holds 500 and so does the engine");
    assert_eq!(shared.portfolio.position(instrument), 500.0);

    // A later statement is adopted too, not accumulated on top.
    ccp.handle_position_feed(
        "6008=265598\x016064=300\x016101=151.0\x01".as_bytes(),
        &mut None, &mut context, &shared, &None, &mut hb,
    );
    assert_eq!(context.position(instrument), 300.0, "the server's number wins, it is not added");
}

/// The 75 feed leaves a position alone where its running quantity is
/// absent: an entry carrying a conId but no parseable 6064 would otherwise
/// flatten a live position and publish it, as on the account-update path.
#[test]
fn a_position_feed_entry_without_a_quantity_leaves_the_position_alone() {
    for body in [
        // no 6064 at all
        "6008=265598\x016101=151.0\x01",
        // present but not a number
        "6008=265598\x016064=abc\x016101=151.0\x01",
        // parses, but is not a quantity
        "6008=265598\x016064=NaN\x016101=151.0\x01",
        // the same entry flushed by the next conId rather than by the end
        // of the message — a repeating group publishes at both boundaries.
        "6008=265598\x016101=151.0\x016008=756733\x016064=5\x01",
        "6008=265598\x016064=abc\x016101=151.0\x016008=756733\x016064=5\x01",
    ] {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let mut hb = HeartbeatState::new();
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        let event_tx = Some(crate::engine::hot_loop::EventSink::new(tx, Default::default()));
        let instrument = context.market.register(265598);
        shared.portfolio.set_position_info(PositionInfo {
            con_id: 265598, position: 100.0, avg_cost: 0, ..Default::default()
        });
        shared.portfolio.set_position(instrument, 100.0);

        ccp.handle_position_feed(
            body.as_bytes(), &mut None, &mut context, &shared, &event_tx, &mut hb);

        // All three stores move together, so all three are asserted: the
        // row callers read, the atomic the engine reads, and the event.
        assert_eq!(
            shared.portfolio.position_info(265598).map(|p| p.position), Some(100.0),
            "{body:?} must not flatten the position row",
        );
        assert_eq!(
            shared.portfolio.position(instrument), 100.0,
            "{body:?} must not flatten the shared position",
        );
        let flattened = rx.try_iter().any(|e| matches!(
            e, Event::PositionUpdate { con_id: 265598, position: 0.0, .. }));
        assert!(!flattened, "{body:?} must not publish a flat");
    }
}

/// The positive control for the test above: an entry that does state a
/// quantity has to reach all three stores, and at the flush triggered by
/// the next conId rather than only at the end of the message. Without this
/// the absence assertions pass just as well against a feed that publishes
/// nothing at all.
#[test]
fn a_position_feed_entry_with_a_quantity_publishes_it_everywhere() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let event_tx = Some(crate::engine::hot_loop::EventSink::new(tx, Default::default()));
    let instrument = context.market.register(265598);

    // Two entries, so the first is flushed by the second's conId.
    let body = "6008=265598\x016064=42\x016101=151.0\x016008=756733\x016064=5\x01";
    ccp.handle_position_feed(
        body.as_bytes(), &mut None, &mut context, &shared, &event_tx, &mut hb);

    assert_eq!(
        shared.portfolio.position_info(265598).map(|p| p.position), Some(42.0),
        "the position row",
    );
    assert_eq!(shared.portfolio.position(instrument), 42.0, "the shared position");
    assert!(
        rx.try_iter().any(|e| matches!(
            e, Event::PositionUpdate { con_id: 265598, position: 42.0, .. })),
        "the published event",
    );
}

/// An explicit zero is a genuine flat and must still be published.
#[test]
fn a_position_feed_entry_with_an_explicit_zero_still_flattens() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    let mut hb = HeartbeatState::new();
    let instrument = context.market.register(265598);
    shared.portfolio.set_position_info(PositionInfo {
        con_id: 265598, position: 100.0, avg_cost: 0, ..Default::default()
    });
    shared.portfolio.set_position(instrument, 100.0);

    ccp.handle_position_feed(
        b"6008=265598\x016064=0\x016101=151.0\x01",
        &mut None, &mut context, &shared, &None, &mut hb);

    assert_eq!(
        shared.portfolio.position_info(265598).map(|p| p.position), Some(0.0),
        "an explicit zero is a genuine flat",
    );
}

// A margin-reducing preview (close, cash-account sell) resolves to a
// post-trade init margin of exactly 0, which the gateway sends as numeric "0". The old
// `> 0.0` guard dropped it and the caller timed out.
#[test]
fn what_if_zero_init_margin_is_delivered() {
    let (mut ccp, mut context, shared) = what_if_test_state();
    let frame = what_if_frame(&ZERO_CLOSE_FIELDS);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let responses = shared.orders.drain_what_if_responses();
    assert_eq!(responses.len(), 1, "zero-margin preview must be delivered");
    assert_eq!(responses[0].init_margin_after, 0);
    // The completed preview consumes the pending order.
    assert!(context.order(42).is_none());
}

// The not-ready ack carries the literal "n/a" in all six margin fields
 //; it must be skipped so only the real data frame surfaces.
#[test]
fn what_if_not_ready_ack_is_skipped() {
    let (mut ccp, mut context, shared) = what_if_test_state();
    let frame = what_if_frame(&[
        (6826, "n/a"), (6827, "n/a"), (6828, "n/a"),
        (6092, "n/a"), (6093, "n/a"), (6094, "n/a"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    assert!(shared.orders.drain_what_if_responses().is_empty(),
        "n/a ack must not surface as a response");
    // The order stays pending for the subsequent data frame.
    assert!(context.order(42).is_some());
}

// The gateway's real-frame test is "any of the six margin fields
// is set", not "6092 is set". A preview that omits 6092 but carries
// numeric siblings must be delivered, with the absent field read as 0.
#[test]
fn what_if_without_6092_but_numeric_siblings_is_delivered() {
    let (mut ccp, mut context, shared) = what_if_test_state();
    let frame = what_if_frame(&[(6093, "0"), (6094, "945923.47")]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let responses = shared.orders.drain_what_if_responses();
    assert_eq!(responses.len(), 1, "sibling-only preview must be delivered");
    assert_eq!(responses[0].init_margin_after, 0);
    assert_eq!(responses[0].equity_with_loan_after,
        (945923.47 * PRICE_SCALE as f64) as Price);
    assert!(context.order(42).is_none());
}

// "nan" parses as f64::NAN, so it passed the old parse-success
// gate and surfaced as a bogus zero-margin preview. The gateway treats
// nan as unset, so an all-nan frame is not a data frame.
#[test]
fn what_if_nan_sentinels_are_skipped() {
    let (mut ccp, mut context, shared) = what_if_test_state();
    let frame = what_if_frame(&[
        (6826, "nan"), (6827, "nan"), (6828, "nan"),
        (6092, "nan"), (6093, "nan"), (6094, "nan"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    assert!(shared.orders.drain_what_if_responses().is_empty(),
        "all-nan frame must not surface as a response");
    assert!(context.order(42).is_some());
}

// Mixed frame: a nan field is unset, but one finite sibling makes the
// frame real. The nan field itself must read as 0, not poison the price.
#[test]
fn what_if_nan_field_with_finite_sibling_is_delivered() {
    let (mut ccp, mut context, shared) = what_if_test_state();
    let frame = what_if_frame(&[(6092, "nan"), (6094, "945923.47")]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let responses = shared.orders.drain_what_if_responses();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].init_margin_after, 0, "nan field reads as unset/0");
    assert_eq!(responses[0].equity_with_loan_after,
        (945923.47 * PRICE_SCALE as f64) as Price);
}

// A working order carries wire 39=0 whether it is routed or not.
// The gateway reports PreSubmitted while it waits (e.g. placed pre-market)
// and Submitted only once routed to an exchange. The discriminator is the
// routing tags on the same exec report, not a distinct wire status.
fn ord_status_test_state() -> (CcpState, Context, SharedState) {
    let mut context = Context::new();
    let instrument = context.register_instrument(756733);
    context.insert_order(crate::types::Order::new(
        42, instrument, Side::Buy, crate::types::QTY_SCALE, 100 * PRICE_SCALE, b'2', b'0', 0,
    )); // starts at PendingSubmit
    (CcpState::new(), context, SharedState::new())
}

/// A recovery record arriving with the instrument table already full used
/// to take the engine down. A missing order beats a dead hot loop, and the
/// conversion to the fallible register is what makes that true — nothing
/// else in the suite fails if it is reverted.
#[test]
fn a_full_instrument_table_does_not_abort_the_recovery_path() {
    let mut context = Context::new();
    let mut ccp = CcpState::new();
    let shared = SharedState::new();

    // Fill every slot, so the next registration has nowhere to go.
    for con_id in 1..=(crate::types::MAX_INSTRUMENTS as i64) {
        assert!(context.try_register_instrument(con_id).is_some(), "slot {con_id}");
    }
    assert!(
        context.try_register_instrument(999_999).is_none(),
        "the table really is full",
    );

    let mut frame = std::collections::HashMap::new();
    for (tag, val) in [
        (11u32, "42"), (150, "0"), (39, "0"), (6008, "888888"),
        (38, "100"), (55, "SPY"), (54, "1"),
    ] {
        frame.insert(tag, val.to_string());
    }

    // The point of the test: this must return rather than panic.
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    assert!(
        context.order(42).is_none(),
        "the order is not tracked, which is the acknowledged cost",
    );
}
/// Build a fill report for order 42. `extra` adds or overrides tags.
fn fill_frame(extra: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
    let mut m = std::collections::HashMap::new();
    for (tag, val) in [
        (11u32, "42"), (150u32, "F"), (39u32, "1"),
        (17u32, "EXEC-1"), (31u32, "100.0"), (32u32, "10"), (151u32, "90"),
        (14u32, "10"),
        (60u32, "20260101-16:00:00"),
    ] {
        m.insert(tag, val.to_string());
    }
    for (tag, val) in extra {
        m.insert(*tag, val.to_string());
    }
    m
}

fn tracked_order_state() -> (CcpState, Context, SharedState) {
    let mut context = Context::new();
    let instrument = context.register_instrument(756733);
    context.insert_order(crate::types::Order::new(
        42, instrument, Side::Buy, 100 * crate::types::QTY_SCALE, 100 * PRICE_SCALE, b'2', b'0', 0,
    ));
    (CcpState::new(), context, SharedState::new())
}

/// At session start the venue replays recent executions, each carrying its
/// original ExecID and a resend marker. A fresh process has never seen that
/// ID, so the dedup window cannot stop it, and the order is tracked by then
/// because the recovery insert runs first. The marker is what keeps it from
/// becoming a fill event and a position move for something that happened
/// before the process started.
#[test]
fn a_resent_execution_does_not_book_a_fill() {
    for marker in [(97u32, "Y"), (43u32, "Y")] {
        let (mut ccp, mut context, shared) = tracked_order_state();
        context.adjust_order_filled(42, 10 * crate::types::QTY_SCALE); // already counted
        let frame = fill_frame(&[marker]);
        ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

        assert!(
            shared.orders.drain_fills().is_empty(),
            "tag {} = Y restates history and must not book", marker.0,
        );
        assert_eq!(context.position(0), 0.0, "and must not move the position");
    }

    // The positive control: the same report without a marker is a real
    // execution and still books, so the assertions above are not passing
    // against a handler that books nothing.
    let (mut ccp, mut context, shared) = tracked_order_state();
    ccp.handle_exec_report(&fill_frame(&[]), b"", &mut context, &shared, &None, "");
    assert_eq!(shared.orders.drain_fills().len(), 1, "a live execution books");
    assert_eq!(context.position(0), 10.0);
}

/// end to end, as a fresh process sees it: the gateway replays the
/// order as a recovery record and then replays its executions. The record
/// carries the cumulative quantity already filled, so the executions behind
/// it state nothing new. Treating that record as unfilled made every one of
/// them look like fresh quantity, and each emitted a fill for something
/// that happened before the process started.
#[test]
fn a_fresh_process_does_not_book_the_history_it_is_replayed() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();

    // 1. The recovery record: not tracked locally, ten of a hundred filled.
    let mut recovery = std::collections::HashMap::new();
    for (tag, val) in [
        (11u32, "78"), (150u32, "0"), (39u32, "0"), (6008u32, "756733"),
        (38u32, "100"), (14u32, "10"), (55u32, "SPY"), (54u32, "1"), (40u32, "2"),
    ] {
        recovery.insert(tag, val.to_string());
    }
    ccp.handle_exec_report(&recovery, b"", &mut context, &shared, &None, "");
    let _ = shared.orders.drain_fills();

    assert_eq!(
        context.order(78).expect("recovered").filled, 10 * QTY_SCALE,
        "the record's own cumulative quantity is the baseline",
    );

    // 2. Its replayed execution, carrying the same cumulative quantity.
    let mut replay = std::collections::HashMap::new();
    for (tag, val) in [
        (11u32, "78"), (150u32, "F"), (39u32, "1"), (97u32, "Y"),
        (17u32, "OLD-EXEC"), (14u32, "10"), (32u32, "10"), (31u32, "100.0"),
        (151u32, "90"), (60u32, "20260101-16:00:00"),
    ] {
        replay.insert(tag, val.to_string());
    }
    ccp.handle_exec_report(&replay, b"", &mut context, &shared, &None, "");

    assert!(
        shared.orders.drain_fills().is_empty(),
        "the replayed execution states nothing the record did not already carry",
    );
    assert_eq!(context.order(78).expect("tracked").filled, 10 * QTY_SCALE, "and nothing is double-counted");
}

/// The case a blanket suppression of marked reports loses. A CCP reconnect
/// keeps this state — window and order book both survive — and the gateway
/// replays recent executions on the new session. A fill that executed
/// during the outage therefore arrives marked, with an ExecID this session
/// has never seen, and is the first news of it. Refusing it would leave the
/// order permanently short a real fill.
#[test]
fn a_resent_execution_carrying_new_quantity_is_still_booked() {
    let (mut ccp, mut context, shared) = tracked_order_state();

    // Five already booked before the outage.
    context.adjust_order_filled(42, 5 * crate::types::QTY_SCALE);

    // The replay carries eight cumulative — three of which are news.
    let frame = fill_frame(&[(97, "Y"), (14, "8"), (32, "3"), (151, "92")]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    assert_eq!(
        shared.orders.drain_fills().len(), 1,
        "a marked report carrying quantity the order does not have is a real fill",
    );
    assert_eq!(context.position(0), 3.0);

    // And a second copy of that same replay states no more, so it is history.
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    assert!(
        shared.orders.drain_fills().is_empty(),
        "restating the same cumulative quantity is not new",
    );
    assert_eq!(context.position(0), 3.0);
}

/// Two genuine slices of one order, same size and price inside one
/// timestamp tick — the ordinary shape of algo and iceberg execution. The
/// synthesised key must tell them apart, which the cumulative quantity does
/// because it advances with every execution on the order.
#[test]
fn two_same_priced_slices_in_one_tick_are_not_one_execution() {
    let (mut ccp, mut context, shared) = tracked_order_state();

    let mut first = fill_frame(&[(32, "10"), (151, "90"), (14, "10")]);
    first.remove(&17);
    let mut second = fill_frame(&[(32, "10"), (151, "80"), (14, "20")]);
    second.remove(&17);

    ccp.handle_exec_report(&first, b"", &mut context, &shared, &None, "");
    ccp.handle_exec_report(&second, b"", &mut context, &shared, &None, "");

    assert_eq!(shared.orders.drain_fills().len(), 2, "both slices book");
    assert_eq!(context.position(0), 20.0);
}

/// Without an ExecID the execution is keyed on the fields that identify
/// it, rather than skipping the dedup window. Absent tag 17 is a shape a
/// replay takes, so skipping it books the copy a second time and doubles
/// the position.
#[test]
fn an_execution_without_an_exec_id_is_still_deduplicated() {
    let (mut ccp, mut context, shared) = tracked_order_state();
    let mut frame = fill_frame(&[]);
    frame.remove(&17);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    assert_eq!(shared.orders.drain_fills().len(), 1, "booked once, not twice");
    assert_eq!(context.position(0), 10.0, "and the position moved once");

    // A genuinely different execution on the same order is not swallowed by
    // the synthesised key.
    let mut other = fill_frame(&[]);
    other.remove(&17);
    other.insert(32, "5".to_string());
    ccp.handle_exec_report(&other, b"", &mut context, &shared, &None, "");
    assert_eq!(shared.orders.drain_fills().len(), 1, "a distinct execution still books");
    assert_eq!(context.position(0), 15.0);
}

/// A long session rolls executions out of the ExecID window, and a replay
/// arrives unordered and without ExecIDs of its own. Summing what each
/// report says it executed counts quantity the order already holds; reading
/// the cumulative figure it reports settles on the true total whatever
/// order the copies arrive in.
#[test]
fn a_replay_of_booked_history_adds_nothing_to_the_order() {
    let (mut ccp, mut context, shared) = tracked_order_state();
    context.adjust_order_filled(42, 12 * crate::types::QTY_SCALE); // both executions already booked

    let mut later = fill_frame(&[(97, "Y"), (14, "12"), (32, "4"), (151, "88")]);
    later.remove(&17);
    let mut earlier = fill_frame(&[(97, "Y"), (14, "8"), (32, "3"), (151, "92")]);
    earlier.remove(&17);
    ccp.handle_exec_report(&later, b"", &mut context, &shared, &None, "");
    ccp.handle_exec_report(&earlier, b"", &mut context, &shared, &None, "");

    assert!(shared.orders.drain_fills().is_empty(), "history restated is not new quantity");
    assert_eq!(context.order(42).unwrap().filled, 12 * QTY_SCALE, "and the order is not overcounted");
    assert_eq!(context.position(0), 0.0);

    // A fill from the same replay that this session has not booked is news
    // and still reaches the caller.
    let mut fresh = fill_frame(&[(97, "Y"), (14, "15"), (32, "3"), (151, "85")]);
    fresh.remove(&17);
    ccp.handle_exec_report(&fresh, b"", &mut context, &shared, &None, "");
    assert_eq!(shared.orders.drain_fills().len(), 1, "quantity the order lacks still books");
    assert_eq!(context.position(0), 3.0);
}

/// The same execution delivered marked and then unmarked. The cumulative
/// figure decides the marked copy, but the unmarked one is an ordinary
/// report and the window is the only thing that can catch it — so a marked
/// report has to be remembered even though it was not judged by the window.
#[test]
fn a_marked_execution_is_remembered_for_its_unmarked_twin() {
    let (mut ccp, mut context, shared) = tracked_order_state();
    context.adjust_order_filled(42, 5 * crate::types::QTY_SCALE);

    let marked = fill_frame(&[(97, "Y"), (17, "E-9"), (14, "9"), (32, "4"), (151, "91")]);
    ccp.handle_exec_report(&marked, b"", &mut context, &shared, &None, "");
    assert_eq!(shared.orders.drain_fills().len(), 1, "the marked copy books what is new");
    assert_eq!(context.order(42).unwrap().filled, 9 * QTY_SCALE);

    // The same execution again, this time without its marker.
    let unmarked = fill_frame(&[(17, "E-9"), (14, "9"), (32, "4"), (151, "91")]);
    ccp.handle_exec_report(&unmarked, b"", &mut context, &shared, &None, "");

    assert!(
        shared.orders.drain_fills().is_empty(),
        "the window catches the copy the cumulative figure cannot judge",
    );
    assert_eq!(context.order(42).unwrap().filled, 9 * QTY_SCALE, "and nothing is double-booked");
}

/// The ExecID window evicts oldest-first, so a replay batch deeper than
/// the window no longer holds its own head and the duplicate would book a
/// second time. For an order this session tracks, that window is the only
/// guard the ID itself provides.
///
/// A replayed execution is marked, so it is booked on the cumulative
/// quantity it reports rather than on the increment — and a copy that
/// restates quantity the order already holds adds nothing whether or not
/// its ExecID is still in the window. The window stops being the guard.
#[test]
fn a_replay_deeper_than_the_exec_id_window_does_not_double_count() {
    let (mut ccp, mut context, shared) = tracked_order_state();
    context.adjust_order_filled(42, 12 * crate::types::QTY_SCALE);

    // The window has rolled past this execution, so its ID is unseen here —
    // which is the whole point: the dedup window cannot be what saves this.
    let replayed = fill_frame(&[(97, "Y"), (17, "EVICTED-1"), (14, "12"), (32, "4"), (151, "88")]);

    ccp.handle_exec_report(&replayed, b"", &mut context, &shared, &None, "");

    assert!(
        shared.orders.drain_fills().is_empty(),
        "a replay the window has forgotten still adds no quantity the order holds",
    );
    assert_eq!(context.order(42).unwrap().filled, 12 * QTY_SCALE);
    assert_eq!(context.position(0), 0.0);
}

/// The same marked execution delivered twice, both copies carrying more
/// cumulative quantity than the order held when the first arrived.
#[test]
fn a_marked_execution_delivered_twice_books_once() {
    let (mut ccp, mut context, shared) = tracked_order_state();
    context.adjust_order_filled(42, 5 * crate::types::QTY_SCALE);

    let frame = fill_frame(&[(97, "Y"), (14, "12"), (32, "4"), (151, "88")]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    assert_eq!(shared.orders.drain_fills().len(), 1, "the second copy adds nothing");
    assert_eq!(context.order(42).unwrap().filled, 12 * QTY_SCALE);
    assert_eq!(context.position(0), 7.0);
}

/// A replacement that raises the total lets an order fill the same size at
/// the same price and leave the same quantity behind twice. Everything the
/// synthesised key had to work with repeats except the cumulative figure.
#[test]
fn a_raised_total_does_not_collapse_two_slices_into_one() {
    let (mut ccp, mut context, shared) = tracked_order_state();

    let mut first = fill_frame(&[(32, "10"), (151, "90"), (14, "10")]);
    first.remove(&17);
    // Total raised from 100 to 110; the next slice again leaves 90.
    let mut second = fill_frame(&[(32, "10"), (151, "90"), (14, "20")]);
    second.remove(&17);

    ccp.handle_exec_report(&first, b"", &mut context, &shared, &None, "");
    ccp.handle_exec_report(&second, b"", &mut context, &shared, &None, "");

    assert_eq!(shared.orders.drain_fills().len(), 2, "both slices book");
    assert_eq!(context.position(0), 20.0);
}

/// An execution with no ExecID that arrives ahead of the recovery record
/// for its order. The key must not be spent on the copy that had nothing to
/// book against, or the delivery that finally could is refused.
#[test]
fn a_key_is_not_spent_before_the_order_exists() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    let mut frame = fill_frame(&[]);
    frame.remove(&17);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    assert!(shared.orders.drain_fills().is_empty(), "nothing to book against yet");

    let instrument = context.register_instrument(756733);
    context.insert_order(crate::types::Order::new(
        42, instrument, Side::Buy, 100 * crate::types::QTY_SCALE, 100 * PRICE_SCALE, b'2', b'0', 0,
    ));
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    assert_eq!(shared.orders.drain_fills().len(), 1, "the execution is still bookable");
    assert_eq!(context.position(0), 10.0);
}
/// A request is recorded as pending only where it went out. Recording it
/// regardless — discarding the send error, pushing outside the block that
/// needs a connection — queues a request issued while the transport is down
/// with nothing on the wire to answer it.
#[test]
fn a_matching_symbols_request_that_was_not_sent_is_not_recorded() {
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let shared = SharedState::new();

    // No transport at all.
    let mut no_conn: Option<Connection> = None;
    ccp.send_matching_symbols_request(7, "AAPL", &mut no_conn, &mut hb, &shared);
    assert!(
        ccp.pending_matching_symbols.is_empty(),
        "nothing was sent, so nothing is awaiting a reply",
    );

    // And with one, it is recorded.
    let listener = std::net::TcpListener::bind("127.0.1:0").unwrap();
    let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
    ccp.send_matching_symbols_request(8, "AAPL", &mut conn, &mut hb, &shared);
    assert_eq!(ccp.pending_matching_symbols.len(), 1, "a sent request is awaited");
    assert_eq!(ccp.pending_matching_symbols[0].0, 8);
}

/// An advisor's configuration request names its partition on tag 6906.
///
/// Tag 6158 carries the request's own number. The number is stated first and
/// the partition second, which is the order asserted here.
#[test]
fn an_advisor_request_names_the_partition_on_the_tag_that_carries_it() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let mut buf = [0u8; 4096];
    let sent = |peer: &mut std::net::TcpStream, buf: &mut [u8]| -> Vec<(String, String)> {
        let n = peer.read(buf).unwrap();
        String::from_utf8_lossy(&buf[..n])
            .split('\u{1}')
            .filter_map(|f| f.split_once('=').map(|(t, v)| (t.to_string(), v.to_string())))
            .skip_while(|(t, _)| t != "6040")
            .take_while(|(t, _)| t != "10")
            .collect()
    };

    // Asking for one partition: the whole of it, under command five.
    ccp.send_advisor_config(5, "Profile", None, &mut conn, &mut hb);
    let fields = sent(&mut peer, &mut buf);
    let names: Vec<&str> = fields.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, ["6040", "6905", "6158", "6906"], "{fields:?}");
    assert_eq!(fields[0].1, "116");
    assert_eq!(fields[1].1, "5");
    assert_eq!(fields[2].1, "1", "the first request of the session states itself as one");
    assert_eq!(fields[3].1, "Profile", "the partition, on the tag that carries it");

    // Replacing one carries the document beside it, and the next number.
    ccp.send_advisor_config(3, "Group", Some("<xml/>"), &mut conn, &mut hb);
    let fields = sent(&mut peer, &mut buf);
    let names: Vec<&str> = fields.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, ["6040", "6905", "6158", "6906", "6118"], "{fields:?}");
    assert_eq!(fields[2].1, "2", "each request states a number of its own");
    assert_eq!(fields[3].1, "Group");
    assert_eq!(fields[4].1, "<xml/>");
}

/// The venue reads a chain request positionally, so the tags have to be
/// stated in the order it expects them and the underlying has to be named
/// on the tag that suits the derivative being asked for.
#[test]
fn a_chain_request_states_its_tags_in_order() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let shared = SharedState::new();
    let mut buf = [0u8; 4096];
    // The tags a caller cannot see are the session's own; the request
    // itself starts at the sub-message type.
    let sent = |peer: &mut std::net::TcpStream, buf: &mut [u8]| -> Vec<(String, String)> {
        let n = peer.read(buf).unwrap();
        String::from_utf8_lossy(&buf[..n])
            .split('\u{1}')
            .filter_map(|f| f.split_once('=').map(|(t, v)| (t.to_string(), v.to_string())))
            .skip_while(|(t, _)| t != "6040")
            .take_while(|(t, _)| t != "10")
            .collect()
    };

    ccp.send_option_params_request(7, "aapl", "", "STK", 265598, &mut conn, &mut hb, &shared);
    let fields = sent(&mut peer, &mut buf);
    let names: Vec<&str> = fields.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, ["6040", "55", "310", "6346", "6320", "6994"], "an equity chain: {fields:?}");
    assert_eq!(fields[0].1, "138");
    assert_eq!(fields[1].1, "AAPL", "the symbol is stated upper cased");
    // The underlying's own type. Naming the derivative here is answered
    // "Unknown contract": there is no option contract by that symbol.
    assert_eq!(fields[2].1, "STK");
    assert_eq!(fields[3].1, "265598");
    assert_eq!(ccp.pending_option_params.len(), 1, "and the request awaits its reply");

    ccp.send_option_params_request(8, "ES", "CME", "FUT", 495512563, &mut conn, &mut hb, &shared);
    let fields = sent(&mut peer, &mut buf);
    let names: Vec<&str> = fields.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, ["6040", "55", "310", "6346", "6320", "6994", "6995"], "a futures chain: {fields:?}");
    assert_eq!(fields[2].1, "FUT", "a future names itself, not its options");
    assert_eq!(fields[6].1, "CME", "and the venue rides only for a future");

    ccp.send_option_params_request(9, "SPX", "CME", "IND", 416904, &mut conn, &mut hb, &shared);
    let fields = sent(&mut peer, &mut buf);
    let names: Vec<&str> = fields.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, ["6040", "55", "310", "6457", "6320", "6994"], "a futures chain on an index: {fields:?}");
    assert_eq!(fields[2].1, "IND");

    // A caller who states no type claims nothing. Standing STK in asked for a
    // stock's chain on whatever that symbol is, which for an index or a future
    // is a different contract or none.
    ccp.send_option_params_request(10, "SPX", "", "", 416904, &mut conn, &mut hb, &shared);
    let fields = sent(&mut peer, &mut buf);
    let names: Vec<&str> = fields.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, ["6040", "55", "6346", "6320", "6994"], "an unstated type: {fields:?}");
}

/// A caller is waiting for the end of a request that never reached the
/// wire. Nothing on the socket will ever end it, so the client does.
#[test]
fn a_chain_request_that_could_not_be_sent_is_still_answered() {
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let shared = SharedState::new();
    let mut no_conn: Option<Connection> = None;

    ccp.send_option_params_request(7, "AAPL", "", "STK", 265598, &mut no_conn, &mut hb, &shared);

    assert!(ccp.pending_option_params.is_empty(), "nothing was sent, so nothing is awaited");
    let answered = shared.reference.drain_option_params();
    assert_eq!(answered.len(), 1, "the request still ends");
    assert!(answered[0].2.is_empty(), "with nothing in the chain");
}

/// The reply states no request id, so the symbol it names is what ties it
/// back to the request, and the conId the caller asked under is what the
/// callback reports.
#[test]
fn a_chain_reply_answers_the_request_that_named_its_underlying() {
    let mut ccp = CcpState::new();
    let shared = SharedState::new();
    ccp.pending_option_params.push((3, "SPY".into(), 756733, Instant::now() + OPTION_CHAIN_TIMEOUT));
    ccp.pending_option_params.push((7, "AAPL".into(), 265598, Instant::now() + OPTION_CHAIN_TIMEOUT));
    let msg = fix::fix_build(
        &[
            (fix::TAG_MSG_TYPE, "U"),
            (6040, "139"),
            (55, "AAPL"),
            (6775, "20260116/20260320/EXPW=20260109"),
            (6346, "265598"),
            (100, "SMART"),
            (6058, "AAPL"),
            (231, "100"),
            (6997, "140.0;145.0"),
        ],
        1,
    );

    ccp.handle_option_chain(&msg, &shared);

    assert_eq!(ccp.pending_option_params.len(), 1, "only the request it answers is spent");
    assert_eq!(ccp.pending_option_params[0].0, 3);
    let answered = shared.reference.drain_option_params();
    assert_eq!(answered.len(), 1);
    let (req_id, con_id, scopes) = &answered[0];
    assert_eq!(*req_id, 7);
    assert_eq!(*con_id, 265598, "the underlying the caller asked about");
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].exchange, "SMART");
    assert_eq!(scopes[0].trading_class, "AAPL");
    assert_eq!(scopes[0].multiplier, "100");
    assert_eq!(scopes[0].expirations, vec!["20260116", "20260320"]);
    assert_eq!(scopes[0].strikes, vec![140.0, 145.0]);
}

/// An entry left in the queue would both hang its caller and stand ready
/// to absorb the answer to a later request for the same underlying.
#[test]
fn an_unanswered_chain_request_is_given_up_on() {
    let mut ccp = CcpState::new();
    let shared = SharedState::new();
    ccp.pending_option_params.push((7, "AAPL".into(), 265598, Instant::now() - Duration::from_secs(1)));
    ccp.pending_option_params.push((8, "SPY".into(), 756733, Instant::now() + OPTION_CHAIN_TIMEOUT));

    ccp.sweep_pending_option_params(&shared);

    assert_eq!(ccp.pending_option_params.len(), 1, "the expired one is dropped");
    assert_eq!(ccp.pending_option_params[0].0, 8, "and the live one is kept");
    let answered = shared.reference.drain_option_params();
    assert_eq!(answered.len(), 1, "the caller of the expired one is told it is over");
        assert_eq!(answered[0].0, 7);
}

/// Nothing expired an unanswered request, so it stayed queued for the life
/// of the process — and the reply matcher falls back to the head of that
/// queue when a reply carries no echoed request id, so a stale entry could
/// absorb a later request's answer.
#[test]
fn an_unanswered_matching_symbols_request_is_given_up_on() {
    let mut ccp = CcpState::new();
    let shared = SharedState::new();
    ccp.pending_matching_symbols.push((7, Instant::now() - Duration::from_secs(1)));
    ccp.pending_matching_symbols.push((8, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

    ccp.sweep_pending_matching_symbols(&shared);

    assert_eq!(ccp.pending_matching_symbols.len(), 1, "the expired one is dropped");
    assert_eq!(ccp.pending_matching_symbols[0].0, 8, "and the live one is kept");
}
/// Tag 583 is the link id the engine sends the OCA group on. Reading it
/// back as a parent produced a stable non-zero value shared by every order
/// in the group — none of which has a parent — and nothing told it apart
/// from a real link.
#[test]
fn an_oca_group_is_not_reported_as_a_parent() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1"),
        (583, "PROBE-OCA-1"),
    ]);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let updates = shared.orders.drain_order_updates();
    assert_eq!(updates.len(), 1, "the status is still reported");
    assert_eq!(
        updates[0].parent_id, 0,
        "an order in an OCA group has no parent, so none is reported",
    );
}

/// Not just the one group name, and not just one status: any value on 583
/// is a link id rather than a parent, at every point in the order's life.
#[test]
fn no_group_name_or_status_produces_a_parent() {
    for group in ["PROBE-OCA-1", "G", "12345", "a name with spaces"] {
        for (ord_status, exec_type) in [("0", "0"), ("1", "2"), ("2", "2"), ("4", "4")] {
            let (mut ccp, mut context, shared) = ord_status_test_state();
            let frame = exec_report_frame(&[
                (39, ord_status), (150, exec_type), (100, "ARCA"), (198, "ARCA:1"),
                (583, group),
            ]);
            ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
            let updates = shared.orders.drain_order_updates();
            // Without this a case that produced no update at all would
            // pass the loop below by never entering it.
            assert_eq!(
                updates.len(), 1,
                "group {group:?} at status {ord_status} must produce one update",
            );
            assert_eq!(
                updates[0].parent_id, 0,
                "group {group:?} at status {ord_status} must not become a parent",
            );
        }
    }
}

/// A report carrying no group reported no parent before this change too, so
/// that case alone cannot tell the fix from the bug.
#[test]
fn a_report_without_a_group_still_has_no_parent() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[(39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1")]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let updates = shared.orders.drain_order_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].parent_id, 0);
}

/// Tag 6107 is what the bracket path *sends* a parent on. Whether the
/// gateway ever echoes it on a report has not been established here, and
/// the engine does not read it either way; this pins that, so wiring it up
/// becomes a deliberate change with evidence behind it rather than a
/// silent one. It passes on the old implementation too — it guards a
/// different invariant from the rest of this change.
#[test]
fn tag_6107_is_not_read_back_as_a_parent() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1"), (6107, "4242"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let updates = shared.orders.drain_order_updates();
    assert_eq!(updates[0].parent_id, 0, "6107 is a client id, not a parent order");
    }

/// A refused revision arrives on the same message as an accepted one and
/// was read as the acceptance, so a modify the gateway would not make was
/// reported to the caller as made.
#[test]
fn a_refused_revision_is_not_an_acknowledgement() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "5"), (150, "5"), (100, "ARCA"), (198, "ARCA:1"), (378, "102"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let updates = shared.orders.drain_order_updates();
    assert!(
        !updates.iter().any(|u| u.status == crate::types::OrderStatus::Submitted),
        "a refused revision does not put the order back to working: {updates:?}",
    );
}

/// A busted trade arrives as an execution like any other. Its quantity
/// reconciles against the order's cumulative figure rather than adding to it,
/// and the reconciliation may be negative.
#[test]
fn a_busted_execution_reconciles_rather_than_adds() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    // The order has already filled fifty, and the position holds them.
    let booked = exec_report_frame(&[
        (39, "1"), (150, "F"), (100, "ARCA"), (198, "ARCA:1"),
        (17, "exec-1"), (32, "50"), (31, "412.25"), (14, "50"), (38, "100"),
    ]);
    ccp.handle_exec_report(&booked, b"", &mut context, &shared, &None, "");
    assert_eq!(context.order(42).unwrap().filled, 50 * QTY_SCALE, "the trade is booked");
    assert_eq!(context.position(0), 50.0);
    let _ = shared.orders.drain_fills();

    // The venue busts it: the cumulative quantity goes back to nothing.
    let bust = exec_report_frame(&[
        (39, "1"), (150, "F"), (100, "ARCA"), (198, "ARCA:1"),
        (17, "exec-2"), (20, "1"), (32, "50"), (31, "412.25"), (14, "0"), (38, "100"),
    ]);
    ccp.handle_exec_report(&bust, b"", &mut context, &shared, &None, "");

    assert_eq!(
        context.order(42).unwrap().filled, 0,
        "the order no longer holds a trade the venue undid",
    );
    assert_eq!(context.position(0), 0.0, "and neither does the position");
    let fills = shared.orders.drain_fills();
    assert!(
        fills.iter().any(|f| f.qty == -50 * QTY_SCALE),
        "the caller is told what was taken back: {fills:?}",
    );
}

/// A correction restates an execution that was already booked. Adding its
/// quantity on top counts the same trade twice; the cumulative figure is
/// what the order actually holds.
#[test]
fn a_corrected_execution_reconciles_to_the_cumulative_figure() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    // The order already holds 50. The correction restates the trade at 60.
    let first = exec_report_frame(&[
        (39, "1"), (150, "F"), (100, "ARCA"), (198, "ARCA:1"),
        (17, "exec-1"), (32, "50"), (31, "412.25"), (14, "50"), (38, "100"),
    ]);
    ccp.handle_exec_report(&first, b"", &mut context, &shared, &None, "");
    let booked: i64 = shared.orders.drain_fills().iter().map(|f| f.qty).sum();
    assert_eq!(booked, 50 * QTY_SCALE, "the original execution books what it states");

    let corrected = exec_report_frame(&[
        (39, "1"), (150, "F"), (100, "ARCA"), (198, "ARCA:1"),
        (17, "exec-2"), (20, "2"), (32, "60"), (31, "412.25"), (14, "60"), (38, "100"),
    ]);
    ccp.handle_exec_report(&corrected, b"", &mut context, &shared, &None, "");
    let after: i64 = shared.orders.drain_fills().iter().map(|f| f.qty).sum();
    assert_eq!(after, 10 * QTY_SCALE, "the correction books the difference, not the whole trade again");
}

/// A live order was retired by this: `D` is not in the terminal's terminal
/// set, and reading it as cancelled told the caller an order was gone while
/// it was still working and still able to fill.
#[test]
fn a_pending_status_does_not_retire_the_order() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[(39, "D"), (150, "D"), (100, "ARCA"), (198, "ARCA:1")]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let updates = shared.orders.drain_order_updates();
    assert_eq!(updates[0].status, crate::types::OrderStatus::PendingCancel,
        "D is pending, not cancelled");
    assert_ne!(updates[0].status, crate::types::OrderStatus::Cancelled);
}

/// The fill was thrown away with the report: an unrecognised status returned
/// before anything read the execution, so a real fill on a status this did
/// not know about was silently lost.
#[test]
fn an_unknown_status_still_books_its_fill() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "\u{7}"), (150, "F"), (100, "ARCA"), (198, "ARCA:1"),
        (32, "50"), (31, "412.25"), (14, "50"), (6, "412.25"), (38, "100"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let fills = shared.orders.drain_fills();
    assert_eq!(fills.len(), 1, "the fill survives a status this does not know");
    assert_eq!(fills[0].qty, 50 * QTY_SCALE);
}

/// Absent is not zero. Without 151 the caller was told nothing was left on an
/// order that was still working, which reads as done.
#[test]
fn a_missing_leaves_qty_falls_back_to_what_is_unfilled() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "1"), (150, "1"), (100, "ARCA"), (198, "ARCA:1"),
        (38, "100"), (14, "30"), (32, "30"), (31, "412.25"), (6, "412.25"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let updates = shared.orders.drain_order_updates();
    assert_eq!(updates[0].remaining_qty, 70.0, "100 ordered less 30 filled, not 0");
}

/// A report is written down in full before any of it is announced.
///
/// A caller acts on a notification the moment it arrives — withdrawing the
/// order it names, reading the fill, listing what has finished — and each of
/// those asks this session for a record. Announcing first meant answering those
/// questions about a report still being applied: the caller was told, asked,
/// and was told no such thing had happened.
#[test]
fn a_finished_order_is_written_down_before_it_is_announced() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let sink = Some(crate::engine::hot_loop::EventSink::new(
        tx,
        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    ));
    let frame = exec_report_frame(&[
        (39, "2"), (150, "2"), (100, "ARCA"), (198, "ARCA:1"),
        (38, "100"), (14, "100"), (32, "100"), (31, "412.25"), (6, "412.25"),
        (151, "0"), (6008, "265598"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &sink, "");

    // Everything the report changed is readable.
    assert_eq!(shared.orders.drain_fills().len(), 1, "the fill is recorded");
    assert_eq!(
        shared.orders.drain_completed_orders().len(),
        1,
        "and so is the order having finished",
    );
    assert_eq!(shared.orders.drain_order_updates().len(), 1, "and the status it finished in");

    // And the fill was announced before the status that followed from it.
    let announced: Vec<_> = rx.try_iter().collect();
    let kinds: Vec<&str> = announced
        .iter()
        .map(|e| match e {
            crate::engine::hot_loop::Event::Fill(_) => "fill",
            crate::engine::hot_loop::Event::OrderUpdate(_) => "status",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["fill", "status"], "what traded, then where the order stands");
}

/// The order id hash on tag 37 is a separate concern and must keep working.
#[test]
fn the_order_id_still_produces_a_stable_perm_id() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1"),
        (37, "0256d0f1.0001417e.6a6982d2.0001"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let updates = shared.orders.drain_order_updates();
    assert_ne!(updates[0].perm_id, 0, "the order id still yields a permId");
}

/// The recovered side is not confined to the recovered record: every later
/// fill for that order books through the tracked path and takes its side
/// from here, so a guess moves the position by twice the fill in the wrong
/// direction, and nothing afterwards distinguishes it from a stated side.
#[test]
fn a_recovery_record_without_a_side_is_not_tracked() {
    for missing in ["", "9", "X"] {
        let mut context = Context::new();
        let mut ccp = CcpState::new();
        let shared = SharedState::new();
        let mut frame = std::collections::HashMap::new();
        for (tag, val) in [
            (11u32, "77"), (150, "0"), (39, "0"), (6008, "756733"), (38, "100"), (55, "SPY"),
        ] {
            frame.insert(tag, val.to_string());
        }
        if !missing.is_empty() {
            frame.insert(54, missing.to_string());
        }

        ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

        assert!(
            context.order(77).is_none(),
            "Side={missing:?} must not be guessed into a tracked order",
            );
    }
}

/// A stated side is still recovered.
#[test]
fn a_recovery_record_with_a_side_is_tracked() {
    for (tag54, expected) in [("1", Side::Buy), ("2", Side::Sell), ("5", Side::ShortSell)] {
        let mut context = Context::new();
        let mut ccp = CcpState::new();
        let shared = SharedState::new();
        let mut frame = std::collections::HashMap::new();
        for (tag, val) in [
            (11u32, "77"), (150, "0"), (39, "0"), (6008, "756733"), (38, "100"),
            (55, "SPY"), (54, tag54),
        ] {
            frame.insert(tag, val.to_string());
        }

        ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

        let order = context.order(77).expect("a stated side is recovered");
        assert_eq!(order.side, expected, "Side={tag54}");
    }
}
/// An unrecognised or absent tag 59 leaves the wire match with nothing to
/// report, so the fallback that knows what the caller submitted can run. An
/// arm producing `DAY` for those cases keeps the fallback from ever
/// running, and `DAY` is an ordinary value: a caller reconciling its own
/// orders gets a plausible answer that disagrees with what it sent.
#[test]
fn an_unknown_time_in_force_falls_back_to_the_one_that_was_submitted() {
    // A tracked order submitted GTC, so a wrong answer is visibly wrong.
    let tracked = |ccp: &mut CcpState, context: &mut Context, shared: &SharedState, tif59: Option<&str>| {
        context.insert_order(crate::types::Order::new(
            42, 0, Side::Buy, crate::types::QTY_SCALE, 100 * PRICE_SCALE, b'2', b'1', 0,
        ));
        let mut pairs = vec![(39u32, "0"), (150u32, "0"), (100u32, "ARCA"), (198u32, "ARCA:1")];
        if let Some(v) = tif59 {
            pairs.push((59, v));
        }
        let frame = exec_report_frame(&pairs);
        ccp.handle_exec_report(&frame, b"", context, shared, &None, "");
        shared.orders.get_order_info(42).expect("published").order.tif.clone()
    };

    // Absence is the only case the fallback answers: the report states no
    // time-in-force, and this client knows what it submitted.
    let (mut ccp, mut context, shared) = ord_status_test_state();
    assert_eq!(
        tracked(&mut ccp, &mut context, &shared, None), "GTC",
        "the submitted time-in-force, not a plausible default",
    );

    // A stated code is still taken from the wire, including one that
    // happens to differ from the tracked order — the gateway is
    // authoritative when it says anything at all.
    let (mut ccp, mut context, shared) = ord_status_test_state();
    assert_eq!(tracked(&mut ccp, &mut context, &shared, Some("0")), "DAY");
    let (mut ccp, mut context, shared) = ord_status_test_state();
    assert_eq!(tracked(&mut ccp, &mut context, &shared, Some("4")), "FOK");

    // Including a code this does not name: seen as stated rather than
    // silently replaced by the local order's unrelated value.
    let (mut ccp, mut context, shared) = ord_status_test_state();
    assert_eq!(tracked(&mut ccp, &mut context, &shared, Some("5")), "5");
}

/// The case the test above cannot reach: an order this session never
/// placed, arriving on the session-start recovery push with no tag 59.
///
/// There is nothing to recover the time-in-force from, and an invented one
/// would be read as though it were the caller's own. An invented GTC rests
/// until cancelled; an invented DAY expires with the session. Neither is
/// knowledge, so the safer of the two is the one that does not leave an
/// order resting.
#[test]
fn a_recovered_order_without_a_time_in_force_states_none() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();

    // A recovery record: not tracked locally, states a contract and size,
    // states no time-in-force.
    let mut frame = std::collections::HashMap::new();
    for (tag, val) in [
        (11u32, "78"), (150u32, "0"), (39u32, "0"), (6008u32, "756733"),
        (38u32, "100"), (55u32, "SPY"), (54u32, "1"), (40u32, "2"),
    ] {
        frame.insert(tag, val.to_string());
    }
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    assert_eq!(
        context.order(78).expect("recovered").tif, crate::types::TIF_UNSTATED,
        "an absent time-in-force is recorded as unstated, not guessed",
    );
    assert_eq!(
        shared.orders.get_order_info(78).expect("published").order.tif, "",
        "and is reported as unstated rather than as an ordinary value",
    );

    // And a replace of it carries no tag 59, so the guess is never sent to
    // the gateway as an instruction — a fabricated DAY would expire an
    // order that is resting until cancelled.
    let listener = std::net::TcpListener::bind("127.0.1:0").unwrap();
    let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
    let mut hb = crate::engine::hot_loop::HeartbeatState::new();
    let shared_arc = std::sync::Arc::new(SharedState::new());

    context.modify(78, 100 * PRICE_SCALE, 100, false);
    crate::engine::hot_loop::order_builder::drain_and_send_orders(
        &mut conn, &mut context, "DU1", &mut hb, false, &shared_arc, false, &None,
    );

    let mut buf = [0u8; 4096];
    let n = std::io::Read::read(&mut peer, &mut buf).unwrap();
    let msg = String::from_utf8_lossy(&buf[..n]);
    assert!(msg.contains("35=G"), "a replace was sent: {msg}");
    assert!(!msg.split('\u{1}').any(|f| f.starts_with("59=")),
        "a replace must not restate a time-in-force the order never had: {msg}");
}

fn cancel_reject_frame(reason_code: &str) -> std::collections::HashMap<u32, String> {
    let mut m = std::collections::HashMap::new();
    m.insert(41u32, "C42".to_string()); // OrigClOrdID
    m.insert(434u32, "1".to_string());
    m.insert(102u32, reason_code.to_string());
    m
}

fn tracked_for_cancel(context: &mut Context) {
    let instrument = context.register_instrument(756733);
    context.insert_order(crate::types::Order::new(
        42, instrument, Side::Buy, 100 * crate::types::QTY_SCALE, 100 * PRICE_SCALE, b'2', b'0', 0,
    ));
    context.update_order_status(42, crate::types::OrderStatus::PendingCancel, false);
}

/// A cancel answered with UnknownOrder says the order does not exist on
/// the venue's side. Forcing it back to working asserts the opposite of the
/// message being handled, and the engine's own view governs subsequent
/// cancels, modifies and reconnect bookkeeping, so a phantom order persists
/// there while the cache row that would surface it is removed.
#[test]
fn an_unknown_order_rejection_retires_the_order() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    tracked_for_cancel(&mut context);
    shared.orders.push_order_info(42, RichOrderInfo {
        contract: api::Contract::default(),
        order: api::Order::default(),
        order_state: api::OrderState::default(),
        last_exec: api::Execution::default(),
    });

    ccp.handle_cancel_reject(&cancel_reject_frame("1"), &mut context, &shared, &None);

    assert!(
        context.order(42).is_none(),
        "the engine must not keep asserting an order the gateway says is not there",
    );
    assert!(
        shared.orders.get_order_info(42).is_none(),
        "and the cache row goes with it",
    );
    // The rejection itself is the report. A synthetic status update queued
    // here would reach the caller behind a fill that raced it, because both
    // dispatchers drain fills ahead of order updates.
    assert!(shared.orders.drain_order_updates().is_empty());
    assert_eq!(shared.orders.drain_cancel_rejects().len(), 1);
}

/// A fill that raced the rejection is recoverable, on the terms the
/// untracked-fill path sets: the execution has to carry its
/// contract id, because nothing else says which instrument moved, and it
/// must not be resend-marked, because a replayed execution for an order
/// this session does not track is history rather than news. An execution
/// that carries neither is dropped — the same as it was before this change
/// for any order already removed from the book.
#[test]
fn an_execution_racing_an_unknown_order_rejection_still_books() {
    let mut ccp = CcpState::new();
    let mut context = Context::new();
    let shared = SharedState::new();
    tracked_for_cancel(&mut context);

    ccp.handle_cancel_reject(&cancel_reject_frame("1"), &mut context, &shared, &None);
    let frame = exec_report_frame(&[
        (39, "1"), (17, "e-1"), (150, "F"), (32, "40"), (31, "100.0"), (151, "60"),
        (6008, "756733"), (38, "100"), (54, "1"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    assert_eq!(shared.orders.drain_fills().len(), 1, "the fill books");
    assert_eq!(context.position(0), 40.0, "and the position moves");
}

/// Only a stated UnknownOrder retires the order. Every other stated reason
/// means it is still working and the cancel arrived at the wrong moment; an
/// absent or unparseable tag 102 states nothing at all and is synthesized
/// as -1, so it takes the same path rather than retiring on an absence.
#[test]
fn any_other_rejection_leaves_the_order_in_place() {
    for code in ["0", "2", "-1", ""] {
        let mut ccp = CcpState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        tracked_for_cancel(&mut context);

        ccp.handle_cancel_reject(&cancel_reject_frame(code), &mut context, &shared, &None);

        assert_eq!(
            context.order(42).expect("still tracked").status,
            crate::types::OrderStatus::Submitted,
            "reason {code:?} does not say the order is gone",
        );
    }
}

fn exec_report_frame(pairs: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
    let mut m = std::collections::HashMap::new();
    m.insert(11u32, "42".to_string()); // ClOrdID
    for (tag, val) in pairs {
        m.insert(*tag, val.to_string());
    }
    m
}

#[test]
fn ord_status_new_unrouted_is_presubmitted() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    // 39=0, no ExDestination, exec ref "NONE" — waiting, not yet routed.
    let frame = exec_report_frame(&[(39, "0"), (150, "0"), (198, "NONE")]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    assert_eq!(context.order(42).unwrap().status,
        crate::types::OrderStatus::PreSubmitted);
}

#[test]
fn ord_status_new_routed_is_submitted() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    // 39=0 with the order routed to ARCA — working.
    let frame = exec_report_frame(&[(39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1")]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    assert_eq!(context.order(42).unwrap().status,
        crate::types::OrderStatus::Submitted);
}

#[test]
fn ord_status_presubmitted_then_routed_advances_to_submitted() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let waiting = exec_report_frame(&[(39, "0"), (150, "0"), (198, "NONE")]);
    ccp.handle_exec_report(&waiting, b"", &mut context, &shared, &None, "");
    assert_eq!(context.order(42).unwrap().status,
        crate::types::OrderStatus::PreSubmitted);
    let routed = exec_report_frame(&[(39, "0"), (150, "0"), (100, "ARCA"), (198, "ARCA:1")]);
    ccp.handle_exec_report(&routed, b"", &mut context, &shared, &None, "");
    assert_eq!(context.order(42).unwrap().status,
        crate::types::OrderStatus::Submitted);
}

// 39=I (Inactive) and 39=8 (Rejected) both stringify to
// "Inactive" downstream (types::order_status::order_status_str), but must not be
// treated the same here. A parked (39=I) order's reason is queued for
// delivery through Wrapper::error, and its completed_status stays empty
// (it is not completed and may reactivate). A rejected order's reason
// stays on the order snapshot, and nothing is queued for it — the
// engine still holds the order at this point, so context still knows it
// as Inactive/reactivatable while a Rejected order is retired below.
/// The report that fills an order states its new status on the same
/// report. Announcing the execution and withholding the status left a
/// caller watching order status believing the order was still working,
/// which is the one thing it most needed not to believe.
#[test]
fn a_report_that_fills_an_order_also_says_the_order_is_filled() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    // 39=2 filled, 150=F the execution, with a quantity and a price on it.
    let frame = exec_report_frame(&[
        (39, "2"), (150, "F"), (32, "100"), (31, "150.00"), (14, "100"), (151, "0"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &Some(crate::engine::hot_loop::EventSink::new(tx, Default::default())), "");

    let events: Vec<_> = rx.try_iter().collect();
    assert!(
        events.iter().any(|e| matches!(e, Event::Fill(_))),
        "the execution is reported: {events:?}",
    );
    assert!(
        events.iter().any(|e| matches!(
            e, Event::OrderUpdate(u) if u.status == crate::types::OrderStatus::Filled
        )),
        "and so is the status it left the order in: {events:?}",
    );
}

#[test]
fn ord_status_inactive_reason_reaches_inactive_queue() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "I"), (150, "0"),
        (58, "Order held pending margin check"), (103, "0"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    assert_eq!(context.order(42).unwrap().status, crate::types::OrderStatus::Inactive);

    let inactive = shared.orders.drain_order_inactive();
    assert_eq!(inactive.len(), 1);
    assert_eq!(inactive[0].0, 42);
    assert_eq!(inactive[0].2, "Order held pending margin check (reason code 0)");

    let info = shared.orders.get_order_info(42).unwrap();
    assert!(info.order_state.completed_status.is_empty());
}

#[test]
fn a_refused_order_tells_the_caller_why() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "8"), (150, "0"),
        (58, "No valid bid/ask"), (103, "1"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    // Rejected is terminal — the engine retires the order.
    assert!(context.order(42).is_none());

    // The venue said why. A caller that has to read a log to find out is a
    // caller that cannot act on it, so the reason goes out on the channel a
    // refusal is reported on.
    let reported = shared.orders.drain_order_inactive();
    assert_eq!(reported.len(), 1, "the refusal reaches the caller: {reported:?}");
    assert_eq!(reported[0].0, 42);
    assert!(reported[0].2.contains("No valid bid/ask"), "and says why: {reported:?}");

    // It stays on the order's own record too, which is where a caller that
    // asks after the fact looks.
    let info = shared.orders.get_order_info(42).unwrap();
    assert_eq!(info.order_state.completed_status, "No valid bid/ask");
}

/// `completed_status` carries the reject text alone, so a caller reading it
/// cannot tell a venue refusing an order type from a malformed request when
/// the text is generic. The reason code (tag 103) is what separates them.
#[test]
fn ord_status_rejected_records_the_reason_with_its_code() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "8"), (150, "0"),
        (58, "No valid bid/ask"), (103, "1"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let info = shared.orders.get_order_info(42).unwrap();
    assert_eq!(info.order_state.reject_reason, "No valid bid/ask (reason code 1)");
}

// /: in the UP portfolio snapshot the average cost is
// tag 6101 and 6065 is the market price. The handler previously read 6065 as
// the average cost. Verify the mapping and that all marks are stored.
#[test]
fn position_update_maps_marks_and_avg_cost_from_correct_tags() {
    let mut context = Context::new();
    let shared = SharedState::new();
    let mut m = std::collections::HashMap::new();
    m.insert(6008u32, "756733".to_string());   // conId
    m.insert(6064u32, "10".to_string());        // position
    m.insert(6101u32, "100.50".to_string());    // averageCost
    m.insert(6065u32, "110.25".to_string());    // marketPrice
    m.insert(6067u32, "1102.50".to_string());   // marketValue
    m.insert(6100u32, "97.50".to_string());     // unrealizedPNL
    m.insert(6099u32, "5.00".to_string());      // realizedPNL
    positions::handle_position_update(&m, &mut context, &shared, &None);

    let pi = shared.portfolio.position_info(756733).expect("position stored");
    assert_eq!(pi.position, 10.0);
    assert_eq!(pi.avg_cost, (100.50 * PRICE_SCALE as f64) as Price);
    assert_eq!(pi.market_price, (110.25 * PRICE_SCALE as f64) as Price);
    assert_eq!(pi.market_value, (1102.50 * PRICE_SCALE as f64) as Price);
    assert_eq!(pi.unrealized_pnl, (97.50 * PRICE_SCALE as f64) as Price);
    assert_eq!(pi.realized_pnl, (5.00 * PRICE_SCALE as f64) as Price);
}

// The lean position feed carries no marks; it must not zero the marks the
// portfolio snapshot set.
#[test]
fn lean_position_feed_does_not_clobber_marks() {
    let shared = SharedState::new();
    shared.portfolio.set_position_info(PositionInfo {
        con_id: 1, position: 10.0, avg_cost: 100 * PRICE_SCALE, ..Default::default()
    });
    shared.portfolio.set_position_marks(1, 110 * PRICE_SCALE, 1100 * PRICE_SCALE, 100 * PRICE_SCALE, 5 * PRICE_SCALE);
    // Lean feed updates position + avg_cost only.
    shared.portfolio.set_position_info(PositionInfo {
        con_id: 1, position: 12.0, avg_cost: 101 * PRICE_SCALE, ..Default::default()
    });
    let pi = shared.portfolio.position_info(1).unwrap();
    assert_eq!(pi.position, 12.0);
    assert_eq!(pi.avg_cost, 101 * PRICE_SCALE);
    assert_eq!(pi.market_price, 110 * PRICE_SCALE, "marks survive the lean feed");
    assert_eq!(pi.market_value, 1100 * PRICE_SCALE);
    assert_eq!(pi.unrealized_pnl, 100 * PRICE_SCALE);
}

// The TIF decoder must be the exact inverse of the outbound
// encoder. The old map decoded '7' (never emitted) as OPG and dropped
// OPG and AUC to "".
#[test]
fn tif_round_trips_through_encoder_and_decoder() {
    for tif in ["DAY", "GTC", "OPG", "IOC", "FOK", "GTD", "GTX", "AUC"] {
        let order = api::Order { tif: tif.to_string(), ..Default::default() };
        assert_eq!(decode_tif(order.tif_byte()), tif,
            "TIF {tif} must survive encode->decode");
    }
    // DTC shares the GTD wire byte and decodes as GTD: the counterpart names
    // the two differently but tag 59 does not carry the difference.
    let dtc = api::Order { tif: "DTC".to_string(), ..Default::default() };
    assert_eq!(decode_tif(dtc.tif_byte()), "GTD");
    // Unknown bytes decode to empty, not a wrong TIF.
    assert_eq!(decode_tif(b'7'), "");
}

// ── contract-details deadline sweep ──

#[test]
fn sweep_times_out_pending_secdef_with_error_and_end() {
    let mut ccp = CcpState::new();
    let shared = SharedState::new();
    let past = Instant::now() - std::time::Duration::from_secs(1);
    ccp.pending_secdef.push((7, true, past));

    ccp.sweep_contract_details(&shared, &None);

    assert!(ccp.pending_secdef.is_empty(), "expired entry must be reclaimed");
    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 7);
    assert_eq!(errors[0].1, 200);
    assert_eq!(shared.reference.drain_contract_details_end(), vec![7],
        "end must fire so a blocked wait unblocks");
}

#[test]
fn sweep_drops_internal_secdef_silently() {
    let mut ccp = CcpState::new();
    let shared = SharedState::new();
    let past = Instant::now() - std::time::Duration::from_secs(1);
    // Internal sentinel (cache auto-fetch): no user is waiting on it.
    ccp.pending_secdef.push((0xF000_0001, true, past));

    ccp.sweep_contract_details(&shared, &None);

    assert!(ccp.pending_secdef.is_empty());
    assert!(shared.reference.drain_historical_errors().is_empty());
    assert!(shared.reference.drain_contract_details_end().is_empty());
}

#[test]
fn sweep_times_out_incomplete_fanout() {
    let mut ccp = CcpState::new();
    let shared = SharedState::new();
    ccp.pending_fanout.push(PendingFanout {
        api_req_id: 9,
        fanout_req_ids: (0..27).map(|i| format!("ibxfan-9-{i}")).collect(),
        // one leg never answered — previously hung forever
        answered: (0..26).map(|i| format!("ibxfan-9-{i}")).collect(),
        deadline: Instant::now() - std::time::Duration::from_secs(1),
    });

    ccp.sweep_contract_details(&shared, &None);

    assert!(ccp.pending_fanout.is_empty());
    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 9);
    assert_eq!(errors[0].1, 200);
    assert_eq!(shared.reference.drain_contract_details_end(), vec![9]);
}

// ── a con_id=0 secdef reply is "not found", not a contract ──

/// The gateway's "no security definition" answer: a `35=d` echoing the
/// request id and carrying con_id 0 — no symbol, no price-increment block.
fn secdef_not_found(req_id: &str) -> Vec<u8> {
    crate::protocol::fix::fix_build(&[
        (fix::TAG_MSG_TYPE, "d"),
        (crate::control::contracts::TAG_SECURITY_REQ_ID, req_id),
        (crate::control::contracts::TAG_SECURITY_RESPONSE_TYPE, "4"),
        (crate::control::contracts::TAG_IB_CON_ID, "0"),
    ], 1)
}

/// A request carrying a contract rather than an id waits for the venue to
/// name it, and goes out once it has. Sent as it stood, it went out under
/// id zero and the venue answered a complete series with nothing in it.
#[test]
fn a_request_naming_a_contract_waits_to_be_given_its_id() {
    let (mut ccp, _context, shared) = u186_test_state();
    let bars = crate::types::ControlCommand::FetchHistorical {
        contract: crate::types::ContractRef { con_id: 0, symbol: "SPY".into(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "USD".into(), ..Default::default() },
        req_id: 7,
        end_date_time: String::new(),
        duration: "2 D".into(),
        bar_size: "1 hour".into(),
            what_to_show: "TRADES".into(),
        use_rth: true,
        keep_up_to_date: false,
        include_expired: false,
        filters: Default::default(),
    };

    assert!(
        ccp.hold_until_named(bars, &mut None, &mut HeartbeatState::new()).is_none(),
        "held rather than sent under no id",
    );
    assert_eq!(ccp.pending_named.len(), 1);

    // The venue names it.
    let lookup = ccp.pending_named[0].0;
    let (_, mut held, _) = ccp.pending_named.remove(0);
    name_the_contract(&mut held, 756_733);
    let _ = lookup;
    match ccp.hold_until_named(held, &mut None, &mut HeartbeatState::new()) {
        Some(crate::types::ControlCommand::FetchHistorical { contract: crate::types::ContractRef { con_id, .. }, req_id, .. }) => {
            assert_eq!((req_id, con_id), (7, 756_733), "sent under the id it was given");
        }
        other => panic!("a named request is handled, not held again: {other:?}"),
    }

    // And one the venue never names is reported rather than left waiting.
    let unnamed = crate::types::ControlCommand::FetchHistorical { contract: crate::types::ContractRef { con_id: 0, symbol: "NOSUCH".into(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "USD".into(), ..Default::default() }, end_date_time: String::new(), req_id: 8, duration: "1 D".into(), bar_size: "1 hour".into(), what_to_show: "TRADES".into(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() };
    ccp.hold_until_named(unnamed, &mut None, &mut HeartbeatState::new());
    ccp.pending_named[0].2 -= CcpState::NAMING_TIMEOUT + Duration::from_secs(1);
    ccp.sweep_pending_named(&shared);
    assert!(ccp.pending_named.is_empty());
    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 8);
}

/// A lookup the venue never answers must not leave the subscription
/// waiting in silence. That is the failure this whole path exists to
/// remove, and it would have reappeared one level down.
#[test]
fn a_subscription_the_venue_never_names_is_reported() {
    let (mut ccp, _context, shared) = u186_test_state();
    let parked = PendingSubscribe {
        instrument: 4,
        symbol: "NOSUCH".into(),
        exchange: "SMART".into(),
        sec_type: "STK".into(),
        currency: "USD".into(),
        last_trade_date: String::new(),
        strike: 0.0,
        right: String::new(),
        multiplier: String::new(),
        mode_9887: 0, regulatory_snapshot: false,
    };
    ccp.resolve_for_subscribe(parked, &mut None, &mut HeartbeatState::new());

    ccp.sweep_pending_subscribes(&shared);
    assert_eq!(ccp.pending_md_subscribe.len(), 1, "still within its wait");
    assert!(shared.market.drain_subscription_failures().is_empty());

    // Wind the clock back past the wait.
    let asked_at = &mut ccp.pending_md_subscribe[0].2;
    *asked_at -= CcpState::NAMING_TIMEOUT + Duration::from_secs(1);
    ccp.sweep_pending_subscribes(&shared);

    assert!(ccp.pending_md_subscribe.is_empty(), "given up on");
    let failures = shared.market.drain_subscription_failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0, 4, "reported against the slot that asked");
    assert!(failures[0].1.contains("NOSUCH"), "and names it: {}", failures[0].1);
}

/// The venue answers a market data subscription only when it is named by
/// contract id, so a subscription for a contract named by symbol waits on
/// the lookup that names it. It is held until the definition arrives, and
/// released with the id the definition carried.
#[test]
fn a_subscription_waits_for_the_lookup_that_names_its_contract() {
    let (mut ccp, mut context, shared) = u186_test_state();
    let parked = PendingSubscribe {
        instrument: 3,
        symbol: "SPY".into(),
        exchange: "SMART".into(),
        sec_type: "STK".into(),
        currency: "USD".into(),
        last_trade_date: String::new(),
        strike: 0.0,
        right: String::new(),
        multiplier: String::new(),
        mode_9887: 0, regulatory_snapshot: false,
    };
    ccp.resolve_for_subscribe(parked, &mut None, &mut HeartbeatState::new());
    let req_id = ccp.pending_md_subscribe[0].0;
    assert!(req_id >= 0xF000_0000, "asked for on the engine's own account, not a caller's");
    assert!(ccp.resolved_md_subscribe.is_empty(), "nothing to send until it is named");

    let named = crate::protocol::fix::fix_build(&[
        (fix::TAG_MSG_TYPE, "d"),
        (crate::control::contracts::TAG_SECURITY_REQ_ID, &req_id.to_string()),
        (crate::control::contracts::TAG_SECURITY_RESPONSE_TYPE, "4"),
        (55, "SPY"),
        (crate::control::contracts::TAG_IB_CON_ID, "756733"),
    ], 1);
    ccp.process_ccp_message(&named, &mut None, &mut context, &shared,
        &None, &mut HeartbeatState::new(), "DU1");

    assert!(ccp.pending_md_subscribe.is_empty(), "no longer waiting");
    assert_eq!(ccp.resolved_md_subscribe.len(), 1);
    let (con_id, released) = &ccp.resolved_md_subscribe[0];
    assert_eq!(*con_id, 756733, "the id the venue gave it");
    assert_eq!(released.instrument, 3, "for the slot that asked");
    assert_eq!(released.symbol, "SPY");
}

#[test]
fn secdef_not_found_by_symbol_is_an_error_not_a_row() {
    let (mut ccp, mut context, shared) = u186_test_state();
    ccp.pending_secdef.push((7, false, Instant::now() + SECDEF_TIMEOUT));

    ccp.process_ccp_message(&secdef_not_found("7"), &mut None, &mut context, &shared,
        &None, &mut HeartbeatState::new(), "DU1");

    assert!(shared.reference.drain_contract_details().is_empty(),
        "con_id=0 is the gateway saying 'no definition' — emitting it as a row \
         hands the caller a fabricated min_tick that reads like a hit");
    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 7);
    assert_eq!(errors[0].1, 200);
    assert_eq!(shared.reference.drain_contract_details_end(), vec![7],
        "end must still fire so a blocked wait unblocks");
}

#[test]
fn secdef_not_found_by_conid_errors_and_ends() {
    let (mut ccp, mut context, shared) = u186_test_state();
    // Known-conId lookup: single record, is_last regardless of the wire flag.
    ccp.pending_secdef.push((7, true, Instant::now() + SECDEF_TIMEOUT));

    ccp.process_ccp_message(&secdef_not_found("7"), &mut None, &mut context, &shared,
        &None, &mut HeartbeatState::new(), "DU1");

    assert!(shared.reference.drain_contract_details().is_empty());
    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 7);
    assert_eq!(errors[0].1, 200);
    assert_eq!(shared.reference.drain_contract_details_end(), vec![7]);
    assert!(ccp.pending_secdef.is_empty(), "the request is finished");
}

#[test]
fn secdef_not_found_stays_silent_for_an_internal_fetch() {
    let (mut ccp, mut context, shared) = u186_test_state();
    // Cache auto-fetch sentinel: no user is waiting on it.
    ccp.pending_secdef.push((0xF000_0001, true, Instant::now() + SECDEF_TIMEOUT));

    ccp.process_ccp_message(&secdef_not_found("4026531841"), &mut None, &mut context,
        &shared, &None, &mut HeartbeatState::new(), "DU1");

    assert!(shared.reference.drain_contract_details().is_empty());
    assert!(shared.reference.drain_historical_errors().is_empty());
    assert!(shared.reference.drain_contract_details_end().is_empty());
}

#[test]
fn a_fanout_reply_without_a_con_id_is_not_a_row() {
    let (mut ccp, mut context, shared) = u186_test_state();
    ccp.pending_fanout.push(PendingFanout {
        api_req_id: 9,
        fanout_req_ids: vec!["ibxfan-9-0".to_string()],
        answered: Vec::new(),
        deadline: Instant::now() + SECDEF_TIMEOUT,
    });

    ccp.process_ccp_message(&secdef_not_found("ibxfan-9-0"), &mut None, &mut context,
        &shared, &None, &mut HeartbeatState::new(), "DU1");

    assert!(shared.reference.drain_contract_details().is_empty(),
        "a per-exchange leg with no con_id is not a contract either");
        assert_eq!(shared.reference.drain_contract_details_end(), vec![9],
        "the fan-out still completes");
    assert!(ccp.pending_fanout.is_empty());
}

// ── matching-symbols attribution ──

fn matching_symbols_msg(req_id: &str, symbols: &[(&str, &str)]) -> Vec<u8> {
    let count = symbols.len().to_string();
    let mut fields: Vec<(u32, &str)> = vec![
        (crate::protocol::fix::TAG_MSG_TYPE, "U"),
        (6040, "186"),
        (320, req_id),
        (146, &count), // match count — marks a data frame (even when 0)
    ];
    for (sym, con_id) in symbols {
        fields.push((55, sym));
        fields.push((167, "CS"));
        fields.push((15, "USD"));
        fields.push((6008, con_id));
    }
    crate::protocol::fix::fix_build(&fields, 1)
}

/// A 186 frame with no match-count tag: the not-ready ack that precedes
/// the data frame.
fn matching_symbols_ack(req_id: &str) -> Vec<u8> {
    crate::protocol::fix::fix_build(&[
        (crate::protocol::fix::TAG_MSG_TYPE, "U"),
        (6040, "186"),
        (320, req_id),
    ], 1)
}

fn u186_test_state() -> (CcpState, Context, SharedState) {
    (CcpState::new(), Context::new(), SharedState::new())
}

/// Every deadline the engine keeps for a caller's request has to expire
/// before the caller stops waiting, or the caller is told nothing arrived
/// while the reason is still held here and reported to nobody.
#[test]
fn the_engine_answers_before_a_caller_gives_up() {
    let caller = Duration::from_secs(crate::config::ANSWER_TIMEOUT_SECS);
    assert!(
        SECDEF_TIMEOUT < CcpState::NAMING_TIMEOUT,
        "a lookup's own answer is preferred to the fallback that covers it",
    );
    assert!(
        CcpState::NAMING_TIMEOUT < caller,
        "a held request is reported before the caller stops listening",
    );
    assert!(SECDEF_TIMEOUT < caller);
}

/// A lookup the venue never answers has to end anyway. A caller that
/// asked through a library holding a future is waiting on the end of this
/// request, and a request that simply stops existing leaves it waiting for
/// as long as the program runs.
#[test]
fn a_lookup_the_venue_never_answers_is_ended_rather_than_left() {
    let (mut ccp, _context, shared) = u186_test_state();
    // Asked for by the venue's id for the contract, which is the shape
    // a caller uses when it names nothing else.
    ccp.pending_secdef.push((4242, true, Instant::now() - Duration::from_secs(1)));

    ccp.sweep_contract_details(&shared, &None);

    let refused = shared.reference.drain_historical_errors();
    assert_eq!(refused.len(), 1, "the caller is told, once");
    assert_eq!(refused[0].0, 4242, "under the id it asked with");
    assert_eq!(
        shared.reference.drain_contract_details_end(),
        vec![4242],
        "and the request ends, which is what a waiting future is waiting for",
        );
    assert!(ccp.pending_secdef.is_empty(), "and nothing is left pending");
}

#[test]
fn matching_symbols_matched_by_echoed_req_id_not_fifo() {
    let (mut ccp, mut context, shared) = u186_test_state();
    ccp.pending_matching_symbols.push((1, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));
    ccp.pending_matching_symbols.push((2, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

    // Request 2's reply arrives FIRST (out of order).
    let msg = matching_symbols_msg("2", &[("AAPL", "265598")]);
    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

    let delivered = shared.reference.drain_matching_symbols();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, 2, "reply must land on the echoed req_id, not the queue head");
    assert_eq!(delivered[0].1.len(), 1);
    assert_eq!(ccp.pending_matching_symbols.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![1]);
}

#[test]
fn matching_symbols_empty_result_pops_and_delivers() {
    let (mut ccp, mut context, shared) = u186_test_state();
    ccp.pending_matching_symbols.push((1, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));
    ccp.pending_matching_symbols.push((2, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

    // Unknown pattern: zero matches. Must still pop req 1 and deliver
    // the empty answer — previously this poisoned the queue head and
    // every later reply was off by one, forever.
    let msg = matching_symbols_msg("1", &[]);
    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

    let delivered = shared.reference.drain_matching_symbols();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, 1);
    assert!(delivered[0].1.is_empty(), "empty result is a legitimate answer");
        assert_eq!(ccp.pending_matching_symbols.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![2],
        "queue must not be poisoned by an empty result");

    // The next reply attributes correctly.
    let msg = matching_symbols_msg("2", &[("MSFT", "272093")]);
    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");
    let delivered = shared.reference.drain_matching_symbols();
    assert_eq!(delivered[0].0, 2);
    assert!(ccp.pending_matching_symbols.is_empty());
}

#[test]
fn matching_symbols_ack_frame_does_not_consume_the_request() {
    let (mut ccp, mut context, shared) = u186_test_state();
    ccp.pending_matching_symbols.push((1, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

    // The not-ready ack (no tag 146) arrives first — it must not pop the
    // request; delivering it as an empty answer orphans the data frame
    // that follows (observed live).
    let msg = matching_symbols_ack("1");
    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");
    assert!(shared.reference.drain_matching_symbols().is_empty());
    assert_eq!(ccp.pending_matching_symbols.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![1]);

    // The data frame then delivers.
    let msg = matching_symbols_msg("1", &[("AAPL", "265598")]);
    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");
    let delivered = shared.reference.drain_matching_symbols();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, 1);
    assert_eq!(delivered[0].1.len(), 1);
    assert!(ccp.pending_matching_symbols.is_empty());
}

#[test]
fn matching_symbols_unattributable_reply_is_dropped_not_misattributed() {
    let (mut ccp, mut context, shared) = u186_test_state();
    ccp.pending_matching_symbols.push((1, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));
    ccp.pending_matching_symbols.push((2, Instant::now() + MATCHING_SYMBOLS_TIMEOUT));

    // Echoed id matches nothing pending: with two in flight, guessing
    // would cross-attribute — drop with a warn instead.
    let msg = matching_symbols_msg("99", &[("AAPL", "265598")]);
    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

    assert!(shared.reference.drain_matching_symbols().is_empty());
    assert_eq!(ccp.pending_matching_symbols.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![1, 2]);
}

#[test]
fn sweep_spares_live_entries() {
    let mut ccp = CcpState::new();
    let shared = SharedState::new();
    let future = Instant::now() + SECDEF_TIMEOUT;
    ccp.pending_secdef.push((7, true, future));
    ccp.pending_fanout.push(PendingFanout {
        api_req_id: 9,
        fanout_req_ids: vec!["ibxfan-9-0".to_string()],
        answered: Vec::new(),
        deadline: future,
    });

    ccp.sweep_contract_details(&shared, &None);

    assert_eq!(ccp.pending_secdef.len(), 1);
    assert_eq!(ccp.pending_fanout.len(), 1);
    assert!(shared.reference.drain_historical_errors().is_empty());
    assert!(shared.reference.drain_contract_details_end().is_empty());
}

/// A fill whose ClOrdID this session never tracked. Every field the engine
/// needs to book it is on the report itself.
fn untracked_fill(pairs: &[(u32, &str)]) -> std::collections::HashMap<u32, String> {
    let mut m = std::collections::HashMap::new();
    for (tag, val) in [
        (11u32, "99"),      // ClOrdID the context does not know
        (150, "2"),         // ExecType: trade
        (39, "2"),          // OrdStatus: filled
        (32, "5"),          // LastShares
        (31, "100.00"),     // LastPx
        (54, "1"),          // Side: buy
        (6008, "888888"),   // ContractID
        (55, "ZZZ"),
        (17, "EXEC-1"),
    ] {
        m.insert(tag, val.to_string());
    }
    for (tag, val) in pairs {
        if val.is_empty() {
            m.remove(tag);
        } else {
            m.insert(*tag, val.to_string());
        }
    }
    m
}

/// A fill for an order this session does not track is still a position the
/// account holds. Dropping it leaves the engine short of the truth with
/// nothing to say so — the cancel/fill race reaches this every time.
#[test]
fn a_fill_for_an_untracked_order_is_still_booked() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = untracked_fill(&[]);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let fills = shared.orders.drain_fills();
    assert_eq!(fills.len(), 1, "the fill must be reported");
    assert_eq!(fills[0].qty, 5 * QTY_SCALE);
    assert_eq!(fills[0].order_id, 99);
    assert_eq!(fills[0].side, Side::Buy);
    assert_eq!(
        context.position(fills[0].instrument), 5.0,
        "the position must move by the filled quantity",
    );
}

/// A sell books the other way. Taking the side from the report rather than
/// defaulting is the whole point: the wrong sign is worse than no fill.
#[test]
fn an_untracked_sell_moves_the_position_down() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = untracked_fill(&[(54, "2")]);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let fills = shared.orders.drain_fills();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].side, Side::Sell);
    assert_eq!(context.position(fills[0].instrument), -5.0);
}

/// Without a contract or a side there is nothing to book against, and
/// guessing either one would move a real position the wrong way.
#[test]
fn an_untracked_fill_is_not_booked_on_a_guess() {
    for missing in [6008u32, 54] {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let frame = untracked_fill(&[(missing, "")]);

        ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

        assert!(
            shared.orders.drain_fills().is_empty(),
            "tag {missing} missing: must not book a guessed fill",
        );
    }
}

/// On a fresh process the gateway resends prior executions with 97=Y and
/// their original ExecIDs, for orders no session tracks. Booking those
/// builds a position out of history on top of the one the position feed
/// already reports.
#[test]
fn a_replayed_execution_is_not_booked_as_a_new_position() {
    for (tag, name) in [(97u32, "PossResend"), (43, "PossDupFlag")] {
        let (mut ccp, mut context, shared) = ord_status_test_state();

        ccp.handle_exec_report(&untracked_fill(&[(tag, "Y")]), b"", &mut context, &shared, &None, "");

        assert!(
            shared.orders.drain_fills().is_empty(),
            "{name}=Y restates history and must not move the position",
        );
    }

    // The same report without the marker is booked, so the guard is the
    // marker and not something else about the frame.
    let (mut ccp, mut context, shared) = ord_status_test_state();
    ccp.handle_exec_report(&untracked_fill(&[(97, "N")]), b"", &mut context, &shared, &None, "");
    assert_eq!(shared.orders.drain_fills().len(), 1);
}

/// A completed order's replay carries a cumulative quantity and no local
/// record to reconcile against. The marker is what stops it, and it must
/// stop it before the cumulative figure is read.
#[test]
fn a_replay_with_a_cumulative_quantity_is_still_not_booked() {
    let (mut ccp, mut context, shared) = ord_status_test_state();

    ccp.handle_exec_report(
        &untracked_fill(&[(97, "Y"), (14, "100"), (32, "100")]),
        b"", &mut context, &shared, &None, "",
    );

    assert!(
        shared.orders.drain_fills().is_empty(),
        "a replayed history for an order this session never saw is not a fill",
    );
}

/// An execution that could not be booked must stay replayable. Consuming
/// the ExecID for a fill that was dropped makes the loss permanent: the
/// replay after a reconnect is then rejected as a duplicate.
#[test]
fn an_unbookable_fill_does_not_consume_its_exec_id() {
    let (mut ccp, mut context, shared) = ord_status_test_state();

    // Same execution, first seen without the contract that would let the
    // engine place it.
    ccp.handle_exec_report(&untracked_fill(&[(6008, "")]), b"", &mut context, &shared, &None, "");
    assert!(shared.orders.drain_fills().is_empty());

    // Replayed in full — it must not be rejected as already seen.
    ccp.handle_exec_report(&untracked_fill(&[]), b"", &mut context, &shared, &None, "");
    assert_eq!(
        shared.orders.drain_fills().len(), 1,
        "the replay must be booked, not dropped as a duplicate",
    );
}

/// A fractional order fills in fractions, and the venue states them as
/// decimals. Read as an integer, `32=0.5` parsed to nothing: the fill was
/// reported as zero shares and the position never moved, on a client that
/// accepts fractional orders.
#[test]
fn a_fractional_print_books_the_fraction_it_states() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let before = context.position(0);
    let frame = exec_report_frame(&[
        (150, "F"), (17, "EXEC-FRAC"), (100, "ARCA"), (198, "ARCA:1"),
        (32, "0.5"), (31, "101.00"), (14, "0.5"), (6, "101.00"), (151, "0.25"), (39, "1"),
    ]);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let fills = shared.orders.drain_fills();
    assert_eq!(fills.len(), 1, "the fill is reported");
    assert_eq!(fills[0].qty, QTY_SCALE / 2, "half a share books as half a share");
    assert_eq!(fills[0].cum_qty, QTY_SCALE / 2, "and the order total states the same");
    assert_eq!(fills[0].remaining, QTY_SCALE / 4, "as does what is still working");
    assert_eq!(
        context.position(0) - before, 0.5,
        "and the position moves by the fraction that filled",
    );
    assert_eq!(
        context.order(42).unwrap().filled, QTY_SCALE / 2,
        "and the order records the fraction as filled",
    );
}

/// The cumulative pair has to come off the wire. Tag 14 is the order's
/// filled total and tag 6 its volume-weighted average; 32 and 31 describe
/// only the print that triggered the report.
#[test]
fn the_fill_carries_the_orders_totals_not_the_prints() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    // Second print of 5 at 101, taking the order to 12 filled at 100.50.
    let frame = untracked_fill(&[
        (32, "5"), (31, "101.00"), (14, "12"), (6, "100.50"), (151, "3"), (39, "1"),
    ]);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let fills = shared.orders.drain_fills();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].qty, 5 * QTY_SCALE, "qty stays the print");
    assert_eq!(fills[0].price, 101 * PRICE_SCALE, "price stays the print");
    assert_eq!(fills[0].cum_qty, 12 * QTY_SCALE, "cum_qty is the order total from tag 14");
    assert_eq!(
        fills[0].avg_price, 100 * PRICE_SCALE + PRICE_SCALE / 2,
        "avg_price is the volume-weighted average from tag 6",
    );
}

/// Without tag 14 the print alone is not a substitute: on a later fill it
/// is smaller than what was already reported, so `filled` would go
/// backwards. The order's own accumulated quantity carries it instead.
#[test]
fn a_missing_cumulative_quantity_does_not_walk_backwards() {
    let (mut ccp, mut context, shared) = ord_status_test_state();

    // Seven filled so far, stated.
    ccp.handle_exec_report(
        &exec_report_frame(&[
            (150, "2"), (39, "1"), (32, "7"), (31, "100.00"), (14, "7"), (6, "100.00"),
            (151, "3"), (17, "E1"),
        ]), b"",
        &mut context, &shared, &None, "",
    );
    let first = shared.orders.drain_fills();
    assert_eq!(first[0].cum_qty, 7 * QTY_SCALE);

    // One more, with the cumulative fields absent.
    ccp.handle_exec_report(
        &exec_report_frame(&[
            (150, "2"), (39, "1"), (32, "1"), (31, "101.00"), (151, "2"), (17, "E2"),
        ]), b"",
        &mut context, &shared, &None, "",
    );
    let second = shared.orders.drain_fills();
    assert_eq!(
        second[0].cum_qty, 8 * QTY_SCALE,
        "the order's own total carries it, rather than dropping back to the print",
    );
}

/// A negative average price is a real value for a spread quoted as a net
/// credit, so only an absent or unparseable tag falls back.
#[test]
fn a_negative_average_price_is_not_treated_as_absent() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = untracked_fill(&[(32, "5"), (31, "-2.00"), (14, "5"), (6, "-1.50")]);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let fills = shared.orders.drain_fills();
    assert_eq!(fills[0].avg_price, -(PRICE_SCALE + PRICE_SCALE / 2), "-1.50 is kept");
}

/// With no order to accumulate against and no tags, the print is all there
/// is — which is what the callback reported before.
#[test]
fn the_fill_falls_back_to_the_print_when_the_totals_are_absent() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = untracked_fill(&[(32, "5"), (31, "101.00"), (14, ""), (6, "")]);

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let fills = shared.orders.drain_fills();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].cum_qty, 5 * QTY_SCALE);
    assert_eq!(fills[0].avg_price, 101 * PRICE_SCALE);
}

/// The side mapping is the whole sign of the position delta, so every arm
/// is pinned — a short sale booked as an ordinary sell is the same
/// direction, but a buy booked as a sell is twice the fill in the wrong one.
#[test]
fn every_side_maps_to_the_right_position_delta() {
    for (tag54, expected_side, expected_delta) in [
        ("1", Side::Buy, 5),
        ("2", Side::Sell, -5),
        ("5", Side::ShortSell, -5),
    ] {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        ccp.handle_exec_report(
            &untracked_fill(&[(54, tag54)]), b"", &mut context, &shared, &None, "",
        );
        let fills = shared.orders.drain_fills();
        assert_eq!(fills.len(), 1, "Side={tag54} books");
        assert_eq!(fills[0].side, expected_side, "Side={tag54}");
        assert_eq!(
            context.position(fills[0].instrument), expected_delta as f64,
            "Side={tag54} moves the position {expected_delta}",
        );
    }
}

/// Deduplication exists to stop a fill being counted twice. Returning out
/// of the whole handler also skips the status and the terminal bookkeeping,
/// so a replayed final fill leaves the order in `open_orders` for good and
/// `req_open_orders` keeps reporting a filled order as working.
#[test]
fn a_duplicate_exec_id_suppresses_the_fill_and_nothing_else() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let event_tx = Some(crate::engine::hot_loop::EventSink::new(event_tx, Default::default()));

    // Partial fill, booked normally.
    ccp.handle_exec_report(
        &exec_report_frame(&[
            (150, "2"), (39, "1"), (32, "1"), (31, "100.00"), (151, "9"), (17, "DUP-1"),
        ]), b"",
        &mut context, &shared, &event_tx, "",
    );
    assert_eq!(shared.orders.drain_fills().len(), 1, "the first delivery books");
    assert!(context.order(42).is_some(), "and the order is still working");

    // The same execution replayed, this time carrying the terminal status.
    ccp.handle_exec_report(
        &exec_report_frame(&[
            (150, "2"), (39, "2"), (32, "1"), (31, "100.00"), (151, "0"), (17, "DUP-1"),
        ]), b"",
        &mut context, &shared, &event_tx, "",
    );

    assert!(
        shared.orders.drain_fills().is_empty(),
        "the fill is not counted twice",
    );
    let position_after = context.position(0);
    assert!(
        context.order(42).is_none(),
        "but the order still reaches its terminal state and is removed",
    );
    let completed = shared.orders.drain_completed_orders();
    assert_eq!(completed.len(), 1, "and is reported completed");
    assert_eq!(completed[0].order_id, 42);
    assert_eq!(completed[0].status, crate::types::OrderStatus::Filled);

    // The terminal status still reaches the application. Treating the
    // duplicate as though it had booked a fill would swallow it, since the
    // status notification is suppressed when a fill was reported instead.
    let updates = shared.orders.drain_order_updates();
    assert_eq!(updates.len(), 1, "exactly one status notification, not none and not two");
    assert_eq!(updates[0].order_id, 42);
    assert_eq!(updates[0].status, crate::types::OrderStatus::Filled);

    // The position is what deduplication exists to protect. One share was
    // filled; the replay must not make it two.
    assert_eq!(position_after, 1.0, "the duplicate must not move the position again");
    assert_eq!(updates[0].filled_qty, 1.0, "nor inflate the filled quantity");
    assert_eq!(completed[0].filled_qty, QTY_SCALE);

    // The event channel is a second delivery path for the same fill, and
    // every other test here passes None for it, so it is checked once.
    let events: Vec<_> = event_rx.try_iter().collect();
    assert_eq!(
        events.iter().filter(|e| matches!(e, Event::Fill(_))).count(), 1,
        "exactly one Fill reaches the channel across both deliveries: {events:?}",
    );
}

/// The report restates the order, and a caller asking what its orders are
/// is answered from that. An order that came back naming neither the
/// reference the caller gave it nor the client that placed it is not the
/// order they placed.
#[test]
fn the_order_a_report_restates_carries_what_the_caller_gave_it() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let mut frame = exec_report_frame(&[
        (150, "2"), (39, "2"), (32, "1"), (31, "100.00"), (151, "0"), (17, "E1"),
        (6008, "756733"), (55, "SPY"),
    ]);
    frame.insert(6010, "my-strategy".to_string());
    frame.insert(47, "A".to_string());
    frame.insert(432, "20260401-16:00:00".to_string());
    frame.insert(109, "7".to_string());
    frame.insert(6160, "GROUP1".to_string());
    frame.insert(6159, "PctChange".to_string());
    frame.insert(6164, "25".to_string());

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let info = shared.orders.get_order_info(42).expect("the order is recorded");
    assert_eq!(info.order.order_ref, "my-strategy", "the caller's own name for it");
    assert_eq!(info.order.rule80a, "A");
    assert_eq!(info.order.good_till_date, "20260401-16:00:00");
    assert_eq!(info.order.client_id, 7);
    assert_eq!(info.order.fa_group, "GROUP1");
    assert_eq!(info.order.fa_method, "PctChange");
    assert_eq!(info.order.fa_percentage, "25");
}

/// A broker liquidating a position says so by naming the order with a
/// leading L, not by setting a field. Read as a field it was never set, and
/// a caller could not tell a forced liquidation from any other fill.
#[test]
fn a_liquidation_is_told_apart_from_an_ordinary_fill() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let mut frame = exec_report_frame(&[
        (150, "2"), (39, "2"), (32, "1"), (31, "100.00"), (151, "0"), (17, "E1"),
        (6008, "756733"), (55, "SPY"),
    ]);
    frame.insert(11, "L42".to_string());
    frame.insert(6858, "AVG_LEG_CLOSE_DIFF".to_string());
    frame.insert(6859, "2.5".to_string());
    frame.insert(8497, "1".to_string());

    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let info = shared.orders.get_order_info(42).expect("the order is recorded");
    assert_eq!(info.last_exec.liquidation, 1, "the broker liquidated this");
    assert_eq!(info.last_exec.ev_rule, "AVG_LEG_CLOSE_DIFF");
    assert_eq!(info.last_exec.ev_multiplier, 2.5, "the multiplier is the number beside the rule");
    // Read off the text tag, it parsed to nothing and every fill carried a
    // multiplier of zero.
    assert_ne!(info.last_exec.ev_multiplier, 0.0);
    assert!(info.last_exec.pending_price_revision, "the price may still be revised");
}

/// An ordinary fill is not a liquidation, and states none of the rest.
#[test]
fn an_ordinary_fill_claims_none_of_it() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (150, "2"), (39, "2"), (32, "1"), (31, "100.00"), (151, "0"), (17, "E1"),
        (6008, "756733"), (55, "SPY"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");

    let info = shared.orders.get_order_info(42).expect("the order is recorded");
    assert_eq!(info.last_exec.liquidation, 0);
    assert!(info.last_exec.ev_rule.is_empty());
    assert!(!info.last_exec.pending_price_revision);
}

/// A late duplicate of an earlier partial must not put a finished order
/// back on the open list. The cache is what `req_open_orders` reads.
#[test]
fn a_late_partial_does_not_reopen_a_completed_order() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let partial = |exec: &str| exec_report_frame(&[
        (150, "2"), (39, "1"), (32, "1"), (31, "100.00"), (151, "9"), (17, exec),
        (6008, "756733"), (55, "SPY"),
    ]);

    ccp.handle_exec_report(&partial("E1"), b"", &mut context, &shared, &None, "");
    ccp.handle_exec_report(
        &exec_report_frame(&[
            (150, "2"), (39, "2"), (32, "9"), (31, "100.00"), (151, "0"), (17, "E2"),
            (6008, "756733"), (55, "SPY"),
        ]), b"",
        &mut context, &shared, &None, "",
    );
    let terminal = shared.orders.get_order_info(42).map(|i| i.order_state.status.clone());

    // The earlier partial arrives again.
    ccp.handle_exec_report(&partial("E1"), b"", &mut context, &shared, &None, "");

    assert_eq!(
        shared.orders.get_order_info(42).map(|i| i.order_state.status.clone()),
        terminal,
        "the completed order stays completed",
    );
}

// A fill with no ExecID is deduped on its content instead, and the key
// includes CumQty, which advances with every execution on an order. Two
// real fills therefore never collide. The case that asserted the opposite
// sent one frame twice with no CumQty tag at all, so both read as zero: a
// shape the gateway does not produce, and treating it as two fills would
// give back the replay double-booking the content key exists to stop.

/// The gateway's answer to a symbol it cannot resolve: a `35=d` echoing
/// the request id with no contract fields (live: "BRK.A").
fn secdef_no_match(req_id: &str, response_type: &str) -> Vec<u8> {
    crate::protocol::fix::fix_build(&[
        (crate::protocol::fix::TAG_MSG_TYPE, "d"),
        (crate::control::contracts::TAG_SECURITY_REQ_ID, req_id),
        (crate::control::contracts::TAG_SECURITY_RESPONSE_TYPE, response_type),
    ], 1)
}

#[test]
fn secdef_no_match_reports_error_200_not_a_zeroed_row() {
    let (mut ccp, mut context, shared) = u186_test_state();
    // By-symbol lookup: not single-shot.
    ccp.pending_secdef.push((1005, false, Instant::now() + SECDEF_TIMEOUT));

    let msg = secdef_no_match("1005", "6");
    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

    assert!(shared.reference.drain_contract_details().is_empty(),
        "a contract-less reply must not surface as a ContractDetails row");
    let errors = shared.reference.drain_historical_errors();
    assert_eq!(errors.len(), 1, "the caller must be told the symbol did not resolve");
    assert_eq!(errors[0].0, 1005);
    assert_eq!(errors[0].1, 200);
    assert_eq!(shared.reference.drain_contract_details_end(), vec![1005]);
    assert!(ccp.pending_secdef.is_empty(), "the request must not outlive its answer");
    }

/// Same reply without the 323 terminator: the by-symbol path reached end
/// through the fan-out branch instead, and must not fire end twice.
#[test]
fn secdef_no_match_without_terminator_ends_once() {
    let (mut ccp, mut context, shared) = u186_test_state();
    ccp.pending_secdef.push((1005, false, Instant::now() + SECDEF_TIMEOUT));

    let msg = secdef_no_match("1005", "4");
    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

    assert!(shared.reference.drain_contract_details().is_empty());
    assert_eq!(shared.reference.drain_historical_errors().len(), 1);
    assert_eq!(shared.reference.drain_contract_details_end(), vec![1005]);
    assert!(ccp.pending_secdef.is_empty());
}

#[test]
fn secdef_no_match_on_internal_req_id_stays_silent() {
    let (mut ccp, mut context, shared) = u186_test_state();
    ccp.pending_secdef.push((0xF000_0001, true, Instant::now() + SECDEF_TIMEOUT));

    let msg = secdef_no_match("4026531841", "6"); // 0xF0000001
    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None, &mut HeartbeatState::new(), "DU1");

    assert!(shared.reference.drain_contract_details().is_empty());
    assert!(shared.reference.drain_historical_errors().is_empty());
    assert!(shared.reference.drain_contract_details_end().is_empty());
    assert!(ccp.pending_secdef.is_empty());
}

/// The venue's answer, verbatim from a live session. Nothing read it,
/// so a caller could not know which algorithms the account may use.
#[test]
fn the_venue_states_which_algorithms_it_offers() {
    let offered = super::parse_algorithms(
        "FOXRIVER/STK:FOXRIVER-AE,FOXRIVER-AL-COMMON;IBALGO/BAG:IBALGO-AE,IBALGO-AL-BAG;\
         IBALGO/CASH:IBALGO-AE,IBALGO-AL-CASH;IBALGO/OPT:IBALGO-AE,IBALGO-AL-OPT"
    );
    assert_eq!(offered.len(), 4, "one entry per provider and security type: {offered:?}");
    assert_eq!(offered["FOXRIVER/STK"], ["FOXRIVER-AE", "FOXRIVER-AL-COMMON"]);
    assert_eq!(offered["IBALGO/OPT"], ["IBALGO-AE", "IBALGO-AL-OPT"]);
    assert!(super::parse_algorithms("").is_empty(), "a session that offered none");
    assert!(
        super::parse_algorithms("NOCOLON").is_empty(),
        "an entry naming no algorithms states nothing",
    );
}

/// A message nobody has looked at and a message deliberately not read are
/// both discarded, but only one is a gap.
#[test]
fn a_message_not_read_on_purpose_is_told_apart_from_one_overlooked() {
    // 93 is excused on what it carries — the account, a request id and two
    // flags — read off a live session, not on an assumption about it.
    // Both of these arrive on a real session, which is how they came to be
    // named here: the notes had them down as never sent.
    assert!(super::known_unread("18").is_some(), "the clock every message already carries");
    assert!(super::known_unread("93").is_some(), "an answer carrying nothing new");
    assert!(super::known_unread("194").is_some(), "defaults for a user interface");
    assert!(super::known_unread("81").is_none(), "the algorithms are read, not excused");
    // Excused for years as a fill already stated by the execution reports.
    // The fill is, and what it cost is not: those reports carry no charge at
    // all, so this is the only place a caller's commission comes from.
    assert!(super::known_unread("60").is_none(), "what a fill cost is read, not excused");
    assert!(super::known_unread("99999").is_none(), "and anything unexamined is a gap");
}

/// The venue states trouble as text, with no code and, for all but a
/// narrow family of requests, nothing saying which request it belongs to.
#[test]
fn what_the_venue_says_went_wrong_reaches_the_caller() {
    let (_ccp, _context, shared) = u186_test_state();

    let mut said = std::collections::HashMap::new();
    said.insert(58u32, "Order rejected for margin".to_string());
    super::handle_venue_error(&said, &shared);
    assert_eq!(shared.market.drain_venue_errors(), ["Order rejected for margin"]);

    // Where it names something code-like, that travels with the text
    // rather than being read as a number it never stated.
    said.insert(149u32, "MARGIN".to_string());
    super::handle_venue_error(&said, &shared);
    assert_eq!(shared.market.drain_venue_errors(), ["Order rejected for margin (MARGIN)"]);

    // Trouble it says nothing about leaves nothing to report.
    super::handle_venue_error(&std::collections::HashMap::new(), &shared);
    assert!(shared.market.drain_venue_errors().is_empty());
}

/// The venue keeps three sets of holdings and this client read one. The
/// others carry the same fields in the same tags, so they are read the
/// same way — and kept apart, because a caller asking what the account
/// holds does not mean what it holds somewhere else.
#[test]
fn a_holding_the_account_does_not_hold_is_kept_apart() {
    let (_ccp, _context, shared) = u186_test_state();
    let mut row = std::collections::HashMap::new();
    row.insert(6008u32, "265598".to_string());
    row.insert(6068u32, "AAPL  ".to_string());
    row.insert(167u32, "STK".to_string());
    row.insert(15u32, "USD".to_string());
    row.insert(6064u32, "100".to_string());
    row.insert(6101u32, "150.0".to_string());

    super::positions::handle_position_elsewhere(&row, &shared, crate::types::HeldElsewhere::Away);
    let held = shared.portfolio.positions_elsewhere();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].con_id, 265598);
    assert_eq!(held[0].symbol, "AAPL", "the venue pads a symbol out");
    assert_eq!(held[0].position, 100.0);
    assert_eq!(held[0].avg_cost, 150 * PRICE_SCALE);
    assert_eq!(held[0].held, crate::types::HeldElsewhere::Away);

    // It stays out of what the account itself holds.
    assert!(shared.portfolio.position_infos().is_empty(), "not one of the account's own");

    // The venue restates a row rather than withdrawing it.
    row.insert(6064u32, "50".to_string());
    super::positions::handle_position_elsewhere(&row, &shared, crate::types::HeldElsewhere::DisplayOnly);
    let held = shared.portfolio.positions_elsewhere();
    assert_eq!(held.len(), 1, "restated, not added again");
    assert_eq!(held[0].position, 50.0);
    assert_eq!(held[0].held, crate::types::HeldElsewhere::DisplayOnly);

    // A row naming no contract names nothing.
    super::positions::handle_position_elsewhere(
        &std::collections::HashMap::new(), &shared, crate::types::HeldElsewhere::Away,
    );
    assert_eq!(shared.portfolio.positions_elsewhere().len(), 1);
}

/// The venue states figures for the sets of holdings the account does not
/// hold itself the same way it states the account's own. Applied to the
/// account's own they would overstate what it is worth.
#[test]
fn figures_for_other_holdings_stay_out_of_the_account() {
    let (_ccp, context, shared) = u186_test_state();
    let msg = b"8=FIX.4.1\x0135=U\x018001=NetLiquidation\x018004=12345.67\x018001=GrossPositionValue\x018004=999.00\x01";

    super::handle_account_update_elsewhere(msg, &shared, crate::types::HeldElsewhere::Away);
    let mut stated = shared.portfolio.values_elsewhere(crate::types::HeldElsewhere::Away);
    stated.sort();
    assert_eq!(stated, [
        ("GrossPositionValue".to_string(), "999.00".to_string()),
        ("NetLiquidation".to_string(), "12345.67".to_string()),
    ]);
    assert!(
        shared.portfolio.values_elsewhere(crate::types::HeldElsewhere::Aside).is_empty(),
        "one set's figures do not describe another",
        );
    assert_eq!(
        context.account().net_liquidation, 0,
        "and none of it is what the account itself is worth",
    );
}
mod unnamed_execution_tests {

    /// A report carries far more than any one client reads. What is not read
    /// is kept, so a fact the venue stated about a fill remains reachable.
    #[test]
    fn a_field_a_report_states_and_nothing_names_is_kept() {
        let frame = b"35=8\x0117=E1\x0132=100\x019997=something\x019998=42\x01";
        let kept = super::executions::unnamed_execution_fields(frame);
        let tags: Vec<u32> = kept.iter().map(|(t, _)| *t).collect();
        assert!(tags.contains(&9997));
        assert!(tags.contains(&9998));
        assert_eq!(kept.iter().find(|(t, _)| *t == 9997).unwrap().1, "something");
    }

    /// A field the handler reads is read into its own place, not left as a
    /// number, and the message's own fields are not the fill's.
    #[test]
    fn what_is_read_and_what_belongs_to_the_message_are_both_excluded() {
        let frame = b"35=8\x0117=E1\x0152=20260101-00:00:00\x01";
        let tags: Vec<u32> = super::executions::unnamed_execution_fields(frame).iter().map(|(t, _)| *t).collect();
        assert!(!tags.contains(&17), "the execution id is read");
        assert!(!tags.contains(&35), "the message type belongs to the message");
        assert!(!tags.contains(&52), "the sending time belongs to the message");
    }

    /// The handler reads a good many tags, so the derived list is not empty or
    /// tiny — which would make everything look unread.
    #[test]
    fn the_tags_the_handler_reads_are_derived_from_the_handler() {
        let read = super::executions::tags_read_from_an_execution();
        assert!(read.len() > 30, "only {} tags reported as read", read.len());
        assert!(read.contains(&17), "the execution id is read");
    }
}
mod stated_account_value_tests {
    use crate::bridge::SharedState;

    /// The venue states a great many more figures than any client names, and a
    /// figure nobody named is still a figure about the account. They are kept
    /// where they arrive rather than dropped.
    #[test]
    fn a_figure_nothing_names_is_still_kept() {
        let shared = SharedState::new();
        shared.portfolio.note_account_value("NetLiquidation", "12345.67", "USD");
        shared.portfolio.note_account_value("SomethingNobodyNames", "42", "EUR");

        let stated = shared.portfolio.stated_account_values();
        assert_eq!(stated.len(), 2);
        assert!(stated.iter().any(|(k, v, c)| k == "SomethingNobodyNames" && v == "42" && c == "EUR"));
    }

    /// The same figure in two currencies is two figures. Collapsing them would
    /// report one account's worth in a currency it is not held in.
    #[test]
    fn the_same_figure_in_two_currencies_is_two_figures() {
        let shared = SharedState::new();
        shared.portfolio.note_account_value("TotalCashValue", "100", "USD");
        shared.portfolio.note_account_value("TotalCashValue", "90", "EUR");
        assert_eq!(shared.portfolio.stated_account_values().len(), 2);
    }

    /// A figure restated in the same currency replaces the earlier statement
    /// rather than piling up beside it.
    #[test]
    fn a_figure_restated_replaces_what_it_restates() {
        let shared = SharedState::new();
        shared.portfolio.note_account_value("BuyingPower", "100", "USD");
        shared.portfolio.note_account_value("BuyingPower", "200", "USD");
        let stated = shared.portfolio.stated_account_values();
        assert_eq!(stated.len(), 1);
        assert_eq!(stated[0].1, "200");
    }
}

/// Every holding a position frame names is read, not only the last.
///
/// Captured from a session: one frame, five holdings. A flat parse keeps only
/// the last value of each tag and reports a single holding of zero.
#[test]
fn a_position_frame_names_every_holding() {
    use super::positions::split_position_entries;

    let frame = concat!(
        "35=UP\x016529=AR.1\x01",
        "6068=IWM\x016288=0\x018001=PositionList\x016064=-80\x0115=USD\x016008=9579970\x01",
        "6068=MES SEP2026\x016288=0\x018001=PositionList\x016064=1\x0115=USD\x016008=793356217\x01167=FUT\x01",
        "6068=QQQ\x016288=0\x018001=PositionList\x016064=100\x0115=USD\x016008=320227571\x01",
        "6068=SPY\x016288=0\x018001=PositionList\x016064=342\x0115=USD\x016008=756733\x01",
        "6068=VOD\x016288=0\x018001=PositionList\x016064=0\x0115=GBP\x016008=140148322\x01",
    );

    let held = split_position_entries(frame.as_bytes());
    assert_eq!(held.len(), 5, "five holdings were named, so five are read");

    let by_con_id: Vec<(i64, f64)> = held
        .iter()
        .map(|h| (
            h.get(&6008).unwrap().parse().unwrap(),
            h.get(&6064).unwrap().parse().unwrap(),
        ))
        .collect();
    assert_eq!(
        by_con_id,
        vec![(9579970, -80.0), (793356217, 1.0), (320227571, 100.0),
             (756733, 342.0), (140148322, 0.0)],
    );

    // What the frame says about itself belongs to each holding in it.
    for one in &held {
        assert_eq!(one.get(&6529).map(String::as_str), Some("AR.1"));
        assert_eq!(one.get(&35).map(String::as_str), Some("UP"));
    }

    // And the one that had just traded is present, which a flat parse lost.
    assert!(by_con_id.iter().any(|(con_id, qty)| *con_id == 756733 && *qty == 342.0));

    // A holding describes itself. Read flat, only the last holding's symbol
    // and security type survived, so every other one — a future among them —
    // reached a caller as an id and a quantity and nothing else, and looked
    // like a contract the definition service had refused to name.
    let future = held
        .iter()
        .find(|h| h.get(&6008).map(String::as_str) == Some("793356217"))
        .expect("the future is one of the holdings");
    assert_eq!(future.get(&6068).map(|s| s.trim_end()), Some("MES SEP2026"));
    assert_eq!(future.get(&167).map(String::as_str), Some("FUT"));
    assert_eq!(future.get(&15).map(String::as_str), Some("USD"));
}

/// A frame naming one holding still reads as one.
#[test]
fn a_single_holding_frame_is_unchanged() {
    use super::positions::split_position_entries;

    let frame = "35=UP\x016529=AR.1\x016068=SPY\x016064=342\x016008=756733\x01";
    let held = split_position_entries(frame.as_bytes());
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].get(&6008).map(String::as_str), Some("756733"));
    assert_eq!(held[0].get(&6064).map(String::as_str), Some("342"));
}

/// The conditions the venue states an order under are read back off it.
///
/// Captured from a session: the report for a resting order carrying one price
/// condition. Nothing read these, so an order this session did not place came
/// back stating none — and a program that read one back and placed it again
/// sent an order that went live at once where the original waited for its
/// price.
#[test]
fn an_order_states_the_conditions_it_waits_on() {
    use super::executions::decode_conditions;
    use crate::types::OrderCondition;

    let report = concat!(
        "35=8\x0111=1787087979010000.0\x016136=1\x01",
        "6222=1\x016123=756733\x016169=Invalid\x016168=0\x016166=nan\x016220=0\x01",
        "6124=BEST\x016126=<=\x016125=0.01\x018569=\x016223=\x016246=\x01",
        "6947=\x016245=\x016263=\x016137=n\x016128=0\x016151=0\x01",
    );

    let waits_on = decode_conditions(report.as_bytes());
    assert_eq!(waits_on.len(), 1, "the order states one condition");
    match &waits_on[0] {
        OrderCondition::Price { con_id, exchange, price, is_more, .. } => {
            assert_eq!(*con_id, 756733);
            assert_eq!(exchange, "BEST");
            assert_eq!(*price, crate::types::PRICE_SCALE / 100, "one cent");
            assert!(!*is_more, "`<=` is met below the price, not above it");
        }
        other => panic!("a price condition was stated, not {other:?}"),
    }
}

/// Two conditions on one order are both read.
///
/// They arrive as a group per condition, and a flat parse keeps the last value
/// of each tag — so an order waiting on two came back waiting on one.
#[test]
fn two_conditions_are_both_read() {
    use super::executions::decode_conditions;

    let report = concat!(
        "35=8\x0111=1\x016136=2\x01",
        "6222=1\x016123=756733\x016124=BEST\x016126=<=\x016125=0.01\x016137=a\x01",
        "6222=1\x016123=9579970\x016124=SMART\x016126=>=\x016125=999.00\x016137=n\x01",
    );

    let waits_on = decode_conditions(report.as_bytes());
    assert_eq!(waits_on.len(), 2, "both conditions are read, not only the last");
}

/// A report carrying no conditions states none, rather than one made up.
#[test]
fn an_unconditional_order_states_no_conditions() {
    use super::executions::decode_conditions;

    let report = "35=8\x0111=1\x0139=0\x0155=SPY\x01";
    assert!(decode_conditions(report.as_bytes()).is_empty());
}

/// A condition whose direction the venue states in terms this cannot read is
/// left out, the way every other unreadable field leaves its condition out.
/// Read as "at most" it stated a trigger the venue never described, and an
/// order read back and placed again waited for the opposite of what it had.
#[test]
fn a_condition_with_no_readable_direction_is_left_out() {
    use super::executions::decode_conditions;

    let report = concat!(
        "35=8\x0111=1\x016136=2\x01",
        "6222=1\x016123=756733\x016124=BEST\x016126=!!\x016125=0.01\x016137=a\x01",
        "6222=1\x016123=9579970\x016124=SMART\x016126=>=\x016125=999.00\x016137=n\x01",
    );

    let waits_on = decode_conditions(report.as_bytes());
    assert_eq!(waits_on.len(), 1, "only the condition that read is kept: {waits_on:?}");
    match &waits_on[0] {
        crate::types::OrderCondition::Price { con_id, is_more, .. } => {
            assert_eq!(*con_id, 9579970);
            assert!(*is_more, "the one that read is the `>=` one");
        }
        other => panic!("a price condition was stated, not {other:?}"),
    }
}

/// An order restated by an ordinary report keeps the group it cancels
/// together with. Read only on the recovery record, the first report about a
/// recovered order replaced the cached row with one saying the order stood
/// alone.
#[test]
fn a_restated_order_keeps_the_group_it_cancels_with() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "0"), (150, "0"), (55, "SPY"), (6008, "756733"), (583, "OCA_42"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let info = shared.orders.get_order_info(42).expect("the order was restated");
    assert_eq!(info.order.oca_group, "OCA_42", "the group is on the report and was read");
}

/// The venue turning an order down and the venue saying something about one
/// are different things, and a caller classifies on the code.
#[test]
fn a_refused_order_is_reported_under_the_rejection_code() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let frame = exec_report_frame(&[
        (39, "8"), (150, "8"), (58, "No trading permissions"),
    ]);
    ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
    let told = shared.orders.drain_order_inactive();
    assert!(
        told.iter().any(|(id, code, _)| *id == 42 && *code == 201),
        "the refusal is reported as one: {told:?}",
    );
}

/// A fan-out ends when every exchange it asked has answered. Counted per
/// frame instead, a leg answered with more than one row completes the request
/// twice over and drops the legs still outstanding.
#[test]
fn a_leg_that_answers_twice_does_not_end_the_fanout() {
    let (mut ccp, mut context, shared) = u186_test_state();
    ccp.pending_fanout.push(PendingFanout {
        api_req_id: 9,
        fanout_req_ids: vec!["ibxfan-9-0".to_string(), "ibxfan-9-1".to_string()],
        answered: Vec::new(),
        deadline: Instant::now() + SECDEF_TIMEOUT,
    });

    for _ in 0..2 {
        ccp.process_ccp_message(&secdef_not_found("ibxfan-9-0"), &mut None, &mut context,
            &shared, &None, &mut HeartbeatState::new(), "DU1");
    }

    assert!(
        shared.reference.drain_contract_details_end().is_empty(),
        "the second exchange has not answered, so the request has not ended",
    );
    assert_eq!(ccp.pending_fanout.len(), 1, "and it is still awaiting that leg");
}

/// A request the transport could not carry, and one the venue never answered,
/// both leave a caller waiting on an end that nothing on the wire will send.
#[test]
fn a_matching_symbols_request_that_goes_nowhere_still_answers() {
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let shared = SharedState::new();

    let mut no_conn: Option<Connection> = None;
    ccp.send_matching_symbols_request(7, "AAPL", &mut no_conn, &mut hb, &shared);
    assert_eq!(
        shared.reference.drain_matching_symbols().len(), 1,
        "a request that never went out is answered rather than dropped",
    );

    ccp.pending_matching_symbols.push((8, Instant::now() - Duration::from_secs(1)));
    ccp.sweep_pending_matching_symbols(&shared);
    let answered = shared.reference.drain_matching_symbols();
    assert_eq!(answered.len(), 1, "and so is one the venue never answered");
    assert_eq!(answered[0].0, 8);
}

/// A bulletin whose urgency names no type here is still a message the venue
/// sent. Dropped in silence it left no callback, no log and nothing to say
/// data had arrived and gone nowhere.
#[test]
fn a_bulletin_with_an_unnamed_urgency_is_recorded_as_unread() {
    let (mut ccp, mut context, shared) = u186_test_state();
    let msg = crate::protocol::fix::fix_build(&[
        (fix::TAG_MSG_TYPE, "B"),
        (fix::TAG_URGENCY, "7"),
        (fix::TAG_HEADLINE, "something the venue said"),
    ], 1);

    ccp.process_ccp_message(&msg, &mut None, &mut context, &shared, &None,
        &mut HeartbeatState::new(), "DU1");

    assert!(
        shared.market.unread_wire().iter().any(|(_, what)| what.contains("urgency 7")),
        "the drop is recorded: {:?}", shared.market.unread_wire(),
    );
}

/// A by-symbol lookup answers with the row that carries the trading hours.
/// The master row waits for its schedule while the per-exchange legs answer
/// for the same contract, and whichever reached the dedup gate first won —
/// so a caller's hours were decided by which reply the venue sent faster.
#[test]
fn a_fanout_leg_does_not_displace_the_row_that_carries_the_hours() {
    let (mut ccp, mut context, shared) = u186_test_state();
    let def = crate::control::contracts::ContractDefinition {
        con_id: 265598,
        symbol: "AAPL".to_string(),
        ..Default::default()
    };
    ccp.pending_schedule_pair.push(PendingSchedulePair {
        api_req_id: 9,
        join_key: "AAPL".to_string(),
        def,
        is_last: true,
        deadline: Instant::now() + Duration::from_secs(3),
    });
    ccp.pending_fanout.push(PendingFanout {
        api_req_id: 9,
        fanout_req_ids: vec!["ibxfan-9-0".to_string()],
        answered: Vec::new(),
        deadline: Instant::now() + SECDEF_TIMEOUT,
    });

    let leg = crate::protocol::fix::fix_build(&[
        (fix::TAG_MSG_TYPE, "d"),
        (crate::control::contracts::TAG_SECURITY_REQ_ID, "ibxfan-9-0"),
        (crate::control::contracts::TAG_SECURITY_RESPONSE_TYPE, "2"),
        (crate::control::contracts::TAG_IB_CON_ID, "265598"),
        (55, "AAPL"),
    ], 1);
    ccp.process_ccp_message(&leg, &mut None, &mut context, &shared, &None,
        &mut HeartbeatState::new(), "DU1");

    assert!(
        shared.reference.drain_contract_details().is_empty(),
        "the row still waiting for its hours is the one that answers",
    );
    assert!(
        shared.reference.drain_contract_details_end().is_empty(),
        "and the end waits for it too",
    );
    assert!(
        ccp.pending_schedule_pair.iter().any(|p| p.api_req_id == 9),
        "the master row is still parked",
    );
}

/// A reconnect has named nothing yet, so the replay flag is cleared. Left set
/// from the previous connection, a caller asking what it has on is answered
/// from the pre-drop book before the new account arrives.
#[test]
fn a_reconnect_waits_for_the_new_account_of_what_is_working() {
    let mut ccp = CcpState::new();
    let shared = SharedState::new();
    let market = crate::engine::market_state::MarketState::new();
    let mut hb = HeartbeatState::new();
    ccp.hydrated_any = true;
    shared.orders.set_replay_done();

    let (conn, _peer) = crate::protocol::connection::Connection::for_test();
    let mut ccp_conn: Option<Connection> = None;
    ccp.reconnect(conn, &mut ccp_conn, &mut hb, "DU1", &market, &shared);

    assert!(!shared.orders.replay_done(), "the new connection has named nothing yet");
    assert!(!ccp.hydrated_any, "and nothing has been hydrated from it");
}

/// The venue says why it would not cancel an order, and the structured
/// rejection carries two numbers and no text. The reason went to a log no
/// caller reads, where "the order does not exist" and "it is too late" look
/// the same.
#[test]
fn a_refused_cancel_carries_the_reason_the_venue_gave() {
    let (mut ccp, mut context, shared) = ord_status_test_state();
    let mut frame = std::collections::HashMap::new();
    frame.insert(41u32, "42".to_string());
    frame.insert(434u32, "1".to_string());
    frame.insert(102u32, "0".to_string());
    frame.insert(58u32, "Too late to cancel".to_string());

    ccp.handle_cancel_reject(&frame, &mut context, &shared, &None);

    let told = shared.orders.drain_order_inactive();
    assert!(
        told.iter().any(|(id, _, text)| *id == 42 && text == "Too late to cancel"),
        "the caller is told what the venue said: {told:?}",
    );
}

/// Whether the venue manages an order's price for it is a field of its own,
/// beside the algo rather than part of it. Read off the algo, an adaptive
/// order gained price management it may not have and every other order lost
/// it.
#[test]
fn price_management_is_read_from_its_own_field() {
    for (adaptive, stated, wanted) in [
        ("Adaptive", None, 0),
        ("Adaptive", Some("1"), 1),
        ("", Some("1"), 1),
        ("", None, 0),
    ] {
        let (mut ccp, mut context, shared) = ord_status_test_state();
        let mut pairs = vec![("39", "0"), ("150", "0"), ("55", "SPY")];
        if !adaptive.is_empty() {
            pairs.push(("847", adaptive));
        }
        if let Some(v) = stated {
            pairs.push(("8339", v));
        }
        let frame = exec_report_frame(
            &pairs.iter().map(|(t, v)| (t.parse().unwrap(), *v)).collect::<Vec<_>>(),
        );
        ccp.handle_exec_report(&frame, b"", &mut context, &shared, &None, "");
        let info = shared.orders.get_order_info(42).expect("the order was restated");
        assert_eq!(
            info.order.use_price_mgmt_algo, wanted,
            "847={adaptive:?} 8339={stated:?}",
        );
    }
}

/// Each public identifier rides the tags the venue reads it on. A CUSIP was
/// going out as `22=1|48=<id>`, which is the pair an ISIN uses, and a FIGI was
/// not going out at all — the lookup fell through to the symbol and answered
/// with whatever that matched.
#[test]
fn a_public_identifier_rides_the_tags_its_own_kind_uses() {
    use std::io::Read;
    for (kind, id, wanted, unwanted) in [
        ("CUSIP", "037833100", vec!["454=1", "455=037833100", "456=1"], vec!["22=", "48="]),
        ("ISIN", "US0378331005", vec!["22=4", "48=US0378331005"], vec!["454=", "455="]),
        ("FIGI", "BBG000B9XRY4", vec!["22=S", "48=BBG000B9XRY4"], vec!["454=", "455="]),
    ] {
        let (conn, mut peer) = crate::protocol::connection::Connection::for_test();
        let mut ccp = CcpState::new();
        let mut hb = HeartbeatState::new();
        let mut conn = Some(conn);
        let filters = crate::types::SecDefFilters {
            sec_id: id.to_string(),
            sec_id_type: kind.to_string(),
            ..Default::default()
        };
        ccp.send_secdef_request_by_symbol(
            9, "AAPL", "STK", "SMART", "USD", &filters, &mut conn, &mut hb,
        );

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|");
        for field in wanted {
            assert!(msg.contains(&format!("|{field}|")), "{kind} states {field}: {msg}");
        }
        for field in unwanted {
            assert!(!msg.contains(&format!("|{field}")), "{kind} does not state {field}: {msg}");
        }
        // The identifier replaces the symbol, and asking by both is asking a
        // different question from the one the caller put.
        assert!(!msg.contains("|55=AAPL|"), "{kind} does not also ask by symbol: {msg}");
    }
}

/// A lookup states the symbol and the venue's local symbol as two separate
/// fields, because they are two separate statements about the contract. Sending
/// only the local symbol asked a narrower question than the caller put, and a
/// symbol that disagrees with it — which the venue would refuse — matched
/// whatever the local symbol named.
#[test]
fn a_lookup_states_both_the_symbol_and_the_local_symbol() {
    use std::io::Read;
    let (conn, mut peer) = crate::protocol::connection::Connection::for_test();
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let mut conn = Some(conn);
    let filters = crate::types::SecDefFilters {
        local_symbol: "ESZ6".to_string(),
        ..Default::default()
    };
    ccp.send_secdef_request_by_symbol(
        11, "ES", "FUT", "CME", "USD", &filters, &mut conn, &mut hb,
    );

    let mut buf = [0u8; 4096];
    let n = peer.read(&mut buf).unwrap();
    let msg = String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|");
    assert!(msg.contains("|55=ES|"), "the symbol: {msg}");
    assert!(msg.contains("|6035=ESZ6|"), "and the contract's own name: {msg}");
}

/// A news stream is withdrawn by naming which tick and which contract, not
/// only the request number. The option model beside it is withdrawn the same
/// way. Naming only the request leaves the venue serving the subscription.
#[test]
fn a_news_stream_is_withdrawn_by_naming_what_it_was() {
    use std::io::Read;
    let (conn, mut peer) = crate::protocol::connection::Connection::for_test();
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let mut conn = Some(conn);
    ccp.send_news_subscribe(756733, 3, "STK", "BRFG", 41, &mut conn, &mut hb);
    let mut buf = [0u8; 4096];
    let _subscribe = peer.read(&mut buf).unwrap();

    ccp.send_news_unsubscribe(3, &mut conn, &mut hb);
    let n = peer.read(&mut buf).unwrap();
    let msg = String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|");
    assert!(msg.contains("|263=2|"), "it is a withdrawal: {msg}");
    assert!(msg.contains("|146=1|"), "of one entry: {msg}");
    assert!(msg.contains("|262=41|"), "under the request it was asked under: {msg}");
    assert!(msg.contains("|6008=756733|"), "naming the contract: {msg}");
    assert!(msg.contains("|264=292|"), "and which tick: {msg}");
    assert!(
        ccp.news_subscriptions.is_empty(),
        "and nothing is left waiting to deliver it",
    );
}

/// A contract fetched without a caller asking is remembered so it is not
/// fetched twice. The record is dropped when the fetch times out, or one lost
/// request leaves that contract unnamed for the life of the session.
#[test]
fn a_fetch_that_is_never_answered_is_asked_again() {
    let (conn, _peer) = crate::protocol::connection::Connection::for_test();
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let shared = SharedState::new();
    let mut conn = Some(conn);

    ccp.auto_fetch_secdef_if_cold(756733, &mut conn, &shared, &mut hb);
    assert_eq!(ccp.pending_secdef.len(), 1, "the fetch went out");
    assert!(ccp.auto_fetched_conids.contains_key(&756733), "and is remembered while it is out");

    // Nothing answers it.
    for entry in &mut ccp.pending_secdef {
        entry.2 = Instant::now() - std::time::Duration::from_secs(1);
    }
    ccp.sweep_contract_details(&shared, &None);
    assert!(
        !ccp.auto_fetched_conids.contains_key(&756733),
        "a fetch that never came back is forgotten, so the next report asks again",
    );

    ccp.auto_fetch_secdef_if_cold(756733, &mut conn, &shared, &mut hb);
    assert_eq!(ccp.pending_secdef.len(), 1, "and it is asked again");
}

/// One frame names every holding in it, and the sets the account does not hold
/// itself are no different from its own. Handed the flat map instead, the
/// generic parser kept the last value of each repeated tag, so a frame naming
/// three holdings arrived as one and the other two were gone before anything
/// could see them.
#[test]
fn an_away_position_frame_names_every_holding_in_it() {
    let (mut ccp, mut context, shared) = u186_test_state();
    let mut conn: Option<Connection> = None;
    let mut hb = HeartbeatState::new();
    let mut fields: Vec<(u32, &str)> = vec![
        (crate::protocol::fix::TAG_MSG_TYPE, "AP"),
    ];
    for (symbol, con_id, qty, cost) in [
        ("AAPL  ", "265598", "100", "150.0"),
        ("MSFT  ", "272093", "25", "300.0"),
        ("SPY   ", "756733", "7", "500.0"),
    ] {
        fields.push((6068, symbol));
        fields.push((6008, con_id));
        fields.push((167, "STK"));
        fields.push((15, "USD"));
        fields.push((6064, qty));
        fields.push((6101, cost));
    }
    let frame = crate::protocol::fix::fix_build(&fields, 1);
    ccp.process_ccp_message(&frame, &mut conn, &mut context, &shared, &None, &mut hb, "DU1");

    let mut held = shared.portfolio.positions_elsewhere();
    held.sort_by_key(|row| row.con_id);
    assert_eq!(held.len(), 3, "every holding the frame names: {held:?}");
    assert_eq!(held[0].symbol, "AAPL", "the venue pads a symbol out");
    assert_eq!(held[0].position, 100.0);
    assert_eq!(held[1].symbol, "MSFT");
    assert_eq!(held[1].avg_cost, 300 * PRICE_SCALE);
    assert_eq!(held[2].position, 7.0, "and the last one is not the only one");
    assert!(
        held.iter().all(|row| row.held == crate::types::HeldElsewhere::Away),
        "and all of them in the set the frame belongs to: {held:?}",
    );
}

/// A frame restating a holding without its cost was replacing a real basis
/// with nothing. The account's own holdings already keep theirs.
#[test]
fn a_holding_elsewhere_keeps_the_basis_a_later_frame_leaves_out() {
    let (_ccp, _context, shared) = u186_test_state();
    let mut row = std::collections::HashMap::new();
    row.insert(6008u32, "265598".to_string());
    row.insert(6064u32, "100".to_string());
    row.insert(6101u32, "150.0".to_string());
    super::positions::handle_position_elsewhere(&row, &shared, crate::types::HeldElsewhere::Away);

    row.remove(&6101);
    row.insert(6064u32, "120".to_string());
    super::positions::handle_position_elsewhere(&row, &shared, crate::types::HeldElsewhere::Away);

    let held = shared.portfolio.positions_elsewhere();
    assert_eq!(held[0].position, 120.0, "the new quantity");
    assert_eq!(held[0].avg_cost, 150 * PRICE_SCALE, "and the basis it already had");
}

/// Subscribing to account updates asks the venue for the figures.
///
/// The venue restates them on its own schedule. Measured against a live
/// session: a subscription alone is answered after 39 seconds, and the same
/// subscription with this request after 750 milliseconds.
#[test]
fn an_account_refresh_asks_for_the_figures() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let mut buf = [0u8; 4096];

    ccp.send_account_refresh("DU123456", &mut conn, &mut hb);

    let n = peer.read(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|");
    let key_of = |t: &str| -> String {
        t.split('|').find(|f| f.starts_with("6529=")).unwrap_or("").to_string()
    };

    // The display request carries the positions beside the figures.
    assert!(text.contains("|6040=91|"), "the display request is stated: {text}");
    assert!(text.contains("|6556=DR.1|"), "under its own key: {text}");
    // The keyed account request.
    assert!(text.contains("|6040=6|"), "the account request is stated: {text}");
    let first = key_of(&text);

    // A second request states a different key, and a second state does too:
    // the venue answers a key it is already serving with nothing, and a
    // connection outlives the loops that use it.
    ccp.send_account_refresh("DU123456", &mut conn, &mut hb);
    let n = peer.read(&mut buf).unwrap();
    let second = key_of(&String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|"));
    assert_ne!(first, second, "a second request states a key of its own");

    let mut fresh = CcpState::new();
    fresh.send_account_refresh("DU123456", &mut conn, &mut hb);
    let n = peer.read(&mut buf).unwrap();
    let third = key_of(&String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|"));
    assert_ne!(second, third, "and so does a request from a state built later");

    // The subscription is closed under the key it was opened with. Tag 6036
    // states which of the two the request is.
    fresh.send_account_unsubscribe("DU123456", &mut conn, &mut hb);
    let n = peer.read(&mut buf).unwrap();
    let closing = String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|");
    assert!(closing.contains("|6036=0|"), "the request closes rather than opens: {closing}");
    assert_eq!(key_of(&closing), third, "and names the subscription it opened: {closing}");
    assert!(text.contains("|6095=DU123456|"), "naming the account: {text}");
    // The account rides its own tag on the display request, not inside a key.
    assert!(text.contains("|1=DU123456|"), "and tag 1 names it too: {text}");
}

/// A holding names its own contract.
///
/// The feed states the symbol on 6068 and the security type on 167 beside the
/// quantity. Read only for the contract id, a holding reaches the caller
/// carrying an id and nothing else, and stays that way until a definition
/// lookup answers.
#[test]
fn a_holding_carries_the_contract_the_feed_names() {
    let shared = SharedState::new();
    let mut context = Context::new();
    let mut ccp = CcpState::new();
    let mut hb = HeartbeatState::new();
    let mut conn = None;

    let msg = ["6008=756733", "6068=SPY", "167=STK", "6064=100", "6101=768.5",
               "6008=265598", "6068=AAPL", "167=STK", "6064=50", "6101=316.2",
               "6008=0"].join("\u{1}");
    ccp.handle_position_feed(msg.as_bytes(), &mut conn, &mut context, &shared, &None, &mut hb);

    let held = shared.portfolio.position_infos();
    let spy = held.iter().find(|p| p.con_id == 756733).expect("the first holding");
    assert_eq!(spy.symbol, "SPY", "named as the feed names it");
    assert_eq!(spy.sec_type, "STK");
    let aapl = held.iter().find(|p| p.con_id == 265598).expect("the second holding");
    assert_eq!(aapl.symbol, "AAPL", "each entry carries its own, not the one before it");
    assert_eq!(aapl.sec_type, "STK");
}

/// Every ask for account and position data draws its own key, and the state
/// remembers the one it last asked under.
///
/// The venue answers a key it is already serving with nothing, so two asks
/// sharing a key means the second is not answered — and a reconnect that named
/// a fixed key was asking under one the refreshes had already spent, leaving
/// the position pushes not resuming after a drop. The recorded key matters for
/// the same reason: the unsubscribe closes what it names.
#[test]
fn every_account_request_draws_its_own_key() {
    let mut ccp = CcpState::new();

    let first = ccp.next_account_request_key();
    assert_eq!(ccp.account_request_key.as_deref(), Some(first.as_str()));

    let second = ccp.next_account_request_key();
    assert_ne!(first, second, "two asks shared a key, so one goes unanswered");
    assert_eq!(
        ccp.account_request_key.as_deref(),
        Some(second.as_str()),
        "the unsubscribe would close a key this connection is no longer served under",
    );
}

/// The venue's urgency and the caller's kind are two numberings, and the two
/// exchange kinds sit the other way round in each. Passed straight through, a
/// caller halting on an exchange that had stopped trading acted on one that
/// had started.
#[test]
fn a_bulletin_is_reported_as_the_kind_a_caller_reads_not_the_urgency_stated() {
    // Stated urgency -> the kind a caller is told, and what that kind means.
    let cases = [
        (1, 1, "ordinary news"),
        (2, 3, "an exchange that has stopped trading"),
        (3, 2, "an exchange that has started"),
        (8, 4, "plain text"),
        (9, 5, "a message meant to be shown"),
        (10, 6, "one written as markup"),
    ];
    for (urgency, kind, what) in cases {
        let mut ccp = CcpState::new();
        let shared = SharedState::new();
        let parsed = std::collections::HashMap::from([
            (crate::protocol::fix::TAG_URGENCY, urgency.to_string()),
            (crate::protocol::fix::TAG_HEADLINE, what.to_string()),
            (crate::protocol::fix::TAG_SECURITY_EXCHANGE, "NASDAQ".to_string()),
            (crate::protocol::fix::TAG_BULLETIN_ID, "4242".to_string()),
        ]);
        ccp.handle_news_bulletin(&parsed, &shared);
        let sent = shared.market.drain_news_bulletins();
        assert_eq!(sent.len(), 1, "urgency {urgency} was dropped");
        assert_eq!(sent[0].msg_type, kind, "urgency {urgency} names {what}");
        assert_eq!(sent[0].msg_id, 4242, "the venue numbers its own bulletins");
    }
}

/// A bulletin the venue did not number stands at the widest number one is
/// carried under, rather than at a count this session kept.
#[test]
fn an_unnumbered_bulletin_says_so() {
    let mut ccp = CcpState::new();
    let shared = SharedState::new();
    let parsed = std::collections::HashMap::from([
        (crate::protocol::fix::TAG_URGENCY, "1".to_string()),
        (crate::protocol::fix::TAG_HEADLINE, "something".to_string()),
    ]);
    ccp.handle_news_bulletin(&parsed, &shared);
    assert_eq!(shared.market.drain_news_bulletins()[0].msg_id, i32::MAX);
}

/// The charge is on a record of its own, and the execution report carries
/// none: captured against a real fill, the report has no commission tag at
/// all. Taken from there, every caller was told its fills were free.
#[test]
fn what_a_fill_cost_is_read_off_the_record_that_states_it() {
    let shared = SharedState::new();
    // The record as the venue sent it, from a captured session: the execution
    // it belongs to, what it cost, and the currency that is charged in.
    let parsed = std::collections::HashMap::from([
        (crate::protocol::fix::TAG_EXEC_ID, "00025b49.6a8880e4.01.01".to_string()),
        (crate::protocol::fix::TAG_TRADE_CHARGE, "1.000003".to_string()),
        (crate::protocol::fix::TAG_TRADE_CHARGE_CURRENCY, "USD".to_string()),
    ]);
    super::handle_trade_charge(&parsed, &shared);

    let charged = shared.orders.drain_charges();
    assert_eq!(charged.len(), 1);
    assert_eq!(charged[0].exec_id, "00025b49.6a8880e4.01.01");
    assert!((charged[0].commission_and_fees - 1.000003).abs() < 1e-9);
    assert_eq!(charged[0].currency, "USD", "as the venue charges it, not as the contract is priced");
    assert!(shared.orders.drain_charges().is_empty(), "read once");
}

/// A record naming no execution, or stating no charge, says nothing — and
/// nothing is what is reported, rather than a zero against some other fill.
#[test]
fn a_record_that_states_no_charge_reports_none() {
    for parsed in [
        // No execution named.
        std::collections::HashMap::from([
            (crate::protocol::fix::TAG_TRADE_CHARGE, "1.5".to_string()),
        ]),
        // Named, and no charge stated.
        std::collections::HashMap::from([
            (crate::protocol::fix::TAG_EXEC_ID, "abc.def.01.01".to_string()),
        ]),
    ] {
        let shared = SharedState::new();
        super::handle_trade_charge(&parsed, &shared);
        assert!(shared.orders.drain_charges().is_empty());
    }
}
