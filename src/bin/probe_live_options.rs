//! Which option markets are quoting right now.
//!
//! The option model can only be checked against a contract the venue is
//! actively stating a model for, and the US options session is one of several.
//! This asks a spread of venues across the world what they are quoting, and
//! reports which of them answered — so the check runs when a market is open
//! rather than when one is assumed to be.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin probe_live_options

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    models: usize,
    errors: Vec<String>,
    answers: Vec<(i64, f64)>,
}

/// The venue's own statement about one contract, as this probe reads it back.
#[derive(Clone, Copy, Default)]
struct Stated {
    implied_vol: f64,
    cal_days: f64,
    und_price: f64,
    opt_price: f64,
}

impl Wrapper for Heard {
    fn tick_option_computation(
        &mut self, req: i64, tick: i32, _attrib: i32, implied_vol: f64, _delta: f64,
        _opt_price: f64, _pv_div: f64, _gamma: f64, _vega: f64, _theta: f64, _und: f64,
    ) {
        if implied_vol > 0.0 && implied_vol != f64::MAX {
            self.models += 1;
            // 53 is an answer to a question this client asked; 13 is the
            // venue's own model streaming.
            if tick == 53 {
                self.answers.push((req, implied_vol));
            }
        }
    }
    fn error(&mut self, _req: i64, code: i64, message: &str, _adv: &str) {
        if !matches!(code, 2104 | 2106 | 2107 | 2119 | 2158) {
            self.errors.push(format!("{code}: {message}"));
        }
    }
}

/// An underlying, and the venue its options trade on.
fn subjects() -> Vec<(&'static str, Contract)> {
    let fut = |symbol: &str, exch: &'static str, expiry: &str| Contract {
        symbol: symbol.into(), sec_type: "FUT".into(), exchange: exch.into(),
        last_trade_date_or_contract_month: expiry.into(), ..Default::default()
    };
    let idx = |symbol: &str, exch: &'static str, ccy: &'static str| Contract {
        symbol: symbol.into(), sec_type: "IND".into(), exchange: exch.into(),
        currency: ccy.into(), ..Default::default()
    };
    vec![
        // Globex reopens Sunday evening New York time and runs nearly around
        // the clock, so its options are the first thing open after a weekend.
        ("US index future (CME)", fut("MES", "CME", "202709")),
        ("US index future, full size", fut("ES", "CME", "202509")),
        // Asia is a business day ahead: these are mid-session while New York
        // is asleep.
        ("Japan index (OSE)", idx("N225", "OSE.JPN", "JPY")),
        ("Hong Kong index (HKFE)", idx("HSI", "HKFE", "HKD")),
        ("Australia index (ASX)", idx("SPI", "SNFE", "AUD")),
        ("Korea index (KSE)", idx("K200", "KSE", "KRW")),
        // The one that is definitely shut, as the control.
        ("US listing (closed now)", Contract {
            symbol: "SPY".into(), sec_type: "STK".into(),
            exchange: "SMART".into(), currency: "USD".into(), ..Default::default()
        }),
    ]
}

