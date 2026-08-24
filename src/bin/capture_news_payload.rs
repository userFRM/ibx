//! What a historical-news answer actually carries.
//!
//! The rows are parsed by splitting on a delimiter and counting fields, the
//! "more to come" flag is read as the word `true`, and a `{...}` prefix is
//! cut off every headline. None of the three rests on a captured answer.
//! This asks for real headlines and prints the answer as it arrived.
//!
//!     IB_USERNAME=… IB_PASSWORD=… cargo run --features dev-tools --bin capture_news_payload

use std::time::{Duration, Instant};

use ibx::api::client::{EClient, EClientConfig};
use ibx::api::types::Contract;
use ibx::api::wrapper::Wrapper;

#[derive(Default)]
struct Heard {
    lines: Vec<String>,
    rows: usize,
    misread: usize,
}

impl Wrapper for Heard {
    fn historical_news(
        &mut self, req_id: i64, time: &str, provider: &str, article_id: &str, headline: &str,
    ) {
        // The row is split on a delimiter and read by position. The vendor
        // finds the date first and treats everything before it as the
        // headline, precisely because a headline may contain the delimiter.
        // A time that is not a time is that going wrong.
        let time_is_a_time = time.len() >= 16
            && time.as_bytes()[4] == b'-' && time.as_bytes()[7] == b'-'
            && time.as_bytes()[10] == b' ' && time.as_bytes()[13] == b':';
        if !time_is_a_time {
            self.misread += 1;
            self.lines.push(format!(
                "  MISREAD {req_id}: time={time:?} provider={provider:?} id={article_id:?}\n           headline={headline:?}"
            ));
            return;
        }
        self.rows += 1;
        self.lines.push(format!(
            "  parsed {req_id}: time={time:?} provider={provider:?} id={article_id:?}\n           headline={headline:?}"
        ));
    }
    fn historical_news_end(&mut self, req_id: i64, has_more: bool) {
        self.lines.push(format!("  end {req_id}: has_more={has_more}"));
    }
    fn error(&mut self, req_id: i64, code: i64, message: &str, _adv: &str) {
        self.lines.push(format!("  error {req_id}/{code}: {message}"));
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

    let providers: String = client.shared_state().reference.news_providers()
        .iter().map(|p| p.code.as_str()).collect::<Vec<_>>().join("+");
    println!("  entitled providers: {providers:?}");
    if providers.is_empty() {
        println!("  no news entitlement on this login — nothing to ask for");
        return;
    }

    let contract = Contract {
        symbol: "AAPL".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), currency: "USD".into(), ..Default::default()
    };
    let resolved = match client.qualify_contract(&contract) {
        Ok(c) => c,
        Err(e) => { println!("  the contract could not be resolved: {e}"); return; }
    };

    let mut heard = Heard::default();
    if let Err(e) = client.req_historical_news(9001, resolved.con_id, &providers, "", "", 250) {
        println!("  refused before sending: {e}");
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(25);
    while Instant::now() < deadline {
        client.process_msgs(&mut heard);
        for l in heard.lines.drain(..) { println!("{l}"); }
        std::thread::sleep(Duration::from_millis(100));
    }

    println!("\n  {} rows read, {} where the fields did not line up", heard.rows, heard.misread);
    println!("\n[the answer as it arrived]");
    let mut shown = 0;
    for (conn, hex) in client.unread_wire() {
        let Ok(bytes) = (0..hex.len()).step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>() else { continue };
        let body = String::from_utf8_lossy(&bytes);
        // Only the connection the answer would come back on.
        if conn != "hmds-msg" { continue; }
        println!("  --- {} bytes ---", bytes.len());
        let printable: String = body.chars()
            .map(|c| if c.is_ascii_graphic() || c == ' ' || c == '\n' { c } else { '.' })
            .take(600)
            .collect();
        for line in printable.lines().take(14) {
            println!("  {}", line.trim_end());
        }
        shown += 1;
        if shown >= 3 { break; }
    }
    if shown == 0 {
        println!(
            "  nothing came back. The answer decides three things this client\n               currently decides for itself: whether the flag for more to come is\n               stated as a word or a number, whether a row can be read by counting\n               its fields, and whether a headline really carries a brace-wrapped\n               prefix worth cutting off. Run it on a login the archive answers.",
        );
    }
}
