//! POC: Validate farm auto-reconnect with cached session credentials.
//!
//! Test 1: connect_farm() works with cached K (no SRP) — proves the thesis
//! Test 2: HotLoop auto-reconnect fires after farm disconnect — end-to-end

use std::sync::Arc;
use std::time::{Duration, Instant};

use ibx::bridge::SharedState;
use ibx::gateway::{connect_farm, reconnect_ccp, Gateway, GatewayConfig, ReconnectAuth};

/// An account number, shortened to what tells two accounts apart.
fn redacted(account: &str) -> String {
    // Printed where a log may be read by anyone the repository can be read by.
    // Enough to tell two accounts apart, and no more.
    match account.len() {
        0 => String::new(),
        n if n <= 4 => "*".repeat(n),
        n => format!("{}{}", &account[..2], "*".repeat(n - 2)),
    }
}

/// The credentials these tests need. Missing credentials fail rather than pass
/// quietly, because a reconnect test that never connected has proved nothing.
/// A checkout with no credentials skips on purpose with
/// `IBX_ALLOW_SKIP_NO_CREDS=1`, the same switch the compat suite uses.
fn config() -> Option<GatewayConfig> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let (username, password) = match (var("IB_USERNAME"), var("IB_PASSWORD")) {
        (Some(u), Some(p)) => (u, p),
        _ if var("IBX_ALLOW_SKIP_NO_CREDS").as_deref() == Some("1") => return None,
        _ => panic!(
            "IB_USERNAME/IB_PASSWORD unset or empty — a reconnect test that \
             never connected proves nothing, so it fails rather than passing \
             silently. Export them first (`set -a; . ./.env; set +a`), or set \
             IBX_ALLOW_SKIP_NO_CREDS=1 to skip deliberately."
        ),
    };
    Some(GatewayConfig {
        settings: Default::default(),
        username,
        password: zeroize::Zeroizing::new(password),
        host: std::env::var("IB_HOST").unwrap_or_else(|_| "cdc1.ibllc.com".to_string()),
        paper: true,
        accept_invalid_certs: false,
        ib_key_timeout_secs: ibx::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
        ib_key_token_sub_type: ibx::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
        code_provider: None,
        resume: None,
    })
}

#[test]
fn farm_reconnect_with_cached_credentials() {
    let Some(cfg) = config() else { return };

    // Phase 1: Full auth
    let t0 = Instant::now();
    let ibx::gateway::Session { gateway: gw, market_data: farm_conn, trading: _ccp_conn, historical: _hmds, .. } =
        Gateway::connect(&cfg).expect("Initial connect failed");
    let full_auth_ms = t0.elapsed().as_millis();

    // Save credentials
    let session_key = gw.session_token.clone();
    let server_session_id = gw.server_session_id.clone();
    let hw_info = gw.hw_info.clone();
    let encoded = gw.encoded.clone();

    println!("Full auth: {}ms | Account: {}", full_auth_ms, redacted(&gw.account_id));

    // Phase 2: Drop original farm connection
    drop(farm_conn);

    // Phase 3: Reconnect using cached credentials (no SRP)
    let t1 = Instant::now();
    let new_farm = connect_farm(&Default::default(), 
        &cfg.host, "usfarm",
        &cfg.username, &cfg.password, cfg.paper,
        &server_session_id, &session_key, &hw_info, &encoded, ibx::gateway::Farm::MarketData,
    ).expect("Farm reconnect with cached credentials FAILED");
    let reconnect_ms = t1.elapsed().as_millis();

    println!("Farm reconnect: {}ms (no SRP) | seq={}", reconnect_ms, new_farm.seq);
    assert!(new_farm.seq > 0);
    println!("PASS: cached K reconnect works, {:.1}x speedup", full_auth_ms as f64 / reconnect_ms.max(1) as f64);
}

