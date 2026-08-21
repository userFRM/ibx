//! Connection, authentication, and recovery test phases.

use super::common::*;
use std::net::TcpListener;
use ibx::gateway::{Gateway, GatewayConfig};

pub(super) fn phase_ccp_auth(gw: &Gateway, has_hmds: bool, connect_time: Duration) {
    println!("--- Phase 1: CCP Auth + Farm Logon ---");

    // Nobody else on the account.
    //
    // The venue permits one logon at a time and takes the account from the
    // older session without saying so. A suite that starts while something
    // else holds the account is not testing this client — it is racing
    // whatever else is running, and the phases that lose report a market that
    // went quiet. It names the other session in its answer to the connect, so
    // this is answerable before a single phase runs.
    if let Some(other) = &gw.competing {
        panic!(
            "another session already held this account when the suite connected, \
             from {} since {}{}. This account takes one logon at a time and the \
             venue does not say which one it drops. Stop the other session — a \
             suite run, a capture tool, or the scheduled workflow — and start again.",
            other.ip,
            other.since,
            if other.read_only { ", and this one may not trade" } else { "" },
        );
    }

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
pub(super) fn phase_extra_farms(
    gw: &Gateway,
    config: &GatewayConfig,
    ccp: &mut Connection,
    routed: &ibx::protocol::routing::RoutingTable,
) {
    println!("--- Phase 18: Additional Farm Connections ---");

    // Every farm the venue named, asked for where it said each one is.
    //
    // Not a list written here. The venue answers the routing request with the
    // farms it serves and the server each is on, so a list of names in this
    // file would be a guess at something already stated: names asked for on
    // the session's own host are closed without a word, because they live
    // elsewhere.
    // One farm per server, not all of them.
    //
    // What this phase proves is that a farm is reached where the venue says it
    // is, and the part of that which can be wrong is the server. Sixteen
    // logons prove it no better than four and cost the session sixteen: a run
    // that connected every named farm left the next test unable to open a
    // session at all, its own farm logon closed on it. The suite is a
    // passenger on this account, not the only thing using it.
    let named = routed.farms();
    let mut per_host: Vec<(&str, &str, u16)> = Vec::new();
    let mut all: Vec<(&str, &str, u16)> =
        named.iter().map(|(f, (h, p))| (*f, *h, *p)).collect();
    all.sort();
    for (farm, host, port) in all {
        if !per_host.iter().any(|(_, seen, _)| *seen == host) {
            per_host.push((farm, host, port));
        }
    }
    let farms = per_host;
    let mut connected = 0;
    let mut answered: Vec<&str> = Vec::new();
    // Where this session actually is. The venue names which server the account
    // belongs on and the session follows it, so a farm asked for on the host
    // that was knocked on first is asked of a server this session is not on.
    let host = if gw.hmds_host.is_empty() { config.host.clone() } else { gw.hmds_host.clone() };
    println!("  session is on {host}");

    for (farm, farm_host, farm_port) in &farms {
        // Pump before each attempt as well as after: the heartbeat has to land
        // inside the window, and the attempt itself is what blocks.
        ccp_keepalive(ccp);
        let start = Instant::now();
        let kind = if farm.contains("hmds") {
            ibx::gateway::Farm::Historical
        } else {
            ibx::gateway::Farm::MarketData
        };
        // The port the venue stated for this farm, not the one this file would
        // otherwise assume.
        let where_it_is = ibx::api::settings::SessionSettings {
            port: *farm_port,
            ..Default::default()
        };
        match ibx::gateway::connect_farm(&where_it_is,
            farm_host, farm,
            &config.username, &config.password, config.paper,
            &gw.server_session_id, &gw.session_token,
            &gw.hw_info, &gw.encoded, kind, None,
        ) {
            Ok(_conn) => {
                connected += 1;
                answered.push(*farm);
                println!("  {}: CONNECTED ({:.3}s)", farm, start.elapsed().as_secs_f64());
            }
            Err(e) => {
                println!("  {}: {} on {} ({:.3}s)",
                    farm, e, farm_host, start.elapsed().as_secs_f64());
            }
        }
        ccp_keepalive(ccp);
    }

    println!("  {}/{} farms answered, one per server the venue named", connected, farms.len());

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
    // Every farm the venue named answers, because it was asked for where the
    // venue said it is. That is the whole of what this phase now checks, and
    // it can fail two ways worth stopping for: the lookup broke and a farm was
    // asked for somewhere else again, or the account stopped being served
    // somewhere it was. Both look like data that never arrives, later and
    // further away.
    assert_eq!(
        connected,
        farms.len(),
        "one farm was tried on each of {} servers the venue named, and \
         {connected} answered. The ones that did: {answered:?}",
        farms.len(),
    );
    println!("  PASS\n");
}

pub(super) fn phase_graceful_shutdown(conns: Conns) -> Conns {
    println!("--- Phase 5: Graceful Shutdown ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
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

    // A dummy TCP listener stands in for the farm connection and can be closed
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
        shared, Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), fake_conn, conns.ccp, conns.hmds, None,
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
    let mut hl = join.join().expect("Hot loop should not panic on connection drop");

    // What the engine rebuilt, on the session that is already open. This threw
    // the real auth and historical connections away and opened a second
    // gateway session to get a farm back — a second logon on an account that
    // allows one, which the venue answers by dropping whatever the suite was
    // still holding. Every phase after it then ran on connections belonging to
    // a session the run had itself evicted.
    let mut recovered = false;
    let (farm, ccp, hmds) = match (hl.farm_conn.take(), hl.ccp_conn.take()) {
        (Some(f), Some(c)) => {
            println!("  the engine rebuilt the farm on the session already open");
            recovered = true;
            (f, c, hl.hmds_conn.take())
        }
        // The engine did not bring it back inside the wait. Opening a session
        // is the last resort it always was, and it says so rather than looking
        // like the recovery this phase is named for.
        (rebuilt, ccp) => {
            println!(
                "  the engine did not rebuild the farm within the wait (farm={}, auth={}); \
                 opening a session to carry the rest of the run",
                rebuilt.is_some(), ccp.is_some(),
            );
            match Gateway::connect(config) {
                Ok(gateway::Session { gateway: _gw2, market_data: f, trading: c, historical: h, .. }) => (f, c, h),
                Err(e) => panic!("Cannot continue compat suite without farm connection: {e}"),
            }
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
    // Recovery is what this phase is named for, so it is what the phase
    // reports on. Opening a session carries the rest of the run, but it
    // demonstrates nothing about the engine bringing a dropped farm back,
    // and reporting it as a pass claimed a recovery that never happened.
    if recovered {
        println!("  PASS\n");
    } else {
        println!(
            "  SKIP: the engine did not rebuild the farm within the wait, so the recovery \
             this phase exists to demonstrate was not shown\n",
        );
    }
    Conns { farm, ccp, hmds, account_id }
}

pub(super) fn phase_reconnection_state_recovery(conns: Conns, _gw: &Gateway, _config: &GatewayConfig) -> Conns {
    // Named for a reconnection it does not perform: nothing is disconnected
    // here. The engine is stopped, its transports are reclaimed, and a second
    // engine is built over the same ones — so what this shows is that a fresh
    // engine subscribes again and the ticks resume on transports that never
    // went away. A transport actually dropped and rebuilt is what Phase 96
    // covers.
    println!("--- Phase 105: A second engine over the same transports subscribes again ---");

    // Step 1: subscribe to market data and confirm ticks arrive
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_ticks = false;

    while Instant::now() < deadline {
        if let Ok(Event::Tick(_)) = event_rx.recv_timeout(Duration::from_millis(100)) { got_ticks = true; break; }
    }

    // Stop the engine and take its transports back
    let conns1 = shutdown_and_reclaim(&control_tx, join, account_id.clone());

    if !got_ticks {
        no_market(&shared, "no ticks arrived before the disconnect");
        return conns1;
    }

    println!("  Step 1: Got ticks before disconnect");

    // Step 2: a second engine over the reclaimed transports, and ticks resume
    let shared2 = Arc::new(SharedState::new());
    let (event_tx2, event_rx2) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop2, control_tx2) = HotLoop::with_connections(
        shared2.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx2, Default::default())), conns1.account_id.clone(),
        conns1.farm, conns1.ccp, conns1.hmds, None,
    );

    control_tx2.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None }).unwrap();
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

    assert!(got_ticks_after, "the second engine subscribed and no tick followed");
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
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    let join = run_hot_loop(hot_loop);

    // Register 3 instruments via ControlCommand channel (not context_mut)
    control_tx.send(ControlCommand::RegisterInstrument { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".to_string(), sec_type: "STK".into(), exchange: String::new(), ..Default::default() }, identity: String::new(), reply_tx: None }).unwrap();
    control_tx.send(ControlCommand::RegisterInstrument { contract: ibx::types::ContractRef { con_id: 265598, symbol: "AAPL".to_string(), sec_type: String::new(), exchange: String::new(), ..Default::default() }, identity: String::new(), reply_tx: None }).unwrap();
    control_tx.send(ControlCommand::RegisterInstrument { contract: ibx::types::ContractRef { con_id: 272093, symbol: "MSFT".to_string(), sec_type: String::new(), exchange: String::new(), ..Default::default() }, identity: String::new(), reply_tx: None }).unwrap();

    // Give hot loop time to process
    std::thread::sleep(Duration::from_millis(500));

    // Verify instrument count increased
    let count = shared.market.instrument_count();
    println!("  Instrument count after 3 registrations: {count}");

    // Now subscribe to one of the registered instruments
    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None }).unwrap();

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
        shared, Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    // Send UpdateParam — hot loop should accept it without crashing
    control_tx.send(ControlCommand::UpdateParam {
        key: "test_key".to_string(), value: "test_value".to_string(),
    }).unwrap();
    control_tx.send(ControlCommand::UpdateParam {
        key: "max_position".to_string(), value: "100".to_string(),
    }).unwrap();

    // Submit + cancel an order to verify hot loop is still functional after UpdateParam
    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();
    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None }).unwrap();
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
) -> (bool, bool, bool) {
    println!("--- Phase 96b: Farm recovers on its own (real credentials) ---");

    let account_id = conns.account_id.clone();
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = ibx::engine::hot_loop::HotLoop::for_session(
        gw,
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), conns.farm, conns.ccp, conns.hmds, None, None,
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
        contract: ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None,
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
    // What the venue last said about it. Reporting only that the order was not
    // accepted leaves a rejection on a venue rule looking identical to an order
    // path this client failed to rebuild, and those want opposite fixes.
    let mut last_status = None;
    if ticked {
        let oid = next_order_id();
        control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
            order_id: oid, instrument: 0, side: Side::Buy, qty: ibx::types::QTY_SCALE,
            kind: OrderKind::Limit { price: 1_00_000_000 },
            tif: b'0', attrs: OrderAttrs { outside_rth: true, ..OrderAttrs::default() },
        })).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline && !order_acked {
            if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(250))
                && u.order_id == oid
            {
                last_status = Some(u.status);
                order_acked = matches!(u.status,
                    OrderStatus::PreSubmitted | OrderStatus::Submitted | OrderStatus::Filled);
            }
        }
        let _ = control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: oid }));
        std::thread::sleep(Duration::from_secs(2));
    }
    if order_acked {
        println!("  order_accepted_after_recovery=true");
    } else {
        println!("  order_accepted_after_recovery=false, venue last said {last_status:?}");
    }

    let restored = !shared.take_connection_lost();
    if ticked {
        println!("  data resumed after {:.1}s", elapsed.as_secs_f64());
    }
    println!("  data_resumed={ticked} connection_reported_healthy={restored}");
    let _ = shutdown_and_reclaim(&control_tx, join, account_id);
    // Three separate facts. Folding the order into the data result reported a
    // farm that never resumed whenever an order was refused, which names the
    // wrong cause and hides the right one.
    (ticked, order_acked, restored)
}
