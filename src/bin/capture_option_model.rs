//! This client's option arithmetic against the venue's own.
//!
//! The venue publishes a model per option — the volatility it used, the price
//! that came out, the rate it discounted at, the days it counted, and the
//! greeks it worked out. This asks for a run of strikes on one expiry and
//! prints, for each, what this client makes of the same inputs.
//!
//! Two models are compared, because the venue states its dividends two ways
//! and this client can read either: the carry recovered from the venue's own
//! published price, and the schedule of ex-dates the venue keeps for the
//! underlying. The price, the delta and the gamma were already close under the
//! first; what the schedule is for is vega and theta, which depend on *when*
//! the underlying drops rather than only on how much.
//!
//! Reads only. It places nothing.

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::control::dividends;
use ibx::control::option_model::{greeks, recover_yield, OptionTerms, VenueModel};

fn main() {
    let _ = ibx::logging::try_init_from_env("error");
    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    if username.trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
        std::process::exit(2);
    }
    let client = match EClient::connect(&EClientConfig {
        username,
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true,
        ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not open a session: {e}");
            std::process::exit(1);
        }
    };
    println!("session open");

    let symbol = std::env::var("IBX_SYMBOL").unwrap_or_else(|_| "SPY".to_string());
    let expiry = std::env::var("IBX_EXPIRY").unwrap_or_else(|_| "20261218".to_string());
    let strikes: Vec<f64> = std::env::var("IBX_STRIKES")
        .unwrap_or_else(|_| "640,660,680,700,720,740".to_string())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    let shared = client.shared_state();
    let mut req_id = 1i64;
    let mut rows = Vec::new();

    for &strike in &strikes {
        for right in ["C", "P"] {
            let contract = Contract {
                symbol: symbol.clone(),
                sec_type: "OPT".to_string(),
                exchange: "SMART".to_string(),
                currency: "USD".to_string(),
                last_trade_date_or_contract_month: expiry.clone(),
                strike,
                right: right.to_string(),
                ..Default::default()
            };
            let Ok(resolved) = client.qualify_contract(&contract) else {
                println!("{symbol} {expiry} {strike} {right}: the venue does not know it");
                continue;
            };
            if client.req_mkt_data(req_id, &resolved, "", false, false).is_err() {
                continue;
            }
            req_id += 1;
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut stated = None;
            while Instant::now() < deadline && stated.is_none() {
                std::thread::sleep(Duration::from_millis(200));
                if let Some(instrument) = client.instrument_of(resolved.con_id) {
                    stated = shared.market.option_model(instrument);
                }
            }
            match stated {
                Some(model) => rows.push((resolved, right == "C", model)),
                None => println!("{symbol} {expiry} {strike} {right}: no model in twenty seconds"),
            }
        }
    }

    if rows.is_empty() {
        println!("the venue stated no models to measure against");
        client.disconnect();
        return;
    }

    // The schedule belongs to the underlying, which the definitions above
    // named. Asked for once, when the first of them arrived.
    let under = shared.reference.under_con_id(rows[0].0.con_id as u32);
    let schedule = under.and_then(|under| shared.reference.dividend_schedule(under));
    match (&under, &schedule) {
        (Some(under), Some(schedule)) => println!(
            "written on {under}, which pays {} times on the venue's books",
            schedule.payments.len(),
        ),
        _ => println!("the venue stated no schedule for what these are written on"),
    }
    let today = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| (since.as_secs() / 86_400) as i64)
        .unwrap_or(0);

    println!();
    println!(
        "{:>6} {:>2} {:>9} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "strike", "", "what", "venue", "carry only", "off %", "both", "off %",
    );
    let stated_or_none = |v: f64| (v.is_finite() && v != f64::MAX).then_some(v);
    for (contract, is_call, model) in &rows {
        let Some(days) = stated_or_none(model.cal_days).filter(|d| *d > 0.0) else { continue };
        let years = days / 365.0;
        let terms = OptionTerms {
            strike: contract.strike,
            years_to_expiry: years,
            is_call: *is_call,
            on_a_future: false,
        };
        let bare = VenueModel {
            volatility: model.implied_vol,
            option_price: model.opt_price,
            underlying_price: model.und_price,
            present_value_of_dividends: stated_or_none(model.pv_dividend).unwrap_or(0.0),
            rate: stated_or_none(model.rate).unwrap_or(0.0),
            yield_rate: 0.0,
        };
        let with_yield = recover_yield(terms, bare, &[])
            .map(|yield_rate| VenueModel { yield_rate, ..bare })
            .and_then(|m| greeks(terms, m, &[], model.implied_vol, model.und_price));
        let over = schedule
            .as_ref()
            .map(|s| dividends::over_the_life(s, today, years, &contract.currency))
            .unwrap_or_default();
        // The schedule with a carry recovered on top of it, which is what
        // this client now solves with.
        let with_schedule = recover_yield(terms, bare, &over)
            .map(|yield_rate| VenueModel { yield_rate, ..bare })
            .and_then(|m| greeks(terms, m, &over, model.implied_vol, model.und_price));

        for (name, stated) in [
            ("price", model.opt_price),
            ("delta", model.delta),
            ("gamma", model.gamma),
            ("vega", model.vega),
            ("theta", model.theta),
        ] {
            let pick = |g: &Option<ibx::control::option_model::Greeks>| {
                g.map(|g| match name {
                    "price" => g.price,
                    "delta" => g.delta,
                    "gamma" => g.gamma,
                    "vega" => g.vega,
                    _ => g.theta,
                })
            };
            let off = |ours: Option<f64>| match (ours, stated_or_none(stated)) {
                (Some(ours), Some(stated)) if stated.abs() > 1e-12 => {
                    format!("{:+.2}", 100.0 * (ours - stated) / stated)
                }
                _ => "—".to_string(),
            };
            println!(
                "{:>6} {:>2} {name:>9} {:>10} {:>10} {:>10} {:>10} {:>10}",
                contract.strike,
                if *is_call { "C" } else { "P" },
                stated_or_none(stated).map(|v| format!("{v:.4}")).unwrap_or("—".into()),
                pick(&with_yield).map(|v| format!("{v:.4}")).unwrap_or("—".into()),
                off(pick(&with_yield)),
                pick(&with_schedule).map(|v| format!("{v:.4}")).unwrap_or("—".into()),
                off(pick(&with_schedule)),
            );
        }
    }
    client.disconnect();
}
