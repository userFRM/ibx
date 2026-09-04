//! The tests for this module.
//!
//! One file per module, as `api/client` already does it. Each block below
//! reaches the code it tests through `super::super`, which is the module this
//! file belongs to.

use super::*;
use crate::types::SmartComponent;
use crate::bridge::RichOrderInfo;
use crate::types::model::OrderState as ApiOrderState;

/// A market-data type nobody recognises does not become the venue's word.
///
/// Subscriptions stay realtime whatever it names, and the callback that
/// reports a subscription's type reads what was stored — so storing the number
/// would tell a caller their data is of a type the venue never stated and
/// their subscription is not on.
#[test]
fn an_unknown_market_data_type_is_not_kept() {
    let core = ClientCore::new();
    core.set_market_data_type(MDT_DELAYED);
    assert_eq!(core.subscription_mode(), 1);

    core.set_market_data_type(99);
    assert_eq!(core.subscription_mode(), 1, "the last known type still stands");
}

/// An account holding nothing still has a P&L, and a P&L of zero is an
/// answer. Neither an empty position list nor a zero value withholds it.
#[test]
fn an_account_with_no_positions_still_reports_its_pnl() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    shared.portfolio.set_account(&crate::types::AccountState::default());

    core.subscribe_pnl(7).unwrap();
    let update = core.poll_pnl(&shared).expect("a subscription is answered");
    assert_eq!(update.req_id, 7);
    assert_eq!(update.daily_pnl, 0.0);
    assert_eq!(update.unrealized_pnl, 0.0);
    assert_eq!(update.realized_pnl, 0.0);
    assert!(core.poll_pnl(&shared).is_none(), "the same figures do not repeat");
}

/// The type a caller asks for has to reach the subscription, or asking for
/// delayed data got realtime-shaped subscriptions and no delayed ticks.
#[test]
fn the_requested_market_data_type_picks_the_subscription_mode() {
    let core = ClientCore::new();
    assert_eq!(core.subscription_mode(), 0, "realtime until asked otherwise");
    for (requested, mode) in [
        (MDT_DELAYED, 1),
        (MDT_FROZEN, 2),
        (MDT_DELAYED_FROZEN, 3),
        (MDT_REALTIME, 0),
    ] {
        core.set_market_data_type(requested);
        assert_eq!(core.subscription_mode(), mode, "type {requested}");
        assert_eq!(
            core.check_mdt_needed(requested as i64, true),
            Some(requested),
            "the callback names the type the data was asked for",
            );
    }
}

// ── Rejected/Inactive snapshot admission ──

#[test]
fn is_open_or_reactivatable_admits_genuine_inactive() {
    assert!(is_open_or_reactivatable("Inactive", ""));
}

#[test]
fn is_open_or_reactivatable_excludes_rejected_shaped_inactive() {
    // A rejected order also stringifies to "Inactive", but always carries
    // a non-empty completed_status — that is what must exclude it.
    assert!(!is_open_or_reactivatable("Inactive", "No valid bid/ask"));
}

#[test]
fn is_open_or_reactivatable_still_admits_ordinary_open_status() {
    assert!(is_open_or_reactivatable("Submitted", ""));
}

#[test]
fn is_open_or_reactivatable_still_excludes_terminal_status() {
    assert!(!is_open_or_reactivatable("Filled", ""));
    assert!(!is_open_or_reactivatable("Cancelled", ""));
}

#[test]
fn collect_open_orders_admits_inactive_but_excludes_rejected_locally_tracked() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.track_order(80, ApiContract::default(), ApiOrder { order_id: 80, ..Default::default() }, 0);
    core.track_order(81, ApiContract::default(), ApiOrder { order_id: 81, ..Default::default() }, 0);

    core.update_order_status(&shared, 80, OrderStatus::Inactive, 0.0, 100.0);
    core.update_order_status(&shared, 81, OrderStatus::Rejected, 0.0, 100.0);

    let result = core.collect_open_orders(&shared);
    assert!(result.iter().any(|(id, _)| *id == 80),
        "genuinely-inactive order must remain in the open-order snapshot");
    assert!(!result.iter().any(|(id, _)| *id == 81),
        "rejected order must not resurrect into the open-order snapshot");
}

#[test]
fn collect_open_orders_shared_only_admits_inactive_but_excludes_rejected() {
    let core = ClientCore::new();
    let shared = SharedState::new();

    shared.orders.push_order_info(90, RichOrderInfo {
        contract: ApiContract::default(),
        order: ApiOrder { order_id: 90, ..Default::default() },
        order_state: ApiOrderState { status: "Inactive".into(), ..Default::default() },
        last_exec: Default::default(),
    });
    shared.orders.push_order_info(91, RichOrderInfo {
        contract: ApiContract::default(),
        order: ApiOrder { order_id: 91, ..Default::default() },
        order_state: ApiOrderState {
            status: "Inactive".into(),
            completed_status: "No valid bid/ask".into(),
            ..Default::default()
        },
        last_exec: Default::default(),
    });

    let result = core.collect_open_orders(&shared);
    assert!(result.iter().any(|(id, _)| *id == 90),
        "genuinely-inactive shared-only order must be admitted to the open-order snapshot");
    assert!(!result.iter().any(|(id, _)| *id == 91),
        "rejected shared-only order must not resurrect into the open-order snapshot");
}

/// An order this client did not place still arrives through the shared
/// cache, and it carries its own filled quantity. Reporting zero made a
/// partially filled order read as untouched to anything polling
/// `req_open_orders`.
#[test]
fn a_shared_order_reports_its_filled_quantity() {
    let shared = SharedState::new();
    let core = ClientCore::new();
    let order = crate::types::model::Order {
        total_quantity: 10.0,
        filled_quantity: 4.0,
        ..Default::default()
    };
    let order_state = crate::types::model::OrderState {
        status: "Submitted".to_string(),
        ..Default::default()
    };
    shared.orders.push_order_info(55, crate::bridge::RichOrderInfo {
        contract: crate::types::model::Contract::default(),
        order,
        order_state,
        last_exec: crate::types::model::Execution::default(),
    });

    let open = core.collect_open_orders(&shared);
    let (_, tracked) = open.iter().find(|(id, _)| *id == 55).expect("the shared order");
        assert_eq!(tracked.filled, 4.0, "the filled quantity it carries");
    assert_eq!(tracked.remaining, 6.0, "and what is left of the order");
    }

fn shared_with_components(comps: Vec<(i32, &str)>) -> SharedState {
    let s = SharedState::new();
    s.reference.set_smart_components(
        comps.into_iter().map(|(bit, letter)| SmartComponent {
            bit_number: bit,
            exchange: format!("EX{bit}"),
            exchange_letter: letter.to_string(),
        }).collect()
    );
    s
}

#[test]
fn render_exchange_mask_zero_is_empty() {
    let s = shared_with_components(vec![(0, "Q"), (1, "N")]);
    assert_eq!(render_exchange_mask(0, &s), "");
}

#[test]
fn render_exchange_mask_single_bit() {
    let s = shared_with_components(vec![(0, "Q"), (1, "N"), (2, "P")]);
    assert_eq!(render_exchange_mask(0b001, &s), "Q");
    assert_eq!(render_exchange_mask(0b100, &s), "P");
}

#[test]
fn render_exchange_mask_multiple_bits() {
    let s = shared_with_components(vec![
        (0, "Q"), (1, "N"), (2, "P"), (3, "Z"),
    ]);
    // bits 0, 2, 3 set → letters in bit-order: Q, P, Z
    assert_eq!(render_exchange_mask(0b1101, &s), "QPZ");
}

#[test]
fn render_exchange_mask_unknown_bit_skipped() {
    let s = shared_with_components(vec![(0, "Q")]);
    // bit 5 set, no component at bit 5 — skipped
    assert_eq!(render_exchange_mask(0b100000, &s), "");
}

