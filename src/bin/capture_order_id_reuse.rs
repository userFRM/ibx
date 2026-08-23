//! What the venue answers when an order id is used a second time.
//!
//! Several places in this client number orders from the clock so that a run
//! never reuses an id, each saying the venue refuses a reused one. None of
//! them rests on a captured refusal. This places an order, withdraws it, and
//! places another under the same id, so the answer is on the record.
//!
//! Two runs are needed for the whole question. The first prints the id it
//! used; giving that id back as `IBX_REUSE_ID` on a later run asks the same
//! question across sessions rather than within one, which is what a counter
//! kept between runs would exist for.
//!
//! It places resting orders far from the market and withdraws them.
//!
//!     IB_USERNAME=… IB_PASSWORD=… IBX_REUSE_ID=… cargo run --features dev-tools --bin capture_order_id_reuse

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::{Contract, Order, OrderState};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    lines: Vec<String>,
}

impl Wrapper for Heard {
    fn order_status(
        &mut self, order_id: i64, status: &str, filled: f64, remaining: f64,
        _avg_fill_price: f64, perm_id: i64, _parent_id: i64,
        _last_fill_price: f64, _client_id: i64, why_held: &str, _mkt_cap_price: f64,
    ) {
        self.lines.push(format!(
            "  status  order {order_id}: {status} filled={filled} remaining={remaining} permId={perm_id} whyHeld={why_held:?}"
        ));
    }
    fn open_order(&mut self, order_id: i64, _c: &Contract, _o: &Order, state: &OrderState) {
        self.lines.push(format!("  open    order {order_id}: {}", state.status));
    }
    fn error(&mut self, req_id: i64, code: i64, message: &str, _advanced: &str) {
        self.lines.push(format!("  error   {req_id}/{code}: {message}"));
    }
}

/// Everything heard while waiting, printed as it is drained.
fn drain(client: &EClient, heard: &mut Heard, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        client.process_msgs(heard);
        for line in heard.lines.drain(..) {
            println!("{line}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn main() {
    let _ = env_logger::try_init();
    // Kept so tag 37 can be read as the venue stated it.
    unsafe { std::env::set_var("IBX_CAPTURE_WIRE", "1") };
    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    let password = std::env::var("IB_PASSWORD").unwrap_or_default();
    if username.trim().is_empty() || password.trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
        std::process::exit(2);
    }
    let client = match EClient::connect(&EClientConfig {
        username, password, paper: true, ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => { eprintln!("could not open a session: {e}"); std::process::exit(1); }
    };
    println!("session open");

    let contract = Contract {
        symbol: "SPY".to_string(), sec_type: "STK".to_string(),
        exchange: "SMART".to_string(), currency: "USD".to_string(),
        ..Default::default()
    };
    let resolved = match client.qualify_contract(&contract) {
        Ok(c) => c,
        Err(e) => { println!("the contract could not be resolved: {e}"); return; }
    };

    // Far below the market and good only for the day, so it rests rather than
    // trades and is gone by tomorrow whatever happens here.
    let resting = Order {
        action: "BUY".to_string(), order_type: "LMT".to_string(),
        total_quantity: 1.0, lmt_price: 1.0, tif: "DAY".to_string(),
        ..Default::default()
    };

    let reused: Option<i64> = std::env::var("IBX_REUSE_ID").ok().and_then(|s| s.parse().ok());
    let id = reused.unwrap_or_else(|| client.next_order_id());
    let mut heard = Heard::default();

    if std::env::var("IBX_WITHDRAW_ONLY").is_ok() {
        println!("\n[withdrawing {id} and stating what is working]");
        let _ = client.cancel_order(id, "");
        drain(&client, &mut heard, 4);
        client.req_all_open_orders(&mut heard);
        drain(&client, &mut heard, 5);
        return;
    }

    match reused {
        Some(_) => println!("\n[across sessions] placing under id {id}, used by an earlier run"),
        None => println!("\n[first placement] id {id}"),
    }
    if let Err(e) = client.place_order(id, &resolved, &resting) {
        println!("  refused before sending: {e}");
        return;
    }
    drain(&client, &mut heard, 6);

    if std::env::var("IBX_LEAVE_WORKING").is_ok() {
        println!("\n[left working] run again with IBX_REUSE_ID={id} to place a new order \
                  under an id the account is still working, from a session that has no \
                  record of it");
        return;
    }

    println!("\n[while the first is still working] placing again under {id}");
    match client.place_order(id, &resolved, &resting) {
        Ok(()) => drain(&client, &mut heard, 8),
        Err(e) => println!("  refused before sending: {e}"),
    }

    println!("\n[withdrawing {id}]");
    if let Err(e) = client.cancel_order(id, "") {
        println!("  refused before sending: {e}");
    }
    drain(&client, &mut heard, 6);

    println!("\n[after it is withdrawn] placing again under {id}");
    match client.place_order(id, &resolved, &resting) {
        Ok(()) => drain(&client, &mut heard, 8),
        Err(e) => println!("  refused before sending: {e}"),
    }

    println!("\n[withdrawing {id} again]");
    let _ = client.cancel_order(id, "");
    drain(&client, &mut heard, 5);

    // What the venue actually states as the order's own id, byte for byte.
    // The shape decides whether a permId can be read off it or not.
    println!("\n[tag 37, as the venue stated it]");
    let mut seen: Vec<String> = Vec::new();
    for (conn, hex) in client.unread_wire() {
        if conn != "trading-msg" { continue; }
        let Ok(bytes) = (0..hex.len()).step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>() else { continue };
        let body = String::from_utf8_lossy(&bytes);
        let field = |tag: &str| body.split('\u{1}')
            .find_map(|f| f.strip_prefix(tag).map(|v| v.to_string()));
        if let Some(order_id) = field("37=") {
            let line = format!(
                "  35={} 37={order_id} 11={} 150={}",
                field("35=").unwrap_or_default(),
                field("11=").unwrap_or_default(),
                field("150=").unwrap_or_default(),
            );
            if !seen.contains(&line) {
                seen.push(line);
            }
        }
    }
    for line in seen.iter().take(4) {
        println!("{line}");
    }

    // Every tag one execution report carried, so a field holding an id the
    // venue issues is found rather than guessed at.
    println!("\n[one execution report, every field]");
    for (conn, hex) in client.unread_wire() {
        if conn != "trading-msg" { continue; }
        let Ok(bytes) = (0..hex.len()).step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>() else { continue };
        let body = String::from_utf8_lossy(&bytes);
        if !body.contains("35=8\u{1}") { continue; }
        for f in body.split('\u{1}').filter(|f| !f.is_empty()) {
            print!("  {f}");
        }
        println!();
        break;
    }
    if seen.is_empty() {
        println!("  nothing captured — IBX_CAPTURE_WIRE was not set");
    }

    println!("\nRun again with IBX_REUSE_ID={id} to ask the same across sessions.");
}
