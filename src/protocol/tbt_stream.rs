//! Every trade and every quote change, as the venue packs them.
//!
//! A frame carries one subscription's id, one timestamp, and then records until
//! the bits run out. There is no marker between records and no length in front
//! of one: a record ends when the fields its kind has are read, and each field
//! ends itself — a variable-length integer by its continuation bit, a string by
//! its own length.
//!
//! A price is not sent. What is sent is how far it moved, in whole ticks of the
//! contract's own smallest increment, added to where the price already was.
//! That running price starts at zero when a subscription opens, so the first
//! record's move is the whole price rather than a small step — a decoder that
//! expects a small step rejects the first record of every subscription.

/// Reads the bits of a frame, in the order the venue wrote them.
///
/// Every read is from the same position, so a field read wrongly does not
/// return a wrong value and carry on: it leaves the position in the middle of
/// the next field, and everything after it is nonsense. That is why a kind's
/// field count is followed exactly rather than guessed at.
pub struct Bits<'a> {
    bytes: &'a [u8],
    /// Position in bits, not bytes.
    at: usize,
}

impl<'a> Bits<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// How many bits are left unread.
    pub fn remaining(&self) -> usize {
        (self.bytes.len() * 8).saturating_sub(self.at)
    }

    fn bit(&mut self) -> Option<u8> {
        let byte = self.bytes.get(self.at / 8)?;
        let shift = 7 - (self.at % 8);
        self.at += 1;
        Some((byte >> shift) & 1)
    }

    /// One octet, which need not be byte-aligned.
    fn octet(&mut self) -> Option<u8> {
        let mut out = 0u8;
        for _ in 0..8 {
            out = (out << 1) | self.bit()?;
        }
        Some(out)
    }

    /// A whole number, seven bits at a time, most significant first, ending on
    /// the first octet whose top bit is clear.
    pub fn unsigned(&mut self) -> Option<u64> {
        let mut acc: u64 = 0;
        loop {
            let octet = self.octet()?;
            acc = (acc << 7) | u64::from(octet & 0x7F);
            if octet & 0x80 == 0 {
                return Some(acc);
            }
        }
    }

    /// A whole number that may be negative.
    ///
    /// The sign is bit six of the leading octet, and it is not removed from the
    /// value: it seeds the accumulator with all ones and then stays put, which
    /// is ordinary two's-complement sign extension seven bits at a time. Masking
    /// it away as though it were a flag makes every negative move wrong.
    pub fn signed(&mut self) -> Option<i64> {
        let first = self.octet()?;
        let mut acc: i64 = if first & 0x40 != 0 { -1 } else { 0 };
        acc = (acc << 7) | i64::from(first & 0x7F);
        if first & 0x80 == 0 {
            return Some(acc);
        }
        loop {
            let octet = self.octet()?;
            acc = (acc << 7) | i64::from(octet & 0x7F);
            if octet & 0x80 == 0 {
                return Some(acc);
            }
        }
    }

    /// A string, one character per octet, as long as its own length says.
    pub fn text(&mut self) -> Option<String> {
        let len = self.unsigned()?;
        let mut out = String::with_capacity(len as usize);
        for _ in 0..len {
            out.push(self.octet()? as char);
        }
        Some(out)
    }
}

/// Which kind of record a subscription carries, and how many fields one has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TbtKind {
    Last,
    AllLast,
    BidAsk,
    MidPoint,
}

impl TbtKind {
    /// A record's field count. The venue writes exactly this many and the next
    /// record starts straight after, so reading a different number desynchronises
    /// everything that follows rather than losing one record.
    pub fn fields(self) -> usize {
        match self {
            Self::Last | Self::AllLast | Self::BidAsk => 5,
            Self::MidPoint => 2,
        }
    }
}

/// What the venue said about a trade.
#[derive(Debug, Clone, PartialEq)]
pub struct TbtTradeRecord {
    pub price: f64,
    pub size: u64,
    /// The venue may still revise this print.
    pub past_limit: bool,
    /// The print did not go to the tape.
    pub unreported: bool,
    pub exchange: String,
    pub conditions: String,
}

/// What the venue said about the top of the book.
#[derive(Debug, Clone, PartialEq)]
pub struct TbtQuoteRecord {
    pub bid: f64,
    pub ask: f64,
    pub bid_size: u64,
    pub ask_size: u64,
    pub bid_past_low: bool,
    pub ask_past_high: bool,
}

/// One record.
#[derive(Debug, Clone, PartialEq)]
pub enum TbtRecord {
    Trade(TbtTradeRecord),
    Quote(TbtQuoteRecord),
    MidPoint { price: f64 },
}

