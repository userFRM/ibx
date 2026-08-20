//! Ask the corporate-events calendar what it carries, and for one contract's
//! events.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin capture_calendar

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;

/// Collects what the calendar answers, so a run says which of the three
/// happened: an answer, a refusal, or nothing at all.
#[derive(Default)]
struct Heard {
    meta: Vec<String>,
    events: Vec<String>,
    refusals: Vec<String>,
}

impl Wrapper for Heard {
    fn wsh_meta_data(&mut self, _req_id: i64, data_json: &str) {
        self.meta.push(data_json.to_string());
    }
    fn wsh_event_data(&mut self, _req_id: i64, data_json: &str) {
        self.events.push(data_json.to_string());
    }
    fn error(&mut self, _req_id: i64, code: i64, message: &str, _advanced: &str) {
        self.refusals.push(format!("{code}: {message}"));
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

    let config = EClientConfig {
        username,
        password,
        // Deliberately unstated: a login is enough, and the venue names the
        // server this account belongs on.
        host: std::env::var("IB_HOST").unwrap_or_default(),
        paper: true,
        core_id: None,
        code_provider: None,
        ..Default::default()
    };

    let client = match EClient::connect(&config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not open a session: {e}");
            std::process::exit(1);
        }
    };
    println!("session open");

    let mut heard = Heard::default();

    // The event types first. An event request is not accepted without them.
    if let Err(e) = client.req_wsh_meta_data(1) {
        println!("  the event types could not be asked for: {e}");
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        client.process_msgs(&mut heard);
        std::thread::sleep(Duration::from_millis(200));
    }

    // Then one contract's events.
    let apple = Contract {
        symbol: "AAPL".to_string(),
        sec_type: "STK".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        ..Default::default()
    };
    match client.qualify_contract(&apple) {
        Ok(resolved) => {
            println!("  asking for events on conId={}", resolved.con_id);
            let query = ibx::types::CalendarQuery {
                con_id: Some(resolved.con_id),
                total_limit: Some(20),
                ..Default::default()
            };
            if let Err(e) = client.req_wsh_event_data(2, query) {
                println!("  the events could not be asked for: {e}");
            }
        }
        Err(e) => println!("  the contract could not be resolved: {e}"),
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        client.process_msgs(&mut heard);
        std::thread::sleep(Duration::from_millis(200));
    }

    println!("\nwhat the venue said:");
    for said in &heard.refusals {
        println!("  refused: {said}");
    }
    for json in &heard.meta {
        println!("  event types: {} bytes", json.len());
        println!("    {}", &json[..json.len().min(400)]);
    }
    for json in &heard.events {
        println!("  events: {} bytes", json.len());
        println!("    {}", &json[..json.len().min(400)]);
    }
    if heard.refusals.is_empty() && heard.meta.is_empty() && heard.events.is_empty() {
        println!("  nothing at all, which is the one answer that says the request never landed");
    }

    client.disconnect();
}
