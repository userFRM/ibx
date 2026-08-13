//! What a farm says while it is being logged on to, and what it does not say.
//!
//! Some farms answer a logon in under two seconds. Others accept the
//! connection, and about ten seconds later close it without the client having
//! read a single message — which the logon reports as the connection having
//! been closed, and which is indistinguishable from two different causes:
//!
//! - the server said nothing, because the account is not served there;
//! - the server said something this client does not answer, and gave up.
//!
//! The two are told apart by whether anything arrives, so this connects one
//! farm at a time with every frame logged, including the ones the logon has no
//! branch for.
//!
//! ```text
//! IB_USERNAME=… IB_PASSWORD=… cargo run --bin capture_farm_logon -- eufarm usopt
//! ```
//!
//! Farms default to the set the compatibility suite tries. Reads only: it logs
//! on, reports, and disconnects.

use std::time::Instant;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ibx=debug".into()),
        )
        .init();

    let asked: Vec<String> = std::env::args().skip(1).collect();

    let username = std::env::var("IB_USERNAME").unwrap_or_default();
    let password = std::env::var("IB_PASSWORD").unwrap_or_default();
    if username.is_empty() || password.is_empty() {
        eprintln!("IB_USERNAME and IB_PASSWORD are unset; this needs a session.");
        std::process::exit(2);
    }

    let config = ibx::gateway::GatewayConfig {
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
    };

    // One session, held open for every farm below: each farm logon is
    // authenticated against it, and a second session would compete with the
    // first rather than add anything.
    let session = match ibx::gateway::Gateway::connect(&config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not open a session: {e}");
            std::process::exit(1);
        }
    };
    let gw = session.gateway;

    // The host the session moved to, not the one it was opened against. The
    // venue names which server this account belongs on and the session follows
    // it; a farm asked for on the host that was knocked on first is asked of a
    // server this account is not on, which answers by saying nothing.
    let host = if gw.hmds_host.is_empty() { config.host.clone() } else { gw.hmds_host.clone() };
    println!("session is on {host}\n");

    // The market-data connection the session was routed to answered with the
    // table of every market the venue serves, and where. A farm named in it is
    // reached where it says; a farm not named in it can only be guessed at.
    let routed = session.market_data.routing.clone();
    if !routed.is_empty() {
        println!("the venue named {} farms:", routed.farms().len());
        let mut named: Vec<_> = routed.farms().into_iter().collect();
        named.sort();
        for (farm, (h, p)) in &named {
            println!("  {farm:16} {h}:{p}");
        }
        println!();
    }

    // Every farm the venue named, unless the caller named some itself. A list
    // written here would be a guess at something the session already states.
    let farms: Vec<String> = if asked.is_empty() {
        let mut named: Vec<String> = routed.farms().keys().map(|f| f.to_string()).collect();
        named.sort();
        named
    } else {
        asked
    };

    for farm in &farms {
        let started = Instant::now();
        let kind = if farm.contains("hmds") {
            ibx::gateway::Farm::Historical
        } else {
            ibx::gateway::Farm::MarketData
        };
        // Where the venue says this farm is. A farm it did not name can only
        // be asked for beside the session, on the port that session uses.
        let (farm_host, farm_port) = routed
            .host_of(farm)
            .map(|(h, p)| (h.to_string(), p))
            .unwrap_or_else(|| (host.clone(), config.settings.port));
        let where_it_is = ibx::api::settings::SessionSettings {
            port: farm_port,
            ..Default::default()
        };
        print!("{farm} ({farm_host}:{farm_port}): ");
        match ibx::gateway::connect_farm(
            &where_it_is,
            &farm_host,
            farm,
            &config.username,
            &config.password,
            config.paper,
            &gw.server_session_id,
            &gw.session_token,
            &gw.hw_info,
            &gw.encoded,
            kind,
        ) {
            Ok(_) => println!("answered in {:.3}s", started.elapsed().as_secs_f64()),
            Err(e) => println!("{e} after {:.3}s", started.elapsed().as_secs_f64()),
        }
    }

    println!(
        "\nA farm that logged nothing above said nothing: the account is not served there.\n\
         A farm that logged an unhandled message was answered and this client did not reply."
    );
}
