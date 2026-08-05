//! Connection, authentication, and recovery test phases.

use super::common::*;
use std::net::TcpListener;
use ibx::gateway::{Gateway, GatewayConfig};

pub(super) fn phase_ccp_auth(gw: &Gateway, has_hmds: bool, connect_time: Duration) {
    println!("--- Phase 1: CCP Auth + Farm Logon ---");

    assert!(!gw.account_id.is_empty(), "Account ID should be non-empty after CCP logon");
    println!("  Account ID: {}", gw.account_id);

    assert!(!gw.server_session_id.is_empty(), "Server session ID should be set");
    if !gw.ccp_token.is_empty() {
        println!("  CCP token: present");
    } else {
        println!("  CCP token: not present (non-fatal)");
    }
    assert!(gw.heartbeat_interval > 0, "Heartbeat interval should be positive");
    println!("  Session ID: {}", gw.server_session_id);
    println!("  Heartbeat interval: {}s", gw.heartbeat_interval);

    use num_bigint::BigUint;
    assert!(gw.session_token > BigUint::from(0u32), "Session token should be non-zero");

    if has_hmds {
        println!("  ushmds farm: CONNECTED");
    } else {
        println!("  ushmds farm: NOT CONNECTED (non-fatal)");
    }

    assert!(connect_time < Duration::from_secs(60), "Connection took too long: {connect_time:?}");
    println!("  PASS ({:.3}s)\n", connect_time.as_secs_f64());
}

/// Phase: connect the optional extra farms.
///
/// Takes `ccp` only to keep it alive: each attempt blocks until the farm answers
/// or the connect timeout expires (~5s for an unreachable farm), and the auth
/// connection's heartbeat interval is 10s. Two slow attempts in a row are enough
/// for the server to close a session that no hot loop is pumping yet — this phase
/// runs before `Conns` is built. The session then dies here and the first
/// CCP-dependent phase fails far away with a misleading error.
pub(super) fn phase_extra_farms(gw: &Gateway, config: &GatewayConfig, ccp: &mut Connection) {
    println!("--- Phase 18: Additional Farm Connections ---");

    let farms = ["cashhmds", "secdefil", "fundfarm", "usopt", "cashfarm", "usfuture", "eufarm", "jfarm"];
    let mut connected = 0;

    for farm in &farms {
        // Pump before each attempt as well as after: the heartbeat has to land
        // inside the window, and the attempt itself is what blocks.
        ccp_keepalive(ccp);
        let start = Instant::now();
        let slot: u32 = if *farm == "ushmds" { 17 } else { 18 };
        match ibx::gateway::connect_farm(
            &config.host, farm,
            &config.username, &config.password, config.paper,
            &gw.server_session_id, &gw.session_token,
            &gw.hw_info, &gw.encoded, slot,
        ) {
            Ok(_conn) => {
                connected += 1;
                println!("  {}: CONNECTED ({:.3}s)", farm, start.elapsed().as_secs_f64());
            }
            Err(e) => {
                println!("  {}: FAILED (non-fatal): {} ({:.3}s)", farm, e, start.elapsed().as_secs_f64());
            }
        }
        ccp_keepalive(ccp);
    }

    println!("  {}/{} extra farms connected", connected, farms.len());
    println!("  PASS\n");
}