// ── what a P&L poll reports ──

fn seed_pnl_position(
    core: &ClientCore,
    shared: &SharedState,
    con_id: i64,
    iid: InstrumentId,
    position: f64,
    avg_cost_dollars: f64,
    last_dollars: f64,
    close_dollars: f64,
) {
    core.con_id_to_instrument.lock().unwrap().insert(con_id, iid);
    core.instrument_to_req.lock().unwrap().insert(iid, 1);
    shared.portfolio.set_position_info(PositionInfo {
        con_id,
        position,
        avg_cost: (avg_cost_dollars * PRICE_SCALE_F) as i64,
        symbol: format!("SYM{con_id}"),
        sec_type: "STK".into(),
        currency: "USD".into(),
        multiplier: String::new(),
        ..Default::default()
    });
    let q = Quote {
        last: (last_dollars * PRICE_SCALE_F) as i64,
        close: (close_dollars * PRICE_SCALE_F) as i64,
        ..Default::default()
    };
    shared.market.push_quote(iid, &q);
}

#[test]
fn poll_pnl_no_subscription_returns_none() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    assert!(core.poll_pnl(&shared).is_none());
}

/// A total missing one position is not a smaller correct total. When a
/// position cannot be priced the client-side sum is incomplete — and the
/// realized figure has already accrued for it, so the three do not even
/// agree with each other. The venue's account numbers are complete by
/// construction, so one unpriceable position sends the whole account there.
#[test]
fn one_unpriceable_position_sends_the_whole_account_to_the_gateway() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(11).unwrap();

    // One ordinary position that prices fine.
    seed_pnl_position(&core, &shared, 1, 0, 1.0, 100.00, 101.00, 100.00);

    // And one held overnight that this session cannot size.
    core.con_id_to_instrument.lock().unwrap().insert(2, 1);
    core.instrument_to_req.lock().unwrap().insert(1, 1);
    let q = Quote {
        last: (735.00 * PRICE_SCALE_F) as i64,
        close: (730.00 * PRICE_SCALE_F) as i64,
        ..Default::default()
    };
    shared.market.push_quote(1, &q);
    shared.portfolio.set_midnight_seeds(String::new(), vec![MidnightSeed {
        con_id: 2, qty_midnight: Some(10.0), cost_midnight: None, qty_traded: None,
        money_traded: 0.0, realized_pnl: 0.0,
    }]);
    shared.portfolio.set_account(&AccountState {
        daily_pnl: (51.0 * PRICE_SCALE_F) as i64,
        unrealized_pnl: (351.0 * PRICE_SCALE_F) as i64,
        ..Default::default()
    });

    let update = core.poll_pnl(&shared).expect("callback must fire");
    assert!(
        (update.daily_pnl - 51.0).abs() < 1e-6,
        "the gateway's complete figure, not the one priceable position: daily={}",
        update.daily_pnl,
    );
}

/// `pnlSingle` loses only its daily figure when the overnight size is
/// unknown. The position, its value, the unrealized and the realized are all
/// still known, and suppressing the callback would leave every one of them
/// stale on the caller's side.
#[test]
fn an_unknown_seed_does_not_suppress_the_rest_of_a_single_callback() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl_single(21, 756733);

    seed_pnl_position(&core, &shared, 756733, 0, 10.0, 700.00, 735.00, 730.00);
    shared.portfolio.set_midnight_seeds(String::new(), vec![MidnightSeed {
        con_id: 756733, qty_midnight: None, cost_midnight: None, qty_traded: None,
        money_traded: 0.0, realized_pnl: 0.0,
    }]);

    let first = core.poll_pnl_single(&shared);
    assert!(!first.is_empty(), "the known fields must still be reported");
    assert!((first[0].pos - 10.0).abs() < 1e-6, "position");
    assert!((first[0].unrealized_pnl - 350.0).abs() < 1e-6, "unrealized");

    // And a later change to a field that IS known still produces an update.
    let q = Quote {
        last: (736.00 * PRICE_SCALE_F) as i64,
        close: (730.00 * PRICE_SCALE_F) as i64,
        ..Default::default()
    };
    shared.market.push_quote(0, &q);
    let second = core.poll_pnl_single(&shared);
    assert!(!second.is_empty(), "a moved quote must still reach the caller");
        assert!((second[0].unrealized_pnl - 360.0).abs() < 1e-6, "unrealized moved");
}

 ///, consumer side. Dropping an unusable position row stops the feed
/// publishing a flat, but P&L reads the absence back as zero shares and
/// reports the whole overnight holding as sold. Held 10 at a $730 close,
/// now $735: the honest answer is 50, the flat reading is -7300, and with
/// nothing priceable the venue's account figure stands instead.
#[test]
fn an_unsizeable_overnight_position_is_not_priced_as_sold() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(7).unwrap();

    // A quote and a seed, but no position row — the feed dropped it.
    core.con_id_to_instrument.lock().unwrap().insert(756733, 0);
    core.instrument_to_req.lock().unwrap().insert(0, 1);
    let q = Quote {
        last: (735.00 * PRICE_SCALE_F) as i64,
        close: (730.00 * PRICE_SCALE_F) as i64,
        ..Default::default()
    };
    shared.market.push_quote(0, &q);
    shared.portfolio.set_midnight_seeds(String::new(), vec![MidnightSeed {
        con_id: 756733,
        qty_midnight: Some(10.0),
        cost_midnight: None,
        qty_traded: None,
        money_traded: 0.0,
        realized_pnl: 0.0,
    }]);
    shared.portfolio.set_account(&AccountState {
        daily_pnl: (50.0 * PRICE_SCALE_F) as i64,
        unrealized_pnl: (350.0 * PRICE_SCALE_F) as i64,
        ..Default::default()
    });

    let update = core.poll_pnl(&shared).expect("callback must fire");
    assert!(
        (update.daily_pnl - 50.0).abs() < 1e-6,
        "the stated figure stands; -7300 is the flat reading: daily={}",
        update.daily_pnl,
    );
}

/// The same absence on the overnight leg. A seed row that stated no
/// quantity means the position's midnight size is unknown — not that it was
/// opened today, which is what a missing row means. Held 10 from $700, a
/// $730 close and $735 now: the intraday reading synthesizes cash from
/// average cost and reports 350, the unrealized figure, as the day's move.
#[test]
fn a_seed_without_a_quantity_is_not_read_as_opened_today() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(8).unwrap();

    seed_pnl_position(&core, &shared, 756733, 0, 10.0, 700.00, 735.00, 730.00);
    shared.portfolio.set_midnight_seeds(String::new(), vec![MidnightSeed {
        con_id: 756733,
        qty_midnight: None,
        cost_midnight: None,
        qty_traded: None,
        money_traded: 0.0,
        realized_pnl: 0.0,
    }]);
    shared.portfolio.set_account(&AccountState {
        daily_pnl: (50.0 * PRICE_SCALE_F) as i64,
        unrealized_pnl: (350.0 * PRICE_SCALE_F) as i64,
        ..Default::default()
    });

    let update = core.poll_pnl(&shared).expect("callback must fire");
    assert!(
        (update.daily_pnl - 50.0).abs() < 1e-6,
        "350 is the intraday synthesis, not the day's move: daily={}",
        update.daily_pnl,
    );
}