/// Where a subscription's prices have got to.
///
/// Kept for the life of a subscription and never reset between records: a move
/// is measured from the last price, not from nothing. Held as a whole number of
/// ticks rather than as a price, so a session's worth of additions cannot drift
/// the way repeatedly adding fractions would.
#[derive(Debug, Default, Clone, Copy)]
pub struct RunningPrice {
    last_ticks: i64,
    bid_ticks: i64,
    ask_ticks: i64,
    mid_ticks: i64,
}

/// A frame: one subscription, one time, and the records that share them.
#[derive(Debug, Clone, PartialEq)]
pub struct TbtFrame {
    pub ticker_id: u64,
    /// The venue states seconds; every record in the frame shares this one.
    pub timestamp_ms: u64,
    pub records: Vec<TbtRecord>,
}

/// Read every record in a frame.
///
/// `min_tick` is the contract's own smallest price increment, which is what a
/// move is counted in. `running` carries the prices forward and must be the
/// same one across every frame of a subscription.
///
/// Stops at the first record it cannot read whole rather than guessing at the
/// rest: once a field is misread the position is inside the next one, and
/// everything after would be invented.
pub fn decode_frame(
    body: &[u8],
    kind: TbtKind,
    min_tick: f64,
    running: &mut RunningPrice,
) -> Option<TbtFrame> {
    let mut bits = Bits::new(body);
    let ticker_id = bits.unsigned()?;
    let seconds = bits.unsigned()?;

    let mut records = Vec::new();
    // A record needs at least one octet. Anything less is the frame's padding,
    // not a record that failed to read.
    while bits.remaining() >= 8 {
        match read_record(&mut bits, kind, min_tick, running) {
            Some(record) => records.push(record),
            None => break,
        }
    }

    Some(TbtFrame {
        ticker_id,
        timestamp_ms: seconds.saturating_mul(1000),
        records,
    })
}

/// A size, which the venue may state in one number or in two.
///
/// The two-number form sends the low half first. Reading one number where it
/// sent two leaves the second half in the stream and every field after it is
/// read from the wrong place — which is worse than a wrong size, because it is
/// a wrong everything.
fn size(bits: &mut Bits<'_>, extended: bool) -> Option<u64> {
    if extended {
        let low = bits.unsigned()?;
        let high = bits.unsigned()?;
        Some(low | (high << 32))
    } else {
        bits.unsigned()
    }
}

