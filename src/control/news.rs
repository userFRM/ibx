//! News queries via the data connection.
//!
//! Request format: FIX msg_type=U, 6040=10030, 6118=XML
//! Response format: FIX msg_type=U, 6040=10032, 6118=XML (id echo), 96=binary payload
//!
//! The binary payload in tag 96 is: "200\n" + offset table + ZIP archive.
//! The ZIP contains a single ENTRY file in Java Properties format.

use std::io::Read;

/// FIX tag 6040: the sub protocol.
pub const TAG_SUB_PROTOCOL: u32 = 6040;
/// FIX tag 95: the raw data length.
pub const TAG_RAW_DATA_LENGTH: u32 = 95;
/// FIX tag 96: the raw data.
pub const TAG_RAW_DATA: u32 = 96;

/// Parameters for a historical news request.
#[derive(Debug, Clone)]
pub struct HistoricalNewsRequest {
    /// The name this client gave the query, which the answer echoes.
    pub query_id: String,
    /// The venue's id for the contract.
    pub con_id: u32,
    /// Which providers to ask, separated by the venue's separator.
    pub provider_codes: String,
    /// The start of the window asked for.
    pub start_time: String,
    /// Its end.
    pub end_time: String,
    /// The most rows wanted.
    pub max_results: u32,
}

/// Parameters for a news article request.
#[derive(Debug, Clone)]
pub struct NewsArticleRequest {
    /// The name this client gave the query, which the answer echoes.
    pub query_id: String,
    /// Which provider published it.
    pub provider_code: String,
    /// Its id, for fetching the body.
    pub article_id: String,
}

/// A single news headline parsed from a response.
#[derive(Debug, Clone)]
pub struct NewsHeadline {
    /// When it was published.
    pub time: String,
    /// Which provider published it.
    pub provider_code: String,
    /// Its id, for fetching the body.
    pub article_id: String,
    /// The headline itself.
    pub headline: String,
}

/// URL-encode a string for the `<query>` field.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'*' | b';' | b'\\' | b'=' | b':' | b'@' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            b'"' => out.push_str("%22"),
            _ => {
                out.push('%');
                out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
                out.push(char::from(b"0123456789ABCDEF"[(b & 0x0F) as usize]));
            }
        }
    }
    out
}

/// Build the `<id>` value for a news request.
fn build_news_id(req_num: &str, cmd: &str) -> String {
    format!("{};;NewsQuery;;0;;true;;0;;U", format_args!("{}-{}", req_num, cmd))
}

/// What a historical-news window has to state to be askable.
///
/// The query this client sends names a contract, a set of providers and a row
/// count, and carries no time bounds at all. Accepting a start and an end and
/// dropping them returns the most recent headlines for every request, which
/// read as the ones inside the window asked for.
pub fn validate_news_window(start_time: &str, end_time: &str) -> Result<(), String> {
    if start_time.is_empty() && end_time.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the news query this client sends carries no time bounds, and this \
         request names {}. Leave both empty and bound the answer with \
         total_results, which is what limits it.",
        match (start_time.is_empty(), end_time.is_empty()) {
            (false, false) => format!("{start_time} to {end_time}"),
            (false, true) => format!("a start of {start_time}"),
            _ => format!("an end of {end_time}"),
        },
    ))
}