/// A stated zero is a figure, not a silence.
///
/// A position marked at what it cost has made nothing, and the venue says so.
/// Read as though nothing had been said, that fell through to a figure worked
/// out here from the last print — so a caller was told a position had made
/// something on a report where the venue said it had made nothing.
#[test]
fn a_position_the_venue_says_has_made_nothing_is_reported_as_nothing() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl_single(11, 8002);

    // Held at 100, and the last print is 105 — so a figure worked out here
    // would say 50. The venue marks it at what it cost and states that it has
    // made nothing, which is the answer.
    seed_pnl_position(&core, &shared, 8002, 0, 10.0, 100.0, 105.0, 100.0);
    shared.portfolio.set_position_marks(8002, Some((100.0 * PRICE_SCALE_F) as i64), None, Some(0), None);

    let updates = core.poll_pnl_single(&shared);
    let update = updates.first().expect("callback must fire");
    assert_eq!(
        update.unrealized_pnl, 0.0,
        "the venue said nothing was made, so nothing was made",
    );
}

#[test]
fn poll_pnl_intraday_opened_position_fires_callback() {
    // An account flat at midnight that opens a position during the day.
    // Before fix: poll_pnl early-returned on empty seeds → no callback.
    // After fix: position iterated, money_traded synthesized, daily P&L = unrealized.
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(42).unwrap();

    // 1 share bought at $735.00, now $735.07. No midnight seed (flat at midnight).
    seed_pnl_position(&core, &shared, 756733, 0, 1.0, 735.00, 735.07, 0.0);

    let update = core.poll_pnl(&shared).expect("callback must fire");
    assert_eq!(update.req_id, 42);
    assert!((update.daily_pnl - 0.07).abs() < 1e-6, "daily={}", update.daily_pnl);
    assert!((update.unrealized_pnl - 0.07).abs() < 1e-6);
    assert!((update.realized_pnl - 0.0).abs() < 1e-6);
}

#[test]
fn poll_pnl_overnight_position_with_seed_unchanged() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(99).unwrap();

    // Held 10 SPY through midnight: qty_midnight=10, prev_close=$730, avg_cost=$700.
    // No fills today (money_traded=0). Current price $735.
    seed_pnl_position(&core, &shared, 756733, 0, 10.0, 700.00, 735.00, 730.00);
    shared.portfolio.set_midnight_seeds(String::new(), vec![MidnightSeed {
        con_id: 756733,
        qty_midnight: Some(10.0),
        cost_midnight: None,
        qty_traded: None,
        money_traded: 0.0,
        realized_pnl: 0.0,
    }]);

    let update = core.poll_pnl(&shared).expect("callback must fire");
    // daily = 10×735 - 10×730 - 0 = 50
    assert!((update.daily_pnl - 50.0).abs() < 1e-6, "daily={}", update.daily_pnl);
    // unrealized = 10 × (735 - 700) = 350
    assert!((update.unrealized_pnl - 350.0).abs() < 1e-6);
}

#[test]
fn poll_pnl_seeded_position_traded_intraday_uses_signed_net_cash() {
    // /: a position held at midnight AND traded intraday
    // carries a non-zero moneyTradedSinceMidnight (6822), signed SELL+/BUY-.
    // The daily formula must ADD it. Sold 3 of 10 at $110 (avg $100): the
    // seed carries +330 net cash (sell proceeds) and +30 realized.
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(31).unwrap();

    // Now holding 7 (was 10 at midnight), avg $100, last $110, prev close $100.
    seed_pnl_position(&core, &shared, 1, 0, 7.0, 100.00, 110.00, 100.00);
    shared.portfolio.set_midnight_seeds(String::new(), vec![MidnightSeed {
        con_id: 1,
        qty_midnight: Some(10.0),
        cost_midnight: None,
        qty_traded: None,
        money_traded: 330.0,   // +330 = sold 3 @ $110 (wire sign, SELL positive)
        realized_pnl: 30.0,
    }]);

    let update = core.poll_pnl(&shared).expect("callback must fire");
    // daily = 7×110 - 10×100 + 330 = 100 (70 remaining unrealized + 30 realized)
    assert!((update.daily_pnl - 100.0).abs() < 1e-6, "daily={}", update.daily_pnl);
    // unrealized = 7 × (110 - 100) = 70
    assert!((update.unrealized_pnl - 70.0).abs() < 1e-6, "unreal={}", update.unrealized_pnl);
    assert!((update.realized_pnl - 30.0).abs() < 1e-6, "real={}", update.realized_pnl);
}

#[test]
fn poll_pnl_change_detection_suppresses_duplicate() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(7).unwrap();
    seed_pnl_position(&core, &shared, 1, 0, 1.0, 100.0, 101.0, 0.0);
    assert!(core.poll_pnl(&shared).is_some());
    // Same inputs → no callback.
    assert!(core.poll_pnl(&shared).is_none());
}

#[test]
fn poll_pnl_falls_back_to_account_level_without_market_data() {
    // A client that asks only for P&L never subscribes to market data, so no
    // position has a live quote (con_id_to_instrument is empty and every
    // position hits `continue`). poll_pnl must then emit the venue's
    // account-level P&L instead of returning None forever.
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(21).unwrap();

    // Open position, but NO instrument mapping and NO quote pushed.
    shared.portfolio.set_position_info(PositionInfo {
        con_id: 756733,
        position: 10.0,
        avg_cost: (700.00 * PRICE_SCALE_F) as i64,
        symbol: "SPY".into(),
        sec_type: "STK".into(),
        currency: "USD".into(),
        multiplier: String::new(),
        ..Default::default()
    });

    // Gateway-pushed account-level P&L (from the DailyPnL/UnrealizedPnL/
    // RealizedPnL account-value keys).
    let acct = AccountState {
        daily_pnl: (12.50 * PRICE_SCALE_F) as i64,
        unrealized_pnl: (35.00 * PRICE_SCALE_F) as i64,
        realized_pnl: (4.00 * PRICE_SCALE_F) as i64,
        ..Default::default()
    };
    shared.portfolio.set_account(&acct);

    let update = core.poll_pnl(&shared).expect("callback must fire from account-level P&L");
    assert_eq!(update.req_id, 21);
    assert!((update.daily_pnl - 12.50).abs() < 1e-6, "daily={}", update.daily_pnl);
    assert!((update.unrealized_pnl - 35.00).abs() < 1e-6, "unreal={}", update.unrealized_pnl);
    assert!((update.realized_pnl - 4.00).abs() < 1e-6, "real={}", update.realized_pnl);
}

/// A contract this session never quoted is valued from what the venue
/// states for it: a price, and what it was worth at midnight. The account
/// total is computed from those rather than deferred elsewhere.
#[test]
fn the_overnight_leg_is_valued_at_the_mark_the_venue_states() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(31).unwrap();

    // Quoted at 101.25 now, with no locally derived previous close. The venue
    // states the mark it closed the contract at, which is what the
    // overnight leg is valued against.
    seed_pnl_position(&core, &shared, 5001, 0, 10.0, 100.00, 101.25, 0.0);
    shared.portfolio.set_midnight_seeds("PLR.31".into(), vec![MidnightSeed {
        con_id: 5001,
        qty_midnight: Some(10.0),
        cost_midnight: None,
        qty_traded: Some(0.0),
        money_traded: 0.0,
        realized_pnl: 2.50,
    }]);
    shared.portfolio.set_venue_prices([(5001i64, "100.00".to_string())].into());

    // Account-level figures that would be visible if the client fell back.
    shared.portfolio.set_account(&AccountState {
        daily_pnl: (999.0 * PRICE_SCALE_F) as i64,
        unrealized_pnl: (999.0 * PRICE_SCALE_F) as i64,
        realized_pnl: (999.0 * PRICE_SCALE_F) as i64,
        ..Default::default()
    });

    let update = core.poll_pnl(&shared).expect("callback must fire");
    // 10 × 101.25 now, against 10 × 100.00 the venue marked it at overnight.
    assert!((update.daily_pnl - 12.50).abs() < 1e-6, "daily={}", update.daily_pnl);
    assert!((update.unrealized_pnl - 12.50).abs() < 1e-6, "unreal={}", update.unrealized_pnl);
    assert!((update.realized_pnl - 2.50).abs() < 1e-6, "real={}", update.realized_pnl);
}

