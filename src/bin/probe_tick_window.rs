//! Whether the venue serves a tick window named from either side.
//!
//! Historical ticks were asked for by their end alone, and a request naming
//! only a start was refused before it went out. The venue holds the two apart
//! and the reference client passes both, so this asks it all three ways.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin probe_tick_window

use std::time::{Duration, Instant};
use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard { ticks: usize, done: bool, said: Vec<String> }
impl Wrapper for Heard {
    fn historical_ticks_last(
        &mut self, _r: i64, ticks: &ibx::types::HistoricalTickData, done: bool,
    ) {
        self.ticks += match ticks {
            ibx::types::HistoricalTickData::Last(v) => v.len(),
            ibx::types::HistoricalTickData::Midpoint(v) => v.len(),
            ibx::types::HistoricalTickData::BidAsk(v) => v.len(),
        };
        self.done |= done;
    }
    fn error(&mut self, r: i64, c: i64, m: &str, _: &str) {
        if !matches!(c, 2104|2106|2107|2119|2158) { self.said.push(format!("{r}/{c}: {m}")); }
    }
}

fn main() {
    let _ = env_logger::try_init();
    let client = EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true, ..Default::default() }).expect("session");
    println!("session open");
    let c = Contract {
        symbol: "AAPL".into(), sec_type: "STK".into(), exchange: "SMART".into(),
        currency: "USD".into(), ..Default::default() };
    let resolved = client.qualify_contract(&c).expect("named");

    let day = std::env::var("IBX_DAY").unwrap_or_else(|_| "20260821".into());
    let cases = [
        ("from a start", format!("{day} 14:30:00"), String::new()),
        ("to an end", String::new(), format!("{day} 20:00:00")),
        ("between both", format!("{day} 14:30:00"), format!("{day} 14:35:00")),
        ("neither", String::new(), String::new()),
    ];
    let mut req = 0i64;
    for (what, start, end) in cases {
        req += 1;
        let mut heard = Heard::default();
        match client.req_historical_ticks(req, &resolved, &start, &end, 100, "TRADES", true) {
            Ok(()) => {}
            Err(e) => { println!("  {what:14} refused before sending: {e}"); continue; }
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            if heard.done { break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        println!(
            "  {what:14} {} ticks{}",
            heard.ticks,
            heard.said.first().map(|s| format!("  — {s}")).unwrap_or_default(),
        );
    }
}
