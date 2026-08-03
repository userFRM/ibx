//! Diagnostic: connect the way a caller does and report whether account values
//! ever arrive, and how long they take.
//!
//! Usage: IB_USERNAME=… IB_PASSWORD=… cargo run --example account_probe -- [secs]

use std::env;
use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Probe {
    summary: Vec<(String, String)>,
    account_value: Vec<(String, String)>,
}

impl Wrapper for Probe {
    fn account_summary(&mut self, _req_id: i64, _acct: &str, tag: &str, value: &str, _cur: &str) {
        self.summary.push((tag.into(), value.into()));
    }
    fn update_account_value(&mut self, key: &str, val: &str, _cur: &str, _acct: &str) {
        self.account_value.push((key.into(), val.into()));
    }
    fn error(&mut self, req_id: i64, code: i64, msg: &str, _adv: &str) {
        if !matches!(code, 2104 | 2106 | 2158) {
            eprintln!("[error] req_id={req_id} code={code} msg={msg}");
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let secs: u64 = env::args().nth(1).map_or(30, |s| s.parse().unwrap());

    let t0 = Instant::now();
    let client = EClient::connect(&EClientConfig {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        host: "cdc1.ibllc.com".into(),
        paper: true,
        core_id: None,
        code_provider: None,
    })?;
    eprintln!("connected in {:.1}s", t0.elapsed().as_secs_f64());

    // ibapi requires the subscription before update_account_value fires, and a
    // req_id'd summary before account_summary does. Ask for both, then also read
    // the engine's own stored state, so a callback gap and an empty engine can be
    // told apart.
    let mut probe = Probe::default();
    // Drain whatever the logon burst delivers, then ask again. What arrives
    // after the reset is the re-subscribe answering, not the burst.
    let drain = Instant::now() + Duration::from_secs(20);
    while Instant::now() < drain {
        client.process_msgs(&mut probe);
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("from the logon burst: {} value(s)", probe.account_value.len());
    probe.account_value.clear();
    probe.summary.clear();
    client.req_account_updates(true, "");
    eprintln!("re-subscribe sent at {:.1}s", t0.elapsed().as_secs_f64());

    let mut first_value_at: Option<f64> = None;
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        client.process_msgs(&mut probe);
        if first_value_at.is_none() && !(probe.summary.is_empty() && probe.account_value.is_empty()) {
            first_value_at = Some(t0.elapsed().as_secs_f64());
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    println!("=== update_account_value: {} ===", probe.account_value.len());
    for (k, v) in probe.account_value.iter().take(24) {
        println!("  {k} = {v}");
    }
    println!("=== account_summary: {} ===", probe.summary.len());
    for (k, v) in probe.summary.iter().take(24) {
        println!("  {k} = {v}");
    }
    let acct = client.account();
    println!("=== engine stored state ===");
    println!("  net_liquidation = {}", acct.net_liquidation);
    println!("  buying_power    = {}", acct.buying_power);
    println!("  available_funds = {}", acct.available_funds);
    match first_value_at {
        Some(t) => println!("first account value {t:.1}s after connect started"),
        None => println!("NO account value in {secs}s"),
    }
    client.disconnect();
    Ok(())
}
