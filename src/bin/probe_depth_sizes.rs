//! What size a book level actually carries.
//!
//! Depth sizes here are scaled by the width the venue encoded them in — a
//! one-byte size is multiplied by a hundred and a two-byte one is not — while
//! the vendor scales every size by a figure belonging to the CONTRACT, which
//! is one for anything that is not a share. So a future's book should come
//! back in whole contracts, and a hundred times that says the width is being
//! read as a unit.
//!
//! A future trades when the shares do not, so this answers outside a New York
//! session as well as inside one.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin probe_depth_sizes

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    levels: Vec<(i32, i32, f64, f64)>,
    said: Vec<String>,
}

impl Wrapper for Heard {
    fn update_mkt_depth(
        &mut self, _req: i64, position: i32, _operation: i32,
        side: i32, price: f64, size: f64,
    ) {
        self.levels.push((position, side, price, size));
    }
    fn update_mkt_depth_l2(
        &mut self, _req: i64, position: i32, _mm: &str, _operation: i32,
        side: i32, price: f64, size: f64, _smart: bool,
    ) {
        self.levels.push((position, side, price, size));
    }
    fn error(&mut self, _req: i64, code: i64, message: &str, _adv: &str) {
        if !matches!(code, 2104 | 2106 | 2107 | 2119 | 2158) {
            self.said.push(format!("{code}: {message}"));
        }
    }
}

fn main() {
    let _ = env_logger::try_init();
    let client = match EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true, ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => { eprintln!("could not open a session: {e}"); std::process::exit(1); }
    };
    println!("session open");

    // A future, which trades tonight, and a share, which does not — the
    // question is which of them this login may see a book for at all.
    let subjects = [
        ("a future", Contract {
            symbol: "MES".into(), sec_type: "FUT".into(), exchange: "CME".into(),
            currency: "USD".into(),
            last_trade_date_or_contract_month: "202709".into(), ..Default::default()
        }),
        ("a share", Contract {
            symbol: "SPY".into(), sec_type: "STK".into(), exchange: "ARCA".into(),
            currency: "USD".into(), ..Default::default()
        }),
        ("a share, smart", Contract {
            symbol: "AAPL".into(), sec_type: "STK".into(), exchange: "ISLAND".into(),
            currency: "USD".into(), ..Default::default()
        }),
    ];
    let mut heard = Heard::default();
    let mut req = 0i64;
    for (what, contract) in subjects {
        req += 1;
        let resolved = match client.qualify_contract(&contract) {
            Ok(c) => c,
            Err(e) => { println!("  {what}: could not be named: {e}"); continue; }
        };
        heard.said.clear();
        let before = heard.levels.len();
        if let Err(e) = client.req_mkt_depth(req, &resolved, 5, false) {
            println!("  {what}: refused before sending: {e}");
            continue;
        }
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            client.process_msgs(&mut heard);
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = client.cancel_mkt_depth(req);
        println!(
            "  {what:16} {} levels{}",
            heard.levels.len() - before,
            heard.said.first().map(|s| format!("  — {s}")).unwrap_or_default(),
        );
    }

    println!("\n  levels heard: {}", heard.levels.len());
    for (position, side, price, size) in heard.levels.iter().rev().take(10) {
        println!(
            "    {} row {position}  price={price}  size={size}",
            if *side == 1 { "bid" } else { "ask" },
        );
    }
    for s in heard.said.iter().take(4) {
        println!("  said: {s}");
    }
    println!(
        "\n  A book on this future is quoted in whole contracts, a handful to a\n  \
         few tens at a level. Sizes in the hundreds or thousands are the width\n  \
         being read as a unit.",
    );
}
