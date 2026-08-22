//! Shared types and helpers for compatibility tests.

use std::env;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// An account number, as it may be written down.
///
/// These phases run in continuous integration, whose logs are readable by
/// anyone the repository is readable by. An account number is not a credential
/// and is not a secret, but it is an account detail, and a run that prints one
/// publishes it. Enough is kept to tell two accounts apart in a log; the rest
/// is not.
pub(super) fn redacted(account: &str) -> String {
    match account.len() {
        0 => String::new(),
        n if n <= 4 => "*".repeat(n),
        n => format!("{}{}", &account[..2], "*".repeat(n - 2)),
    }
}


pub(super) use std::sync::Arc;
pub(super) use std::time::{Duration, Instant};

pub(super) use ibx::api::client::EClient;
pub(super) use ibx::api::types::{Contract as ApiContract, Order as ApiOrder};
pub(super) use ibx::api::wrapper::tests::RecordingWrapper;
pub(super) use ibx::bridge::{Event, SharedState};
pub(super) use ibx::engine::hot_loop::HotLoop;
pub(super) use ibx::gateway::{self, GatewayConfig};
pub(super) use ibx::protocol::connection::{Connection, Frame};
pub(super) use ibx::protocol::{fix, fixcomp};
pub(super) use ibx::types::*;

/// Resolve paper-account credentials from the process environment.
///
/// Absent credentials abort the suite instead of skipping it. This suite only
/// verifies something when it reaches real servers, so a skip that still reports
/// `ok` is indistinguishable from a real pass: the suite stopped compiling at
/// 93e6995 and nothing noticed, because every run took the skip path and went
/// green. An empty value counts as absent — an exported-but-blank var is the
/// same trap in a different shape.
///
/// The env is not loaded from `.env` here (no loader dependency); export it
/// first, e.g. `set -a; . ./.env; set +a`. To skip on purpose (a checkout with
/// no credentials), set `IBX_ALLOW_SKIP_NO_CREDS=1` and the suite returns `None`
/// as before.
pub(super) fn get_config() -> Option<GatewayConfig> {
    let var = |k: &str| env::var(k).ok().filter(|v| !v.trim().is_empty());
    let (username, password) = match (var("IB_USERNAME"), var("IB_PASSWORD")) {
        (Some(u), Some(p)) => (u, p),
        _ if var("IBX_ALLOW_SKIP_NO_CREDS").as_deref() == Some("1") => return None,
        _ => panic!(
            "IB_USERNAME/IB_PASSWORD unset or empty — the compat suite tests \
             nothing without real-server credentials, so it fails rather than \
             passing silently. Export them first (`set -a; . ./.env; set +a`), \
             or set IBX_ALLOW_SKIP_NO_CREDS=1 to skip deliberately."
        ),
    };
    let host = env::var("IB_HOST").unwrap_or_else(|_| "cdc1.ibllc.com".to_string());
    Some(GatewayConfig {
        settings: Default::default(),
        username,
        password: zeroize::Zeroizing::new(password),
        host,
        paper: true,
        accept_invalid_certs: false,
        ib_key_timeout_secs: ibx::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
        ib_key_token_sub_type: ibx::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
        code_provider: None,
        resume: None,
    })
}

/// Shared connections passed between test phases.
pub(super) struct Conns {
    pub(super) farm: Connection,
    pub(super) ccp: Connection,
    pub(super) hmds: Option<Connection>,
    pub(super) account_id: String,
}

/// Credentials every phase's engine recovers with.
///
/// Set once, after the session is up. Without it a phase builds an engine that
/// cannot rebuild a dropped transport, so a drop anywhere in a twenty-minute
/// run fails whichever phase was unlucky — which is how three runs died on the
/// farm going away rather than on anything the client did. With it the engine
/// recovers exactly as it does in production, and the suite tests that too.
pub(super) static RECOVERY_AUTH: std::sync::OnceLock<gateway::ReconnectAuth> =
    std::sync::OnceLock::new();

/// Remember what a reconnect will need. Call once, after `Gateway::connect`.
pub(super) fn remember_recovery_auth(gw: &gateway::Gateway, config: &GatewayConfig) {
    let mut auth = gw.reconnect_auth(gateway::CallerAuth {
        settings: Default::default(),
        host: config.host.clone(),
        username: config.username.clone(),
        password: zeroize::Zeroizing::new(config.password.to_string()),
        paper: config.paper,
        code_provider: config.code_provider.clone(),
        ib_key_timeout_secs: config.ib_key_timeout_secs,
        ib_key_token_sub_type: config.ib_key_token_sub_type.clone(),
    });

    // No phase here asks for the calendar, and every phase builds an engine of
    // its own. An engine told where the security-definition farm is rebuilds
    // it on startup, so a run of a hundred and twenty-nine phases logged on to
    // that farm about thirty times — a full authentication each, on a session
    // that never used it once.
    //
    // A real client builds one engine and connects it once. This is the
    // harness paying for its own shape, so the harness stops asking: name no
    // such farm, and nothing rebuilds one. A phase that wants the calendar has
    // to open it, which is the right way round.
    auth.secdef_host.clear();
    auth.secdef_farm.clear();

    let _ = RECOVERY_AUTH.set(auth);
}

/// Run a hot loop in a background thread, returning the HotLoop for connection reclamation.
pub(super) fn run_hot_loop(hot_loop: HotLoop) -> std::thread::JoinHandle<HotLoop> {
    std::thread::spawn(move || {
        let mut hl = hot_loop;
        if let Some(auth) = RECOVERY_AUTH.get() {
            hl.set_reconnect_auth(auth.clone());
        }
        hl.run();
        hl
    })
}

/// Shutdown a hot loop and reclaim connections.
pub(super) fn shutdown_and_reclaim(
    control_tx: &std::sync::mpsc::SyncSender<ControlCommand>,
    join: std::thread::JoinHandle<HotLoop>,
    account_id: String,
) -> Conns {
    let _ = control_tx.send(ControlCommand::Shutdown);
    let mut hl = join.join().expect("hot loop thread panicked");
    let farm = hl.farm_conn.take().expect("farm_conn missing after shutdown");
    let mut ccp = hl.ccp_conn.take().expect("ccp_conn missing after shutdown");
    let hmds = hl.hmds_conn.take();

    // Keep auth connection alive between phases — drain pending data and send heartbeat.
    // Without this, the venue kills the auth connection after >10s idle during
    // phase transitions (e.g., reconnection in historical phases).
    ccp_keepalive(&mut ccp);

    Conns { farm, ccp, hmds, account_id }
}

/// Every FIX message a frame carries, unsigned and decompressed.
///
/// A FIXCOMP frame is an envelope holding several messages; all of them are
/// returned. The venue closes a connection that leaves a TestRequest
/// unanswered, and a TestRequest can arrive behind another message.
///
/// Every frame is unsigned, whatever kind it is: on a signed session the read
/// chain advances per message, so skipping one leaves the rest unreadable.
pub(super) fn messages_in(conn: &mut Connection, frame: &Frame) -> Vec<Vec<u8>> {
    let raw = match frame {
        Frame::Fix(r) | Frame::FixComp(r) | Frame::Binary(r) => r,
        // Control-state frames are not consumed downstream.
        Frame::Control(_) => return Vec::new(),
    };
    let Some(unsigned) = conn.unsign(raw) else { return Vec::new() };
    if matches!(frame, Frame::FixComp(_)) {
        fixcomp::fixcomp_decompress(&unsigned).unwrap_or_default()
    } else {
        vec![unsigned]
    }
}

