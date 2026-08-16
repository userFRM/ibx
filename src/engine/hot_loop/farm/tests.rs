//! The tests for this module.
//!
//! One file per module, as `api/client` already does it. Each block below
//! reaches the code it tests through `super::super`, which is the module this
//! file belongs to.

mod news_tests {
    use super::super::*;
    use crate::bridge::SharedState;
    use crate::engine::context::Context;

    /// Frame records the way the venue frames them: the length of everything
    /// after it in bits, then each record as its own tick states lengths.
    fn framed_generic_ticks(records: &[(u32, u32, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (server_tag, tick, payload) in records {
            body.extend_from_slice(&server_tag.to_be_bytes());
            match PayloadLength::of(*tick) {
                PayloadLength::OneByte => body.push(payload.len() as u8),
                PayloadLength::TwoBytes => {
                    body.extend_from_slice(&(payload.len() as u16).to_be_bytes())
                }
                PayloadLength::ToTheEnd => {}
            }
            body.extend_from_slice(payload);
        }
        let mut msg = b"35=G\x01".to_vec();
        msg.extend_from_slice(&(((body.len() * 8) % 65_536) as u16).to_be_bytes());
        msg.extend_from_slice(&body);
        msg
    }

    /// One news record.
    fn framed_news(server_tag: u32, payload: &[u8]) -> Vec<u8> {
        framed_generic_ticks(&[(server_tag, NEWS_REQUEST_TYPE, payload)])
    }

    /// One article, laid out as the handler reads it.
    fn one_article() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&1u32.to_be_bytes());
        body.extend_from_slice(&4u32.to_be_bytes());
        body.extend_from_slice(b"BRFG");
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(b"id");
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&1_785_325_554u32.to_be_bytes());
        body.extend_from_slice(&8u32.to_be_bytes());
        body.extend_from_slice(b"headline");
        body
    }

    /// A frame under a number nothing asked a generic tick under says nothing
    /// about which tick it is, so it is dropped rather than guessed at.
    /// Instrument 0 is a real instrument — the first one registered — so a
    /// guess would pin somebody else's article on it.
    #[test]
    fn a_tick_under_an_unasked_number_is_dropped_not_misattributed() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let first = context.market.register(756733);
        assert_eq!(first, 0, "the first instrument really is id 0");
        context.market.register_server_tag(999_999, first);

        let msg = framed_news(999_999, &one_article());
        farm.handle_generic_tick(&msg, &mut context, &shared, &None);
        assert!(
            shared.market.drain_tick_news().is_empty(),
            "a number nothing asked a generic tick under delivers nothing",
        );

        // Positive control: the same frame, once this client has said what it
        // asked for under that number.
        farm.generic_tick_tags.push((999_999, NEWS_REQUEST_TYPE, first));
        farm.handle_generic_tick(&msg, &mut context, &shared, &None);
        assert_eq!(
            shared.market.drain_tick_news().len(), 1,
            "so the drop above is what was asked for, not the frame",
        );
    }

    /// Which tick a frame carries is what was asked for under its number, not
    /// how long the frame is. Read off the length, every tick whose payload
    /// happened to be the size of an option model read as an option model.
    #[test]
    fn two_ticks_of_one_length_are_told_apart() {
        let shared = SharedState::new();
        let article = one_article();

        let mut context = Context::new();
        let mut as_news = FarmState::new();
        as_news.generic_tick_tags.push((7, NEWS_REQUEST_TYPE, 0));
        as_news.handle_generic_tick(&framed_news(7, &article), &mut context, &shared, &None);
        assert_eq!(shared.market.drain_tick_news().len(), 1);

        // The same bytes, the same length, asked for as something else.
        let mut as_status = FarmState::new();
        as_status.generic_tick_tags.push((7, TRADING_STATUS_REQUEST_TYPE, 0));
        as_status.handle_generic_tick(&framed_news(7, &article), &mut context, &shared, &None);
        assert!(
            shared.market.drain_tick_news().is_empty(),
            "the same bytes under a different tick are not an article",
        );
    }

    /// A message carries one record after another, and each is delivered. Read
    /// as a single record, everything after the first went unread.
    #[test]
    fn every_record_in_a_message_is_read() {
        let article = one_article();
        let msg = framed_generic_ticks(&[
            (7, NEWS_REQUEST_TYPE, &article),
            (9, NEWS_REQUEST_TYPE, &article),
        ]);

        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        farm.generic_tick_tags.push((7, NEWS_REQUEST_TYPE, 0));
        farm.generic_tick_tags.push((9, NEWS_REQUEST_TYPE, 1));
        farm.handle_generic_tick(&msg, &mut context, &shared, &None);

        let delivered = shared.market.drain_tick_news();
        assert_eq!(delivered.len(), 2, "the second record went unread");
        assert_eq!(delivered[0].instrument, 0);
        assert_eq!(delivered[1].instrument, 1);
    }

    /// Where a record ends depends on the tick it carries, so a number nothing
    /// asked for stops the reading. Carrying on would read the next record
    /// from the middle of this one and deliver whatever that happened to spell.
    #[test]
    fn an_unasked_number_stops_the_reading() {
        let article = one_article();
        let msg = framed_generic_ticks(&[
            (5, NEWS_REQUEST_TYPE, &article),
            (7, NEWS_REQUEST_TYPE, &article),
        ]);

        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        // Only the second record's number is known.
        farm.generic_tick_tags.push((7, NEWS_REQUEST_TYPE, 0));
        farm.handle_generic_tick(&msg, &mut context, &shared, &None);
        assert!(
            shared.market.drain_tick_news().is_empty(),
            "reading carried on past a record whose end was unknown",
        );
    }

    /// The venue states the length in bits in two bytes, so it wraps at eight
    /// thousand one hundred and ninety-two. What was carried is recovered
    /// against how much arrived, or a long message is cut off in the middle
    /// with nothing to say it had been.
    #[test]
    fn a_message_longer_than_the_length_field_holds_is_recovered() {
        assert_eq!(generic_tick_length(168, 23), Some(21));
        // Nine thousand bytes: the stated count has wrapped once, and what
        // arrived is what says so.
        let carried = 9_000usize;
        let stated = ((carried * 8) % 65_536) as u16;
        assert_eq!(generic_tick_length(stated, carried + 2), Some(carried));
    }

    /// A tick that states its length in two bytes is read that way. Which
    /// ticks do is a property of the tick, not something on the frame.
    #[test]
    fn the_length_form_follows_the_tick() {
        assert_eq!(PayloadLength::of(NEWS_REQUEST_TYPE), PayloadLength::TwoBytes);
        assert_eq!(PayloadLength::of(TRADING_STATUS_REQUEST_TYPE), PayloadLength::OneByte);
        assert_eq!(PayloadLength::of(GREEKS_REQUEST_TYPE), PayloadLength::OneByte);
        assert_eq!(PayloadLength::of(320), PayloadLength::ToTheEnd);

        let payload = vec![3u8; 300];
        let msg = framed_news(11, &payload);
        let mut seen = Vec::new();
        read_generic_ticks(&msg[5..], |_| Some(NEWS_REQUEST_TYPE), |_, record| {
            seen.push(record.payload.len())
        });
        assert_eq!(seen, vec![300]);
    }

}
mod decode_publish_tests {
    use super::super::*;
    use crate::bridge::SharedState;
    use crate::engine::context::Context;
    use crate::protocol::tick_decoder;
    use crate::types::QTY_SCALE;

