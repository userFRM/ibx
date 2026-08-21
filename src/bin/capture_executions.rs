//! What executions a session opens with.
//!
//! Asks for every execution the venue still holds, then asks the narrow way,
//! and counts both. The difference is what the narrow request omits.
//!
//! Reads only. It places nothing.

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};

fn opened_with(scope: &str) -> usize {
    // Safety: single-threaded here, and set before the session opens.
    unsafe { std::env::set_var("IBX_EXECUTION_REPORTS", scope) };
    let config = EClientConfig {
        username: std::env::var("IB_USERNAME").unwrap_or_default(),
        password: std::env::var("IB_PASSWORD").unwrap_or_default(),
        paper: true,
        ..Default::default()
    };
    let client = match EClient::connect(&config) {
        Ok(c) => c,
        Err(e) => {
            println!("  {scope}: could not open a session: {e}");
            return 0;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
    }
    let fills = client.shared_state().orders.drain_fills().len();
    client.disconnect();
    fills
}

fn main() {
    let _ = env_logger::try_init();
    if std::env::var("IB_USERNAME").unwrap_or_default().trim().is_empty() {
        eprintln!("IB_USERNAME/IB_PASSWORD unset. This reads from real servers.");
        std::process::exit(2);
    }
    let all = opened_with("all");
    println!("  asking the documented way: {all} execution(s)");
    let today = opened_with("today");
    println!("  asking only for today's:            {today} execution(s)");
    println!(
        "\n  a caller migrating would have lost {} execution(s) at connect",
        all.saturating_sub(today),
    );
}
