//! The tests for this module.
//!
//! One file per module, as `api/client` already does it. Each block below
//! reaches the code it tests through `super::super`, which is the module this
//! file belongs to.


/// The producer and the consumer have to agree on the units. This is the
/// disagreement that shipped: the decode path stored the wire magnitude
/// and every reader divided by `QTY_SCALE`, so one contract arrived as
/// 0.0001. Pinning both halves together is what catches it — either side
/// changing alone fails here.
#[test]
fn wire_quantity_survives_the_round_trip_through_qty_scale() {
    for wire in [0i64, 1, 2, 7, 500, 10_000, 1_000_000] {
        let stored = qty_from_wire(wire);
        let delivered = stored as f64 / QTY_SCALE as f64;
        assert_eq!(delivered, wire as f64, "wire quantity {wire} came back as {delivered}");
    }
}

#[test]
fn qty_from_wire_clamps_instead_of_wrapping() {
    // Server-supplied magnitude; a wrapped quantity would read as a
    // plausible negative size rather than an obvious ceiling.
    assert_eq!(qty_from_wire(i64::MAX), i64::MAX);
    assert_eq!(qty_from_wire(i64::MIN), i64::MIN);
    // Not a fixed point of the identity function, so this fails if the
    // conversion is dropped as well as if it wraps.
    assert_eq!(qty_from_wire(i64::MAX / 2), i64::MAX);
}

use super::*;
use std::mem;

// --- Quote layout ---

#[test]
fn quote_alignment_is_64() {
    assert_eq!(mem::align_of::<Quote>(), 64);
}

#[test]
fn quote_size_is_128() {
    // 11 × i64 (88) + 1 × u64 (8) = 96 bytes data, padded to 128 (2 cache lines)
    assert_eq!(mem::size_of::<Quote>(), 128);
}

#[test]
fn quote_is_copy() {
    let q = Quote::default();
    let q2 = q; // Copy
    assert_eq!(q.bid, q2.bid);
}

// --- Price fixed-point ---

#[test]
fn price_150_25() {
    let p: Price = 15_025 * (PRICE_SCALE / 100);
    assert_eq!(p, 15_025_000_000);
}

#[test]
fn price_to_float() {
    let p: Price = 15_025_000_000;
    let f = p as f64 / PRICE_SCALE as f64;
    assert!((f - 150.25).abs() < 1e-10);
}

#[test]
fn price_negative() {
    let p: Price = -500 * PRICE_SCALE;
    assert_eq!(p, -50_000_000_000);
}

// --- Qty fixed-point ---

#[test]
fn qty_100_shares() {
    let q: Qty = 100 * QTY_SCALE;
    assert_eq!(q as f64 / QTY_SCALE as f64, 100.0);
}

#[test]
fn qty_fractional() {
    // 0.5 shares (fractional shares)
    let q: Qty = QTY_SCALE / 2;
    assert_eq!(q as f64 / QTY_SCALE as f64, 0.5);
}

// --- OrderBuffer ---

#[test]
fn order_buffer_starts_empty() {
    let buf = OrderBuffer::new();
    assert!(buf.is_empty());
}

#[test]
fn order_buffer_push_and_drain() {
    let mut buf = OrderBuffer::new();
    buf.push(OrderRequest::SubmitEx {
        order_id: 1, instrument: 0, side: Side::Buy, qty: 100 * crate::types::QTY_SCALE,
        kind: OrderKind::Limit { price: 150 * PRICE_SCALE },
        tif: b'0', attrs: OrderAttrs::default(),
    });
    buf.push(OrderRequest::Cancel { order_id: 42 });
    assert!(!buf.is_empty());

    let drained: Vec<_> = buf.drain().collect();
    assert_eq!(drained.len(), 2);
    assert!(buf.is_empty());
}

#[test]
fn order_buffer_drain_reusable() {
    let mut buf = OrderBuffer::new();
    buf.push(OrderRequest::SubmitEx {
        order_id: 1, instrument: 0, side: Side::Sell, qty: 50 * crate::types::QTY_SCALE,
        kind: OrderKind::Market,
        tif: b'0', attrs: OrderAttrs::default(),
    });
    let _: Vec<_> = buf.drain().collect();
    assert!(buf.is_empty());

    // Can push again after drain
    buf.push(OrderRequest::CancelAll { instrument: 1 });
    assert!(!buf.is_empty());
}