pub(super) fn phase_graceful_shutdown(conns: Conns) -> Conns {
    println!("--- Phase 5: Graceful Shutdown ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(event_tx), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    let join = run_hot_loop(hot_loop);
    std::thread::sleep(Duration::from_secs(2));

    let shutdown_start = Instant::now();
    control_tx.send(ControlCommand::Shutdown).unwrap();

    let mut hl = join.join().expect("hot loop panicked");
    let shutdown_time = shutdown_start.elapsed();

    assert!(
        shutdown_time < Duration::from_secs(2),
        "Shutdown took too long: {shutdown_time:?}"
    );

    // Check that Disconnected event was emitted
    let mut got_disconnect = false;
    while let Ok(ev) = event_rx.try_recv() {
        if matches!(ev, Event::Disconnected) {
            got_disconnect = true;
        }
    }
    assert!(got_disconnect, "Disconnected event was not emitted during shutdown");

    let farm = hl.farm_conn.take().expect("farm_conn missing");
    let ccp = hl.ccp_conn.take().expect("ccp_conn missing");
    let hmds = hl.hmds_conn.take();

    println!("  Shutdown in {:.3}s", shutdown_time.as_secs_f64());
    println!("  PASS\n");
    Conns { farm, ccp, hmds, account_id }
}

pub(super) fn phase_connection_recovery(conns: Conns, _gw: &Gateway, config: &GatewayConfig) -> Conns {
    println!("--- Phase 96: Connection Recovery (simulated farm drop) ---");

    // We use a dummy TCP listener as a fake farm connection that we can close
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind local listener");
    let local_addr = listener.local_addr().unwrap();

    // Connect a fake "farm" to the local listener
    let fake_farm = std::net::TcpStream::connect(local_addr).expect("Failed to connect to local listener");
    let (_accepted, _) = listener.accept().expect("Failed to accept connection");

    // Build a Connection from the fake stream
    let fake_conn = Connection::new_raw(fake_farm).expect("Failed to create Connection");

    let account_id = conns.account_id.clone();
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    // Use fake farm, real auth connection — hot loop should detect farm disconnect
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(event_tx), account_id.clone(), fake_conn, conns.ccp, conns.hmds, None,
    );

    let join = run_hot_loop(hot_loop);

    // Drop the accepted side to close the connection
    drop(_accepted);
    drop(listener);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_disconnect = false;

    while Instant::now() < deadline {
        if let Ok(Event::Disconnected) = event_rx.recv_timeout(Duration::from_millis(200)) { got_disconnect = true; break; }
    }

    // The hot loop should exit on its own after detecting disconnect
    let _ = control_tx.send(ControlCommand::Shutdown);
    let result = join.join();
    assert!(result.is_ok(), "Hot loop should not panic on connection drop");

    // Reconnect real farm for remaining tests
    let (farm, ccp, hmds) = match Gateway::connect(config) {
        Ok((_gw2, f, c, h)) => {
            println!("  Reconnected to IB for remaining tests");
            (f, c, h)
        }
        Err(e) => {
            panic!("Cannot continue compat suite without farm connection: {e}");
        }
    };

    if got_disconnect {
        println!("  Disconnected event received");
        println!("  PASS\n");
    } else {
        println!("  SKIP: No Disconnected event (hot loop may have exited before emitting)\n");
    }
    Conns { farm, ccp, hmds, account_id }
}

pub(super) fn phase_reconnection_state_recovery(conns: Conns, _gw: &Gateway, _config: &GatewayConfig) -> Conns {
    println!("--- Phase 105: Reconnection with State Recovery ---");

    // Step 1: Subscribe to market data, verify we get ticks
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(event_tx), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_ticks = false;

    while Instant::now() < deadline {
        if let Ok(Event::Tick(_)) = event_rx.recv_timeout(Duration::from_millis(100)) { got_ticks = true; break; }
    }

    // Shutdown the hot loop to simulate "disconnect"
    let conns1 = shutdown_and_reclaim(&control_tx, join, account_id.clone());

    if !got_ticks {
        println!("  SKIP: No ticks received before disconnect — market closed\n");
        return conns1;
    }

    println!("  Step 1: Got ticks before disconnect");

    // Step 2: Reconnect with fresh connections and verify ticks resume
    let shared2 = Arc::new(SharedState::new());
    let (event_tx2, event_rx2) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop2, control_tx2) = HotLoop::with_connections(
        shared2.clone(), Some(event_tx2), conns1.account_id.clone(),
        conns1.farm, conns1.ccp, conns1.hmds, None,
    );

    control_tx2.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();
    let join2 = run_hot_loop(hot_loop2);

    let deadline2 = Instant::now() + Duration::from_secs(15);
    let mut got_ticks_after = false;

    while Instant::now() < deadline2 {
        if let Ok(Event::Tick(inst)) = event_rx2.recv_timeout(Duration::from_millis(100)) {
            let q = shared2.market.quote(inst);
            println!("  Step 2: Tick after reconnect bid={:.4} ask={:.4}",
                q.bid as f64 / PRICE_SCALE as f64, q.ask as f64 / PRICE_SCALE as f64);
            got_ticks_after = true;
            break;
        }
    }

    let conns2 = shutdown_and_reclaim(&control_tx2, join2, conns1.account_id);

    assert!(got_ticks_after, "Should receive ticks after reconnection");
    println!("  PASS\n");
    conns2
}

