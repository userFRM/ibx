//! Binary tick decoder for market data messages.
//!
//! Decodes bid/ask/last/size/volume fields from IB's proprietary binary format.
//! Also includes VLQ and hi-bit string decoders for 35=E tick-by-tick data,
//! and the RTBAR decoder for 35=G real-time bar data.

/// MSB-first bit-level reader for 8=O 35=P binary tick data.
pub struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
    total_bits: usize,
}

impl<'a> BitReader<'a> {
    /// Read from the start of these bytes.
    pub fn new(data: &'a [u8], total_bits: usize) -> Self {
        let max_bits = data.len() * 8;
        let total_bits = if total_bits > 0 {
            total_bits.min(max_bits)
        } else {
            max_bits
        };
        Self {
            data,
            bit_pos: 0,
            total_bits,
        }
    }

    /// How many bytes are left unread.
    pub fn remaining(&self) -> usize {
        self.total_bits.saturating_sub(self.bit_pos)
    }

    /// Advance past `n` bits without decoding them. Used to step over a field
    /// this decoder cannot represent, so the rest of the message still decodes.
    ///
    /// Compares against `remaining`, which saturates, so a width read off the
    /// wire cannot overflow the position.
    #[inline]
    fn skip(&mut self, n: usize) -> bool {
        if n > self.remaining() {
            return false;
        }
        self.bit_pos += n;
        true
    }

    /// Read n bits as unsigned integer (MSB first).
    /// Uses word-aligned reads for performance (1-3 ops instead of n iterations).
    #[inline]
    pub fn read_unsigned(&mut self, n: usize) -> Option<u64> {
        if n == 0 {
            return Some(0);
        }
        if n > 64 || self.bit_pos + n > self.total_bits {
            return None;
        }
        let byte_idx = self.bit_pos >> 3;
        let bit_offset = self.bit_pos & 7;
        self.bit_pos += n;

        // Total bits taken from the byte stream: bit_offset + n.
        // If <= 64, one u64 load suffices. If > 64 (cross-word), load two words.
        let needed = bit_offset + n;

        let remaining_bytes = self.data.len() - byte_idx;

        if needed <= 64 {
            // Fast path: single word load
            let word = if remaining_bytes >= 8 {
                u64::from_be_bytes(self.data[byte_idx..byte_idx + 8].try_into().unwrap())
            } else {
                let mut buf = [0u8; 8];
                buf[..remaining_bytes].copy_from_slice(&self.data[byte_idx..]);
                u64::from_be_bytes(buf)
            };
            let result = (word << bit_offset) >> (64 - n);
            Some(result)
        } else {
            // Cross-word: need bits from two consecutive u64s
            let load_be = |off: usize| -> u64 {
                let rem = self.data.len().saturating_sub(off);
                if rem >= 8 {
                    u64::from_be_bytes(self.data[off..off + 8].try_into().unwrap())
                } else if rem > 0 {
                    let mut buf = [0u8; 8];
                    buf[..rem].copy_from_slice(&self.data[off..off + rem]);
                    u64::from_be_bytes(buf)
                } else {
                    0
                }
            };
            let hi = load_be(byte_idx);
            let lo = load_be(byte_idx + 8);
            // Combine: take (64 - bit_offset) bits from hi, then (n - (64 -
            // bit_offset)) from lo
            let hi_bits = 64 - bit_offset; // bits available in hi after discarding offset
            let lo_bits = n - hi_bits;
            let result = ((hi << bit_offset) >> (64 - n)) | (lo >> (64 - lo_bits));
            Some(result)
        }
    }
}