// --- OrderRequest variants ---

#[test]
fn order_request_is_copy() {
    let req = OrderRequest::Modify {
        order_id: 1,
        price: 100 * PRICE_SCALE,
        qty: 200 * crate::types::QTY_SCALE,
        outside_rth: false,
        ord_type: 0,
        tif: 0,
        stop_price: 0,
    };
    let req2 = req.clone();
    match (req, req2) {
        (
            OrderRequest::Modify { order_id: a, .. },
            OrderRequest::Modify { order_id: b, .. },
        ) => assert_eq!(a, b),
        _ => panic!("should both be Modify"),
    }
}

// --- Quote field independence ---

#[test]
fn quote_default_all_zeros() {
    let q = Quote::default();
    assert_eq!(q.bid, 0);
    assert_eq!(q.ask, 0);
    assert_eq!(q.last, 0);
    assert_eq!(q.bid_size, 0);
    assert_eq!(q.ask_size, 0);
    assert_eq!(q.last_size, 0);
    assert_eq!(q.volume, 0);
    assert_eq!(q.open, 0);
    assert_eq!(q.high, 0);
    assert_eq!(q.low, 0);
    assert_eq!(q.close, 0);
    assert_eq!(q.timestamp_ns, 0);
}

#[test]
fn quote_field_independence() {
    let mut q = Quote { bid: 100 * PRICE_SCALE, ..Default::default() };
    assert_eq!(q.ask, 0); // other fields untouched
    assert_eq!(q.last, 0);
    q.ask = 101 * PRICE_SCALE;
    assert_eq!(q.bid, 100 * PRICE_SCALE); // bid unchanged
}

#[test]
fn quote_in_array_no_false_sharing() {
    // Two adjacent quotes should be on different cache lines
    let quotes = [Quote::default(); 4];
    let ptr0 = &quotes[0] as *const Quote as usize;
    let ptr1 = &quotes[1] as *const Quote as usize;
    // Each quote is 128 bytes (2 cache lines), so stride should be 128
    assert_eq!(ptr1 - ptr0, 128);
}

// --- Price edge cases ---

#[test]
fn price_zero() {
    let p: Price = 0;
    assert_eq!(p as f64 / PRICE_SCALE as f64, 0.0);
}

#[test]
fn price_one_cent() {
    let p: Price = PRICE_SCALE / 100; // $0.01
    let f = p as f64 / PRICE_SCALE as f64;
    assert!((f - 0.01).abs() < 1e-10);
}

#[test]
fn price_sub_penny() {
    // $0.0001 (minimum tick for some instruments)
    let p: Price = PRICE_SCALE / 10_000;
    assert_eq!(p, PRICE_SCALE / 10_000);
    let f = p as f64 / PRICE_SCALE as f64;
    assert!((f - 0.0001).abs() < 1e-12);
}

#[test]
fn price_large_value() {
    // $100,000.00 (like BRK.A)
    let p: Price = 100_000 * PRICE_SCALE;
    assert_eq!(p, 10_000_000_000_000);
    // Should be well within i64 range (max ~9.2 * 10^18)
    assert!(p < i64::MAX);
}

#[test]
fn price_max_representable() {
    // Maximum price: i64::MAX / PRICE_SCALE = ~92,233,720,368
    let max_price = i64::MAX / PRICE_SCALE;
    let p: Price = max_price * PRICE_SCALE;
    // Should not overflow
    assert!(p > 0);
}

// --- Qty edge cases ---

#[test]
fn qty_zero() {
    let q: Qty = 0;
    assert_eq!(q, 0);
}

#[test]
fn qty_negative() {
    let q: Qty = -100 * QTY_SCALE;
    assert_eq!(q as f64 / QTY_SCALE as f64, -100.0);
}

#[test]
fn qty_smallest_representable() {
    let q: Qty = 1;
    let f = q as f64 / QTY_SCALE as f64;
    assert!((f - 1e-8).abs() < 1e-12, "the smallest size a venue counts in");
}

