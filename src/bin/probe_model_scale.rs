//! Whether this library's option model reproduces the venue's own price, on
//! the scale the venue states its figures on.
//!
//! The venue states its volatility and its rate over a day, beside a count of
//! days. Read as a year's, both are short by the root of a year, and a strike
//! a little way out of the money prices at nothing — which is what left one
//! unsolvable. This asks a spread of strikes what the venue states, prices
//! each one, and solves each back, so a run says plainly whether the model and
//! the venue still agree.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin probe_model_scale

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    errors: Vec<String>,
}
impl Wrapper for Heard {
    fn error(&mut self, _req: i64, code: i64, message: &str, _adv: &str) {
        if !matches!(code, 2104 | 2106 | 2107 | 2119 | 2158) {
            self.errors.push(format!("{code}: {message}"));
        }
    }
}

/// The standard normal density.
fn phi(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// The standard normal distribution, by the series the venue's own library
/// uses (Abramowitz and Stegun 26.2.17).
fn cdf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.2316419 * x);
    let poly = t
        * (0.319381530
            + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
    let upper = phi(x) * poly;
    if sign > 0.0 { 1.0 - upper } else { upper }
}

/// The same call priced on a volatility read as a fraction of the underlying,
/// which is the reading a lognormal model takes.
fn call_on_fractional_vol(forward: f64, strike: f64, sigma: f64, years: f64, discount: f64) -> f64 {
    let spread = sigma * years.sqrt();
    if spread <= 0.0 {
        return discount * (forward - strike).max(0.0);
    }
    let d1 = ((forward / strike).ln() + 0.5 * spread * spread) / spread;
    let d2 = d1 - spread;
    discount * (forward * cdf(d1) - strike * cdf(d2))
}

