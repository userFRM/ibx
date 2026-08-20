//! Subscribe to prices on contracts a venue states a trading status for, and
//! keep every message the market-data connection carries.
//!
//! The status decoder is otherwise exercised only against synthetic bodies,
//! which tests the decoding and not the arrival. This captures real ones.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin capture_status

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;

/// Contracts whose venue states a status. A halt is a US-equity notion above
/// all, so these are American listings; a status arrives whether or not one is
/// halted, which is the point — the absence of a halt is itself a statement.
fn subjects() -> Vec<(&'static str, Contract)> {
    let stock = |symbol: &str, exchange: &'static str| Contract {
        symbol: symbol.to_string(),
        sec_type: "STK".to_string(),
        exchange: exchange.to_string(),
        currency: "USD".to_string(),
        ..Default::default()
    };
    vec![
        ("a large listing", stock("AAPL", "SMART")),
        ("one on its own venue", stock("SPY", "ARCA")),
        // Small and volatile, where a venue is likeliest to have stopped
        // trading at some point in the session.
        ("a volatile small cap", stock("SIRI", "SMART")),
        // An option, which is the one contract the venue models a volatility
        // for. Its model arrives on the same envelope as the trading status,
        // so the two together say whether that envelope's leading number is a
        // kind or a length.
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
        // A contract whose size increment is not one. The acknowledgement
        // carries an increment for sizes beside the one for prices, and on
        // every American listing both read as ordinary numbers, so nothing
        // there tells the two apart.
        ("a crypto", Contract {
            symbol: "BTC".to_string(),
            sec_type: "CRYPTO".to_string(),
            exchange: "PAXOS".to_string(),
            currency: "USD".to_string(),
            ..Default::default()
        }),
    ]
}

/// What every reader divides a quantity by.
fn qty_scale() -> f64 {
    ibx::types::QTY_SCALE as f64
}

fn main() {
    let _ = env_logger::try_init();

    // Paper credentials, which nothing else is using, so this runs at any hour.
    // Market data is the same feed on both accounts.
    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    let password = std::env::var("IB_PASSWORD").unwrap_or_default();
    if username.trim().is_empty() || password.trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
        std::process::exit(2);
    }
    // Safety: set before anything reads it, and this binary is single-threaded
    // until the engine starts.
    unsafe { std::env::set_var("IBX_CAPTURE_WIRE", "1") };

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

    for (n, (what, contract)) in subjects().into_iter().enumerate() {
        let req = n as i64 + 1;
        let resolved = match client.qualify_contract(&contract) {
            Ok(c) => c,
            Err(e) => {
                println!("  {what:<24} could not be resolved: {e}");
                continue;
            }
        };
        println!(
            "  {what:<24} conId={} routed={} listed={}",
            resolved.con_id, resolved.exchange, resolved.primary_exchange,
        );
        // 292 is the news tick. Asked for here because what is being checked
        // is which connection the venue answers a generic tick on, and news is
        // the one this client asks for over the trading connection.
        if let Err(e) = client.req_mkt_data(req, &resolved, "292", false, false) {
            println!("  {what:<24} the subscription was refused: {e}");
            continue;
        }
    }

    // Long enough to span whatever is being watched for. A status changes
    // when the venue's day does, so a run that wants to see one change has to
    // still be listening when it does.
    let seconds: u64 = std::env::var("IBX_CAPTURE_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
    }

    // What the connection carried, by message type, so the shape of the traffic
    // is visible rather than a wall of bytes.
    let all = client.unread_wire();
    for connection in ["farm-msg", "trading-msg", "hmds-msg"] {
        let count = all.iter().filter(|(kind, _)| *kind == connection).count();
        let generic = all
            .iter()
            .filter(|(kind, hex)| *kind == connection && hex.contains("33353d4701"))
            .count();
        println!("  {connection:<14} {count} message(s), {generic} generic tick(s)");
    }
    let frames: Vec<String> = all
        .iter()
        .filter(|(kind, _)| *kind == "farm-msg")
        .map(|(_, hex)| hex.clone())
        .collect();
    let bytes_of = |hex: &str| -> Vec<u8> {
        (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
            .collect()
    };

    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    for hex in &frames {
        let text = String::from_utf8_lossy(&bytes_of(hex)).to_string();
        let kind = text
            .split('\u{1}')
            .find_map(|f| f.strip_prefix("35=").map(|v| v.to_string()))
            .unwrap_or_else(|| "?".to_string());
        *kinds.entry(kind).or_default() += 1;
    }
    println!("\n{} frame(s) on the market data connection:", frames.len());
    for (kind, count) in &kinds {
        println!("        35={kind}: {count}");
    }

    // Anything that is not a price tick, whole. A status is twelve bytes, and
    // twelve bytes are lost in a summary.
    println!("\nnot price ticks:");
    for hex in frames.iter().take(4000) {
        let bytes = bytes_of(hex);
        let text = String::from_utf8_lossy(&bytes);
        if text.starts_with("35=P\u{1}") || text.contains("\u{1}35=P\u{1}") {
            continue;
        }
        let shown: String = text
            .chars()
            .map(|c| if c == '\u{1}' { '|' } else if c.is_control() { '.' } else { c })
            .collect();
        println!("        {} bytes  {shown}", bytes.len());
        println!("        hex {hex}");
    }

    // What reached a caller.
    for (what, contract) in subjects() {
        let con_id = client.qualify_contract(&contract).map(|c| c.con_id).unwrap_or(0);
        if let Some(instrument) = client.instrument_of(con_id) {
            let quote = client.shared_state().market.quote(instrument);
            println!(
                "  {what:<24} bid {:.4} x {:.8}   ask {:.4} x {:.8}  halted={:?}",
                quote.bid as f64 / 1e8,
                quote.bid_size as f64 / qty_scale(),
                quote.ask as f64 / 1e8,
                quote.ask_size as f64 / qty_scale(),
                quote.halted
            );
        }
    }

    client.disconnect();
}