fn read_record(
    bits: &mut Bits<'_>,
    kind: TbtKind,
    min_tick: f64,
    running: &mut RunningPrice,
) -> Option<TbtRecord> {
    // Every kind opens with a price move.
    let first_move = bits.signed()?;

    match kind {
        TbtKind::Last | TbtKind::AllLast => {
            running.last_ticks = running.last_ticks.checked_add(first_move)?;
            let price = running.last_ticks as f64 * min_tick;
            let plain_size = bits.unsigned()?;
            let flags = bits.unsigned()?;
            // Bit five says the size is stated again, at greater width; bit
            // four says that restatement comes in two numbers.
            let size = if flags & (1 << 5) != 0 {
                size(bits, flags & (1 << 4) != 0)?
            } else {
                plain_size
            };
            Some(TbtRecord::Trade(TbtTradeRecord {
                price,
                size,
                past_limit: flags & 1 != 0,
                unreported: flags & (1 << 1) != 0,
                exchange: bits.text()?,
                conditions: bits.text()?,
            }))
        }
        TbtKind::BidAsk => {
            let ask_move = bits.signed()?;
            running.bid_ticks = running.bid_ticks.checked_add(first_move)?;
            running.ask_ticks = running.ask_ticks.checked_add(ask_move)?;
            let plain_bid = bits.unsigned()?;
            let plain_ask = bits.unsigned()?;
            let flags = bits.unsigned()?;
            let bid_size = if flags & (1 << 2) != 0 { size(bits, true)? } else { plain_bid };
            let ask_size = if flags & (1 << 3) != 0 { size(bits, true)? } else { plain_ask };
            Some(TbtRecord::Quote(TbtQuoteRecord {
                bid: running.bid_ticks as f64 * min_tick,
                ask: running.ask_ticks as f64 * min_tick,
                bid_size,
                ask_size,
                bid_past_low: flags & 1 != 0,
                ask_past_high: flags & (1 << 1) != 0,
            }))
        }
        TbtKind::MidPoint => {
            running.mid_ticks = running.mid_ticks.checked_add(first_move)?;
            // Two fields: the move, and a flag word. No size, no venue, no text.
            let _flags = bits.unsigned()?;
            Some(TbtRecord::MidPoint {
                price: running.mid_ticks as f64 * min_tick,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write the venue's own encodings, so a test frame is built the way a
    /// frame is built rather than by hand.
    #[derive(Default)]
    struct Writer {
        out: Vec<u8>,
    }

    impl Writer {
        fn unsigned(&mut self, mut v: u64) -> &mut Self {
            let mut groups = Vec::new();
            loop {
                groups.push((v & 0x7F) as u8);
                v >>= 7;
                if v == 0 {
                    break;
                }
            }
            groups.reverse();
            let last = groups.len() - 1;
            for (i, g) in groups.iter().enumerate() {
                self.out.push(if i == last { *g } else { g | 0x80 });
            }
            self
        }

        fn signed(&mut self, v: i64) -> &mut Self {
            // Seven bits at a time, most significant first, stopping once what
            // is left is only sign.
            let mut groups = Vec::new();
            // The highest seven-bit group of a 64-bit value.
            let mut shift: i32 = 63;
            let mut started = false;
            while shift >= 0 {
                let g = ((v >> shift) & 0x7F) as u8;
                let sign_only = if v < 0 { g == 0x7F } else { g == 0 };
                if started || !sign_only || shift == 0 {
                    started = true;
                    groups.push(g);
                }
                shift -= 7;
            }
            // The leading group must carry the sign in bit six.
            if let Some(first) = groups.first().copied() {
                let sign_set = first & 0x40 != 0;
                if (v < 0) != sign_set {
                    groups.insert(0, if v < 0 { 0x7F } else { 0x00 });
                }
            }
            let last = groups.len() - 1;
            for (i, g) in groups.iter().enumerate() {
                self.out.push(if i == last { *g } else { g | 0x80 });
            }
            self
        }

        fn text(&mut self, s: &str) -> &mut Self {
            self.unsigned(s.len() as u64);
            self.out.extend_from_slice(s.as_bytes());
            self
        }
    }

    /// A number that may be negative survives being written and read back,
    /// including the boundaries where the sign bit sits.
    #[test]
    fn a_move_that_may_be_negative_reads_back_as_itself() {
        for v in [0i64, 1, -1, 63, -63, 64, -64, 8191, -8191, 1_000_000, -1_000_000] {
            let mut w = Writer::default();
            w.signed(v);
            let mut bits = Bits::new(&w.out);
            assert_eq!(bits.signed(), Some(v), "{v} did not read back");
        }
    }

    /// The first record of a subscription states the whole price, because the
    /// running price starts at nothing. A decoder expecting a small step
    /// rejects the first record of every subscription.
    #[test]
    fn the_first_record_carries_the_whole_price() {
        let mut w = Writer::default();
        w.unsigned(7).unsigned(1_767_000_000);
        // 40000 ticks of a penny is 400.00.
        w.signed(40_000).unsigned(100).unsigned(0).text("ARCA").text("");

        let mut running = RunningPrice::default();
        let frame = decode_frame(&w.out, TbtKind::AllLast, 0.01, &mut running).expect("a frame");
        assert_eq!(frame.ticker_id, 7);
        assert_eq!(frame.timestamp_ms, 1_767_000_000_000);
        match &frame.records[0] {
            TbtRecord::Trade(t) => {
                assert!((t.price - 400.0).abs() < 1e-9, "price was {}", t.price);
                assert_eq!(t.size, 100);
                assert_eq!(t.exchange, "ARCA");
            }
            other => panic!("expected a trade, got {other:?}"),
        }
    }

    /// A move is measured from where the price already was, and the running
    /// price carries across records and across frames.
    #[test]
    fn a_move_is_measured_from_the_last_price() {
        let mut running = RunningPrice::default();

        let mut first = Writer::default();
        first.unsigned(1).unsigned(0);
        first.signed(40_000).unsigned(1).unsigned(0).text("").text("");
        decode_frame(&first.out, TbtKind::AllLast, 0.01, &mut running).expect("a frame");

        // A later frame moves down three ticks.
        let mut second = Writer::default();
        second.unsigned(1).unsigned(0);
        second.signed(-3).unsigned(1).unsigned(0).text("").text("");
        let frame = decode_frame(&second.out, TbtKind::AllLast, 0.01, &mut running).expect("a frame");
        match &frame.records[0] {
            TbtRecord::Trade(t) => assert!((t.price - 399.97).abs() < 1e-9, "price was {}", t.price),
            other => panic!("expected a trade, got {other:?}"),
        }
    }

    /// Several records share one frame, with no marker between them and no
    /// timestamp of their own.
    #[test]
    fn records_run_on_with_nothing_between_them() {
        let mut w = Writer::default();
        w.unsigned(1).unsigned(1_700_000_000);
        for _ in 0..3 {
            w.signed(1).unsigned(5).unsigned(0).text("Q").text("");
        }
        let mut running = RunningPrice::default();
        let frame = decode_frame(&w.out, TbtKind::AllLast, 0.01, &mut running).expect("a frame");
        assert_eq!(frame.records.len(), 3, "three records share the frame");
        // Every record carries the frame's time, not one of its own.
        assert_eq!(frame.timestamp_ms, 1_700_000_000_000);
    }

    /// A size stated again at greater width is read from the two numbers the
    /// venue sent, low half first. Reading one leaves the other in the stream
    /// and everything after it comes from the wrong place.
    #[test]
    fn a_size_stated_in_two_halves_is_read_as_one_number() {
        let mut w = Writer::default();
        w.unsigned(1).unsigned(0);
        // Bit five says restated, bit four says in two halves.
        w.signed(1).unsigned(0).unsigned((1 << 5) | (1 << 4));
        w.unsigned(7).unsigned(1); // low, then high
        w.text("Q").text("");
        let mut running = RunningPrice::default();
        let frame = decode_frame(&w.out, TbtKind::AllLast, 0.01, &mut running).expect("a frame");
        match &frame.records[0] {
            TbtRecord::Trade(t) => {
                assert_eq!(t.size, 7 | (1u64 << 32));
                assert_eq!(t.exchange, "Q", "the fields after the size still line up");
            }
            other => panic!("expected a trade, got {other:?}"),
        }
    }

    /// The flags say what the venue said about a print.
    #[test]
    fn a_print_the_venue_may_revise_says_so() {
        let mut w = Writer::default();
        w.unsigned(1).unsigned(0);
        w.signed(1).unsigned(1).unsigned(0b11).text("").text("");
        let mut running = RunningPrice::default();
        let frame = decode_frame(&w.out, TbtKind::AllLast, 0.01, &mut running).expect("a frame");
        match &frame.records[0] {
            TbtRecord::Trade(t) => {
                assert!(t.past_limit);
                assert!(t.unreported);
            }
            other => panic!("expected a trade, got {other:?}"),
        }
    }

    /// A quote states two moves, one for each side.
    #[test]
    fn a_quote_moves_each_side_on_its_own() {
        let mut w = Writer::default();
        w.unsigned(1).unsigned(0);
        w.signed(40_000).signed(40_001).unsigned(300).unsigned(200).unsigned(0);
        let mut running = RunningPrice::default();
        let frame = decode_frame(&w.out, TbtKind::BidAsk, 0.01, &mut running).expect("a frame");
        match &frame.records[0] {
            TbtRecord::Quote(q) => {
                assert!((q.bid - 400.00).abs() < 1e-9, "bid was {}", q.bid);
                assert!((q.ask - 400.01).abs() < 1e-9, "ask was {}", q.ask);
                assert_eq!((q.bid_size, q.ask_size), (300, 200));
            }
            other => panic!("expected a quote, got {other:?}"),
        }
    }

    /// A midpoint has a price and nothing else — no size, no venue, no text.
    #[test]
    fn a_midpoint_carries_only_a_price() {
        let mut w = Writer::default();
        w.unsigned(1).unsigned(0);
        w.signed(40_000).unsigned(0);
        let mut running = RunningPrice::default();
        let frame = decode_frame(&w.out, TbtKind::MidPoint, 0.01, &mut running).expect("a frame");
        assert_eq!(frame.records.len(), 1);
        match &frame.records[0] {
            TbtRecord::MidPoint { price } => assert!((price - 400.0).abs() < 1e-9),
            other => panic!("expected a midpoint, got {other:?}"),
        }
    }

    /// A frame that ends inside a record keeps what it read whole and stops,
    /// rather than inventing the rest.
    #[test]
    fn a_record_that_does_not_finish_is_not_guessed_at() {
        let mut w = Writer::default();
        w.unsigned(1).unsigned(0);
        w.signed(1).unsigned(5).unsigned(0).text("Q").text("");
        w.signed(1).unsigned(5); // cut off mid-record
        let mut running = RunningPrice::default();
        let frame = decode_frame(&w.out, TbtKind::AllLast, 0.01, &mut running).expect("a frame");
        assert_eq!(frame.records.len(), 1, "only the whole record is kept");
    }

    /// Every kind states how many fields it has, and a decoder that reads a
    /// different number desynchronises everything after it.
    #[test]
    fn each_kind_states_its_field_count() {
        assert_eq!(TbtKind::Last.fields(), 5);
        assert_eq!(TbtKind::AllLast.fields(), 5);
        assert_eq!(TbtKind::BidAsk.fields(), 5);
        assert_eq!(TbtKind::MidPoint.fields(), 2);
    }
}