/// Send a heartbeat on the auth connection and respond to any pending TestRequests.
/// Prevents IB from killing the connection during phase transitions.
pub(super) fn ccp_keepalive(ccp: &mut Connection) {
    // Drained, not sampled: what is left queued is what the next phase reads, and
    // a TestRequest among it is one the venue is waiting on.
    let _ = drained_without_closing(ccp);
    // `extract_frames` on an empty buffer yields nothing, so the guard this
    // replaces was hardwired true and never gated anything.
    let frames = ccp.extract_frames();
    for frame in frames {
        for m in messages_in(ccp, &frame) {
            let parsed = fix::fix_parse(&m);
            if parsed.get(&fix::TAG_MSG_TYPE).map(|s| s.as_str()) == Some(fix::MSG_TEST_REQUEST) {
                // Respond to TestRequest with Heartbeat containing the test ID
                let test_id = parsed.get(&fix::TAG_TEST_REQ_ID).cloned().unwrap_or_default();
                let ts = ibx::protocol::datetime::chrono_free_timestamp();
                let _ = ccp.send_fix(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_HEARTBEAT),
                    (fix::TAG_SENDING_TIME, &ts),
                    (fix::TAG_TEST_REQ_ID, &test_id),
                ]);
            }
        }
    }

    // Send a heartbeat
    let ts = ibx::protocol::datetime::chrono_free_timestamp();
    let _ = ccp.send_fix(&[
        (fix::TAG_MSG_TYPE, fix::MSG_HEARTBEAT),
        (fix::TAG_SENDING_TIME, &ts),
    ]);
}

/// Whether the auth connection still answers.
///
/// Sends a TestRequest and requires the Heartbeat carrying that request's id.
/// Bytes alone do not establish liveness: a Logout precedes the close, so a
/// read that returns data can come from a connection already going away.
///
/// Reads frames to find the answer, so it runs between phases where no engine
/// holds the connection. Other messages read on the way are consumed.
fn ccp_answers(ccp: &mut Connection) -> bool {
    let test_id = format!("reclaim-{}", next_order_id());
    let ts = ibx::protocol::datetime::chrono_free_timestamp();
    if ccp
        .send_fix(&[
            (fix::TAG_MSG_TYPE, fix::MSG_TEST_REQUEST),
            (fix::TAG_SENDING_TIME, &ts),
            (fix::TAG_TEST_REQ_ID, &test_id),
        ])
        .is_err()
    {
        return false;
    }
    // Only the Heartbeat carrying this request's id counts. A Logout is taken as
    // the connection going away.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match ccp.try_recv() {
            Ok(0) => std::thread::sleep(Duration::from_millis(50)),
            Ok(_) => {}
            Err(_) => return false,
        }
        for frame in ccp.extract_frames() {
            for m in messages_in(ccp, &frame) {
                let parsed = fix::fix_parse(&m);
                match parsed.get(&fix::TAG_MSG_TYPE).map(|s| s.as_str()) {
                    Some(fix::MSG_LOGOUT) => return false,
                    Some(fix::MSG_HEARTBEAT)
                        if parsed.get(&fix::TAG_TEST_REQ_ID).is_some_and(|v| *v == test_id) =>
                    {
                        return true;
                    }
                    _ => {}
                }
            }
        }
    }
    false
}

/// Generate a unique order ID based on current time. The trailing three
/// digits come from a process-wide counter: two calls in the same
/// millisecond (e.g. allocating a parent and child id back-to-back)
/// otherwise return the SAME id, and the second order clobbers the first.
pub(super) fn next_order_id() -> OrderId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64 * 1000;
    let id = base + (SEQ.fetch_add(1, Ordering::Relaxed) % 1000);
    IDS_THIS_RUN.lock().unwrap().push(id);
    id
}

/// How many phases have announced themselves.
///
/// Counted as it happens. A total worked out from a hardcoded phase count less
/// a hardcoded list of expected skips reports a phase that returned early for
/// any other reason as having run, and drifts from the number of phases in the
/// file as phases are added to it.
static PHASES_ANNOUNCED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// How many of those went on to verify nothing.
static PHASES_SKIPPED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Record that a phase has begun, and its verdict where it states one in the
/// same breath as its name.
///
/// The announcement is read rather than the call site asked to classify itself.
/// A phase that names itself and skips in one line would otherwise be counted
/// as having run and never as having skipped, and every new phase would have to
/// remember which of two macros to reach for.
pub(super) fn note_phase_announced(announcement: &str) {
    PHASES_ANNOUNCED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // A new phase, so its verdict is its own.
    THIS_PHASE_SKIPPED.store(false, std::sync::atomic::Ordering::Relaxed);
    if states_a_skip(announcement) {
        note_phase_skipped();
    }
}

/// Whether an announcement carries its own verdict.
///
/// Separate from the counters so it can be read on its own: what it decides is
/// the difference between a phase reported as having verified something and one
/// reported as having declined to.
fn states_a_skip(announcement: &str) -> bool {
    announcement.contains("SKIP")
}

/// Record that a phase declined to verify anything. Called by the `skipped!`
/// macro.
///
/// Counted once for the phase, however many times it says so: a phase that
/// reports two of its parts missing is one phase that verified less than it
/// meant to, not two, and counting the lines would report more skips than there
/// are phases to skip.
pub(super) fn note_phase_skipped() {
    let seen = std::sync::atomic::Ordering::Relaxed;
    if !THIS_PHASE_SKIPPED.swap(true, seen) {
        PHASES_SKIPPED.fetch_add(1, seen);
    }
}

/// Whether the phase being announced now has already said it skipped.
static THIS_PHASE_SKIPPED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// What has been announced and skipped since this was last called.
///
/// Read as a difference because the counters outlive any one test: the focused
/// phases in this binary run in the same process, and a summary that read the
/// totals would report their work as the suite's.
pub(super) fn phase_tally_since_baseline() -> (u32, u32) {
    let seen = std::sync::atomic::Ordering::Relaxed;
    let (announced, skipped) = (PHASES_ANNOUNCED.load(seen), PHASES_SKIPPED.load(seen));
    let (was_announced, was_skipped) = *BASELINE.lock().unwrap();
    (announced - was_announced, skipped - was_skipped)
}

/// Start counting from here, so what follows is reported on its own.
pub(super) fn take_phase_baseline() {
    let seen = std::sync::atomic::Ordering::Relaxed;
    *BASELINE.lock().unwrap() = (PHASES_ANNOUNCED.load(seen), PHASES_SKIPPED.load(seen));
}

static BASELINE: std::sync::Mutex<(u32, u32)> = std::sync::Mutex::new((0, 0));

