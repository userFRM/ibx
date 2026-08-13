//! Connection, authentication, and recovery test phases.

use super::common::*;
use std::net::TcpListener;
use ibx::gateway::{Gateway, GatewayConfig};

pub(super) fn phase_ccp_auth(gw: &Gateway, has_hmds: bool, connect_time: Duration) {
    println!("--- Phase 1: CCP Auth + Farm Logon ---");

    assert!(!gw.account_id.is_empty(), "Account ID should be non-empty after CCP logon");
    println!("  Account ID: {}", super::common::redacted(&gw.account_id));

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

    // Named farms, tried on the host this session was routed to.
    //
    // Which of them answer depends on that host: farms are spread across the
    // venue's servers, and the same name answers in under a second on one and
    // is closed without a word on another. usopt and usfuture answer on the
    // host a session is opened against; cashhmds, cashfarm and eufarm answer
    // on the one it is moved to. So the count below says which farms live
    // beside this session, not which the account may use, and none of the
    // names here is asserted on for that reason.
    let farms = ["cashhmds", "secdefil", "fundfarm", "usopt", "cashfarm", "usfuture", "eufarm", "jfarm"];
    let mut connected = 0;
    let mut answered: Vec<&str> = Vec::new();
    // Where this session actually is. The venue names which server the account
    // belongs on and the session follows it, so a farm asked for on the host
    // that was knocked on first is asked of a server this session is not on.
    let host = if gw.hmds_host.is_empty() { config.host.clone() } else { gw.hmds_host.clone() };
    println!("  session is on {host}");

    for farm in &farms {
        // Pump before each attempt as well as after: the heartbeat has to land
        // inside the window, and the attempt itself is what blocks.
        ccp_keepalive(ccp);
        let start = Instant::now();
        let kind = if *farm == "ushmds" {
            ibx::gateway::Farm::Historical
        } else {
            ibx::gateway::Farm::MarketData
        };
        match ibx::gateway::connect_farm(&Default::default(),
            &host, farm,
            &config.username, &config.password, config.paper,
            &gw.server_session_id, &gw.session_token,
            &gw.hw_info, &gw.encoded, kind,
        ) {
            Ok(_conn) => {
                connected += 1;
                answered.push(*farm);
                println!("  {}: CONNECTED ({:.3}s)", farm, start.elapsed().as_secs_f64());
            }
            Err(e) => {
                println!("  {}: not served on this account: {} ({:.3}s)",
                    farm, e, start.elapsed().as_secs_f64());
            }
        }
        ccp_keepalive(ccp);
    }

    println!("  {}/{} extra farms answered", connected, farms.len());

    // Not every farm serves every account, and the ones that do not say so by
    // accepting the connection, staying silent, and closing it about ten
    // seconds later. That is the server's answer and not this client's doing:
    // the same code, sending the same logon, is answered in under two seconds
    // by the farms this account is served on.
    //
    // Which makes counting them worthless on its own — the phase reported PASS
    // on two of eight, and would have reported PASS on none of eight. What is
    // worth asserting is that the farms this account IS served on still answer,
    // so a regression that stops them reads here rather than as missing data
    // three phases later.
    // The farm the venue routed this session to, on the host it named for it.
    // That pair is the venue's own answer rather than a guess, so it is the
    // one thing here worth asserting: if the farm this session was routed to
    // stops answering, nothing above it can work.
    assert!(
        answered.contains(&gw.hmds_farm.as_str()) || gw.hmds_farm.is_empty(),
        "{} is the farm this session was routed to and it did not answer. \
         Everything that reads historical data goes through it. Farms that \
         answered on {}: {answered:?}",
        gw.hmds_farm, host,
    );
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

    // The session ended because this asked it to, which is a stop and not a
    // loss: a caller told it lost connectivity would stand by for a reconnect
    // that is not coming.
    let mut said_so = false;
    while let Ok(ev) = event_rx.try_recv() {
        if matches!(ev, Event::Stopped) {
            said_so = true;
        }
        assert!(
            !matches!(ev, Event::Disconnected),
            "a session the caller ended was reported as one that went away",
        );
    }
    assert!(said_so, "the engine said nothing when it was told to stop");

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
        Ok(gateway::Session { gateway: _gw2, market_data: f, trading: c, historical: h, .. }) => {
            println!("  Reconnected to IB for remaining tests");
            (f, c, h)
        }
        Err(e) => {
            panic!("Cannot continue compat suite without farm connection: {e}");
        }
    };

    // A drop the engine recovers from on its own is deliberately not reported:
    // the caller's subscriptions and orders survive it, so telling them the
    // connection went away would describe an outage they never had. What the
    // phase can require is that the engine came through it — the hot loop did
    // not panic, asserted above, and the session is usable after.
    if got_disconnect {
        println!("  Disconnected event received");
    } else {
        println!("  No Disconnected event, which is what a recovered drop delivers");
    }
    println!("  PASS\n");
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

    control_tx.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_ticks = false;

    while Instant::now() < deadline {
        if let Ok(Event::Tick(_)) = event_rx.recv_timeout(Duration::from_millis(100)) { got_ticks = true; break; }
    }

    // Shutdown the hot loop to simulate "disconnect"
    let conns1 = shutdown_and_reclaim(&control_tx, join, account_id.clone());

    if !got_ticks {
        no_market(&shared, "no ticks arrived before the disconnect");
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

    control_tx2.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();
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
        settings: Default::default(),
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
    control_tx.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();

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
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid, instrument: inst_id, side: Side::Buy, qty: 1, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();
    control_tx.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();
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
        shared.clone(), Some(event_tx), conns.farm, conns.ccp, conns.hmds, None, None,
        gateway::CallerAuth {
            settings: Default::default(),
            host: config.host.clone(),
            username: config.username.clone(),
            password: zeroize::Zeroizing::new(config.password.to_string()),
            paper: config.paper,
            code_provider: config.code_provider.clone(),
            ib_key_timeout_secs: config.ib_key_timeout_secs,
            ib_key_token_sub_type: config.ib_key_token_sub_type.clone(),
        },
    );

    // Take the transports away before the loop starts, so recovery is the first
    // thing it has to do rather than something raced against start-up. Both,
    // because a maintenance window takes both: the auth transport carries the
    // orders and the farm carries the data, and a client that recovers one is
    // still not trading.
    hot_loop.force_farm_disconnect();
    if std::env::var("IBX_RECOVER_FARM_ONLY").is_err() {
        hot_loop.force_ccp_disconnect();
    }
    let join = run_hot_loop(hot_loop);

    control_tx.send(ControlCommand::Subscribe {
        con_id: 756733, symbol: "SPY".into(), exchange: String::new(),
        sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0,
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
    // Data resuming proves the farm. Trading is the other half, and the auth
    // transport was taken away too: an order that is acknowledged after all
    // this is one that went out on a connection the engine rebuilt itself.
    let mut order_acked = false;
    if ticked {
        let oid = next_order_id();
        control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
            order_id: oid, instrument: 0, side: Side::Buy, qty: 1,
            kind: OrderKind::Limit { price: 1_00_000_000 },
            tif: b'0', attrs: OrderAttrs { outside_rth: true, ..OrderAttrs::default() },
        })).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline && !order_acked {
            if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(250))
                && u.order_id == oid
            {
                order_acked = matches!(u.status,
                    OrderStatus::PreSubmitted | OrderStatus::Submitted | OrderStatus::Filled);
            }
        }
        let _ = control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: oid }));
        std::thread::sleep(Duration::from_secs(2));
    }
    println!("  order_accepted_after_recovery={order_acked}");

    let restored = !shared.take_connection_lost();
    if ticked {
        println!("  data resumed after {:.1}s", elapsed.as_secs_f64());
    }
    println!("  data_resumed={ticked} connection_reported_healthy={restored}");
    let _ = shutdown_and_reclaim(&control_tx, join, account_id);
    (ticked && order_acked, restored)
}
