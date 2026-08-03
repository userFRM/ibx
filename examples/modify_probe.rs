//! Diagnostic: submit a resting limit, modify it, and report every status the
//! gateway sends for each leg. Answers whether a replace is acknowledged at all
//! and, if so, under which order id.
//!
//! The limit sits far below the market so it never fills, and it is cancelled
//! before the probe exits.
//!
//! Usage: IB_USERNAME=… IB_PASSWORD=… cargo run --example modify_probe

use std::env;
use std::time::{Duration, Instant};

use ibx::api::client::{Contract, EClient, EClientConfig, Order};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Probe {
    seen: Vec<(i64, String, f64)>,
}

impl Wrapper for Probe {
    fn order_status(
        &mut self, order_id: i64, status: &str, filled: f64, remaining: f64, _avg: f64,
        perm: i64, _parent: i64, _lfp: f64, _cid: i64, _why: &str, _mtp: f64,
    ) {
        println!("  status id={order_id} {status} filled={filled} rem={remaining} perm={perm}");
        self.seen.push((order_id, status.into(), remaining));
    }
    fn open_order(&mut self, order_id: i64, _c: &Contract, o: &Order, _st: &ibx::api::types::OrderState) {
        println!("  open_order id={order_id} type={} lmt={} qty={}", o.order_type, o.lmt_price, o.total_quantity);
    }
    fn error(&mut self, req_id: i64, code: i64, msg: &str, _adv: &str) {
        if !matches!(code, 2104 | 2106 | 2158) {
            println!("  error req_id={req_id} code={code} msg={msg}");
        }
    }
}

fn pump(client: &EClient, probe: &mut Probe, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        client.process_msgs(probe);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
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
        action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
        lmt_price: 1.00, tif: "GTC".into(), outside_rth: true, ..Default::default()
    };

    let oid = 91_001;
    let mut probe = Probe::default();

    println!("== submit  lmt=1.00");
    client.place_order(oid, &spy, &order)?;
    pump(&client, &mut probe, 20);
    let after_submit = probe.seen.len();

    println!("== modify  lmt=2.00 (same order id, which is the replace)");
    order.lmt_price = 2.00;
    client.place_order(oid, &spy, &order)?;
    pump(&client, &mut probe, 25);
    let after_modify = probe.seen.len();

    println!("== cancel");
    client.cancel_order(oid, "");
    pump(&client, &mut probe, 15);

    println!("=== statuses before the modify: {after_submit}");
    println!("=== statuses the modify produced: {}", after_modify - after_submit);
    println!("=== all: {:?}", probe.seen);
    client.disconnect();
    Ok(())
}
