//! Ask the venue the same questions from the Rust client, and print the
//! answers in a form the Python client's answers can be compared against.
//!
//! Four gates already compare the two clients offline: what settings each
//! carries, what an order's fields do, what each surface is called, and what
//! each says when a call cannot be served. None of them asks the venue
//! anything, so none of them can catch the two agreeing on paper and
//! answering differently in front of a real server.
//!
//! Run this and `scripts/conformance.py` in turn and compare the two blocks.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin capture_conformance

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::{Contract, Order};

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

    let asked = Contract {
        symbol: "SPY".to_string(),
        sec_type: "STK".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        ..Default::default()
    };
    let details = client.contract_details(&asked).expect("SPY resolves");
    let spy = &details[0].contract;
    println!("con_id={}", spy.con_id);
    println!("listed_on={}", spy.primary_exchange);
    println!("min_tick={}", details[0].min_tick);
    println!("trading_class={}", spy.trading_class);

    // Bars for a window that has already closed, so both clients ask about
    // the same hours however long apart they run.
    let bars = client
        .historical_data(&asked, "", "2 D", "1 hour", "TRADES", true)
        .unwrap_or_default();
    println!("bars={}", bars.len());
    println!("first_bar={}", bars.first().map(|b| b.date.clone()).unwrap_or_default());

    let chains = client.option_chain(spy).unwrap_or_default();
    let mut exchanges: Vec<String> = chains.iter().map(|c| c.exchange.clone()).collect();
    exchanges.sort();
    println!("chain_exchanges={}", exchanges.join(","));

    let matches = client.matching_symbols("APP").unwrap_or_default();
    println!("symbol_matches={}", matches.len());

    // What the venue says an order would cost, which is the same question
    // whichever client asks it.
    let preview = client.what_if_order(spy, &Order {
        action: "BUY".into(),
        order_type: "LMT".into(),
        total_quantity: 1.0,
        lmt_price: 1.0,
        ..Default::default()
    });
    match preview {
        Ok(state) => {
            println!("preview_status={}", state.status);
            println!("preview_commission={}", state.commission_and_fees);
        }
        Err(e) => println!("preview_error={} ({})", e.message.lines().next().unwrap_or(""), e.code),
    }

    client.disconnect();
}