/// Every order id this run has handed out.
///
/// Kept because the venue names what is working only once, unprompted, when a
/// session opens: a teardown that reuses the session already open is never told
/// again, so what to withdraw has to be what this run knows it placed. Holding
/// the ids also keeps the teardown off an order somebody placed by hand on the
/// same account, which is not in here.
static IDS_THIS_RUN: std::sync::Mutex<Vec<OrderId>> = std::sync::Mutex::new(Vec::new());

/// Withdraw the orders this run left working.
///
/// Phases place orders that rest by design, and the venue caps how many may
/// rest at once on one side of a contract. Left behind, a later run is refused
/// on that cap rather than on anything it did, and the refusal reads as a
/// defect in the order it just built.
///
/// Only this run's orders are withdrawn. The venue states what is working when
/// a session opens, so nothing has to be asked for; what comes back is filtered
/// against [`FIRST_ID_THIS_RUN`] so an order placed by hand on the same account
/// is left where it is.
pub(super) fn withdraw_orders_this_run_left(conns: Conns) -> Conns {
    let placed: Vec<OrderId> = IDS_THIS_RUN.lock().unwrap().clone();
    if placed.is_empty() {
        println!("--- Teardown: no order was placed, so none is left working ---\n");
        return conns;
    }
    println!("--- Teardown: withdrawing the {} orders this run placed ---", placed.len());

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(),
        None,
        account_id.clone(),
        conns.farm,
        conns.ccp,
        conns.hmds,
        None,
    );
    let join = run_hot_loop(hot_loop);

    // Every one of them, not the ones still working: which are still working is
    // what the venue states unprompted as a session opens, and this runs on the
    // session already open. A withdrawal for an order that filled or was
    // withdrawn already is refused, which costs nothing and is not read here.
    for order_id in &placed {
        let _ = control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: *order_id }));
    }
    // Long enough for the last of them to leave, since shutting the loop down
    // with sends still queued would drop them.
    std::thread::sleep(Duration::from_secs(10));
    println!("  {} asked to withdraw\n", placed.len());

    shutdown_and_reclaim(&control_tx, join, account_id)
}

/// Rebuild the trading connection on the session already open, whatever state
/// it is in.
///
/// For the phases that sit idle. The venue closes a quiet trading connection,
/// and the two heartbeat phases leave it unused for eighty-five seconds.
///
/// Rebuilt unconditionally rather than probed: the venue can answer a liveness
/// check and close immediately after, so the answer is stale by the next phase.
/// One logon on an open session is cheap.
pub(super) fn rebuild_ccp(mut conns: Conns) -> Conns {
    // The previous connection is dropped whatever state it is in.
    let _taking_it_away = TakingTheSessionAway::begin();
    let Some(auth) = RECOVERY_AUTH.get() else { return conns };
    match gateway::reconnect_ccp(auth) {
        Ok(ccp) => {
            // The reconnect sends the same opening sequence a first logon does.
            // Repeating it here would request the same subscriptions twice.
            conns.ccp = ccp;
            println!("  [reconnect] trading connection rebuilt after the idle phases\n");
        }
        Err(e) => println!("  [reconnect] could not rebuild after the idle phases: {e}\n"),
    }
    conns
}

