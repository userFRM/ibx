//! The session API, driven against the venue the way a caller drives it.
//!
//! Every other live suite here reaches for the wire or pumps the client by
//! hand. This one opens a session — which starts a reader thread of its own —
//! and then asks it questions, which is the shape the module's own
//! documentation shows and the shape a program written against this library
//! takes.
//!
//! Nothing covered that shape until now, and a deadlock lived in it: the
//! reader took the record and then the turn, every question takes the turn and
//! then the record, and the two wedged on the locks themselves where no
//! deadline reaches them. It was found by reading, not by running, because the
//! offline session tests leave the kept record unset and never take the pair
//! both ways.
//!
//! Needs `IB_USERNAME` and `IB_PASSWORD`, and answers nothing without them:
//!
//!     cargo test --test session_api_live -- --nocapture

use std::time::{Duration, Instant};

use ibx::api::client::{Contract, EClientConfig};
use ibx::api::session::Client;

/// The session, or nothing where this checkout has no credentials.
fn a_session() -> Option<Client> {
    let username = std::env::var("IB_USERNAME").ok().filter(|v| !v.trim().is_empty())?;
    let password = std::env::var("IB_PASSWORD").ok().filter(|v| !v.trim().is_empty())?;
    let config = EClientConfig { username, password, paper: true, ..Default::default() };
    match Client::connect(&config) {
        Ok(session) => Some(session),
        Err(why) => panic!("the session did not open: {why}"),
    }
}

#[test]
fn a_session_answers_questions_while_its_reader_runs() {
    let Some(session) = a_session() else {
        println!("SKIP: no credentials in this checkout");
        return;
    };

    // The shape that wedged. A question holds the turn for its whole length
    // and locks the record on every pump inside it, while the reader thread
    // this session started is doing the same two locks on its own schedule.
    // Bounded so a wedge fails the test rather than hanging the suite.
    let asked_at = Instant::now();
    let named = session.qualify(Contract {
        symbol: "SPY".into(),
        sec_type: "STK".into(),
        exchange: "SMART".into(),
        currency: "USD".into(),
        ..Default::default()
    });
    let took = asked_at.elapsed();
    assert!(
        took < Duration::from_secs(30),
        "a question asked of a session with a reader running took {took:?}",
    );
    let named = named.expect("the venue names a contract it lists");
    assert!(named.con_id != 0, "the venue stated its own number for it");
    println!("qualify: con_id={} in {took:?}", named.con_id);

    // And again, because the wedge needs the reader to be mid-pass: one
    // question can win the race by luck, several in a row cannot.
    for round in 0..5 {
        let at = Instant::now();
        let again = session.qualify(Contract {
            symbol: "MSFT".into(),
            sec_type: "STK".into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
            ..Default::default()
        });
        assert!(
            at.elapsed() < Duration::from_secs(30),
            "round {round}: the session stopped answering after {:?}",
            at.elapsed(),
        );
        assert!(again.is_ok(), "round {round}: {again:?}");
    }
    println!("five more questions answered, none wedged");

    // What the session kept while its reader ran: the account the logon named.
    let held = session.account_values();
    println!("account values held by the reader: {}", held.len());

    // And it closes, which is the other half of the wedge — a disconnect that
    // joins a reader stopped on a lock never returns.
    let closed_at = Instant::now();
    drop(session);
    assert!(
        closed_at.elapsed() < Duration::from_secs(30),
        "closing the session took {:?}",
        closed_at.elapsed(),
    );
    println!("session closed in {:?}", closed_at.elapsed());
}