/// Build the XML query for a historical news request.
pub fn build_historical_news_xml(req: &HistoricalNewsRequest) -> String {
    // The venue joins provider codes with a star where the caller uses a plus.
    let providers_star = req.provider_codes.replace('+', "*");

    // Each entry names the contract and the providers wanted for it. The
    // venue's own word for every provider a session is entitled to is
    // ALL_SUB, which is what stands where the caller named none. The codes
    // used to ride in `url_key` instead, which is not a slot for them.
    let wanted = if providers_star.is_empty() { "ALL_SUB" } else { &providers_star };
    let tags = format!("@@{}:{wanted}@", req.con_id);

    // The identity fields are an authorisation pair the news service issues,
    // not something a client invents. Empty strings are valid where none is
    // held, and the same slot carries a refusal back — the vendor keeps both
    // as one pair, the unheld one two empty strings and the refused one an
    // empty key beside the words that say so. Asking for headlines, it sends
    // the unheld pair.
    //
    // A key was composed here out of the provider codes the caller asked for,
    // directly beneath the sentence saying a client does not invent one.
    let query_raw = format!(
        "conid_count=\"{count}\";\
         total_count=\"{count}\";\
         ip=\"\";\
         fingerprint=\"\";\
         cmd=\"history\";\
         tags=\"{tags}\";\
         url_key=\"\";\
         ",
        count = req.max_results,
        tags = tags,
    );

    let id = build_news_id(&req.query_id, "history");
    let query_encoded = url_encode(&query_raw);

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <NewsHMDSQuery>\
         <id>{id}</id>\
         <exchange>NEWS</exchange>\
         <secType>*</secType>\
         <source>API</source>\
         <needTotalValue>false</needTotalValue>\
         <wholeDays>false</wholeDays>\
         <delay>auto</delay>\
         <query>{query_encoded}</query>\
         <currency>*</currency>\
         </NewsHMDSQuery>\
         </ListOfQueries>",
    )
}

/// Build the XML query for a news article request.
pub fn build_article_request_xml(req: &NewsArticleRequest) -> String {
    let query_raw = format!(
        "eId=\"{article_id}*{provider}\";\
         ip=\"\";\
         fingerprint=\"\";\
         cmd=\"article_file\";\
         url_key=\"\";\
         ",
        article_id = req.article_id,
        provider = req.provider_code,
    );

    let id = build_news_id(&req.query_id, "article_file");
    let query_encoded = url_encode(&query_raw);

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListOfQueries>\
         <NewsHMDSQuery>\
         <id>{id}</id>\
         <exchange>NEWS</exchange>\
         <secType>*</secType>\
         <source>API</source>\
         <needTotalValue>false</needTotalValue>\
         <wholeDays>false</wholeDays>\
         <delay>auto</delay>\
         <query>{query_encoded}</query>\
         <currency>*</currency>\
         </NewsHMDSQuery>\
         </ListOfQueries>",
    )
}

/// Extract the query ID from a news response XML (tag 6118).
pub fn parse_news_response_id(xml: &str) -> Option<String> {
    crate::control::xml::tag(xml, "id").map(|s| s.to_string())
}

/// Extract the first file from a ZIP archive embedded in raw bytes.
/// Finds PK\x03\x04 magic and extracts using deflate.
/// Handles both sized entries and streamed entries (data descriptor with csize=0).
fn extract_zip_entry(data: &[u8]) -> Option<Vec<u8>> {
    // Find ZIP local file header magic
    let pk_pos = data.windows(4).position(|w| w == b"PK\x03\x04")?;
    let zip_data = &data[pk_pos..];

    // Parse local file header (30 bytes minimum)
    if zip_data.len() < 30 {
        return None;
    }
    let compression = u16::from_le_bytes([zip_data[8], zip_data[9]]);
    let compressed_size = u32::from_le_bytes([zip_data[18], zip_data[19], zip_data[20], zip_data[21]]) as usize;
    let filename_len = u16::from_le_bytes([zip_data[26], zip_data[27]]) as usize;
    let extra_len = u16::from_le_bytes([zip_data[28], zip_data[29]]) as usize;

    let data_start = 30 + filename_len + extra_len;
    if data_start > zip_data.len() {
        return None;
    }

    let entry_data = if compressed_size > 0 && data_start + compressed_size <= zip_data.len() {
        &zip_data[data_start..data_start + compressed_size]
    } else {
        // Data descriptor: csize=0 in header; feed all remaining data to decoder.
        // The deflate decoder will stop when the stream ends naturally.
        &zip_data[data_start..]
    };

    match compression {
        0 => Some(entry_data.to_vec()), // stored
        8 => {
            // deflate — feed all remaining data; decoder stops at stream end.
            // Use chunked reads to tolerate trailing garbage after the deflate stream.
            let mut decoder = flate2::read::DeflateDecoder::new(entry_data);
            let mut out = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match decoder.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => out.extend_from_slice(&buf[..n]),
                    Err(_) => break, // trailing data after deflate stream
                }
            }
            if out.is_empty() { None } else { Some(out) }
        }
        _ => None,
    }
}

