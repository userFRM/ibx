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
        // Unbounded over the rest of the frame, which is a real weakness: a
        // "96=" byte run inside a payload stands in for a tag that is not
        // there, and the payload is then read from the wrong place. Anchoring
        // one byte later does not help — the byte at the separator is the
        // separator, so the search could never have matched there — and
        // nothing here knows where the tags end, so bounding it needs
        // something this does not have.
        let payload_start = if let Some(idx96) = find_tag(&data[soh..], b"96=") {
            soh + idx96 + 3
        } else {
            soh + 1
        };
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
        // On inflate failure, dump the raw zlib payload and the full
        // enclosing frame as hex: that is what separates a slicing error, a
        // deflate stream cut mid-message, and genuinely corrupt bytes.
        let raw_hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let unsigned_hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
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
    let Some(tag9) = find_tag(&data[soh1..], b"9=").map(|p| soh1 + p) else {
        return if data.len() > MAX_HEADER_SCAN {
            FrameLength::Unreadable
        } else {
            FrameLength::Incomplete
        };
    };
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
    data.windows(needle.len()).position(|w| w == needle)
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

        let fix_start = find_tag(remaining, b"8=FIX.");
        let o_start = find_tag(remaining, b"8=O\x01");

        match (fix_start, o_start) {
            (None, None) => break,
            (fix_s, o_s) => {
                // Pick whichever comes first
                let o_first = match (o_s, fix_s) {
                    (Some(_), None) => true,
                    (Some(o), Some(f)) if o < f => true,
                    _ => false,
                };

                if o_first {
                    let o = o_s.unwrap();
                    let chunk = &remaining[o..];
                    // 8=O protocol: length-delimited via tag 9
                    let tag9 = match find_tag(&chunk[4..], b"9=") {
                        Some(p) => 4 + p,
                        None => break,
                    };
                    let soh9 = match chunk[tag9..].iter().position(|&b| b == SOH) {
                        Some(p) => tag9 + p,
                        None => break,
                    };
                    let body_len: usize = match std::str::from_utf8(&chunk[tag9 + 2..soh9]) {
                        Ok(s) => match s.parse() {
                            Ok(n) => n,
                            Err(_) => break,
                        },
                        Err(_) => break,
                    };
                    // Unchecked, a stated length near the width of the type
                    // wraps to zero, an empty message is pushed, and the scan
                    // advances by nothing — the loop over this buffer never
                    // ends.
                    let Some(total) = soh9.checked_add(1).and_then(|n| n.checked_add(body_len))
                    else {
                        break;
                    };
                    if total > chunk.len() {
                        break;
                    }
                    messages.push(chunk[..total].to_vec());
                    pos += o + total;
                } else {
                    let f = fix_start.unwrap();
                    let chunk = &remaining[f..];
                    // Standard FIX: find 10=XXX SOH, skip past raw data blocks
                    let mut scan = 0;
                    let mut cksum = None;
                    loop {
                        let raw_tag = find_tag(&chunk[scan..], b"\x0195=").map(|p| scan + p);
                        let ck = find_tag(&chunk[scan..], b"\x0110=").map(|p| scan + p);

                        if let (Some(rt), _) = (raw_tag, ck)
                            && (ck.is_none() || rt < ck.unwrap()) {
                                // Skip past raw data block
                                let after95 = match chunk[rt + 4..]
                                    .iter()
                                    .position(|&b| b == SOH)
                                    .map(|p| rt + 4 + p)
                                {
                                    Some(p) => p,
                                    None => break,
                                };
                                let rdl: usize =
                                    match std::str::from_utf8(&chunk[rt + 4..after95]) {
                                        Ok(s) => match s.parse() {
                                            Ok(n) => n,
                                            Err(_) => break,
                                        },
                                        Err(_) => break,
                                    };
                                let tag96 = match find_tag(&chunk[after95..], b"96=") {
                                    Some(p) => after95 + p,
                                    None => break,
                                };
                                // The length is the sender's, and it is read
                                // before the bytes it counts have been seen. One
                                // that runs past the end is what a frame cut
                                // mid-block looks like, so the frame is dropped
                                // the way every other unreadable one here is —
                                // rather than indexing past the buffer, which
                                // takes the whole session down through the panic
                                // handler instead of one bad frame.
                                scan = match tag96.checked_add(3).and_then(|n| n.checked_add(rdl)) {
                                    Some(n) if n <= chunk.len() => n,
                                    _ => break,
                                };
                                continue;
                            }
                        cksum = ck;
                        break;
                    }

                    let ck = match cksum {
                        Some(c) => c,
                        None => break,
                    };
                    let end = match chunk[ck + 4..].iter().position(|&b| b == SOH) {
                        Some(p) => ck + 4 + p,
                        None => break,
                    };
                    messages.push(chunk[..end + 1].to_vec());
                    pos += f + end + 1;
                }
            }
        }
    }

    (messages, buf.len() - pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::fix::{fix_build, fix_parse};

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
        msg.extend_from_slice(b"8=FIX.4.2\x019=40\x0135=A\x0195=99999\x0196=AB\x0110=000\x01");
        let (messages, leftover) = split_messages(&msg);
        assert!(messages.is_empty(), "a frame that cannot be read is not a message");
        assert_eq!(leftover, msg.len(), "and every byte of it is still unconsumed");

        // The same shape with a length the bytes do satisfy is still read.
        let mut whole = Vec::new();
        whole.extend_from_slice(b"8=FIX.4.2\x019=40\x0135=A\x0195=2\x0196=AB\x0110=000\x01");
        let (messages, _) = split_messages(&whole);
        assert_eq!(messages.len(), 1, "a length the frame satisfies is followed");
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
}
