//! Building the tick payloads the benchmarks decode.
//!
//! Two benchmarks measure the same decoder from opposite ends — one over a
//! captured session, one over payloads composed here — so the composing is
//! written once and read by both.


use ibx::protocol::tick_decoder::{self, RawTick};
use ibx::types::Quote;

/// One tick as the payload builder states it: price, size, time and a flag.
pub type TickTuple = (u64, u64, u64, bool);

pub fn push_bits(bits: &mut Vec<u8>, val: u64, n: usize) {
    for i in (0..n).rev() {
        bits.push(((val >> i) & 1) as u8);
    }
}

pub fn build_35p_payload(server_tag: u32, ticks: &[(u64, u64, u64, bool)]) -> Vec<u8> {
    let mut bits: Vec<u8> = Vec::new();

    // server_tag header: 1-bit cont + 31-bit tag
    push_bits(&mut bits, 0, 1);
    push_bits(&mut bits, server_tag as u64, 31);

    for (i, &(tick_type, width, value, negative)) in ticks.iter().enumerate() {
        let has_more = if i < ticks.len() - 1 { 1 } else { 0 };
        push_bits(&mut bits, tick_type, 5);
        push_bits(&mut bits, has_more, 1);
        push_bits(&mut bits, width - 1, 2);
        push_bits(&mut bits, if negative { 1 } else { 0 }, 1);
        push_bits(&mut bits, value, (width * 8 - 1) as usize);
    }

    finalize_payload(&bits)
}

pub fn build_35p_payload_multi(tags: &[(u32, &[TickTuple])]) -> Vec<u8> {
    let mut bits: Vec<u8> = Vec::new();

    for (tag_idx, &(server_tag, ticks)) in tags.iter().enumerate() {
        let cont = if tag_idx > 0 { 1 } else { 0 };
        push_bits(&mut bits, cont, 1);
        push_bits(&mut bits, server_tag as u64, 31);

        for (i, &(tick_type, width, value, negative)) in ticks.iter().enumerate() {
            let has_more = if i < ticks.len() - 1 { 1 } else { 0 };
            push_bits(&mut bits, tick_type, 5);
            push_bits(&mut bits, has_more, 1);
            push_bits(&mut bits, width - 1, 2);
            push_bits(&mut bits, if negative { 1 } else { 0 }, 1);
            push_bits(&mut bits, value, (width * 8 - 1) as usize);
        }
    }

    finalize_payload(&bits)
}

pub fn finalize_payload(bits: &[u8]) -> Vec<u8> {
    let bit_count = bits.len();
    let byte_count = bit_count.div_ceil(8);
    let mut payload = vec![0u8; byte_count];
    for (i, &b) in bits.iter().enumerate() {
        if b == 1 {
            payload[i >> 3] |= 1 << (7 - (i & 7));
        }
    }
    let mut body = Vec::with_capacity(2 + byte_count);
    body.push((bit_count >> 8) as u8);
    body.push((bit_count & 0xFF) as u8);
    body.extend_from_slice(&payload);
    body
}

#[inline]
pub fn apply_tick(q: &mut Quote, tick: &RawTick, min_tick_scaled: i64) {
    match tick.tick_type {
        tick_decoder::O_BID_PRICE => q.bid = tick.magnitude * min_tick_scaled,
        tick_decoder::O_ASK_PRICE => q.ask = tick.magnitude * min_tick_scaled,
        tick_decoder::O_LAST_PRICE => q.last = tick.magnitude * min_tick_scaled,
        tick_decoder::O_HIGH_PRICE => q.high = tick.magnitude * min_tick_scaled,
        tick_decoder::O_LOW_PRICE => q.low = tick.magnitude * min_tick_scaled,
        tick_decoder::O_OPEN_PRICE => q.open = tick.magnitude * min_tick_scaled,
        tick_decoder::O_CLOSE_PRICE => q.close = tick.magnitude * min_tick_scaled,
        tick_decoder::O_BID_SIZE => q.bid_size = ibx::types::qty_from_wire(tick.magnitude),
        tick_decoder::O_ASK_SIZE => q.ask_size = ibx::types::qty_from_wire(tick.magnitude),
        tick_decoder::O_LAST_SIZE => q.last_size = ibx::types::qty_from_wire(tick.magnitude),
        tick_decoder::O_VOLUME => q.volume = ibx::types::qty_from_wire(tick.magnitude),
        tick_decoder::O_TS_BASE if tick.magnitude > 1_000_000_000 => {
            q.timestamp_ns = (tick.magnitude as u64).saturating_mul(1_000_000_000);
        }
        _ => {}
    }
}