// --- OrderBuffer edge cases ---

#[test]
fn order_buffer_multiple_drain_cycles() {
    let mut buf = OrderBuffer::new();
    for cycle in 0..10 {
        for i in 0..5 {
            buf.push(OrderRequest::Cancel { order_id: (cycle * 5 + i) as u64 });
        }
        let drained: Vec<_> = buf.drain().collect();
        assert_eq!(drained.len(), 5);
        assert!(buf.is_empty());
    }
}

#[test]
fn order_buffer_drain_empty() {
    let mut buf = OrderBuffer::new();
    let drained: Vec<_> = buf.drain().collect();
    assert!(drained.is_empty());
}

// --- All OrderRequest variants ---

// ── snap-to-tick ──

#[test]
fn instrument_accessor_covers_submits() {
    let req = OrderRequest::SubmitEx {
        order_id: 1, instrument: 7, side: Side::Buy, qty: crate::types::QTY_SCALE,
        kind: OrderKind::Market, tif: b'0', attrs: OrderAttrs::default(),
    };
    assert_eq!(req.instrument(), Some(7));
    assert_eq!(OrderRequest::Cancel { order_id: 1 }.instrument(), None);
    assert_eq!(
        OrderRequest::Modify { order_id: 1, price: 0, qty: crate::types::QTY_SCALE, outside_rth: false, ord_type: 0, tif: 0, stop_price: 0 }.instrument(),
        None
    );
}

