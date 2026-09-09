//! Close every position a paper account is carrying.
//!
//! The compatibility suite places orders that fill and never closes them, so an
//! account it has been run against for weeks accumulates holdings until the
//! venue refuses new orders on margin grounds — a refusal that names the rule
//! rather than the order, and that reads in the suite's output as though the
//! order this client built were wrong. Emptying the account is what makes the
//! next run mean something.
//!
//! Paper only, checked twice: the session must have been opened as paper and
//! the account must be one of the venue's paper names. Neither check is a
//! formality — this places market orders.

use std::time::{Duration, Instant};

use ibx::api::session::Client;
use ibx::api::client::EClientConfig;
use ibx::api::types::Order;

fn main() {
    let _ = ibx::logging::try_init_from_env("error");
    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    let password = std::env::var("IB_PASSWORD").unwrap_or_default();
    if username.trim().is_empty() || password.trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This trades against a real session.");
        std::process::exit(2);
    }

    let config = EClientConfig {
        username,
        password,
        host: std::env::var("IB_HOST").unwrap_or_default(),
        paper: true,
        core_id: None,
        code_provider: None,
        ..Default::default()
    };
    let session = match Client::connect(&config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not open a session: {e}");
            std::process::exit(1);
        }
    };

    // The account the venue named, not the one asked for. A live account
    // reached through a paper configuration would still trade.
    let account = session.managed_accounts().first().cloned().unwrap_or_default();
    if !account.starts_with("DU") && !account.starts_with("DF") {
        eprintln!("account {account} is not a paper account; refusing to trade it");
        std::process::exit(1);
    }
    println!("account {account}");

    // The venue sends the holdings once the session settles; asking before it
    // has says the account is empty, which is the one answer that reads as
    // success and does nothing.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut held = session.positions();
    while held.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        held = session.positions();
    }

    let held: Vec<_> = held.into_iter().filter(|p| p.quantity != 0.0).collect();
    if held.is_empty() {
        println!("nothing held");
        return;
    }
    println!("{} position(s) to close", held.len());

    let mut closed = 0usize;
    for position in &held {
        let side = if position.quantity > 0.0 { "SELL" } else { "BUY" };
        let order = Order {
            action: side.to_string(),
            total_quantity: position.quantity.abs(),
            order_type: "MKT".to_string(),
            tif: "DAY".to_string(),
            ..Default::default()
        };
        let symbol = position.contract.symbol.clone();
        match session.place(&position.contract, &order) {
            Ok(placed) => {
                let done = placed.wait_done(Duration::from_secs(30));
                println!("  {side} {} {symbol}: {}", position.quantity.abs(),
                    if done { "closed" } else { "sent, still working" });
                closed += 1;
            }
            Err(why) => println!("  {side} {} {symbol}: refused — {why}", position.quantity.abs()),
        }
    }
    println!("{closed} of {} placed", held.len());
}
