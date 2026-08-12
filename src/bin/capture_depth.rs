//! Keep a book's frames exactly as the venue sends them.
//!
//! What a level says about the venue it stands on decides whether an aggregate
//! book can be attributed at all. A reading checked only against frames this
//! client made up says nothing about the ones that arrive, so this asks for
//! real ones — an aggregate and a named venue, on the same contract.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --bin capture_depth

use std::time::Duration;

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;

fn spy(exchange: &str) -> Contract {
    Contract {
        symbol: "SPY".to_string(),
        sec_type: "STK".to_string(),
        exchange: exchange.to_string(),
        currency: "USD".to_string(),
        ..Default::default()
    }
}

fn main() {
    unsafe { std::env::set_var("IBX_CAPTURE_WIRE", "1") };
    env_logger::init();

    let config = EClientConfig {
        username: std::env::var("IB_USERNAME").expect("IB_USERNAME"),
        password: std::env::var("IB_PASSWORD").expect("IB_PASSWORD"),
        host: std::env::var("IB_HOST").unwrap_or_else(|_| "cdc1.ibllc.com".to_string()),
        paper: true,
        ..Default::default()
    };
    let client = EClient::connect(&config).expect("no session");

    // The aggregate and one named venue, so the two shapes can be compared.
    client.req_mkt_depth(1, &spy("SMART"), 10, true).expect("aggregate");
    client.req_mkt_depth(2, &spy("IEX"), 10, false).expect("named venue");

    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(500));
    }

    let kept = client.unread_wire();
    let books: Vec<_> = kept.iter().filter(|(kind, _)| kind.starts_with("depth")).collect();
    println!("{} frame(s) kept, {} of them a book", kept.len(), books.len());
    for (kind, hex) in books.iter().take(10) {
        println!("{kind} {} bytes", hex.len() / 2);
        println!("  {hex}");
    }
    let _ = client.cancel_mkt_depth(1);
    let _ = client.cancel_mkt_depth(2);
}
