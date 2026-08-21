//! Whether a venue is trading a contract, or has stopped.
//!
//! A halt changes what every other tick means: the prices standing are the ones
//! from before it stopped, not a market anyone can deal on. A caller told a
//! contract is trading when it is not will price against a book that is not
//! there.
//!
//! The venue states it as its own tick, whose body is three big-endian 32-bit
//! integers and nothing else — a status mask, a timestamp, and an index naming
//! one status. The mask is what says whether trading has stopped; the index is
//! a weaker label that can say nothing while the mask says halted.

/// One thing a venue can be doing with a contract.
///
/// The values are the venue's. `None` sits at 16 rather than 4, which
/// matters more than it looks: each status's mask is `1 << index`, so a `None`
/// at 4 would carry mask 4 — the same mask as a volatility halt. "No status
/// available" and "halted for volatility" would have been one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradingStatus {
    /// The venue is trading.
    ExchangeOpen,
    /// A regulator stopped it.
    RegulatoryHalt,
    /// The venue stopped it because the price moved too far.
    VolatilityHalt,
    /// Short selling is restricted.
    ShortSaleRestriction,
    /// The venue stated no status.
    None,
}

impl TradingStatus {
    /// The index the venue names this status by.
    pub fn index(self) -> u32 {
        match self {
            Self::ExchangeOpen => 0,
            Self::RegulatoryHalt => 1,
            Self::VolatilityHalt => 2,
            Self::ShortSaleRestriction => 3,
            Self::None => 16,
        }
    }

    /// The bit this status sets in the mask.
    pub fn mask(self) -> u32 {
        1 << self.index()
    }

    /// The status an index names, or `None` where it names nothing.
    pub fn from_index(index: u32) -> Self {
        match index {
            0 => Self::ExchangeOpen,
            1 => Self::RegulatoryHalt,
            2 => Self::VolatilityHalt,
            3 => Self::ShortSaleRestriction,
            _ => Self::None,
        }
    }
}

/// What the venue said about trading in a contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExchangeTradingStatus {
    /// Every status in force at once. More than one can be.
    pub mask: u32,
    /// Timestamp the venue put on this record.
    ///
    /// Kept as stated. Its unit is not established: it is a signed 32-bit
    /// integer, so epoch milliseconds cannot fit, and no reader of this message
    /// settles which of the remaining readings is right. Handing it over as a
    /// time would invent one.
    pub stamp: i32,
    /// The one status the venue named, which can be nothing while the mask
    /// says trading has stopped.
    pub named: TradingStatus,
}

impl ExchangeTradingStatus {
    /// Whether trading has stopped.
    ///
    /// The mask alone determines this: the two halt bits are tested against
    /// the mask and nothing else is read. Whether a status was named is a
    /// separate question — see [`Self::named_a_status`].
    ///
    /// Requiring a name as well gives a different answer: a venue that stops
    /// trading and names nothing sets a halt bit and leaves the index at 16,
    /// and that contract reports as trading. Reading the index instead has the
    /// opposite fault — a live
    /// American listing states, in the premarket, a mask of nothing and an
    /// index of one, which as an index alone reads as a regulatory halt.
    pub fn is_halted(self) -> bool {
        let halt_bits =
            TradingStatus::RegulatoryHalt.mask() | TradingStatus::VolatilityHalt.mask();
        self.mask & halt_bits != 0
    }

    /// Whether the venue named a status at all, which it need not have done.
    pub fn named_a_status(self) -> bool {
        self.named != TradingStatus::None
    }

    /// Whether the venue is restricting short sales in this contract.
    pub fn short_sales_restricted(self) -> bool {
        self.mask & TradingStatus::ShortSaleRestriction.mask() != 0
    }
}

