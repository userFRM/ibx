//! Compressed FIX message framing for market data connections.

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::{self, Read, Write};

use super::fix::SOH;

fn parse_err(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Wrap a FIX message in compressed framing.
pub fn fixcomp_build(inner_msg: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(inner_msg).unwrap();
    let compressed = encoder.finish().unwrap();

    // Body: 95=<len>\x01 96=<compressed>\x01
    let mut body = Vec::new();
    body.extend_from_slice(format!("95={}\x01", compressed.len()).as_bytes());
    body.extend_from_slice(b"96=");
    body.extend_from_slice(&compressed);
    body.push(SOH);

    // Header: 8=FIXCOMP\x01 9=<body_len>\x01
    let mut msg = Vec::new();
    msg.extend_from_slice(format!("8=FIXCOMP\x019={}\x01", body.len()).as_bytes());
    msg.extend_from_slice(&body);
    msg
}

/// Decompress a compressed message into individual inner messages.
///
/// Returns `Err` if the frame is malformed (no tag 95, bad raw-data-length, etc.)
/// or if the zlib payload fails to inflate. Hot-loop callers should `log::warn!`
/// and skip the frame rather than propagate.
///
/// Inflated content that cannot be framed into a message is warned about and
/// the messages before it are still returned: discarding the whole frame loses
/// what did arrive, and returning the prefix with nothing said loses the rest
/// silently.
pub fn fixcomp_decompress(data: &[u8]) -> io::Result<Vec<Vec<u8>>> {
    let raw = if let Some(idx95) = find_tag(data, b"\x0195=").map(|p| p + 1) {
        let soh = data[idx95..]
            .iter()
            .position(|&b| b == SOH)
            .map(|p| idx95 + p)
            .ok_or_else(|| parse_err("fixcomp: tag 95 has no terminating SOH"))?;
        let raw_len: usize = std::str::from_utf8(&data[idx95 + 3..soh])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| parse_err("fixcomp: tag 95 value is not a usize"))?;
        // Tag 96 begins immediately after the separator ending tag 95, and
        // nowhere else. The client this one replaces requires exactly that
        // position and refuses the frame otherwise; it does not search.
        //
        // Searched instead, a "96=" byte run inside a payload stood in for a
        // tag that was not there, and the payload was then read from the wrong
        // place — inflated from the middle of itself, or from bytes an
        // attacker chose.
        if data.len() < soh + 4 || &data[soh + 1..soh + 4] != b"96=" {
            return Err(parse_err("fixcomp: tag 96 does not follow tag 95"));
        }
        let payload_start = soh + 4;
        let payload_end = payload_start
            .checked_add(raw_len)
            .ok_or_else(|| parse_err("fixcomp: tag 95 length overflows usize"))?;
        if payload_end > data.len() {
            return Err(parse_err("fixcomp: tag 95 length exceeds frame size"));
        }
        &data[payload_start..payload_end]
    } else {
        // Fallback: zlib data starts after second SOH
        let soh1 = data
            .iter()
            .position(|&b| b == SOH)
            .ok_or_else(|| parse_err("fixcomp: no SOH in frame"))?;
        let soh2 = data[soh1 + 1..]
            .iter()
            .position(|&b| b == SOH)
            .map(|p| p + soh1 + 1)
            .ok_or_else(|| parse_err("fixcomp: no second SOH in frame"))?;
        &data[soh2 + 1..]
    };

    let mut decoder = ZlibDecoder::new(raw).take(MAX_INFLATED + 1);
    let mut decompressed = Vec::new();
    if let Err(e) = decoder.read_to_end(&mut decompressed) {
        // On inflate failure, dump the head of the raw zlib payload and the
        // enclosing frame as hex: that is what separates a slicing error, a
        // deflate stream cut mid-message, and genuinely corrupt bytes. A head
        // rather than the whole of each — a frame can sit at the ceiling this
        // reader holds, and a diagnostic built about it must not ask for more
        // memory than the frame that provoked it. The lengths beside the dump
        // say what the dump does not carry.
        let raw_hex = hex_head(raw);
        let unsigned_hex = hex_head(data);
        log::warn!(
            "fixcomp tee: inflate failed ({}); unsigned_len={} raw_payload_len={} raw_hex={} unsigned_hex={}",
            e, data.len(), raw.len(), raw_hex, unsigned_hex,
        );
        return Err(e);
    }
    // Past the ceiling, so what this frame carries cannot be read whole. Told
    // rather than truncated: half a batch of messages read as the whole of one
    // is a fill or an acknowledgement that silently never arrives.
    if decompressed.len() as u64 > MAX_INFLATED {
        return Err(parse_err(
            "fixcomp: a frame inflating past what this client holds for one",
        ));
    }

    let (messages, unread) = split_messages(&decompressed);
    if unread > 0 {
        // The bytes after the last message this could frame. They are a
        // message the venue sent, and reporting nothing about them turns a
        // framing fault into an order ack or routing tag that never arrives.
        let head = &decompressed[decompressed.len() - unread..];
        let head_hex: String =
            head.iter().take(64).map(|b| format!("{b:02x}")).collect();
        log::warn!(
            "fixcomp: {} of {} inflated bytes follow the last message this \
             could frame, after {} message(s); first bytes hex={head_hex}",
            unread, decompressed.len(), messages.len(),
        );
    }
    Ok(messages)
}

