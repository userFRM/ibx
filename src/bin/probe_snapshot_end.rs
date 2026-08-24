//! How long a snapshot takes to end, and on what.
use std::time::{Duration, Instant};
use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;
#[derive(Default)]
struct Heard { ended: Vec<(i64, Instant)>, said: Vec<String> }
impl Wrapper for Heard {
    fn tick_snapshot_end(&mut self, req_id: i64) { self.ended.push((req_id, Instant::now())); }
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
        ("a future, quoting now", Contract {
            symbol: "MES".into(), sec_type: "FUT".into(), exchange: "CME".into(),
            currency: "USD".into(),
            last_trade_date_or_contract_month: "202709".into(), ..Default::default() }),
        ("a share, market shut", Contract {
            symbol: "SPY".into(), sec_type: "STK".into(), exchange: "SMART".into(),
            currency: "USD".into(), ..Default::default() }),
        ("a pair with no last", Contract {
            symbol: "EUR".into(), sec_type: "CASH".into(), exchange: "IDEALPRO".into(),
            currency: "USD".into(), ..Default::default() }),
    ];
    let mut heard = Heard::default();
    let mut req = 10i64;
    for (what, c) in subjects {
        req += 1;
        let resolved = match client.qualify_contract(&c) {
            Ok(r) => r, Err(e) => { println!("  {what:24} not named: {e}"); continue } };
        let asked = Instant::now();
        if let Err(e) = client.req_mkt_data(req, &resolved, "", true, false) {
            println!("  {what:24} refused: {e}"); continue;
        }
        let deadline = Instant::now() + Duration::from_secs(16);
        // Kept as it stood while the snapshot was still open: the end of one
        // takes the subscription down, and the quote goes with it.
        let mut q = ibx::types::Quote::default();
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            if let Some(live) = client.quote(req) { q = live; }
            if heard.ended.iter().any(|(r, _)| *r == req) { break; }
            std::thread::sleep(Duration::from_millis(50));
        }
        let sc = ibx::types::PRICE_SCALE as f64;
        let miss = |v: i64| if v > 0 { "" } else { " MISSING" };
        match heard.ended.iter().find(|(r, _)| *r == req) {
            Some((_, at)) => println!("  {what:24} ended after {:?}", at.duration_since(asked)),
            None => println!("  {what:24} never ended"),
        }
        println!(
            "      bid={:.2}{} ask={:.2}{} last={:.2}{} open={:.2}{} close={:.2}{}",
            q.bid as f64 / sc, miss(q.bid), q.ask as f64 / sc, miss(q.ask),
            q.last as f64 / sc, miss(q.last), q.open as f64 / sc, miss(q.open),
            q.close as f64 / sc, miss(q.close),
        );
    }
    for s in heard.said.iter().take(3) { println!("  said: {s}"); }
}
