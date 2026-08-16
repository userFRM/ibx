//! Microbenchmark: tick decoder + market state update hot path.
//!
//! Measures pure decode + state update latency without any network or I/O.
//! Generates realistic 35=P binary payloads and benchmarks:
//!   1. decode_ticks_35p() alone
//!   2. decode + market state update (the handle_tick_data equivalent)
//!   3. decode + state update + SeqLock push + channel send (full notify path)
//!
//! Also benchmarks BitReader, fix_parse, and find_body_after_tag individually.

#[path = "support/payload.rs"]
mod payload;
use payload::*;

use std::sync::Arc;
use std::time::Instant;

use std::sync::mpsc::sync_channel;

use ibx::bridge::{Event, SharedState};
use ibx::engine::market_state::MarketState;
use ibx::protocol::tick_decoder::{self};

const ITERATIONS: u64 = 1_000_000;
const WARMUP: u64 = 100_000;

fn main() {
    // ── Build realistic 35=P payloads ──
    // Simulate a typical SPY tick message: 1 server_tag, 3 ticks (bid, ask, last)
    let payload_small = build_35p_payload(1, &[
        (0, 1, 15025, false),  // O_BID_PRICE, width=1... we'll use width 2
    ]);

    // Typical: bid_price + ask_price + last_price + bid_size + ask_size
    let payload_typical = build_35p_payload(1, &[
        (0, 2, 15025, false),  // bid_price
        (1, 2, 15030, false),  // ask_price
        (2, 2, 15028, false),  // last_price
        (4, 2, 500, false),    // bid_size
        (5, 2, 300, false),    // ask_size
    ]);

    // Heavy: 2 server tags, 5 ticks each
    let payload_heavy = build_35p_payload_multi(&[
        (1, &[
            (0, 2, 15025, false),
            (1, 2, 15030, false),
            (2, 2, 15028, false),
            (4, 2, 500, false),
            (5, 2, 300, false),
        ]),
        (2, &[
            (0, 2, 20050, false),
            (1, 2, 20055, false),
            (2, 2, 20052, false),
            (4, 2, 200, false),
            (tick_decoder::O_VOLUME, 4, 1_000_000, false),
        ]),
    ]);

    // Build a full FIX-framed 35=P message for find_body_after_tag + fix_parse
    // benchmarks
    let fix_framed = build_fix_framed_35p(&payload_typical);

    println!("========================================");
    println!("  Bench: Tick Decoder Hot Path");
    println!("========================================");
    println!();
    println!("  Iterations:     {ITERATIONS}");
    println!("  Warmup:         {WARMUP}");
    println!();

    // ── 1. Pure decode_ticks_35p ──
    bench("decode_ticks_35p (1 tick)", ITERATIONS, || {
        let _ = tick_decoder::decode_ticks_35p(&payload_small);
    });

    bench("decode_ticks_35p (5 ticks, typical)", ITERATIONS, || {
        let _ = tick_decoder::decode_ticks_35p(&payload_typical);
    });

    bench("decode_ticks_35p (10 ticks, 2 tags)", ITERATIONS, || {
        let _ = tick_decoder::decode_ticks_35p(&payload_heavy);
    });

    // ── 2. find_body_after_tag ──
    bench("find_body_after_tag", ITERATIONS, || {
        let _ = find_body_after_tag(&fix_framed, b"35=P\x01");
    });

    // ── 3. fix_parse (HashMap alloc) ──
    bench("fix_parse (full HashMap)", ITERATIONS, || {
        let _ = ibx::protocol::fix::fix_parse(&fix_framed);
    });

    // ── 4. fast_msg_type extraction, the alternative to fix_parse ──
    bench("fast_msg_type (byte scan)", ITERATIONS, || {
        let _ = fast_msg_type(&fix_framed);
    });

    // ── 5. decode + state update (handle_tick_data equivalent) ──
    {
        let mut market = MarketState::new();
        let id = market.register(756733); // SPY
        market.register_server_tag(1, id);
        market.set_min_tick(id, 0.01);

        bench("decode + state update (5 ticks)", ITERATIONS, || {
            let ticks = tick_decoder::decode_ticks_35p(&payload_typical);
            for tick in &ticks {
                if let Some(instrument) = market.instrument_by_server_tag(tick.server_tag) {
                    let mts = market.min_tick_scaled(instrument);
                    let q = market.quote_mut(instrument);
                    apply_tick(q, tick, mts);
                }
            }
        });
    }

    // ── 6. decode + state + SeqLock push ──
    {
        let mut market = MarketState::new();
        let id = market.register(756733);
        market.register_server_tag(1, id);
        market.set_min_tick(id, 0.01);
        let shared = Arc::new(SharedState::new());

        bench("decode + state + SeqLock push (5 ticks)", ITERATIONS, || {
            let ticks = tick_decoder::decode_ticks_35p(&payload_typical);
            for tick in &ticks {
                if let Some(instrument) = market.instrument_by_server_tag(tick.server_tag) {
                    let mts = market.min_tick_scaled(instrument);
                    let q = market.quote_mut(instrument);
                    apply_tick(q, tick, mts);
                }
            }
            shared.market.push_quote(id, market.quote(id));
        });
    }

    // ── 7. Full path: decode + state + SeqLock + channel send ──
    {
        let mut market = MarketState::new();
        let id = market.register(756733);
        market.register_server_tag(1, id);
        market.set_min_tick(id, 0.01);
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = sync_channel::<Event>(65536);
        let mut sent = 0u64;

        bench("full path: decode+state+seqlock+channel (5 ticks)", ITERATIONS, || {
            let ticks = tick_decoder::decode_ticks_35p(&payload_typical);
            for tick in &ticks {
                if let Some(instrument) = market.instrument_by_server_tag(tick.server_tag) {
                    let mts = market.min_tick_scaled(instrument);
                    let q = market.quote_mut(instrument);
                    apply_tick(q, tick, mts);
                }
            }
            shared.market.push_quote(id, market.quote(id));
            let _ = tx.try_send(Event::Tick(id));
            sent += 1;
            if sent.is_multiple_of(60000) {
                while rx.try_recv().is_ok() {}
            }
        });

        while rx.try_recv().is_ok() {}
    }

    // ── 8. notify_tick overhead: set_account (Mutex) ──
    {
        let shared = Arc::new(SharedState::new());
        let account = ibx::engine::context::Context::new();

        bench("set_account (Mutex lock)", ITERATIONS, || {
            shared.portfolio.set_account(account.account());
        });
    }

    // ── 9. BitReader: read 5 ticks worth of bits ──
    {
        // Read 5 x (5+1+2+16) = 120 bits
        let data = vec![0xAA; 20]; // 160 bits
        bench("BitReader: 120 bits (5 ticks)", ITERATIONS, || {
            let mut reader = tick_decoder::BitReader::new(&data, 120);
            for _ in 0..5 {
                let _ = reader.read_unsigned(5);
                let _ = reader.read_unsigned(1);
                let _ = reader.read_unsigned(2);
                let _ = reader.read_unsigned(16);
            }
        });
    }
}