/// The venue states what a position was worth at midnight. The client's own
/// answer is the overnight size times a previous close, which it holds for
/// no contract it never quoted and which is the wrong figure whenever the
/// two disagree. The stated one wins.
#[test]
fn the_venues_midnight_value_beats_the_clients_previous_close() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(32).unwrap();

    // Quoted at 101.00 with a previous close of 90.00.
    seed_pnl_position(&core, &shared, 7001, 0, 10.0, 100.00, 101.00, 90.00);
    shared.portfolio.set_midnight_seeds("PLR.32".into(), vec![MidnightSeed {
        con_id: 7001,
        qty_midnight: Some(10.0),
        cost_midnight: Some(1000.00),
        qty_traded: Some(0.0),
        money_traded: 0.0,
        realized_pnl: 0.0,
    }]);

    let update = core.poll_pnl(&shared).expect("callback must fire");
    // 1010.00 against the stated 1000.00, not against 10 × 90.00.
    assert!((update.daily_pnl - 10.0).abs() < 1e-6, "daily={}", update.daily_pnl);
}

/// The table is kept as the venue wrote it and read where it is used, so text
/// that is not a price costs its own contract a valuation. What it must not do
/// is leave the rest of the account reported as the whole of it: the realized
/// figure accrues for a contract that cannot be marked while the daily and
/// unrealized ones do not, so a partial sum does not even agree with itself.
/// The account goes to the venue's figures instead.
#[test]
fn a_mark_that_does_not_read_as_a_number_sends_the_account_to_the_venue() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(33).unwrap();

    for (i, con_id) in [6001i64, 6002, 6003].into_iter().enumerate() {
        seed_pnl_position(&core, &shared, con_id, i as u32, 1.0, 50.00, 51.00, 0.0);
    }
    shared.portfolio.set_midnight_seeds("PLR.33".into(), vec![
        MidnightSeed {
            con_id: 6001, qty_midnight: Some(1.0), cost_midnight: None,
            qty_traded: Some(0.0), money_traded: 0.0, realized_pnl: 2.00,
        },
        MidnightSeed {
            con_id: 6002, qty_midnight: Some(1.0), cost_midnight: None,
            qty_traded: Some(0.0), money_traded: 0.0, realized_pnl: 3.00,
        },
        MidnightSeed {
            con_id: 6003, qty_midnight: Some(1.0), cost_midnight: None,
            qty_traded: Some(0.0), money_traded: 0.0, realized_pnl: 4.00,
        },
    ]);
    shared.portfolio.set_venue_prices([
        (6001i64, "50.00".to_string()),
        (6002i64, "n/a".to_string()),
        // Nothing is worth nothing. A table that has yet to mark a contract
        // says so with a zero, and valuing the holding at it reports the
        // whole position as having gone to nought overnight.
        (6003i64, "0.00".to_string()),
    ].into());

    // What the venue says about the account as a whole, which is complete by
    // construction where a sum built here is not.
    shared.portfolio.set_account(&AccountState {
        daily_pnl: (12.0 * PRICE_SCALE_F) as i64,
        unrealized_pnl: (34.0 * PRICE_SCALE_F) as i64,
        realized_pnl: (9.0 * PRICE_SCALE_F) as i64,
        ..Default::default()
    });

    let update = core.poll_pnl(&shared).expect("callback must fire");
    assert!((update.daily_pnl - 12.0).abs() < 1e-6,
        "two contracts could not be marked, so the venue's total stands, daily={}",
        update.daily_pnl);
    assert!((update.unrealized_pnl - 34.0).abs() < 1e-6, "unreal={}", update.unrealized_pnl);
    assert!((update.realized_pnl - 9.0).abs() < 1e-6, "real={}", update.realized_pnl);
    assert_eq!(
        shared.portfolio.venue_price(6002).as_deref(), Some("n/a"),
        "the table holds what the venue wrote; reading it is the caller's job",
    );
}

#[test]
fn poll_pnl_prefers_quotes_over_account_level_when_priced() {
    // When market data IS subscribed, the per-position quote synthesis wins;
    // the account-level fallback must not override it.
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(22).unwrap();

    // Priced position: 1 share, avg 100, last 101 → daily/unrealized = 1.00.
    seed_pnl_position(&core, &shared, 1, 0, 1.0, 100.0, 101.0, 0.0);

    // Divergent account-level values that must be ignored while priced.
    let acct = AccountState {
        daily_pnl: (999.0 * PRICE_SCALE_F) as i64,
        unrealized_pnl: (999.0 * PRICE_SCALE_F) as i64,
        ..Default::default()
    };
    shared.portfolio.set_account(&acct);

    let update = core.poll_pnl(&shared).expect("callback must fire");
    assert!((update.daily_pnl - 1.0).abs() < 1e-6, "daily={}", update.daily_pnl);
    assert!((update.unrealized_pnl - 1.0).abs() < 1e-6, "unreal={}", update.unrealized_pnl);
}

// ── what a single-position P&L poll reports ──

#[test]
fn poll_pnl_single_routes_quote_by_con_id() {
    // #168 (bug 3): two subscribed instruments, different prices — each req_id
    // must see the price of its own con_id, not the first non-zero quote.
    let core = ClientCore::new();
    let shared = SharedState::new();

    seed_pnl_position(&core, &shared, 111, 0, 1.0, 100.0, 105.0, 0.0);  // SPY
    seed_pnl_position(&core, &shared, 222, 1, 1.0, 200.0, 210.0, 0.0);  // QQQ

    core.subscribe_pnl_single(50, 111);
    core.subscribe_pnl_single(51, 222);

    let updates = core.poll_pnl_single(&shared);
    assert_eq!(updates.len(), 2);

    let spy = updates.iter().find(|u| u.req_id == 50).expect("SPY update");
    let qqq = updates.iter().find(|u| u.req_id == 51).expect("QQQ update");
    // Unrealized = qty × (last - avg_cost). SPY: 1×(105-100)=5; QQQ: 1×(210-200)=10.
    assert!((spy.unrealized_pnl - 5.0).abs() < 1e-6);
    assert!((qqq.unrealized_pnl - 10.0).abs() < 1e-6);
    // Value = qty × last. SPY: 105; QQQ: 210.
    assert!((spy.value - 105.0).abs() < 1e-6);
    assert!((qqq.value - 210.0).abs() < 1e-6);
}

#[test]
fn poll_pnl_single_intraday_opened_position() {
    // #168 (bug 1): daily_pnl must be computed, not hardcoded 0.
    // No seed → money_traded synthesized, daily collapses to unrealized.
    let core = ClientCore::new();
    let shared = SharedState::new();
    seed_pnl_position(&core, &shared, 756733, 0, 1.0, 735.00, 735.07, 0.0);
    core.subscribe_pnl_single(42, 756733);

    let updates = core.poll_pnl_single(&shared);
    assert_eq!(updates.len(), 1);
    let u = &updates[0];
    assert_eq!(u.req_id, 42);
    assert!((u.daily_pnl - 0.07).abs() < 1e-6, "daily={}", u.daily_pnl);
    assert!((u.unrealized_pnl - 0.07).abs() < 1e-6);
    assert!((u.realized_pnl - 0.0).abs() < 1e-6);
}

#[test]
fn poll_pnl_single_overnight_position_with_seed() {
    // #168 (bug 2): realized_pnl must come from the seed, not hardcoded 0.
    let core = ClientCore::new();
    let shared = SharedState::new();
    seed_pnl_position(&core, &shared, 756733, 0, 10.0, 700.00, 735.00, 730.00);
    shared.portfolio.set_midnight_seeds(String::new(), vec![MidnightSeed {
        con_id: 756733,
        qty_midnight: Some(10.0),
        cost_midnight: None,
        qty_traded: None,
        money_traded: 0.0,
        realized_pnl: 12.34,
    }]);
    core.subscribe_pnl_single(99, 756733);

    let updates = core.poll_pnl_single(&shared);
    assert_eq!(updates.len(), 1);
    let u = &updates[0];
    // daily = 10×735 − 10×730 − 0 = 50
    assert!((u.daily_pnl - 50.0).abs() < 1e-6);
    // unrealized = 10 × (735 − 700) = 350
    assert!((u.unrealized_pnl - 350.0).abs() < 1e-6);
    assert!((u.realized_pnl - 12.34).abs() < 1e-6);
}

