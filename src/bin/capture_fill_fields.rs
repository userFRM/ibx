//! Every field a real execution report carries, captured from a fill.
//!
//! A fill's commission is read off tag 12, and the offline fixtures inject
//! their own, so nothing but a real fill says whether the venue states it
//! there. It does not: the reports this prints carry no tag 12, and the
//! charge arrives on a record of its own.
//!
//! Trades a small amount of something quoted around the clock, buying and
//! then selling, so the position it opens is closed again. Priced through
//! the spread rather than sent at market, which this instrument refuses.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin capture_fill_fields

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::{Contract, Order};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    fills: Vec<String>,
    lines: Vec<String>,
}

impl Wrapper for Heard {
    fn exec_details(&mut self, _req: i64, _c: &Contract, e: &ibx::api::types::Execution) {
        self.fills.push(format!(
            "  exec {}: shares={} price={} commission-bearing report follows",
            e.exec_id, e.shares, e.price,
        ));
    }
    fn commission_and_fees_report(&mut self, r: &ibx::api::types::CommissionAndFeesReport) {
        self.lines.push(format!(
            "  commissionAndFees: execId={} charged={} currency={:?} realizedPnl={}",
            r.exec_id, r.commission_and_fees, r.currency, r.realized_pnl,
        ));
    }
    fn order_status(
        &mut self, order_id: i64, status: &str, filled: f64, _remaining: f64,
        avg_fill_price: f64, _perm: i64, _parent: i64, _last: f64, _client: i64,
        _why: &str, _cap: f64,
    ) {
        self.lines.push(format!(
            "  status order {order_id}: {status} filled={filled} avg={avg_fill_price}"
        ));
    }
    fn error(&mut self, req_id: i64, code: i64, message: &str, _adv: &str) {
        self.lines.push(format!("  error {req_id}/{code}: {message}"));
    }
}

fn drain(client: &EClient, heard: &mut Heard, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        client.process_msgs(heard);
        for l in heard.lines.drain(..) { println!("{l}"); }
        for l in heard.fills.drain(..) { println!("{l}"); }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn main() {
    let _ = env_logger::try_init();
    unsafe { std::env::set_var("IBX_CAPTURE_WIRE", "1") };
    let client = EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true, ..Default::default()
    }).expect("session");
    println!("session open");

    let contract = Contract {
        symbol: "BTC".into(), sec_type: "CRYPTO".into(),
        exchange: "PAXOS".into(), currency: "USD".into(), ..Default::default()
    };
    let resolved = match client.qualify_contract(&contract) {
        Ok(c) => c,
        Err(e) => { println!("the contract could not be resolved: {e}"); return; }
    };

    let mut heard = Heard::default();
    // Priced off the book rather than sent at market: this venue refuses a
    // market order on this instrument, and a limit through the spread trades
    // just the same.
    client.req_mkt_data(1, &resolved, "", false, false).expect("quotes");
    drain(&client, &mut heard, 6);
    // Under the request id it was asked for, not the contract id.
    let mut quote = None;
    for _ in 0..40 {
        match client.quote(1) {
            Some(q) if q.ask > 0 => { quote = Some(q); break; }
            _ => {}
        }
        drain(&client, &mut heard, 1);
    }
    let Some(quote) = quote else { println!("  no quote arrived"); return };
    let scale = ibx::types::PRICE_SCALE as f64;
    let (bid, ask) = (quote.bid as f64 / scale, quote.ask as f64 / scale);
    println!("  BTC bid={bid} ask={ask}");
    if ask <= 0.0 { println!("  no offer to buy against"); return; }

    let qty = std::env::var("IBX_QTY").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0005);
    // Through the spread on both sides, so each trades rather than rests.
    // Bought first: the account holds none of this, and a sale it cannot
    // cover is refused.
    for (side, price) in [("BUY", ask * 1.002), ("SELL", bid * 0.998)] {
        let id = client.next_order_id();
        let price = (price * 4.0).round() / 4.0; // its own tick
        println!("\n[{side} {qty} BTC at {price} under {id}]");
        let order = Order {
            action: side.into(), order_type: "LMT".into(), lmt_price: price,
            total_quantity: qty, tif: "IOC".into(), ..Default::default()
        };
        if let Err(e) = client.place_order(id, &resolved, &order) {
            println!("  refused before sending: {e}");
            continue;
        }
        drain(&client, &mut heard, 10);
    }

    println!("\n[the fills, every field — tag 12 is where a commission would be]");
    let mut shown = 0;
    for (conn, hex) in client.unread_wire() {
        if conn != "trading-msg" { continue; }
        let Ok(bytes) = (0..hex.len()).step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>() else { continue };
        let body = String::from_utf8_lossy(&bytes);
        // The reports that carry a traded quantity — the fills themselves, not
        // the working-order acks and replays that share the message type.
        if !body.contains("35=8\u{1}") || !body.contains("\u{1}55=BTC\u{1}") { continue; }
        if body.contains("\u{1}97=Y\u{1}") { continue; }
        let field = |tag: &str| body.split('\u{1}')
            .find_map(|f| f.strip_prefix(tag).map(|v| v.to_string()))
            .unwrap_or_default();
        let traded = field("32=");
        if traded.is_empty() || traded.parse::<f64>().unwrap_or(0.0) == 0.0 { continue; }
        println!(
            "  traded {traded} — tag 12 (commission) {}, 6378 {}, 6381 {:?}",
            if body.contains("\u{1}12=") { "PRESENT" } else { "ABSENT" },
            if body.contains("\u{1}6378=") { "present" } else { "absent" },
            field("6381="),
        );
        for f in body.split('\u{1}').filter(|f| !f.is_empty()) { print!("  {f}"); }
        println!("\n");
        shown += 1;
        if shown >= 3 { break; }
    }
    if shown == 0 {
        println!("  no fill of this session's own was captured");
    }
}
