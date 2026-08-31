//! Ask the venue how many contracts an account may watch at once.
//!
//! Subscribes to one contract after another and reads what the venue says when
//! it stops serving them. Reads only; it places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin capture_line_limit

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;

/// What the venue said, and about which request.
#[derive(Default)]
struct Heard {
    refusals: Vec<(i64, i64, String)>,
    ticking: std::collections::BTreeSet<i64>,
    absent: usize,
}

impl Wrapper for Heard {
    fn error(&mut self, req_id: i64, code: i64, message: &str, _advanced: &str) {
        // The codes every session carries about its farms are not about a
        // subscription; only what names this request is.
        // Nor is a strike that does not exist: that names the contract, not
        // the allowance.
        if !(2100..=2200).contains(&code) && code != 200 {
            self.refusals.push((req_id, code, message.to_string()));
        } else if code == 200 {
            self.absent += 1;
        }
    }
    fn tick_price(&mut self, req_id: i64, _field: i32, _price: f64, _attrib: &ibx::api::types::TickAttrib) {
        self.ticking.insert(req_id);
    }
    fn tick_size(&mut self, req_id: i64, _field: i32, _size: f64) {
        self.ticking.insert(req_id);
    }
}

/// One underlying's chain, which is where a real caller meets this first: a
/// few hundred contracts on one name, asked for at once.
fn chain(expiry: &str) -> Vec<Contract> {
    let mut out = Vec::new();
    for strike in 400..=700 {
        for right in ["C", "P"] {
            out.push(Contract {
                symbol: "SPY".to_string(),
                sec_type: "OPT".to_string(),
                exchange: "SMART".to_string(),
                currency: "USD".to_string(),
                last_trade_date_or_contract_month: expiry.to_string(),
                strike: strike as f64,
                right: right.to_string(),
                ..Default::default()
            });
        }
    }
    out
}

fn main() {
    let _ = ibx::logging::try_init_from_env("error");
    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    let password = std::env::var("IB_PASSWORD").unwrap_or_default();
    if username.trim().is_empty() || password.trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
        std::process::exit(2);
    }

    let config = EClientConfig {
        username, password,
        host: std::env::var("IB_HOST").unwrap_or_default(),
        paper: true, core_id: None, code_provider: None,
        ..Default::default()
    };
    let client = match EClient::connect(&config) {
        Ok(c) => c,
        Err(e) => { eprintln!("could not open a session: {e}"); std::process::exit(1); }
    };
    println!("session open");

    let mut heard = Heard::default();
    let mut asked = 0i64;
    let expiry = std::env::var("IBX_EXPIRY").unwrap_or_else(|_| "20260918".to_string());
    for (i, contract) in chain(&expiry).into_iter().enumerate() {
        let req_id = 1000 + i as i64;
        match client.req_mkt_data(req_id, &contract, "", false, false) {
            Ok(()) => asked += 1,
            Err(e) => {
                println!("  this client refused subscription {asked}: {e}");
                break;
            }
        }
        // Read as we go: the venue answers the one that goes too far, and
        // asking the rest afterwards would not say which one it was.
        let until = Instant::now() + Duration::from_millis(40);
        while Instant::now() < until {
            client.process_msgs(&mut heard);
            std::thread::sleep(Duration::from_millis(20));
        }
        if !heard.refusals.is_empty() {
            break;
        }
    }

    let settle = Instant::now() + Duration::from_secs(5);
    while Instant::now() < settle {
        client.process_msgs(&mut heard);
        std::thread::sleep(Duration::from_millis(50));
    }

    println!("\n  asked for {asked} subscriptions");
    println!("  {} named a contract that does not exist", heard.absent);
    println!("  {} of them ticked", heard.ticking.len());
    if heard.refusals.is_empty() {
        println!("  the venue refused none of them, so its allowance is above {asked}");
    } else {
        for (req_id, code, message) in &heard.refusals {
            println!("  refused req {req_id}: {code} {message}");
        }
    }
    client.disconnect();
}
