//! The tests for this module.

use super::*;
use crate::api::wrapper::Wrapper;
use crate::types::model::OrderState;

/// A session built on nothing, for the reading thread to run against. The
/// control channel comes back with it, because a session whose engine has gone
/// is a different test from the ones below.
fn a_session(
    shared: &Arc<crate::bridge::SharedState>,
) -> (Client, std::sync::mpsc::Receiver<crate::types::ControlCommand>) {
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    let session = Client {
        client: Arc::new(crate::api::EClient::from_parts(
            Arc::clone(shared), tx, std::thread::spawn(|| {}), "DU123".into(),
        )),
        state: Arc::new(Mutex::new(LiveState::default())),
        stop: Arc::new(AtomicBool::new(false)),
        reader: Arc::new(Mutex::new(None)),
    };
    (session, rx)
}

/// The last holder stops the session, whichever two go at once.
///
/// Read as a count and then acted on, the last two clones can both read two
/// and neither stop anything — leaving the reading thread, the engine and the
/// connection alive with nobody holding them. Dropping the handle answers who
/// was last as one step, so one of the two always sets it.
#[test]
fn the_last_two_holders_do_not_both_stand_aside() {
    for _ in 0..200 {
        let shared = Arc::new(crate::bridge::SharedState::new());
        let (session, _rx) = a_session(&shared);
        let stop = Arc::clone(&session.stop);
        let other = session.clone();

        // Both let go at the same instant. Spawned without one, the first
        // thread is finished before the second starts and the two never
        // overlap — which is the window this is about.
        let gate = Arc::new(std::sync::Barrier::new(2));
        let g1 = Arc::clone(&gate);
        let g2 = Arc::clone(&gate);
        let a = std::thread::spawn(move || { g1.wait(); drop(session); });
        let b = std::thread::spawn(move || { g2.wait(); drop(other); });
        a.join().unwrap();
        b.join().unwrap();

        assert!(
            stop.load(std::sync::atomic::Ordering::Relaxed),
            "whichever went last stopped the session",
        );
    }
}