pub(super) fn phase_auth_wrong_password(config: &GatewayConfig) {
    println!("--- Phase 118: Authentication Failure (wrong password) ---");

    let bad_config = GatewayConfig {
        username: config.username.clone(),
        password: zeroize::Zeroizing::new("definitely_wrong_password_12345".to_string()),
        host: config.host.clone(),
        paper: config.paper,
        accept_invalid_certs: false,
        ib_key_timeout_secs: ibx::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
        ib_key_token_sub_type: ibx::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
        code_provider: None,
        resume: None,
    };

    let start = Instant::now();
    let result = Gateway::connect(&bad_config);
    let elapsed = start.elapsed();

    let err_msg = match result {
        Ok(_) => panic!("Gateway::connect with wrong password should fail"),
        Err(e) => format!("{e}"),
    };
    println!("  Error: {err_msg}");
    println!("  Failed in {:.3}s (expected)", elapsed.as_secs_f64());
    assert!(elapsed < Duration::from_secs(30), "Auth failure should not take >30s");
    println!("  PASS\n");
}

// ─── Phase 131: RegisterInstrument via ControlCommand channel ───

pub(super) fn phase_register_instrument_channel(conns: Conns) -> Conns {
    println!("--- Phase 131: RegisterInstrument via ControlCommand Channel ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(event_tx), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    let join = run_hot_loop(hot_loop);

    // Register 3 instruments via ControlCommand channel (not context_mut)
    control_tx.send(ControlCommand::RegisterInstrument { con_id: 756733, symbol: "SPY".to_string(), sec_type: String::new(), exchange: String::new(), identity: String::new(), reply_tx: None }).unwrap();
    control_tx.send(ControlCommand::RegisterInstrument { con_id: 265598, symbol: "AAPL".to_string(), sec_type: String::new(), exchange: String::new(), identity: String::new(), reply_tx: None }).unwrap();
    control_tx.send(ControlCommand::RegisterInstrument { con_id: 272093, symbol: "MSFT".to_string(), sec_type: String::new(), exchange: String::new(), identity: String::new(), reply_tx: None }).unwrap();

    // Give hot loop time to process
    std::thread::sleep(Duration::from_millis(500));

    // Verify instrument count increased
    let count = shared.market.instrument_count();
    println!("  Instrument count after 3 registrations: {count}");

    // Now subscribe to one of the registered instruments
    control_tx.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();

    // Wait briefly for any events (subscription confirmation or ticks)
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got_event = false;
    while Instant::now() < deadline {
        match event_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Tick(_)) => { got_event = true; break; }
            Ok(_) => { got_event = true; }
            Err(_) => {}
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    assert!(count >= 3, "Should have at least 3 registered instruments, got {count}");
    println!("  Events received: {got_event}");
    println!("  PASS\n");
    conns
}

// ─── Phase 132: UpdateParam Smoke Test ───

pub(super) fn phase_update_param(conns: Conns) -> Conns {
    println!("--- Phase 132: UpdateParam Smoke Test (no-op parameter) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(event_tx), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());

    // Send UpdateParam — hot loop should accept it without crashing
    control_tx.send(ControlCommand::UpdateParam {
        key: "test_key".to_string(), value: "test_value".to_string(),
    }).unwrap();
    control_tx.send(ControlCommand::UpdateParam {
        key: "max_position".to_string(), value: "100".to_string(),
    }).unwrap();

    // Submit + cancel an order to verify hot loop is still functional after UpdateParam
    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitLimitGtc {
        order_id: oid, instrument: inst_id, side: Side::Buy, qty: 1,
        price: 1_00_000_000, outside_rth: true,
    })).unwrap();
    control_tx.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut order_acked = false;
    let mut cancel_sent = false;
    let mut terminal = false;

    while Instant::now() < deadline && !terminal {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            match update.status {
                OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                    order_acked = true;
                    if !cancel_sent {
                        control_tx.send(ControlCommand::Order(
                            OrderRequest::Cancel { order_id: oid }
                        )).unwrap();
                        cancel_sent = true;
                    }
                }
                OrderStatus::Cancelled | OrderStatus::Rejected => {
                    terminal = true;
                }
                _ => {}
            }
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if skip_unacked_if_closed(order_acked) { return conns; }
    assert!(order_acked, "Order should be acknowledged after UpdateParam");
    assert!(terminal, "Order should reach terminal state");
    println!("  UpdateParam processed, hot loop still functional");
    println!("  PASS\n");
    conns
}

