//! What a depth subscription looks like on the wire, without the engine.
//!
//! Sent directly on the market-data connection, so what is being read is the
//! message this client writes and the venue's answer to it — not the engine's
//! handling of either.
use ibx::gateway::{Gateway, GatewayConfig};
use ibx::protocol::fixcomp;
use std::time::{Duration, Instant};

fn config() -> GatewayConfig {
    GatewayConfig {
        settings: Default::default(),
        username: std::env::var("IB_USERNAME").expect("IB_USERNAME"),
        password: zeroize::Zeroizing::new(std::env::var("IB_PASSWORD").expect("IB_PASSWORD")),
        host: std::env::var("IB_HOST").unwrap_or_else(|_| "cdc1.ibllc.com".to_string()),
        paper: true,
        accept_invalid_certs: false,
        ib_key_timeout_secs: ibx::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
        ib_key_token_sub_type: ibx::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
        code_provider: None,
        resume: None,
    }
}

#[test]
#[ignore = "opens a session of its own, which the account allows one of; run it with --ignored"]
fn raw_farm_subscribe_test() {
    let cfg = config();
    let ibx::gateway::Session { gateway: _gw, market_data: mut farm, trading: _ccp, historical: _hmds, .. } =
        Gateway::connect(&cfg).expect("Gateway connect failed");

    eprintln!("Farm connected, seq={}", farm.seq);

    eprintln!("sign_iv={:02x?}", &farm.sign_iv[..8]);
    farm.seq = 0;

    // L1: 35=V|263=1|146=1|262=1|6008=265598|207=BEST|167=CS|264=0|9830=1|
    let r1 = farm.send_fixcomp(&[
        (35, "V"), (263, "1"), (146, "1"), (262, "1"),
        (6008, "265598"), (207, "BEST"), (167, "CS"), (264, "0"), (9830, "1"),
    ]);
    r1.expect("the level-one subscription is sent");
    eprintln!("L1 subscribe: seq={}", farm.seq);

    // Depth: 35=V|263=1|146=1|262=2|6008=265598|207=NASDAQ|167=CS|264=626|
    let r2 = farm.send_fixcomp(&[
        (35, "V"), (263, "1"), (146, "1"), (262, "2"),
        (6008, "265598"), (207, "NASDAQ"), (167, "CS"), (264, "626"),
    ]);
    r2.expect("the depth subscription is sent");
    eprintln!("Depth subscribe: seq={}", farm.seq);

    // Poll for 15 seconds
    let start = Instant::now();
    let mut total_msgs = 0;
    let mut rejected: Vec<String> = Vec::new();
    let mut acked = 0usize;
    while start.elapsed() < Duration::from_secs(15) {
        match farm.try_recv() {
            Ok(n) if n > 0 => eprintln!("  recv {n} bytes"),
            _ => {}
        }

        let frames = farm.extract_frames();
        for frame in &frames {
            total_msgs += 1;
            let raw = match &frame {
                ibx::protocol::connection::Frame::Fix(d) |
                ibx::protocol::connection::Frame::FixComp(d) |
                ibx::protocol::connection::Frame::Binary(d) => d.as_slice(),
                // Control-state frames are not consumed downstream.
                ibx::protocol::connection::Frame::Control(_) => continue,
            };
            // Decompress if FIXCOMP
            let msgs = if raw.starts_with(b"8=FIXCOMP") {
                let Some(unsigned) = farm.unsign(raw) else { continue };
                fixcomp::fixcomp_decompress(&unsigned).unwrap_or_default()
            } else {
                let Some(unsigned) = farm.unsign(raw) else { continue };
                vec![unsigned]
            };
            for msg in &msgs {
                let text = String::from_utf8_lossy(msg);
                // Extract 35=
                let mt = text.split('\x01')
                    .find(|s| s.starts_with("35="))
                    .map(|s| &s[3..])
                    .unwrap_or("?");
                if mt == "P" || mt == "Y" {
                    eprintln!("[{}] 35={} ({} bytes) — DATA!", total_msgs, mt, msg.len());
                } else if mt == "Q" {
                    acked += 1;
                    eprintln!("[{}] 35=Q ack: {}", total_msgs, &text[..text.len().min(120)]);
                } else if mt == "Q" || mt == "3" {
                    // Counted above for the ack; a reject is the venue refusing
                    // the message this test exists to state correctly.
                    if mt == "3" {
                        rejected.push(text[..text.len().min(200)].to_string());
                        eprintln!("[{}] 35=3 REJECT: {}", total_msgs, &text[..text.len().min(200)]);
                    }
                } else {
                    eprintln!("[{}] 35={} ({} bytes)", total_msgs, mt, msg.len());
                }
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    eprintln!("\nTotal: {total_msgs} messages in 15s, {acked} acknowledged");

    // What this states, rather than what it printed. Every path through the
    // loop above ended in a line on the terminal, so a subscription the venue
    // refused, a message that would not decompress and a connection that said
    // nothing at all all finished the same way: reported as passing.
    assert!(
        rejected.is_empty(),
        "the venue refused a subscription this client sent: {rejected:?}",
    );
    assert!(
        total_msgs > 0,
        "the venue said nothing at all in 15s on a connection it had accepted, \
         so neither subscription reached it in a form it recognised",
    );
    assert!(
        acked > 0,
        "{total_msgs} message(s) arrived and none of them acknowledged either \
         subscription, so what the venue answered was about something else",
    );
}
