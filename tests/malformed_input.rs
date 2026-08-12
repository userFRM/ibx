//! Every wire parser, against input the venue would never send.
//!
//! A parser reads bytes off a socket. What arrives is whatever the socket
//! produced — a frame cut off mid-field by a close, a length that overruns what
//! followed it, a byte flipped in transit. None of that may take the process
//! down, because a client that panics on one malformed frame loses the session
//! and every subscription on it.
//!
//! Each parser is given: every prefix of a well-formed frame, that frame with
//! one byte replaced at each position, and runs of bytes that are not a frame
//! at all. The assertion is the absence of a panic — a parser that unwraps a
//! `None`, slices past an end, or subtracts below zero fails the test by
//! aborting it.
//!
//! The corruption is generated rather than recorded: a fixed sequence, so a
//! failure is reproducible from the test name and nothing else.

use ibx::control::contracts;
use ibx::control::{historical, news};
use ibx::protocol::{fix, ns, tbt_stream, tick_decoder, trading_status};

/// A byte sequence that is not a frame, from a stated seed. The same seed
/// always produces the same bytes, so a failure reproduces.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        })
        .collect()
}

/// Every way one frame can arrive wrong: cut short at each length, one byte
/// replaced at each position, and noise of the same length.
fn every_corruption(frame: &[u8]) -> Vec<Vec<u8>> {
    let mut all = Vec::new();
    for cut in 0..=frame.len() {
        all.push(frame[..cut].to_vec());
    }
    for at in 0..frame.len() {
        for byte in [0u8, 1, b'=', 0x01, 0x7f, 0xff] {
            let mut copy = frame.to_vec();
            copy[at] = byte;
            all.push(copy);
        }
    }
    for seed in 0..8 {
        all.push(noise(seed, frame.len()));
        all.push(noise(seed, frame.len() * 3));
    }
    all
}

/// A FIX-shaped frame: fields separated by SOH, as every control message is.
fn fix_frame(fields: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (tag, value) in fields {
        out.extend_from_slice(tag.as_bytes());
        out.push(b'=');
        out.extend_from_slice(value.as_bytes());
        out.push(0x01);
    }
    out
}

fn a_contract_definition() -> Vec<u8> {
    fix_frame(&[
        ("55", "SPY"), ("6008", "756733"), ("461", "CS"), ("15", "USD"),
        ("207", "ARCA"), ("6035", "SPY"), ("6177", "ARCA,BATS,NASDAQ"),
        ("711", "2"), ("311", "US78462F1030"), ("22", "4"),
    ])
}

#[test]
fn a_contract_definition_that_arrives_wrong_is_not_fatal() {
    for wrong in every_corruption(&a_contract_definition()) {
        let _ = contracts::parse_secdef_responses(&wrong, true);
        let _ = contracts::parse_secdef_responses(&wrong, false);
        let _ = contracts::secdef_response_req_id(&wrong);
        let _ = contracts::secdef_response_is_last(&wrong);
        let _ = contracts::unread_definition_tags(&wrong);
    }
}

#[test]
fn a_reference_answer_that_arrives_wrong_is_not_fatal() {
    let frames = [
        fix_frame(&[("6503", "26"), ("6504", "0.01"), ("6505", "0.01")]),
        fix_frame(&[("6531", "20260812"), ("6532", "0930"), ("6533", "1600")]),
        fix_frame(&[("55", "SP"), ("6008", "756733"), ("461", "CS"), ("207", "ARCA")]),
        fix_frame(&[("6183", "SPY"), ("6184", "AMEX"), ("6185", "100"),
                    ("6186", "20260918,20261016"), ("6187", "400,405,410")]),
    ];
    for frame in frames {
        for wrong in every_corruption(&frame) {
            let _ = contracts::parse_market_rules(&wrong);
            let _ = contracts::parse_schedule_response(&wrong);
            let _ = contracts::parse_matching_symbols_response(&wrong);
            let _ = contracts::parse_option_chain_response(&wrong);
        }
    }
}

#[test]
fn a_news_payload_that_arrives_wrong_is_not_fatal() {
    let frame = fix_frame(&[
        ("6401", "BRFG$abc123"), ("6402", "20260812-13:30:00"),
        ("6403", "A headline"), ("6404", "BRFG"),
    ]);
    for wrong in every_corruption(&frame) {
        let _ = news::parse_news_payload(&wrong);
        let _ = news::parse_article_payload(&wrong);
    }
}

#[test]
fn a_tick_stream_that_arrives_wrong_is_not_fatal() {
    // Ticks are not FIX: a stream of variable-length quantities, each ending on
    // the byte with the high bit set. A cut in the middle of one leaves a
    // length that runs past what followed it.
    let stream: Vec<u8> = vec![
        0x81, 0x02, 0x83, 0xd0, 0x0f, 0x81, 0x84, 0x01, 0x02, 0x03, 0xff,
        0x80, 0x00, 0x7f, 0xfe, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81,
    ];
    for wrong in every_corruption(&stream) {
        let _ = tick_decoder::decode_ticks_35p(&wrong);
        let _ = tick_decoder::decode_ticks_35e(&wrong);
        let _ = tick_decoder::decode_bar_payload(&wrong, 0.01);
        let _ = historical::decode_bar_payload(&wrong, 0.01);
        let _ = tbt_stream::frame_ticker_id(&wrong);
        let _ = trading_status::parse_trading_status(&wrong);
    }
}

#[test]
fn a_session_message_that_arrives_wrong_is_not_fatal() {
    let frames = [
        fix_frame(&[("35", "A"), ("49", "user"), ("56", "cdc"), ("34", "1")]),
        fix_frame(&[("35", "1"), ("112", "20260812-13:30:00.000")]),
        b"NS\x0140\x01hello\x01".to_vec(),
    ];
    for frame in frames {
        for wrong in every_corruption(&frame) {
            let _ = fix::fix_parse(&wrong);
            let _ = fix::fix_checksum(&wrong);
            let _ = ns::ns_parse(&wrong);
            let _ = ns::is_ns_text(&wrong);
            let _ = ns::parse_test_request_timestamp(&wrong);
        }
    }
}

#[test]
fn a_frame_of_pure_noise_is_not_fatal() {
    // Nothing about these is a frame. A parser reached by a resynchronising
    // reader sees exactly this.
    for seed in 0..64u64 {
        for len in [0usize, 1, 2, 3, 7, 16, 64, 255, 256, 1024] {
            let bytes = noise(seed, len);
            let _ = contracts::parse_secdef_responses(&bytes, true);
            let _ = contracts::parse_market_rules(&bytes);
            let _ = contracts::parse_schedule_response(&bytes);
            let _ = contracts::parse_matching_symbols_response(&bytes);
            let _ = contracts::parse_option_chain_response(&bytes);
            let _ = news::parse_news_payload(&bytes);
            let _ = news::parse_article_payload(&bytes);
            let _ = tick_decoder::decode_ticks_35p(&bytes);
            let _ = tick_decoder::decode_ticks_35e(&bytes);
            let _ = tick_decoder::decode_bar_payload(&bytes, 0.01);
            let _ = historical::decode_bar_payload(&bytes, 0.01);
            let _ = tbt_stream::frame_ticker_id(&bytes);
            let _ = trading_status::parse_trading_status(&bytes);
            let _ = fix::fix_parse(&bytes);
            let _ = ns::ns_parse(&bytes);
            let _ = ns::parse_test_request_timestamp(&bytes);
        }
    }
}