    fn push_bits(bits: &mut Vec<u8>, val: u64, n: usize) {
        for i in (0..n).rev() {
            bits.push(((val >> i) & 1) as u8);
        }
    }

    /// One 35=P body carrying `ticks` for `server_tag`, framed as the farm
    /// connection delivers it.
    fn framed_35p(server_tag: u32, ticks: &[(u64, u64, u64)]) -> Vec<u8> {
        let mut bits: Vec<u8> = Vec::new();
        push_bits(&mut bits, 0, 1);
        push_bits(&mut bits, server_tag as u64, 31);
        for (i, &(tick_type, width, value)) in ticks.iter().enumerate() {
            push_bits(&mut bits, tick_type, 5);
            push_bits(&mut bits, if i < ticks.len() - 1 { 1 } else { 0 }, 1);
            push_bits(&mut bits, width - 1, 2);
            push_bits(&mut bits, 0, 1); // positive
            push_bits(&mut bits, value, (width * 8 - 1) as usize);
        }
        let byte_count = bits.len().div_ceil(8);
        let mut payload = vec![0u8; byte_count];
        for (i, &b) in bits.iter().enumerate() {
            if b == 1 {
                payload[i >> 3] |= 1 << (7 - (i & 7));
            }
        }
        let mut tick_payload = Vec::with_capacity(2 + byte_count);
        tick_payload.push((bits.len() >> 8) as u8);
        tick_payload.push((bits.len() & 0xFF) as u8);
        tick_payload.extend_from_slice(&payload);

        let body_len = 5 + tick_payload.len() + 15;
        let mut msg = format!("8=O\x019={body_len}\x01").into_bytes();
        msg.extend_from_slice(b"35=P\x01");
        msg.extend_from_slice(&tick_payload);
        msg.extend_from_slice(b"\x018349=AABBCCDD\x01");
        msg
    }

    /// The constants table says which wire type is which; this says where each
    /// one lands. Nothing else pins that: swapping the open and close arms with
    /// the table intact passes the whole suite, and that is precisely the
    /// failure this decode change exists to remove — two plausible prices
    /// exchanged, with the P&L path reading the wrong one.
    #[test]
    fn each_price_type_lands_in_its_own_quote_field() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(9, id);
        context.market.set_min_tick(id, 0.01);

        // Distinct magnitudes, so no two fields can be confused.
        let msg = framed_35p(9, &[
            (tick_decoder::O_LAST_PRICE, 2, 501),
            (tick_decoder::O_HIGH_PRICE, 2, 502),
            (tick_decoder::O_LOW_PRICE, 2, 503),
            (tick_decoder::O_OPEN_PRICE, 2, 504),
            (tick_decoder::O_CLOSE_PRICE, 2, 505),
        ]);
        farm.handle_tick_data(&msg, &mut context, &shared, &None);

