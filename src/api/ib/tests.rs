//! The tests for this module.

use super::*;
use crate::api::wrapper::Wrapper;
use crate::types::model::OrderState;

fn an_order(id: i64) -> Order {
    Order { order_id: id, ..Order::limit("BUY", 100.0, 42.5) }
}

/// What the session is told stays, and the last word wins.
///
/// A status is a statement about now, so a second one replaces the first. Kept
/// side by side, a caller reading the list would find an order both working
/// and filled and could not tell which it was.
#[test]
fn the_last_thing_the_venue_said_is_what_is_held() {
    let mut kept = LiveState::default();
    kept.order_status(1, "Submitted", 0.0, 100.0, 0.0, 77, 0, 0.0, 1, "", 0.0);
    assert!(kept.trade(1).unwrap().is_active());
    assert_eq!(kept.open_trades().len(), 1);

    kept.order_status(1, "Filled", 100.0, 0.0, 42.5, 77, 0, 0.0, 1, "", 0.0);
    let trade = kept.trade(1).unwrap();
    assert!(trade.is_done(), "filled is not still working");
    assert_eq!(trade.status.filled, 100.0);
    assert_eq!(trade.status.average_price, 42.5);
    assert_eq!(kept.trades().len(), 1, "one order, not two");
    assert!(kept.open_trades().is_empty());
}

/// An average that arrives as nothing does not erase the one already reported.
///
/// A status after a fill can state no average, and taking it would show an
/// order filled at zero.
#[test]
fn an_average_already_reported_survives_a_status_that_states_none() {
    let mut kept = LiveState::default();
    kept.order_status(2, "Filled", 100.0, 0.0, 42.5, 0, 0, 0.0, 1, "", 0.0);
    kept.order_status(2, "Filled", 100.0, 0.0, 0.0, 0, 0, 0.0, 1, "", 0.0);
    assert_eq!(kept.trade(2).unwrap().status.average_price, 42.5);
}

/// A holding is reported as it stands, so the same contract replaces itself
/// and one reported as zero is gone.
///
/// Appended instead, an account that closed and reopened a position would show
/// it twice and a closed one would show for ever.
#[test]
fn a_holding_is_what_the_account_holds_now() {
    let mut kept = LiveState::default();
    let spy = Contract { con_id: 756733, symbol: "SPY".into(), ..Default::default() };
    kept.position("DU1", &spy, 100.0, 42.0);
    kept.position("DU1", &spy, 250.0, 43.0);
    assert_eq!(kept.positions().len(), 1);
    assert_eq!(kept.positions()[0].quantity, 250.0);

    kept.position("DU1", &spy, 0.0, 0.0);
    assert!(kept.positions().is_empty(), "a holding of none is not a holding");
}

/// A fill lands against its own order as well as in the session's list, so a
/// caller holding one trade sees its fills without matching them up.
#[test]
fn a_fill_lands_against_the_order_it_belongs_to() {
    let mut kept = LiveState::default();
    kept.open_order(3, &Contract::stock("SPY"), &an_order(3), &OrderState::default());
    let spy = Contract::stock("SPY");
    let execution = crate::types::model::Execution { order_id: 3, ..Default::default() };
    kept.exec_details(0, &spy, &execution);

    assert_eq!(kept.fills().len(), 1);
    assert_eq!(kept.trade(3).unwrap().fills.len(), 1);

    // And one against an order this session never saw is still the session's.
    kept.exec_details(0, &spy, &crate::types::model::Execution { order_id: 99, ..Default::default() });
    assert_eq!(kept.fills().len(), 2);
}

/// An account value is keyed by account, tag and currency together.
///
/// Keyed by the tag alone, the same account's dollar and euro cash overwrite
/// each other and a login holding two accounts reports one.
#[test]
fn an_account_value_is_one_per_account_tag_and_currency() {
    let mut kept = LiveState::default();
    kept.update_account_value("NetLiquidation", "1000", "USD", "DU1");
    kept.update_account_value("NetLiquidation", "900", "EUR", "DU1");
    kept.update_account_value("NetLiquidation", "50", "USD", "DU2");
    assert_eq!(kept.account_values().len(), 3);

    kept.update_account_value("NetLiquidation", "1100", "USD", "DU1");
    assert_eq!(kept.account_values().len(), 3, "the same line is replaced");
    let dollars = kept.account_values().into_iter()
        .find(|v| v.account == "DU1" && v.currency == "USD")
        .expect("the line is there");
    assert_eq!(dollars.value, "1100");
}

/// Every change is counted, so a caller can wait for the next one rather than
/// for a length of time.
#[test]
fn a_caller_can_wait_for_the_next_change_rather_than_for_a_while() {
    let mut kept = LiveState::default();
    let before = kept.changes();
    kept.order_status(4, "Submitted", 0.0, 1.0, 0.0, 0, 0, 0.0, 1, "", 0.0);
    assert_ne!(kept.changes(), before);
}

/// Every handler is told everything, and so is the session's own record.
///
/// This is what a queue cannot do: a message taken off a queue is taken off it
/// for everyone, so a second reader gets the messages the first did not happen
/// to take. Here each is told, in the order they were added, and the session
/// keeps its own account regardless of who is listening.
#[test]
fn every_handler_is_told_and_so_is_the_session() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Clone)]
    struct Counting(Arc<AtomicUsize>);
    impl crate::api::wrapper::Wrapper for Counting {
        fn order_status(
            &mut self, _: i64, _: &str, _: f64, _: f64, _: f64, _: i64, _: i64,
            _: f64, _: i64, _: &str, _: f64,
        ) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (first, second) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let mut kept = LiveState::default();
    let mut handlers: Vec<Box<dyn crate::api::ib::Handler + Send>> =
        vec![Box::new(Counting(Arc::clone(&first))), Box::new(Counting(Arc::clone(&second)))];
    let mut fanout = super::Fanout { kept: &mut kept, handlers: &mut handlers };

    crate::api::wrapper::Wrapper::order_status(
        &mut fanout, 1, "Submitted", 0.0, 100.0, 0.0, 0, 0, 0.0, 1, "", 0.0,
    );

    assert_eq!(first.load(Ordering::SeqCst), 1, "the first handler was told");
    assert_eq!(second.load(Ordering::SeqCst), 1, "and so was the second");
    assert_eq!(kept.trade(1).unwrap().status.status, "Submitted", "and the session kept it");
}