// 8=O binary tick type IDs (what comes off the wire in 35=P)
/// Tick type 0 on the wire: the bid price.
pub const O_BID_PRICE: u64 = 0;
/// Tick type 1 on the wire: the ask price.
pub const O_ASK_PRICE: u64 = 1;
/// Tick type 2 on the wire: the last price.
pub const O_LAST_PRICE: u64 = 2;
/// Tick type 4 on the wire: the bid size.
pub const O_BID_SIZE: u64 = 4;
/// Tick type 5 on the wire: the ask size.
pub const O_ASK_SIZE: u64 = 5;
/// Size of the trade this record reports, not the day's volume. Measured on a
/// live front-month future: 120 samples, values 1..6, decreasing 60 times in
/// 119 transitions — a running total cannot decrease.
pub const O_LAST_SIZE: u64 = 6;
/// Session high — 28177.25 on the wire against a daily-bar high of exactly
/// 28177.25 for the same contract and session.
pub const O_HIGH_PRICE: u64 = 8;
/// Tick type 9 on the wire: the low price.
pub const O_LOW_PRICE: u64 = 9;
/// Cumulative session volume. Measured: 228 samples, zero decreases across 227
/// transitions, increments of 1..6 matching the per-trade sizes on type 6.
pub const O_VOLUME: u64 = 10;
/// Trade timestamp, in two parts: 20 carries a Unix-seconds base (measured
/// 1785325554) and 21 an offset advancing by exactly 1 per wall-clock second
/// across 164 samples. Neither was decoded before.
pub const O_TS_BASE: u64 = 20;
/// Tick type 21 on the wire: the ts offset.
pub const O_TS_OFFSET: u64 = 21;
/// Tick type 13 on the wire: the last exch.
pub const O_LAST_EXCH: u64 = 13;
/// Tick type 16 on the wire: the bid exch.
pub const O_BID_EXCH: u64 = 16;
/// Tick type 17 on the wire: the ask exch.
pub const O_ASK_EXCH: u64 = 17;
/// UNVERIFIED. Named for a halt on no evidence — unlike its neighbours, which
/// cite measurements against daily bars and wall-clock samples.
///
/// The venue does state a halt, but the counterpart reads it elsewhere: as a
/// generic tick under id 437, whose payload is three big-endian 32-bit ints
/// (a status bitmask, a timestamp, a status index) rather than anything in this
/// stream. Its statuses are named in an enum that survived obfuscation:
/// 0 exchange open, 1 regulatory halt, 2 volatility halt, 3 short-sale
/// restriction, 4 no status available — and "halted" is bit 0 or bit 1 of the
/// mask, not an index equal to one.
///
/// So this opcode is something else, and nothing may report a halt from it
/// until the real path is read. A wrong halt is worse than no halt: a caller
/// told a contract is trading when it is not will price against a book that is
/// not there.
pub const O_UNVERIFIED_18: u64 = 18;
/// Previous session's close. Settled against the authoritative daily bars for
/// the same contract: the wire carried 27922.00 while the current session's
/// bar closed at 27913.75 and the prior session's closed at exactly 27922.00.
pub const O_CLOSE_PRICE: u64 = 3;
/// Current session's open — 27962.25 on the wire against a daily-bar open of
/// exactly 27962.25.
pub const O_OPEN_PRICE: u64 = 22;
/// Type 23 was read as the last-trade timestamp and no longer is: the
/// timestamp arrives on 20, and captures show 23 carrying something else on
/// this feed. Retained under its old name until that something is identified.
pub const O_LAST_TS: u64 = 23;

// Type 12 was read as the last size, which type 6 carries. Left undecoded
// rather than remapped, since nothing in the captures says what it is.

/// A single decoded tick from a 35=P message.
#[derive(Debug, Clone, Copy)]
pub struct RawTick {
    /// The venue's own number for the subscription.
    pub server_tag: u32,
    /// Which tick this is, as the wire numbers it.
    pub tick_type: u64,
    /// Its value, before the contract's own scale is applied.
    pub magnitude: i64,
}

/// Decode all ticks from a 35=P binary payload.
///
/// `body` is the raw message body after stripping FIX framing and HMAC signature.
/// Returns a list of raw ticks with server_tag, tick_type, and signed magnitude.
pub fn decode_ticks_35p(body: &[u8]) -> Vec<RawTick> {
    let mut ticks = Vec::with_capacity(8);
    decode_ticks_35p_into(body, &mut ticks);
    ticks
}