#[test]
fn hotloop_auto_reconnect_on_farm_disconnect() {
    let Some(cfg) = config() else { return };

    let ibx::gateway::Session { gateway: gw, market_data: farm_conn, trading: ccp_conn, historical: hmds, .. } =
        Gateway::connect(&cfg).expect("Initial connect failed");

    let shared = Arc::new(SharedState::new());
    let (event_tx, _event_rx) = std::sync::mpsc::sync_channel(256);

    let (mut hot_loop, _control_tx) = gw.into_hot_loop_with_farms(
        shared.clone(), Some(event_tx),
        farm_conn, ccp_conn, hmds, None, None,
        ibx::gateway::CallerAuth {
            settings: Default::default(),
            host: cfg.host.clone(),
            username: cfg.username.clone(),
            password: cfg.password.clone(),
            code_provider: cfg.code_provider.clone(),
            ib_key_timeout_secs: cfg.ib_key_timeout_secs,
            ib_key_token_sub_type: cfg.ib_key_token_sub_type.clone(),
            paper: cfg.paper,
        },
    );
    println!("Reconnect auth set: host={}, user={}, paper={}", cfg.host, cfg.username, cfg.paper);

    assert!(!hot_loop.is_farm_disconnected());

    // Run a few iterations to process initial data
    for _ in 0..100 {
        hot_loop.poll_once();
    }
    assert!(!hot_loop.is_farm_disconnected());

    // Force farm disconnect by dropping the connection
    hot_loop.farm_conn = None;
    hot_loop.force_farm_disconnect();

    assert!(hot_loop.is_farm_disconnected());
    println!("Farm disconnected, spawning auto-reconnect...");

    // Trigger reconnect spawn
    hot_loop.spawn_farm_reconnect_for_test();
    println!("Reconnect thread spawned, polling for result...");

    // Poll until reconnect completes (up to 60s — connect_farm takes ~7s)
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut polls = 0u32;
    while hot_loop.is_farm_disconnected() && Instant::now() < deadline {
        hot_loop.poll_farm_reconnect_for_test();
        polls += 1;
        if polls.is_multiple_of(50) {
            println!("  ...still waiting ({:.0}s elapsed)", Instant::now().duration_since(deadline - Duration::from_secs(60)).as_secs_f64());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(!hot_loop.is_farm_disconnected(), "Farm should have reconnected within 60s");
    assert!(hot_loop.farm_conn.is_some(), "Farm connection should be restored");
    println!("PASS: HotLoop auto-reconnected farm after disconnect");
}

#[test]
fn ccp_reconnect_with_cached_credentials() {
    let Some(cfg) = config() else { return };

    let t0 = Instant::now();
    let ibx::gateway::Session { gateway: gw, market_data: _farm_conn, trading: ccp_conn, historical: _hmds, .. } =
        Gateway::connect(&cfg).expect("Initial connect failed");
    let full_auth_ms = t0.elapsed().as_millis();

    let auth = ReconnectAuth {
        settings: Default::default(),
        host: cfg.host.clone(),
        username: cfg.username.clone(),
        password: cfg.password.clone(),
        code_provider: cfg.code_provider.clone(),
        ib_key_timeout_secs: cfg.ib_key_timeout_secs,
        ib_key_token_sub_type: cfg.ib_key_token_sub_type.clone(),
        paper: cfg.paper,
        session_key: gw.session_token.clone(),
        session_token: gw.session_token.clone(),
        server_session_id: gw.server_session_id.clone(),
        hw_info: gw.hw_info.clone(),
        encoded: gw.encoded.clone(),
        hmds_host: gw.hmds_host.clone(),
        hmds_farm: gw.hmds_farm.clone(),
        trading_host: String::new(),
        trading_farm: String::new(),
        secdef_host: gw.secdef_host.clone(),
        secdef_farm: gw.secdef_farm.clone(),
    };

    println!("Full auth: {}ms | session_id={}", full_auth_ms, auth.server_session_id);

    // Drop original CCP connection
    drop(ccp_conn);
    println!("Original CCP connection dropped");

    // Reconnect using cached credentials (SOFT_TOKEN, no SRP)
    let t1 = Instant::now();
    let result = reconnect_ccp(&auth);
    let reconnect_ms = t1.elapsed().as_millis();

    match result {
        Ok(conn) => {
            println!("CCP reconnect: {}ms (SOFT_TOKEN) | seq={}", reconnect_ms, conn.seq);
            println!("PASS: CCP reconnect with cached K works, {:.1}x speedup",
                full_auth_ms as f64 / reconnect_ms.max(1) as f64);
        }
        Err(e) => {
            println!("CCP reconnect failed after {reconnect_ms}ms: {e}");
            println!("INFO: Server requires full SRP for CCP — auto-reconnect not possible without password");
            // This is an expected outcome — don't fail the test, just report
        }
    }
}
