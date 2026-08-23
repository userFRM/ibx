//! What the venue sends as it states the account's holdings at session open,
//! and whether it says when it has finished.
//!
//! `req_positions` waits for what it calls a batch-end signal. This is what
//! actually arrives, in order, so that signal can be named rather than assumed.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin capture_position_burst

use std::time::Duration;

use ibx::api::client::{EClient, EClientConfig};

fn main() {
    let _ = env_logger::try_init();
    unsafe { std::env::set_var("IBX_CAPTURE_WIRE", "1") };
    let client = EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true, ..Default::default()
    }).expect("session");
    println!("session open");
    std::thread::sleep(Duration::from_secs(12));

    let mut order: Vec<String> = Vec::new();
    let mut tally: std::collections::BTreeMap<String, usize> = Default::default();
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut n = 0usize;
    for (conn, hex) in client.unread_wire() {
        if conn != "trading-msg" { continue; }
        let Ok(bytes) = (0..hex.len()).step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>() else { continue };
        let body = String::from_utf8_lossy(&bytes);
        let field = |tag: &str| body.split('\u{1}')
            .find_map(|f| f.strip_prefix(tag).map(|v| v.to_string()))
            .unwrap_or_default();
        let kind = field("35=");
        // The account and holdings traffic, and anything that looks like an end.
        if !matches!(kind.as_str(), "U" | "EB" | "UT" | "UM" | "RL") { continue; }
        n += 1;
        let sub = format!("35={kind} 6040={}", field("6040="));
        *tally.entry(sub.clone()).or_default() += 1;
        // The run of one subtype collapses; every change of subtype shows,
        // which is what a terminator would look like.
        if order.last().map(String::as_str) != Some(sub.as_str()) {
            order.push(sub.clone());
            print!("  {n:4}  {sub:16}");
            // Whole the first time, so a subtype nothing reads can still be
            // named by what it carries rather than by its number alone.
            if seen.insert(sub.clone()) {
                for f in body.split('\u{1}').filter(|f| !f.is_empty()) { print!("  {f}"); }
            }
            println!();
        }
    }
    println!("\n  {n} account messages, in order of arrival above (runs collapsed)");
    println!("  counts:");
    for (sub, c) in &tally {
        println!("    {sub:18} x{c}");
    }
}
