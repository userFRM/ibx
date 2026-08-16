//! Take both transports away, repeatedly, while data is flowing.
//!
//! A client that recovers once has recovered once. The interesting failures are
//! the ones that need a second: state kept from the connection that went, a
//! subscription re-asked under an id the venue already holds, a host learned
//! and then forgotten. None of them show up in a single recovery.
//!
//! Each cycle takes both transports away — the auth one carries the orders and
//! the data one carries the quotes, and a maintenance window takes both — then
//! waits for quotes to resume on their own. A cycle that recovers but delivers
//! nothing is a failure: the connection coming back is not the point.
//!
//! ```text
//! IB_USERNAME=… IB_PASSWORD=… cargo run --release --features dev-tools --bin soak_reconnect -- --cycles 5
//! ```
//!
//! Reads only. Places no orders.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ibx::bridge::SharedState;
use ibx::engine::hot_loop::HotLoop;
use ibx::gateway::{CallerAuth, Gateway, GatewayConfig, Session};
use ibx::types::{ContractRef, ControlCommand};

/// Liquid enough that a quiet minute is the client's fault rather than the
/// market's, and more than one so a single silent contract cannot pass.
const SUBJECTS: [(i64, &str); 3] = [(756733, "SPY"), (265598, "AAPL"), (320227571, "QQQ")];

/// How long a recovery may take before it is a failure rather than a wait. The
/// ladder backs off, so this has to allow for a few rungs.
const RECOVERY_DEADLINE: Duration = Duration::from_secs(90);

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ibx=info".into()),
        )
        .init();

    let cycles: u32 = std::env::args()
        .skip_while(|a| a != "--cycles")
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(3);

    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    let password = std::env::var("IB_PASSWORD").unwrap_or_default();
    if username.is_empty() || password.is_empty() {
        eprintln!("IB_USERNAME and IB_PASSWORD are unset; this needs a session.");
        std::process::exit(2);
    }

    let config = GatewayConfig {
        settings: Default::default(),
        username: username.clone(),
        password: zeroize::Zeroizing::new(password.clone()),
        host: std::env::var("IB_HOST").unwrap_or_else(|_| ibx::config::CCP_HOSTS[0].to_string()),
        paper: true,
        accept_invalid_certs: false,
        ib_key_timeout_secs: ibx::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
        ib_key_token_sub_type: ibx::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
        code_provider: None,
        resume: None,
    };

    let Session { gateway: gw, market_data, trading, historical, .. } =
        match Gateway::connect(&config) {
            Ok(session) => session,
            Err(e) => {
                eprintln!("could not open a session: {e}");
                std::process::exit(1);
            }
        };

    // Nobody else on the account: a run that is being taken over measures the
    // takeover, and every cycle here looks like a recovery that failed.
    if let Some(other) = &gw.competing {
        eprintln!("another session already holds this account, from {} since {}",
                  other.ip, other.since);
        std::process::exit(2);
    }

    let shared = Arc::new(SharedState::new());
    let caller = CallerAuth {
        settings: Default::default(),
        host: config.host.clone(),
        username: config.username.clone(),
        password: zeroize::Zeroizing::new(config.password.to_string()),
        paper: config.paper,
        code_provider: config.code_provider.clone(),
        ib_key_timeout_secs: config.ib_key_timeout_secs,
        ib_key_token_sub_type: config.ib_key_token_sub_type.clone(),
    };
    let auth = gw.reconnect_auth(caller);

    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), None, gw.account_id.clone(), market_data, trading, historical, None,
    );
    hot_loop.set_reconnect_auth(auth);
    let engine = std::thread::spawn(move || {
        let mut hl = hot_loop;
        hl.run();
        hl
    });

    for (con_id, symbol) in SUBJECTS {
        control_tx.send(ControlCommand::Subscribe {
            contract: ContractRef {
                con_id,
                symbol: symbol.to_string(),
                ..Default::default()
            },
            mode_9887: 0,
            reply_tx: None,
        }).expect("the engine takes the subscription");
    }

    // Anything about a quote changing counts as data arriving.
    //
    // Watching the stamp alone was too narrow: it does not move on every tick,
    // and a thin session can leave it still for a minute while sizes and
    // prices are updating. A cycle read as a failed recovery on that basis is
    // the measure failing, not the client.
    let state_of_the_book = || -> Vec<(i64, i64, i64, i64, i64, u64)> {
        (0..SUBJECTS.len() as u32)
            .map(|id| {
                let q = shared.market.quote(id);
                (q.bid, q.ask, q.last, q.bid_size, q.ask_size, q.timestamp_ns)
            })
            .collect()
    };
    let ticks = state_of_the_book;
    std::thread::sleep(Duration::from_secs(10));
    let mut before = ticks();
    println!("settled; taking the transports away {cycles} times\n");

    let mut failures = Vec::new();
    for cycle in 1..=cycles {
        control_tx.send(ControlCommand::ForceDisconnect).ok();
        let dropped_at = Instant::now();

        let mut resumed = None;
        while dropped_at.elapsed() < RECOVERY_DEADLINE {
            std::thread::sleep(Duration::from_secs(2));
            let now = ticks();
            if now != before {
                resumed = Some((dropped_at.elapsed(), now));
                break;
            }
        }

        match resumed {
            Some((took, _stamp)) => {
                println!("[cycle {cycle}] data resumed after {:.1}s", took.as_secs_f64());
            }
            None => {
                println!("[cycle {cycle}] nothing arrived in {}s after the drop",
                         RECOVERY_DEADLINE.as_secs());
                failures.push(cycle);
            }
        }
        before = ticks();
        std::thread::sleep(Duration::from_secs(5));
    }

    control_tx.send(ControlCommand::Shutdown).ok();
    let _ = engine.join();

    if failures.is_empty() {
        println!("\nrecovered every time, {cycles} cycles");
    } else {
        println!("\nFAILED on cycle(s) {failures:?}: a recovery that delivers nothing \
                  is not a recovery");
        std::process::exit(1);
    }
}
