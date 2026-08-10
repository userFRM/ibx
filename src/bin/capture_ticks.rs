//! Subscribe to every trade and quote change on a contract, and keep the frames
//! exactly as the venue sends them.
//!
//! The layout this client now decodes was read from the counterpart, not from a
//! capture. A decoder checked only against frames it made up proves nothing
//! about the ones that arrive, so this asks for real ones.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… IBX_CAPTURE_TBT=1 cargo run --bin capture_ticks

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;

/// Contracts that trade outside the American session, so this can be run
/// before the New York open. Both are entitled on this account.
fn subjects() -> Vec<(&'static str, Contract)> {
    vec![
        ("a currency pair", Contract {
            symbol: "EUR".to_string(),
            sec_type: "CASH".to_string(),
            exchange: "IDEALPRO".to_string(),
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
    ]
}

fn main() {
    let _ = env_logger::try_init();
    // Safety: set before anything reads it, and this binary is single-threaded
    // until the engine starts.
    unsafe { std::env::set_var("IBX_CAPTURE_TBT", "1") };

    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    let password = std::env::var("IB_PASSWORD").unwrap_or_default();
    if username.trim().is_empty() || password.trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
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
        // Resolve it first: a subscription wants the contract's own id, and the
        // id is also what says the venue knows the contract at all.
        let resolved = match client.qualify_contract(&contract) {
            Ok(c) => c,
            Err(e) => {
                println!("  {what:<20} could not be resolved: {e}");
                continue;
            }
        };
        println!("  {what:<20} conId={}", resolved.con_id);

        if let Err(e) = client.req_tick_by_tick_data(1, &resolved, "AllLast", 0, false) {
            println!("  {what:<20} the subscription was refused: {e}");
            continue;
        }

        // Wait for frames. A quiet market and a feed that will never speak look
        // the same from here, so the wait is bounded and reported either way.
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }

        let frames: Vec<String> = client
            .unread_wire()
            .into_iter()
            .filter(|(kind, _)| *kind == "tbt-frame")
            .map(|(_, hex)| hex)
            .collect();

        if frames.is_empty() {
            println!("  {what:<20} nothing arrived in twenty seconds");
        } else {
            println!("  {what:<20} {} frame(s):", frames.len());
            for hex in frames.iter().take(6) {
                println!("        {hex}");
            }
        }
    }

    client.disconnect();
}