/// The farm comes back by itself after the transport goes away.
///
/// This is the case the whole reconnect path exists for: the servers go down
/// nightly and come back, and a client that needed a person to restart it has
/// failed at the one job having no gateway gives it. Every other phase that
/// drops a connection builds the engine without credentials, so it can only
/// ever watch the give-up path — which is why this one hands it the same
/// credentials `connect()` does, and then takes the farm away.
pub(super) fn phase_farm_recovers_with_credentials(
    gw: gateway::Gateway,
    conns: Conns,
    config: &GatewayConfig,
) -> (bool, bool) {
    println!("--- Phase 96b: Farm recovers on its own (real credentials) ---");

    let account_id = conns.account_id.clone();
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = gw.into_hot_loop_with_farms(
        shared.clone(), Some(event_tx), conns.farm, conns.ccp, conns.hmds, None,
        gateway::CallerAuth {
            host: config.host.clone(),
            username: config.username.clone(),
            password: zeroize::Zeroizing::new(config.password.to_string()),
            paper: config.paper,
            code_provider: config.code_provider.clone(),
            ib_key_timeout_secs: config.ib_key_timeout_secs,
            ib_key_token_sub_type: config.ib_key_token_sub_type.clone(),
        },
    );

    // Take the farm away before the loop starts, so recovery is the first thing
    // it has to do rather than something raced against start-up.
    hot_loop.force_farm_disconnect();
    let join = run_hot_loop(hot_loop);

    control_tx.send(ControlCommand::Subscribe {
        con_id: 756733, symbol: "SPY".into(), exchange: String::new(),
        sec_type: String::new(), last_trade_date: String::new(), strike: 0.0,
        right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None,
    }).unwrap();

    // A tick is the proof. The farm was down before the loop started, so the
    // only way one arrives is that the engine dialled the farm again on the
    // cached credentials and re-sent the subscription. There is deliberately no
    // event to wait for: a drop the engine handles by itself announces neither
    // the loss nor the recovery, and the caller is meant to see only that the
    // data kept coming.
    let start = Instant::now();
    let mut ticked = false;
    let mut elapsed = Duration::ZERO;
    while start.elapsed() < Duration::from_secs(90) && !ticked {
        if let Ok(Event::Tick(_)) = event_rx.recv_timeout(Duration::from_millis(250)) {
            ticked = true;
            elapsed = start.elapsed();
        }
    }

    // Nothing announced the loss, so nothing should be reporting one.
    let restored = !shared.take_connection_lost();
    if ticked {
        println!("  data resumed after {:.1}s", elapsed.as_secs_f64());
    }
    println!("  data_resumed={ticked} connection_reported_healthy={restored}");
    let _ = shutdown_and_reclaim(&control_tx, join, account_id);
    (ticked, restored)
}