/// Read one `h:N=` row into a headline.
///
/// Split out so the reading can be checked on its own: the row is found by its
/// time rather than counted from the left, because a headline may carry the
/// delimiter.
pub(crate) fn parse_news_payload_rows(value: &str) -> Option<NewsHeadline> {
    let fields: Vec<&str> = value.split('|').collect();
    let at_time = fields.iter().position(|f| is_a_time(f)).unwrap_or(1);
    let headline_raw = fields[..at_time].join("|");
    let mut parts: Vec<&str> = Vec::with_capacity(fields.len() - at_time + 1);
    parts.push(headline_raw.as_str());
    parts.extend_from_slice(&fields[at_time..]);
    if parts.len() < 6 {
        return None;
    }
    let raw = parts[0];
    let headline = match raw.strip_prefix('{').and_then(|_| raw.find('}')) {
        Some(i) => raw[i + 1..].to_string(),
        None => raw.to_string(),
    };
    Some(NewsHeadline {
        headline: unescape_venue_characters(&headline),
        time: parts[1].to_string(),
        article_id: parts[2].to_string(),
        provider_code: parts[5].to_string(),
    })
}

/// Whether a field is shaped like the time the venue stamps a headline with.
///
/// `yyyy-MM-dd HH:mm:ss.S`, which is what the reference client looks for when
/// it works out where the headline ends.
fn is_a_time(field: &str) -> bool {
    let b = field.as_bytes();
    b.len() >= 19
        && b.len() <= 24
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b' '
        && b[13] == b':'
        && b[16] == b':'
        && b[..4].iter().all(u8::is_ascii_digit)
}

