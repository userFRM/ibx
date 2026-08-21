//! Ask the venue for the account and record what comes back.
//!
//! The opening sequence subscribes to account values. A session that receives
//! none cannot tell a venue that sent nothing from a message this client did
//! not read, so this prints both: the account state the session holds, and
//! every message type the connection carried that nothing here consumes.
//!
//! Reads only. It places nothing.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin capture_account

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};

fn main() {
    let (Ok(username), Ok(password)) = (std::env::var("IB_USERNAME"), std::env::var("IB_PASSWORD"))
    else {
        println!("IB_USERNAME and IB_PASSWORD are required");
        return;
    };

    let config = EClientConfig { username, password, paper: true, ..Default::default() };
    let client = match EClient::connect(&config) {
        Ok(c) => c,
        Err(e) => {
            println!("the session did not open: {e}");
            return;
        }
    };
    println!("session open, account {}", client.account_id);

    // Ask for them. Without this the venue restates them on its own schedule.
    if std::env::var("IBX_NO_REFRESH").is_err() {
        client.req_account_updates(true, "");
        println!("  asked for the account");
    } else {
        println!("  not asking; waiting on the venue's schedule");
    }

    // Poll the state rather than a callback so an arrival that reaches the
    // store but no caller is still visible.
    let started = Instant::now();
    let deadline = started + Duration::from_secs(60);
    let mut seen = false;
    while Instant::now() < deadline {
        let a = client.shared_state().portfolio.account();
        if a.net_liquidation != 0 || a.total_cash_value != 0 || a.buying_power != 0 {
            println!(
                "  account arrived after {:?}: net_liquidation={} cash={} buying_power={}",
                started.elapsed(),
                a.net_liquidation,
                a.total_cash_value,
                a.buying_power
            );
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if !seen {
        println!("  no account value arrived in forty seconds");
    }

    println!("\n  message types carried but not read here:");
    let unread = client.unread_wire();
    if unread.is_empty() {
        println!("    none");
    }
    for (connection, what) in unread.iter().take(60) {
        println!("    {connection}: {what}");
    }
}
