//! Per-request market-data mode (FIX field 9887).
//!
//! Subscribes to a thinly-traded ticker with parallel realtime + frozen subs
//! And prints which feed delivers ticks. Frozen sub keeps streaming after-hours
//! When realtime is silent.
//!
//! Usage:
//!   IB_USERNAME=user IB_PASSWORD=pass cargo run --example per_request_mode

use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ibx::api::client::{Contract, EClient, EClientConfig};
use ibx::api::types::TickAttrib;
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Counts {
    realtime_ticks: u64,
    frozen_ticks: u64,
    delayed_frozen_ticks: u64,
}

struct PrintWrapper {
    counts: Arc<Mutex<Counts>>,
}

impl Wrapper for PrintWrapper {
    fn tick_price(&mut self, req_id: i64, _tick_type: i32, price: f64, _attrib: &TickAttrib) {
        let mut c = self.counts.lock().unwrap();
        let label = match req_id {
            1 => { c.realtime_ticks += 1; "realtime" }
            2 => { c.frozen_ticks += 1; "frozen" }
            3 => { c.delayed_frozen_ticks += 1; "delayed_frozen" }
            _ => "?",
        };
        println!("[{label:>14}] req_id={req_id} price={price:.4}");
    }

    fn tick_size(&mut self, req_id: i64, _tick_type: i32, size: f64) {
        let mut c = self.counts.lock().unwrap();
        let label = match req_id {
            1 => { c.realtime_ticks += 1; "realtime" }
            2 => { c.frozen_ticks += 1; "frozen" }
            3 => { c.delayed_frozen_ticks += 1; "delayed_frozen" }
            _ => "?",
        };
        println!("[{label:>14}] req_id={req_id} size={size:.0}");
    }

    fn market_data_type(&mut self, req_id: i64, mdt: i32) {
        println!("[market_data_type] req_id={req_id} mdt={mdt}");
    }

    fn error(&mut self, req_id: i64, code: i64, msg: &str, _adv: &str) {
        eprintln!("[error] req_id={req_id} code={code} msg={msg}");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let username = env::var("IB_USERNAME")?;
    let password = env::var("IB_PASSWORD")?;
    let host = env::var("IB_HOST").unwrap_or_else(|_| "cdc1.ibllc.com".to_string());
    let symbol = env::var("SYMBOL").unwrap_or_else(|_| "AAPL".to_string());
    let con_id: i64 = env::var("CON_ID").unwrap_or_else(|_| "265598".to_string()).parse()?;
    let duration: u64 = env::var("DURATION_SECS").unwrap_or_else(|_| "20".to_string()).parse()?;

    let client = EClient::connect(&EClientConfig {
        username, password, host, paper: true, core_id: None, code_provider: None,
        ..Default::default()
    })?;

    let contract = Contract { con_id, symbol: symbol.clone(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "USD".into(), ..Default::default() };

    let counts = Arc::new(Mutex::new(Counts::default()));
    let mut wrapper = PrintWrapper { counts: counts.clone() };

    // One at a time, because a contract holds one subscription at a time
    // Each mode gets the same stretch of wall clock, so the counts
    // below are comparable.
    let each = Duration::from_secs((duration / 3).max(1));
    for (req_id, mode, label) in [(1i64, 0i32, "realtime"), (2, 2, "frozen"), (3, 3, "delayed_frozen")] {
        println!("Subscribing to {symbol} (con_id={con_id}) as {label}...");
        client.req_mkt_data_ex(req_id, &contract, "", false, false, mode)?;
        let deadline = Instant::now() + each;
        while Instant::now() < deadline {
            client.process_msgs(&mut wrapper);
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = client.cancel_mkt_data(req_id);
        // The cancel has to reach the engine before the next mode claims the
        // contract, or the next subscription is refused as a duplicate.
        let settle = Instant::now() + Duration::from_millis(500);
        while Instant::now() < settle {
            client.process_msgs(&mut wrapper);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    client.disconnect();

    let c = counts.lock().unwrap();
    println!("\n── Tick counts after {duration}s ──");
    println!("  realtime       : {}", c.realtime_ticks);
    println!("  frozen         : {}", c.frozen_ticks);
    println!("  delayed_frozen : {}", c.delayed_frozen_ticks);

    Ok(())
}
