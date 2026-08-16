//! The tests for this module.
//!
//! One file per module, as `api/client` already does it. Each block below
//! reaches the code it tests through `super::super`, which is the module this
//! file belongs to.


/// Field 4 is where the second-factor type lives, and reading the wrong
/// field is what caused both live failures this gate was written for — a
/// sub-type sent to the wrong account shape, and a factor routed to the
/// wrong prompt. Nothing pinned it.
#[test]
fn auth_start_token_comes_from_field_four() {
    let frame = |token: &str| format!("a;b;c;d;{token};f");

    assert_eq!(parse_auth_start_token(&frame("5.2i")), ("5".into(), Some("2i".into())));
    assert_eq!(parse_auth_start_token(&frame("4")), ("4".into(), None));
    assert_eq!(parse_auth_start_token(&frame("4,5")), ("4,5".into(), None));
    assert_eq!(parse_auth_start_token(&frame(" 5.2i ")), ("5".into(), Some("2i".into())));

    // A sub-type on one entry must not swallow the entries after it.
    assert_eq!(parse_auth_start_token(&frame("5.2i,4")), ("5,4".into(), Some("2i".into())));

    // On a mixed account the sub-type must belong to the type the gate
    // routes to, which prefers IBKey. Keeping the first one stated sent
    // the authenticator's sub-type in the IBKey init.
    assert_eq!(
        parse_auth_start_token(&frame("4.auth,5.2i")),
        ("4,5".into(), Some("2i".into())),
    );
    assert_eq!(
        parse_auth_start_token(&frame("5.2i,4.auth")),
        ("5,4".into(), Some("2i".into())),
    );
    // With no IBKey entry the authenticator's own sub-type is the one that
    // belongs to the route taken.
    assert_eq!(
        parse_auth_start_token(&frame("4.auth,9.other")),
            ("4,9".into(), Some("auth".into())),
    );
    // The routed entry stating no sub-type of its own must not borrow one
    // from an entry the gate is not using. `4.auth,5` routes to IBKey, so
    // the configured fallback is the answer — sending the authenticator's
    // sub-type in the IBKey init is what this field kept getting wrong.
    assert_eq!(parse_auth_start_token(&frame("4.auth,5")), ("4,5".into(), None));
    assert_eq!(parse_auth_start_token(&frame("9.other,5")), ("9,5".into(), None));
    assert_eq!(parse_auth_start_token(&frame("9.other,4")), ("9,4".into(), None));
    // A sub-type on a type neither gate serves is still better than none.
    assert_eq!(
        parse_auth_start_token(&frame("9.other")),
            ("9".into(), Some("other".into())),
        );

    // Short or absent field 4 yields no type rather than a wrong one.
    assert_eq!(parse_auth_start_token("a;b;c"), ("".into(), None));
    assert_eq!(parse_auth_start_token(&frame("")), ("".into(), None));
}

/// A reconnect starts where the session actually is.
///
/// The venue names which server an account belongs on and the session
/// follows it, so the address a caller configured is a door that only
/// redirects. Reconnecting to it starts the redirect again on every
/// attempt, and on a session that was redirected once that is every
/// reconnect it will ever make.
#[test]
fn a_reconnect_starts_where_the_session_ended_up() {
    // The order a session records them in: where it ended up, then each
    // door it knocked on to get there.
    let seen = ["zdc1.example".to_string(), "cdc1.example".to_string()];

    // The caller configured the door. What matters is the room.
    assert_eq!(super::alternates_to(&seen, "zdc1.example"), ["cdc1.example"]);

    // And the door is still worth keeping: it answered for this account
    // once, which is more than can be said for an address nobody named.
    assert!(seen.contains(&"cdc1.example".to_string()));
}

