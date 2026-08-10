//! Put an order on the wire without placing it.
//!
//! A preview goes through the whole order path — every attribute the caller
//! set, encoded and sent — and the venue answers with what the order would do
//! to the account. Nothing is placed. What is being checked is that the venue
//! takes the message: a tag it does not know, or one carrying the wrong kind
//! of value, comes back as a refusal rather than a margin figure.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --bin capture_whatif

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::{Contract, Order};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    previews: Vec<String>,
    refusals: Vec<String>,
}

impl Wrapper for Heard {
    fn open_order(&mut self, order_id: i64, _c: &Contract, _o: &Order, state: &ibx::api::types::OrderState) {
        self.previews.push(format!(
            "order {order_id}: status={} margin after={} commission={}",
            state.status, state.maint_margin_after, state.commission_and_fees,
        ));
    }
    fn error(&mut self, req_id: i64, code: i64, message: &str, _advanced: &str) {
        self.refusals.push(format!("{req_id}/{code}: {message}"));
    }
}

fn main() {
    let _ = env_logger::try_init();
    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    let password = std::env::var("IB_PASSWORD").unwrap_or_default();
    if username.trim().is_empty() || password.trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
        std::process::exit(2);
    }
    let client = match EClient::connect(&EClientConfig {
        username, password, paper: true, ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => { eprintln!("could not open a session: {e}"); std::process::exit(1); }
    };
    println!("session open");

    let contract = Contract {
        symbol: "SPY".to_string(),
        sec_type: "STK".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        ..Default::default()
    };
    let resolved = match client.qualify_contract(&contract) {
        Ok(c) => c,
        Err(e) => { println!("the contract could not be resolved: {e}"); return; }
    };

    // Every attribute wired today, on one order. A tag the venue will not take
    // comes back as a refusal naming it.
    let limit = || Order {
        action: "BUY".into(), order_type: "LMT".into(), total_quantity: 100.0,
        lmt_price: 100.0, what_if: true, ..Default::default()
    };
    // One attribute at a time, so a refusal names the one that caused it.
    let cases: Vec<(&str, Order)> = vec![
        ("nothing extra", limit()),
        ("a soft-dollar tier", Order {
            soft_dollar_tier_name: "Tier".into(), soft_dollar_tier_val: "1".into(),
            ..limit()
        }),
        ("an algo name, with no algo", Order { algo_id: "ibx-preview".into(), ..limit() }),
        ("an algo name, on an algo", Order {
            algo_strategy: "Adaptive".into(),
            algo_params: vec![ibx::api::types::TagValue {
                tag: "adaptivePriority".to_string(),
                value: "Normal".to_string(),
            }],
            algo_id: "ibx-preview".into(),
            ..limit()
        }),
        ("discretion to the limit", Order { discretionary_up_to_limit_price: true, ..limit() }),
        ("a settling firm", Order { settling_firm: "FIRM".into(), ..limit() }),
        ("a ladder", Order {
            scale_init_level_size: 10, scale_price_increment: 0.05, ..limit()
        }),
        ("a ladder against a position", Order {
            scale_init_level_size: 10, scale_price_increment: 0.05,
            scale_init_position: 50, ..limit()
        }),
        ("a ladder with its first part filled", Order {
            scale_init_level_size: 10, scale_price_increment: 0.05,
            scale_init_fill_qty: 5, ..limit()
        }),
        ("a ladder with varied sizes", Order {
            scale_init_level_size: 10, scale_price_increment: 0.05,
            randomize_size: true, ..limit()
        }),
    ];

    let mut heard = Heard::default();
    for (n, (what, order)) in cases.into_iter().enumerate() {
        let id = 9000 + n as i64;
        println!("\n  {what}");
        if let Err(e) = client.place_order(id, &resolved, &order) {
            println!("    refused before sending: {e}");
            continue;
        }
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            std::thread::sleep(Duration::from_millis(200));
        }
        for line in heard.previews.drain(..) { println!("    {line}"); }
        for line in heard.refusals.drain(..) { println!("    the venue says: {line}"); }
    }

    client.disconnect();
}
