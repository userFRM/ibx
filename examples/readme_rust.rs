//! The Rust example from the README, run against the venue.
//!
//! Kept as an example so it is compiled with everything else: a snippet in a
//! README that no longer builds is a snippet that turns readers away.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --example readme_rust

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::{Contract, Order};

fn main() -> Result<(), String> {
    let client = EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true,
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;

    let spy = client.qualify_contract(&Contract {
        symbol: "SPY".into(),
        sec_type: "STK".into(),
        exchange: "SMART".into(),
        currency: "USD".into(),
        ..Default::default()
    })?;

    let bars = client.historical_data(&spy, "", "2 D", "1 hour", "TRADES", true)?;
    let preview = client.what_if_order(&spy, &Order {
        action: "BUY".into(),
        order_type: "LMT".into(),
        total_quantity: 1.0,
        lmt_price: 1.0,
        ..Default::default()
    })?;
    println!("{} bars, preview {}", bars.len(), preview.status);

    client.req_mkt_data(1, &spy, "", false, false)?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    if let Some(instrument) = client.instrument_of(spy.con_id) {
        let quote = client.shared_state().market.quote(instrument);
        println!("bid {} ask {}", quote.bid, quote.ask);
    }

    client.disconnect();
    Ok(())
}