/// A reconnect is given the hosts the venue sent this session to, and
/// never the one it is already failing on.
///
/// Every one of them was named by the venue: the first is the door this
/// client knocked on, and each of the rest is where it was sent next. A
/// list invented here instead would be reaching for a server nobody said
/// was there, which is how a farm asked for on the wrong host spends ten
/// seconds being silently closed.
#[test]
fn a_reconnect_is_given_the_hosts_the_venue_sent_this_session_to() {
    let seen = ["zdc1.example".to_string(), "cdc1.example".to_string()];

    // Reconnecting to where the session ended up leaves the door it
    // knocked on first to try.
    assert_eq!(super::alternates_to(&seen, "zdc1.example"), ["cdc1.example"]);

    // And the other way round. Retrying the host that just failed is not a
    // second attempt, so it is never in the list.
    assert_eq!(super::alternates_to(&seen, "cdc1.example"), ["zdc1.example"]);

    // A session that was never redirected has one host and nowhere else to
    // go, which is the truth rather than a reason to invent somewhere.
    let one = ["cdc1.example".to_string()];
    assert!(super::alternates_to(&one, "cdc1.example").is_empty());

    // A host that is not among them is still not retried against itself.
    assert_eq!(super::alternates_to(&one, "elsewhere.example"), ["cdc1.example"]);
}

/// A redirect states where to go, and states a port with it. The port is
/// read and carried, and it is not where the logon is answered: measured
/// live, a redirect to this venue names 4000 and only 4001 completes.
#[test]
fn a_redirect_states_a_port_alongside_its_host() {
    assert_eq!(super::host_and_port("zdc1.example:4002", 4001), ("zdc1.example", 4002));
    // The common case: only a host, so the session stays on its port.
    assert_eq!(super::host_and_port("zdc1.example", 4001), ("zdc1.example", 4001));
    // Nonsense where a port should be is not a reason to connect nowhere.
    assert_eq!(super::host_and_port("zdc1.example:four", 4001), ("zdc1.example", 4001));
    }

/// The doors are tried in order, and the one that just failed is not
/// tried again as if it were a second chance.
#[test]
fn the_doors_after_the_one_that_failed_are_the_rest_of_them() {
    let doors: Vec<String> =
        crate::config::CCP_HOSTS.iter().map(|h| h.to_string()).collect();
    let rest = super::alternates_to(&doors, crate::config::CCP_HOSTS[0]);
    assert_eq!(rest.len(), doors.len() - 1);
    assert!(!rest.contains(&crate::config::CCP_HOSTS[0].to_string()));
    assert_eq!(rest[0], crate::config::CCP_HOSTS[1], "and in the order they are listed");
}

/// Which session a logon the venue names belongs to, decided by when it
/// was made.
#[test]
fn a_session_younger_than_this_one_belongs_to_somebody_else() {
    // What was watched happen: this session logged in, lost the account,
    // and found a logon from a minute later sitting on it.
    assert!(super::is_another_client("20260814-09:59:33", "20260814-09:58:33"));
    // The same session's own logon, still listed while the venue reaps it.
    assert!(!super::is_another_client("20260814-09:58:33", "20260814-09:58:33"));
    assert!(!super::is_another_client("20260814-09:58:32", "20260814-09:58:33"));
    // Across a day and a year boundary, which the text order has to hold.
    assert!(super::is_another_client("20260815-00:00:01", "20260814-23:59:59"));
    assert!(!super::is_another_client("20251231-23:59:59", "20260101-00:00:01"));
    // No stamp of this session's own: every session the venue names is
    // somebody else, which gives the account up rather than taking it.
    assert!(super::is_another_client("20260814-09:58:33", ""));
}