#[test]
fn order_request_modify_fields() {
    let req = OrderRequest::Modify { order_id: 99, price: 200 * PRICE_SCALE, qty: 10 * crate::types::QTY_SCALE, outside_rth: false, ord_type: 0, tif: 0, stop_price: 0 };
    match req {
        OrderRequest::Modify { order_id, price, qty, .. } => {
            assert_eq!(order_id, 99);
            assert_eq!(price, 200 * PRICE_SCALE);
            assert_eq!(qty, 10 * QTY_SCALE);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn order_request_cancel_all_fields() {
    let req = OrderRequest::CancelAll { instrument: 7 };
    match req {
        OrderRequest::CancelAll { instrument } => assert_eq!(instrument, 7),
        _ => panic!("wrong variant"),
    }
}

// --- AccountState ---

#[test]
fn account_state_default() {
    let a = AccountState::default();
    assert_eq!(a.net_liquidation, 0);
    assert_eq!(a.buying_power, 0);
    assert_eq!(a.margin_used, 0);
    assert_eq!(a.unrealized_pnl, 0);
    assert_eq!(a.realized_pnl, 0);
}

#[test]
fn account_state_copy() {
    let a = AccountState { net_liquidation: 100_000 * PRICE_SCALE, ..Default::default() };
    let b = a; // Copy
    assert_eq!(b.net_liquidation, 100_000 * PRICE_SCALE);
}

// --- Fill ---

#[test]
fn fill_is_copy() {
    let f = Fill {
        instrument: 0,
        order_id: 1,
        side: Side::Buy,
        price: 150 * PRICE_SCALE,
        qty: 100 * QTY_SCALE,
        remaining: 0,
        commission: 0,
        timestamp_ns: 123456789,
        cum_qty: 100 * QTY_SCALE, avg_price: 150 * PRICE_SCALE,
    };
    let f2 = f; // Copy
    assert_eq!(f.order_id, f2.order_id);
    assert_eq!(f.timestamp_ns, f2.timestamp_ns);
}

// --- Order ---

#[test]
fn order_is_copy() {
    let o = Order {
        order_id: 42,
        instrument: 0,
        side: Side::Sell,
        price: 200 * PRICE_SCALE,
        qty: 50 * QTY_SCALE,
        filled: 10 * QTY_SCALE,
        status: OrderStatus::PartiallyFilled,
        ord_type: b'2',
        tif: b'0',
        stop_price: 0,
    };
    let o2 = o; // Copy
    assert_eq!(o.order_id, o2.order_id);
    assert_eq!(o.filled, o2.filled);
}

// --- Side ---

#[test]
fn side_equality() {
    assert_eq!(Side::Buy, Side::Buy);
    assert_eq!(Side::Sell, Side::Sell);
    assert_ne!(Side::Buy, Side::Sell);
}

// --- OrderStatus ---

#[test]
fn order_status_equality() {
    assert_eq!(OrderStatus::Submitted, OrderStatus::Submitted);
    assert_ne!(OrderStatus::Filled, OrderStatus::Cancelled);
    assert_ne!(OrderStatus::PartiallyFilled, OrderStatus::Filled);

    // The outward vocabulary has no status of its own for a partly filled
    // working order: the venue reports it as submitted and the filled and
    // remaining quantities carry the distinction. Named as one it sat in
    // neither the active set nor the done set of a program reading this, so a
    // working order read as neither running nor finished.
    use crate::types::order_status::{is_open_status, order_status_str};
    assert_eq!(order_status_str(OrderStatus::PartiallyFilled), "Submitted");
    assert!(is_open_status(order_status_str(OrderStatus::PartiallyFilled)));
    assert!(!is_open_status("PartiallyFilled"), "and it is not a status this states");
}

// --- WhatIfResponse ---

#[test]
fn what_if_response_is_copy() {
    let r = WhatIfResponse {
        order_id: 1,
        instrument: 0,
        init_margin_before: 136_401 * (PRICE_SCALE / 100),
        maint_margin_before: 113_167 * (PRICE_SCALE / 100),
        equity_with_loan_before: 75_425_514 * (PRICE_SCALE / 100),
        init_margin_after: 895_786 * (PRICE_SCALE / 100),
        maint_margin_after: 814_351 * (PRICE_SCALE / 100),
        equity_with_loan_after: 75_425_514 * (PRICE_SCALE / 100),
        commission: PRICE_SCALE,
        min_commission: 0,
        max_commission: 0,
        commission_currency: String::new(),
        warning_text: String::new(),
    };
    // The reply carries venue-supplied text, so it is cloned rather than
    // copied.
    let r2 = r.clone();
    assert_eq!(r.init_margin_after, r2.init_margin_after);
    assert_eq!(r.commission, r2.commission);
    // The change is the difference, which the venue leaves to be taken.
    assert_eq!(r.init_margin_change(), r.init_margin_after - r.init_margin_before);
}

/// A preview carries what the order would cost, and where the venue can only
/// bound that cost it says so as a range in a stated currency, with any warning
/// it has about the order beside it. Dropped on the way to the callback, a
/// preview reported a cost of zero for every such order.
#[test]
fn a_preview_carries_the_cost_the_venue_quoted() {
    let reply = WhatIfResponse {
        order_id: 1,
        instrument: 0,
        init_margin_before: 0,
        maint_margin_before: 0,
        equity_with_loan_before: 0,
        init_margin_after: 0,
        maint_margin_after: 0,
        equity_with_loan_after: 0,
        commission: 0,
        min_commission: 175 * (PRICE_SCALE / 100),
        max_commission: 320 * (PRICE_SCALE / 100),
        commission_currency: "USD".into(),
        warning_text: "this order will be routed away".into(),
    };
    let state = crate::types::model::OrderState::from(&reply);
    assert!((state.min_commission_and_fees - 1.75).abs() < 1e-9, "{state:?}");
    assert!((state.max_commission_and_fees - 3.20).abs() < 1e-9, "{state:?}");
    assert_eq!(state.commission_and_fees_currency, "USD");
    assert_eq!(state.warning_text, "this order will be routed away");
}

// --- AdjustedOrderType ---

/// Tag 6261 carries the order type's registry code, which is a number for
/// only two of the four conversions.
#[test]
fn adjusted_order_type_fix_codes() {
    assert_eq!(AdjustedOrderType::Stop.fix_code(), "3");
    assert_eq!(AdjustedOrderType::StopLimit.fix_code(), "4");
    assert_eq!(AdjustedOrderType::Trail.fix_code(), "T");
    assert_eq!(AdjustedOrderType::TrailLimit.fix_code(), "TSL");
}

// --- OrderAttrs cash_qty ---

#[test]
fn order_attrs_cash_qty_default_zero() {
    let attrs = OrderAttrs::default();
    assert_eq!(attrs.cash_qty, 0);
}

/// A decimal price converts to the price the caller stated, not to the one a
/// binary double sits just below. Truncating sends 0.29 as 0.28999999 — off the
/// instrument's tick, and not the price that was asked for — and better than
/// five in a hundred ordinary two-decimal prices land on that side.
#[test]
fn a_stated_price_converts_to_the_price_that_was_stated() {
    use super::{price_from_f64, PRICE_SCALE};

    for cents in 1..10_000i64 {
        let stated = cents as f64 / 100.0;
        assert_eq!(
            price_from_f64(stated),
            cents * (PRICE_SCALE / 100),
            "{stated} did not convert to itself",
        );
    }

    // The ones that truncate low, named so a regression says which rule broke.
    assert_eq!(price_from_f64(0.29), 29_000_000);
    assert_eq!(price_from_f64(8.62), 862_000_000);
    assert_eq!(price_from_f64(-0.29), -29_000_000, "and on the sell side");
    assert_eq!(price_from_f64(f64::NAN), 0, "nothing is not a price");
}

/// A caller's decimal becomes the fixed-point form exactly, both ways, for
/// every quantity the order path accepts.
#[test]
fn qty_from_f64_is_exact_up_to_the_bound() {
    use super::{qty_from_f64, qty_to_f64, MAX_EXACT_QTY_SHARES, QTY_SCALE};

    assert_eq!(qty_from_f64(0.5), QTY_SCALE / 2, "half a share");
    assert_eq!(qty_from_f64(100.0), 100 * QTY_SCALE, "a whole one");
    assert_eq!(qty_from_f64(0.00000001), 1, "the finest the scale holds");
    assert_eq!(qty_from_f64(f64::NAN), 0, "not a number is not a quantity");
    assert_eq!(qty_from_f64(f64::INFINITY), 0);

    // Rounded, not truncated: a tenth is not exact in binary, so truncation
    // puts three tenths one hundred-millionth low.
    assert_eq!(qty_from_f64(0.3), 3 * QTY_SCALE / 10);

    // The bound is where the product still fits the 53 bits an f64 carries,
    // so the round trip is lossless everywhere it is accepted.
    let largest = MAX_EXACT_QTY_SHARES;
    assert_eq!(qty_to_f64(qty_from_f64(largest)), largest, "exact at the bound");
    assert_eq!(qty_to_f64(qty_from_f64(1234.5678)), 1234.5678, "and below it");
}

mod counted_size_tests {
    use super::super::{qty_from_counted, qty_from_wire, QTY_SCALE};

    /// A share is counted in whole ones, and reads the way it always did.
    #[test]
    fn a_share_is_counted_in_whole_ones() {
        assert_eq!(qty_from_counted(300, 1.0), qty_from_wire(300));
        assert_eq!(qty_from_counted(300, 1.0), 300 * QTY_SCALE);
    }

    /// An instrument the venue stated no increment for is counted in whole
    /// ones, which is what stating none means.
    #[test]
    fn no_stated_increment_is_whole_ones() {
        assert_eq!(qty_from_counted(300, 0.0), qty_from_wire(300));
    }

    /// A crypto is counted in hundred-millionths. Taken as whole ones, a
    /// hundredth of a coin reads as a million of them.
    #[test]
    fn a_crypto_is_counted_in_hundred_millionths() {
        let hundredth_of_a_coin = 1_000_000;
        let scaled = qty_from_counted(hundredth_of_a_coin, 1e-8);
        assert_eq!(scaled, (0.01 * QTY_SCALE as f64) as i64);
        assert_ne!(scaled, qty_from_wire(hundredth_of_a_coin));
    }
}
mod quantity_scale_tests {
    use super::super::{qty_from_counted, Qty, QTY_SCALE};

    /// The smallest size a venue counts in survives being held. At a
    /// ten-thousandth it did not: everything finer rounded to nothing, and a
    /// quote for a thousandth of a coin came back as no quote at all.
    #[test]
    fn the_smallest_counted_size_survives() {
        let one_count = qty_from_counted(1, 1e-8);
        assert!(one_count > 0, "a hundred-millionth rounded away");
        assert_eq!(one_count as f64 / QTY_SCALE as f64, 1e-8);
    }

    /// A day's volume in the busiest listing still fits.
    #[test]
    fn a_whole_market_day_still_fits() {
        let shares = 5_000_000_000i64;
        let held = qty_from_counted(shares, 1.0);
        assert_eq!(held / QTY_SCALE, shares);
        assert!(held < Qty::MAX / 2, "a day's volume is nowhere near the ceiling");
    }
}

/// A preview names the order it was asked about.
///
/// The narrower set a replace may restate was shared with previews, so a
/// trailing stop, a relative, a midprice, a snap and a pegged order all went
/// out as limits. The margin is the same either way — it follows the position
/// the order would leave, not the instruction that reaches it — but a security
/// that refuses limits refused a preview of an order that was not one.
#[test]
fn a_preview_states_the_type_it_was_asked_about() {
    use crate::types::model::Order;

    // What tag 40 carries, which is what the venue reads. Asserting the byte
    // instead let a type whose value is more than one character pass while the
    // wire got the discriminant itself — an unprintable byte, not a type.
    let previewed = |kind: &str| {
        let mut o = Order::limit("BUY", 1.0, 1.00);
        o.order_type = kind.to_string();
        crate::types::ord_type_fix_str(o.what_if_byte())
    };

    // The types a replace also states, unchanged.
    assert_eq!(previewed("MKT"), "1");
    assert_eq!(previewed("LMT"), "2");
    assert_eq!(previewed("STP LMT"), "4");

    // The types a replace does not state. A preview states the same value the
    // order itself would be sent as, so these are the strings the new-order
    // path writes on tag 40.
    // Trailing, relative and pegged orders are all sent as "P" and told apart
    // by their ExecInst, so that is what a preview of one states.
    assert_eq!(previewed("TRAIL"), "P");
    assert_eq!(previewed("REL"), "P");
    assert_eq!(previewed("PEG MID"), "P");
    assert_eq!(previewed("TRAIL LIMIT"), "TSL");
    assert_eq!(previewed("LIT"), "LT");
    assert_eq!(previewed("MTL"), "K");
    assert_eq!(previewed("MIDPRICE"), "MIDPX");
    assert_eq!(previewed("SNAP MID"), "SMID");
    assert_eq!(previewed("STP PRT"), "SP");
    assert_eq!(previewed("PEG BENCH"), "PB");

    // Spelled the way a caller spells it.
    assert_eq!(previewed("peg mid"), previewed("PEG MID"));

    // Every type a preview names is one the table spells out. A discriminant
    // with no entry falls back to a limit, which reaches the venue as a
    // preview of an order the caller did not describe.
    for kind in [
        "MKT", "LMT", "STP", "STP LMT", "MOC", "LOC", "MIT", "MTL", "BOX TOP",
        "MKT PRT", "REL", "TRAIL", "TRAIL LIMIT", "LIT", "STP PRT", "MIDPRICE",
        "SNAP MKT", "SNAP MID", "SNAP PRI", "PEG MKT", "PEG MID", "PEG BENCH",
    ] {
        let s = previewed(kind);
        assert!(
            s.is_ascii() && !s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic()),
            "{kind} previews as {s:?}, which is not a type the venue can read",
        );
    }

    // A replace still states only what it can carry, and reads no byte as
    // "leave the resting order's type alone" — widening that is a change to
    // modification, not to previews.
    let mut trailing = Order::limit("BUY", 1.0, 1.00);
    trailing.order_type = "TRAIL".to_string();
    assert_eq!(trailing.ord_type_byte(), 0, "a replace cannot restate a trailing stop");

    // Both spellings of a midprice, since the placement path takes both.
    assert_eq!(previewed("MIDPX"), "MIDPX");

    // A type nobody here knows falls back to a limit, and validation refuses
    // it before a preview gets this far — see
    // `a_preview_is_refused_for_a_type_this_client_cannot_send`.
    assert_eq!(previewed("SOMETHING NEW"), "2");
}
