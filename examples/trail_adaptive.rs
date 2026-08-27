//! Verify the TRAIL, TRAIL LIMIT and Adaptive wire shapes on paper.
//!
//! Submits one of each order type via EClient and waits for a non-rejected
//! Status, then cancels. Pass = orderStatus reaches "PreSubmitted" or
//! "Submitted" without an error/reject for that req_id.

use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ibx::api::client::{Contract, EClient, EClientConfig, Order};
use ibx::api::wrapper::Wrapper;

#[derive(Default, Clone, Debug)]
struct State {
    statuses: Vec<(i64, String)>,           // (order_id, status)
    rejects: Vec<(i64, i64, String)>,       // (req_id, code, msg) errors via error()
}

struct ProbeWrapper {
    state: Arc<Mutex<State>>,
}

impl Wrapper for ProbeWrapper {
    fn order_status(
        &mut self,
        order_id: i64,
        status: &str,
        _filled: f64,
        _remaining: f64,
        _avg_fill_price: f64,
        _perm_id: i64,
        _parent_id: i64,
        _last_fill_price: f64,
        _client_id: i64,
        _why_held: &str,
        _mkt_cap_price: f64,
    ) {
        println!("[order_status] id={order_id} status={status}");
        self.state.lock().unwrap().statuses.push((order_id, status.into()));
    }
    fn error(&mut self, req_id: i64, code: i64, msg: &str, _adv: &str) {
        eprintln!("[error] req_id={req_id} code={code} msg={msg}");
        self.state.lock().unwrap().rejects.push((req_id, code, msg.into()));
    }
}

fn aapl() -> Contract {
    Contract {
        con_id: 265598,
        symbol: "AAPL".into(),
        sec_type: "STK".into(),
        exchange: "SMART".into(),
        currency: "USD".into(),
        ..Default::default()
    }
}

fn run_one(
    client: &EClient,
    state: &Arc<Mutex<State>>,
    wrapper: &mut ProbeWrapper,
    label: &str,
    order_id: i64,
    order: Order,
) -> bool {
    println!("\n== {label} (order_id={order_id})");
    state.lock().unwrap().statuses.clear();
    state.lock().unwrap().rejects.clear();

    if let Err(e) = client.place_order(order_id, &aapl(), &order) {
        eprintln!("  place_order failed: {e}");
        return false;
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut accepted = false;
    let mut rejected_for_us = false;
    while Instant::now() < deadline {
        client.process_msgs(wrapper);
        let s = state.lock().unwrap();
        for (rid, _code, _msg) in &s.rejects {
            if *rid == order_id {
                rejected_for_us = true;
            }
        }
        for (oid, st) in &s.statuses {
            if *oid == order_id && (st == "PreSubmitted" || st == "Submitted") {
                accepted = true;
            }
        }
        drop(s);
        if accepted || rejected_for_us { break; }
        std::thread::sleep(Duration::from_millis(20));
    }

    if accepted {
        println!("  -> accepted (will cancel)");
        let _ = client.cancel_order(order_id, "");
        // Drain briefly, so the cancel ack is visible.
        let cancel_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < cancel_deadline {
            client.process_msgs(wrapper);
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    } else if rejected_for_us {
        println!("  -> REJECTED");
        false
    } else {
        println!("  -> TIMEOUT (no ack and no reject)");
        false
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = ibx::logging::try_init_from_env("error");

    let username = env::var("IB_USERNAME")?;
    let password = env::var("IB_PASSWORD")?;
    let host = env::var("IB_HOST").unwrap_or_else(|_| "cdc1.ibllc.com".to_string());

    println!("== Connecting to paper ({host})...");
    let t0 = Instant::now();
    let client = EClient::connect(&EClientConfig {
        username, password, host, paper: true, core_id: None, code_provider: None,
        ..Default::default()
    })?;
    println!("== Connected in {:.1}s", t0.elapsed().as_secs_f64());

    let state = Arc::new(Mutex::new(State::default()));
    let mut wrapper = ProbeWrapper { state: state.clone() };

    let next_id = || -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64
    };

    // 1) TRAIL — Sell 1 AAPL @ trail $2.00
    let trail = Order {
        action: "SELL".into(),
        order_type: "TRAIL".into(),
        total_quantity: 1.0,
        aux_price: 2.0,
        tif: "DAY".into(),
        ..Order::default()
    };
    let p1 = run_one(&client, &state, &mut wrapper, "TRAIL", next_id(), trail);

    std::thread::sleep(Duration::from_millis(500));

    // 2) TRAIL LIMIT — Sell 1 AAPL @ trail $2.00, lmt offset $0.50
    let trail_lmt = Order {
        action: "SELL".into(),
        order_type: "TRAIL LIMIT".into(),
        total_quantity: 1.0,
        aux_price: 2.0,
        lmt_price_offset: 0.50,
        tif: "DAY".into(),
        ..Order::default()
    };
    let p2 = run_one(&client, &state, &mut wrapper, "TRAIL LIMIT", next_id(), trail_lmt);

    std::thread::sleep(Duration::from_millis(500));

    // 3) Adaptive Limit — Buy 1 AAPL @ $1.00 (won't fill), Adaptive Normal
    let adaptive = Order {
        action: "BUY".into(),
        order_type: "LMT".into(),
        total_quantity: 1.0,
        lmt_price: 1.0,
        tif: "DAY".into(),
        algo_strategy: "Adaptive".into(),
        algo_params: vec![ibx::api::types::TagValue {
            tag: "adaptivePriority".into(),
            value: "Normal".into(),
        }],
        ..Order::default()
    };
    let p3 = run_one(&client, &state, &mut wrapper, "Adaptive Limit", next_id(), adaptive);

    println!("\n== Summary ==");
    println!("  TRAIL          : {}", if p1 { "PASS" } else { "FAIL" });
    println!("  TRAIL LIMIT    : {}", if p2 { "PASS" } else { "FAIL" });
    println!("  Adaptive Limit : {}", if p3 { "PASS" } else { "FAIL" });

    client.disconnect();

    if p1 && p2 && p3 {
        println!("\nALL PASS");
        Ok(())
    } else {
        Err("One or more order types failed".into())
    }
}