/// Return total byte length of a compressed message, or None if incomplete.
pub fn fixcomp_length(data: &[u8]) -> Option<usize> {
    match fixcomp_frame_length(data) {
        FrameLength::Complete(total) => Some(total),
        _ => None,
    }
}

/// What a frame's stated length says about the bytes in hand.
///
/// The three answers are not the same answer. A frame still arriving is worth
/// waiting for; one whose own header cannot be read never becomes complete, and
/// waiting for it holds every frame behind it forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameLength {
    /// The whole frame is here, and this is how long it is.
    Complete(usize),
    /// The header reads, and the rest of the frame has not arrived yet.
    Incomplete,
    /// The header does not read, and no number of further bytes will change
    /// that.
    Unreadable,
}

/// Read a compressed frame's stated length.
pub fn fixcomp_frame_length(data: &[u8]) -> FrameLength {
    // Too short to hold a header yet, which says nothing about whether the
    // header is good.
    if data.len() < 10 {
        return FrameLength::Incomplete;
    }
    let Some(soh1) = data.iter().position(|&b| b == SOH) else {
        // No field separator in what is here. It may still be coming, unless
        // there is already more than a header's worth of it.
        return if data.len() > MAX_HEADER_SCAN {
            FrameLength::Unreadable
        } else {
            FrameLength::Incomplete
        };
    };
    // Immediately after the separator ending tag 8, and nowhere else. This
    // header is a fixed width too, so a "9=" found further along the buffer is
    // one the peer wrote into a payload — and the total it names reaches over
    // whatever is queued behind this frame, taking all of it as part of this
    // one.
    let tag9 = soh1 + 1;
    if data.len() < tag9 + 2 {
        return FrameLength::Incomplete;
    }
    if &data[tag9..tag9 + 2] != b"9=" {
        return FrameLength::Unreadable;
    }
    let Some(soh2) = data[tag9..].iter().position(|&b| b == SOH).map(|p| tag9 + p) else {
        return if data.len() > MAX_HEADER_SCAN {
            FrameLength::Unreadable
        } else {
            FrameLength::Incomplete
        };
    };
    // The length itself. A field that is not a number is not a length, and no
    // further bytes make it one.
    let Some(body_len) = std::str::from_utf8(&data[tag9 + 2..soh2]).ok().and_then(|t| t.parse::<usize>().ok()) else {
        return FrameLength::Unreadable;
    };
    // The length is whatever the peer wrote, so a total that does not fit is
    // a length no frame can have rather than something to add anyway. The two
    // readers on the plain socket guard this for the same reason.
    let Some(total) = soh2.checked_add(1).and_then(|n| n.checked_add(body_len)) else {
        return FrameLength::Unreadable;
    };
    if data.len() < total {
        FrameLength::Incomplete
    } else {
        FrameLength::Complete(total)
    }
}

