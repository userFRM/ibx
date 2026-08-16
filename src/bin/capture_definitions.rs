//! Ask the venue about a few contracts and report what it told us that nothing
//! reads.
//!
//! Seven fields the reference client publishes are not carried here, and the
//! tags that hold them could not be settled from anything offline. They arrive
//! on a real reply or they do not exist, so this asks for real replies and
//! names every tag on them that the parser ignores.
//!
//! Reads only. It places nothing and cancels nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin capture_definitions

#[path = "support/window.rs"]
mod window;
use window::live_window_is_open;

use std::time::Duration;

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;

/// One contract of each kind whose fields differ: a share, a fund, a bond, an
/// option and a future. A definition only carries the fields its kind has, so
/// asking about one kind answers for one kind.
fn subjects() -> Vec<(&'static str, Contract)> {
    let stk = |symbol: &str| Contract {
        symbol: symbol.to_string(),
        sec_type: "STK".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        ..Default::default()
    };
    vec![
        ("a share", stk("SPY")),
        // A share that is not traded in fractions, so its smallest size and
        // its size step should differ from one that is.
        ("a share priced too high to fraction", stk("BRK A")),
        ("a share on a venue outside the United States", Contract {
            symbol: "VOD".to_string(),
            sec_type: "STK".to_string(),
            exchange: "LSE".to_string(),
            currency: "GBP".to_string(),
            ..Default::default()
        }),
        ("an index", Contract {
            symbol: "SPX".to_string(),
            sec_type: "IND".to_string(),
            exchange: "CBOE".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        // A bond states its maturity, issue date, coupon and ratings. Which
        // description resolves one is itself unknown, so several are tried and
        // whichever answers is the one that teaches something.
        ("a bond by issuer", Contract {
            symbol: "IBM".to_string(),
            sec_type: "BOND".to_string(),
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("a treasury by issuer", Contract {
            symbol: "T-BILL".to_string(),
            sec_type: "BOND".to_string(),
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("a bond by its identifier", Contract {
            sec_id_type: "ISIN".to_string(),
            sec_id: "US912797ND90".to_string(),
            sec_type: "BOND".to_string(),
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("a fund", Contract {
            symbol: "VWELX".to_string(),
            sec_type: "FUND".to_string(),
            exchange: "FUNDSERV".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("an option", Contract {
            symbol: "SPY".to_string(),
            sec_type: "OPT".to_string(),
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            last_trade_date_or_contract_month: "20260918".to_string(),
            strike: 600.0,
            right: "C".to_string(),
            ..Default::default()
        }),
        ("a currency pair", Contract {
            symbol: "EUR".to_string(),
            sec_type: "CASH".to_string(),
            exchange: "IDEALPRO".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("a contract for difference", Contract {
            symbol: "AAPL".to_string(),
            sec_type: "CFD".to_string(),
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("a warrant", Contract {
            symbol: "ALV".to_string(),
            sec_type: "WAR".to_string(),
            exchange: "FWB".to_string(),
            currency: "EUR".to_string(),
            ..Default::default()
        }),
        ("a commodity", Contract {
            symbol: "XAUUSD".to_string(),
            sec_type: "CMDTY".to_string(),
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("a crypto", Contract {
            symbol: "BTC".to_string(),
            sec_type: "CRYPTO".to_string(),
            exchange: "PAXOS".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("an option on a future", Contract {
            symbol: "ES".to_string(),
            sec_type: "FOP".to_string(),
            exchange: "CME".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("a future", Contract {
            symbol: "ES".to_string(),
            sec_type: "FUT".to_string(),
            exchange: "CME".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
    ]
}

fn main() {
    let _ = env_logger::try_init();

    // This binary logs in with the paper credentials, which nothing else is
    // using. Only a run against the live account has to wait for a window.
    let against_live = std::env::var("IB_PAPER").as_deref() == Ok("0");
    if against_live && !live_window_is_open() {
        eprintln!(
            "the live account is in use by the daemon that trades it — a live run \
             waits for the premarket window or for after the close. The paper \
             account is reachable now."
        );
        std::process::exit(3);
    }

    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    let password = std::env::var("IB_PASSWORD").unwrap_or_default();
    if username.trim().is_empty() || password.trim().is_empty() {
        eprintln!(
            "IB_USERNAME/IB_PASSWORD unset. This reads from real servers and \
             does nothing without them."
        );
        std::process::exit(2);
    }

    let config = EClientConfig {
        username,
        password,
        host: std::env::var("IB_HOST").unwrap_or_else(|_| "cdc1.ibllc.com".to_string()),
        paper: true,
        core_id: None,
        code_provider: None,
        ..Default::default()
    };

    let client = match EClient::connect(&config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not open a session: {e}");
            std::process::exit(1);
        }
    };
    println!("session open");

    for (what, contract) in subjects() {
        match client.contract_details(&contract) {
            Ok(found) => {
                // The richest reply, not the first. A lookup answering with
                // many contracts often answers with a sparse header first, and
                // reading only that learns nothing about the kind.
                let richest = found
                    .iter()
                    .max_by_key(|d| d.unnamed_fields.len());
                let kept = richest.map(|d| d.unnamed_fields.len()).unwrap_or(0);
                // How the stated fields are spread across the replies. One
                // reply holding far more than the rest means records are being
                // run together, and a field from one contract would then be
                // read as a field of another.
                if found.len() > 1 {
                    for d in found.iter().take(3) {
                        println!(
                            "        row: conId={} symbol={:?} secType={:?} fields={}",
                            d.contract.con_id,
                            d.contract.symbol,
                            d.contract.sec_type,
                            d.unnamed_fields.len(),
                        );
                    }
                    let mut sizes: Vec<usize> =
                        found.iter().map(|d| d.unnamed_fields.len()).collect();
                    sizes.sort_unstable();
                    println!(
                        "        spread: n={} smallest={} median={} largest={} distinct ids={}",
                        found.len(),
                        sizes.first().copied().unwrap_or(0),
                        sizes[sizes.len() / 2],
                        sizes.last().copied().unwrap_or(0),
                        found.iter().map(|d| d.contract.con_id).collect::<std::collections::HashSet<_>>().len(),
                    );
                }
                println!(
                    "  {what:<44} {:>2} definition(s), {kept:>2} field(s) kept unnamed",
                    found.len()
                );
                // The values are what pair a tag with the field it holds: a
                // date reads as a date, a contract id as a contract id. The
                // field list is known; the numbers are not, and a value is what
                // joins the two.
                if let Some(d) = richest {
                    println!(
                        "        named: minSize={} minTick={} pricePrec={} sizePrec={} settle={:?}",
                        d.min_size,
                        d.min_tick,
                        d.last_price_precision,
                        d.last_size_precision,
                        d.settlement_method,
                    );
                    for (tag, value) in &d.unnamed_fields {
                        let shown: String = value.chars().take(48).collect();
                        println!("        {tag:>6} = {shown}");
                    }
                }
            }
            // A contract this account cannot see is not a failure of the
            // capture: the tags come from whichever replies do arrive.
            Err(e) => println!("  {what:<48} {e}"),
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    // Give the last reply a moment to be parsed before reading what it left.
    std::thread::sleep(Duration::from_secs(1));

    let unread = client.unread_wire();
    let mut tags: Vec<u32> = unread
        .iter()
        .filter(|(kind, _)| *kind == "definition")
        .flat_map(|(_, list)| list.split(','))
        .filter_map(|t| t.trim().parse::<u32>().ok())
        .collect();
    tags.sort_unstable();
    tags.dedup();

    println!();
    if tags.is_empty() {
        println!("every tag on every definition received was read");
    } else {
        println!("tags received on a definition that nothing reads ({}):", tags.len());
        for chunk in tags.chunks(16) {
            println!(
                "  {}",
                chunk.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(" ")
            );
        }
    }

    client.disconnect();
}
