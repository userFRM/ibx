//! Whether the venue streams the trade series it is asked for by name.
//!
//! Both trade streams are asked for here under one name and told apart
//! afterwards, on the grounds that the venue acknowledges the other name and
//! sends nothing. The vendor build states them apart — `Last` and `AllLast`
//! are distinct wire values — so the grounds are worth re-checking against a
//! contract that is trading.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin probe_tbt_last

use std::time::{Duration, Instant};
use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard { ticks: usize, said: Vec<String> }
impl Wrapper for Heard {
    fn tick_by_tick_all_last(
        &mut self, _r: i64, _t: i32, _time: i64, _price: f64, _size: f64,
        _attrib: &ibx::api::types::TickAttribLast, _exch: &str, _cond: &str,
    ) { self.ticks += 1; }
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
        symbol: "MES".into(), sec_type: "FUT".into(), exchange: "CME".into(),
        currency: "USD".into(),
        last_trade_date_or_contract_month: "202709".into(), ..Default::default() };
    let resolved = client.qualify_contract(&c).expect("named");
    let mut heard = Heard::default();
    for (req, kind) in [(1i64, "AllLast"), (2, "Last")] {
        let before = heard.ticks;
        if let Err(e) = client.req_tick_by_tick_data(req, &resolved, kind, 0, false) {
            println!("  {kind:8} refused: {e}"); continue;
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = client.cancel_tick_by_tick_data(req);
        println!("  {kind:8} {} trades in 20s", heard.ticks - before);
    }
    for s in heard.said.iter().take(4) { println!("  said: {s}"); }
}
