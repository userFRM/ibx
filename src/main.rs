use std::env;

use ibx::api::{EClient, EClientConfig, Wrapper, Contract, TickAttrib};

struct QuotePrinter;

impl Wrapper for QuotePrinter {
    fn error(&mut self, req_id: i64, error_code: i64, error_string: &str, _: &str) {
        eprintln!("Error req_id={req_id} code={error_code}: {error_string}");
    }

    fn tick_price(&mut self, req_id: i64, tick_type: i32, price: f64, _attrib: &TickAttrib) {
        let label = match tick_type {
            1 => "bid",
            2 => "ask",
            4 => "last",
            _ => return,
        };
        println!("req_id={req_id} {label}={price:.2}");
    }

    fn tick_size(&mut self, req_id: i64, tick_type: i32, size: f64) {
        let label = match tick_type {
            0 => "bid_size",
            3 => "ask_size",
            5 => "last_size",
            8 => "volume",
            _ => return,
        };
        println!("req_id={req_id} {label}={size}");
    }
}

fn main() {
    let _log = ibx::logging::init(&ibx::logging::LogConfig::from_env());
    println!("ibx v{}", env!("CARGO_PKG_VERSION"));

    let username = env::var("IB_USERNAME").unwrap_or_else(|_| {
        eprintln!("Set IB_USERNAME and IB_PASSWORD environment variables");
        std::process::exit(1);
    });
    let password = env::var("IB_PASSWORD").unwrap_or_else(|_| {
        eprintln!("Set IB_PASSWORD environment variable");
        std::process::exit(1);
    });
    let paper = env::var("IB_PAPER").unwrap_or_else(|_| "true".to_string()) == "true";
    // The door every session knocks on, from the one place it is written. The
    // venue answers by naming which server this account is on, so this is a
    // starting point rather than a destination.
    let host = env::var("IB_HOST")
        .unwrap_or_else(|_| ibx::config::CCP_HOSTS[0].to_string());

    let client = EClient::connect(&EClientConfig {
        username,
        password,
        host,
        paper,
        core_id: None,
        code_provider: None,
        ..Default::default()
    }).unwrap_or_else(|e| {
        eprintln!("Connection failed: {e}");
        std::process::exit(1);
    });

    println!("Connected!");

    let contract = Contract {
        con_id: 756733,
        symbol: "SPY".into(),
        sec_type: "STK".into(),
        exchange: "SMART".into(),
        currency: "USD".into(),
        ..Contract::default()
    };

    client.req_mkt_data(1, &contract, "", false, false).unwrap();
    println!("Requested market data for SPY (req_id=1)");

    // Zero-copy SeqLock escape hatch still available:
    // let q = client.quote(1);

    let mut wrapper = QuotePrinter;
    loop {
        client.process_msgs(&mut wrapper);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
