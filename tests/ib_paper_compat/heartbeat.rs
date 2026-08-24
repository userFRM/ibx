//! Heartbeat keepalive and timeout detection test phases.

use super::common::*;
use std::net::TcpListener;

pub(super) fn phase_heartbeat_keepalive(conns: Conns) -> Conns {
    phase!("--- Phase 13: Heartbeat Keepalive (20s > CCP 10s interval) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, regulatory_snapshot: false, reply_tx: None }).unwrap();
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

    // Asked rather than inferred. One read that hands back bytes is not a live
    // connection: a venue closing one says goodbye before it sends the close,
    // so the phase read the goodbye, called the socket healthy, and proved
    // nothing about the heartbeat it is named for. This drains what is queued
    // and then requires the venue to answer a test request.
    let ccp_alive = ccp_still_carrying(&mut conns.ccp);
    if announced && ccp_alive {
        println!("  (a loss was announced for the other transport, which this does not test)");
    }
    assert!(ccp_alive, "Auth connection closed after {:.1}s — heartbeat mechanism failed",
        elapsed.as_secs_f64());
    println!("  PASS ({:.1}s, auth connection still open)\n", elapsed.as_secs_f64());
    conns
}

pub(super) fn phase_farm_heartbeat_keepalive(conns: Conns) -> Conns {
    phase!("--- Phase 55: Farm Heartbeat Keepalive (65s > 2x farm 30s interval) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
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

    // As above, the socket is read out rather than sampled once: the bytes
    // that arrive before a close arrive first, and one read of them reported a
    // farm that had already gone as one the heartbeat had kept. The farm is
    // not asked a test request the way the auth connection is — nothing here
    // has seen it answer one — so this is what can be shown and no more.
    let farm_alive = drained_without_closing(&mut conns.farm);
    if auth_dropped {
        println!("  (the auth connection dropped during this phase, which is the other transport)");
    }
    assert!(farm_alive, "Farm socket closed after {:.1}s — heartbeat failed", elapsed.as_secs_f64());
    println!("  PASS ({:.1}s, farm still open across 2x its heartbeat interval)\n", elapsed.as_secs_f64());
    conns
}

pub(super) fn phase_heartbeat_timeout_detection(conns: Conns) -> Conns {
    phase!("--- Phase 56: Heartbeat Timeout Detection (simulated stale CCP) ---");

    // This phase takes the session away itself: it parks the real connection
    // behind a dead socket to measure how long the engine takes to notice. A
    // loss while this is held is the phase's own doing, so it is not counted
    // against the run the way an unasked-for one is.
    let _taking_it_away = super::common::TakingTheSessionAway::begin();

    // Read the deadline off the engine rather than restating it. A version of
    // this phase spelled out the thresholds it was written against and then
    // failed for a year's worth of runs after they were widened to match the
    // gateway's — reporting a broken timeout when the timeout was fine and
    // the arithmetic here was not.
    //
    // Nothing has been received since the connection was made, so detection
    // lands on the dead threshold itself, and the report follows one reconnect
    // attempt later.
    use ibx::engine::hot_loop::LIVENESS_DEAD_SECS;
    let detect_at = Duration::from_secs(LIVENESS_DEAD_SECS);
    let report_by = detect_at + Duration::from_secs(25);
    let budget = report_by + Duration::from_secs(15);

    let account_id = conns.account_id;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind localhost");
    let addr = listener.local_addr().unwrap();
    let client = std::net::TcpStream::connect(addr).expect("connect to localhost");
    let mut server = listener.accept().expect("accept dead socket").0;
    // The far end of the dead socket, read to see whether the engine let go of
    // it. A deadline, so a socket still held reads as nothing rather than
    // blocking here for the rest of the phase.
    server
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("a deadline on the dead socket");
    let dead_ccp = Connection::new_raw(client).expect("wrap dead socket as Connection");
    let real_ccp = conns.ccp;

    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, dead_ccp, conns.hmds, None,
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
    // Whether the engine let go of the dead connection. It replaces the
    // connection when it rebuilds one, and the socket underneath closes with
    // it, so the far end reading end-of-file is the engine having noticed.
    // Reading the clock instead asserted nothing: the loop above runs to its
    // budget, which is past the threshold by construction, so "it has been
    // long enough" was true whatever the engine did.
    let released = {
        use std::io::Read;
        let mut byte = [0u8; 1];
        loop {
            match server.read(&mut byte) {
                Ok(0) => break true,
                // A heartbeat still going onto the socket: the engine has not
                // let go of it yet.
                Ok(_) => continue,
                Err(_) => break false,
            }
        }
    };
    let noticed = disconnect_count > 0 || shared.take_connection_lost() || released;
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
    } else if released {
        println!("  the connection was rebuilt under the caller, so nothing was reported");
    } else {
        println!("  the loss was recorded on the session");
    }
    println!("  Loop survived timeout (graceful shutdown succeeded)");
    println!("  PASS\n");

    Conns { farm: reclaimed.farm, ccp: real_ccp, hmds: reclaimed.hmds, account_id }
}