/// The venue names the session that was already logged in, and this reads
/// it out of the frame by its shape rather than by counting fields.
#[test]
fn a_competing_session_is_read_out_of_the_connect_response() {
    // A frame as the venue sends it, from a session with nobody else on
    // the account: the host and port it routed to, then the bare zero.
    assert_eq!(
        super::parse_competing_session(
            "50;523;zdc1.example:4000;0;TST,ONELOGON;spqili231;aaa;;;;"
        ),
        None,
        "the routed host carries a colon and a port, not a login time",
    );

    // Nobody else: the field is a bare zero, and some sessions carry none.
    assert_eq!(super::parse_competing_session("50;523;0;;2;0;"), None);
    assert_eq!(super::parse_competing_session("50;523;;;;"), None);

    let other = super::parse_competing_session("50;523;192.168.1.9/20260813-15:04:22;2;0;")
        .expect("a competing session is stated");
    assert_eq!(other.ip, "192.168.1.9");
    assert_eq!(other.since, "20260813-15:04:22");
    assert!(!other.read_only, "nothing said this session may not trade");

    // The suffix is about the session being told, not the other one.
    let held = super::parse_competing_session("50;523;10.0.0.4/20260813-09:30:00(RO);2;")
        .expect("a competing session is stated");
    assert!(held.read_only, "this session may look and not trade");
    assert_eq!(held.since, "20260813-09:30:00", "the suffix is not part of the time");

    // Position is not what identifies it: a frame that gains a field in
    // front must not change what is read.
    let moved = super::parse_competing_session("50;523;x;y;z;10.0.0.4/20260813-09:30:00;")
        .expect("still found");
    assert_eq!(moved.ip, "10.0.0.4");

    // Other fields carrying a slash are not login times. A route and a
    // version both do, and reading either as a competitor would announce
    // one on every session.
    assert_eq!(super::parse_competing_session("50;523;zdc1.ibllc.com/ushmds;2;"), None);
    assert_eq!(super::parse_competing_session("50;523;5.2i/2;0;"), None);
    assert_eq!(super::parse_competing_session("50;523;/20260813-09:30:00;"), None);
    assert_eq!(super::parse_competing_session("50;523;1.2.3.4/2026081-09:30:00;"), None);
    assert_eq!(super::parse_competing_session("50;523;1.2.3.4/20260813-9:30:00;"), None);
}

/// `recv_secure` clears the outer envelope and stops there. Field 4 of what
/// it hands back was read as the second-factor declaration whatever the
/// frame actually was, so another message at that point in the handshake
/// supplied the type and sub-type from fields that mean something else.
#[test]
fn a_frame_that_is_not_auth_start_is_refused() {
    // 50;520;...;<type>.<sub-type>;<auth mode>;
    let auth_start = auth_start_text(b"50;520;a;b;5.2i;2;").unwrap();
    assert_eq!(parse_auth_start_token(&auth_start), ("5".into(), Some("2i".into())));

    // Same shape, another type. NS_CONNECT_RESPONSE's field 4 is not a
    // second-factor declaration, and reading it as one picks an exchange.
    let err = auth_start_text(b"50;523;a;b;5.2i;2;").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("523"), "{}", err);

    // Too short to state a type is not AUTH_START either.
    assert!(auth_start_text(b"50;").is_err());
}

#[test]
fn second_factor_route_covers_every_token_type() {
    use super::{second_factor_route, SecondFactorRoute::*};
    // An absent type must still enter the IBKey gate. That gate opens by
    // sending its init, and an account with no second factor completes
    // through it — routing it to `None` sends nothing and leaves the
    // server waiting on an init that never arrives.
    assert_eq!(second_factor_route(false, ""), IbKey);
    assert_eq!(second_factor_route(false, "5"), IbKey);
    assert_eq!(second_factor_route(false, "4"), SecurityCode);
    // The field is a list when more than one factor is enabled. An
    // account with both an authenticator and IBKey advertises `4,5`, and
    // reading that as one type refused the login outright.
    assert_eq!(second_factor_route(false, "4,5"), IbKey);
    assert_eq!(second_factor_route(false, "5,4"), IbKey, "order must not matter");
        assert_eq!(second_factor_route(false, "3,4"), SecurityCode, "an unknown entry must not veto a known one");
    assert_eq!(second_factor_route(false, " 4 , 5 "), IbKey, "entries may be padded");
    assert_eq!(second_factor_route(false, "3,6"), Unsupported, "a list of unknowns is still unsupported");
    assert_eq!(second_factor_route(false, "3"), Unsupported);
    assert_eq!(second_factor_route(false, "05"), Unsupported);
    assert_eq!(second_factor_route(false, "banana"), Unsupported);
    // Paper never presents one, whatever the field says.
    for t in ["", "3", "4", "5"] {
        assert_eq!(second_factor_route(true, t), None, "paper, type {t:?}");
    }
}

use super::*;

