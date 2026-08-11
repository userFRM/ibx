//! Check this client's option arithmetic against the venue's own.
//!
//! The venue states what its model made of a contract: the volatility it used,
//! the price that came out, the underlying it used and the dividends it took
//! off. Given those, this client should be able to produce the venue's own
//! price from the venue's own volatility — and if it cannot, its answer to a
//! caller's question is worth nothing either.
//!
//! Reads only. It places nothing.

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::control::option_model::{implied_volatility, option_price, OptionTerms, VenueModel};

fn main() {
    let _ = env_logger::try_init();
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
        Err(e) => { eprintln!("could not open a session: {e}"); std::process::exit(1); }
    };
    println!("session open");

    let contract = Contract {
        symbol: "SPY".to_string(),
        sec_type: "OPT".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        last_trade_date_or_contract_month: "20260918".to_string(),
        strike: 775.0,
        right: "C".to_string(),
        ..Default::default()
    };
    let resolved = match client.qualify_contract(&contract) {
        Ok(c) => c,
        Err(e) => { println!("the contract could not be resolved: {e}"); return; }
    };
    println!("  conId={} strike={} expiry={}",
        resolved.con_id, resolved.strike, resolved.last_trade_date_or_contract_month);

    if let Err(e) = client.req_mkt_data(1, &resolved, "", false, false) {
        println!("  the model could not be asked for: {e}");
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stated = None;
    while Instant::now() < deadline && stated.is_none() {
        std::thread::sleep(Duration::from_millis(250));
        if let Some(instrument) = client.instrument_of(resolved.con_id) {
            stated = client.shared_state().market.option_model(instrument);
        }
    }
    let Some(stated) = stated else {
        println!("  the venue stated no model for it in thirty seconds");
        client.disconnect();
        return;
    };
    println!(
        "  the venue says: vol={:.6} price={:.4} underlying={:.4} dividends={:.4}",
        stated.implied_vol, stated.opt_price, stated.und_price, stated.pv_dividend,
    );

    let years = {
        let d = &resolved.last_trade_date_or_contract_month;
        let (y, m, day): (i64, i64, i64) = (
            d[0..4].parse().unwrap_or(0), d[4..6].parse().unwrap_or(0), d[6..8].parse().unwrap_or(0),
        );
        let days = (y - 2026) as f64 * 365.0 + (m - 8) as f64 * 30.4 + (day - 11) as f64;
        days / 365.0
    };
    let terms = OptionTerms { strike: resolved.strike, years_to_expiry: years, is_call: true };
    let model = VenueModel {
        volatility: stated.implied_vol,
        option_price: stated.opt_price,
        underlying_price: stated.und_price,
        present_value_of_dividends: if stated.pv_dividend.is_finite()
            && stated.pv_dividend != f64::MAX { stated.pv_dividend } else { 0.0 },
    };

    match option_price(terms, model, stated.implied_vol, stated.und_price) {
        Some(ours) => {
            let off = (ours - stated.opt_price).abs();
            println!("  this client says: price={ours:.4}  ({off:.4} from the venue's own)");
        }
        None => println!("  this client could not price it from the venue's own numbers"),
    }
    // What the solve has to work with, when it cannot work.
    if let Some(rate) = ibx::control::option_model::rate_the_venue_used(terms, model) {
        let step = terms.years_to_expiry / 256.0;
        let floor = (rate.abs() * step.sqrt() * 1.02).max(1e-4);
        println!("  rate={rate:.6} years={:.4} floor={floor:.6}", terms.years_to_expiry);
        for v in [floor, 0.01, 0.018692, 0.05, 1.0, 5.0] {
            match ibx::control::option_model::price(terms, stated.und_price, v, rate, 0.0) {
                Some(p) => println!("    vol={v:.6} -> {p:.4}"),
                None => println!("    vol={v:.6} -> the tree does not hold"),
            }
        }
    } else {
        println!("  no rate reproduces the venue's price");
    }
    match implied_volatility(terms, model, stated.opt_price, stated.und_price) {
        Some(ours) => {
            let off = (ours - stated.implied_vol).abs();
            println!("  this client says: vol={ours:.6}  ({off:.6} from the venue's own)");
        }
        None => println!("  this client could not solve it from the venue's own numbers"),
    }

    client.disconnect();
}
