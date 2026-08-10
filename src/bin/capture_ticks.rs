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
fn subjects() -> Vec<(&'static str, &'static str, Contract)> {
    vec![
        // A currency pair has quotes and no trades, so asking for trades is
        // asking for something that does not exist — and the venue says so.
        ("a currency pair", "BidAsk", Contract {
            symbol: "EUR".to_string(),
            sec_type: "CASH".to_string(),
            exchange: "IDEALPRO".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("a crypto, quotes", "BidAsk", Contract {
            symbol: "BTC".to_string(),
            sec_type: "CRYPTO".to_string(),
            exchange: "PAXOS".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
        ("a crypto, trades", "AllLast", Contract {
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

    for (what, kind, contract) in subjects() {
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

        if let Err(e) = client.req_tick_by_tick_data(1, &resolved, kind, 0, false) {
            println!("  {what:<20} the subscription was refused: {e}");
            continue;
        }

        // Wait for frames. A quiet market and a feed that will never speak look
        // the same from here, so the wait is bounded and reported either way.
        let deadline = Instant::now() + Duration::from_secs(25);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }

        let frames: Vec<String> = client
            .unread_wire()
            .into_iter()
            .filter(|(kind, _)| *kind == "hmds-msg")
            .map(|(_, hex)| hex)
            .collect();

        if frames.is_empty() {
            println!("  {what:<20} nothing arrived in twenty seconds");
        } else {
            println!("  {what:<20} {} frame(s):", frames.len());
            // Group by message type, so the shape of the traffic is visible
            // rather than a wall of bytes.
            let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
            for hex in &frames {
                let bytes: Vec<u8> = (0..hex.len() / 2)
                    .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
                    .collect();
                let text = String::from_utf8_lossy(&bytes);
                let kind = text
                    .split('\u{1}')
                    .find_map(|f| f.strip_prefix("35=").map(|v| v.to_string()))
                    .unwrap_or_else(|| "?".to_string());
                *kinds.entry(kind).or_default() += 1;
            }
            for (kind, n) in &kinds {
                println!("        35={kind}: {n}");
            }
            // A few whole frames, so the layout can be checked against the
            // decoder rather than against a guess.
            for hex in frames.iter().filter(|h| {
                let bytes: Vec<u8> = (0..h.len() / 2)
                    .map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap_or(0))
                    .collect();
                String::from_utf8_lossy(&bytes).contains("35=E")
            }).take(4) {
                println!("        E {hex}");
            }
            // The venue states refusals in plain words. Show them.
            for hex in &frames {
                let bytes: Vec<u8> = (0..hex.len() / 2)
                    .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
                    .collect();
                let text = String::from_utf8_lossy(&bytes);
                if let Some(at) = text.find("<error>") {
                    let rest = &text[at + 7..];
                    if let Some(end) = rest.find("</error>") {
                        println!("        the venue says: {}", &rest[..end]);
                    }
                }
            }
        }
    }

    client.disconnect();
}