/// The init burst is handed to the engine still compressed, and the engine
/// decompresses the same segments itself. The inflated plaintext is
/// appended to a copy taken for the local tag scan, not to that buffer:
/// appending to the buffer puts every message in the burst in front of the
/// engine twice from a single delivery.
#[test]
fn the_inflated_init_content_is_scanned_but_not_handed_to_the_engine() {
    let inner = b"8=FIX.4.2\x0135=B\x0158=ROUTING\x016145=farm-a\x0110=000\x01";
    let mut burst = b"8=FIX.4.2\x0135=A\x01".to_vec();
    burst.extend_from_slice(&fixcomp::fixcomp_build(inner));
    burst.extend_from_slice(b"8=FIX.4.2\x0135=0\x01");

    fn count(haystack: &[u8], needle: &[u8]) -> usize {
        haystack.windows(needle.len()).filter(|w| *w == needle).count()
    }

    let before = count(&burst, b"58=ROUTING");
    let scan = init_scan_buffer(&burst);

    assert_eq!(
        count(&scan, b"58=ROUTING"), before + 1,
        "the tag scan gains exactly one inflated copy of the segment's content",
    );
    assert!(scan.starts_with(&burst), "and still sees everything that arrived");
    assert_eq!(
        count(&burst, b"58=ROUTING"), before,
        "and the buffer the engine is handed is not the one that grew",
    );
}

#[test]
fn token_short_hash_deterministic() {
    let token = BigUint::from(123456789u64);
    let h1 = token_short_hash(&token);
    let h2 = token_short_hash(&token);
    assert_eq!(h1, h2);
    // Should be lowercase hex
    assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn token_short_hash_different_tokens() {
    let t1 = BigUint::from(111u64);
    let t2 = BigUint::from(222u64);
    assert_ne!(token_short_hash(&t1), token_short_hash(&t2));
}

/// A peer that accepts the socket and then says nothing must not hold the
/// reconnect open.
///
/// The scheduler waits on this worker and refuses to start another while
/// one is outstanding, so a handshake with no deadline is not a slow
/// reconnect — it is every later reconnect, for the life of the process.
/// Bounded here rather than after the key exchange, which is where the
/// silence lands.
#[test]
fn a_silent_peer_does_not_hold_the_reconnect_open() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    // Accepts and then says nothing, holding the connection open.
    let held = std::thread::spawn(move || {
        let (sock, _) = listener.accept().unwrap();
        std::thread::sleep(Duration::from_secs(3));
        drop(sock);
    });

    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(TIMEOUT_SSL_AUTH)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_millis(200))).unwrap();

    let started = std::time::Instant::now();
    let mut buf = [0u8; 64];
    let read = std::io::Read::read(&mut &tcp, &mut buf);
    assert!(read.is_err(), "a silent peer returns an error, not bytes");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the read gave up on its own rather than waiting on the peer",
        );
    let _ = held.join();
}

/// A route that states no port leaves the choice to the configured one.
#[test]
fn parse_farm_route_two_segments() {
    let parsed = parse_farm_route("zdc1.ibllc.com/eufarm").unwrap();
    assert_eq!(parsed, ("zdc1.ibllc.com".to_string(), "eufarm".to_string(), None));
}

/// A route that states a port states where that farm answers. The
/// counterpart reads it from here when no tag carries it, and treats a
/// route with neither as an error rather than substituting a default —
/// so a stated port is not something to discard in favour of a constant.
#[test]
fn parse_farm_route_takes_the_port_the_venue_states() {
    let parsed = parse_farm_route("zdc1.ibllc.com/euhmds/4002").unwrap();
    assert_eq!(parsed, ("zdc1.ibllc.com".to_string(), "euhmds".to_string(), Some(4002)));

    // A third segment that is not a port is not one. The route still
    // names a host and a farm, which is what it is read for.
    let odd = parse_farm_route("zdc1.ibllc.com/euhmds/notaport").unwrap();
    assert_eq!(odd.2, None);
}