/// Read the venue's trading-status record.
///
/// Three big-endian 32-bit integers. A body too short to hold them is refused
/// rather than read from as far as it goes: a partly-read status is a claim
/// about whether a market is open, made from bytes that were not the venue's.
///
/// A longer body is read for its three words and the rest left alone. The
/// venue sends a fourth word whose meaning is not established, so it is not
/// interpreted.
pub fn parse_trading_status(body: &[u8]) -> Option<ExchangeTradingStatus> {
    if body.len() < 12 {
        return None;
    }
    let word = |at: usize| i32::from_be_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
    Some(ExchangeTradingStatus {
        mask: word(0) as u32,
        stamp: word(4),
        named: TradingStatus::from_index(word(8) as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(mask: i32, stamp: i32, index: i32) -> Vec<u8> {
        let mut out = Vec::with_capacity(12);
        out.extend_from_slice(&mask.to_be_bytes());
        out.extend_from_slice(&stamp.to_be_bytes());
        out.extend_from_slice(&index.to_be_bytes());
        out
    }

    /// A venue that is trading says so, and nothing reads as halted.
    #[test]
    fn an_open_venue_is_not_halted() {
        let status = parse_trading_status(&body(0, 1_767_000_000, 0)).expect("twelve bytes parse");
        assert_eq!(status.named, TradingStatus::ExchangeOpen);
        assert!(!status.is_halted());
    }

    /// Either kind of halt stops trading, and both are read from the mask.
    #[test]
    fn either_kind_of_halt_reads_as_halted() {
        for halt in [TradingStatus::RegulatoryHalt, TradingStatus::VolatilityHalt] {
            let status = parse_trading_status(&body(halt.mask() as i32, 0, halt.index() as i32))
                .expect("twelve bytes parse");
            assert!(status.is_halted(), "{halt:?} did not read as halted");
        }
    }

    /// The mask says whether trading has stopped, on its own. A venue that
    /// halts a contract and names no status leaves the index at 16, and that
    /// contract is halted: the halt bits decide it, and the name is not read.
    #[test]
    fn a_halt_with_no_name_is_still_a_halt() {
        let status = parse_trading_status(&body(
            TradingStatus::RegulatoryHalt.mask() as i32,
            0,
            TradingStatus::None.index() as i32,
        ))
        .expect("twelve bytes parse");
        assert_eq!(status.named, TradingStatus::None);
        assert!(!status.named_a_status(), "the venue named nothing");
        assert!(status.is_halted(), "and the mask says trading has stopped");
    }

    /// "No status" sits at 16, so its mask is 65536. Placed at 4 it would carry
    /// mask 4 — the mask of a volatility halt — and a venue saying nothing
    /// would be indistinguishable from one that had stopped trading.
    #[test]
    fn no_status_cannot_be_mistaken_for_a_volatility_halt() {
        assert_eq!(TradingStatus::None.index(), 16);
        assert_eq!(TradingStatus::None.mask(), 65_536);
        assert_ne!(TradingStatus::None.mask(), TradingStatus::VolatilityHalt.mask());

        let nothing_said = parse_trading_status(&body(
            TradingStatus::None.mask() as i32,
            0,
            TradingStatus::None.index() as i32,
        ))
        .expect("twelve bytes parse");
        assert!(!nothing_said.is_halted(), "saying nothing is not a halt");
    }

    /// More than one status can be in force.
    #[test]
    fn several_statuses_can_hold_at_once() {
        let both = TradingStatus::VolatilityHalt.mask() | TradingStatus::ShortSaleRestriction.mask();
        let status = parse_trading_status(&body(both as i32, 0, 2)).expect("twelve bytes parse");
        assert!(status.is_halted());
        assert!(status.short_sales_restricted());
    }

    /// A body too short to hold the three is refused rather than read as far
    /// as it goes. A partly-read status is a claim about whether a market is
    /// open, made from bytes that were not the venue's.
    #[test]
    fn a_body_too_short_for_the_three_is_refused() {
        for len in [0usize, 4, 8, 11] {
            assert!(parse_trading_status(&vec![0u8; len]).is_none(), "len {len} was read");
        }
    }

    /// The venue sends four words where three are read. The fourth is left
    /// alone rather than given a meaning nothing supports.
    #[test]
    fn a_longer_body_is_read_for_its_three() {
        let mut sixteen = body(0, 1_767_000_000, 0);
        sixteen.extend_from_slice(&7i32.to_be_bytes());
        let status = parse_trading_status(&sixteen).expect("the three are there");
        assert_eq!(status.named, TradingStatus::ExchangeOpen);
        assert_eq!(status.stamp, 1_767_000_000);
        assert!(!status.is_halted());
    }

    /// What a live American listing states in the premarket: no bits set and
    /// an index of one. Going by the index alone it reads as a regulatory
    /// halt, and every such contract would be reported halted while trading
    /// normally.
    #[test]
    fn a_premarket_listing_is_not_reported_halted() {
        let mut wire = body(0, 0, 1);
        wire.extend_from_slice(&0i32.to_be_bytes());
        let status = parse_trading_status(&wire).expect("the venue's bytes");
        assert_eq!(status.named, TradingStatus::RegulatoryHalt, "the index says so on its own");
        assert!(!status.is_halted(), "and the mask does not, so trading has not stopped");
    }

    /// The stamp is kept exactly as stated and not turned into a time.
    #[test]
    fn the_stamp_is_kept_as_stated() {
        let status = parse_trading_status(&body(0, 1_767_000_000, 0)).expect("twelve bytes parse");
        assert_eq!(status.stamp, 1_767_000_000);
    }
}