/// Wait for something the reading thread does, rather than for a length of
/// time: on a machine under load a fixed sleep is either slow or a flake.
fn within_a_moment(mut happened: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if happened() {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

/// A session that drops and comes back is still being read.
///
/// The reader stopped at the first loss, and coming back is something only a
/// reader notices: the engine rebuilt the connection and said so, nothing was
/// left to read that, and the session stayed as stale as the moment it dropped
/// while the venue went on speaking to it.
#[test]
fn a_session_that_comes_back_is_read_again() {
    let shared = Arc::new(crate::bridge::SharedState::new());
    let (session, _engine) = a_session(&shared);
    session.start_reading();

    shared.set_connection_lost();
    assert!(within_a_moment(|| !session.is_connected()), "the loss went unread");

    shared.set_connection_restored();
    assert!(
        within_a_moment(|| session.is_connected()),
        "the session came back and nothing was reading it",
    );
    session.stop.store(true, Ordering::Relaxed);
}

/// A contract the venue said nothing about comes back without a quote.
///
/// The subscription registers an empty quote. Handing that back on timeout
/// answers with a bid and an ask of zero, which is a price and reads as a
/// market at nothing rather than as no answer. A quote carrying only a size or
/// a volume is an answer, not silence.
#[test]
fn a_contract_the_venue_said_nothing_about_comes_back_without_a_quote() {
    let shared = Arc::new(crate::bridge::SharedState::new());
    let (session, control) = a_session(&shared);
    // Long enough for the stand-in below to be scheduled; the tests default to
    // a millisecond, which is a real engine answering and a flake here.
    session.client.core.set_registration_timeout(Duration::from_secs(5));
    // Standing in for the engine, which answers a subscription by naming the
    // slot it took.
    let engine = thread::spawn(move || {
        while let Ok(command) = control.recv() {
            if let crate::types::ControlCommand::Subscribe { reply_tx: Some(reply), .. } = command {
                let _ = reply.try_send(Ok(0));
            }
        }
    });
    let spy = Contract {
        con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), ..Default::default()
    };

    let silent = session
        .quotes(std::slice::from_ref(&spy), Duration::from_millis(20))
        .expect("the subscription is made");
    assert_eq!(silent.len(), 1, "one contract asked about, one answer");
    assert!(silent[0].is_none(), "silence came back as a market at zero");

    // What the venue does state is an answer, whichever part of the quote
    // carries it.
    shared.market.push_quote(0, &crate::types::Quote {
        volume: 1_000 * crate::types::QTY_SCALE, ..Default::default()
    });
    let quoted = session
        .quotes(std::slice::from_ref(&spy), Duration::from_millis(20))
        .expect("the subscription is made");
    assert!(quoted[0].is_some(), "a volume the venue stated was read as silence");

    drop(session);
    engine.join().expect("the stand-in engine");
}

/// A stream ends when the session does.
///
/// Disconnecting drops the senders. A caller iterating order events or ticks
/// sees the iterator end rather than blocking in `recv()`.
#[test]
fn a_stream_ends_when_the_session_does() {
    let shared = Arc::new(crate::bridge::SharedState::new());
    let (session, _engine) = a_session(&shared);
    let mut events = session.order_events();
    session.disconnect();
    // On its own thread, so a stream that does not end fails this rather than
    // hanging it.
    let waiting = thread::spawn(move || events.next());
    assert!(within_a_moment(|| waiting.is_finished()), "the stream outlived the session");
    assert!(waiting.join().expect("the waiting thread").is_none());
}

/// And one that is over stops being read, and its streams end with it.
///
/// A session that ends on a refused logon, a takeover or exhausted recovery
/// drops its senders as an explicit disconnect does. Nothing will be sent on
/// it again, so the iterator ends rather than blocking in `recv()`.
#[test]
fn a_session_that_is_over_stops_being_read() {
    let shared = Arc::new(crate::bridge::SharedState::new());
    let (session, _engine) = a_session(&shared);
    let mut events = session.order_events();
    session.start_reading();
    shared.reference.set_session_over(
        crate::reliability::retry::DisconnectReason::ByDesign.as_str(),
    );
    shared.set_connection_lost();
    let reader = session.reader.lock().unwrap().take().expect("the reading thread");
    assert!(
        within_a_moment(|| reader.is_finished()),
        "the reader is still running on a session nothing will rebuild",
    );
    // On its own thread, so a stream that does not end fails this rather than
    // hanging it.
    let waiting = thread::spawn(move || events.next());
    assert!(within_a_moment(|| waiting.is_finished()), "the stream outlived the session");
    assert!(waiting.join().expect("the waiting thread").is_none());
}

fn an_order(id: i64) -> Order {
    Order { order_id: id, ..Order::limit("BUY", 100.0, 42.5) }
}

/// A report stating no permanent id does not take away the one already known.
///
/// Zero is what "unstated" looks like on this field, which is why the order's
/// own copy is only ever filled in from a report that carries one. The status
/// took whatever arrived. A later report without one erased the name the order
/// is known by across sessions, and a cancel addressed by that name then had
/// nothing to address.
#[test]
fn a_report_without_a_permanent_id_leaves_the_one_already_held() {
    let mut kept = LiveState::default();
    kept.order_status(1, "Submitted", 0.0, 100.0, 0.0, 77, 0, 0.0, 1, "", 0.0);
    assert_eq!(kept.trade(1).unwrap().status.perm_id, 77);

    kept.order_status(1, "Submitted", 0.0, 100.0, 0.0, 0, 0, 0.0, 1, "", 0.0);
    assert_eq!(
        kept.trade(1).unwrap().status.perm_id,
        77,
        "a report that states none leaves the one already learned",
    );
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

/// Dropping the last session stops it; dropping a clone does not.
///
/// The count is taken on the reader handle, which only a session holds.
/// Counting anything the reading thread also holds never reaches one while
/// that thread is alive, and the thread, engine and connection outlive every
/// caller.
#[test]
fn the_last_session_to_go_stops_the_reading() {
    let shared = std::sync::Arc::new(crate::bridge::SharedState::new());
    let (tx, _rx) = std::sync::mpsc::sync_channel(16);
    let session = Client {
        client: Arc::new(crate::api::EClient::from_parts(
            shared, tx, std::thread::spawn(|| {}), "DU123".into(),
        )),
        state: Arc::new(Mutex::new(LiveState::default())),
        stop: Arc::new(AtomicBool::new(false)),
        reader: Arc::new(Mutex::new(None)),
    };
    // What the reading thread takes when it starts, and holds for as long as
    // it runs. Counting any of these never reaches one while it does.
    let (_reads, _keeps, stop) = (
        Arc::clone(&session.client),
        Arc::clone(&session.state),
        Arc::clone(&session.stop),
    );

    let clone = session.clone();
    drop(clone);
    assert!(!stop.load(Ordering::Relaxed), "a clone going is not the caller finishing");

    drop(session);
    assert!(stop.load(Ordering::Relaxed), "the last one is");
}

/// A caller who is behind is not a caller who has gone.
///
/// A full buffer and a dropped receiver both fail the send. Treated alike, a
/// caller who read slower than the venue printed had their stream ended — and
/// an ended stream is what a closed session looks like, so there was no telling
/// the two apart.
#[test]
fn a_full_stream_drops_the_event_not_the_reader() {
    let mut kept = LiveState::default();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    kept.stream_order_events(tx);

    for _ in 0..4 {
        kept.order_status(1, "Submitted", 0.0, 100.0, 0.0, 77, 0, 0.0, 1, "", 0.0);
        kept.order_status(1, "PreSubmitted", 0.0, 100.0, 0.0, 77, 0, 0.0, 1, "", 0.0);
    }
    // The buffer filled and stayed filled, and the caller is still subscribed.
    assert!(rx.try_recv().is_ok(), "what fit is there");
    kept.order_status(1, "Submitted", 0.0, 100.0, 0.0, 77, 0, 0.0, 1, "", 0.0);
    assert!(rx.try_recv().is_ok(), "and the stream still delivers once there is room");

    // A caller who has actually gone is forgotten.
    drop(rx);
    kept.order_status(1, "PreSubmitted", 0.0, 100.0, 0.0, 77, 0, 0.0, 1, "", 0.0);
    assert!(kept.order_streams.is_empty(), "a dropped receiver is not kept");
}

/// An order half filled is still working.
///
/// A partial fill is reported under its own status, and a second list of the
/// working statuses left it out — so `wait_done` returned on the first print of
/// a large order and a caller went on as though the rest had traded.
///
/// An order whose status the session lost is not one that has stopped either.
/// Nothing can say it has, and ending the wait there ends it on an order that
/// may still be live.
#[test]
fn an_order_still_working_is_not_reported_as_finished() {
    let mut kept = LiveState::default();
    // A partly filled working order is Submitted, with the filled and
    // remaining quantities carrying the distinction.
    for still_working in ["PendingSubmit", "PreSubmitted", "Submitted", "Unknown"] {
        kept.order_status(1, still_working, 40.0, 60.0, 42.5, 77, 0, 0.0, 1, "", 0.0);
        let trade = kept.trade(1).unwrap();
        assert!(trade.is_active(), "{still_working} is still working");
        assert!(!trade.is_done(), "{still_working} has not finished");
    }
    for finished in ["Filled", "Cancelled"] {
        kept.order_status(1, finished, 100.0, 0.0, 42.5, 77, 0, 0.0, 1, "", 0.0);
        assert!(kept.trade(1).unwrap().is_done(), "{finished} has finished");
    }
}

/// An order the venue is holding has not finished, and one it refused has.
///
/// Both are reported as `Inactive`: this vocabulary has no separate word for a
/// refusal, and what tells them apart is the completed status the venue states
/// beside it: one for a refusal, none for a hold. Counting both as finished
/// drops a held order from `open_trades` and returns `wait_done` on an order
/// the venue may still work.
#[test]
fn an_order_the_venue_is_holding_has_not_finished() {
    let mut held = LiveState::default();
    held.order_status(1, "Inactive", 0.0, 100.0, 0.0, 77, 0, 0.0, 1, "", 0.0);
    held.open_order(
        1, &Contract::default(), &Order::default(),
        &OrderState { status: "Inactive".into(), ..Default::default() },
    );
    let trade = held.trade(1).expect("the order the venue is holding");
    assert!(trade.is_active(), "an order the venue may still work has not finished");
    assert!(held.open_trades().iter().any(|t| t.order.order_id == trade.order.order_id));

    let mut refused = LiveState::default();
    refused.order_status(2, "Inactive", 0.0, 100.0, 0.0, 78, 0, 0.0, 1, "", 0.0);
    refused.open_order(
        2, &Contract::default(), &Order::default(),
        &OrderState {
            status: "Inactive".into(),
            completed_status: "No valid bid/ask".into(),
            ..Default::default()
        },
    );
    assert!(refused.trade(2).expect("the refused order").is_done(), "a refusal is final");
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

    // And a price below zero is a price. Only zero means nothing was stated:
    // read as unstated too, an instrument that trades negative reported the
    // average before it, which is a different number about a different trade.
    kept.order_status(3, "Filled", 1.0, 0.0, 12.0, 0, 0, 0.0, 1, "", 0.0);
    kept.order_status(3, "Filled", 2.0, 0.0, -3.75, 0, 0, 0.0, 1, "", 0.0);
    assert_eq!(kept.trade(3).unwrap().status.average_price, -3.75);
}

/// A holding is reported as it stands, so the same contract replaces itself
/// and one reported as zero is gone.
///
/// Appending instead shows a reopened position twice and keeps a closed one
/// indefinitely.
#[test]
fn a_holding_is_what_the_account_holds_now() {
    let mut kept = LiveState::default();
    let spy = Contract {
        con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), ..Default::default()
    };
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

/// A cost lands on the fill it belongs to and on no other.
///
/// The fill each report names is looked up rather than searched for, so what
/// this guards is the lookup pointing at the right one: several fills across
/// several orders, arriving before any of their costs, and each cost then
/// naming a fill in the middle of the list rather than the one just added.
#[test]
fn a_cost_lands_on_its_own_fill_and_leaves_the_rest_alone() {
    let mut kept = LiveState::default();
    let spy = Contract::stock("SPY");
    for order_id in [5i64, 6] {
        kept.open_order(order_id, &spy, &an_order(order_id), &OrderState::default());
        for n in 0..3 {
            kept.exec_details(0, &spy, &crate::types::model::Execution {
                order_id,
                exec_id: format!("{order_id}.{n}"),
                ..Default::default()
            });
        }
    }

    // The middle fill of the first order, and the last of the second.
    kept.commission_and_fees_report(
        &crate::types::model::CommissionAndFeesReport::charged("5.1", 1.25, "USD"),
    );
    kept.commission_and_fees_report(
        &crate::types::model::CommissionAndFeesReport::charged("6.2", 3.50, "USD"),
    );

    let cost_of = |id: &str| {
        kept.fills().iter()
            .find(|f| f.execution.exec_id == id)
            .and_then(|f| f.commission.clone())
            .map(|c| c.commission_and_fees)
    };
    assert_eq!(cost_of("5.1"), Some(1.25), "the fill the report named");
    assert_eq!(cost_of("6.2"), Some(3.50), "and the one the second named");
    for untouched in ["5.0", "5.2", "6.0", "6.1"] {
        assert_eq!(cost_of(untouched), None, "{untouched} was given a cost of its own");
    }
    // And on the order's own copy, which is a separate list of the same fills.
    assert_eq!(
        kept.trade(5).map(|t| t.fills.iter().filter(|f| f.commission.is_some()).count()),
        Some(1),
        "exactly one of the order's fills has a cost",
    );
}

/// What a fill cost reaches every view of that fill.
///
/// The trade and its cost are two reports and the trade arrives first. The
/// cost is written to both the order and the session's list of fills; writing
/// it to the order alone leaves that list reporting no commission.
#[test]
fn what_a_fill_cost_reaches_the_session_as_well_as_the_order() {
    let mut kept = LiveState::default();
    let spy = Contract::stock("SPY");
    kept.open_order(5, &spy, &an_order(5), &OrderState::default());
    let execution = crate::types::model::Execution {
        order_id: 5, exec_id: "0001.a".into(), ..Default::default()
    };
    kept.exec_details(0, &spy, &execution);
    kept.commission_and_fees_report(
        &crate::types::model::CommissionAndFeesReport::charged("0001.a", 1.25, "USD"),
    );

    assert_eq!(
        kept.trade(5).and_then(|t| t.fills[0].commission.clone()).map(|c| c.commission_and_fees),
        Some(1.25),
        "the order was not told what its fill cost",
    );
    assert_eq!(
        kept.fills()[0].commission.clone().map(|c| c.commission_and_fees),
        Some(1.25),
        "the session was not told what its fill cost",
    );
}

/// An order the venue answers before the caller's own record is written keeps
/// what only the caller knows.
///
/// A status names neither the contract nor the order, and it can arrive in the
/// gap between sending one and writing it down. Keeping the status-only record
/// leaves the trade naming no instrument.
#[test]
fn a_status_that_arrives_first_does_not_cost_the_order_its_contract() {
    let mut kept = LiveState::default();
    kept.order_status(7, "Submitted", 0.0, 100.0, 0.0, 77, 0, 0.0, 1, "", 0.0);
    kept.remember(7, Trade {
        contract: Contract::stock("SPY"),
        order: an_order(7),
        status: OrderStatus { status: "PendingSubmit".to_string(), ..Default::default() },
        state: None,
        fills: Vec::new(),
    });

    let trade = kept.trade(7).expect("the order is held");
    assert_eq!(trade.contract.symbol, "SPY", "the contract was lost");
    assert_eq!(trade.order.lmt_price, an_order(7).lmt_price, "the order was lost");
    assert_eq!(trade.status.status, "Submitted", "the venue's word was overwritten");
}

/// And the open-order snapshot the session opens with does the same.
#[test]
fn a_snapshot_fills_in_an_order_a_status_created_blank() {
    let mut kept = LiveState::default();
    kept.order_status(8, "Submitted", 0.0, 100.0, 0.0, 77, 0, 0.0, 1, "", 0.0);

    let mut answered = LiveState::default();
    answered.open_order(8, &Contract::stock("SPY"), &an_order(8), &OrderState::default());
    kept.absorb(answered);

    let trade = kept.trade(8).expect("the order is held");
    assert_eq!(trade.contract.symbol, "SPY", "the snapshot's contract was thrown away");
    assert_eq!(trade.status.status, "Submitted", "the venue's word was overwritten");
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

    // Including a print, which is the change this session carries most of and
    // the one it did not count.
    let after_status = kept.changes();
    kept.tick_by_tick_all_last(
        7, 1, 1_700_000_000, 42.5, 100.0,
        &crate::types::model::TickAttribLast::default(), "NYSE", "",
    );
    assert_ne!(kept.changes(), after_status, "a print did not count as a change");
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
/// Appending per mark instead shows one row for every price the venue has
/// stated the holding at.
#[test]
fn a_holding_is_priced_as_it_stands() {
    use crate::api::wrapper::Wrapper;
    let mut kept = LiveState::default();
    let spy = Contract {
        con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), ..Default::default()
    };

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

/// Asking through [`AsyncClient::off_reactor`] does not hold the runtime
/// thread that asked.
///
/// The methods `AsyncClient` names cover what a session is usually asked; every
/// other call is reached through this one. Run inline it would block the
/// reactor for the length of a round trip, which on a single-threaded runtime
/// stops everything else the program is doing — and stops it silently, under
/// load, rather than in review. So this runs on a runtime with exactly one
/// worker and asks whether the rest of that runtime kept moving.
#[cfg(feature = "async")]
#[test]
fn asking_off_the_reactor_leaves_the_runtime_free() {
    use crate::AsyncClient;
    use std::sync::atomic::AtomicU64;

    let shared = Arc::new(crate::bridge::SharedState::new());
    let (session, _rx) = a_session(&shared);
    let client = AsyncClient::from_session(session);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a runtime to ask from");

    let ticks = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&ticks);

    runtime.block_on(async move {
        tokio::spawn(async move {
            loop {
                counted.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        // Yield once so the ticker is actually running before the ask starts,
        // or this measures the spawn rather than the ask.
        tokio::task::yield_now().await;

        let answered = client
            .off_reactor(|_| {
                thread::sleep(Duration::from_millis(150));
                "answered"
            })
            .await
            .expect("the ask to come back");

        assert_eq!(answered, "answered");
    });

    // The ticker wakes every millisecond, so 150ms of asking leaves room for
    // well over a hundred. Held reactor, it gets none: the bar is low enough
    // that a loaded machine still clears it and an inline ask still fails.
    let moved = ticks.load(Ordering::Relaxed);
    assert!(moved > 20, "the runtime advanced {moved} times while asking");
}


/// The streams this session opens reach the wire.
///
/// They number themselves from the band reserved for the calls this client
/// makes on its own account, and the request surface refuses that band to
/// anyone else. A stream that took a number from it without saying it was one
/// of those calls was refused by the surface it was calling — the feature was
/// dead and every test still passed, because none of them opened one.
#[test]
fn the_streams_this_session_opens_reach_the_wire() {
    let shared = Arc::new(crate::bridge::SharedState::new());
    let (session, rx) = a_session(&shared);
    let spy = Contract {
        con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), currency: "USD".into(), ..Default::default()
    };

    let bars = session.live_bar_stream(&spy);
    assert!(bars.is_ok(), "a live bar stream must open: {:?}", bars.err());

    // The tick stream waits for the engine to register the instrument, which
    // this harness has none of, so it ends in that wait rather than opening.
    // What it must not end in is a refusal of its own number.
    if let Err(refused) = session.ticks(&spy) {
        assert!(
            !refused.message.contains("req_id"),
            "the tick stream was refused for its number: {refused}",
        );
    }

    let asked: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        asked.iter().any(|c| matches!(c, crate::types::ControlCommand::SubscribeRealTimeBar { .. })),
        "the bar stream reaches the engine, not just the caller",
    );
}