/// Undo the escaping a Java properties file carries.
///
/// The venue writes headlines into one, so a colon, an equals or a hash in a
/// value arrives with a backslash before it. Only the first two were undone,
/// and a hash reached callers still wearing it — which is every headline
/// carrying a character the venue escapes, because it writes those as
/// `&#xNN;` and the hash in that is escaped in turn.
fn unescape_properties(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            // A backslash before anything else is the escape rather than part
            // of the value: a colon, an equals, a hash, a space, a backslash.
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Put back the characters the venue escapes out of a headline.
///
/// It writes anything outside plain ASCII as `&#xNN;`, a byte at a time, so a
/// word with an accent in it arrives as a run of them. The reference client
/// reads them back — it carries the pattern for exactly this. Left alone, a
/// caller reading a headline in any language but English got the escapes.
fn unescape_venue_characters(text: &str) -> String {
    // Nothing escaped, nothing to put back.
    if !text.contains('&') {
        return text.to_string();
    }
    let raw = text.as_bytes();
    let mut bytes: Vec<u8> = Vec::with_capacity(raw.len());
    let mut at = 0usize;
    while at < raw.len() {
        let taken = raw[at..].starts_with(b"&#x")
            .then(|| raw[at + 3..].iter().position(|b| *b == b';'))
            .flatten()
            .filter(|end| *end > 0)
            .and_then(|end| {
                let hex = std::str::from_utf8(&raw[at + 3..at + 3 + end]).ok()?;
                let value = u32::from_str_radix(hex, 16).ok()?;
                Some((value, 3 + end + 1))
            });
        match taken {
            // Up to a byte it is one, which is how the venue writes a
            // character outside ASCII: several of them, one per byte, and
            // reading each as a character of its own turns a word into
            // nonsense. Past a byte it is the character itself.
            Some((value, width)) if value <= 0xFF => {
                bytes.push(value as u8);
                at += width;
            }
            Some((value, width)) => {
                match char::from_u32(value) {
                    Some(c) => {
                        let mut buf = [0u8; 4];
                        bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                    None => bytes.extend_from_slice(&raw[at..at + width]),
                }
                at += width;
            }
            None => { bytes.push(raw[at]); at += 1; }
        }
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // And the five it names rather than numbers.
    if text.contains('&') {
        return text
            .replace("&apos;", "'")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
    }
    text
}

/// Parse historical news headlines from the binary payload in tag 96.
///
/// Format: "200\n" + offset_table + ZIP(ENTRY with Java Properties)
/// Properties: h:0..h:N = pipe-delimited headlines, has_more flag.
/// Headline:
/// `{headline}|{time}|{articleId}|{status}|{hasContent}|{providerCode}|{conIds...}`
pub fn parse_news_payload(raw: &[u8]) -> (Vec<NewsHeadline>, bool) {
    let mut headlines = Vec::new();
    let mut has_more = false;

    // Strip "200\n" status prefix, then decode j.c codec
    let after_status = if raw.starts_with(b"200\n") { &raw[4..] } else { raw };
    let decoded = jc_decode(after_status);

    let entry = match extract_zip_entry(&decoded) {
        Some(e) => e,
        None => {
            // The venue answered and this could not read the answer. An
            // empty result reads to a caller as a contract with no news
            // rather than as headlines that arrived and could not be read.
            log::warn!(
                "news: {} bytes arrived and no archive entry could be read from them,                  so no headline in them reaches the caller",
                raw.len(),
            );
            return (headlines, has_more);
        }
    };

    let text = String::from_utf8_lossy(&entry);

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue; // Skip Java Properties comments and blank lines
        }
        // Java Properties: unescape `\:` → `:`, `\=` → `=`
        let unescaped = unescape_properties(line);
        if unescaped.starts_with("has_more=") {
            // The venue states this as a number and the reference client
            // compares it to "1". Matched against the word instead, a page
            // that said there was more said nothing this could read, and a
            // caller paging an archive stopped at the first page.
            has_more = unescaped.rsplit('=').next().map(str::trim) == Some("1");
            continue;
        }
        // Match h:N= keys
        if let Some(eq_pos) = unescaped.find('=') {
            let key = &unescaped[..eq_pos];
            if key.starts_with("h:") && key[2..].parse::<u32>().is_ok() {
                let value = &unescaped[eq_pos + 1..];
                if let Some(row) = parse_news_payload_rows(value) {
                    headlines.push(row);
                }
            }
        }
    }

    (headlines, has_more)
}

/// Read an interleaved big-endian int32 from j.c codec format.
/// Each int32 is stored as 8 bytes: `[b3, 0x00, b2, 0x00, b1, 0x00, b0, 0x00]`
fn jc_read_int32(buf: &[u8], offset: usize) -> u32 {
    if offset + 7 >= buf.len() {
        return 0;
    }
    (buf[offset] as u32) << 24
        | (buf[offset + 2] as u32) << 16
        | (buf[offset + 4] as u32) << 8
        | buf[offset + 6] as u32
}

/// Reverse the j.c newline-escape codec.
///
/// The codec replaces `0x0a` bytes in the binary with `0x00` and stores
/// their positions in an interleaved int32 offset table.
///
/// Layout after "200\n" status prefix:
/// - Bytes 0–7: count of offsets (interleaved int32)
/// - Bytes 8–8*(count+1)-1: offset entries (each 8 bytes)
/// - Bytes 8*(count+1)+: modified binary payload (ZIP data)
pub fn jc_decode(buf: &[u8]) -> Vec<u8> {
    if buf.len() < 8 {
        return buf.to_vec();
    }
    let count = jc_read_int32(buf, 0) as usize;
    let header_size = (count + 1) * 8;
    if header_size > buf.len() {
        return buf.to_vec();
    }
    let mut out = buf[header_size..].to_vec();
    for i in 0..count {
        let pos = jc_read_int32(buf, (i + 1) * 8) as usize;
        if pos < out.len() {
            out[pos] = 0x0A;
        }
    }
    out
}

/// Decode a signed-byte-encoded array from an article response.
/// Each char in the string represents a signed byte value.
/// Decode a signed-byte-encoded array from an article response.
/// Format: `{length}#{signed_byte}{signed_byte}...` where each signed byte
/// is a decimal integer prefixed by `+` or `-`, e.g. `1725#+31-117+8+0`.
fn decode_byte_array(s: &str) -> Vec<u8> {
    // Strip length prefix before '#'
    let data = match s.find('#') {
        Some(pos) => &s[pos + 1..],
        None => s,
    };
    let mut result = Vec::new();
    let mut num_start = 0;
    let bytes = data.as_bytes();
    let mut i = 0;
    while i <= bytes.len() {
        let at_delim = i == bytes.len()
            || (i > num_start && (bytes[i] == b'+' || bytes[i] == b'-'));
        if at_delim {
            let token = &data[num_start..i];
            if let Ok(val) = token.parse::<i16>() {
                result.push(val as u8);
            }
            num_start = i;
        }
        i += 1;
    }
    result
}

/// Parse a news article body from the binary payload in tag 96.
/// Returns (article_type, article_text).
pub fn parse_article_payload(raw: &[u8]) -> Option<(i32, String)> {
    let after_status = if raw.starts_with(b"200\n") { &raw[4..] } else { raw };
    let decoded = jc_decode(after_status);
    let entry = extract_zip_entry(&decoded)?;
    let text = String::from_utf8_lossy(&entry);

    let mut body_encoded: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        let unescaped = unescape_properties(line);
        if let Some(val) = unescaped.strip_prefix("b=") {
            body_encoded = Some(val.to_string());
        }
    }

    if let Some(encoded) = &body_encoded {
        let compressed = decode_byte_array(encoded);
        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
        let mut article = String::new();
        if decoder.read_to_string(&mut article).is_ok() {
            return Some((0, article));
        }
    }

    // Fallback: return raw properties text
    Some((1, text.into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The query carries no time bounds. Accepting a start and an end and
    /// dropping them returns the most recent headlines for every request,
    /// which read as the ones inside the window asked for.
    #[test]
    fn a_news_window_this_query_cannot_carry_is_refused() {
        assert!(validate_news_window("", "").is_ok());
        assert!(validate_news_window("2026-01-01", "2026-03-01").is_err());
        assert!(validate_news_window("2026-01-01", "").is_err());
        assert!(validate_news_window("", "2026-03-01").is_err());
    }

    #[test]
    fn historical_news_xml_structure() {
        let req = HistoricalNewsRequest {
            query_id: "1".to_string(),
            con_id: 265598,
            provider_codes: "BRFG+BRFUPDN".to_string(),
            start_time: String::new(),
            end_time: String::new(),
            max_results: 10,
        };
        let xml = build_historical_news_xml(&req);
        assert!(xml.contains("<ListOfQueries>"));
        assert!(xml.contains("<NewsHMDSQuery>"));
        assert!(xml.contains("<id>1-history;;NewsQuery;;0;;true;;0;;U</id>"));
        assert!(xml.contains("<exchange>NEWS</exchange>"));
        assert!(xml.contains("<query>"));
        assert!(xml.contains("cmd="));
        assert!(xml.contains("265598"));
        assert!(xml.contains("BRFG*BRFUPDN"));
    }

    #[test]
    fn article_request_xml_structure() {
        let req = NewsArticleRequest {
            query_id: "2".to_string(),
            provider_code: "BRFG".to_string(),
            article_id: "BRFG$12345678".to_string(),
        };
        let xml = build_article_request_xml(&req);
        assert!(xml.contains("<ListOfQueries>"));
        assert!(xml.contains("<NewsHMDSQuery>"));
        assert!(xml.contains("<id>2-article_file;;NewsQuery;;0;;true;;0;;U</id>"));
        assert!(xml.contains("BRFG%2412345678*BRFG"));
    }

    #[test]
    fn parse_news_response_id_basic() {
        let xml = r#"<NewsResponse><id>1-history;;NewsQuery;;0;;true;;0;;U</id></NewsResponse>"#;
        assert_eq!(
            parse_news_response_id(xml),
            Some("1-history;;NewsQuery;;0;;true;;0;;U".to_string())
        );
        assert_eq!(parse_news_response_id("<other>no id here</other>"), None);
    }

    #[test]
    fn parse_news_payload_from_zip() {
        // Build a minimal ZIP with a single ENTRY containing news properties
        let props = b"h:0=Earnings beat|2026-01-15 10:00:00|BRFG$100|200|1|BRFG|265598\n\
                       h:1=Guidance raised|2026-01-16 11:00:00|BRFG$101|200|1|BRFG|265598\n\
                       has_more=false\n";
        let zip = build_test_zip(b"ENTRY", props);
        // Prefix with "200\n" status
        let mut payload = b"200\n".to_vec();
        payload.extend_from_slice(&zip);

        let (headlines, has_more) = parse_news_payload(&payload);
        assert_eq!(headlines.len(), 2);
        assert_eq!(headlines[0].headline, "Earnings beat");
        assert_eq!(headlines[0].time, "2026-01-15 10:00:00");
        assert_eq!(headlines[0].article_id, "BRFG$100");
        assert_eq!(headlines[0].provider_code, "BRFG");
        assert_eq!(headlines[1].headline, "Guidance raised");
        assert!(!has_more);
    }

    #[test]
    fn parse_news_payload_strips_metadata_prefix() {
        let props = b"h:0={A=1;B=2}Earnings beat|2026-01-15 10:00:00|BRFG$100|200|1|BRFG|265598\n\
                       h:1={}Empty meta|2026-01-15 11:00:00|BRFG$101|200|1|BRFG|265598\n\
                       h:2=Plain headline|2026-01-15 12:00:00|BRFG$102|200|1|BRFG|265598\n";
        let zip = build_test_zip(b"ENTRY", props);
        let mut payload = b"200\n".to_vec();
        payload.extend_from_slice(&zip);

        let (headlines, _) = parse_news_payload(&payload);
        assert_eq!(headlines.len(), 3);
        assert_eq!(headlines[0].headline, "Earnings beat");
        assert_eq!(headlines[1].headline, "Empty meta");
        assert_eq!(headlines[2].headline, "Plain headline");
    }

    /// The venue states this as a number, and the reference client compares
    /// it to "1". The fixture used to write the word `true`, which is not a
    /// value the reference client can read as more — so the parser was being
    /// checked against a payload only it would ever produce.
    #[test]
    fn parse_news_payload_has_more() {
        let more = |stated: &[u8]| {
            let mut props = b"h:0=Test|2026-01-01|ART1|200|1|DJ-N|1234\n".to_vec();
            props.extend_from_slice(stated);
            let zip = build_test_zip(b"ENTRY", &props);
            let mut payload = b"200\n".to_vec();
            payload.extend_from_slice(&zip);
            let (headlines, has_more) = parse_news_payload(&payload);
            assert_eq!(headlines.len(), 1);
            has_more
        };
        assert!(more(b"has_more=1\n"), "the number the venue states");
        assert!(!more(b"has_more=0\n"), "and the number that says there is not");
        assert!(!more(b""), "nothing stated is not more");
    }

    /// Build a minimal ZIP archive with one stored (uncompressed) file.
    fn build_test_zip(name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut zip = Vec::new();
        let crc = crc32(data);
        // Local file header
        zip.extend_from_slice(b"PK\x03\x04");      // signature
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes());  // flags
        zip.extend_from_slice(&0u16.to_le_bytes());  // compression: stored
        zip.extend_from_slice(&0u16.to_le_bytes());  // mod time
        zip.extend_from_slice(&0u16.to_le_bytes());  // mod date
        zip.extend_from_slice(&crc.to_le_bytes());   // crc32
        zip.extend_from_slice(&(data.len() as u32).to_le_bytes()); // compressed
        zip.extend_from_slice(&(data.len() as u32).to_le_bytes()); // uncompressed
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes()); // name len
        zip.extend_from_slice(&0u16.to_le_bytes());  // extra len
        zip.extend_from_slice(name);
        zip.extend_from_slice(data);
        zip
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFFFFFF;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                if crc & 1 != 0 { crc = (crc >> 1) ^ 0xEDB88320; }
                else { crc >>= 1; }
            }
        }
        !crc
    }
}