/// Decode ticks into a caller-supplied buffer (avoids heap allocation on hot path).
pub fn decode_ticks_35p_into(body: &[u8], ticks: &mut Vec<RawTick>) {
    ticks.clear();
    if body.len() < 4 {
        return;
    }

    let bit_count = ((body[0] as usize) << 8) | (body[1] as usize);
    let payload = &body[2..];
    let mut reader = BitReader::new(payload, bit_count);

    while reader.remaining() > 32 {
        let cont = match reader.read_unsigned(1) {
            Some(v) => v,
            None => break,
        };
        let _ = cont; // continuation flag, not used in decoding
        let server_tag = match reader.read_unsigned(31) {
            Some(v) => v as u32,
            None => break,
        };

        let mut has_more = 1u64;
        while has_more == 1 && reader.remaining() >= 8 {
            let tick_type;
            let byte_width;

            let raw_tick_type = match reader.read_unsigned(5) {
                Some(v) => v,
                None => break,
            };
            has_more = match reader.read_unsigned(1) {
                Some(v) => v,
                None => break,
            };
            let raw_width = match reader.read_unsigned(2) {
                Some(v) => v + 1,
                None => break,
            };

            if raw_tick_type == 31 {
                // Extended format
                if reader.remaining() < 16 {
                    return;
                }
                tick_type = match reader.read_unsigned(8) {
                    Some(v) => v,
                    None => return,
                };
                byte_width = match reader.read_unsigned(8) {
                    Some(v) => v,
                    None => return,
                };
            } else {
                tick_type = raw_tick_type;
                byte_width = raw_width;
            }

            let total_value_bits = (8 * byte_width) as usize;
            if reader.remaining() < total_value_bits {
                return;
            }

            // An extended entry states its width in a full byte, so it can name
            // a value wider than this decoder reads. That entry is lost either
            // way; abandoning the message threw away every tick after it as
            // well, including the other server tags in the same 35=P.
            // Stepping over it keeps the rest.
            if total_value_bits > 64 {
                log::debug!("35=P: skipping a {byte_width}-byte tick value");
                if !reader.skip(total_value_bits) {
                    return;
                }
                continue;
            }

            let sign = match reader.read_unsigned(1) {
                Some(v) => v,
                None => return,
            };
            let magnitude_unsigned = if total_value_bits > 1 {
                match reader.read_unsigned(total_value_bits - 1) {
                    Some(v) => v as i64,
                    None => return,
                }
            } else {
                0i64
            };

            let magnitude = if sign == 1 {
                -magnitude_unsigned
            } else {
                magnitude_unsigned
            };

            ticks.push(RawTick {
                server_tag,
                tick_type,
                magnitude,
            });
        }
    }
}

/// Read a VLQ-encoded unsigned integer (hi-bit terminated).
///
/// Bit7=1 means last byte. 7 data bits per byte, MSB first.
/// Returns (value, num_bytes).
pub fn read_vlq(data: &[u8], pos: usize) -> (u64, usize) {
    let mut val: u64 = 0;
    let mut n = 0usize;
    let mut p = pos;
    while p < data.len() {
        let b = data[p];
        val = (val << 7) | (b as u64 & 0x7F);
        n += 1;
        p += 1;
        if b & 0x80 != 0 {
            return (val, n);
        }
    }
    (val, n)
}

/// Convert VLQ value to signed (upper half of range = negative).
pub fn vlq_signed(val: u64, num_bytes: usize) -> i64 {
    // Two overflow traps, both reachable from the wire.
    //
    // read_vlq walks to the end of the buffer when no byte terminates the run,
    // so `7 * num_bytes` can exceed the width of the shift below. Nine groups
    // carry 63 bits, which is every value this encoding can represent, so a
    // longer run is malformed and is read at the full width rather than
    // shifting by more than the type allows.
    //
    // The sign correction itself also overflows at the top width: for a
    // well-formed nine-byte negative value, `val as i64 - (1i64 << 63)` is
    // outside i64. Doing the subtraction in u64 and reinterpreting gives the
    // same two's-complement answer at every width without the trap.
    let bits = (7 * num_bytes).clamp(1, 63);
    let half: u64 = 1 << (bits - 1);
    if val >= half {
        val.wrapping_sub(1u64 << bits) as i64
    } else {
        val as i64
    }
}

/// Read a high-bit terminated ASCII string.
///
/// Last character has bit7 set. Single 0x80 byte = empty string.
/// Returns (string, bytes_consumed).
pub fn read_hibit_str(data: &[u8], pos: usize) -> (String, usize) {
    let mut chars = Vec::new();
    let mut p = pos;
    while p < data.len() {
        let b = data[p];
        p += 1;
        if b & 0x80 != 0 {
            let ch = b & 0x7F;
            if ch != 0 {
                chars.push(ch as char);
            }
            return (chars.into_iter().collect(), p - pos);
        }
        chars.push(b as char);
    }
    (chars.into_iter().collect(), p - pos)
}

/// Marker bytes for tick-by-tick 35=E binary entries.
pub const TBT_MARKER_ALL_LAST: u8 = 0x81;
/// The byte that starts a bid ask record: 0x82.
pub const TBT_MARKER_BID_ASK: u8 = 0x82;

