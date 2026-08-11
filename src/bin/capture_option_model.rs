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
    // Keep every message the historical connection carries, so a reply this
    // client does not yet read still shows itself.
    // Safety: set before the engine starts, single-threaded here.
    unsafe { std::env::set_var("IBX_CAPTURE_WIRE", "1") };
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
    if let Some(rate) = ibx::control::option_model::carry_that_matches_the_venue(terms, model) {
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

    // The one number an option model needs that no tick states. The
    // counterpart's own option tools ask the venue for it as a series, so ask
    // for it the same way and see whether it answers.
    #[derive(Default)]
    struct Heard { bars: Vec<(String, f64)>, said: Vec<String> }
    impl ibx::api::wrapper::Wrapper for Heard {
        fn historical_data(&mut self, _req: i64, bar: &ibx::api::types::BarData) {
            self.bars.push((bar.date.clone(), bar.close));
        }
        fn error(&mut self, _req: i64, code: i64, message: &str, _adv: &str) {
            self.said.push(format!("{code}: {message}"));
        }
    }
    let mut heard = Heard::default();
    // Asked as a tick series rather than as bars: the venue's refusal named
    // the query type, and a tick type is served under a tick query.
    println!("  asking for it as a tick series");
    // The venue wants two of start, end and length. Give it start and end.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamp = |secs: u64| {
        let days = secs / 86_400;
        let (mut y, mut m, mut d) = (1970i64, 1i64, days as i64 + 1);
        loop {
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            let feb = if leap { 29 } else { 28 };
            let len = [31, feb, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][(m - 1) as usize];
            if d <= len { break }
            d -= len; m += 1;
            if m > 12 { m = 1; y += 1 }
        }
        let rest = secs % 86_400;
        format!("{y:04}{m:02}{d:02} {:02}:{:02}:{:02}", rest / 3600, (rest % 3600) / 60, rest % 60)
    };
    let (from, to) = (stamp(now - 5 * 86_400), stamp(now));
    println!("    from {from} to {to}");
    // Asked of the option and of the underlying: a rate belongs to what the
    // option is written on at least as plausibly as to the option itself.
    let underlying = Contract {
        symbol: "SPY".to_string(),
        sec_type: "STK".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        ..Default::default()
    };
    let under = client.qualify_contract(&underlying).unwrap_or(underlying);
    for (what, c) in [("the option", &resolved), ("the underlying", &under)] {
        println!("    of {what}");
        let _ = client.req_historical_ticks(
            if what == "the option" { 90 } else { 91 },
            c, &from, &to, 10, "OptExInterestRate", false,
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        client.process_msgs(&mut heard);
        std::thread::sleep(Duration::from_millis(200));
    }
    for said in heard.said.drain(..).take(2) { println!("    the venue says: {said}"); }
    for (kind, hex) in client.unread_wire() {
        if kind != "hmds-msg" { continue }
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
            .collect();
        let text = String::from_utf8_lossy(&bytes);
        if text.contains("OptExInterestRate") || text.contains("ResultSet") {
            let shown: String = text.chars().map(|c| if c == '\u{1}' { '|' } else { c }).collect();
            println!("    reply: {}", &shown[..shown.len().min(600)]);
        }
    }

    let shapes: [(&str, &str); 0] = [];
    let mut asked = 2;
    for (duration, bar) in shapes {
        asked += 1;
        println!("  asking {duration} of {bar}");
        let _ = client.req_historical_data(
            asked, &resolved, "", duration, bar, "OPTION_EXERCISE_INTEREST_RATE", false, 1, false,
        );
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            std::thread::sleep(Duration::from_millis(200));
        }
        for (when, rate) in heard.bars.drain(..).take(3) { println!("    {when}  {rate}"); }
        for said in heard.said.drain(..).take(1) { println!("    the venue says: {said}"); }
    }
    match client.req_historical_data(
        2, &resolved, "", "5 D", "1 day", "OPTION_EXERCISE_INTEREST_RATE", false, 1, false,
    ) {
        Ok(()) => {
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline {
                client.process_msgs(&mut heard);
                std::thread::sleep(Duration::from_millis(200));
            }
            println!("\n  the venue's own rate series:");
            for (when, rate) in heard.bars.iter().take(5) {
                println!("    {when}  {rate}");
            }
            for said in heard.said.iter().take(3) {
                println!("    the venue says: {said}");
            }
            if heard.bars.is_empty() && heard.said.is_empty() {
                println!("    nothing arrived");
            }
        }
        Err(e) => println!("  the rate series could not be asked for: {e}"),
    }

    client.disconnect();
}
