//! What the venue answers when asked for its own chargeable snapshot.
//!
//! The request type is the venue's, not this client's: an account without the
//! entitlement is refused by name, which is what this reports. On an entitled
//! account the same run reports the quote the snapshot delivered instead.
use std::time::{Duration, Instant};
use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;
#[derive(Default)]
struct Heard { ended: Vec<i64>, said: Vec<String> }
impl Wrapper for Heard {
    fn tick_snapshot_end(&mut self, req_id: i64) { self.ended.push(req_id); }
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
    let subjects = [
        ("SPY", Contract { symbol: "SPY".into(), sec_type: "STK".into(),
            exchange: "SMART".into(), currency: "USD".into(), ..Default::default() }),
        ("EUR.USD", Contract { symbol: "EUR".into(), sec_type: "CASH".into(),
            exchange: "IDEALPRO".into(), currency: "USD".into(), ..Default::default() }),
    ];
    let mut heard = Heard::default();
    let mut req = 600i64;
    for (what, c) in subjects {
        req += 1;
        let resolved = match client.qualify_contract(&c) {
            Ok(r) => r, Err(e) => { println!("  {what:8} not named: {e}"); continue } };
        if let Err(e) = client.req_mkt_data_ex(req, &resolved, "", false, true, 0) {
            println!("  {what:8} refused here: {e}"); continue;
        }
        let deadline = Instant::now() + Duration::from_secs(13);
        let mut q = ibx::types::Quote::default();
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            if let Some(live) = client.quote(req) { q = live; }
            if heard.ended.contains(&req) { break; }
            std::thread::sleep(Duration::from_millis(50));
        }
        let sc = ibx::types::PRICE_SCALE as f64;
        println!("  {what:8} bid={:.2} ask={:.2} last={:.2}",
            q.bid as f64 / sc, q.ask as f64 / sc, q.last as f64 / sc);
        match heard.said.iter().find(|s| s.starts_with(&format!("{req}/"))) {
            Some(s) => println!("           venue said: {s}"),
            None => println!("           venue refused nothing"),
        }
    }
}
