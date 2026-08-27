//! Settle which way round a combination's legs go.
//!
//! A vertical call spread bought — the nearer strike bought, the further sold
//! — costs money and risks what it cost. Sold, it takes money in and risks the
//! distance between the strikes. The venue prices both, so asking it which one
//! it thinks it has been handed settles the convention: the margin it quotes
//! says which side it read.
//!
//! Reads only. The venue prices these and places nothing.

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::{ComboLeg, Contract, Order};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    priced: Vec<(i64, String, String, String)>,
    said: Vec<String>,
}

impl Wrapper for Heard {
    fn open_order(
        &mut self, id: i64, _c: &Contract, _o: &Order,
        state: &ibx::api::types::OrderState,
    ) {
        self.priced.push((
            id,
            state.status.clone(),
            state.init_margin_change.clone(),
            state.maint_margin_change.clone(),
        ));
    }
    fn error(&mut self, _r: i64, code: i64, message: &str, _a: &str) {
        if !matches!(code, 2104 | 2106 | 2158 | 2107) {
            self.said.push(format!("{code}: {}", message.lines().next().unwrap_or("")));
        }
    }
}

fn option(strike: f64) -> Contract {
    Contract {
        symbol: "SPY".to_string(),
        sec_type: "OPT".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        last_trade_date_or_contract_month: "20260918".to_string(),
        strike,
        right: "C".to_string(),
        ..Default::default()
    }
}

fn main() {
    let _ = ibx::logging::try_init_from_env("error");
    if std::env::var("IB_USERNAME").unwrap_or_default().trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
        std::process::exit(2);
    }
    let client = match EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true,
        ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => { eprintln!("could not open a session: {e}"); std::process::exit(1); }
    };
    println!("session open");

    let near = client.qualify_contract(&option(770.0)).expect("the nearer strike resolves");
    let far = client.qualify_contract(&option(780.0)).expect("the further strike resolves");
    println!("  bought leg {} at 770, sold leg {} at 780", near.con_id, far.con_id);

    let leg = |con_id: i64, action: &str| ComboLeg {
        con_id,
        ratio: 1,
        action: action.to_string(),
        exchange: "SMART".to_string(),
        open_close: 0,
        shorting_policy: 0,
        designated_location: String::new(),
        exempt_code: -1,
    };
    let spread = |legs: Vec<ComboLeg>| Contract {
        symbol: "SPY".to_string(),
        sec_type: "BAG".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        combo_legs: legs,
        ..Default::default()
    };

    let id = client.next_order_id();

    let mut heard = Heard::default();
    let cases = [
        ("bought: buy the nearer, sell the further", vec![
            leg(near.con_id, "BUY"), leg(far.con_id, "SELL"),
        ]),
        ("sold: sell the nearer, buy the further", vec![
            leg(far.con_id, "BUY"), leg(near.con_id, "SELL"),
        ]),
    ];
    for (n, (what, legs)) in cases.into_iter().enumerate() {
        let order = Order {
            action: "BUY".into(),
            order_type: "LMT".into(),
            total_quantity: 1.0,
            lmt_price: 1.0,
            what_if: true,
            ..Default::default()
        };
        println!("\n  {what}");
        match client.place_order(id + n as i64, &spread(legs), &order) {
            Ok(()) => {}
            Err(e) => { println!("    refused before sending: {e}"); continue }
        }
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            std::thread::sleep(Duration::from_millis(200));
        }
        for (id, status, init, maint) in heard.priced.drain(..) {
            println!("    {id}: {status}  initial margin {init}  maintenance {maint}");
        }
        for said in heard.said.drain(..).take(2) {
            println!("    the venue says: {said}");
        }
    }

    client.disconnect();
}