        let mts = context.market.min_tick_scaled(id);
        let q = context.market.quote(id);
        assert_eq!(q.last, 501 * mts, "last");
        assert_eq!(q.high, 502 * mts, "high");
        assert_eq!(q.low, 503 * mts, "low");
        assert_eq!(q.open, 504 * mts, "open");
        assert_eq!(q.close, 505 * mts, "close");
    }

    /// The timestamp arm carries seconds and is stored in nanoseconds, and the
    /// guard is what keeps a date-shaped value out of the field.
    #[test]
    fn the_timestamp_is_seconds_stored_as_nanoseconds() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(11, id);
        context.market.set_min_tick(id, 0.01);

        farm.handle_tick_data(
            &framed_35p(11, &[(tick_decoder::O_TS_BASE, 4, 1_785_325_554)]),
            &mut context, &shared, &None,
        );
        assert_eq!(
            context.market.quote(id).timestamp_ns, 1_785_325_554_000_000_000,
            "an epoch second is stored as nanoseconds",
        );

        // A yyyymmdd-shaped value is not a timestamp and must not land here.
        let id2 = context.market.register(265598);
        context.market.register_server_tag(12, id2);
        context.market.set_min_tick(id2, 0.01);
        farm.handle_tick_data(
            &framed_35p(12, &[(tick_decoder::O_TS_BASE, 4, 20_260_729)]),
            &mut context, &shared, &None,
        );
        assert_eq!(
            context.market.quote(id2).timestamp_ns, 0,
            "a date-shaped magnitude is dropped rather than stored",
        );
    }

    /// The producer half of the quantity contract. Everything downstream
    /// divides by `QTY_SCALE`, so a decode path that stores the wire magnitude
    /// raw delivers quantities 10_000x too small — and nothing else
    /// in the suite reaches this function, which is why that shipped.
    #[test]
    fn decoded_quantities_are_stored_as_fixed_point() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(7, id);
        context.market.set_min_tick(id, 0.01);

        let msg = framed_35p(7, &[
            (tick_decoder::O_BID_SIZE, 1, 42),
            (tick_decoder::O_ASK_SIZE, 1, 17),
            (tick_decoder::O_LAST_SIZE, 1, 5),
            (tick_decoder::O_VOLUME, 2, 1234),
        ]);
        farm.handle_tick_data(&msg, &mut context, &shared, &None);

        let q = context.market.quote(id);
        assert_eq!(q.bid_size, 42 * QTY_SCALE, "bid_size must be stored fixed-point");
        assert_eq!(q.ask_size, 17 * QTY_SCALE, "ask_size must be stored fixed-point");
        assert_eq!(q.last_size, 5 * QTY_SCALE, "last_size must be stored fixed-point");
        assert_eq!(q.volume, 1234 * QTY_SCALE, "volume must be stored fixed-point");
    }

    /// Prices were already scaled correctly; pin that the quantity change did
    /// not disturb them.
    #[test]
    fn decoded_prices_are_still_scaled_by_min_tick() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(9, id);
        context.market.set_min_tick(id, 0.01);
        let mts = context.market.min_tick_scaled(id);

        let msg = framed_35p(9, &[(tick_decoder::O_BID_PRICE, 2, 15000)]);
        farm.handle_tick_data(&msg, &mut context, &shared, &None);

        assert_eq!(context.market.quote(id).bid, 15000 * mts);
    }
}
mod resub_tests {
    use super::super::*;
    use crate::engine::market_state::MarketState;

    /// A disconnect clears `instrument_md_reqs` and keeps `md_resub_info`.
    /// Selecting the reconnect's work from the cleared list re-subscribed
    /// nothing, so the farm came back healthy and delivered no ticks for the
    /// rest of the session.
    ///
    /// Drives the real `handle_disconnect` rather than simulating what it does
    /// — the test-only hook that skips the clearing is what let this survive,
    /// and a hand-written stand-in can drift from the real one the same way.
    #[test]
    fn resub_targets_survive_a_real_disconnect() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let mut hb = HeartbeatState::new();
        let instrument = context.market.register(756733);

        farm.send_mktdata_subscribe(
            756733, "SPY", "SMART", "STK", "", 0.0, "", "", instrument, 0,
            &mut None, &mut hb,
        );
        farm.handle_disconnect(&mut context, &None);
        assert!(farm.instrument_md_reqs.is_empty(), "the disconnect clears the request list");

        let targets = farm.take_resub_targets(&context.market);
        assert_eq!(targets.len(), 1, "the subscription must survive the disconnect");
        assert_eq!(targets[0].0, instrument);
        assert_eq!(targets[0].1, 756733, "con_id must be resolved for the re-issue");
        assert_eq!(targets[0].2, "SPY");

