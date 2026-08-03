//! Diagnostic: submit a GTC stop outside RTH and report what the gateway says.
//!
//! A sell stop far below the market never triggers, and it is cancelled before
//! the probe exits.
//!
//! Usage: IB_USERNAME=… IB_PASSWORD=… cargo run --example stop_probe -- [STP|LMT]

use std::env;
use std::time::{Duration, Instant};

use ibx::api::client::{Contract, EClient, EClientConfig, Order};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Probe {
    statuses: Vec<String>,
    errors: Vec<(i64, String)>,
}

impl Wrapper for Probe {
    fn order_status(
        &mut self, order_id: i64, status: &str, filled: f64, _rem: f64, _avg: f64,
        _perm: i64, _parent: i64, _lfp: f64, _cid: i64, _why: &str, _mtp: f64,
    ) {
        println!("  order_status id={order_id} status={status} filled={filled}");
        self.statuses.push(status.into());
    }
    fn error(&mut self, req_id: i64, code: i64, msg: &str, _adv: &str) {
        if !matches!(code, 2104 | 2106 | 2158) {
            println!("  error req_id={req_id} code={code} msg={msg}");
            self.errors.push((code, msg.into()));
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let kind = env::args().nth(1).unwrap_or_else(|| "STP".into());

    let client = EClient::connect(&EClientConfig {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        host: "cdc1.ibllc.com".into(),
        paper: true,
        core_id: None,
        code_provider: None,
    })?;

    let spy = Contract {
        con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), currency: "USD".into(), ..Default::default()
    };

    let mut order = Order {
        action: "SELL".into(), total_quantity: 1.0, tif: "GTC".into(),
        outside_rth: true, ..Default::default()
    };
    match kind.as_str() {
        // Far below the market, so it rests rather than triggering.
        "STP" => { order.order_type = "STP".into(); order.aux_price = 1.0; }
        _ => { order.order_type = "LMT".into(); order.lmt_price = 9999.0; }
    }

    let oid = 90_001;
    println!("submitting {kind} GTC outsideRTH…");
    client.place_order(oid, &spy, &order)?;

    let mut probe = Probe::default();
    let deadline = Instant::now() + Duration::from_secs(25);
    while Instant::now() < deadline {
        client.process_msgs(&mut probe);
        std::thread::sleep(Duration::from_millis(20));
    }

    println!("cancelling…");
    client.cancel_order(oid, "");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        client.process_msgs(&mut probe);
        std::thread::sleep(Duration::from_millis(20));
    }

    println!("=== statuses: {:?}", probe.statuses);
    println!("=== errors:   {:?}", probe.errors);
    client.disconnect();
    Ok(())
}