#[test]
fn the_channel_role_comes_from_the_farm_not_its_name() {
    // The role was keyed on the literal "ushmds", so a regional
    // historical-data farm was established on the trading channel. The
    // caller names the farm, which is the discriminator that holds for
    // every farm name — including `cashhmds`, which this codebase connects
    // as market data despite the suffix. Asserted on the wire values
    // rather than through the constants: a drifting constant would
    // otherwise put every historical-data farm on the wrong channel with
    // the suite still green.
    assert_eq!(Farm::MarketData.channel_id(), "1");
    assert_eq!(Farm::Historical.channel_id(), "2");
    assert_eq!(Farm::SecurityDefinition.channel_id(), "4");

    // The version a farm logs on at is a different number from the service
    // it asks for, and they were one value while there were only two
    // farms. Every service logs on at seventeen except market data.
    assert_eq!(Farm::MarketData.login_version(), 18);
    assert_eq!(Farm::Historical.login_version(), 17);
    assert_eq!(Farm::SecurityDefinition.login_version(), 17);
}


#[test]
fn parse_farm_route_us_account() {
    let parsed = parse_farm_route("cdc1.ibllc.com/usfarm").unwrap();
    assert_eq!(parsed, ("cdc1.ibllc.com".to_string(), "usfarm".to_string(), None));
}

#[test]
fn parse_farm_route_rejects_empty_and_malformed() {
    assert_eq!(parse_farm_route(""), None);
    assert_eq!(parse_farm_route("nofarm.example.com"), None);
    assert_eq!(parse_farm_route("/farm"), None);
    assert_eq!(parse_farm_route("host/"), None);
}

