//! What the venue says a contract pays out.
//!
//! The option model needs the dates, not just a present value: a tree that
//! folds a year of dividends into one yield reproduces the price it was
//! calibrated to and gets the shape wrong. This asks the venue for the
//! schedule of an option's underlying and prints what comes back.
//!
//! Reads only. It places nothing.

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;

fn main() {
    let _ = ibx::logging::try_init_from_env("error");
    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    if username.trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
        std::process::exit(2);
    }
    let client = match EClient::connect(&EClientConfig {
        username,
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true,
        ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not open a session: {e}");
            std::process::exit(1);
        }
    };
    println!("session open");

    // An option, so that resolving it states what it is written on — which is
    // the contract whose schedule the model wants.
    let option = Contract {
        symbol: std::env::var("IBX_SYMBOL").unwrap_or_else(|_| "SPY".to_string()),
        sec_type: "OPT".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        last_trade_date_or_contract_month: std::env::var("IBX_EXPIRY")
            .unwrap_or_else(|_| "20260918".to_string()),
        strike: std::env::var("IBX_STRIKE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(775.0),
        right: "C".to_string(),
        ..Default::default()
    };
    let resolved = match client.qualify_contract(&option) {
        Ok(c) => c,
        Err(e) => {
            println!("the contract could not be resolved: {e}");
            return;
        }
    };
    println!("  option conId={}", resolved.con_id);

    let shared = client.shared_state();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut under = None;
    while Instant::now() < deadline && under.is_none() {
        std::thread::sleep(Duration::from_millis(250));
        under = shared.reference.under_con_id(resolved.con_id as u32);
    }
    let Some(under) = under else {
        println!("the venue did not say what this option is written on");
        return;
    };
    println!("  written on conId={under}");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut schedule = None;
    while Instant::now() < deadline && schedule.is_none() {
        std::thread::sleep(Duration::from_millis(250));
        schedule = shared.reference.dividend_schedule(under);
    }
    match schedule {
        None => println!("no schedule arrived for {under}"),
        Some(schedule) => {
            println!("  tax adjustment ratio {}", schedule.tax_adjustment);
            println!("  term rates {:?}", schedule.term_rates);
            println!("  {} payments:", schedule.payments.len());
            for payment in &schedule.payments {
                println!(
                    "    ex {} pay {} rec {} {} {} ({})",
                    payment.ex_date, payment.pay_date, payment.record_date,
                    payment.amount, payment.currency, payment.distribution_type,
                );
            }
        }
    }
}
