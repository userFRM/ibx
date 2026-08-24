//! Whether the venue knows the series name this client asks it for.
//!
//! An adjusted daily series is asked for here under `<data>AdjustedLast</data>`.
//! The vendor client asks for `<data>Last</data>` and sets a flag, then applies
//! the corporate actions itself from a feed of its own — the string this client
//! sends appears nowhere in its build. Whether the venue nonetheless answers to
//! it is the question, and only the venue can settle it.
//!
//! Asks for the same contract both ways and prints what came back.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin probe_adjusted_last

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::{BarData, Contract};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    bars: Vec<(i64, BarData)>,
    ended: Vec<i64>,
    said: Vec<String>,
}

impl Wrapper for Heard {
    fn historical_data(&mut self, req_id: i64, bar: &BarData) {
        self.bars.push((req_id, bar.clone()));
    }
    fn historical_data_end(&mut self, req_id: i64, _start: &str, _end: &str) {
        self.ended.push(req_id);
    }
    fn error(&mut self, req_id: i64, code: i64, message: &str, _adv: &str) {
        if !matches!(code, 2104 | 2106 | 2107 | 2119 | 2158) {
            self.said.push(format!("req {req_id} — {code}: {message}"));
        }
    }
}

fn main() {
    let _ = env_logger::try_init();
    unsafe { std::env::set_var("IBX_CAPTURE_WIRE", "1") };
    let client = match EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true, ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => { eprintln!("could not open a session: {e}"); std::process::exit(1); }
    };
    println!("session open");

    let contract = Contract {
        symbol: "AAPL".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), currency: "USD".into(), ..Default::default()
    };
    let resolved = match client.qualify_contract(&contract) {
        Ok(c) => c,
        Err(e) => { println!("  the contract could not be named: {e}"); return; }
    };

    let mut heard = Heard::default();
    // A span long enough to cross a split or a dividend, so an adjusted
    // series and a raw one have something to differ about.
    for (req, what) in [(1i64, "TRADES"), (2, "ADJUSTED_LAST")] {
        if let Err(e) = client.req_historical_data(
            req, &resolved, "", "1 Y", "1 day", what, true, 1, false,
        ) {
            println!("  {what:14} refused before sending: {e}");
            continue;
        }
        let deadline = Instant::now() + Duration::from_secs(25);
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            if heard.ended.contains(&req) { break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        let mine: Vec<_> = heard.bars.iter().filter(|(r, _)| *r == req).collect();
        println!(
            "\n  {what:14} {} bars, ended: {}",
            mine.len(), heard.ended.contains(&req),
        );
        if let Some((_, first)) = mine.first() {
            println!("    first {} close={}", first.date, first.close);
        }
        if let Some((_, last)) = mine.last() {
            println!("    last  {} close={}", last.date, last.close);
        }
        for s in heard.said.iter().filter(|s| s.starts_with(&format!("req {req} "))).take(2) {
            println!("    {s}");
        }
    }

    // What actually went out, so the series name is on the record.
    println!("\n[what this client asked for]");
    for (conn, text) in client.unread_wire() {
        if conn != "historical-query" { continue; }
        if let Some(at) = text.find("<data>") {
            let end = text[at..].find("</data>").map(|e| at + e + 7).unwrap_or(text.len());
            println!("  {}", &text[at..end]);
        }
    }
}
