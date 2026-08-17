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

/// A stream is told about its own contract and nobody else's.
///
/// Told about everything, a caller watching one thing filters out the rest —
/// which is the work this exists to do for them.
#[test]
fn a_tick_stream_is_told_about_its_own_contract() {
    use crate::api::wrapper::Wrapper;
    use crate::types::model::TickAttribLast;

    let mut kept = LiveState::default();
    let (watching, watching_rx) = std::sync::mpsc::sync_channel(8);
    let (other, other_rx) = std::sync::mpsc::sync_channel(8);
    kept.stream_ticks(11, watching);
    kept.stream_ticks(22, other);

    let attrib = TickAttribLast::default();
    kept.tick_by_tick_all_last(11, 1, 1_000, 42.5, 100.0, &attrib, "NYSE", "");
    kept.tick_by_tick_all_last(33, 1, 1_001, 99.0, 1.0, &attrib, "NYSE", "");

    let mine: Vec<_> = std::iter::from_fn(|| watching_rx.try_recv().ok()).collect();
    assert_eq!(mine.len(), 1, "the one printed on this contract");
    assert_eq!(mine[0].price, 42.5);
    assert_eq!(mine[0].exchange, "NYSE");
    assert!(other_rx.try_recv().is_err(), "and nothing printed on another");
}

/// A status change and a fill both reach a caller watching orders, and a
/// caller who stopped watching is stopped being sent to.
#[test]
fn order_events_carry_both_kinds_and_forget_a_reader_that_left() {
    use crate::api::wrapper::Wrapper;

    let mut kept = LiveState::default();
    let (to, rx) = std::sync::mpsc::sync_channel(8);
    kept.stream_order_events(to);

    kept.order_status(7, "Submitted", 0.0, 100.0, 0.0, 0, 0, 0.0, 1, "", 0.0);
    kept.exec_details(0, &Contract::stock("SPY"),
                      &crate::types::model::Execution { order_id: 7, ..Default::default() });

    let seen: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(seen.len(), 2, "the status and the fill");
    assert_eq!(seen[0].status, "Submitted");
    assert!(seen[0].fill.is_none(), "a status change is not a fill");
    assert!(seen[1].fill.is_some(), "and a fill is");

    drop(rx);
    kept.order_status(7, "Filled", 100.0, 0.0, 42.5, 0, 0, 0.0, 1, "", 0.0);
    assert_eq!(kept.trade(7).unwrap().status.status, "Filled", "the session still keeps it");
}

/// The names a session defines over the client it dereferences to are the ones
/// it means to define.
///
/// An inherent method is found before a dereferenced one, so any name written
/// on the session hides the client's. Where that is intended it is an
/// improvement — reading what is already held instead of asking again, handing
/// back the order instead of a snapshot. Where it is not, a caller silently
/// gets a different method from the one they read about, and nothing says so.
#[test]
fn shadowed_deliberately() {
    use std::collections::BTreeSet;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let names = |text: &str| -> BTreeSet<String> {
        text.lines()
            .filter_map(|l| l.trim().strip_prefix("pub fn "))
            .filter_map(|l| l.split('(').next())
            .map(str::to_string)
            .collect()
    };
    let session = {
        let text = std::fs::read_to_string(root.join("src/api/session/mod.rs")).expect("the session");
        let at = text.find("impl Client {").expect("the session's own methods");
        names(&text[at..text[at..].find("\n}\n").map_or(text.len(), |e| at + e)])
    };
    let mut client = BTreeSet::new();
    for entry in std::fs::read_dir(root.join("src/api/client")).expect("the client") {
        let path = entry.expect("a readable entry").path();
        if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "tests.rs") {
            client.extend(names(&std::fs::read_to_string(&path).expect("a readable file")));
        }
    }

    /// Written on the session on purpose, each because the session's answer is
    /// the better one. Anything else shadowing is an accident.
    const ON_PURPOSE: [&str; 12] = [
        "bars", "cancel_order", "connect", "disconnect", "is_connected", "is_done",
        "lookup", "place", "place_bracket", "positions", "qualify", "watch",
    ];
    let shadowing: Vec<_> = session
        .intersection(&client)
        .filter(|n| !ON_PURPOSE.contains(&n.as_str()))
        .collect();
    assert!(
        shadowing.is_empty(),
        "these hide a method of the client's and nobody said they meant to: {shadowing:?}",
    );
}