fn main() {
    let _ = env_logger::try_init();
    let client = EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true,
        ..Default::default()
    })
    .expect("session");
    println!("session open\n");
    let heard = Heard::default();

    let chain_for = Contract {
        symbol: "MES".into(), sec_type: "FOP".into(), exchange: "CME".into(),
        currency: "USD".into(), ..Default::default()
    };
    let found = match client.contract_details(&chain_for) {
        Ok(f) => f,
        Err(e) => { println!("the venue named no chain: {e}"); return }
    };
    let mut rows: Vec<_> = found.iter()
        .map(|d| (
            d.contract.last_trade_date_or_contract_month.clone(),
            d.contract.strike,
            d.contract.right.clone(),
            d.contract.con_id,
        ))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    // The chain still names expiries that have been and gone; one of those
    // states no model however open its venue is.
    let today = std::env::var("IBX_TODAY").unwrap_or_else(|_| "20260824".into());
    rows.retain(|r| r.0.as_str() >= today.as_str());
    // Which expiry to ask about, counting from the nearest. The nearest is a
    // contract with hours left, where a count in days and a count in years are
    // told apart only by the calendar; one a month out separates them by two
    // orders of magnitude.
    let nth: usize = std::env::var("IBX_EXPIRY_NTH").ok()
        .and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut expiries: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    expiries.dedup();
    let Some(expiry) = expiries.get(nth).cloned() else {
        println!("no {nth}th expiry ahead of {today}; {} exist", expiries.len());
        return;
    };
    println!("expiries ahead: {:?}", &expiries[..expiries.len().min(6)]);
    let calls: Vec<_> = rows.iter().filter(|r| r.0 == expiry && r.2 == "C").collect();
    println!("nearest expiry {expiry}: {} calls\n", calls.len());

    println!("  {:>7}  {:>4}  {:>10}  {:>9}  {:>8}  {:>7}  {:>8}  {:>9}  {:>9}  {:>9}  {:>8}",
        "strike", "bit", "underlying", "venue", "sigma", "days", "rate/yr",
        "as annual", "venue math", "ibx model", "solved v");
    println!("  {}", "-".repeat(112));

    let step = (calls.len() / 9).max(1);
    let mut req = 900i64;
    let mut agree_price = 0u32;
    let mut agree_fraction = 0u32;
    let mut seen = 0u32;
    for row in calls.iter().step_by(step).take(9) {
        req += 1;
        let c = Contract {
            con_id: row.3, symbol: "MES".into(), sec_type: "FOP".into(),
            exchange: "CME".into(), currency: "USD".into(), ..Default::default()
        };
        if client.req_mkt_data(req, &c, "", false, false).is_err() { continue }
        let mut stated = None;
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            // Drained rather than taken off the callback: the day count and
            // the volatility's own kind are the point of this run, and the
            // reference client's callback carries neither.
            for m in client.shared_state().market.drain_option_computations() {
                if m.implied_vol > 0.0 && m.implied_vol != f64::MAX && m.opt_price != f64::MAX {
                    stated = Some(m);
                }
            }
            if stated.is_some() { break }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = client.cancel_mkt_data(req);
        let Some(m) = stated else {
            println!("  {:>7.0}  {:>4}  {:>9}", row.1, "-", "silent");
            continue;
        };
        if m.cal_days == f64::MAX || m.und_price == f64::MAX {
            println!("  {:>7.0}  {:>4}  {:>9.4}  (no days or no underlying stated)",
                row.1, m.price_based_vol as u8, m.opt_price);
            continue;
        }
        let years = m.cal_days / 365.0;
        let rate = if m.rate == f64::MAX { 0.0 } else { m.rate };
        // Both readings of the rate, told apart by which one prices the deep
        // strikes: their whole value is the discount, so they show it plainly.
        let per_year = (-rate * years).exp();
        let per_day = (-rate / 365.0 * m.cal_days).exp();
        // The volatility read as an annual figure, which is what a year in
        // the time makes it, against the same figure read as a day's.
        // What the wire carries, before it is carried over to a year: the
        // reading this had before, kept for the contrast.
        let a_day = m.implied_vol / 365.0_f64.sqrt();
        let as_annual = call_on_fractional_vol(m.und_price, row.1, a_day, years, per_year);
        let as_daily_both =
            call_on_fractional_vol(m.und_price, row.1, a_day, m.cal_days, per_day);
        // The same contract through this library's own model, which carries
        // the venue's figures into the units it works in itself.
        let ours = ibx::control::option_model::option_price(
            ibx::control::option_model::OptionTerms {
                strike: row.1,
                years_to_expiry: years,
                is_call: true,
                on_a_future: true,
            },
            ibx::control::option_model::VenueModel {
                volatility: m.implied_vol,
                option_price: m.opt_price,
                underlying_price: m.und_price,
                present_value_of_dividends: 0.0,
                rate,
            },
            m.implied_vol,
            m.und_price,
        ).unwrap_or(f64::NAN);
        // And back the other way: the venue's own price, solved for the
        // volatility that produces it. It has to come back as the volatility
        // the venue stated, on the scale the venue stated it.
        let solved = ibx::control::option_model::implied_volatility(
            ibx::control::option_model::OptionTerms {
                strike: row.1,
                years_to_expiry: years,
                is_call: true,
                on_a_future: true,
            },
            ibx::control::option_model::VenueModel {
                volatility: m.implied_vol,
                option_price: m.opt_price,
                underlying_price: m.und_price,
                present_value_of_dividends: 0.0,
                rate,
            },
            m.opt_price,
            m.und_price,
        ).unwrap_or(f64::NAN);
        seen += 1;
        if (ours - m.opt_price).abs() < 0.01 { agree_price += 1 }
        if (as_annual - m.opt_price).abs() < 0.01 { agree_fraction += 1 }
        println!("  {:>7.0}  {:>4}  {:>10.2}  {:>9.4}  {:>8.5}  {:>7.4}  {:>8.5}  {:>9.4}  {:>9.4}  {:>9.4}  {:>8.4}",
            row.1, m.price_based_vol as u8, m.und_price, m.opt_price, m.implied_vol,
            m.cal_days, rate, as_annual, as_daily_both, ours, solved);
    }
    println!("\n  of {seen} strikes the venue priced: {agree_price} reproduced by this \
              library's model on a day's volatility, {agree_fraction} on a year's");
    for e in heard.errors.iter().take(4) { println!("  said: {e}") }
}