/// A decoded tick-by-tick entry from 35=E.
#[derive(Debug, Clone)]
pub enum TbtEntry {
    /// AllLast trade tick.
    Trade {
        /// When, as the record states it.
        timestamp: u64,
        /// How far the price moved from the running one, in cents.
        price_cents_delta: i64,
        /// How much.
        size: u64,
        /// Which venue.
        exchange: String,
        /// What the venue notes about the trade.
        conditions: String,
    },
    /// BidAsk quote tick.
    Quote {
        /// When, as the record states it.
        timestamp: u64,
        /// How far the bid moved.
        bid_cents_delta: i64,
        /// How far the ask moved.
        ask_cents_delta: i64,
        /// How much at the bid.
        bid_size: u64,
        /// How much at the ask.
        ask_size: u64,
    },
}

/// Decode tick-by-tick entries from a 35=E binary payload.
///
/// `body` is the raw message body after stripping FIX framing and HMAC.
/// Prices are signed VLQ deltas in cents from a running state (caller tracks).
/// Tick-by-tick records out of a `35=E` body.
///
/// The body opens with two bytes that are not a record and are not identified:
/// they track neither the body's length nor the number of records in it across
/// the payloads seen. Whatever they are, records begin at the first marker, so
/// the scan starts there rather than at byte zero. Reading from byte zero found
/// no marker and returned nothing, which is why a subscription that was
/// receiving several payloads a second delivered not one tick to a caller.
pub fn decode_ticks_35e(body: &[u8]) -> Vec<TbtEntry> {
    let mut entries = Vec::new();
    let mut pos = match body.iter().position(|b| matches!(*b, TBT_MARKER_ALL_LAST | TBT_MARKER_BID_ASK)) {
        Some(at) => at,
        None => return entries,
    };

    while pos < body.len() {
        let marker = body[pos];
        pos += 1;

        match marker {
            TBT_MARKER_ALL_LAST => {
                if pos >= body.len() { break; }
                let (ts, n) = read_vlq(body, pos);
                pos += n;
                if pos >= body.len() { break; }
                let (price_raw, n) = read_vlq(body, pos);
                let price_delta = vlq_signed(price_raw, n);
                pos += n;
                if pos >= body.len() { break; }
                let (attribs, n) = read_vlq(body, pos);
                let _ = attribs; // reserved
                pos += n;
                if pos >= body.len() { break; }
                let (size, n) = read_vlq(body, pos);
                pos += n;
                if pos >= body.len() { break; }
                let (exchange, n) = read_hibit_str(body, pos);
                pos += n;
                if pos >= body.len() { break; }
                let (conditions, n) = read_hibit_str(body, pos);
                pos += n;

                entries.push(TbtEntry::Trade {
                    timestamp: ts,
                    price_cents_delta: price_delta,
                    size,
                    exchange,
                    conditions,
                });
            }
            TBT_MARKER_BID_ASK => {
                if pos >= body.len() { break; }
                let (ts, n) = read_vlq(body, pos);
                pos += n;
                if pos >= body.len() { break; }
                let (bid_raw, n) = read_vlq(body, pos);
                let bid_delta = vlq_signed(bid_raw, n);
                pos += n;
                if pos >= body.len() { break; }
                let (ask_raw, n) = read_vlq(body, pos);
                let ask_delta = vlq_signed(ask_raw, n);
                pos += n;
                if pos >= body.len() { break; }
                let (attribs, n) = read_vlq(body, pos);
                let _ = attribs;
                pos += n;
                if pos >= body.len() { break; }
                let (bid_size, n) = read_vlq(body, pos);
                pos += n;
                if pos >= body.len() { break; }
                let (ask_size, n) = read_vlq(body, pos);
                pos += n;

                entries.push(TbtEntry::Quote {
                    timestamp: ts,
                    bid_cents_delta: bid_delta,
                    ask_cents_delta: ask_delta,
                    bid_size,
                    ask_size,
                });
            }
            // A byte that starts no record. The records are self-delimiting and
            // each begins with a marker, so the next one is found rather than
            // the rest of the payload abandoned.
            _ => match body[pos..].iter()
                .position(|b| matches!(*b, TBT_MARKER_ALL_LAST | TBT_MARKER_BID_ASK))
            {
                Some(at) => pos += at,
                None => break,
            },
        }
    }

    entries
}

#[cfg(test)]
mod tests;