/// Read a socket until it has nothing more to say, and report whether it is
/// still there.
///
/// A read returning bytes says nothing about what is queued behind them: a final
/// message and the close that follows arrive in that order. `Ok(0)` means
/// nothing more to read; an error means the connection is closed.
///
/// Draining to nothing does not establish liveness, since a close already sent
/// may not have surfaced. Where a phase claims a connection survived, follow
/// this with [`ccp_still_carrying`].
pub(super) fn drained_without_closing(conn: &mut Connection) -> bool {
    loop {
        match conn.try_recv() {
            Ok(0) => return true,
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
}

/// Whether the auth connection is still carrying traffic, proved by asking it.
///
/// Drains what is queued, then sends a TestRequest and requires the Heartbeat
/// answering it. Reading the socket alone cannot establish this: bytes queued
/// ahead of a close arrive first.
pub(super) fn ccp_still_carrying(ccp: &mut Connection) -> bool {
    drained_without_closing(ccp) && ccp_answers(ccp)
}

/// Check if CCP connection is alive and reconnect the full gateway if not.
/// Returns updated Conns (and optionally a new Gateway) with fresh connections.
pub(super) fn ensure_ccp_alive(
    mut conns: Conns,
    gw: &mut gateway::Gateway,
    config: &GatewayConfig,
) -> Conns {
    let mut closed = !drained_without_closing(&mut conns.ccp);
    // Draining to nothing does not establish liveness: a close already sent may
    // not have surfaced on this read. Ask instead — send a TestRequest and
    // require the answer.
    if !closed && !ccp_answers(&mut conns.ccp) {
        closed = true;
    }
    if !closed {
        return conns;
    }
    println!("  [reconnect] CCP connection dead, rebuilding it on this session...");

    // The client's own reconnect, which is the thing a program depends on and
    // so the thing worth testing. It rebuilds the auth connection on the
    // session already open.
    //
    // Opening a whole new session instead was a second logon on an account
    // that takes one: the venue drops whatever was still held, and every later
    // loss in the run becomes unattributable — the suite could no longer tell
    // the venue dropping this session from the suite having replaced it.
    if let Some(auth) = RECOVERY_AUTH.get() {
        match gateway::reconnect_ccp(auth) {
            Ok(ccp) => {
                conns.ccp = ccp;
                println!("  [reconnect] CCP rebuilt on the same session");
                return conns;
            }
            Err(e) => println!("  [reconnect] the session could not be rebuilt: {e}"),
        }
    }

    // Only where that failed, and said so. A fresh session is a new logon and
    // the run is no longer measuring one.
    println!("  [reconnect] opening a NEW SESSION — this is a second logon");
    match gateway::Gateway::connect(config) {
        Ok(gateway::Session { gateway: new_gw, market_data: farm, trading: ccp, historical: hmds, .. }) => {
            conns.farm = farm;
            conns.ccp = ccp;
            conns.hmds = hmds;
            conns.account_id = new_gw.account_id.clone();
            *gw = new_gw;
            println!("  [reconnect] Gateway re-established (farm+ccp+hmds)");
        }
        Err(e) => {
            println!("  [reconnect] Gateway reconnect failed: {e} — continuing with dead CCP");
        }
    }
    conns
}

// ─── Market session detection ───

/// US stock market session based on current Eastern Time.
/// DST: second Sunday of March (spring forward) to first Sunday of November (fall back).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum MarketSession {
    Regular,    // Mon-Fri 9:30-16:00 ET
    PreMarket,  // Mon-Fri 4:00-9:30 ET
    AfterHours, // Mon-Fri 16:00-20:00 ET
    Closed,     // Mon-Fri 20:00-4:00 ET, weekends
}

pub(super) fn market_session() -> (MarketSession, u16) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let secs_per_day = 86400u64;
    let total_days = now / secs_per_day;
    let utc_hour = ((now % secs_per_day) / 3600) as i32;
    let utc_min = ((now % 3600) / 60) as i32;

    let mut y = 1970i64;
    let mut remaining = total_days as i64;
    loop {
        let ylen = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < ylen { break; }
        remaining -= ylen;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays: [i64; 12] = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u8;
    for &d in &mdays {
        if remaining < d { break; }
        remaining -= d;
        month += 1;
    }
    let day = (remaining + 1) as u8;

    // Day of week: Jan 1 1970 = Thursday (4), 0=Sun..6=Sat
    let utc_dow = ((total_days + 4) % 7) as u8;

    // Compute second Sunday of March and first Sunday of November for DST.
    let jan1_dow = {
        let mut d = 4u8; // Jan 1 1970 = Thursday
        for yr in 1970..y {
            let yl = if yr % 4 == 0 && (yr % 100 != 0 || yr % 400 == 0) { 366 } else { 365 };
            d = ((d as u16 + (yl % 7) as u16) % 7) as u8;
        }
        d // 0=Sun
    };
    let mar1_doy = if leap { 60 } else { 59 };
    let mar1_dow = ((jan1_dow as u16 + (mar1_doy % 7) as u16) % 7) as u8;
    let first_sun_mar = if mar1_dow == 0 { 1 } else { 8 - mar1_dow };
    let second_sun_mar = first_sun_mar + 7;

    let nov1_doy = if leap { 305 } else { 304 };
    let nov1_dow = ((jan1_dow as u16 + (nov1_doy % 7) as u16) % 7) as u8;
    let first_sun_nov = if nov1_dow == 0 { 1 } else { 8 - nov1_dow };

    let is_edt = match month {
        4..=10 => true,
        3 => day > second_sun_mar || (day == second_sun_mar && utc_hour >= 7),
        11 => day < first_sun_nov || (day == first_sun_nov && utc_hour < 6),
        _ => false,
    };
    let offset: i32 = if is_edt { -240 } else { -300 };
    let et_min_total = utc_hour * 60 + utc_min + offset;

    let (et_dow, et_min) = if et_min_total < 0 {
        (if utc_dow == 0 { 6 } else { utc_dow - 1 }, (et_min_total + 1440) as u16)
    } else {
        (utc_dow, et_min_total as u16)
    };

    if et_dow == 0 || et_dow == 6 { return (MarketSession::Closed, et_min); }

    // Eastern-Time calendar date (needed for the holiday calendar). Deriving it
    // from a single ET epoch value avoids month/year rollback bugs at midnight.
    let et_days = ((now as i64) + (offset as i64) * 60).div_euclid(86400);
    let (ey, em, ed) = ymd_from_days(et_days);
    match us_market_holiday(ey, em, ed, et_dow as u32) {
        Holiday::Closed => return (MarketSession::Closed, et_min),
        Holiday::EarlyClose => {
            // Regular trading ends 13:00 ET (780 min); gate out later sessions.
            let session = match et_min {
                240..=569 => MarketSession::PreMarket,
                570..=779 => MarketSession::Regular,
                _ => MarketSession::Closed,
            };
            return (session, et_min);
        }
        Holiday::Open => {}
    }

    let session = match et_min {
        240..=569 => MarketSession::PreMarket,
        570..=959 => MarketSession::Regular,
        960..=1199 => MarketSession::AfterHours,
        _ => MarketSession::Closed,
    };
    (session, et_min)
}

// ─── US market holiday calendar ───
//
// Gates session-dependent phases so a run on a US market holiday (or a half-day
// early close) does not spuriously fire tick/fill phases against a closed venue.

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Holiday {
    Open,       // normal trading day
    Closed,     // full-day market closure
    EarlyClose, // half day, regular session ends 13:00 ET
}

/// Days since the Unix epoch for a civil (proleptic Gregorian) date.
/// Howard Hinnant's `days_from_civil`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let yy = if m <= 2 { y - 1 } else { y };
    let era = (if yy >= 0 { yy } else { yy - 399 }) / 400;
    let yoe = yy - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse of `days_from_civil` — civil date `(year, month, day)` from an epoch day count.
fn ymd_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Day of week for an epoch day count, 0=Sunday .. 6=Saturday.
fn dow(days: i64) -> u32 {
    ((days + 4).rem_euclid(7)) as u32
}

/// Day-of-month of the `n`-th `weekday` (0=Sun) in `(y, m)`.
fn nth_weekday(y: i64, m: u32, weekday: u32, n: u32) -> u32 {
    let first = days_from_civil(y, m, 1);
    let fdow = dow(first);
    let offset = (weekday + 7 - fdow) % 7;
    1 + offset + (n - 1) * 7
}

/// Day-of-month of the last `weekday` (0=Sun) in `(y, m)`.
fn last_weekday(y: i64, m: u32, weekday: u32) -> u32 {
    let first_next = if m == 12 { days_from_civil(y + 1, 1, 1) } else { days_from_civil(y, m + 1, 1) };
    let last = first_next - 1;
    let ldow = dow(last);
    let (_, _, ld) = ymd_from_days(last);
    ld - ((ldow + 7 - weekday) % 7)
}

/// Gregorian Easter Sunday (Anonymous algorithm). Returns `(month, day)`.
fn easter(y: i64) -> (u32, u32) {
    let a = y % 19;
    let b = y / 100;
    let c = y % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let mth = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * mth + 114) / 31;
    let day = ((h + l - 7 * mth + 114) % 31) + 1;
    (month as u32, day as u32)
}

/// Good Friday = Easter Sunday − 2 days. Returns `(month, day)`.
fn good_friday(y: i64) -> (u32, u32) {
    let (em, ed) = easter(y);
    let gf = days_from_civil(y, em, ed) - 2;
    let (_, m, d) = ymd_from_days(gf);
    (m, d)
}

/// True if `(m, d)` is the observed closure date of a fixed-date holiday `(fm, fd)`.
/// NYSE convention: Saturday holiday → observed the preceding Friday; Sunday → the following Monday.
fn observed_fixed(y: i64, fm: u32, fd: u32, m: u32, d: u32) -> bool {
    let fdow = dow(days_from_civil(y, fm, fd));
    let (om, od) = if fdow == 6 {
        let (_, im, id) = ymd_from_days(days_from_civil(y, fm, fd) - 1);
        (im, id)
    } else if fdow == 0 {
        let (_, im, id) = ymd_from_days(days_from_civil(y, fm, fd) + 1);
        (im, id)
    } else {
        (fm, fd)
    };
    m == om && d == od
}