#[cfg(test)]
mod escaping_tests {
    use super::{unescape_properties, unescape_venue_characters};

    /// The venue writes headlines into a properties file, so a hash inside one
    /// arrives escaped — and every character it writes as `&#xNN;` carries a
    /// hash. Undoing only the colon and the equals left every such headline
    /// wearing its own escaping.
    #[test]
    fn a_hash_inside_a_headline_is_unescaped_like_the_rest() {
        assert_eq!(unescape_properties(r"d&\#xC3;&\#xB3;lares"), "d&#xC3;&#xB3;lares");
        assert_eq!(unescape_properties(r"a\: b\= c"), "a: b= c");
    }

    /// Captured from the venue: it writes a character outside plain ASCII as
    /// one escape per BYTE, and a character above a byte as itself.
    #[test]
    fn the_characters_the_venue_escapes_are_put_back() {
        assert_eq!(unescape_venue_characters("d&#xC3;&#xB3;lares"), "dólares");
        assert_eq!(unescape_venue_characters("d&#xE2;&#x80;&#x99;un"), "d’un");
        assert_eq!(unescape_venue_characters("Nvidia&#x2019;s"), "Nvidia’s");
        assert_eq!(unescape_venue_characters("l&apos;IA"), "l'IA");
        assert_eq!(
            unescape_venue_characters("plain ASCII passes"), "plain ASCII passes",
            "and nothing is done where there is nothing to do",
        );
    }
}