#[test]
fn poll_pnl_single_change_detection_suppresses_duplicate() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    seed_pnl_position(&core, &shared, 1, 0, 1.0, 100.0, 101.0, 0.0);
    core.subscribe_pnl_single(7, 1);
    assert_eq!(core.poll_pnl_single(&shared).len(), 1);
    // Same inputs → no emit.
    assert!(core.poll_pnl_single(&shared).is_empty());
}

#[test]
fn poll_pnl_single_unsubscribe_clears_cache() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    seed_pnl_position(&core, &shared, 1, 0, 1.0, 100.0, 101.0, 0.0);
    core.subscribe_pnl_single(7, 1);
    let _ = core.poll_pnl_single(&shared);
    core.unsubscribe_pnl_single(7);
    // Re-subscribing with same req_id must re-emit (cache cleared on unsubscribe).
    core.subscribe_pnl_single(7, 1);
    assert_eq!(core.poll_pnl_single(&shared).len(), 1);
}
/// Adaptive, algo and what-if orders leave `build_order_request` through
/// their own branches, and each still reaches the extended-attribute
/// block: outside-RTH, a parent link, an OCA group and a non-DAY tif are
/// carried on all of them rather than accepted and dropped. Asserted on
/// the request the API layer produces, which is where a drop would occur.
#[test]
fn the_algo_order_types_carry_the_attributes_the_caller_set() {
    let base = ApiOrder {
        action: "BUY".into(),
        total_quantity: 100.0,
        order_type: "LMT".into(),
        lmt_price: 150.0,
        tif: "GTC".into(),
        outside_rth: true,
        parent_id: 42,
        oca_group: "bracket_1".into(),
        ..Default::default()
    };
    let cases = [
        ("adaptive", ApiOrder { algo_strategy: "Adaptive".into(), ..base.clone() }),
        ("algo", ApiOrder { algo_strategy: "Vwap".into(), ..base.clone() }),
        ("what-if", ApiOrder { what_if: true, ..base.clone() }),
    ];
    for (label, order) in cases {
        let cmd = ClientCore::build_order_request(&order, 7, 0, None)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        let ControlCommand::Order(OrderRequest::SubmitEx { tif, attrs, .. }) = cmd else {
            panic!("{label} must route through the shared extended submission");
        };
        assert!(attrs.outside_rth, "{label} dropped outside RTH");
        assert_eq!(attrs.parent_id, 42, "{label} dropped the parent link");
        assert_eq!(attrs.oca_group_str, "bracket_1", "{label} dropped the OCA group");
        assert_eq!(tif, b'1', "{label} was submitted DAY rather than GTC");
    }
}

mod contract_gate_tests {
    use super::super::ClientCore;

    /// A currency pair carries no expiry, strike or right, so an order names it
    /// completely with symbol, currency, security type and destination. Options
    /// and futures do not, and an order for one would go out saying nothing
    /// about which contract it meant.
    #[test]
    fn cash_is_admitted_and_the_underspecified_types_are_not() {
        assert!(ClientCore::validate_order_contract(0, "CASH", "").is_ok(), "an FX pair is fully named");

        // A spread's legs are carried and not sent, so an order for one would
        // be an order for something else. Refused until they are encoded.
        // An instruction that is carried and not sent makes the order a
        // different one, so it is refused by name.
        use crate::types::model::Order as ApiOrder;
        let plain = ApiOrder::default();
        assert!(ClientCore::validate_supported_instructions(&plain).is_ok(), "a plain order is fine");
        // Sent now, so no longer refused.
        for (label, o) in [
            ("volatility", ApiOrder { volatility: 0.25, ..ApiOrder::default() }),
            ("volatility type", ApiOrder { volatility_type: 2, ..ApiOrder::default() }),
            ("scale", ApiOrder { scale_init_level_size: 100, scale_price_increment: 0.05,
                                 ..ApiOrder::default() }),
            ("delta neutral", ApiOrder { delta_neutral_order_type: "MKT".into(),
                                         ..ApiOrder::default() }),
            ("percent offset", ApiOrder { percent_offset: 0.5, ..ApiOrder::default() }),
            ("not held", ApiOrder { not_held: true, ..ApiOrder::default() }),
            ("open/close", ApiOrder { open_close: "O".into(), ..ApiOrder::default() }),


        ] {
            assert!(ClientCore::validate_supported_instructions(&o).is_ok(), "{label} is sent");
        }
        // Sent now, so accepted.
        for (label, o) in [
            ("hedge", ApiOrder { hedge_type: "B".into(), hedge_param: "1.5".into(),
                                 ..ApiOrder::default() }),
            ("short sale", ApiOrder { short_sale_slot: 2,
                                      designated_location: "IBKR".into(),
                                      exempt_code: 3, ..ApiOrder::default() }),
        ] {
            assert!(ClientCore::validate_supported_instructions(&o).is_ok(), "{label} is sent");
        }

        // Still refused: an instruction that cannot be acted on as given.
        for (label, mut o) in [
            ("hedge param on a kind that takes none",
             ApiOrder { hedge_type: "D".into(), hedge_param: "1.5".into(), ..ApiOrder::default() }),
            ("delta neutral with no order type",
             ApiOrder { delta_neutral_con_id: 265598, ..ApiOrder::default() }),
        ] {
            o.action = "BUY".into();
            let err = ClientCore::validate_supported_instructions(&o)
                .expect_err("{label} must be refused, not silently dropped");
            assert!(err.contains("not sent"), "{label}: {err}");
        }

        assert!(ClientCore::validate_combo_legs("STK", 0).is_ok(), "an ordinary contract has none");
        assert!(ClientCore::validate_combo_legs("BAG", 2).is_ok(), "a combination states its legs");
        assert!(ClientCore::validate_combo_legs("BAG", 0).is_err(), "a combination with none is refused");
        assert!(ClientCore::validate_order_contract(0, "cash", "").is_ok(), "and the check is case-insensitive");
        assert!(ClientCore::validate_order_contract(0, "STK", "").is_ok());
        assert!(ClientCore::validate_order_contract(0, "", "").is_ok());

        // One of a chain or one of a series has to say which one.
        for st in ["OPT", "FUT", "FOP", "WAR"] {
            assert!(
                ClientCore::validate_order_contract(0, st, "20260619|230|C|100").is_ok(),
                "{st} with an identity names one contract",
            );
            let err = ClientCore::validate_order_contract(0, st, "")
                .expect_err("and without one it names a whole chain");
            assert!(err.contains(st), "the refusal names the type: {err}");
        }
        // Everything else is named completely by its symbol and the contract id
        // and local symbol that travel with it. Requiring an expiry or a strike
        // of a kind that has neither refused it forever: an index and a crypto
        // pair could not be ordered at all.
        for st in ["IND", "CFD", "CRYPTO", "BOND", "CMDTY", "FUND"] {
            assert!(
                ClientCore::validate_order_contract(0, st, "").is_ok(),
                "{st} is named without an expiry or a strike",
            );
        }
        // A combination states its legs on the order, so it needs no identity
        // here. Stating none at all is refused by the leg check instead.
        for st in ["BAG", "COMBO"] {
            assert!(ClientCore::validate_order_contract(0, st, "").is_ok(), "{st} names its legs");
            assert!(ClientCore::validate_combo_legs(st, 0).is_err(), "{st} with no legs");
            assert!(ClientCore::validate_combo_legs(st, 2).is_ok(), "{st} with legs");
        }
    }

}
mod exchange_mask_provenance_tests {
    use crate::bridge::SharedState;

