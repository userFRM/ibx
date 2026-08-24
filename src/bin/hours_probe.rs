//! Which clock a contract's trading hours are stated on.
//!
//! A contract states its hours and names a timezone, and the two do not agree
//! here: the hours come off the wire in UTC and are handed on that way, while
//! the name beside them is the exchange's. A caller converting the one by the
//! other, which is what the reference client's contract says to do, moves
//! every session by the offset.
//!
//! Answers without a market open, so it can be run at any hour.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin hours_probe
use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;

fn main() {
    let _ = env_logger::try_init();
    let client = EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true, ..Default::default()
    }).expect("session");
    println!("session open");
    // Every zone this venue names, across a spread of the world's exchanges,
    // and whether a zone database can resolve each. What it cannot resolve is
    // what stops the hours being stated on the clock they are named with.
    let mut named: Vec<(String, String)> = Vec::new();
    for c in [
        ("SPY", "STK", "SMART", "USD"), ("AAPL", "STK", "ISLAND", "USD"),
        ("SAP", "STK", "IBIS", "EUR"), ("VOD", "STK", "LSE", "GBP"),
        ("7203", "STK", "TSEJ", "JPY"), ("BHP", "STK", "ASX", "AUD"),
        ("0700", "STK", "SEHK", "HKD"), ("NESN", "STK", "EBS", "CHF"),
        ("MES", "FUT", "CME", "USD"), ("RY", "STK", "TSE", "CAD"),
    ] {
        let ask = Contract {
            symbol: c.0.into(), sec_type: c.1.into(), exchange: c.2.into(),
            currency: c.3.into(), ..Default::default()
        };
        if let Ok(found) = client.contract_details(&ask)
            && let Some(d) = found.first()
            && let Some(zone) = d.time_zone_id.as_deref()
            && !zone.is_empty()
        {
            let known = ibx::control::contracts::sessions_are_stated_on(zone);
            named.push((zone.to_string(), format!("{} on {}", c.0, c.2)));
            println!(
                "  {:<20} {}  ({})",
                zone, if known { "stated on it   " } else { "left on UTC    " }, named.last().unwrap().1,
            );
        }
    }
    println!();

    for (what, c) in [
        ("a US listing", Contract {
            symbol: "SPY".into(), sec_type: "STK".into(),
            exchange: "SMART".into(), currency: "USD".into(), ..Default::default() }),
        ("a European listing", Contract {
            symbol: "SAP".into(), sec_type: "STK".into(),
            exchange: "IBIS".into(), currency: "EUR".into(), ..Default::default() }),
    ] {
        match client.contract_details(&c) {
            Ok(found) => for d in found.iter().take(1) {
                println!("\n  {what}: {} on {}", d.contract.symbol, d.contract.exchange);
                println!("    time_zone_id  = {:?}", d.time_zone_id);
                println!("    trading_hours = {:?}", d.trading_hours.as_deref().unwrap_or("").split(';').next().unwrap_or(""));
                println!("    liquid_hours  = {:?}", d.liquid_hours.as_deref().unwrap_or("").split(';').next().unwrap_or(""));

            },
            Err(e) => println!("\n  {what}: refused: {e}"),
        }
    }
}