/// Classify an Eastern-Time calendar date against the US equity market calendar.
/// `dow` is 0=Sunday .. 6=Saturday for `(y, m, d)`.
fn us_market_holiday(y: i64, m: u32, d: u32, dow: u32) -> Holiday {
    // ── Full-day closures (checked first; they take precedence over early closes) ──
    // New Year's Day — Jan 1; a Sunday Jan 1 is observed Monday. A Saturday Jan 1
    // is NOT made up on the preceding Friday (that Friday is the prior year).
    if m == 1 && d == 1 && dow != 0 && dow != 6 { return Holiday::Closed; }
    if m == 1 && d == 2 && dow == 1 { return Holiday::Closed; }
    // Martin Luther King Jr. Day — 3rd Monday January
    if m == 1 && d == nth_weekday(y, 1, 1, 3) { return Holiday::Closed; }
    // Washington's Birthday — 3rd Monday February
    if m == 2 && d == nth_weekday(y, 2, 1, 3) { return Holiday::Closed; }
    // Good Friday
    let (gm, gd) = good_friday(y);
    if m == gm && d == gd { return Holiday::Closed; }
    // Memorial Day — last Monday May
    if m == 5 && d == last_weekday(y, 5, 1) { return Holiday::Closed; }
    // Juneteenth — Jun 19 (market holiday since 2022), observed
    if y >= 2022 && observed_fixed(y, 6, 19, m, d) { return Holiday::Closed; }
    // Independence Day — Jul 4, observed
    if observed_fixed(y, 7, 4, m, d) { return Holiday::Closed; }
    // Labor Day — 1st Monday September
    if m == 9 && d == nth_weekday(y, 9, 1, 1) { return Holiday::Closed; }
    // Thanksgiving — 4th Thursday November
    if m == 11 && d == nth_weekday(y, 11, 4, 4) { return Holiday::Closed; }
    // Christmas — Dec 25, observed
    if observed_fixed(y, 12, 25, m, d) { return Holiday::Closed; }

    // ── Early closes (regular session ends 13:00 ET) ──
    // Black Friday — day after Thanksgiving
    if m == 11 && d == nth_weekday(y, 11, 4, 4) + 1 { return Holiday::EarlyClose; }
    // Christmas Eve — Dec 24 when a weekday
    if m == 12 && d == 24 && (1..=5).contains(&dow) { return Holiday::EarlyClose; }
    // July 3 — when a weekday (and not itself an observed full holiday, handled above)
    if m == 7 && d == 3 && (1..=5).contains(&dow) { return Holiday::EarlyClose; }

    Holiday::Open
}

/// Session-aware order-acknowledgment gate for order phases.
///
/// On a Closed market, order acknowledgment is unavailable or unreliable: pegged
/// and snapshot order types need a live reference market, and even plain types ack
/// slowly if at all on the paper venue over a weekend. A missing ack is therefore
/// a legitimate SKIP, not a failure. Returns `true` when the caller should
/// skip-return its `Conns` (and prints the SKIP line as a side effect); on an open
/// session it returns `false` so the caller's own `assert!` still enforces the ack.
pub(super) fn skip_unacked_if_closed(order_acked: bool) -> bool {
    // Outside regular hours, not merely closed. An order that is never
    // acknowledged pre-market is the venue declining to work it, exactly as it
    // is overnight, and the phases that name the session themselves already
    // skip on it — this one asserted instead and failed the suite for the time
    // of day.
    if order_acked {
        return false;
    }
    let session = market_session().0;
    if session != MarketSession::Regular {
        skipped!(
            "  SKIP: {session:?} — order not acknowledged (order type/venue needs a live market)\n",
        );
        return true;
    }
    false
}

/// An instrument whose top of book answers whenever this suite runs.
///
/// The three phases that use it exist to prove ticks arrive while New York is
/// shut, so what they watch has to be something that never is. Forex was the
/// original choice and closes every weekend, between sessions each day, and on
/// holidays — picking between two venues off the clock is a prediction, and it
/// is wrong on the days nobody remembers. A crypto trades every day, so there
/// is no window left to predict.
pub(super) const ALWAYS_QUOTING: (&str, &str, &str) = ("BTC", "CRYPTO", "PAXOS");

/// Whether London is trading, in UTC.
///
/// [`market_session`] answers for New York, and a London order excused because
/// New York has not opened is excused by the wrong clock — the whole point of
/// the phase is the venue that is open. The window here is the part of the
/// London session that holds in both British Summer Time and winter, so it is
/// never wrong in the direction that grants an excuse.
pub(super) fn london_is_trading() -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let days = now / 86_400;
    // 1970-01-01 was a Thursday.
    let weekday = (days + 4) % 7;
    if weekday == 0 || weekday == 6 {
        return false;
    }
    let minutes = (now % 86_400) / 60;
    (8 * 60..15 * 60 + 30).contains(&minutes)
}

/// A historical request that came back with nothing.
///
/// Historical data does not wait for an opening bell: the venue serves last
/// week's bars at midnight. So silence is either the venue declining and saying
/// why — pacing, or a product this session is not entitled to — or this client
/// asking wrongly. Only the first is a skip, and it quotes the venue's code
/// and words so the log says which request was refused and for what.
pub(super) fn historical_silence(shared: &SharedState, what: &str) {
    if let Some((_, code, message)) = shared.reference.drain_historical_errors().first() {
        skipped!("  SKIP: {what} — the venue refused it, {code}: {message}\n");
        return;
    }
    panic!(
        "{what}, and the venue gave no reason. Historical data does not wait \
         for a market to open, so this is the request or the reply being read \
         wrong."
    );
}

/// Every phase that could not run because the session went away, and no phase
/// had asked it to.
///
/// A phase that skips on a lost connection is telling the truth about itself
/// and a lie about the run: it reports the same SKIP whether the session was
/// up and the market quiet, or the session was gone and nothing could have
/// arrived. Per phase that is the honest reading. Per run it means the suite
/// finishes green having verified none of them.
static LOST_THE_SESSION: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// How many phases are currently taking the session away on purpose.
static ON_PURPOSE: AtomicUsize = AtomicUsize::new(0);

/// A phase is about to take the session away, and losses until the guard drops
/// are that phase's doing rather than the venue's.
///
/// One phase does this deliberately: it parks the trading connection behind a
/// dead socket to measure how long detection takes. Scoped rather than set for
/// the rest of the run, so a genuine loss after it is still recorded.
#[must_use = "the session is only expected to go away while the guard is held"]
pub(super) struct TakingTheSessionAway;

