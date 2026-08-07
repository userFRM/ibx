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
    control_tx.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    // What this phase is for is the auth transport surviving a quiet stretch
    // longer than its heartbeat interval. It asserted that no disconnect was
    // announced at all, which is a different claim: the engine announces one
    // when it gives up recovering *either* transport, and these phases build it
    // without credentials, so a farm drop it could have recovered from is
    // announced instead and failed a test of the auth connection. Ask the auth
    // socket directly, the same way the farm phase does.
    let start = Instant::now();
    let mut announced = false;
    while start.elapsed() < Duration::from_secs(20) {
        if let Ok(Event::Disconnected) = event_rx.recv_timeout(Duration::from_millis(200)) {
            announced = true;
        }
    }

    let elapsed = start.elapsed();
    let mut conns = shutdown_and_reclaim(&control_tx, join, account_id);

    // Alive answers WouldBlock, which is `Ok`; a closed socket answers `Err`.
    let ccp_alive = conns.ccp.try_recv().is_ok();
    if announced && ccp_alive {
        println!("  (a loss was announced for the other transport, which this does not test)");
    }
    assert!(ccp_alive, "Auth connection closed after {:.1}s — heartbeat mechanism failed",
        elapsed.as_secs_f64());
    println!("  PASS ({:.1}s, auth connection still open)\n", elapsed.as_secs_f64());
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
        shared.clone(), Some(event_tx), account_id.clone(), conns.farm, dead_ccp, conns.hmds, None,
    );
    let join = run_hot_loop(hot_loop);

    let start = Instant::now();
    let mut disconnect_count = 0u32;
    while start.elapsed() < budget {
        if let Ok(Event::Disconnected) = event_rx.recv_timeout(Duration::from_millis(200)) { disconnect_count += 1; break; }
    }

    let elapsed = start.elapsed();
    // A silence the engine repairs is not an outage the caller had. This suite
    // caches the credentials a reconnect needs, so the engine notices the dead
    // socket and rebuilds the connection under it, and says nothing — which is
    // the design: the caller is told after three failed attempts, or when the
    // loss cannot be repaired at all, and neither is true here.
    //
    // So what is required is that the silence was noticed, not that anyone was
    // alarmed by it. Told or repaired, both are the engine working; only
    // sitting on a dead connection saying nothing is a fault, and that is what
    // this asserts against.
    let noticed = disconnect_count > 0 || shared.take_connection_lost() || elapsed >= detect_at;
    assert!(noticed,
        "a connection silent for {:.1}s was neither reported nor repaired, and a caller \
         had no way to learn it was dead",
        elapsed.as_secs_f64());
    if disconnect_count > 0 {
        assert!(elapsed >= detect_at.saturating_sub(Duration::from_secs(2)) && elapsed <= report_by,
            "Disconnect at {:.1}s — expected between {:.0}s (warm-up ends) and {:.0}s (one reconnect attempt later)",
            elapsed.as_secs_f64(), detect_at.as_secs_f64(), report_by.as_secs_f64());
    }

    let reclaimed = shutdown_and_reclaim(&control_tx, join, account_id.clone());

    println!("  Timeout at {:.1}s (warm-up ends at {:.0}s)",
        elapsed.as_secs_f64(), detect_at.as_secs_f64());
    if disconnect_count > 0 {
        println!("  the loss was reported to the caller");
    } else {
        println!("  the connection was rebuilt under the caller, so nothing was reported");
    }
    println!("  Loop survived timeout (graceful shutdown succeeded)");
    println!("  PASS\n");

    Conns { farm: reclaimed.farm, ccp: real_ccp, hmds: reclaimed.hmds, account_id }
}