/// How much one frame is allowed to become once inflated.
///
/// This client's own allocation, not a size the venue states. A compressed
/// frame is small whatever it carries, so without a ceiling one peer's frame
/// can ask this process for every byte it has, and everything waiting behind
/// it — a fill, an acknowledgement — never arrives.
///
/// Sixty-four mebibytes. The largest payload a session has been sent is the
/// calendar's own list of event types at a little under a hundred and eighty
/// kilobytes, so this is some hundreds of times the largest thing seen rather
/// than a figure anything is expected to approach.
///
/// Visible to the connection because the largest frame that may be buffered
/// is the largest frame that may be inflated: one bounds the other.
pub(crate) const MAX_INFLATED: u64 = 64 * 1024 * 1024;

/// How much of a frame is read before its header is given up on.
///
/// This client's own bound rather than a length the venue states. A header
/// opens `8=1<SOH>9=NNNN<SOH>`, which is twelve bytes, and the scan gives it
/// ten times that before deciding the bytes are not a header at all — so a
/// frame still arriving is waited for and a frame that is not one ends rather
/// than growing without limit.
const MAX_HEADER_SCAN: usize = 128;

fn find_tag(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).position(|w| {
        #[cfg(test)]
        tests::SCAN_COMPARISONS.set(tests::SCAN_COMPARISONS.get() + 1);
        w == needle
    })
}

/// How many bytes of a frame a diagnostic dumps, as hex.
///
/// A diagnostic is built about a frame that failed to read, and a frame can
/// sit at the ceiling this reader holds: dumped whole, the diagnostic asks
/// for more memory than the frame that provoked it, twice over. A head and
/// the lengths beside it still separate a slicing error, a stream cut
/// mid-message, and genuinely corrupt bytes.
const HEX_HEAD: usize = 64;

/// The head of some bytes as hex, for a diagnostic.
fn hex_head(bytes: &[u8]) -> String {
    bytes.iter().take(HEX_HEAD).map(|b| format!("{b:02x}")).collect()
}

/// Where a message that states no body length ends: at its checksum, stepping
/// over any length-prefixed block on the way.
///
/// The bound is a message's own tag 9 wherever it states one, because a scan
/// cannot tell this message's checksum from the next one's — one arriving
/// without its own ran on to the following message's and the two were handed
/// up as a single message, with nothing left over to say so. Not everything
/// here states a length, and what does not is still read the way it always
/// was rather than dropped for it.
fn checksum_end(chunk: &[u8]) -> Option<usize> {
    let mut scan = 0;
    let cksum;
    loop {
        let raw_tag = find_tag(&chunk[scan..], b"\x0195=").map(|p| scan + p);
        let ck = find_tag(&chunk[scan..], b"\x0110=").map(|p| scan + p);

        if let (Some(rt), _) = (raw_tag, ck)
            && (ck.is_none() || rt < ck.unwrap()) {
                let after95 = chunk[rt + 4..]
                    .iter()
                    .position(|&b| b == SOH)
                    .map(|p| rt + 4 + p)?;
                let rdl: usize = std::str::from_utf8(&chunk[rt + 4..after95]).ok()?.parse().ok()?;
                // The payload follows tag 95 immediately. Searching beyond
                // it can borrow a later message's block and swallow its reply.
                let tag96 = after95 + 1;
                if !chunk.get(tag96..).is_some_and(|rest| rest.starts_with(b"96=")) {
                    return None;
                }
                // The length is the sender's, and it is read before the bytes
                // it counts have been seen. One that runs past the end is what
                // a message cut inside a block looks like, so it is given up
                // on the way every other unreadable one here is — rather than
                // indexing past the buffer, which takes the whole session down
                // through the panic handler instead of one bad frame.
                scan = match tag96.checked_add(3).and_then(|n| n.checked_add(rdl)) {
                    Some(n) if n <= chunk.len() => n,
                    _ => return None,
                };
                continue;
            }
        cksum = ck;
        break;
    }
    let ck = cksum?;
    let end = chunk[ck + 4..].iter().position(|&b| b == SOH).map(|p| ck + 4 + p)?;
    Some(end + 1)
}