        // Re-issuing with no connection must still leave the record standing,
        // so a later reconnect can retry rather than losing the subscription.
        let (id, con_id, sym, exch, st, ltd, k, r, m, mode) = targets.into_iter().next().unwrap();
        farm.send_mktdata_subscribe(
            con_id, &sym, &exch, &st, &ltd, k, &r, &m, id, mode, &mut None, &mut hb,
        );
        assert_eq!(farm.md_resub_info.len(), 1, "the record must survive an absent connection");
    }

    /// An unsubscribe issued while the farm is down must still cancel. The
    /// lookup it does first early-returns during an outage, so a record left
    /// standing would be replayed on reconnect as a subscription the caller
    /// had explicitly cancelled.
    #[test]
    fn unsubscribing_while_down_does_not_leave_a_resubscribe_record() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let mut hb = HeartbeatState::new();
        let instrument = context.market.register(756733);

        farm.send_mktdata_subscribe(
            756733, "SPY", "SMART", "STK", "", 0.0, "", "", instrument, 0,
            &mut None, &mut hb,
        );
        farm.handle_disconnect(&mut context, &None);
        farm.send_mktdata_unsubscribe(instrument, &mut None, &mut hb);

        assert!(
            farm.take_resub_targets(&context.market).is_empty(),
            "a cancelled subscription must not come back on reconnect",
        );
    }

    /// The other side of keeping a slot resident: it has to become releasable
    /// again, or the guard turns a bounded pool into a leak and the instrument
    /// cap becomes cumulative-per-session — the failure exists to
    /// prevent. Every route out of a subscription has to clear all three
    /// references, whether the farm is up or down.
    #[test]
    fn a_slot_becomes_reclaimable_again_once_the_subscription_ends() {
        for down in [false, true] {
            let mut farm = FarmState::new();
            let mut context = Context::new();
            let mut hb = HeartbeatState::new();
            let instrument = context.market.register(756733);

            farm.send_mktdata_subscribe(
                756733, "SPY", "SMART", "STK", "", 0.0, "", "", instrument, 0,
                &mut None, &mut hb,
            );
            assert!(farm.holds_market_data(instrument), "subscribed: held");

            if down {
                farm.handle_disconnect(&mut context, &None);
                // The record deliberately survives a disconnect, so the slot
                // stays held — that is what makes the resubscribe possible.
                assert!(farm.holds_market_data(instrument), "disconnected: still held");
            }

            farm.send_mktdata_unsubscribe(instrument, &mut None, &mut hb);
            assert!(
                !farm.holds_market_data(instrument),
                "unsubscribed (farm down: {down}): the slot must be releasable",
            );
        }
    }

    /// A reconnect's replay is paced, and the pace must not be taken out of
    /// the engine.
    ///
    /// One thread drives every transport, the heartbeats, the reconnects and
    /// shutdown. Sleeping between bursts stops all of it for as long as the
    /// caller's pacing says, so the book is put back across the passes the
    /// loop is already making instead.
    #[test]
    fn a_paced_replay_does_not_hold_the_engine() {
        use crate::engine::hot_loop::{HeartbeatState, ReplayPacing};

        let mut farm = FarmState::new();
        let mut market = MarketState::new();
        let mut context = Context::new();
        let mut hb = HeartbeatState::new();
        let shared = SharedState::new();

        for con_id in 0..5i64 {
            let instrument = market.register(700000 + con_id);
            farm.md_resub_info.push((
                instrument, "SPY".into(), "SMART".into(), "STK".into(), String::new(),
                0.0, String::new(), String::new(), 0,
            ));
        }
        context.market = market;

        // A pace no engine could afford to wait out, and one at a time.
        let replay = ReplayPacing { burst: 1, pace: std::time::Duration::from_secs(30) };

        let (sock, _peer) = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let s = std::net::TcpStream::connect(l.local_addr().unwrap()).unwrap();
            let (p, _) = l.accept().unwrap();
            (s, p)
        };
        let mut conn = Some(Connection::new_raw(sock).unwrap());

        let started = Instant::now();
        farm.replay_queue = farm.take_resub_targets(&context.market).into_iter().collect();
        farm.replay_not_before = None;
        farm.drive_replay(replay, &mut conn, &mut hb);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "the replay returned rather than waiting out its own pacing",
        );

        assert_eq!(farm.replay_queue.len(), 4, "one burst went out, the rest are waiting");
        assert!(farm.replay_not_before.is_some(), "and the next burst has a time");

        // Before the pace elapses, nothing more goes out.
        farm.drive_replay(replay, &mut conn, &mut hb);
        assert_eq!(farm.replay_queue.len(), 4, "the pacing is still honoured");

        // With the pace elapsed, the next burst goes.
        farm.replay_not_before = Some(Instant::now());
        farm.drive_replay(replay, &mut conn, &mut hb);
        assert_eq!(farm.replay_queue.len(), 3);

        // And a book that empties stops asking for time.
        while !farm.replay_queue.is_empty() {
            farm.replay_not_before = Some(Instant::now());
            farm.drive_replay(replay, &mut conn, &mut hb);
        }
        assert!(farm.replay_queue.is_empty(), "every subscription was put back");
        assert!(farm.replay_not_before.is_none(), "nothing left to wait for");
        let _ = shared;
    }

    /// A farm that drops again mid-replay must not lose what was still queued.
    ///
    /// A subscription that has been sent records itself again as it goes out,
    /// so the next reconnect finds it. One still waiting was never sent, and
    /// the reconnect rebuilds the queue from that record — so a queue dropped
    /// on disconnect takes those subscriptions with it, and the market data
    /// the caller asked for never comes back, with nothing to say why.
    #[test]
    fn a_second_drop_mid_replay_keeps_what_was_still_waiting() {
        use crate::engine::hot_loop::{HeartbeatState, ReplayPacing};

        let mut farm = FarmState::new();
        let mut market = MarketState::new();
        let mut context = Context::new();
        let mut hb = HeartbeatState::new();

        for con_id in 0..4i64 {
            let instrument = market.register(700000 + con_id);
            farm.md_resub_info.push((
                instrument, "SPY".into(), "SMART".into(), "STK".into(), String::new(),
                0.0, String::new(), String::new(), 0,
            ));
        }
        context.market = market;

        let (sock, _peer) = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let s = std::net::TcpStream::connect(l.local_addr().unwrap()).unwrap();
            let (p, _) = l.accept().unwrap();
            (s, p)
        };
        let mut conn = Some(Connection::new_raw(sock).unwrap());

        // One goes out; three are still waiting.
        let replay = ReplayPacing { burst: 1, pace: std::time::Duration::from_secs(30) };
        farm.replay_queue = farm.take_resub_targets(&context.market).into_iter().collect();
        farm.drive_replay(replay, &mut conn, &mut hb);
        assert_eq!(farm.replay_queue.len(), 3);

        // And the farm goes before the rest of them do.
        farm.handle_disconnect(&mut context, &None);

        assert!(farm.replay_queue.is_empty(), "nothing is left holding them");
        assert_eq!(
            farm.md_resub_info.len(), 4,
            "all four are recorded for the next reconnect: the one that was \
             sent recorded itself, and the three that were not are put back",
        );
    }

    /// A slot reclaimed while the farm was down has no con_id to subscribe.
    #[test]
    fn resub_targets_skip_an_instrument_reclaimed_while_down() {
        let mut farm = FarmState::new();
        let mut market = MarketState::new();
        let instrument = market.register(756733);
        farm.md_resub_info.push((
            instrument, "SPY".into(), "SMART".into(), "STK".into(), String::new(),
            0.0, String::new(), String::new(), 0,
        ));
        market.unregister(instrument);

        assert!(farm.take_resub_targets(&market).is_empty());
    }

    /// The case the test above does not reach: the slot is not merely freed but
    /// handed to another contract before the reconnect. `md_resub_info` holds
    /// no con_id of its own, so the record is combined with whatever con_id the
    /// id now resolves to — the old contract's descriptor subscribing the new
    /// contract's instrument. The guard is that a slot holding market-data
    /// state is not reclaimable in the first place.
    #[test]
    fn an_instrument_holding_a_resubscribe_record_is_not_reclaimable() {
        let mut farm = FarmState::new();
        let mut market = MarketState::new();
        let instrument = market.register(756733);
        farm.md_resub_info.push((
            instrument, "SPY".into(), "SMART".into(), "STK".into(), String::new(),
            0.0, String::new(), String::new(), 0,
        ));

        assert!(
            farm.holds_market_data(instrument),
            "the record alone must keep the slot resident",
        );

        // And a live subscription does the same on its own.
        let mut farm = FarmState::new();
        farm.instrument_md_reqs.push((instrument, vec![7]));
        assert!(farm.holds_market_data(instrument), "a live subscription");

        // An instrument with none of the three is free to go.
        assert!(!FarmState::new().holds_market_data(instrument));
    }
}
use super::*;
use std::collections::HashMap;

