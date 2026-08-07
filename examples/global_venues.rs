//! Recipe: resolve the same kind of instrument on venues around the world.
//!
//! Every other recipe here trades a US contract quoted in dollars. This one
//! asks the venue to define a share on eight exchanges across seven
//! currencies, and prints what came back for each.
//!
//! Contract definitions are reference data: the venue answers for Tokyo at
//! midnight in Tokyo, so this runs at any hour and does not need the markets
//! it names to be trading.
//!
//! Usage: IB_USERNAME=... IB_PASSWORD=... cargo run --example global_venues

use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ibx::api::client::{Contract, EClient, EClientConfig};
use ibx::api::types::ContractDetails;
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct State {
    rows: Vec<(i64, ContractDetails)>,
    ended: Vec<i64>,
    errors: Vec<(i64, i64, String)>,
}

struct W {
    state: Arc<Mutex<State>>,
}

impl Wrapper for W {
    fn contract_details(&mut self, req_id: i64, details: &ContractDetails) {
        self.state.lock().unwrap().rows.push((req_id, details.clone()));
    }
    fn contract_details_end(&mut self, req_id: i64) {
        self.state.lock().unwrap().ended.push(req_id);
    }
    fn error(&mut self, req_id: i64, code: i64, msg: &str, _adv: &str) {
        self.state.lock().unwrap().errors.push((req_id, code, msg.to_string()));
    }
}

/// Symbol, currency, and the venue the listing is expected to sit on.
const VENUES: &[(&str, &str, &str)] = &[
    ("AAPL", "USD", "NASDAQ"),
    ("VOD", "GBP", "LSE"),
    ("SAP", "EUR", "IBIS"),
    ("ASML", "EUR", "AEB"),
    ("NESN", "CHF", "EBS"),
    ("7203", "JPY", "TSEJ"),
    ("700", "HKD", "SEHK"),
    ("BHP", "AUD", "ASX"),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = EClient::connect(&EClientConfig {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        host: env::var("IB_HOST").unwrap_or_else(|_| "cdc1.ibllc.com".into()),
        paper: true,
        core_id: None,
        code_provider: None,
        ..Default::default()
    })?;

    let state = Arc::new(Mutex::new(State::default()));
    let mut wrapper = W { state: state.clone() };

    for (i, (symbol, currency, _)) in VENUES.iter().enumerate() {
        client.req_contract_details(
            i as i64 + 1,
            &Contract {
                symbol: (*symbol).into(),
                sec_type: "STK".into(),
                exchange: "SMART".into(),
                currency: (*currency).into(),
                ..Default::default()
            },
        )?;
    }

    // A listing SMART does not route for this account is not a listing that
    // does not exist. Ask the venue again by name before believing the refusal.
    let first_deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < first_deadline {
        client.process_msgs(&mut wrapper);
        if state.lock().unwrap().ended.len() + state.lock().unwrap().errors.len() == VENUES.len() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let unresolved: Vec<usize> = {
        let s = state.lock().unwrap();
        (0..VENUES.len())
            .filter(|i| !s.rows.iter().any(|(r, _)| *r == *i as i64 + 1))
            .collect()
    };
    for i in &unresolved {
        let (symbol, currency, venue) = VENUES[*i];
        println!("{symbol}: SMART found nothing, asking {venue} by name");
        client.req_contract_details(
            *i as i64 + 101,
            &Contract {
                symbol: symbol.into(),
                sec_type: "STK".into(),
                exchange: venue.into(),
                currency: currency.into(),
                ..Default::default()
            },
        )?;
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        client.process_msgs(&mut wrapper);
        if state.lock().unwrap().ended.len() == VENUES.len() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let s = state.lock().unwrap();
    for (i, (symbol, currency, expected)) in VENUES.iter().enumerate() {
        let req_id = i as i64 + 1;
        let rows: Vec<_> = s.rows.iter()
            .filter(|(r, _)| *r == req_id || *r == req_id + 100)
            .collect();
        println!("{symbol} ({currency}), expected on {expected}: {} listing(s)", rows.len());

        for (_, d) in rows.iter().take(3) {
            println!(
                "    con_id={:<10} currency={:<4} primary={:<8} class={:<8} min_tick={} tz={}",
                d.contract.con_id,
                d.contract.currency,
                d.contract.primary_exchange,
                d.contract.trading_class,
                d.min_tick,
                d.time_zone_id.as_deref().unwrap_or("-"),
            );
        }
        if rows.len() > 3 {
            println!("    … and {} more listings", rows.len() - 3);
        }
        for (r, code, msg) in s.errors.iter()
            .filter(|(r, _, _)| (*r == req_id || *r == req_id + 100) && rows.is_empty())
        {
            println!("    refused ({code}): {msg}  [req {r}]");
        }
    }

    drop(s);
    client.disconnect();
    Ok(())
}