    /// The letters a quote's bid, ask and last are attributed to come from bit
    /// numbers the venue assigns. This client's own list can only guess at
    /// them, and the guess must be marked as one: a table that renders
    /// confidently is indistinguishable from one that knows.
    #[test]
    fn the_built_in_exchange_table_is_marked_as_a_guess() {
        let shared = SharedState::new();
        // Nothing has been received, so nothing claims to have been.
        assert!(!shared.reference.smart_components_are_provisional());

        shared.reference.note_smart_components_provisional(true);
        assert!(shared.reference.smart_components_are_provisional());
    }

    /// Two contracts a caller would call different have to look different
    /// here, or an order on one is sent under the other's id.
    #[test]
    fn a_description_names_one_contract_and_no_other() {
        use crate::types::model::Contract as ApiContract;
        use super::super::ClientCore;
        let spy = |exchange: &str| ApiContract {
            symbol: "SPY".into(), sec_type: "STK".into(), exchange: exchange.into(),
            currency: "USD".into(), ..Default::default()
        };
        let core = ClientCore::new();
        let key = ClientCore::description_key(&spy("SMART"));
        assert!(core.named_for(&key).is_none(), "nothing is known before the venue answers");

        let mut answered = spy("SMART");
        answered.con_id = 756733;
        core.remember_named(key.clone(), answered);
        assert_eq!(core.named_for(&key).map(|c| c.con_id), Some(756733));

        // The same symbol somewhere else is a different contract, and asking
        // under it must not find the first one.
        assert!(core.named_for(&ClientCore::description_key(&spy("ARCA"))).is_none());

        // So is the same symbol in another currency, which the identity carries.
        let mut abroad = spy("SMART");
        abroad.currency = "EUR".into();
        assert!(core.named_for(&ClientCore::description_key(&abroad)).is_none());

        // And a description that stated no currency at all is its own. The
        // identity folds "" and USD together, which is right for the slot an
        // order goes through: here it would let a lookup answered with a
        // listing in another currency satisfy an order that asked for USD.
        let mut unstated = spy("SMART");
        unstated.currency = String::new();
        assert!(
            core.named_for(&ClientCore::description_key(&unstated)).is_none(),
            "saying nothing about the currency is not the same as saying USD",
        );
    }
}

/// A halt changes what every other tick in a quote means: the prices standing
/// are the ones from before the venue stopped, not a market anyone can deal
/// on. It arrives on the trading-status tick and is written into the quote and
/// compared against the last one. Caching it without emitting a tick consumes
/// the transition, and it cannot be delivered afterwards.
#[test]
fn a_halt_the_venue_states_reaches_the_caller() {
    let core = ClientCore::new();
    let shared = SharedState::new();

    let trading = Quote { last: (735.00 * PRICE_SCALE_F) as i64, ..Default::default() };
    shared.market.push_quote(0, &trading);
    let first = core.poll_instrument_ticks(&shared, 0, 11);
    assert!(
        first.generic_ticks.is_empty(),
        "a contract that has not stopped states no halt",
    );

    shared.market.push_quote(0, &Quote { halted: 1, ..trading });
    let halted = core.poll_instrument_ticks(&shared, 0, 11);
    let tick = halted.generic_ticks.first().expect("the halt is delivered");
    assert_eq!(tick.tick_type, TICK_HALTED);
    assert_eq!(tick.value, 1.0);
    assert_eq!(tick.req_id, 11);
    assert!(!tick.is_price, "a halt is not a price");

    // And it is not repeated while nothing about it has changed.
    assert!(
        core.poll_instrument_ticks(&shared, 0, 11).generic_ticks.is_empty(),
        "a halt that is still standing is not restated",
    );

    // Trading resumes, and that is a transition too.
    shared.market.push_quote(0, &Quote { halted: 0, ..trading });
    let resumed = core.poll_instrument_ticks(&shared, 0, 11);
    assert_eq!(resumed.generic_ticks.first().expect("the resume is delivered").value, 0.0);
}

/// The summary reports what the venue stated, under the venue's names.
/// Matched against a list of sixteen names kept here instead, "All" — the
/// the venue's word for every figure it holds — matched none of them and came
/// back empty, and the figures that were not on that list went with it.
#[test]
fn an_account_summary_reports_every_figure_the_venue_stated() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    for (key, value, currency) in [
        ("NetLiquidation", "75425.51", "USD"),
        ("AccruedCash", "12.40", "USD"),
        ("SMA", "38000.00", "USD"),
        ("FullInitMarginReq", "1364.01", "USD"),
        ("TotalCashValue", "5000.00", "EUR"),
    ] {
        shared.portfolio.note_account_value(key, value, currency);
    }

    core.subscribe_account_summary(3, "All").unwrap();
    let batch = core.prepare_account_summary(&shared, "DU1").expect("a summary");
    assert_eq!(batch.req_id, 3);
    let names: Vec<&str> = batch.entries.iter().map(|e| e.tag.as_str()).collect();
    for stated in ["NetLiquidation", "AccruedCash", "SMA", "FullInitMarginReq"] {
        assert!(names.contains(&stated), "{stated} missing from {names:?}");
    }

    // A figure stated in more than one currency is stated in each of them.
    core.unsubscribe_account_summary(3);
    core.subscribe_account_summary(4, "TotalCashValue").unwrap();
    let batch = core.prepare_account_summary(&shared, "DU1").expect("a summary");
    assert_eq!(batch.entries.len(), 1);
    assert_eq!(batch.entries[0].tag, "TotalCashValue");
    assert_eq!(batch.entries[0].currency, "EUR");

    // And a tag the venue never stated reports nothing rather than a zero.
    core.unsubscribe_account_summary(4);
    core.subscribe_account_summary(5, "Cushion").unwrap();
    let batch = core.prepare_account_summary(&shared, "DU1").expect("a summary");
    assert!(batch.entries.is_empty(), "{:?}", batch.entries.len());
}

/// One slot serves each of these subscriptions. A second asker under another
/// request is refused rather than handed the slot, which took the updates
/// away from the first caller without a word to either one. The first
/// subscription keeps receiving, and asking again under the id that holds the
/// slot is not a second subscription.
#[test]
fn a_second_pnl_or_summary_subscription_is_refused_not_silenced() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    shared.portfolio.set_account(&crate::types::AccountState::default());

    core.subscribe_pnl(7).unwrap();
    let second = core.subscribe_pnl(8);
    let why = second.expect_err("the slot is held, so a second asker is refused");
    assert_eq!(why.code, Refusal::VALIDATION);
    assert!(
        why.message.contains("request 7"),
        "the refusal names the holder: {}", why.message,
    );
    core.subscribe_pnl(7).unwrap_or_else(|e| panic!("asking again under the holder is allowed: {e:?}"));
    assert_eq!(
        core.poll_pnl(&shared).map(|u| u.req_id), Some(7),
        "the first subscription still receives",
    );

    core.subscribe_account_summary(3, "All").unwrap();
    let second = core.subscribe_account_summary(4, "Cushion");
    let why = second.expect_err("the summary slot is held too");
    assert_eq!(why.code, Refusal::VALIDATION);
    assert!(
        why.message.contains("request 3"),
        "the refusal names the holder: {}", why.message,
    );
    core.subscribe_account_summary(3, "NetLiquidation").unwrap_or_else(|e| {
        panic!("asking again under the holder is allowed: {e:?}")
    });
    assert!(
        core.prepare_account_summary(&shared, "DU1").is_some(),
        "the first subscription still receives",
    );

    // A cancelled subscription frees the slot for another.
    core.unsubscribe_pnl(7);
    core.subscribe_pnl(8).unwrap();
    core.unsubscribe_account_summary(3);
    core.subscribe_account_summary(4, "Cushion").unwrap();
}