/// The tag a 35=Y frame opens with sits after the marker, not on it.
///
/// Read a byte early and the value names no subscription at all, so the
/// whole opening section of the book is delivered to nobody. Only a
/// captured frame showed which of the two it was, so the byte range is
/// pinned here rather than left to be rediscovered the same way.
#[test]
fn a_depth_frames_opening_tag_is_read_after_the_marker() {
    // Two bytes, the 0x80 marker, then the tag in three.
    let body = [0x00, 0x01, 0x80, 0x11, 0x22, 0x33, 0xff];
    assert_eq!(FarmState::header_stag(&body), Some(0x11_22_33));

    // The marker also arrives as 0x00, and the tag is in the same place.
    let zero_marker = [0x00, 0x01, 0x00, 0x11, 0x22, 0x33];
    assert_eq!(FarmState::header_stag(&zero_marker), Some(0x11_22_33));

    // Short of a whole tag, the frame names nothing rather than a value
    // built out of whatever follows it.
    assert_eq!(FarmState::header_stag(&[0x00, 0x01, 0x80, 0x11, 0x22]), None);
}

fn tag_values(tags: &[(u32, String)], tag: u32) -> Vec<&str> {
    tags.iter().filter(|(t, _)| *t == tag).map(|(_, v)| v.as_str()).collect()
}

/// The server routes a market-data subscription by SecurityType and
/// Exchange even when a conId is supplied. Describing every contract as a
/// SMART-routed common stock makes the server ack only the trade leg of a
/// futures subscription, so bid/ask never arrives.
#[test]
fn conid_subscribe_describes_the_actual_contract() {
    let fut = build_conid_subscribe_tags(true, 1, 2, 793356225, "CME", "FUT", 0, "T");
    assert_eq!(tag_values(&fut, 167), ["FUT", "FUT"], "SecurityType must say FUT");
    assert_eq!(tag_values(&fut, 207), ["CME", "CME"], "Exchange must say CME");

    // Both legs of the realtime fan-out are requested: 442 bid/ask, 443 last.
    assert_eq!(tag_values(&fut, 264), ["442", "443"]);
    assert_eq!(tag_values(&fut, 262), ["1", "2"]);
    assert_eq!(tag_values(&fut, 146), ["2"]);
}