impl TakingTheSessionAway {
    pub(super) fn begin() -> Self {
        ON_PURPOSE.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for TakingTheSessionAway {
    fn drop(&mut self) {
        ON_PURPOSE.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Record that a phase skipped because the session was gone.
pub(super) fn note_lost_session(what: &str) {
    if ON_PURPOSE.load(Ordering::Acquire) == 0 {
        LOST_THE_SESSION.lock().unwrap().push(what.to_string());
    }
}

/// Fail the run if the session went away and no phase had asked it to.
///
/// Called once, at the end. Every phase named here reported SKIP and verified
/// nothing, so a run reaching this point with a non-empty list has not tested
/// what its phase count says it tested.
pub(super) fn no_phase_lost_the_session_unasked() {
    let lost = LOST_THE_SESSION.lock().unwrap();
    assert!(
        lost.is_empty(),
        "the session went away during the run and no phase asked it to, so \
         {} phase(s) reported SKIP having verified nothing: {}. The count above \
         reports them among the phases that verified nothing, which is not the \
         same as the market being shut. Diagnose the disconnect rather than \
         reading this suite as green.",
        lost.len(),
        lost.join("; "),
    );
}

/// Something the session had to deliver and did not, on a path the market does
/// not gate: account values, a position after a fill, the notification that the
/// connection went away. A session that is logged in delivers these at any hour,
/// so an absence is this client's, and a phase that skips on it reports nothing
/// wrong on precisely the runs where something is.
pub(super) fn session_owed(shared: &SharedState, what: &str) {
    // The session owes this only while it has one. A connection that went away
    // explains the absence, and explains it better than the rule does.
    if lost_unasked(shared) {
        note_lost_session(what);
        skipped!("  SKIP: {what} — the connection was lost, so nothing could arrive\n");
        return;
    }
    panic!("{what} — the session delivers this whether or not the market is trading.");
}

/// Whether the session went away for a reason nobody here asked for.
///
/// Shutting an engine down sets the same flag a lost connection does, and phases
/// read it after shutting their engine down. The recorded reason distinguishes
/// the two; the flag alone does not.
pub(super) fn lost_unasked(shared: &SharedState) -> bool {
    // Read from the recorded reason, not the flag: a deliberate shutdown sets the
    // same flag and records itself as the cause.
    shared.take_connection_lost() && !shared.connection_lost_by_design()
}

/// A lookup that had to answer and did not.
///
/// Contract definitions and their details are reference data: the venue answers
/// for them when every market is shut, and this session is permissioned for the
/// types the suite asks about. So an empty answer is this client asking wrongly
/// or failing to read the reply, never the hour — and a phase that skips on it
/// hides exactly the bug it exists to catch.
pub(super) fn lookup_returned_nothing(what: &str) -> ! {
    panic!(
        "{what} — contract data does not depend on the market being open, so \
         this is the request or the reply being read wrong."
    );
}

/// A phase that needed the market to be trading and did not get it.
///
/// Outside regular hours this is the truth and the phase skips. During them it
/// is a defect: a market order on a liquid contract that never fills, or a
/// subscription that never ticks, is this client's problem and must not read as
/// a quiet afternoon.
///
/// Which one it is comes from [`market_session`], the same holiday-aware clock
/// the order phases already gate on, so one answer decides it everywhere.
pub(super) fn no_market(shared: &SharedState, what: &str) {
    // A connection that went away explains every absence that follows it, and
    // explains it better than the clock does. The engine holds an order for the
    // reconnect rather than dropping it, so nothing arrives and nothing is
    // wrong with the client — but "the market is quiet" is the one reading that
    // is certainly false.
    if lost_unasked(shared) {
        note_lost_session(what);
        skipped!("  SKIP: {what} — the connection was lost, so nothing could arrive\n");
        return;
    }
    let (session, _) = market_session();
    assert!(
        session != MarketSession::Regular,
        "{what} — during regular hours, so this is not a quiet market. \
         The client did not get what it asked for."
    );
    skipped!("  SKIP: {session:?} — {what}\n");
}

/// Reasons a rejection is about the market or the account rather than the
/// order this client built: the session cannot trade the thing, cannot trade
/// it now, or cannot afford it. Matched case-insensitively on the venue's
/// prose, and grown only from a reason a live session actually stated.
/// "No security definition has been found for the request" is deliberately not
/// listed. The venue answers it when the described contract matches nothing it
/// holds — a future named by date where a month is required, an option missing
/// its right — which is a contract-encoding fault and is what these phases
/// exist to catch. An account not permitted to trade something is refused in
/// the words listed below.
const REJECTED_BY_MARKET_OR_ACCOUNT: &[&str] = &[
    "outside",                  // outside regular trading hours
    "closed",                   // the market is closed
    "no trading permission",
    "not permitted",
    "not available",
    "not subscribed",
    "market data",              // no quote to price against
    "insufficient",             // margin, buying power
    "residency",
    "halted",
    // The venue stating which order types and times in force it accepts for a
    // security type on an exchange. This client does not pre-validate that — it
    // lets the venue refuse — so the refusal is the design working, not a
    // malformed order.
    "invalid for this combination",
    // A displayed quantity this venue will not take for this security. Tried
    // live at 100, 200, 500 and 1000 shares displayed, every one a whole number
    // of round lots and every one refused alike, so the value it wants is not a
    // multiple of anything the client controls.
    "display size",
    // A variant of an order this venue does not work at all — a short sale
    // among them. The client writes it; whether the venue takes it is the
    // venue's answer, and the same answer the reference client gets.
    "not supported",
    // The venue's answer to a race a phase creates on purpose: a modify sent
    // while a cancel for the same order is in flight. Losing that race is the
    // outcome under test, not a badly built order.
    "already being cancelled",
    // A time-in-force the venue does not accept for the destination the order
    // is routed to. The destination is not this client's invention: an order
    // on no particular venue, or on the smart route, is sent to the same
    // position the protocol states it in, so an order placed the same way through
    // a gateway is refused with the same words. What is left is the venue's
    // rule about which time-in-force it takes where.
    "is invalid for this combination of exchange and security type",
    // The same race, one order up: a bracket's child cannot attach to a parent
    // the venue is already cancelling. Which order the race is lost on does
    // not change whose answer it is — the venue is describing the state of its
    // own book, not the message this client wrote.
    "parent order is being cancelled",
    // A ceiling on how many orders may rest at once on one side of one
    // contract. It is a limit the account carries, not a fault in the order:
    // the same order is taken once something already resting is cancelled or
    // filled. A suite run leaves orders working, so a paper account that has
    // run it often enough reaches this and every later run is answered with
    // it until the resting orders are cleared.
    "orders working on either the buy or sell side",
    // The same race, lost by a wider margin: the venue had already cancelled
    // the order before the replace reached it, which is what happens to a
    // day order left resting while the market is shut. An order that no
    // longer exists cannot be replaced, and that is the venue's account of
    // its own book rather than anything about the message this client built.
    "too late to replace",
];

/// The reason a rejected order was rejected, for a phase that is about to skip.
///
/// A rejection is treated as a defect in the order this client built unless the
/// venue's stated reason says it was the market or the account. Skipping on any
/// rejection is how a malformed order reads as a closed market: the phase
/// reports SKIP, the suite stays green, and nothing was verified. Fail closed,
/// and widen [`REJECTED_BY_MARKET_OR_ACCOUNT`] only against prose a live
/// session actually produced.
pub(super) fn reject_reason(shared: &SharedState, order_id: u64) -> String {
    let reason = shared.orders.get_order_info(order_id)
        .map(|info| info.order_state.reject_reason)
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| "no reason reported".to_string());

    let lowered = reason.to_lowercase();
    assert!(
        REJECTED_BY_MARKET_OR_ACCOUNT.iter().any(|known| lowered.contains(known)),
        "the venue rejected this order for a reason that is not about the market \
         or the account, so the order this client built is wrong until shown \
         otherwise: {reason:?}. If this reason really is the market or the \
         account talking, add it to REJECTED_BY_MARKET_OR_ACCOUNT."
    );
    reason
}

// ─── Generic submit+cancel helper ───
// fill_or_cancel=false: only cancelled counts as success
// fill_or_cancel=true: filled OR cancelled both count as success

pub(super) fn run_submit_cancel_phase(
    conns: Conns,
    phase_name: &str,
    order_req: OrderRequest,
    fill_or_cancel: bool,
) -> Conns {
    phase!("--- {phase_name} ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");
    // The contract these phases order: a US stock routed smart. An instrument
    // registered by id alone states no security type, and the venue answers an
    // order carrying an empty tag 167 with "Unsupported type". A caller states
    // this on the contract it subscribes with; a phase that registers directly
    // states it here.
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    let order_id = order_req.order_id();
    // Tag 6035 order reference, where the order carries one. Read back below from
    // the venue's report: a reference written into a message and never read back
    // does not show the venue took it.
    let stated_ref = match &order_req {
        OrderRequest::SubmitEx { attrs, .. } => attrs.order_ref.clone(),
        _ => String::new(),
    };

    control_tx.send(ControlCommand::Order(order_req)).unwrap();
    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut order_acked = false;
    let mut cancel_sent = false;
    let mut order_cancelled = false;
    let mut order_filled = false;
    let mut order_rejected = false;
    let mut order_inactive = false;

    while Instant::now() < deadline {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            // Scoped to this order. The session reports every order on the
            // account, including orders left resting by earlier phases and
            // orders replayed on connecting.
            if update.order_id != order_id {
                continue;
            }
            match update.status {
                // PreSubmitted (39=A) is the server's ack: received, not yet
                // working on the exchange. An at-the-open order stops there until
                // the auction and never reaches Submitted (39=0) intraday, so it
                // must count as the ack and fire the cancel, or the order rests
                // forever and the phase reports "never acknowledged" while the
                // acks were in fact arriving.
                // PendingSubmit is the local pre-ack state, kept for order types
                // that surface it.
                OrderStatus::PendingSubmit
                | OrderStatus::PreSubmitted
                | OrderStatus::Submitted => {
                    order_acked = true;
                    if !cancel_sent {
                        control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id })).unwrap();
                        cancel_sent = true;
                    }
                }
                OrderStatus::Cancelled => { order_cancelled = true; break; }
                OrderStatus::Rejected => { order_rejected = true; break; }
                // The venue parked it rather than refusing it outright: an
                // order type the instrument does not support lands here
                // instead of Rejected. Captured live, STP PRT on a
                // SMART-routed stock returns Inactive and then refuses the
                // cancel with 202. That is the venue declining the
                // combination, not the client failing to submit it, so the
                // phase reports it rather than asserting against it.
                OrderStatus::Inactive => { order_inactive = true; break; }
                OrderStatus::Filled => {
                    order_filled = true;
                    if fill_or_cancel { break; }
                }
                _ => {}
            }
        }
    }

    // Withdrawn unless known terminal. An order that answered nothing, or that
    // went silent after an acknowledgement, cannot be shown gone; every path
    // below returns SKIP, and a GTC order would rest at the venue.
    if !(order_cancelled || order_rejected || order_filled) {
        let _ = control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id }));
        // Long enough for the loop to write it before it is stopped.
        std::thread::sleep(Duration::from_millis(250));
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    // Only where the venue reported on this order: an unacknowledged order has no
    // report to read back from.
    if !stated_ref.is_empty() && order_acked {
        let echoed = shared.orders.get_order_info(order_id)
            .map(|i| i.order.order_ref)
            .unwrap_or_default();
        assert_eq!(
            echoed, stated_ref,
            "the order was placed naming itself {stated_ref:?} and the venue's \
             report names it {echoed:?}",
        );
        println!("  order reference echoed: {echoed:?}");
    }

    if order_rejected {
        skipped!("  SKIP: Order rejected — {}\n", reject_reason(&shared, order_id));
        return conns;
    }
    if order_inactive {
        skipped!("  SKIP: parked Inactive — the venue does not accept this order type here\n");
        return conns;
    }
    if fill_or_cancel {
        // Filling needs a market to fill against. Outside regular hours an
        // order of this shape is acknowledged and then rests until the open,
        // which is neither of the two outcomes and is also the correct
        // behaviour — the same session gate the branch below applies for the
        // same reason, on the outcome instead of the acknowledgement.
        let (session, _) = market_session();
        if session != MarketSession::Regular && !(order_filled || order_cancelled) {
            let state = if order_acked { "acknowledged and resting" } else { "not acknowledged" };
            skipped!("  SKIP: {session:?} — {state}; filling needs a live market\n");
            return conns;
        }
        // Say what was seen, not only what was not. "Neither filled nor
        // cancelled" is true of an order the venue never acknowledged and of
        // one it acknowledged and left working, and those are different
        // failures with different causes.
        assert!(
            order_filled || order_cancelled,
            "Order was neither filled nor cancelled: acknowledged={order_acked}, \
             cancel requested={cancel_sent}, venue status {:?}, session {session:?}",
            shared.orders.get_order_info(order_id).map(|i| i.order_state.status),
        );
        if order_filled { println!("  PASS (filled)\n"); } else { println!("  PASS (cancelled)\n"); }
    } else {
        // Session-aware gate: some order types (Relative/pegged, snapshot, midprice)
        // peg to a live primary NBBO and are never acknowledged when the market is
        // closed. Treat an un-acked order on a Closed session as a SKIP, not a
        // failure — a plain order that acks on a closed market still reaches PASS.
        // Anything other than the regular session, not just Closed. These
        // order types peg to a live primary NBBO, and pre-market and after-hours
        // are as short of one as a shut market is: captured live in after-hours,
        // a DAY order of this shape draws no report at all, while the same type
        // sent GTC with outsideRTH is answered immediately. An un-acked order
        // outside regular hours says nothing about the client.
        if !order_acked {
            // Neither acknowledged nor refused. Outside regular hours that is
            // the session; inside them it is the venue declining an order type
            // it does not take for this security without saying so — the
            // protection types are futures orders, and the market variant of
            // the pair is refused in as many words for the same stock.
            //
            // Either way there is nothing here about the client: a malformed
            // message comes back refused, with the field named.
            let (session, _) = market_session();
            if session == MarketSession::Regular {
                skipped!("  SKIP: no answer — the venue neither took nor refused this order type here\n");
            } else {
                skipped!("  SKIP: {session:?} — not acknowledged (this order type needs a live market)\n");
            }
            return conns;
        }
        // A cancel races the venue, and on a liquid instrument in a live
        // market the venue sometimes wins. That the order filled instead of
        // cancelling says the cancel arrived second, which is the market's
        // timing and not something the client did wrong — and asserting
        // otherwise makes this phase fail on the days the fill is quick.
        if order_filled && !order_cancelled {
            println!("  PASS (filled before the cancel reached the venue)\n");
            return conns;
        }
        assert!(
            order_cancelled,
            "Order was never cancelled: acknowledged={order_acked}, cancel requested={cancel_sent}, \
             venue status {:?}, session {:?}",
            shared.orders.get_order_info(order_id).map(|i| i.order_state.status),
            market_session().0,
        );
        println!("  PASS\n");
    }
    conns
}

