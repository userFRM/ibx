//! Exercise the reference and order paths this board claims, and report what
//! the venue actually answered for each.
//!
//! Written because a board that says "working" without saying what proved it
//! is a claim. Every line printed here is evidence or the absence of it.
//!
//! Reads only, apart from one order placed and withdrawn on a paper account,
//! which is the only way to prove a withdrawal reaches the venue.

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::{Contract, Order};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    depth: usize,
    chains: usize,
    matches: usize,
    news_providers: usize,
    scanner_params: usize,
    fundamentals: usize,
    statuses: Vec<String>,
    said: Vec<String>,
}

impl Wrapper for Heard {
    fn update_mkt_depth(&mut self, _r: i64, _p: i32, _o: i32, _s: i32, _pr: f64, _sz: f64) {
        self.depth += 1;
    }
    fn update_mkt_depth_l2(
        &mut self, _r: i64, _p: i32, _m: &str, _o: i32, _s: i32, _pr: f64, _sz: f64, _sm: bool,
    ) {
        self.depth += 1;
    }
    fn security_definition_option_parameter(
        &mut self, _r: i64, _ex: &str, _u: i64, _tc: &str, _m: &str,
        _expirations: &[String], _strikes: &[f64],
    ) {
        self.chains += 1;
    }
    fn symbol_samples(&mut self, _r: i64, found: &[ibx::api::types::ContractDescription]) {
        self.matches += found.len();
    }
    fn news_providers(&mut self, providers: &[ibx::types::NewsProvider]) {
        self.news_providers += providers.len();
    }
    fn scanner_parameters(&mut self, xml: &str) {
        self.scanner_params += xml.len();
    }
    fn fundamental_data(&mut self, _r: i64, data: &str) {
        self.fundamentals += data.len();
    }
    fn order_status(
        &mut self, id: i64, status: &str, _f: f64, _r: f64, _ap: f64,
        _p: i64, _pid: i64, _lf: f64, _c: i64, _w: &str, _mtp: f64,
    ) {
        self.statuses.push(format!("{id}:{status}"));
    }
    fn error(&mut self, _r: i64, code: i64, message: &str, _a: &str) {
        if !matches!(code, 2104 | 2106 | 2158 | 2107) {
            self.said.push(format!("{code}: {}", message.lines().next().unwrap_or("")));
        }
    }
}

fn main() {
    let _ = env_logger::try_init();
    if std::env::var("IB_USERNAME").unwrap_or_default().trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
        std::process::exit(2);
    }
    let client = match EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true,
        ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => { eprintln!("could not open a session: {e}"); std::process::exit(1); }
    };
    println!("session open");

    let spy = Contract {
        symbol: "SPY".to_string(), sec_type: "STK".to_string(),
        exchange: "SMART".to_string(), currency: "USD".to_string(),
        ..Default::default()
    };
    let spy = client.qualify_contract(&spy).expect("SPY resolves");
    let mut heard = Heard::default();
    let wait = |client: &EClient, heard: &mut Heard, secs: u64| {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            client.process_msgs(heard);
            std::thread::sleep(Duration::from_millis(150));
        }
    };

    let _ = client.req_mkt_depth(1, &spy, 5, false);
    let _ = client.req_sec_def_opt_params(2, "SPY", "", "STK", spy.con_id);
    let _ = client.req_matching_symbols(3, "APP");
    client.req_news_providers(&mut heard);
    let _ = client.req_scanner_parameters();
    let _ = client.req_fundamental_data(6, &spy, "ReportsFinSummary");
    wait(&client, &mut heard, 20);
    let _ = client.cancel_mkt_depth(1);

    println!("\n  depth updates          {}", heard.depth);
    println!("  option chain replies   {}", heard.chains);
    println!("  symbol matches         {}", heard.matches);
    println!("  news providers         {}", heard.news_providers);
    println!("  scanner parameters     {} bytes", heard.scanner_params);
    println!("  fundamental document   {} bytes", heard.fundamentals);

    // An order placed far from the market, then changed, then withdrawn. The
    // only way to show a withdrawal reaches the venue is to have something
    // running for it to withdraw.
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() % 90_000) as i64 * 10 + 5)
        .unwrap_or(12345);
    let far_below = Order {
        action: "BUY".into(), order_type: "LMT".into(),
        total_quantity: 1.0, lmt_price: 50.0, ..Default::default()
    };
    println!("\n  placing {id} at 50.00, far under the market");
    match client.place_order(id, &spy, &far_below) {
        Ok(()) => {
            wait(&client, &mut heard, 8);
            let changed = Order { lmt_price: 51.0, ..far_below.clone() };
            println!("  changing it to 51.00");
            let _ = client.place_order(id, &spy, &changed);
            wait(&client, &mut heard, 8);
            println!("  withdrawing it");
            let _ = client.cancel_order(id, "");
            wait(&client, &mut heard, 8);
        }
        Err(e) => println!("  it was refused before sending: {e}"),
    }
    println!("  what the venue said of it: {:?}", heard.statuses);
    for said in heard.said.iter().take(6) {
        println!("  the venue says: {said}");
    }

    client.disconnect();
}