/// Stocks keep the exact wire shape they had before: SMART maps to BEST and
/// STK to CS, so this path is unchanged for equities. Pinned as the whole
/// ordered tag list rather than the two mapped tags, so a reordering or a
/// dropped field is caught here too.
#[test]
fn conid_subscribe_is_unchanged_for_stocks() {
    let stk = build_conid_subscribe_tags(true, 1, 2, 265598, "SMART", "STK", 0, "T");
    assert_eq!(
        stk,
        vec![
            (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
            (fix::TAG_SENDING_TIME, "T".to_string()),
            (263, "1".to_string()),
            (146, "2".to_string()),
            (262, "1".to_string()),
            (6008, "265598".to_string()),
            (207, "BEST".to_string()),
            (167, "CS".to_string()),
            (264, "442".to_string()),
            (6088, "Socket".to_string()),
            (9830, "1".to_string()),
            (9839, "1".to_string()),
            (262, "2".to_string()),
            (6008, "265598".to_string()),
            (207, "BEST".to_string()),
            (167, "CS".to_string()),
            (264, "443".to_string()),
            (6088, "Socket".to_string()),
            (9830, "1".to_string()),
            (9839, "1".to_string()),
        ],
    );

    let delayed = build_conid_subscribe_tags(false, 1, 2, 265598, "SMART", "STK", 3, "T");
    assert_eq!(
        delayed,
        vec![
            (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
            (fix::TAG_SENDING_TIME, "T".to_string()),
            (263, "1".to_string()),
            (146, "1".to_string()),
            (262, "1".to_string()),
            (6008, "265598".to_string()),
            (207, "BEST".to_string()),
            (167, "CS".to_string()),
            (264, "1".to_string()),
            (6088, "Socket".to_string()),
            (9830, "1".to_string()),
            (9839, "1".to_string()),
            (9887, "3".to_string()),
        ],
    );
}

/// Subscribing by conId alone is a supported shape: `Contract` defaults
/// both descriptive fields to empty and the in-tree benchmark relies on it.
/// Those callers receive a smart-routed stock from the two literals;
/// sending an empty SecurityType and Exchange would draw a partial ack.
#[test]
fn conid_subscribe_falls_back_when_the_contract_is_not_described() {
    let bare = build_conid_subscribe_tags(true, 1, 2, 265598, "", "", 0, "T");
    assert_eq!(tag_values(&bare, 167), ["CS", "CS"]);
    assert_eq!(tag_values(&bare, 207), ["BEST", "BEST"]);

    let described = build_conid_subscribe_tags(true, 1, 2, 265598, "SMART", "STK", 0, "T");
    assert_eq!(bare, described, "an undescribed conId keeps the smart-routed stock shape");
}

/// Non-realtime modes collapse to a single TOP subscription carrying 9887.
#[test]
fn conid_subscribe_collapses_to_one_entry_when_not_realtime() {
    let delayed = build_conid_subscribe_tags(false, 7, 8, 265598, "SMART", "STK", 3, "T");
    assert_eq!(tag_values(&delayed, 262), ["7"], "only the first req id is used");
    assert_eq!(tag_values(&delayed, 264), ["1"]);
    assert_eq!(tag_values(&delayed, 146), ["1"]);
    assert_eq!(tag_values(&delayed, 9887), ["3"], "delayed mode must be carried");

    let realtime = build_conid_subscribe_tags(true, 7, 8, 265598, "SMART", "STK", 0, "T");
    assert!(tag_values(&realtime, 9887).is_empty(), "realtime carries no 9887");
}

/// Every entry must be self-contained: the server reads conId per entry.
#[test]
fn each_entry_carries_its_own_conid() {
    let fut = build_conid_subscribe_tags(true, 1, 2, 793356225, "CME", "FUT", 0, "T");
    assert_eq!(tag_values(&fut, 6008), ["793356225", "793356225"]);

    let counts: HashMap<u32, usize> =
        fut.iter().fold(HashMap::new(), |mut m, (t, _)| { *m.entry(*t).or_insert(0) += 1; m });
    for tag in [262, 6008, 207, 167, 264, 6088, 9830, 9839] {
        assert_eq!(counts[&tag], 2, "tag {tag} must appear once per entry");
    }
}
mod stale_ack_tests {
    use super::super::*;
    use crate::engine::context::Context;

    /// A `35=Q` in flight when the unsubscribe goes out resolves its request
    /// id before the slot can be reclaimed. Resolving afterwards would bind its
    /// server tag and minTick onto whichever contract took the slot, scaling
    /// that contract's prices by the previous one's tick size.
    #[test]
    fn a_late_ack_for_an_unsubscribed_request_is_ignored() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let mut hb = HeartbeatState::new();
        let instrument = context.market.register(756733);

        farm.send_mktdata_subscribe(
            756733, "SPY", "SMART", "STK", "", 0.0, "", "", instrument, 0,
            &mut None, &mut hb,
        );
        let pending: Vec<u32> = farm.md_req_to_instrument.iter().map(|(r, _)| *r).collect();
        assert!(!pending.is_empty(), "the subscribe must register at least one request");

        farm.send_mktdata_unsubscribe(instrument, &mut None, &mut hb);

        for req_id in pending {
            assert!(
                !farm.md_req_to_instrument.iter().any(|(r, _)| *r == req_id),
                "request {req_id} must not resolve after its unsubscribe",
            );
        }
    }
}
mod price_scaling_tests {
    use super::super::*;
    use crate::bridge::SharedState;
    use crate::engine::context::Context;
    use crate::protocol::tick_decoder;

    fn push(bits: &mut Vec<u8>, val: u64, n: usize) {
        for i in (0..n).rev() {
            bits.push(((val >> i) & 1) as u8);
        }
    }

    /// One 35=P body carrying a single extended entry, framed as the farm
    /// connection delivers it. The extended header carries a full byte width,
    /// which is how a magnitude large enough to overflow the price scaling
    /// arrives from the wire.
    fn framed_extended(server_tag: u32, tick_type: u64, byte_width: u64, value: u64) -> Vec<u8> {
        let mut bits: Vec<u8> = Vec::new();
        push(&mut bits, 0, 1);
        push(&mut bits, server_tag as u64, 31);
        push(&mut bits, 31, 5); // extended sentinel
        push(&mut bits, 0, 1);  // has_more
        push(&mut bits, 0, 2);  // raw width, ignored for extended
        push(&mut bits, tick_type, 8);
        push(&mut bits, byte_width, 8);
        push(&mut bits, 0, 1);  // sign
        push(&mut bits, value, (byte_width * 8 - 1) as usize);

        let byte_count = bits.len().div_ceil(8);
        let mut payload = vec![0u8; byte_count];
        for (i, &b) in bits.iter().enumerate() {
            if b == 1 {
                payload[i >> 3] |= 1 << (7 - (i & 7));
            }
        }
        let mut tick_payload = Vec::with_capacity(2 + byte_count);
        tick_payload.push((bits.len() >> 8) as u8);
        tick_payload.push((bits.len() & 0xFF) as u8);
        tick_payload.extend_from_slice(&payload);

        let body_len = 5 + tick_payload.len() + 15;
        let mut msg = format!("8=O\x019={body_len}\x01").into_bytes();
        msg.extend_from_slice(b"35=P\x01");
        msg.extend_from_slice(&tick_payload);
        msg.extend_from_slice(b"\x018349=AABBCCDD\x01");
        msg
    }

    /// A magnitude the price scaling cannot represent must leave the previous
    /// quote standing. Wrapping it publishes an arbitrary price — the probe
    /// for this test produces -1000000, a negative price indistinguishable
    /// downstream from a real quote.
    #[test]
    fn a_price_that_cannot_be_scaled_does_not_replace_the_quote() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let shared = SharedState::new();
        let id = context.market.register(756733);
        context.market.register_server_tag(7, id);
        context.market.set_min_tick(id, 0.01);

        farm.handle_tick_data(
            &framed_extended(7, tick_decoder::O_LAST_PRICE, 2, 15_000),
            &mut context, &shared, &None,
        );
        let good = context.market.quote(id).last;
        assert!(good > 0, "the ordinary tick must land");

        farm.handle_tick_data(
            &framed_extended(7, tick_decoder::O_LAST_PRICE, 8, u64::MAX >> 1),
            &mut context, &shared, &None,
        );
        assert_eq!(
            context.market.quote(id).last, good,
            "an unrepresentable price must be dropped, leaving the last good quote",
        );
    }

    /// A frame the venue sent for a deep in-the-money call, byte for byte.
    /// Nothing here is constructed: a wrong alignment does not produce a price
    /// that decomposes into the other two fields by accident.
    #[test]
    fn the_venue_states_an_option_model() {
        const FRAME: &[u8] = &[0x7e, 0xf7, 0x20, 0x01, 0x40, 0x57, 0x04, 0x41, 0xc8, 0xf2, 0xf3, 0x45, 0x3f, 0xef, 0xfc, 0x3a, 0xab, 0x98, 0x37, 0xb3, 0x3f, 0x12, 0xf3, 0x0c, 0x1b, 0xcf, 0xac, 0xe7, 0x3f, 0x53, 0x13, 0xaf, 0x03, 0xfc, 0x00, 0x00, 0xbf, 0xa0, 0x60, 0x85, 0xf4, 0x8d, 0x38, 0x00, 0x40, 0x0d, 0x23, 0xdb, 0x03, 0xb8, 0xf5, 0x14, 0x40, 0x71, 0x7c, 0xb2, 0x05, 0x82, 0x74, 0xf0, 0x3f, 0xf0, 0x07, 0x27, 0xcf, 0x01, 0x13, 0xef, 0x40, 0x73, 0x7f, 0x52, 0x20, 0x00, 0x00, 0x00, 0x3f, 0x9f, 0xf2, 0x61, 0x35, 0xdd, 0x42, 0xd9, 0x40, 0x2e, 0x2b, 0xd8, 0x8e, 0x99, 0xfa, 0xb0, 0x3f, 0x1e, 0x54, 0x91, 0xb1, 0x1c, 0x9a, 0x6c, 0xbe, 0xf5, 0x34, 0xf6, 0xa2, 0xc8, 0x61, 0xb4, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3f, 0xb5, 0x32, 0x2a, 0x5c, 0xf4, 0xd4];
        let c = super::super::decode_greeks(FRAME).expect("the payload is stated valid");
        assert!((c.opt_price - 92.066_515_195_137_14).abs() < 1e-9, "{c:?}");
        assert!((c.delta - 0.999_539_694_925_024_9).abs() < 1e-12, "deep in the money: {c:?}");
        assert!((c.gamma - 0.000_072_286_237_766_827_99).abs() < 1e-15, "{c:?}");
        assert!((c.vega - 0.001_164_360_917_698_559_2).abs() < 1e-15, "{c:?}");
        assert!((c.theta - -0.031_986_414_053_434_94).abs() < 1e-12, "{c:?}");
        assert!((c.und_price - 311.957_550_048_828_1).abs() < 1e-9, "{c:?}");
        assert!((c.implied_vol - 0.031_198_042_786_232_037).abs() < 1e-12, "{c:?}");
        // The strike was 220, so the model price sits just above the
        // intrinsic. A mis-read of the layout does not land there.
        let intrinsic = c.und_price - 220.0;
        assert!(c.opt_price > intrinsic, "worth at least its intrinsic: {c:?}");
        assert!(c.opt_price - intrinsic < 1.0, "and barely more, this close to expiry: {c:?}");
        assert_eq!(c.pv_dividend, f64::MAX, "not stated on this tick");
    }

    /// A payload the venue did not mark valid carries no numbers.
    #[test]
    fn an_invalid_option_model_states_nothing() {
        assert!(super::super::decode_greeks(&[0u8; 32]).is_none());
        assert!(super::super::decode_greeks(&[0xff, 0xff, 0xff, 0xfe]).is_none(), "too short to hold one");
    }

    /// A subscription that asks for the option model has to withdraw it too.
    /// Left behind, the venue keeps sending a model for a contract the caller
    /// stopped watching, and nothing holds a request id to stop it by.
    #[test]
    fn cancelling_an_option_withdraws_its_model() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let instrument = context.market
            .try_register_contract(805711629, "AAPL", "OPT", "SMART", "20260821|220|C|100")
            .unwrap();
        let mut conn = None;
        let mut hb = HeartbeatState::new();
        farm.send_mktdata_subscribe(
            805711629, "AAPL", "SMART", "OPT", "20260821", 220.0, "C", "100",
            instrument, 0, &mut conn, &mut hb,
        );
        assert_eq!(farm.greeks_subs.len(), 1, "an option is worth modelling");
        let reqs = &farm.instrument_md_reqs.iter()
            .find(|(id, _)| *id == instrument).expect("its requests").1;
        assert!(reqs.contains(&farm.greeks_subs[0].0), "and the model is one of them, so a cancel finds it");

        farm.send_mktdata_unsubscribe(instrument, &mut conn, &mut hb);
        assert!(farm.greeks_subs.is_empty(), "withdrawn with the rest");
        assert!(farm.instrument_md_reqs.iter().all(|(id, _)| *id != instrument));
    }

    /// Anything without a volatility to imply is not asked to be modelled: the
    /// venue answers such a request with nothing at all.
    #[test]
    fn a_stock_is_not_asked_for_an_option_model() {
        let mut farm = FarmState::new();
        let mut context = Context::new();
        let instrument = context.market
            .try_register_contract(756733, "SPY", "STK", "SMART", "").unwrap();
        farm.send_mktdata_subscribe(
            756733, "SPY", "SMART", "STK", "", 0.0, "", "",
            instrument, 0, &mut None, &mut HeartbeatState::new(),
        );
        assert!(farm.greeks_subs.is_empty());
    }
}
mod trading_status_subscribe_tests {
    use super::super::build_trading_status_subscribe_tags;