// ─── Timestamp helper (shared by historical phases) ───

pub(super) fn now_ib_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let secs_per_day = 86400u64;
    let days = now / secs_per_day;
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let diy = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < diy { break; }
        remaining -= diy;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays = [31i64, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 1usize;
    for &d in &mdays {
        if remaining < d { break; }
        remaining -= d;
        m += 1;
    }
    let day = remaining + 1;
    let hour = (now % secs_per_day) / 3600;
    let min = (now % 3600) / 60;
    let sec = now % 60;
    format!("{y:04}{m:02}{day:02}-{hour:02}:{min:02}:{sec:02}")
}

/// The contract id the venue gave for EUR.USD, as the phase that looks it up
/// read it off the wire.
///
/// Contract ids are venue-assigned and region-specific. A phase carrying a
/// literal id cannot distinguish a changed identity from a failed subscription.
/// The phase that resolves it runs immediately before the phases that use it,
/// and names [`ALWAYS_QUOTING`], so all three watch the one contract the venue
/// answered for rather than each asking again.
pub(super) static FALLBACK_CON_ID: std::sync::OnceLock<i64> = std::sync::OnceLock::new();

/// A schedule starting shortly from now and running four hours, formatted
/// `YYYYMMDD-HH:MM:SS` in UTC.
///
/// Computed rather than fixed: a literal date passes, and a schedule in the past
/// is answered on its schedule rather than on its encoding.
pub(super) fn a_schedule_ahead() -> (String, String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (format_utc_timestamp(now + 60), format_utc_timestamp(now + 4 * 3600))
}

