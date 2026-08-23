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
                println!("    (SPY trades 0930-1600 in US/Eastern; 1330-2000 is that in UTC)");
            },
            Err(e) => println!("\n  {what}: refused: {e}"),
        }
    }
}