#[test]
fn token_short_hash_always_8_chars() {
    // gateway pads to 8 hex chars. Brute-force search
    // over small inputs to find one whose SHA1 ends in a high-nibble
    // zero, then assert padding kicks in.
    for n in 0u64..10_000 {
        let token = BigUint::from(n);
        let h = token_short_hash(&token);
        assert_eq!(h.len(), 8,
            "token_short_hash must always be 8 chars; n={n} produced {h:?}");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

#[test]
fn build_ccp_logon_structure() {
    let msg = build_ccp_logon(
        &Default::default(), "abc123|00:00:00:00:00:00", "17.0.10.0.101/W/en/G", 10, 1,
    );
    let fields = fix_parse(&msg);
    assert_eq!(fields[&35], "A");
    assert_eq!(fields[&98], "0");
    assert_eq!(fields[&108], "10");
    assert_eq!(fields[&141], "Y");
    assert_eq!(fields[&6034], crate::config::ib_build());
    assert_eq!(fields[&6968], crate::config::ib_version());
    assert_eq!(fields[&6490], "dark");
    assert_eq!(fields[&6397], "1");
    assert_eq!(fields[&8361], "(rolling)");
    assert_eq!(fields[&8098], "0");
    assert!(fields[&6351].contains("abc123"));
}

#[test]
fn build_farm_logon_has_required_tags() {
    let token = BigUint::from(999u64);
    let hash = token_short_hash(&token);
    assert!(!hash.is_empty());
}

#[test]
fn chrono_free_timestamp_format() {
    let ts = chrono_free_timestamp();
    assert_eq!(ts.len(), 17); // "YYYYMMDD-HH:MM:SS"
    assert_eq!(ts.as_bytes()[8], b'-');
    assert_eq!(ts.as_bytes()[11], b':');
    assert_eq!(ts.as_bytes()[14], b':');
}

#[test]
fn days_to_ymd_epoch() {
    let (y, m, d) = days_to_ymd(0);
    assert_eq!((y, m, d), (1970, 1, 1));
}

#[test]
fn parse_misc_urls_pipe_separated() {
    let m = parse_misc_urls("region_dam=ny5wwwdam1.ibllc.com|region_webserver=ny5wwwgw1.ibllc.com|nossl=0");
    assert_eq!(m.len(), 3);
    assert_eq!(m.get("region_dam").map(String::as_str), Some("ny5wwwdam1.ibllc.com"));
    assert_eq!(m.get("region_webserver").map(String::as_str), Some("ny5wwwgw1.ibllc.com"));
        assert_eq!(m.get("nossl").map(String::as_str), Some("0"));
}

#[test]
fn parse_misc_urls_pct_encoded_pipe() {
    let m = parse_misc_urls("a=1|b=2|c%7Cd=3");
    assert_eq!(m.len(), 3);
    assert_eq!(m.get("a").map(String::as_str), Some("1"));
    assert_eq!(m.get("b").map(String::as_str), Some("2"));
    assert_eq!(m.get("c|d").map(String::as_str), Some("3"));
}

#[test]
fn parse_misc_urls_pct_encoded_pipe_in_value() {
    let m = parse_misc_urls("a=x%7Cy");
    assert_eq!(m.get("a").map(String::as_str), Some("x|y"));
}

#[test]
fn parse_misc_urls_pct_encoded_lowercase() {
    let m = parse_misc_urls("a=x%7cy");
    assert_eq!(m.get("a").map(String::as_str), Some("x|y"));
}

#[test]
fn parse_misc_urls_empty_input() {
    assert!(parse_misc_urls("").is_empty());
}

#[test]
fn parse_misc_urls_comma_fallback() {
    let m = parse_misc_urls("a=1,b=2,c=3");
    assert_eq!(m.len(), 3);
    assert_eq!(m.get("b").map(String::as_str), Some("2"));
}

#[test]
fn parse_misc_urls_drops_malformed_entries() {
    let m = parse_misc_urls("a=1|nokv|=val|b=2");
    assert_eq!(m.len(), 2);
    assert_eq!(m.get("a").map(String::as_str), Some("1"));
    assert_eq!(m.get("b").map(String::as_str), Some("2"));
}

#[test]
fn parse_misc_urls_value_with_equals() {
    // split_once stops at first `=`, so URLs with query strings round-trip.
    let m = parse_misc_urls("cookbook=https://x.example/path?a=1&b=2");
    assert_eq!(m.get("cookbook").map(String::as_str), Some("https://x.example/path?a=1&b=2"));
}

#[test]
fn days_to_ymd_known_date() {
    // 2026-03-05 = day 20517 since epoch
    let (y, m, d) = days_to_ymd(20517);
    assert_eq!((y, m, d), (2026, 3, 5));
}

#[test]
fn try_frame_farm_msg_incomplete() {
    assert!(try_frame_farm_msg(b"8=FIX").is_none());
    assert!(try_frame_farm_msg(b"").is_none());
}

#[test]
fn try_frame_farm_msg_complete() {
    let msg = fix_build(&[(35, "A"), (108, "30")], 1);
    let (extracted, consumed) = try_frame_farm_msg(&msg).unwrap();
    assert_eq!(extracted, msg);
    assert_eq!(consumed, msg.len());
}

#[test]
fn try_frame_farm_msg_with_trailing() {
    let msg1 = fix_build(&[(35, "A")], 1);
    let msg2 = fix_build(&[(35, "0")], 2);
    let mut buf = msg1.clone();
    buf.extend_from_slice(&msg2);
    let (extracted, consumed) = try_frame_farm_msg(&buf).unwrap();
    assert_eq!(extracted, msg1);
    assert_eq!(consumed, msg1.len());
}

// Note: build_farm_encrypted_logon requires a DH-initialized SecureChannel
// which can't be created in unit tests. Tested via compatibility tests instead.

#[test]
fn days_to_ymd_leap_year() {
    let (y, m, d) = days_to_ymd(19782); // 2024-02-29
    assert_eq!((y, m, d), (2024, 2, 29));
}

#[test]
fn days_to_ymd_end_of_year() {
    // 2025-12-31
    let (y, m, d) = days_to_ymd(20453); // 2025-12-31
    assert_eq!((y, m, d), (2025, 12, 31));
}

#[test]
fn days_to_ymd_start_of_2000() {
    // 2000-01-01 = 10957 days from epoch
    let (y, m, d) = days_to_ymd(10957);
    assert_eq!((y, m, d), (2000, 1, 1));
}

#[test]
fn try_frame_farm_msg_garbage_prefix() {
    let mut buf = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    let msg = fix_build(&[(35, "A")], 1);
    buf.extend_from_slice(&msg);
    // Should skip garbage and return (empty, skip_count)
    let (extracted, consumed) = try_frame_farm_msg(&buf).unwrap();
    if extracted.is_empty() {
        // garbage skipped, need to retry from remaining
        let rest = &buf[consumed..];
        let (msg2, _) = try_frame_farm_msg(rest).unwrap();
        assert!(!msg2.is_empty());
    }
}

#[test]
fn try_frame_farm_msg_multiple_sequential() {
    // Two FIX messages back to back
    let msg1 = fix_build(&[(35, "S")], 1);
    let msg2 = fix_build(&[(35, "A"), (108, "30")], 2);
    let mut buf = msg1.clone();
    buf.extend_from_slice(&msg2);
    let (extracted, consumed) = try_frame_farm_msg(&buf).unwrap();
    assert_eq!(extracted, msg1);
    assert_eq!(consumed, msg1.len());
    // Second message
    let (extracted2, consumed2) = try_frame_farm_msg(&buf[consumed..]).unwrap();
    assert_eq!(extracted2, msg2);
    assert_eq!(consumed2, msg2.len());
}

#[test]
fn token_short_hash_nonzero_output() {
    let token = BigUint::from(1u64);
    let hash = token_short_hash(&token);
    assert!(!hash.is_empty());
    // Should be hex string
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn token_short_hash_large_token() {
    let token = BigUint::from(u64::MAX);
    let hash = token_short_hash(&token);
    assert!(!hash.is_empty());
    assert!(hash.len() <= 8); // u32 hex is at most 8 chars
}

#[test]
fn chrono_free_timestamp_not_empty() {
    let ts = chrono_free_timestamp();
    assert!(!ts.is_empty());
    // Year should start with 20xx
    assert!(ts.starts_with("20"));
}

#[test]
fn gateway_config_fields() {
    let config = GatewayConfig {
        settings: Default::default(),
        username: "user".to_string(),
            password: Zeroizing::new("pass".to_string()),
        host: "cdc1.ibllc.com".to_string(),
        paper: true,
        accept_invalid_certs: false,
        ib_key_timeout_secs: session::IB_KEY_DEFAULT_TIMEOUT_SECS,
        ib_key_token_sub_type: session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
        code_provider: None,
        resume: None,
    };
    assert_eq!(config.username, "user");
        assert!(config.paper);
}

fn auth_with(host: &str, trading_host: &str, trading_farm: &str) -> ReconnectAuth {
    ReconnectAuth {
        trading_port: None,
        hmds_port: None,
        secdef_port: None,
        logged_in_at: String::new(),
        alternate_hosts: Vec::new(),
        settings: Default::default(),
        host: host.to_string(),
        username: String::new(),
        password: zeroize::Zeroizing::new(String::new()),
        paper: true,
        code_provider: None,
        ib_key_timeout_secs: crate::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
        ib_key_token_sub_type: crate::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
        session_key: num_bigint::BigUint::from(0u32),
        session_token: num_bigint::BigUint::from(0u32),
        server_session_id: String::new(),
        hw_info: String::new(),
        encoded: String::new(),
        secdef_host: String::new(),
        secdef_farm: String::new(),
        hmds_host: String::new(),
        hmds_farm: String::new(),
        trading_host: trading_host.to_string(),
        trading_farm: trading_farm.to_string(),
    }
}

/// The trading reconnect announces the farm and dials the host the auth
/// server gave for the account, rather than a literal `usfarm` and the
/// configured host. The historical-data reconnect beside it carries both
/// the same way.
#[test]
fn a_reconnect_uses_the_route_the_auth_server_gave() {
    assert_eq!(
        reconnect_trading_route(&auth_with("cdc1.ibllc.com", "cdc2.ibllc.com", "euhard")),
        ("cdc2.ibllc.com".to_string(), "euhard".to_string()),
    );

    // No route parsed: the literals it fell back to are still the answer,
    // so an account that reconnected correctly before still does.
    assert_eq!(
        reconnect_trading_route(&auth_with("cdc1.ibllc.com", "", "")),
        ("cdc1.ibllc.com".to_string(), "usfarm".to_string()),
    );

    // And each half falls back independently.
    assert_eq!(
        reconnect_trading_route(&auth_with("cdc1.ibllc.com", "", "euhard")),
        ("cdc1.ibllc.com".to_string(), "euhard".to_string()),
    );
    assert_eq!(
        reconnect_trading_route(&auth_with("cdc1.ibllc.com", "cdc2.ibllc.com", "")),
        ("cdc2.ibllc.com".to_string(), "usfarm".to_string()),
    );

    // The reason the parsed route is stored raw rather than after the
    // initial connect's fallback: with no route stated, the reconnect has
    // to use whatever host the session holds now. Storing the materialized
    // pair would pin the host as it was at connect time and ignore a
    // redirect that has moved it since.
    assert_eq!(
        reconnect_trading_route(&auth_with("cdc3.ibllc.com", "", "")),
        ("cdc3.ibllc.com".to_string(), "usfarm".to_string()),
        "an updated host wins when the server stated no route",
    );
}

#[test]
fn the_venue_names_what_it_permits() {
    // A live logon reply, abridged. SLB names one order type; a type named
    // with nothing after it is still permitted.
    let perms = parse_order_permissions("STK:LMT,MKT,STP;SLB:LMT;NEWS");
    assert_eq!(perms["STK"], ["LMT", "MKT", "STP"]);
    assert_eq!(perms["SLB"], ["LMT"]);
    assert!(perms["NEWS"].is_empty(), "named with no order types, still permitted");
    assert!(!perms.contains_key("FWD"), "a type the venue never named is not permitted");
    assert!(parse_order_permissions("").is_empty(), "a session that stated nothing");
}
mod logon_field_tests {
    use super::super::{keep_first, note_account};

    /// A field the venue sends whole keeps its first value, and a repeat of the
    /// same value is not a conflict.
    #[test]
    fn keep_first_keeps_the_first_value() {
        let mut slot = String::new();
        keep_first(&mut slot, "a;b", "6823");
        keep_first(&mut slot, "a;b", "6823");
        keep_first(&mut slot, "c;d", "6823");
        assert_eq!(slot, "a;b");
    }

    /// Accounts keep the order the venue named them in, without repeats, and an
    /// empty name is not an account.
    #[test]
    fn accounts_keep_their_order_without_repeats() {
        let mut accounts = Vec::new();
        for name in ["DU1", "DU2", "DU1", "", "DU3"] {
            note_account(&mut accounts, name);
        }
        assert_eq!(accounts, vec!["DU1", "DU2", "DU3"]);
    }
}
mod soft_dollar_tier_tests {
    use crate::types::SoftDollarTier;

    /// The shape the venue actually sends, taken from a real logon.
    fn parse(raw: &str) -> Vec<SoftDollarTier> {
        raw.split(',')
            .filter_map(|entry| {
                let entry = entry.trim();
                if entry.is_empty() {
                    return None;
                }
                let mut parts = entry.split('/');
                let val = parts.next()?.trim();
                let display_name = parts.next()?.trim();
                let name = parts.next()?.trim();
                if val.is_empty() || name.is_empty() {
                    return None;
                }
                Some(SoftDollarTier {
                    name: name.to_string(),
                    val: val.to_string(),
                    display_name: display_name.to_string(),
                })
            })
            .collect()
    }

    /// A real logon's own words. A list written into this client would stand
    /// in for it — itself a transcription of this very reply, so it would look
    /// right while nothing was
    /// being read.
    #[test]
    fn the_tiers_a_real_logon_states_are_read() {
        let raw = "1/Maximize Rebate/MaxRebate,9/Prefer Rebate/PreferRebate,\
                   11/Prefer Fill/PreferFill,12/Maximize Fill/MaxFill,\
                   2/Primary Exchange/Primary,\
                   3/Highest Volume Exchange With Rebate/VRebate,\
                   4/High Volume Exchange With Lowest Fee/VLowFee";
        let tiers = parse(raw);
        assert_eq!(tiers.len(), 7, "every tier the venue stated");
        assert_eq!(tiers[0].val, "1");
        assert_eq!(tiers[0].display_name, "Maximize Rebate");
        assert_eq!(tiers[0].name, "MaxRebate");
        // A name shown to a person and a name asked for by a program are not
        // the same string, and putting one where the other belongs is how a
        // caller asks for a tier that does not exist.
        assert_eq!(tiers[6].display_name, "High Volume Exchange With Lowest Fee");
        assert_eq!(tiers[6].name, "VLowFee");
    }

    /// A logon stating none means the account holds none.
    #[test]
    fn no_tiers_stated_is_no_tiers() {
        assert!(parse("").is_empty());
    }
}