/// Split decompressed content into individual messages.
///
/// Returns the messages read and how many bytes were left unread behind them.
/// Every framing error here stops the scan, so what follows is a message the
/// venue sent and this client did not deliver — the caller says so rather than
/// letting the count come out short in silence.
fn split_messages(buf: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let mut messages = Vec::new();
    let mut pos = 0;

    while pos < buf.len() {
        let remaining = &buf[pos..];

        // The first recognised header ends the search. Looking for each kind
        // separately scans the rest of an ordinary FIX batch for every message.
        let Some(start) = (0..remaining.len()).find(|&i| {
            #[cfg(test)]
            tests::SCAN_COMPARISONS.set(tests::SCAN_COMPARISONS.get() + 1);
            remaining[i..].starts_with(b"8=FIX.") || remaining[i..].starts_with(b"8=O\x01")
        }) else { break };
        let chunk = &remaining[start..];

        if chunk.starts_with(b"8=O\x01") {
            // The reader the transport frames this header with, asked
            // again rather than written again. Written again here, it
            // looked for the first `9=` past the header instead of at
            // it and took a length out of whatever tag carried those
            // two characters.
            match super::connection::binary_msg_length(chunk) {
                // Still arriving, so what is here is the caller's to
                // keep until the rest of it lands.
                Some(total) if total > chunk.len() => break,
                Some(total) => {
                    messages.push(chunk[..total].to_vec());
                    pos += start + total;
                }
                // Not a header this can frame, and no later byte makes
                // it one. Stepped over rather than given up on.
                None => pos += start + 1,
            }
        } else {
            let total = match super::connection::fix_msg_length(chunk) {
                Some(total) if total > chunk.len() => break,
                Some(total)
                    if super::connection::trailer_is_where_the_length_says(
                        chunk, total,
                    ) =>
                {
                    Some(total)
                }
                // States a length and does not end where it says. No
                // later byte reconciles those two, so it is stepped
                // over the way an unreadable one is.
                Some(_) => None,
                // States no length at all, which some of these do.
                None => match checksum_end(chunk) {
                    Some(end) => Some(end),
                    None => break,
                },
            };
            match total {
                Some(total) => {
                    messages.push(chunk[..total].to_vec());
                    pos += start + total;
                }
                // Given up on instead of stepped over, a fragment at
                // the front hid every message behind it — and the
                // opening burst of a session begins with one.
                None => pos += start + 1,
            }
        }
    }

    (messages, buf.len() - pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::fix::{fix_build, fix_parse};

    /// A diagnostic built about a frame that failed to read must not ask for
    /// more memory than the frame that provoked it: a frame can sit at the
    /// ceiling this reader holds, and dumped whole the diagnostic doubles it.
    #[test]
    fn a_diagnostic_is_bounded_by_the_frame_it_is_about() {
        let frame = vec![0xABu8; 1 << 20];
        let dumped = hex_head(&frame);
        assert_eq!(dumped.len(), HEX_HEAD * 2, "two hex digits a byte, no more");
        assert!(dumped.starts_with(&"ab".repeat(4)));
    }

    /// A stated length no frame can have is not a length to add.
    ///
    /// The two readers on the plain socket guard the same addition. Added
    /// unchecked here it aborts where overflow is checked, and where it is not
    /// it wraps to a small total that frames the stream from an offset the peer
    /// chose. Framing runs on raw socket bytes, so the peer needs nothing but
    /// the socket.
    #[test]
    fn a_stated_length_that_cannot_fit_is_not_a_frame() {
        let huge = format!("8=FIXCOMP\x019={}\x01body", usize::MAX);
        assert!(matches!(
            fixcomp_frame_length(huge.as_bytes()),
            FrameLength::Unreadable
        ));
    }

    /// A raw-data length that counts more bytes than followed it drops the
    /// frame rather than reading past the end of it.
    ///
    /// The length is stated ahead of the bytes it counts, so a frame cut inside
    /// one carries a length the buffer cannot satisfy. Read as given it indexes
    /// past the end, which takes the session down through the panic handler —
    /// where every caller of this plainly means to drop the one frame.
    #[test]
    fn a_raw_data_length_past_the_end_drops_the_frame() {
        let mut msg = Vec::new();
        msg.extend_from_slice(b"8=FIX.4.2\x0135=A\x0195=99999\x0196=AB\x0110=000\x01");
        let (messages, leftover) = split_messages(&msg);
        assert!(messages.is_empty(), "a frame that cannot be read is not a message");
        assert_eq!(leftover, msg.len(), "and every byte of it is still unconsumed");

        // The same shape with a length the bytes do satisfy is still read.
        let mut whole = Vec::new();
        whole.extend_from_slice(b"8=FIX.4.2\x0135=A\x0195=2\x0196=AB\x0110=000\x01");
        let (messages, _) = split_messages(&whole);
        assert_eq!(messages.len(), 1, "a length the frame satisfies is followed");
    }

    /// A missing payload tag cannot be supplied by another field or message.
    #[test]
    fn a_raw_data_tag_must_follow_its_length() {
        for misplaced in [
            b"58=96=AB\x0110=000\x01".as_slice(),
            b"10=000\x018=FIX.4.2\x0135=B\x0195=2\x0196=AB\x0110=000\x01",
        ] {
            let mut batch = b"8=FIX.4.2\x0135=B\x0195=2\x01".to_vec();
            batch.extend_from_slice(misplaced);
            let (messages, unread) = split_messages(&batch);
            assert!(messages.is_empty(), "a displaced block is not a message: {messages:?}");
            assert_eq!(unread, batch.len(), "no later message supplies the missing tag");
        }
    }

    /// A message that loses its checksum does not take the next one with it.
    ///
    /// Ended by a scan for the first `<SOH>10=` in what remained, a message
    /// arriving without its own ran on to the following message's and the two
    /// were handed up as one — and nothing was left over to say a message had
    /// gone missing, because the scan had consumed both. Its own stated length
    /// is what ends it.
    #[test]
    fn a_message_without_its_checksum_does_not_swallow_the_next() {
        // 9=14 counts `35=A\x0158=FIRST\x01`, and no checksum follows it.
        let mut buf = Vec::new();
        buf.extend_from_slice(b"8=FIX.4.2\x019=14\x0135=A\x0158=FIRST\x01");
        buf.extend_from_slice(b"8=FIX.4.2\x019=15\x0135=A\x0158=SECOND\x0110=000\x01");

        let (messages, _) = split_messages(&buf);

        assert!(
            !messages.iter().any(|m| m.windows(8).any(|w| w == b"58=FIRST")),
            "the message with no checksum of its own ran on to the next one's: \
             {messages:?}",
        );
        assert_eq!(messages.len(), 1, "and the whole one behind it is still read");
        assert!(messages[0].windows(9).any(|w| w == b"58=SECOND"), "{messages:?}");
    }

    #[test]
    fn build_structure() {
        let inner = fix_build(&[(35, "0")], 1);
        let comp = fixcomp_build(&inner);
        assert!(comp.starts_with(b"8=FIXCOMP"));
        assert!(comp.windows(3).any(|w| w == b"95="));
        assert!(comp.windows(3).any(|w| w == b"96="));
    }

    #[test]
    fn roundtrip() {
        let inner = fix_build(&[(35, "D"), (55, "MSFT"), (54, "2")], 7);
        let comp = fixcomp_build(&inner);
        let messages = fixcomp_decompress(&comp).unwrap();
        assert_eq!(messages.len(), 1);
        let parsed = fix_parse(&messages[0]);
        assert_eq!(parsed[&35], "D");
        assert_eq!(parsed[&55], "MSFT");
    }

    #[test]
    fn length_complete() {
        let inner = fix_build(&[(35, "0")], 1);
        let comp = fixcomp_build(&inner);
        assert_eq!(fixcomp_length(&comp), Some(comp.len()));
    }

    #[test]
    fn length_incomplete() {
        let inner = fix_build(&[(35, "0")], 1);
        let comp = fixcomp_build(&inner);
        assert_eq!(fixcomp_length(&comp[..10]), None);
    }

    #[test]
    fn roundtrip_large_message() {
        // Build a FIX message with body > 1000 bytes
        let long_value = "X".repeat(1000);
        let inner = fix_build(&[(35, "B"), (58, &long_value)], 1);
        assert!(inner.len() > 1000);

        let comp = fixcomp_build(&inner);
        let messages = fixcomp_decompress(&comp).unwrap();
        assert_eq!(messages.len(), 1);
        let parsed = fix_parse(&messages[0]);
        assert_eq!(parsed[&35], "B");
        assert_eq!(parsed[&58], long_value);
    }

    /// A frame that inflates past what this client holds for one is told,
    /// not truncated.
    ///
    /// A compressed frame is small whatever it carries, so without a ceiling
    /// one peer's frame asks this process for every byte it has and everything
    /// behind it — a fill, an acknowledgement — never arrives. Read short
    /// instead, half a batch of messages would pass as the whole of one.
    #[test]
    fn a_frame_that_inflates_past_the_ceiling_is_refused() {
        use flate2::{Compression, write::ZlibEncoder};

        // Compresses to almost nothing and inflates past the ceiling.
        let huge = vec![b'x'; (MAX_INFLATED + 4096) as usize];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&huge).unwrap();
        let payload = encoder.finish().unwrap();
        assert!(
            (payload.len() as u64) < MAX_INFLATED / 1000,
            "the frame itself is small: {} bytes",
            payload.len(),
        );

        let mut frame = b"8=X\x019=0\x01".to_vec();
        frame.extend_from_slice(&payload);
        let refused = fixcomp_decompress(&frame).expect_err("it is refused");
        assert!(
            refused.to_string().contains("holds for one"),
            "and says why: {refused}",
        );
    }

    #[test]
    fn decompress_multiple_inner_fix_messages() {
        // Compress two FIX messages together into one FIXCOMP wrapper
        let msg1 = fix_build(&[(35, "0")], 1);
        let msg2 = fix_build(&[(35, "D"), (55, "GOOG")], 2);
        let mut combined = msg1.clone();
        combined.extend_from_slice(&msg2);

        let comp = fixcomp_build(&combined);
        let messages = fixcomp_decompress(&comp).unwrap();
        assert_eq!(messages.len(), 2, "expected 2 inner messages");

        let parsed1 = fix_parse(&messages[0]);
        assert_eq!(parsed1[&35], "0");

        let parsed2 = fix_parse(&messages[1]);
        assert_eq!(parsed2[&35], "D");
        assert_eq!(parsed2[&55], "GOOG");
    }

    /// Every framing error inside the inflated content stops the scan, so what
    /// follows is a message the venue sent and this client did not deliver.
    /// The messages before it are still returned — discarding the whole frame
    /// loses what did arrive — and the bytes left over are counted so the loss
    /// is not silent.
    #[test]
    fn the_bytes_no_message_could_be_framed_from_are_counted() {
        let good = fix_build(&[(35, "0")], 1);
        let mut content = good.clone();
        // A second header whose body length reads as nothing, so the scan for
        // its checksum runs off the end of the content.
        content.extend_from_slice(b"8=FIX.4.1\x019=0099\x0135=D\x01");

        let (messages, unread) = split_messages(&content);
        assert_eq!(messages.len(), 1, "the whole message before it is read");
        assert_eq!(
            unread, content.len() - good.len(),
            "and everything after it is reported rather than dropped",
        );

        // Nothing left over when every message frames.
        let (messages, unread) = split_messages(&good);
        assert_eq!(messages.len(), 1);
        assert_eq!(unread, 0);
    }

    #[test]
    fn fixcomp_length_missing_tag9() {
        // A buffer starting with 8=FIXCOMP but no tag 9 → should return None
        let data = b"8=FIXCOMP\x0195=5\x01";
        assert_eq!(fixcomp_length(data), None);
    }

    #[test]
    fn fixcomp_length_body_shorter_than_declared() {
        // Build a valid FIXCOMP, then check that fixcomp_length returns
        // the expected total even if the actual data is shorter (returns None).
        let inner = fix_build(&[(35, "0")], 1);
        let comp = fixcomp_build(&inner);
        let expected_total = fixcomp_length(&comp).unwrap();

        // Truncate: provide only half the body
        let half = comp.len() / 2;
        assert!(half < expected_total);
        assert_eq!(fixcomp_length(&comp[..half]), None);
    }

    #[test]
    fn decompress_corrupt_deflate_returns_err() {
        // Build a valid FIXCOMP frame, then trash the compressed payload so
        // ZlibDecoder fails. The function must return Err rather than panic
        //
        let inner = fix_build(&[(35, "0")], 1);
        let mut comp = fixcomp_build(&inner);
        let tag96 = comp.windows(3).position(|w| w == b"96=").unwrap();
        // Corrupt the first compressed byte (zlib CMF)
        comp[tag96 + 3] ^= 0xFF;
        let err = fixcomp_decompress(&comp).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("corrupt")
            || err.to_string().to_lowercase().contains("invalid"));
    }

    #[test]
    fn decompress_truncated_payload_returns_err() {
        // tag 95 declares length N but the frame is shorter — must not panic.
        let inner = fix_build(&[(35, "0")], 1);
        let comp = fixcomp_build(&inner);
        let truncated = &comp[..comp.len() - 5];
        assert!(fixcomp_decompress(truncated).is_err());
    }

    /// The payload begins immediately after tag 95, and a "96=" further along
    /// the frame is not that tag.
    ///
    /// Searched for instead of read at its position, a "96=" byte run inside
    /// the payload stood in for the tag and the frame was inflated from the
    /// middle of itself — from bytes whoever sent the frame chose. The client
    /// this one replaces reads that one position and refuses the frame when
    /// the tag is not there.
    #[test]
    fn a_payload_marker_further_along_the_frame_is_not_tag_96() {
        let inner = fix_build(&[(35, "A"), (108, "30")], 1);
        let genuine = fixcomp_build(&inner);
        assert_eq!(
            fixcomp_decompress(&genuine).expect("a well-formed frame decompresses"),
            vec![inner],
            "the frame as the venue sends it still reads",
        );

        // The same frame with the tag moved off its position: whatever follows
        // tag 95 now, a "96=" later on does not name the payload.
        let at95 = genuine.windows(4).position(|w| w == b"\x0195=").expect("tag 95") + 1;
        let soh = genuine[at95..].iter().position(|&b| b == SOH).expect("its value ends") + at95;
        let mut displaced = genuine[..=soh].to_vec();
        displaced.extend_from_slice(b"9999=x\x0196=");
        displaced.extend_from_slice(&genuine[soh + 4..]);
        assert!(
            fixcomp_decompress(&displaced).is_err(),
            "a payload named from somewhere else in the frame is not read",
        );
    }

    #[test]
    fn fixcomp_build_produces_valid_zlib() {
        use flate2::read::ZlibDecoder;
        use std::io::Read as _;

        let inner = fix_build(&[(35, "A"), (108, "30")], 1);
        let comp = fixcomp_build(&inner);

        // Extract the zlib data from tag 96
        let tag96_pos = comp
            .windows(3)
            .position(|w| w == b"96=")
            .expect("tag 96 not found");
        let zlib_start = tag96_pos + 3;

        // Find tag 95 value for length
        let tag95_pos = comp
            .windows(3)
            .position(|w| w == b"95=")
            .expect("tag 95 not found");
        let soh_after_95 = comp[tag95_pos + 3..]
            .iter()
            .position(|&b| b == SOH)
            .unwrap()
            + tag95_pos
            + 3;
        let zlib_len: usize = std::str::from_utf8(&comp[tag95_pos + 3..soh_after_95])
            .unwrap()
            .parse()
            .unwrap();

        let zlib_data = &comp[zlib_start..zlib_start + zlib_len];

        // Decompress with raw flate2 to verify it's valid zlib
        let mut decoder = ZlibDecoder::new(zlib_data);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .expect("zlib decompression failed");

        // Decompressed data should equal the original inner FIX message
        assert_eq!(decompressed, inner);
    }

    thread_local! {
        pub(super) static SCAN_COMPARISONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// A batch of ordinary messages has no binary header to find. The search
    /// ends at each message's own header without visiting the messages behind it.
    #[test]
    fn an_ordinary_batch_scans_each_header_once() {
        let messages: Vec<_> = (1..=256)
            .map(|seq| fix_build(&[(35, "8"), (58, "filled")], seq))
            .collect();
        let batch = messages.concat();
        SCAN_COMPARISONS.set(0);
        let (read, unread) = split_messages(&batch);
        let comparisons = SCAN_COMPARISONS.get();
        assert_eq!(read, messages);
        assert_eq!(unread, 0);
        assert!(comparisons <= batch.len(),
            "{comparisons} comparisons revisit a batch of {} bytes", batch.len());
    }

}