/// A holding is priced as it stands, and one marked to nothing is gone.
///
/// Appended per mark instead, an account that moved would show a row for every
/// price the venue ever stated it at.
#[test]
fn a_holding_is_priced_as_it_stands() {
    use crate::api::wrapper::Wrapper;
    let mut kept = LiveState::default();
    let spy = Contract { con_id: 756733, symbol: "SPY".into(), ..Default::default() };

    kept.update_portfolio(&spy, 100.0, 42.0, 4_200.0, 40.0, 200.0, 0.0, "DU1");
    kept.update_portfolio(&spy, 100.0, 43.0, 4_300.0, 40.0, 300.0, 0.0, "DU1");
    assert_eq!(kept.holdings().len(), 1, "one holding, not one per mark");
    assert_eq!(kept.holdings()[0].market_value, 4_300.0);
    assert_eq!(kept.holdings()[0].unrealized, 300.0);

    kept.update_portfolio(&spy, 0.0, 43.0, 0.0, 0.0, 0.0, 300.0, "DU1");
    assert!(kept.holdings().is_empty(), "a holding of none is not a holding");
}

/// What the account made, and what the venue broadcast, are kept as they
/// arrive — the profit as a statement about now, the notices as a list.
#[test]
fn profit_is_the_latest_word_and_notices_accumulate() {
    use crate::api::wrapper::Wrapper;
    let mut kept = LiveState::default();
    assert_eq!(kept.pnl(), None, "nothing until the venue says");

    // Named through the trait: the reader above is `pnl()` with no arguments
    // and hides the callback of the same name, which is what a caller wants
    // and what a test has to say out loud.
    Wrapper::pnl(&mut kept, 1, 10.0, 20.0, 30.0);
    Wrapper::pnl(&mut kept, 1, 11.0, 21.0, 31.0);
    assert_eq!(kept.pnl().unwrap().daily, 11.0, "the latest, not the first");

    kept.update_news_bulletin(1, 1, "first", "NYSE");
    kept.update_news_bulletin(2, 1, "second", "NYSE");
    assert_eq!(kept.bulletins().len(), 2, "a notice does not replace the one before");
}

/// Bars reach the caller who asked for that contract, and are kept for one who
/// subscribed and then looked instead of iterating.
#[test]
fn live_bars_are_streamed_to_their_own_reader_and_kept_for_everyone() {
    use crate::api::wrapper::Wrapper;
    let mut kept = LiveState::default();
    let (mine, mine_rx) = std::sync::mpsc::sync_channel(8);
    let (theirs, theirs_rx) = std::sync::mpsc::sync_channel(8);
    kept.stream_bars(11, mine);
    kept.stream_bars(22, theirs);

    kept.real_time_bar(11, 1_000, 1.0, 2.0, 0.5, 1.5, 100.0, 1.2, 7);
    kept.real_time_bar(22, 1_005, 9.0, 9.0, 9.0, 9.0, 1.0, 9.0, 1);

    let ours: Vec<_> = std::iter::from_fn(|| mine_rx.try_recv().ok()).collect();
    assert_eq!(ours.len(), 1, "the one on this subscription");
    assert_eq!(ours[0].close, 1.5);
    assert_eq!(theirs_rx.try_recv().map(|b| b.close).unwrap(), 9.0, "and theirs on theirs");
    assert_eq!(kept.live_bars().len(), 2, "both kept, whoever was listening");
}

/// A headline reaches every reader and is kept, because a session subscribes
/// to news once and more than one part of a program may care.
#[test]
fn news_reaches_every_reader_and_is_kept() {
    use crate::api::wrapper::Wrapper;
    let mut kept = LiveState::default();
    let (a, a_rx) = std::sync::mpsc::sync_channel(4);
    let (b, b_rx) = std::sync::mpsc::sync_channel(4);
    kept.stream_news(a);
    kept.stream_news(b);

    kept.tick_news(3, 1_700, "BRFG", "BRFG$1", "something happened", "");
    assert_eq!(a_rx.try_recv().unwrap().headline, "something happened");
    assert_eq!(b_rx.try_recv().unwrap().provider, "BRFG");
    assert_eq!(kept.news().len(), 1);
}
