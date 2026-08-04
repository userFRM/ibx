//! Read-only watch across the nightly server maintenance window.
//!
//! Subscribes to instruments that trade around the clock, then reports what
//! happens to the connection and whether the data comes back on its own. Places
//! nothing and cancels nothing.
//!
//! Usage: IB_USERNAME=... IB_PASSWORD=... PROBE_MINUTES=60 \
//!        cargo run --example probe_overnight_survival

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ibx::api::client::{Contract, EClient, EClientConfig};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Counts {
    ticks: AtomicU64,
    bars: AtomicU64,
    lost: AtomicU64,
    restored: AtomicU64,
}

struct W {
    c: Arc<Counts>,
    con_id: i64,
}

fn stamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}Z")
}

impl Wrapper for W {
    fn tick_price(&mut self, _req_id: i64, _field: i32, _price: f64, _attrib: &ibx::api::types::TickAttrib) {
        self.c.ticks.fetch_add(1, Ordering::Relaxed);
    }
    fn tick_size(&mut self, _req_id: i64, _field: i32, _size: f64) {
        self.c.ticks.fetch_add(1, Ordering::Relaxed);
    }
    fn real_time_bar(
        &mut self, _req_id: i64, _date: i64, _o: f64, _h: f64, _l: f64, _c: f64,
        _v: f64, _wap: f64, _count: i32,
    ) {
        self.c.bars.fetch_add(1, Ordering::Relaxed);
    }
    fn error(&mut self, req_id: i64, code: i64, msg: &str, _adv: &str) {
        // 1100 lost, 1101/1102 restored. These are the whole point of the watch.
        match code {
            1100 => {
                self.c.lost.fetch_add(1, Ordering::Relaxed);
                println!("[{}] CONNECTION LOST   (1100) {msg}", stamp());
            }
            1101 | 1102 => {
                self.c.restored.fetch_add(1, Ordering::Relaxed);
                println!("[{}] CONNECTION BACK   ({code}) {msg}", stamp());
            }
            2104 | 2106 | 2158 => println!("[{}] farm ok ({code}) {msg}", stamp()),
            2103 | 2105 | 2157 => println!("[{}] farm broken ({code}) {msg}", stamp()),
            _ => println!("[{}] error req={req_id} code={code}: {msg}", stamp()),
        }
    }
    fn connection_closed(&mut self) {
        println!("[{}] connection_closed()", stamp());
    }
    fn contract_details(&mut self, _req_id: i64, d: &ibx::api::types::ContractDetails) {
        if self.con_id == 0 { self.con_id = d.contract.con_id; }
    }
    fn position(&mut self, account: &str, contract: &ibx::api::types::Contract, pos: f64, avg: f64) {
        println!("[{}] position {} {} qty={pos} avg={avg}", stamp(), account, contract.symbol);
    }
    fn position_end(&mut self) {
        println!("[{}] position_end", stamp());
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let minutes: u64 = env::var("PROBE_MINUTES").ok().and_then(|s| s.parse().ok()).unwrap_or(60);

    let client = EClient::connect(&EClientConfig {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        host: "cdc1.ibllc.com".into(),
        paper: true,
        core_id: None,
        code_provider: None,
    })?;
    println!("[{}] connected (paper), watching for {minutes} minutes", stamp());

    let c = Arc::new(Counts::default());
    let mut w = W { c: c.clone(), con_id: 0 };

    // Around-the-clock instruments, so silence means the connection rather than
    // the session being shut.
    let fx = Contract {
        symbol: "EUR".into(), sec_type: "CASH".into(),
        exchange: "IDEALPRO".into(), currency: "USD".into(),
        ..Default::default()
    };
    // The bar subscription is keyed by contract id, so resolve it first.
    client.req_contract_details(1, &fx)?;
    let resolve = Instant::now() + Duration::from_secs(20);
    while Instant::now() < resolve && w.con_id == 0 {
        client.process_msgs(&mut w);
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("[{}] EUR.USD con_id={}", stamp(), w.con_id);
    let fx = Contract { con_id: w.con_id, ..fx };

    client.req_mkt_data(101, &fx, "", false, false)?;
    client.req_real_time_bars(201, &fx, 5, "MIDPOINT", false)?;
    client.req_positions(&mut w);

    let end = Instant::now() + Duration::from_secs(minutes * 60);
    let mut next_report = Instant::now() + Duration::from_secs(120);
    let (mut last_ticks, mut last_bars) = (0u64, 0u64);
    while Instant::now() < end {
        client.process_msgs(&mut w);
        if Instant::now() >= next_report {
            let (t, b) = (c.ticks.load(Ordering::Relaxed), c.bars.load(Ordering::Relaxed));
            println!(
                "[{}] ticks={t} (+{}) bars={b} (+{}) lost={} restored={}",
                stamp(), t - last_ticks, b - last_bars,
                c.lost.load(Ordering::Relaxed), c.restored.load(Ordering::Relaxed),
            );
            last_ticks = t;
            last_bars = b;
            next_report = Instant::now() + Duration::from_secs(120);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    println!(
        "\n[{}] done: ticks={} bars={} lost={} restored={}",
        stamp(),
        c.ticks.load(Ordering::Relaxed), c.bars.load(Ordering::Relaxed),
        c.lost.load(Ordering::Relaxed), c.restored.load(Ordering::Relaxed),
    );
    client.disconnect();
    Ok(())
}