    /// The trading status is its own subscription, named by its own tick where
    /// a price subscription names a request type.
    #[test]
    fn the_status_is_asked_for_by_its_own_tick() {
        let tags = build_trading_status_subscribe_tags(7, 756733, "STK", "SMART", "20260810-12:00:00");
        let get = |t: u32| tags.iter().find(|(k, _)| *k == t).map(|(_, v)| v.as_str());
        assert_eq!(get(264), Some("437"), "its own tick, not a request type");
        assert_eq!(get(262), Some("7"), "under the request the prices came under");
        assert_eq!(get(6008), Some("756733"));
    }

    /// It names the contract's own exchange. The option model and the news feed
    /// go by names of their own; everything else is asked for where it trades,
    /// and naming a stand-in here asks a venue that does not list the contract.
    #[test]
    fn it_names_the_exchange_the_contract_trades_on() {
        let tags = build_trading_status_subscribe_tags(1, 1, "STK", "ARCA", "t");
        let venue = tags.iter().find(|(k, _)| *k == 207).map(|(_, v)| v.as_str());
        assert_eq!(venue, Some("ARCA"), "not a stand-in");
    }
}
mod depth_identity_tests {
    use super::super::*;

    /// A subscription registered as the ack path registers one.
    fn acknowledged(farm: &mut FarmState, stag: u32, caller: u32, venue: &str) {
        farm.depth_tag_to_req.push((stag, caller, true, 0.01, 1.0, venue.to_string()));
    }