fn main() {
    let _ = env_logger::try_init();
    let client = match EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true, ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => { eprintln!("could not open a session: {e}"); std::process::exit(1); }
    };
    println!("session open\n");

    let mut heard = Heard::default();
    let mut req = 100i64;

    // The point of the exercise: an option whose model the venue is stating
    // right now. Futures options run on the clock the future does, so they
    // are the ones open while New York is shut.
    let chain_for = Contract {
        symbol: "MES".into(), sec_type: "FOP".into(), exchange: "CME".into(),
        currency: "USD".into(), ..Default::default()
    };
    match client.contract_details(&chain_for) {
        Ok(found) => {
            println!("  MES options named by the venue: {}", found.len());
            // The nearest expiry, and a spread of strikes across it.
            let mut rows: Vec<_> = found.iter()
                .map(|d| (
                    d.contract.last_trade_date_or_contract_month.clone(),
                    d.contract.strike,
                    d.contract.right.clone(),
                    d.contract.con_id,
                ))
                .collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            // The chain still names expiries that have been and gone, and a
            // contract that has expired states no model however open its
            // venue is. The nearest one still ahead is the subject.
            let today = std::env::var("IBX_TODAY").unwrap_or_else(|_| "20260824".into());
            rows.retain(|r| r.0.as_str() >= today.as_str());
            println!("  still ahead of {today}: {} contracts", rows.len());
            if let Some(first_expiry) = rows.first().map(|r| r.0.clone()) {
                let same: Vec<_> = rows.iter()
                    .filter(|r| r.0 == first_expiry && r.2 == "C")
                    .collect();
                println!("  nearest expiry {first_expiry}: {} calls", same.len());
                let step = (same.len() / 6).max(1);
                for row in same.iter().step_by(step).take(6) {
                    req += 1;
                    let c = Contract {
                        con_id: row.3, symbol: "MES".into(), sec_type: "FOP".into(),
                        exchange: "CME".into(), currency: "USD".into(), ..Default::default()
                    };
                    let before = heard.models;
                    if client.req_mkt_data(req, &c, "", false, false).is_err() { continue; }
                    let mut got = Stated::default();
                    let deadline = Instant::now() + Duration::from_secs(8);
                    // Drained here rather than through the message pump: the
                    // day count is the whole point of this run, and the
                    // callback the pump feeds carries the reference client's
                    // fields, which are not all of them.
                    while Instant::now() < deadline {
                        for m in client.shared_state().market.drain_option_computations() {
                            if m.implied_vol > 0.0 && m.implied_vol != f64::MAX {
                                got = Stated {
                                    implied_vol: m.implied_vol,
                                    cal_days: m.cal_days,
                                    und_price: m.und_price,
                                    opt_price: m.opt_price,
                                };
                            }
                        }
                        if got.implied_vol > 0.0 { break; }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    let days = if got.cal_days == f64::MAX {
                        "(not stated)".to_string()
                    } else {
                        format!("{:.6}", got.cal_days)
                    };
                    // The round trip: ask what volatility the venue's own
                    // price implies. It must come back with the volatility
                    // the venue stated, or the model does not reproduce the
                    // anchor it is built on.
                    let asked = Contract {
                        con_id: row.3, symbol: "MES".into(), sec_type: "FOP".into(),
                        exchange: "CME".into(), currency: "USD".into(),
                        strike: row.1, right: "C".into(),
                        last_trade_date_or_contract_month: first_expiry.clone(),
                        ..Default::default()
                    };
                    let model_px = got.opt_price;
                    let round_trip = if model_px == f64::MAX || model_px <= 0.0 {
                        "no price stated".to_string()
                    } else {
                        req += 1;
                        heard.errors.clear();
                        // Taken again here, not from the reading above: the
                        // model moves between ticks, and inverting a price
                        // from one moment against the model of another asks
                        // the wrong question and blames the answer.
                        let mut now = got;
                        for m in client.shared_state().market.drain_option_computations() {
                            if m.implied_vol > 0.0 && m.implied_vol != f64::MAX {
                                now = Stated {
                                    implied_vol: m.implied_vol, cal_days: m.cal_days,
                                    und_price: m.und_price, opt_price: m.opt_price,
                                };
                            }
                        }
                        client.calculate_implied_volatility(
                            req, &asked, now.opt_price, now.und_price,
                        );
                        let until = Instant::now() + Duration::from_secs(4);
                        let mut answer = None;
                        while Instant::now() < until && answer.is_none() {
                            client.process_msgs(&mut heard);
                            answer = heard.answers.iter()
                                .find(|(r, _)| *r == req).map(|(_, v)| *v);
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        match answer {
                            Some(v) => format!(
                                "vol={v:.6} ({:+.2e} from the venue's)", v - now.implied_vol,
                            ),
                            None => format!(
                                "REFUSED — {}",
                                heard.errors.first().map(String::as_str).unwrap_or("said nothing"),
                            ),
                        }
                    };
                    // The model on its own, given exactly what the venue
                    // stated and the time the venue stated, so a refusal is
                    // the model's and not the plumbing's.
                    let direct = {
                        use ibx::control::option_model::{implied_volatility, OptionTerms, VenueModel};
                        let terms = OptionTerms {
                            strike: row.1,
                            years_to_expiry: got.cal_days / 365.0,
                            is_call: true,
                        };
                        let model = VenueModel {
                            volatility: got.implied_vol,
                            option_price: got.opt_price,
                            underlying_price: got.und_price,
                            present_value_of_dividends: 0.0,
                        };
                        match implied_volatility(terms, model, got.opt_price, got.und_price) {
                            Some(v) => format!("{v:.6}"),
                            None => "no solution".into(),
                        }
                    };
                    println!(
                        "    strike {:>8}  vol={:<10.6} px={:<10.3} und={:<10.3} calDays={days}\n                    model direct: {direct}   through the client: {round_trip}",
                        row.1, got.implied_vol, got.opt_price, got.und_price,
                    );
                    let _ = client.cancel_mkt_data(req);
                    let _ = before;
                }
            }
        }
        Err(e) => println!("  MES options: {e}"),
    }
    println!();

    for (what, contract) in subjects() {
        req += 1;
        let resolved = match client.qualify_contract(&contract) {
            Ok(c) => c,
            Err(e) => { println!("  {what:26} — not named: {e}"); continue; }
        };
        if let Err(e) = client.req_mkt_data(req, &resolved, "", false, false) {
            println!("  {what:26} — refused: {e}");
            continue;
        }
        // Long enough for a quote to arrive on a market that is trading.
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            std::thread::sleep(Duration::from_millis(100));
        }
        let q = client.quote(req).unwrap_or_default();
        let scale = ibx::types::PRICE_SCALE as f64;
        let (bid, ask, last) = (q.bid as f64 / scale, q.ask as f64 / scale, q.last as f64 / scale);
        let quoting = bid > 0.0 || ask > 0.0 || last > 0.0;
        println!(
            "  {what:26} {} conId={} bid={bid} ask={ask} last={last}",
            if quoting { "QUOTING" } else { "silent " }, resolved.con_id,
        );
        let _ = client.cancel_mkt_data(req);
    }
    for e in heard.errors.iter().take(8) {
        println!("\n  said: {e}");
    }
}
