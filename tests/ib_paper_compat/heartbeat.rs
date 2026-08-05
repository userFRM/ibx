//! Heartbeat keepalive and timeout detection test phases.

use super::common::*;
use std::net::TcpListener;

pub(super) fn phase_heartbeat_keepalive(conns: Conns) -> Conns {
    println!("--- Phase 13: Heartbeat Keepalive (20s > CCP 10s interval) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(event_tx), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    control_tx.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let start = Instant::now();
    let mut disconnected = false;
    while start.elapsed() < Duration::from_secs(20) {
        if let Ok(Event::Disconnected) = event_rx.recv_timeout(Duration::from_millis(200)) { disconnected = true; break; }
    }

    let elapsed = start.elapsed();
    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    assert!(!disconnected, "Connection dropped after {:.1}s — heartbeat mechanism failed", elapsed.as_secs_f64());
    println!("  PASS ({:.1}s, no disconnect)\n", elapsed.as_secs_f64());
    conns
}

pub(super) fn phase_farm_heartbeat_keepalive(conns: Conns) -> Conns {
    println!("--- Phase 55: Farm Heartbeat Keepalive (65s > 2x farm 30s interval) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(event_tx), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    let join = run_hot_loop(hot_loop);

    // A farm drop is deliberately silent: the engine recovers it without
    // telling anyone, which is what `Event::Disconnected` not being emitted
    // for one means. So waiting for that event tested nothing about the farm,
    // and caught the auth connection dropping instead — a real event, from the
    // other transport, about something this phase does not claim to test.
    //
    // What the farm heartbeat is for is the server not closing the socket
    // while nothing is being asked of it. So: wait out two intervals, then ask
    // the socket.
    let start = Instant::now();
    let mut auth_dropped = false;
    while start.elapsed() < Duration::from_secs(65) {
        if let Ok(Event::Disconnected) = event_rx.recv_timeout(Duration::from_millis(500)) {
            auth_dropped = true;
        }
    }

    let elapsed = start.elapsed();
    let mut conns = shutdown_and_reclaim(&control_tx, join, account_id);

    // Alive answers WouldBlock, which is `Ok`; a closed socket answers `Err`.
    let farm_alive = conns.farm.try_recv().is_ok();
    if auth_dropped {
        println!("  (the auth connection dropped during this phase, which is the other transport)");
    }
    assert!(farm_alive, "Farm socket closed after {:.1}s — heartbeat failed", elapsed.as_secs_f64());
    println!("  PASS ({:.1}s, farm still open across 2x its heartbeat interval)\n", elapsed.as_secs_f64());
    conns
}

pub(super) fn phase_heartbeat_timeout_detection(conns: Conns) -> Conns {
    println!("--- Phase 56: Heartbeat Timeout Detection (simulated stale CCP) ---");

    // Read the deadline off the engine rather than restating it. A version of
    // this phase spelled out the thresholds it was written against and then
    // failed for a year's worth of runs after they were widened to match the
    // gateway's — reporting a broken timeout when the timeout was fine and
    // the arithmetic here was not.
    //
    // Nothing has been received since the connection was made, so the silence
    // is already past the dead threshold when the warm-up ends: detection
    // lands at the warm-up boundary, and the report follows one reconnect
    // attempt later.
    use ibx::engine::hot_loop::{LIVENESS_DEAD_SECS, LIVENESS_WARMUP_SECS};
    let detect_at = Duration::from_secs(LIVENESS_WARMUP_SECS.max(LIVENESS_DEAD_SECS));
    let report_by = detect_at + Duration::from_secs(25);
    let budget = report_by + Duration::from_secs(15);

    let account_id = conns.account_id;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind localhost");
    let addr = listener.local_addr().unwrap();
    let client = std::net::TcpStream::connect(addr).expect("connect to localhost");
    let _server = listener.accept().expect("accept dead socket").0;
    let dead_ccp = Connection::new_raw(client).expect("wrap dead socket as Connection");
    let real_ccp = conns.ccp;

    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(event_tx), account_id.clone(), conns.farm, dead_ccp, conns.hmds, None,
    );
    let join = run_hot_loop(hot_loop);

    let start = Instant::now();
    let mut disconnect_count = 0u32;
    while start.elapsed() < budget {
        if let Ok(Event::Disconnected) = event_rx.recv_timeout(Duration::from_millis(200)) { disconnect_count += 1; break; }
    }

    let elapsed = start.elapsed();
    assert!(disconnect_count > 0,
        "No disconnect after {:.1}s — a silent connection should be reported by {:.0}s",
        elapsed.as_secs_f64(), report_by.as_secs_f64());
    assert!(elapsed >= detect_at.saturating_sub(Duration::from_secs(2)) && elapsed <= report_by,
        "Disconnect at {:.1}s — expected between {:.0}s (warm-up ends) and {:.0}s (one reconnect attempt later)",
        elapsed.as_secs_f64(), detect_at.as_secs_f64(), report_by.as_secs_f64());

    let reclaimed = shutdown_and_reclaim(&control_tx, join, account_id.clone());

    println!("  Timeout at {:.1}s (warm-up ends at {:.0}s)",
        elapsed.as_secs_f64(), detect_at.as_secs_f64());
    println!("  on_disconnect emitted at least once");
    println!("  Loop survived timeout (graceful shutdown succeeded)");
    println!("  PASS\n");

    Conns { farm: reclaimed.farm, ccp: real_ccp, hmds: reclaimed.hmds, account_id }
}