// ── Payload builders ──

fn build_fix_framed_35p(tick_payload: &[u8]) -> Vec<u8> {
    // Build: 8=O\x01 9=<len>\x01 35=P\x01 <tick_payload> \x01 8349=AABBCCDD\x01
    let inner_body_str = "35=P\x01".to_string();
    let inner_body = inner_body_str.as_bytes();
    let body_len = inner_body.len() + tick_payload.len() + 15; // +15 for 8349=AABBCCDD\x01
    let header = format!("8=O\x019={body_len}\x01");
    let mut msg = header.into_bytes();
    msg.extend_from_slice(inner_body);
    msg.extend_from_slice(tick_payload);
    msg.extend_from_slice(b"\x018349=AABBCCDD\x01");
    msg
}

fn find_body_after_tag<'a>(msg: &'a [u8], tag_marker: &[u8]) -> Option<&'a [u8]> {
    msg.windows(tag_marker.len())
        .position(|w| w == tag_marker)
        .map(|pos| &msg[pos + tag_marker.len()..])
}

fn fast_msg_type(msg: &[u8]) -> Option<u8> {
    // Fast extraction: 35= is always within the first 30 bytes of a FIX/binary msg
    let limit = msg.len().min(40);
    let slice = &msg[..limit];
    for i in 0..limit.saturating_sub(3) {
        if slice[i] == b'3' && slice[i + 1] == b'5' && slice[i + 2] == b'=' {
            return Some(slice.get(i + 3).copied().unwrap_or(0));
        }
    }
    None
}

#[inline]
fn bench(label: &str, iterations: u64, mut f: impl FnMut()) {
    // Warmup
    for _ in 0..WARMUP {
        f();
    }

    // Timed run
    let start = Instant::now();
    for _ in 0..iterations {
        f();
    }
    let elapsed = start.elapsed();

    let total_ns = elapsed.as_nanos() as u64;
    let per_iter_ns = total_ns / iterations;

    println!(
        "  {:<52} {:>6} ns/iter  ({:.2}s total)",
        label,
        per_iter_ns,
        elapsed.as_secs_f64(),
    );
}
