//! Diagnostic: submit one order per order type and report what the gateway
//! echoes back as its type, alongside what was asked for.
//!
//! The gateway restates the order on `open_order`, so a type that reaches it as
//! something else says so there. Every order is priced far from the market and
//! cancelled before the next one goes out.
//!
//! Usage: IB_USERNAME=… IB_PASSWORD=… cargo run --example ordtype_probe

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ibx::api::client::{Contract, EClient, EClientConfig, Order};
use ibx::api::types::OrderState;
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Probe {
    echoed: Arc<Mutex<HashMap<i64, String>>>,
    status: Arc<Mutex<HashMap<i64, String>>>,
    errors: Arc<Mutex<Vec<(i64, i64, String)>>>,
}

impl Wrapper for Probe {
    fn open_order(&mut self, order_id: i64, _c: &Contract, o: &Order, _s: &OrderState) {
        self.echoed.lock().unwrap().insert(order_id, o.order_type.clone());
    }
    fn order_status(
        &mut self, order_id: i64, status: &str, _f: f64, _r: f64, _a: f64,
        _p: i64, _pa: i64, _l: f64, _c: i64, _w: &str, _m: f64,
    ) {
        self.status.lock().unwrap().insert(order_id, status.into());
    }
    fn error(&mut self, req_id: i64, code: i64, msg: &str, _adv: &str) {
        if !matches!(code, 2104 | 2106 | 2158) {
            self.errors.lock().unwrap().push((req_id, code, msg.into()));
        }
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

    // (order type, lmt, aux) — all far from the market so none can trigger.
    let cases: &[(&str, f64, f64)] = &[
        ("LMT", 1.0, 0.0),
        ("STP", 0.0, 1.0),
        ("MIT", 0.0, 1.0),
        ("LIT", 1.0, 1.0),
        ("STP PRT", 0.0, 1.0),
        ("MKT PRT", 0.0, 0.0),
        ("SNAP MKT", 0.0, 0.0),
        ("SNAP MID", 0.0, 0.0),
        ("SNAP PRI", 0.0, 0.0),
    ];

    let mut probe = Probe::default();
    let (echoed, status, errors) =
        (probe.echoed.clone(), probe.status.clone(), probe.errors.clone());

    let mut oid = 92_001i64;
    let mut asked: Vec<(i64, &str)> = Vec::new();
    for (ty, lmt, aux) in cases {
        let order = Order {
            action: "SELL".into(), total_quantity: 1.0, order_type: (*ty).into(),
            lmt_price: *lmt, aux_price: *aux, tif: "GTC".into(), outside_rth: true,
            ..Default::default()
        };
        if client.place_order(oid, &spy, &order).is_ok() {
            asked.push((oid, ty));
        }
        let until = Instant::now() + Duration::from_secs(4);
        while Instant::now() < until {
            client.process_msgs(&mut probe);
            std::thread::sleep(Duration::from_millis(20));
        }
        // `open_order` answers a request; without one the gateway's restatement
        // of the order, which is the whole measurement, never arrives.
        client.req_open_orders(&mut probe);
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            client.process_msgs(&mut probe);
            std::thread::sleep(Duration::from_millis(20));
        }
        client.cancel_order(oid, "");
        oid += 1;
    }

    let until = Instant::now() + Duration::from_secs(12);
    while Instant::now() < until {
        client.process_msgs(&mut probe);
        std::thread::sleep(Duration::from_millis(20));
    }

    let (e, s) = (echoed.lock().unwrap(), status.lock().unwrap());
    println!("\n{:<10} {:<12} {:<14} {}", "asked", "echoed back", "status", "verdict");
    for (id, ty) in &asked {
        let back = e.get(id).cloned().unwrap_or_else(|| "-".into());
        let st = s.get(id).cloned().unwrap_or_else(|| "-".into());
        let verdict = if back == "-" { "no echo" }
            else if back.eq_ignore_ascii_case(ty) { "match" } else { "MISMATCH" };
        println!("{ty:<10} {back:<12} {st:<14} {verdict}");
    }
    for (id, code, msg) in errors.lock().unwrap().iter() {
        println!("  err id={id} code={code} {msg}");
    }
    drop(e); drop(s);
    client.disconnect();
    Ok(())
}