#[cfg(test)]
mod row_tests {
    use super::parse_news_payload_rows;

    /// A headline may carry the delimiter. Counted from the left, the rest of
    /// it is read as the time, the time as the article and so on — every field
    /// wrong and none of them empty. Found by the time instead, it cannot
    /// shift.
    #[test]
    fn a_headline_carrying_the_delimiter_does_not_shift_the_rest() {
        let row = "Apple|Google merger talks|2026-08-24 09:11:24.0|ART1|200|1|DJ-N|1234";
        let got = parse_news_payload_rows(row).expect("a row is read");
        assert_eq!(got.headline, "Apple|Google merger talks");
        assert_eq!(got.time, "2026-08-24 09:11:24.0");
        assert_eq!(got.article_id, "ART1");
        assert_eq!(got.provider_code, "DJ-N");
    }

    /// The ordinary shape still reads as it did.
    #[test]
    fn a_headline_without_one_reads_as_before() {
        let row = "Plain headline|2026-08-24 09:11:24.0|ART2|200|1|BZ|99";
        let got = parse_news_payload_rows(row).expect("a row is read");
        assert_eq!(got.headline, "Plain headline");
        assert_eq!(got.time, "2026-08-24 09:11:24.0");
        assert_eq!(got.provider_code, "BZ");
    }
}