    /// The venue echoes back the id it was asked under, so an id taken from
    /// the caller cannot be told apart from one this client allocated. Every
    /// book is asked for under an id this client allocated, and mapped back.
    #[test]
    fn a_callers_id_is_never_what_the_venue_is_asked_under() {
        let mut farm = FarmState::new();
        let shared = SharedState::new();
        let mut conn = None;
        let mut hb = HeartbeatState::new();

        // Two callers, numbered as callers number things.
        farm.send_depth_subscribe(1, 756733, "IEX", "", "STK", 10, false, &mut conn, &mut hb, &shared);
        farm.send_depth_subscribe(2, 756733, "ARCA", "", "STK", 10, false, &mut conn, &mut hb, &shared);

        let asked_under: Vec<u32> = farm.depth_fanout_map.iter().map(|(sub, _)| *sub).collect();
        assert_eq!(asked_under.len(), 2, "one subscription each");
        assert_ne!(asked_under[0], asked_under[1], "and each under its own id");
        for (sub, caller) in &farm.depth_fanout_map {
            let venue = farm.depth_fanout_exchange.iter()
                .find(|(s, _)| s == sub)
                .map(|(_, v)| v.as_str())
                .expect("every subscription names the venue it stands on");
            match caller {
                1 => assert_eq!(venue, "IEX"),
                2 => assert_eq!(venue, "ARCA"),
                other => panic!("a caller nobody asked for: {other}"),
            }
        }
    }

    /// The venue answers a second subscription on a contract and venue it is
    /// already streaming with the tag it is already using.
    #[test]
    fn one_venue_stream_reaches_every_caller_subscribed_to_it() {
        let mut farm = FarmState::new();
        acknowledged(&mut farm, 717550, 1, "IEX");
        acknowledged(&mut farm, 717550, 2, "IEX");
        acknowledged(&mut farm, 990000, 3, "ARCA");

        let both = farm.depth_subscribers_of(717550);
        assert_eq!(both.len(), 2, "a level on this tag belongs to both");
        assert_eq!(both[0].0, 1);
        assert_eq!(both[1].0, 2);
        assert!(both.iter().all(|(_, _, venue)| venue == "IEX"));

        let one = farm.depth_subscribers_of(990000);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].0, 3);
    }

    /// A caller that asked for a shallow book is not handed a deep one.
    ///
    /// The depth is not on the wire. The venue sends the levels it has, and the
    /// reference client shows the number the caller asked for — so a caller
    /// that asked for five and was handed every level got a different book from
    /// the one it asked for.
    #[test]
    fn a_book_is_as_deep_as_the_caller_asked() {
        let mut farm = FarmState::new();
        let mut conn = None;
        let mut hb = HeartbeatState::new();
        let shared = SharedState::new();

        farm.send_depth_subscribe(1, 756733, "IEX", "", "STK", 5, false, &mut conn, &mut hb, &shared);
        assert!(farm.within_asked_depth(1, 0), "the top of the book");
        assert!(farm.within_asked_depth(1, 4), "the fifth level");
        assert!(!farm.within_asked_depth(1, 5), "and no deeper");

        // A caller that named no depth is not held to one.
        farm.send_depth_subscribe(2, 756733, "IEX", "", "STK", 0, false, &mut conn, &mut hb, &shared);
        assert!(farm.within_asked_depth(2, 99));

        // Withdrawn, and the depth goes with it rather than outliving the
        // request and applying to whatever reuses the number.
        farm.send_depth_unsubscribe(1, &mut conn, &mut hb);
        assert!(farm.within_asked_depth(1, 99));
    }

    /// A book is asked for once and withdrawn once, and what is withdrawn is
    /// what this client asked under rather than what the caller stated.
    #[test]
    fn withdrawing_a_book_withdraws_what_was_asked_for() {
        let mut farm = FarmState::new();
        let shared = SharedState::new();
        let mut conn = None;
        let mut hb = HeartbeatState::new();

        farm.send_depth_subscribe(7, 756733, "SMART", "", "STK", 10, true, &mut conn, &mut hb, &shared);
        assert_eq!(farm.depth_subs.len(), 1, "a book on no venue is one subscription");
        assert_eq!(farm.depth_fanout_map[0].1, 7, "and it is the caller's");
        assert_ne!(farm.depth_fanout_map[0].0, 7, "asked under an id of ours");

        farm.send_depth_unsubscribe(7, &mut conn, &mut hb);
        assert!(farm.depth_fanout_map.is_empty(), "nothing is left asking");
        assert!(farm.depth_subs.is_empty());
        assert!(farm.depth_fanout_exchange.is_empty());
        assert!(farm.depth_resub_info.is_empty(), "and no reconnect asks again");
    }
}