/// A quote is per unit and a contract may be worth many of them. Valued from
/// the price alone, an option holding came out at a hundredth of what it is
/// worth and the account total with it, so such a position goes to the venue's
/// own figures instead.
#[test]
fn an_option_holding_is_not_valued_from_a_per_unit_price() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl(41).unwrap();

    seed_pnl_position(&core, &shared, 7001, 0, 2.0, 3.00, 4.00, 3.00);
    shared.portfolio.set_position_info(PositionInfo {
        con_id: 7001,
        position: 2.0,
        avg_cost: (300.0 * PRICE_SCALE_F) as i64,
        symbol: "SPY   260320C00500000".into(),
        sec_type: "OPT".into(),
        currency: "USD".into(),
        multiplier: "100".into(),
        ..Default::default()
    });
    shared.portfolio.set_account(&AccountState {
        daily_pnl: (200.0 * PRICE_SCALE_F) as i64,
        unrealized_pnl: (200.0 * PRICE_SCALE_F) as i64,
        ..Default::default()
    });

    let update = core.poll_pnl(&shared).expect("callback must fire");
    assert!((update.daily_pnl - 200.0).abs() < 1e-6,
        "the venue's total, not two dollars of per-unit move, daily={}",
        update.daily_pnl);
}

/// This subscription does not depend on a market-data one. Answered only from
/// a live quote, a caller who never asked for market data heard nothing at all
/// and was told nothing either, though the venue states its own mark for every
/// position it reports.
#[test]
fn a_position_pnl_is_answered_without_a_market_data_subscription() {
    let core = ClientCore::new();
    let shared = SharedState::new();
    core.subscribe_pnl_single(9, 8001);

    // No entry in con_id_to_instrument: nothing here subscribed to quotes.
    shared.portfolio.set_position_info(PositionInfo {
        con_id: 8001,
        position: 10.0,
        avg_cost: (100.0 * PRICE_SCALE_F) as i64,
        symbol: "SYM8001".into(),
        sec_type: "STK".into(),
        currency: "USD".into(),
        multiplier: String::new(),
        market_price: (105.0 * PRICE_SCALE_F) as i64,
        market_value: (1050.0 * PRICE_SCALE_F) as i64,
        unrealized_pnl: (50.0 * PRICE_SCALE_F) as i64,
        unrealized_stated: true,
        realized_pnl: 0,
    });
    shared.portfolio.set_midnight_seeds(String::new(), vec![MidnightSeed {
        con_id: 8001,
        qty_midnight: Some(10.0),
        cost_midnight: Some(1000.0),
        qty_traded: Some(0.0),
        money_traded: 0.0,
        realized_pnl: 0.0,
    }]);

    let updates = core.poll_pnl_single(&shared);
    let update = updates.first().expect("the venue's mark answers it");
    assert_eq!(update.req_id, 9);
    assert!((update.pos - 10.0).abs() < 1e-6);
    assert!((update.value - 1050.0).abs() < 1e-6, "value={}", update.value);
    assert!((update.unrealized_pnl - 50.0).abs() < 1e-6, "unreal={}", update.unrealized_pnl);
    assert!((update.daily_pnl - 50.0).abs() < 1e-6, "daily={}", update.daily_pnl);
}

#[test]
fn news_is_asked_for_from_the_providers_the_logon_named() {
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let core = ClientCore::new();
    let shared = SharedState::new();
    shared.reference.set_news_providers(vec![
        crate::types::NewsProvider { code: "DJNL".into(), name: "Dow Jones".into() },
        crate::types::NewsProvider { code: "BRFUPDN".into(), name: "Briefing".into() },
    ]);

    // Every provider named on this attempt, and nothing if news was not asked
    // for. The register also emits its own commands, which are not these.
    // A contract of its own each time. The venue is asked for the headlines
    // once per contract, so asking again for one already asked about would
    // answer nothing whatever the entry said, and this is about the entry.
    let next = std::cell::Cell::new(265598i64);
    let asked = |tick_list: &str| -> Option<String> {
        let con_id = next.get();
        next.set(con_id + 1);
        let _ = core.register_mkt_data(
            &shared, &tx, con_id, con_id, "SPY", "SMART", "STK", "USD", "", 0.0, "", "",
            false, false, tick_list, 0,
        );
        let mut named = None;
        while let Ok(cmd) = rx.try_recv() {
            if let ControlCommand::SubscribeNews { providers, .. } = cmd {
                named = Some(providers);
            }
        }
        named
    };

    assert_eq!(asked("1292"), None, "1292 is not 292");
    assert_eq!(asked("100,101"), None, "nor is anything else");
    assert_eq!(
        asked("292").as_deref(), Some("DJNL*BRFUPDN"),
        "the providers the logon named",
    );

    // A caller naming its own set overrides that; emptying it returns to the
    // logon's answer.
    core.set_news_providers("BRFG");
    assert_eq!(asked("292").as_deref(), Some("BRFG"));
    core.set_news_providers("");
    assert_eq!(asked("292").as_deref(), Some("DJNL*BRFUPDN"));
}

/// A mask with bits set and no letters to show for them is one the venue has
/// not named its exchanges for yet. Caching it as delivered leaves it equal to
/// the next mask, so it is never rendered again once the names arrive and the
/// quote's exchange is lost for the life of the subscription.
#[test]
fn an_exchange_mask_is_rendered_once_the_venue_names_its_bits() {
    let core = ClientCore::new();
    let shared = SharedState::new();

    shared.market.push_quote(0, &Quote {
        bid: (150.0 * PRICE_SCALE_F) as i64,
        bid_exch_mask: 0b101,
        ..Default::default()
    });
    let before = core.poll_instrument_ticks(&shared, 0, 5);
    assert!(
        !before.string_ticks.iter().any(|t| t.tick_type == TICK_BID_EXCHANGE),
        "nothing names those bits yet, so nothing is stated about them",
    );

    shared.reference.set_smart_components(vec![
        SmartComponent { bit_number: 0, exchange: "ARCA".into(), exchange_letter: "P".into() },
        SmartComponent { bit_number: 2, exchange: "NASDAQ".into(), exchange_letter: "Q".into() },
    ]);
    let after = core.poll_instrument_ticks(&shared, 0, 5);
    let rendered = after.string_ticks.iter()
        .find(|t| t.tick_type == TICK_BID_EXCHANGE)
        .expect("the same mask is rendered once its bits are named");
    assert_eq!(rendered.value, "PQ");
}

