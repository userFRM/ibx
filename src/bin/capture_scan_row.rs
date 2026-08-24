//! What the venue states on a row of a scan.
//!
//! This client reads a contract id from each row and hands a caller empty
//! strings where the reference client states a distance, a benchmark, a
//! projection and the legs of a combination. Asked of the wire on a
//! top-percent-gainers scan of US stocks, a row is two elements — the contract
//! id and the time it entered the scan — and none of the four is among them,
//! so the empty strings are what there is to say. Run this against another
//! scan before concluding the same of that one.
//!
//! Reads only. It places nothing. Run with `RUST_LOG=ibx=debug`.
//!
//!     IB_USERNAME=… IB_PASSWORD=… RUST_LOG=ibx=debug \
//!       cargo run --features dev-tools --bin capture_scan_row

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard { rows: usize, said: Vec<String> }
impl Wrapper for Heard {
    fn scanner_data(
        &mut self, _req: i64, rank: i32, details: &ibx::types::model::ContractDetails,
        distance: &str, benchmark: &str, projection: &str, legs: &str,
    ) {
        if self.rows < 4 {
            println!(
                "  rank {rank:<3} {:<10} distance={distance:?} benchmark={benchmark:?} \
                 projection={projection:?} legs={legs:?}",
                details.contract.symbol,
            );
        }
        self.rows += 1;
    }
    fn error(&mut self, _r: i64, c: i64, m: &str, _: &str) {
        if !matches!(c, 2104 | 2106 | 2107 | 2119 | 2158) {
            self.said.push(format!("{c}: {m}"));
        }
    }
}

fn main() {
    let _ = env_logger::try_init();
    let client = EClient::connect(&EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true, ..Default::default() }).expect("session");
    println!("session open");
    if let Err(e) = client.req_scanner_subscription(
        9001, "STK", "STK.US.MAJOR", "TOP_PERC_GAIN", 10, &[],
    ) {
        println!("refused: {e}");
        return;
    }
    let mut heard = Heard::default();
    let until = Instant::now() + Duration::from_secs(25);
    while Instant::now() < until {
        client.process_msgs(&mut heard);
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("  {} rows", heard.rows);
    for s in heard.said.iter().take(3) { println!("  said: {s}"); }
}