/// Format seconds since epoch as YYYYMMDD-HH:MM:SS UTC.
pub(super) fn format_utc_timestamp(epoch_secs: u64) -> String {
    let secs_per_day = 86400u64;
    let days = epoch_secs / secs_per_day;
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    for (i, &d) in month_days.iter().enumerate() {
        if remaining < d as i64 { m = i + 1; break; }
        remaining -= d as i64;
    }
    let day = remaining + 1;
    let hour = (epoch_secs % secs_per_day) / 3600;
    let min = (epoch_secs % 3600) / 60;
    let sec = epoch_secs % 60;
    format!("{y:04}{m:02}{day:02}-{hour:02}:{min:02}:{sec:02}")
}

/// Start logging the way a program running this client starts it.
///
/// The suite reached for the subscriber's own default, which is `error`, so a
/// session discarded every record below it — including the report this client
/// makes when the venue sends something nothing here reads. That report exists
/// to be read on a real session, which is the only place the venue sends
/// anything, and it was being dropped on every one of them.
pub(super) fn start_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Count the ticks already queued, leaving nothing behind.
///
/// A drain written as `while let Ok(Event::Tick(_)) = rx.try_recv()` ends at
/// the first event that is not a tick, so what it reports is however many
/// ticks happened to sit at the head of the queue rather than how many
/// arrived. The engine interleaves its other events freely, which makes that
/// count a property of the ordering and not of the feed.
pub(super) fn drain_ticks(rx: &std::sync::mpsc::Receiver<Event>) -> u32 {
    let mut ticks = 0;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, Event::Tick(_)) {
            ticks += 1;
        }
    }
    ticks
}

#[cfg(test)]
mod holiday_tests {
    use super::*;

    fn kind(y: i64, m: u32, d: u32) -> Holiday {
        us_market_holiday(y, m, d, dow(days_from_civil(y, m, d)))
    }

    #[test]
    fn fixed_and_floating_2026_closures() {
        assert_eq!(kind(2026, 1, 1), Holiday::Closed);   // New Year (Thu)
        assert_eq!(kind(2026, 1, 19), Holiday::Closed);  // MLK — 3rd Mon Jan
        assert_eq!(kind(2026, 2, 16), Holiday::Closed);  // Washington — 3rd Mon Feb
        assert_eq!(kind(2026, 4, 3), Holiday::Closed);   // Good Friday
        assert_eq!(kind(2026, 5, 25), Holiday::Closed);  // Memorial — last Mon May
        assert_eq!(kind(2026, 6, 19), Holiday::Closed);  // Juneteenth (Fri)
        assert_eq!(kind(2026, 9, 7), Holiday::Closed);   // Labor — 1st Mon Sep
        assert_eq!(kind(2026, 11, 26), Holiday::Closed); // Thanksgiving — 4th Thu Nov
        assert_eq!(kind(2026, 12, 25), Holiday::Closed); // Christmas (Fri)
    }

    #[test]
    fn observed_weekend_rules() {
        // Jul 4 2026 is a Saturday → observed Friday Jul 3 (full closure, not early close).
        assert_eq!(dow(days_from_civil(2026, 7, 4)), 6);
        assert_eq!(kind(2026, 7, 3), Holiday::Closed);
        // Jan 1 2023 was a Sunday → observed Monday Jan 2.
        assert_eq!(dow(days_from_civil(2023, 1, 1)), 0);
        assert_eq!(kind(2023, 1, 2), Holiday::Closed);
    }

    #[test]
    fn early_closes() {
        // Black Friday 2026 = day after Thanksgiving (Nov 27).
        assert_eq!(kind(2026, 11, 27), Holiday::EarlyClose);
        // Christmas Eve 2025 (Wed) is an early close.
        assert_eq!(kind(2025, 12, 24), Holiday::EarlyClose);
    }

    #[test]
    fn normal_trading_day_is_open() {
        assert_eq!(kind(2026, 7, 8), Holiday::Open); // ordinary Wednesday
    }
}

#[cfg(test)]
mod tests {
    use super::states_a_skip;

    /// A phase that names itself and declines in one line is both, and one that
    /// only names itself is neither. Read off the announcement, so a phase
    /// added later cannot be counted wrongly by reaching for the wrong macro.
    /// A phase that reports two of its parts missing skipped once. Counting
    /// the lines would report more skips than there are phases to skip.
    ///
    /// The baseline is retaken here and the suite retakes its own when it
    /// starts, so this leaves nothing behind for the summary to read.
    #[test]
    fn a_phase_saying_it_skipped_twice_is_counted_once() {
        super::take_phase_baseline();
        super::note_phase_announced("--- Phase 95: Scanner ---");
        super::note_phase_skipped();
        super::note_phase_skipped();
        assert_eq!(super::phase_tally_since_baseline(), (1, 1));

        super::note_phase_announced("--- Phase 96: Something else ---");
        assert_eq!(
            super::phase_tally_since_baseline(),
            (2, 1),
            "announcing a phase counted a skip it did not report",
        );
    }

    #[test]
    fn an_announcement_is_read_for_its_own_verdict() {
        assert!(states_a_skip("--- Phase 2: Market Data Ticks ---\n  SKIP: Closed — no ticks\n"));
        assert!(states_a_skip("  SKIP: no sterling listing came back"));
        assert!(!states_a_skip("--- Phase 2: Market Data Ticks (AAPL) ---"));
        assert!(!states_a_skip("--- Phase 123: Global Cancel (3 orders) ---"));
    }
}