/// An adjustable stop states what the contract states, not only what the
/// order states.
///
/// Its legs, its listing exchange and the contract it hedges against are
/// stated on the contract rather than on the order, and every other order type
/// picks them up on the way through. Built from the order's own attributes
/// alone, an adjustable stop on a combination reached the encoder with no legs
/// at all.
#[test]
fn an_adjustable_stop_carries_what_the_contract_states() {
    let order = ApiOrder {
        action: "SELL".into(),
        total_quantity: 1.0,
        order_type: "STP".into(),
        aux_price: 11.0,
        tif: "DAY".into(),
        adjusted_order_type: "TRAIL".into(),
        adjusted_stop_price: 11.5,
        trigger_price: 12.0,
        ..Default::default()
    };
    let contract = crate::types::model::Contract {
        symbol: "SPX".into(),
        sec_type: "BAG".into(),
        exchange: "SMART".into(),
        currency: "USD".into(),
        primary_exchange: "CBOE".into(),
        combo_legs: vec![
            crate::types::model::ComboLeg {
                con_id: 111, ratio: 1, action: "BUY".into(), exchange: "SMART".into(),
                ..Default::default()
            },
            crate::types::model::ComboLeg {
                con_id: 222, ratio: 1, action: "SELL".into(), exchange: "SMART".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let cmd = ClientCore::build_order_request(&order, 7, 0, Some(&contract)).unwrap();
    let ControlCommand::Order(OrderRequest::SubmitEx { attrs, .. }) = cmd else {
        panic!("an adjustable stop routes through the shared extended submission");
    };
    assert_eq!(attrs.combo_legs.len(), 2, "the legs the contract named");
    assert_eq!(attrs.combo_legs[0].con_id, 111);
    assert_eq!(attrs.combo_legs[1].con_id, 222);
    assert_eq!(attrs.primary_exchange, "CBOE", "the listing exchange it names");
}

/// A passive relative order is taken as the caller states it: the offset on
/// the auxiliary price, the cap on the limit price, which is the pair the
/// venue's own shape for the type is built from.
#[test]
fn a_passive_relative_order_is_built_from_the_prices_it_states() {
    let scale = crate::types::PRICE_SCALE;
    let order = ApiOrder {
        action: "BUY".into(),
        total_quantity: 100.0,
        order_type: "PASSV REL".into(),
        aux_price: 0.01,
        lmt_price: 150.0,
        tif: "DAY".into(),
        ..Default::default()
    };
    ClientCore::validate_order(&order, "DU123").unwrap();
    let cmd = ClientCore::build_order_request(&order, 7, 0, None).unwrap();
    let ControlCommand::Order(OrderRequest::SubmitEx { kind, .. }) = cmd else {
        panic!("a passive relative order routes through the shared extended submission");
    };
    let crate::types::OrderKind::PassiveRel { offset, price_cap } = kind else {
        panic!("built as the wrong kind: {kind:?}");
    };
    assert_eq!(offset, scale / 100, "the offset the caller stated");
    assert_eq!(price_cap, 150 * scale, "the cap the caller stated");
}

/// A pegged-to-best order is taken as the caller states it: one price, on
/// the limit-price field. Stated without one there is nothing to send, and
/// the order is refused rather than sent malformed.
#[test]
fn a_peg_best_order_is_built_from_the_price_it_states() {
    let scale = crate::types::PRICE_SCALE;
    let order = ApiOrder {
        action: "BUY".into(),
        total_quantity: 100.0,
        order_type: "PEG BEST".into(),
        lmt_price: 150.0,
        tif: "DAY".into(),
        ..Default::default()
    };
    ClientCore::validate_order(&order, "DU123").unwrap();
    let cmd = ClientCore::build_order_request(&order, 7, 0, None).unwrap();
    let ControlCommand::Order(OrderRequest::SubmitEx { kind, .. }) = cmd else {
        panic!("a pegged-to-best order routes through the shared extended submission");
    };
    let crate::types::OrderKind::PegBest { price } = kind else {
        panic!("built as the wrong kind: {kind:?}");
    };
    assert_eq!(price, 150 * scale, "the price the caller stated");

    let unpriced = ApiOrder { lmt_price: 0.0, ..order };
    let err = ClientCore::validate_order(&unpriced, "DU123").unwrap_err();
    assert!(err.contains("lmt_price"), "a pegged-to-best order with no price is refused: {err}");
}

/// A snapshot ends on the venue having stated what one is made of, or on the
/// wait running out from when it was ASKED FOR. Waiting on the quiet instead
/// ended one on a pause, and never ended one the venue said nothing about.
#[test]
fn a_snapshot_ends_on_the_venue_or_on_the_wait_from_asking() {
    let core = ClientCore::new();
    core.snapshot_reqs.lock().unwrap().insert(1, (std::time::Instant::now(), 0));
    // Bid, ask, last, open — four of the five.
    for kind in [1, 2, 4, 14] {
        core.note_snapshot_tick(1, kind);
        assert!(!core.check_snapshot_done(1), "kind {kind} still leaves one to come");
    }
    core.note_snapshot_tick(1, 9);
    assert!(core.check_snapshot_done(1), "the close was the last of them");
    assert!(!core.check_snapshot_done(1), "and it is only said once");

    // What a kind CARRIED does not matter, only that it came: a pair states
    // its last as minus one and a contract yet to open states its open as
    // nothing, and both are the venue answering.
    core.snapshot_reqs.lock().unwrap().insert(3, (std::time::Instant::now(), 0));
    for kind in [1, 2, 4, 14, 9] {
        core.note_snapshot_tick(3, kind);
    }
    assert!(core.check_snapshot_done(3), "every kind was stated, whatever it said");

    // A contract the venue says nothing about is let go of on the wait, and
    // the wait is measured from asking — so one that never heard anything is
    // swept rather than held for ever.
    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(12);
    core.snapshot_reqs.lock().unwrap().insert(2, (long_ago, 0));
    assert!(core.check_snapshot_done(2), "nothing was ever stated, and the wait is up");
    assert!(core.snapshot_reqs.lock().unwrap().is_empty(), "and nothing is left waiting");
}

/// The venue restates the day's executions at every logon, so the same one
/// reaches the record more than once. It is stored once, known by its id.
#[test]
fn an_execution_is_stored_once_under_its_id() {
    let core = ClientCore::new();
    let stated = |id: &str| crate::types::model::Execution { exec_id: id.into(), ..Default::default() };
    for id in ["0001f4e8.1", "0001f4e8.1", "0001f4e8.2"] {
        core.push_execution(-1, Default::default(), stated(id), Default::default());
    }
    let stored = core.snapshot_executions(&Default::default());
    let ids: Vec<&str> = stored.iter().map(|s| s.execution.exec_id.as_str()).collect();
    assert_eq!(ids, ["0001f4e8.1", "0001f4e8.2"], "each execution once, by id");
}

/// A family send that stops partway does not leave what it never sent reading
/// as an order the venue is working.
///
/// An order reads as working here by being tracked with no placement held for
/// it. What did not reach the engine comes out of the hold, so it cannot go out
/// behind the next thing that transmits after the caller was told it did not
/// go — and its record has to come out with it. Left standing, an id nothing
/// ever sent was listed among the open orders and placing under it again
/// revised an order the venue has never been given, beside a parent that did
/// go and is resting there with nothing protecting it.
#[test]
fn a_family_send_that_stops_partway_forgets_what_it_did_not_send() {
    let core = ClientCore::new();
    let leg = |order_id: u64, parent_id: i64| {
        let order = ApiOrder {
            order_id: order_id as i64, action: "BUY".into(), total_quantity: 1.0,
            order_type: "LMT".into(), lmt_price: 100.0, tif: "DAY".into(),
            parent_id, transmit: false, ..Default::default()
        };
        let command = ClientCore::build_order_request(&order, order_id, 0, None)
            .expect("a plain limit order is built");
        (command, order)
    };
    // A parent and two children, each built and kept.
    for (order_id, parent_id) in [(80u64, 0i64), (81, 80), (82, 80)] {
        let (command, order) = leg(order_id, parent_id);
        core.hold_until_transmitted(order_id, parent_id, command);
        core.track_order(order_id, ApiContract::default(), order, 0);
    }

    // The parent goes; the engine is gone by the time the sibling behind it is
    // offered, so neither it nor the order that asked to transmit went.
    let (own, _) = leg(81, 80);
    let mut offered = 0;
    let sent = core.transmit_family(81, 80, own, |_| {
        offered += 1;
        offered == 1
    });

    assert!(sent.is_err(), "the caller is told the family did not all go");
    assert!(
        core.is_working_at_the_venue(80),
        "the parent reached the engine and may be live at the venue",
    );
    assert!(
        !core.is_working_at_the_venue(81),
        "the order that asked to transmit did not reach the engine",
    );
    assert!(
        !core.is_working_at_the_venue(82),
        "nor did the sibling behind it, so neither is an order to withdraw or revise",
    );
}
