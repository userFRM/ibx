//! Compatibility tests against IB paper account.
//!
//! Requires IB_USERNAME and IB_PASSWORD environment variables.
//!
//! Run with: `cargo test --test ib_paper_compat -- --test-threads=1 --nocapture`
//!
//! One at a time: the account allows one session and each test opens its own.
//!
//! `compat_suite` is not `#[ignore]`d, so `--ignored` excludes it and runs only
//! the focused phases below, which are asked for by name:
//!
//! `cargo test --test ib_paper_compat <phase> -- --ignored --nocapture`
//!
//! Every phase builds a fresh HotLoop, runs it, then reclaims the connections
//! for the next, so the run holds one session throughout.
//!
//! Prices here are written scaled, grouped as dollars and cents:
//! `1_00_000_000` is $1.00 at `PRICE_SCALE`. The grouping is the unit, which
//! Is why the digits are not grouped in threes.
#![allow(clippy::inconsistent_digit_grouping)]

mod account;
mod common;
mod connection;
mod contracts;
mod coverage;
mod error_handling;
mod heartbeat;
mod historical;
mod market_data;
mod multi_asset;
mod orders;

use std::time::{Duration, Instant};

use ibx::gateway::Gateway;
use ibx::protocol::connection::Frame;
use ibx::protocol::fix;
use ibx::protocol::fixcomp;

use common::*;

/// How long the whole suite may take before it is treated as stuck.
///
/// A phase that waits on something the venue will never send waits forever,
/// and the run then reports nothing at all — no pass, no fail, no last phase.
/// That happened, and cost a session's worth of verification. This turns it
/// into a failure that names where it stopped.
const SUITE_BUDGET: Duration = Duration::from_secs(25 * 60);

fn watch_for_a_stuck_suite() {
    std::thread::spawn(|| {
        std::thread::sleep(SUITE_BUDGET);
        // The last phase header printed is the one it stopped in.
        eprintln!(
            "\n=== the suite passed {} minutes without finishing and is being stopped. \
             The last phase printed above is where it stopped. ===",
            SUITE_BUDGET.as_secs() / 60,
        );
        std::process::exit(2);
    });
}

#[test]
fn compat_suite() {
    let _ = tracing_subscriber::fmt::try_init();
    watch_for_a_stuck_suite();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    let (session, et_min) = market_session();
    // US stocks have ticks during regular hours AND extended hours (pre-market + after-hours)
    let needs_ticks = matches!(session, MarketSession::Regular | MarketSession::PreMarket | MarketSession::AfterHours);
    let needs_moc = needs_ticks && et_min < 945;
    println!("=== Compatibility Suite (session={session:?}) ===\n");
    let suite_start = Instant::now();

    let start = Instant::now();
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: mut ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config)
        .expect("Gateway::connect() failed");
    common::remember_recovery_auth(&gw, &config);
    let connect_time = start.elapsed();

    connection::phase_ccp_auth(&gw, hmds_conn.is_some(), connect_time);
    connection::phase_extra_farms(&gw, &config, &mut ccp_conn, &farm_conn.routing);

    let mut conns = Conns {
        farm: farm_conn,
        ccp: ccp_conn,
        hmds: hmds_conn,
        account_id: gw.account_id.clone(),
    };

    if needs_ticks {
        println!("--- RAW SUBSCRIBE TEST ---");
        let conn = &mut conns.farm;
        let result = conn.send_fixcomp(&[
            (fix::TAG_MSG_TYPE, "V"),
            (fix::TAG_SENDING_TIME, &ibx::protocol::datetime::chrono_free_timestamp()),
            (263, "1"),
            (146, "2"),
            (262, "1"),
            (6008, "756733"),
            (207, "BEST"),
            (167, "CS"),
            (264, "442"),
            (6088, "Socket"),
            (9830, "1"),
            (9839, "1"),
            (262, "2"),
            (6008, "756733"),
            (207, "BEST"),
            (167, "CS"),
            (264, "443"),
            (6088, "Socket"),
            (9830, "1"),
            (9839, "1"),
        ]);
        println!("  subscribe sent: {:?}, seq={}", result, conn.seq);

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut got_data = false;
        while Instant::now() < deadline {
            match conn.try_recv() {
                Ok(0) => {}
                Ok(n) => {
                    println!("  recv {} bytes, total buffered: {}", n, conn.buffered());
                    let frames = conn.extract_frames();
                    println!("  {} frames extracted", frames.len());
                    for frame in &frames {
                        let (raw, label) = match frame {
                            Frame::FixComp(r) => (r, "FIXCOMP"),
                            Frame::Binary(r) => (r, "Binary"),
                            Frame::Fix(r) => (r, "FIX"),
                            Frame::Control(r) => (r, "Control"),
                        };
                        let Some(unsigned) = conn.unsign(raw) else { continue };
                        // Printed as read. This probe validates nothing about a
                        // frame, so it labels nothing valid.
                        if label == "FIXCOMP" {
                            let inner = fixcomp::fixcomp_decompress(&unsigned).unwrap_or_default();
                            for m in &inner {
                                let preview = String::from_utf8_lossy(&m[..std::cmp::min(150, m.len())]);
                                println!("  {label} inner: {preview}");
                            }
                        } else {
                            let preview = String::from_utf8_lossy(&unsigned[..std::cmp::min(150, unsigned.len())]);
                            println!("  {label}: {preview}");
                        }
                        got_data = true;
                    }
                }
                Err(e) => {
                    println!("  recv error: {e}");
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !got_data {
            println!("  NO DATA received in 15s");
        }

        // Raw subscribe kills connections (server may close farm, hmds, and/or CCP).
        // Reconnect everything if CCP died, or just farm+hmds if CCP survived.
        conns = ensure_ccp_alive(conns, &mut gw, &config);
        // If CCP survived but farm or hmds did not, rebuild each on the route the
        // venue gave this session: its farm name, host and port. A farm reached
        // on another host closes rather than refusing.
        match historical::open_farm(ibx::gateway::Farm::MarketData) {
            Ok(c) => { conns.farm = c; println!("  farm reconnected"); }
            Err(e) => { println!("  farm reconnect failed (may already be fresh): {e}"); }
        }
        match historical::open_farm(ibx::gateway::Farm::Historical) {
            Ok(c) => { conns.hmds = Some(c); println!("  hmds reconnected"); }
            Err(e) => { println!("  hmds reconnect failed (may already be fresh): {e}"); }
        }
        println!();
    } else {
        println!("--- RAW SUBSCRIBE TEST ---\n  SKIP: {session:?} — no ticks expected\n");
    }

    conns = account::phase_account_pnl(conns);

    // ── Completed orders, PnL, news bulletins (early — before flaky network phases) ──
    conns = account::phase_completed_orders(conns);
    conns = account::phase_enriched_order_cache(conns);
    conns = account::phase_enriched_open_orders(conns);
    conns = account::phase_enriched_positions(conns);
    if needs_ticks {
        conns = account::phase_enriched_exec_details(conns);
    } else {
        println!("--- Phase 133: Enriched exec_details ---\n  SKIP: {session:?} — needs fills\n");
    }
    conns = account::phase_pnl_subscription(conns);
    conns = account::phase_pnl_subscribe_command(conns);
    conns = account::phase_news_bulletins(conns);

    conns = contracts::phase_contract_details(conns);
    conns = contracts::phase_contract_details_by_symbol(conns);
    conns = contracts::phase_trading_hours(conns);
    conns = contracts::phase_matching_symbols(conns);
    conns = historical::phase_historical_data(conns);
    conns = historical::phase_historical_daily_bars(conns);
    conns = historical::phase_cancel_historical(conns);
    conns = historical::phase_query_error_surfaces(conns);
    conns = historical::phase_head_timestamp(conns);
    conns = historical::phase_scanner_subscription(conns);
    conns = historical::phase_historical_news(conns);
    conns = historical::phase_fundamental_data(conns);
    conns = contracts::phase_market_rule_id(conns);

    if needs_ticks {
        conns = market_data::phase_market_data(conns);
        conns = market_data::phase_multi_instrument(conns);
        conns = account::phase_account_data(conns);
    } else {
        println!("--- Phase 2: Market Data Ticks (AAPL) ---\n  SKIP: {session:?} — no ticks expected\n");
        println!("--- Phase 3: Multi-Instrument Subscription (AAPL+MSFT+SPY) ---\n  SKIP: {session:?} — no ticks expected\n");
        println!("--- Phase 4: Account Data Reception ---\n  SKIP: {session:?} — needs ticks to trigger\n");
    }

    conns = orders::phase_outside_rth(conns);
    conns = orders::phase_outside_rth_stop(conns);
    conns = orders::phase_limit_order(conns);
    conns = orders::phase_stop_order(conns);
    conns = orders::phase_stop_limit_order(conns);
    conns = orders::phase_modify_order(conns);
    conns = orders::phase_modify_qty(conns);
    conns = orders::phase_trailing_stop(conns);
    conns = orders::phase_trailing_stop_limit(conns);
    conns = orders::phase_limit_ioc(conns);
    conns = orders::phase_limit_fok(conns);
    conns = orders::phase_stop_gtc(conns);
    conns = orders::phase_stop_limit_gtc(conns);
    conns = orders::phase_mit_order(conns);
    conns = orders::phase_lit_order(conns);
    conns = orders::phase_bracket_order(conns);
    conns = orders::phase_adaptive_order(conns);
    conns = orders::phase_rel_order(conns);
    conns = orders::phase_limit_opg(conns);
    conns = orders::phase_iceberg_order(conns);
    conns = orders::phase_hidden_order(conns);
    conns = orders::phase_short_sell(conns);
    conns = orders::phase_trailing_stop_pct(conns);
    conns = orders::phase_oca_group(conns);
    conns = orders::phase_mtl_order(conns);
    conns = orders::phase_mkt_prt_order(conns);
    conns = orders::phase_stp_prt_order(conns);
    conns = orders::phase_mid_price_order(conns);
    conns = orders::phase_snap_mkt_order(conns);
    conns = orders::phase_snap_mid_order(conns);
    conns = orders::phase_snap_pri_order(conns);
    conns = orders::phase_peg_mkt_order(conns);
    conns = orders::phase_peg_mid_order(conns);
    conns = orders::phase_discretionary_order(conns);
    conns = orders::phase_sweep_to_fill_order(conns);
    conns = orders::phase_all_or_none_order(conns);
    conns = orders::phase_trigger_method_order(conns);
    conns = orders::phase_price_condition_order(conns);
    conns = orders::phase_time_condition_order(conns);
    conns = orders::phase_volume_condition_order(conns);
    conns = orders::phase_multi_condition_order(conns);
    conns = orders::phase_vwap_order(conns);
    conns = orders::phase_twap_order(conns);
    conns = orders::phase_arrival_px_order(conns);
    conns = orders::phase_close_px_order(conns);
    conns = orders::phase_dark_ice_order(conns);
    conns = orders::phase_pct_vol_order(conns);
    conns = orders::phase_peg_bench_order(conns);
    conns = orders::phase_limit_auc_order(conns);
    conns = orders::phase_mtl_auc_order(conns);
    conns = orders::phase_box_top_order(conns);
    conns = orders::phase_what_if_order(conns);
    conns = orders::phase_cash_qty_order(conns);
    conns = orders::phase_fractional_order(conns);
    conns = orders::phase_adjustable_stop_order(conns);

    if needs_ticks && conns.hmds.is_some() {
        conns = market_data::phase_tbt_subscribe(conns);
    } else {
        println!("--- Phase 61: Tick-by-Tick Data (SPY) ---\n  SKIP: needs ticks+HMDS\n");
    }

    if needs_moc {
        conns = orders::phase_moc_order(conns);
        conns = orders::phase_loc_order(conns);
    } else {
        println!("--- Phase 27: MOC Order (SPY) ---\n  SKIP: {session:?} et_min={et_min} — only before 3:45 PM ET\n");
        println!("--- Phase 28: LOC Order (SPY) ---\n  SKIP: {session:?} et_min={et_min} — only before 3:45 PM ET\n");
    }

    conns = market_data::phase_subscribe_unsubscribe(conns);
    conns = market_data::phase_market_depth(conns);
    conns = market_data::phase_news_ticks(conns);
    conns = heartbeat::phase_heartbeat_keepalive(conns);
    conns = heartbeat::phase_farm_heartbeat_keepalive(conns);
    // Both phases above leave the trading connection idle longer than the venue
    // holds a quiet one open. Rebuilt rather than probed: the venue can answer a
    // liveness check and close immediately after.
    conns = rebuild_ccp(conns);
    // Those two sit idle for longer than the venue leaves a quiet trading
    // connection open, which is the point of them. What follows places orders
    // and needs that connection, so it is brought back before they run rather
    // than each of them failing for the reason the phase before it proved.
    conns = ensure_ccp_alive(conns, &mut gw, &config);

    if needs_ticks {
        conns = orders::phase_market_order(conns);
        conns = orders::phase_commission(conns);
        conns = orders::phase_bracket_fill_cascade(conns);
        conns = orders::phase_pnl_after_round_trip(conns);
    } else {
        println!("--- Phase 6: Market Order Round-Trip (SPY) ---\n  SKIP: {session:?} — needs ticks+fills\n");
        println!("--- Phase 17: Commission Tracking (GTC+OutsideRTH fill) ---\n  SKIP: {session:?} — needs fills\n");
        println!("--- Phase 51: Bracket Fill Cascade (SPY) ---\n  SKIP: {session:?} — needs fills\n");
        println!("--- Phase 52: PnL After Round Trip (SPY) ---\n  SKIP: {session:?} — needs fills\n");
    }

    // CCP may have died during the long-running fill phases. Reconnect the full
    // gateway session if needed before CCP-dependent phases.
    conns = ensure_ccp_alive(conns, &mut gw, &config);

    conns = contracts::phase_contract_details_channel(conns);
    conns = orders::phase_cancel_reject(conns);
    conns = historical::phase_historical_ticks(conns);
    conns = historical::phase_histogram_data(conns);
    conns = historical::phase_historical_schedule(conns);
    conns = historical::phase_realtime_bars(conns);
    conns = historical::phase_news_article(conns);
    conns = historical::phase_fundamental_data_channel(conns);
    conns = historical::phase_parallel_historical(conns);
    conns = historical::phase_scanner_params(conns);
    if needs_ticks {
        conns = account::phase_position_tracking(conns);
    } else {
        println!("--- Phase 97: Position Tracking (SPY) ---\n  SKIP: {session:?} — needs fills\n");
    }
    conns = connection::phase_connection_recovery(conns, &gw, &config);
    conns = ensure_ccp_alive(conns, &mut gw, &config);

    // ── New compatibility test phases (issues #83-) ──
    conns = multi_asset::phase_global_venues(conns);
    conns = multi_asset::phase_forex_order(conns);
    conns = multi_asset::phase_futures_order(conns);
    conns = multi_asset::phase_options_order(conns);
    conns = multi_asset::phase_concurrent_orders(conns);
    if needs_ticks {
        conns = market_data::phase_streaming_validation(conns);
    } else {
        println!("--- Phase 102: Streaming Data Validation (SPY) ---\n  SKIP: {session:?} — needs ticks\n");
    }
    conns = historical::phase_historical_ohlc_validation(conns);
    conns = error_handling::phase_ib_error_handling(conns);
    if needs_ticks {
        conns = connection::phase_reconnection_state_recovery(conns, &gw, &config);
    } else {
        println!("--- Phase 105: Reconnection State Recovery ---\n  SKIP: {session:?} — needs ticks\n");
    }
    conns = account::phase_account_summary(conns);

    // ── New test phases (issues #92-) ──
    if needs_ticks {
        conns = market_data::phase_tick_stress_test(conns);
    } else {
        println!("--- Phase 110: Tick Stress Test (SPY+AAPL+MSFT) ---\n  SKIP: {session:?} — needs ticks\n");
    }
    conns = historical::phase_large_historical_dataset(conns);
    conns = historical::phase_dst_boundary_historical(conns);
    conns = orders::phase_rapid_order_dedup(conns);
    conns = error_handling::phase_pacing_violation_recovery(conns);

    // ── Order modification edge cases (gap #4) ──
    conns = orders::phase_modify_price_and_qty(conns);
    conns = orders::phase_double_modify(conns);
    conns = orders::phase_cancel_during_modify(conns);

    // ── Authentication failure (gap #5) ──
    connection::phase_auth_wrong_password(&config);

    // ── P0: Global cancel (emergency kill switch) ──
    conns = orders::phase_global_cancel(conns);

    // ── P0: Cancel filled order (expect graceful handling) ──
    if needs_ticks {
        conns = orders::phase_cancel_filled_order(conns);
    } else {
        println!("--- Phase 124: Cancel Filled Order ---\n  SKIP: {session:?} — needs fills\n");
    }

    // ── P1: Matching symbols via ControlCommand channel ──
    conns = contracts::phase_matching_symbols_channel(conns);

    // ── P1: TBT unsubscribe lifecycle ──
    if needs_ticks && conns.hmds.is_some() {
        conns = market_data::phase_tbt_unsubscribe(conns);
    } else {
        println!("--- Phase 126: TBT Unsubscribe ---\n  SKIP: needs ticks+HMDS\n");
    }

    // ── P1: Cancel data requests (historical, fundamental, histogram, head timestamp) ──
    conns = historical::phase_cancel_data_requests(conns);

    // ── P2: TBT + regular quotes dual stream ──
    if needs_ticks && conns.hmds.is_some() {
        conns = market_data::phase_tbt_and_quotes_dual_stream(conns);
    } else {
        println!("--- Phase 128: TBT + Regular Quotes Dual Stream ---\n  SKIP: needs ticks+HMDS\n");
    }

    // ── P2: Concurrent subscribe stress (10 instruments) ──
    if needs_ticks {
        conns = market_data::phase_concurrent_subscribe_stress(conns);
    } else {
        println!("--- Phase 129: Concurrent Subscribe Stress ---\n  SKIP: {session:?} — needs ticks\n");
    }

    // ── P2: Historical data + live orders coexistence ──
    conns = historical::phase_historical_and_orders(conns);

    // ── P2: RegisterInstrument via ControlCommand channel ──
    conns = connection::phase_register_instrument_channel(conns);

    // ── P2: UpdateParam smoke test ──
    conns = connection::phase_update_param(conns);
    conns = coverage::phase_endpoint_coverage(conns);

    // ── Session-independent forex fallback phases ──
    // EUR.USD trades ~24h Sun-Fri, so these cover tick reception when US stocks are closed.
    if !needs_ticks {
        conns = market_data::phase_forex_market_data(conns);
        conns = market_data::phase_forex_streaming_validation(conns);
        conns = market_data::phase_forex_reconnection(conns);
    }

    // Runs last: it parks the real CCP behind a dead socket for 30s and is the one
    // phase asserting a hard liveness deadline, so a failure here cannot take the
    // phases behind it down with the suite.
    conns = heartbeat::phase_heartbeat_timeout_detection(conns);

    let _conns = connection::phase_graceful_shutdown(conns);

    // Session-dependent phases: 2,3,4,6,17,27,28,51,52,61,97,102,105,110,124,126,128,129 = 18
    // Forex fallback phases cover 3 of those when !needs_ticks (107,108,109)
    // Phase 134 (PnL subscribe/cancel command) is session-independent and always runs.
    let total_phases = 129;
    let skipped = if needs_ticks { 0 } else { 18 };
    let forex_fallback = if needs_ticks { 0 } else { 3 };
    let ran = total_phases - skipped + forex_fallback;
    println!("\n=== {}/{} phases ran ({} skipped, {} forex-fallback, {:?}) in {:.1}s ===",
        ran, total_phases, skipped, forex_fallback, session, suite_start.elapsed().as_secs_f64());

    // Last, and after the count is printed, so a reader sees both the count
    // this run claims and the reason it does not stand.
    common::no_phase_lost_the_session_unasked();
}

/// Live entry that runs only the QueryError phase, so you do not
/// pay the full ~128-phase suite cost just to validate this fix.
#[test]
fn query_error_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    println!("=== QueryError live test ===\n");
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config)
        .expect("Gateway::connect() failed");

    let conns = Conns {
        farm: farm_conn,
        ccp: ccp_conn,
        hmds: hmds_conn,
        account_id: gw.account_id.clone(),
    };

    let conns = historical::phase_query_error_surfaces(conns);
    let conns = ensure_ccp_alive(conns, &mut gw, &config);
    let _ = connection::phase_graceful_shutdown(conns);
}

/// The venue refuses a pegged-to-benchmark order naming a field the client does
/// not send. Runs that one phase, because a rejection reason is the whole
/// result and the full suite is a long way to read one line.
#[test]
fn peg_bench_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config)
        .expect("Gateway::connect() failed");
    let conns = Conns {
        farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone(),
    };

    let conns = orders::phase_peg_bench_order(conns);
    let conns = ensure_ccp_alive(conns, &mut gw, &config);
    let _ = connection::phase_graceful_shutdown(conns);
}

/// An order on a contract quoted in sterling rather than dollars. Runs on its
/// own because it needs London trading, which overlaps the New York session
/// only in the morning there.
/// Run: cargo test --test ib_paper_compat non_usd_order_phase_live -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn non_usd_order_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() { Some(c) => c, None => return };
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config).expect("connect");
    let mut conns = Conns { farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone() };
    conns = multi_asset::phase_non_usd_order(conns);
    conns = ensure_ccp_alive(conns, &mut gw, &config);
    let _ = connection::phase_graceful_shutdown(conns);
}

/// Forex and futures orders are both refused as unknown contracts. They ask
/// the same question and are cheaper asked together.
#[test]
fn non_stock_order_phases_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() { Some(c) => c, None => return };
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config).expect("connect");
    let mut conns = Conns { farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone() };
    conns = multi_asset::phase_forex_order(conns);
    conns = ensure_ccp_alive(conns, &mut gw, &config);
    conns = multi_asset::phase_futures_order(conns);
    conns = ensure_ccp_alive(conns, &mut gw, &config);
    let _ = connection::phase_graceful_shutdown(conns);
}

/// The options order, which the maturity-tag rule must not break.
#[test]
fn options_order_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() { Some(c) => c, None => return };
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config).expect("connect");
    let conns = Conns { farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone() };
    let conns = multi_asset::phase_options_order(conns);
    let conns = ensure_ccp_alive(conns, &mut gw, &config);
    let _ = connection::phase_graceful_shutdown(conns);
}

/// The nightly-maintenance case: the farm goes away and comes back by itself.
#[test]
fn farm_recovery_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() { Some(c) => c, None => return };
    let ibx::gateway::Session { gateway: gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config).expect("connect");
    let conns = Conns { farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone() };
    let (data_resumed, order_acked, healthy) =
        connection::phase_farm_recovers_with_credentials(gw, conns, &config);
    assert!(data_resumed, "the farm did not come back on its own and resume data");
    assert!(healthy, "data resumed but the connection still reports itself lost");
    assert!(order_acked, "the farm resumed data but an order placed after it was not accepted");
}

/// The multi-condition order, which a change to the condition encoder broke.
#[test]
fn multi_condition_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() { Some(c) => c, None => return };
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config).expect("connect");
    let conns = Conns { farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone() };
    let conns = orders::phase_multi_condition_order(conns);
    let conns = ensure_ccp_alive(conns, &mut gw, &config);
    let _ = connection::phase_graceful_shutdown(conns);
}

/// Instructions the encoder only just started sending.
#[test]
fn carried_instructions_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() { Some(c) => c, None => return };
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config).expect("connect");
    let conns = Conns { farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone() };
    let conns = orders::phase_carried_instructions_order(conns);
    let conns = ensure_ccp_alive(conns, &mut gw, &config);
    let _ = connection::phase_graceful_shutdown(conns);
}

/// The iceberg order, whose display size the venue keeps refusing.
#[test]
fn iceberg_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() { Some(c) => c, None => return };
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config).expect("connect");
    let conns = Conns { farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone() };
    let conns = orders::phase_iceberg_order(conns);
    let conns = ensure_ccp_alive(conns, &mut gw, &config);
    let _ = connection::phase_graceful_shutdown(conns);
}

/// The one condition type the venue refuses, on its own.
#[test]
fn time_condition_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() { Some(c) => c, None => return };
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config).expect("connect");
    let conns = Conns { farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone() };
    let conns = orders::phase_time_condition_order(conns);
    let conns = ensure_ccp_alive(conns, &mut gw, &config);
    let _ = connection::phase_graceful_shutdown(conns);
}

/// The six algo phases were all refused on the same field, so they answer as a
/// group and are cheaper to ask that way than through the whole suite.
#[test]
fn vwap_algo_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };
    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config)
        .expect("Gateway::connect() failed");
    let conns = Conns {
        farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone(),
    };
    let mut conns = conns;
    for phase in [
        orders::phase_vwap_order as fn(Conns) -> Conns,
        orders::phase_twap_order,
        orders::phase_arrival_px_order,
        orders::phase_close_px_order,
        orders::phase_dark_ice_order,
        orders::phase_pct_vol_order,
    ] {
        conns = phase(conns);
        conns = ensure_ccp_alive(conns, &mut gw, &config);
    }
    let _ = connection::phase_graceful_shutdown(conns);
}

/// The fill-or-cancel phase that fails intermittently, on its own and several
/// times over. An intermittent failure needs repetition more than it needs the
/// hundred-odd phases that happen to run before it.
#[test]
fn box_top_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    let ibx::gateway::Session { gateway: mut gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, .. } = Gateway::connect(&config)
        .expect("Gateway::connect() failed");
    let mut conns = Conns {
        farm: farm_conn, ccp: ccp_conn, hmds: hmds_conn,
        account_id: gw.account_id.clone(),
    };

    let rounds: usize = std::env::var("BOX_TOP_ROUNDS")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    for i in 0..rounds {
        println!("=== round {} of {rounds} ===", i + 1);
        conns = orders::phase_box_top_order(conns);
        conns = ensure_ccp_alive(conns, &mut gw, &config);
    }
    let _ = connection::phase_graceful_shutdown(conns);
}

/// Live entry: after a full disconnect,
/// a fresh `Gateway::connect` receives the CCP recovery push (35=8 with
/// 150=0/39=0, and a subsequent
/// `cancel_order(<prior_session_orderId>)` succeeds.
///
/// Two `Gateway::connect` calls in one process. Paper account skips the IBKey
/// gate so this runs unattended. IB throttles back-to-back logins within ~60s,
/// so the test waits 90s between sessions. Run:
///   cargo test --test ib_paper_compat cross_session_recovery_phase_live -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn cross_session_recovery_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    println!("=== cross-session recovery test ===\n");

    // ─── Session A: place resting GTC LMT BUY 1 SPY @ $1 (far below market) ───
    println!("Session A: connecting + placing resting LMT GTC BUY 1 SPY @ $1");
    let ibx::gateway::Session { gateway: gw_a, market_data: farm_a, trading: ccp_a, historical: hmds_a, .. } = Gateway::connect(&config)
        .expect("Session A: Gateway::connect failed");
    let account_id = gw_a.account_id.clone();
    drop(gw_a); // gateway state not needed after sockets are out

    let order_id = common::next_order_id();
    println!("  orderId = {order_id}");

    let session_a_acked = {
        let shared = std::sync::Arc::new(SharedState::new());
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
        let (mut hot_loop, control_tx) = HotLoop::with_connections(
            shared, Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
            farm_a, ccp_a, hmds_a, None,
        );
        let inst_id = hot_loop.context_mut().register_instrument(756733);
        hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
        // A US stock routed smart. Registered by id alone it states no
        // security type, and the venue answers an order carrying an empty
        // tag 167 with "Unsupported type".
        hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

        control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).expect("Session A: send order failed");

        let join = run_hot_loop(hot_loop);

        let deadline = Instant::now() + Duration::from_secs(30);
        let mut acked = false;
        let mut rejected = false;
        while Instant::now() < deadline && !acked && !rejected {
            if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(100)) {
                match u.status {
                    OrderStatus::Submitted | OrderStatus::PreSubmitted => acked = true,
                    OrderStatus::Rejected => rejected = true,
                    _ => {}
                }
            }
        }

        let _ = control_tx.send(ControlCommand::Shutdown);
        let _ = join.join();

        if rejected {
            panic!("Session A: order rejected — cleanup not possible automatically (check TWS for orderId={order_id})");
        }
        acked
    };
    // Conns A dropped → TCP sockets close → CCP session ends.
    assert!(session_a_acked, "Session A: LMT order never acked within 30s");
    println!("  Session A: orderId={order_id} acked + sockets closed");

    // IB throttles back-to-back Gateway::connect calls ("Never received data
    // start after auth" if too soon). Wait ~90s before attempting Session B.
    let wait_secs = 90u64;
    println!("  Waiting {wait_secs}s before Session B to clear IB auth throttle...\n");
    std::thread::sleep(Duration::from_secs(wait_secs));

    // ─── Session B: fresh connect → expect 35=8 recovery push → cancel orderId ───
    println!("Session B: fresh Gateway::connect → expect recovery push for orderId={order_id}");
    let ibx::gateway::Session { gateway: gw_b, market_data: farm_b, trading: ccp_b, historical: hmds_b, .. } = Gateway::connect(&config)
        .expect("Session B: Gateway::connect failed");
    assert_eq!(account_id, gw_b.account_id, "Account ID changed between sessions");
    drop(gw_b);

    let shared = std::sync::Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared, Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        farm_b, ccp_b, hmds_b, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    let join = run_hot_loop(hot_loop);

    // CCP delivers the recovery push <1s after session establishment.
    // Sleep a few seconds so handle_exec_report has time to populate last_clord.
    std::thread::sleep(Duration::from_secs(3));

    println!("  Session B: sending Cancel(orderId={order_id})");
    let cancel_sent = Instant::now();
    control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id }))
        .expect("Session B: send cancel failed");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut cancelled = false;
    let mut cancel_rejected = false;
    while Instant::now() < deadline && !cancelled && !cancel_rejected {
        if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            match u.status {
                OrderStatus::Cancelled => cancelled = true,
                OrderStatus::Rejected => cancel_rejected = true,
                _ => {}
            }
        }
    }
    let cancel_ms = cancel_sent.elapsed().as_millis();

    let _ = control_tx.send(ControlCommand::Shutdown);
    let _ = join.join();

    if cancel_rejected {
        panic!("Session B: cancel rejected — recovery push did not populate last_clord for orderId={order_id}");
    }
    assert!(cancelled,
        "Session B: cancel not confirmed within 30s — recovery push likely not parsed correctly. Cleanup orderId={order_id} via GUI.");

    println!("  Session B: cancel confirmed in {cancel_ms}ms");
    println!("\n  PASS — cross-session cancel works\n");
}

/// What this session answers. Counts every user message it sends of its own
/// accord, waits for the line to go quiet, then asks for things and counts
/// what comes back.
///
/// What it established: nothing this client asks for on a user message is
/// answered. A request carrying no parameters at all, sent three times with
/// the line drained between each, drew no reply, while an idle window of the
/// same length delivered the same messages anyway. What arrives is the
/// session's own cadence rather than an answer, so a single reply after a
/// single send says nothing. A chain request is likewise unanswered in every
/// shape tried. The difference is not in any message.
///
/// Run: cargo test --test ib_paper_compat routing_table_probe -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn routing_table_probe() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };
    let ibx::gateway::Session { gateway: gw, market_data: farm, trading: mut ccp, historical: hmds, .. } = Gateway::connect(&config).expect("Gateway::connect failed");
    let account_id = gw.account_id.clone();
    drop(gw);

    // Count every user message by its own subtype, so a reply can be told from
    // silence and from traffic that was going to arrive anyway.
    let tally = |ccp: &mut Connection, secs: u64| -> std::collections::BTreeMap<String, usize> {
        let mut seen: std::collections::BTreeMap<String, usize> = Default::default();
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            match ccp.try_recv() {
                Ok(0) => { std::thread::sleep(Duration::from_millis(50)); continue; }
                Err(_) => break,
                Ok(_) => {}
            }
            for frame in ccp.extract_frames() {
                let messages = match frame {
                    Frame::FixComp(raw) => {
                        let Some(unsigned) = ccp.unsign(&raw) else { continue };
                        fixcomp::fixcomp_decompress(&unsigned).unwrap_or_default()
                    }
                    Frame::Fix(raw) => vec![raw],
                    _ => continue,
                };
                for msg in messages {
                    let tags = fix::fix_parse(&msg);
                    // Count every message type, not only the one a reply was
                    // expected on. A refusal that arrives as something else is
                    // invisible to a probe that only looks for what it wanted.
                    let mt = tags.get(&fix::TAG_MSG_TYPE).cloned().unwrap_or_default();
                    let key = if mt == "U" {
                        format!("U/{}", tags.get(&6040).cloned().unwrap_or_default())
                    } else {
                        mt.clone()
                    };
                    *seen.entry(key).or_default() += 1;
                    if let Some(text) = tags.get(&58).filter(|t| !t.is_empty()) {
                        println!("  said: {mt} 58={text}");
                    }
                }
            }
        }
        seen
    };

    // Drain what the session sends of its own accord, and keep draining until
    // it has been quiet for a stretch. Counting against a line that is still
    // delivering its opening burst cannot tell a reply from a straggler.
    let mut settle: std::collections::BTreeMap<String, usize> = Default::default();
    for round in 0..12 {
        let batch = tally(&mut ccp, 5);
        let quiet = batch.is_empty();
        for (k, v) in batch {
            *settle.entry(k).or_default() += v;
        }
        if quiet {
            println!("  quiet after {} seconds", (round + 1) * 5);
            break;
        }
    }
    println!("  before the request: {settle:?}");

    // One message, no parameters, whose reply this session has already been
    // observed to receive. If nothing comes back, no user message this client
    // sends is being answered.
    // Ask the same parameterless question three times, draining between each.
    // One reply after one send cannot be told from the line's own cadence; a
    // reply after each of three sends can.
    for round in 1..=3 {
        let now = ibx::protocol::datetime::chrono_free_timestamp();
        ccp.send_fix(&[
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &now),
            (6040, "80"),
        ]).expect("send the algo catalogue request");
        let seen = tally(&mut ccp, 12);
        println!("  ask {round}: {seen:?}");
    }

    // And a stretch of the same length asking nothing at all, to measure what
    // the line delivers on its own.
    let idle = tally(&mut ccp, 12);
    println!("  asking nothing: {idle:?}");

    // An account subscribe, whose answer nothing pushes unasked. If this is
    // served, the session serves genuine queries and the chain is specifically
    // unavailable. If it is silent, only messages the session already pushes
    // ever come back.
    for round in 1..=3 {
        let now = ibx::protocol::datetime::chrono_free_timestamp();
        ccp.send_fix(&[
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &now),
            (6040, "61"),
            (1, account_id.as_str()),
        ]).expect("send the account subscribe");
        let seen = tally(&mut ccp, 12);
        println!("  account ask {round}: {seen:?}");
    }

    let variants: [(&str, Vec<(u32, &str)>); 6] = [
        ("spy 6346", vec![(55, "SPY"), (310, "OPT"), (6346, "756733"), (6320, "1"), (6994, "1")]),
        ("aapl 6346", vec![(55, "AAPL"), (310, "OPT"), (6346, "265598"), (6320, "1"), (6994, "1")]),
        ("aapl 6457", vec![(55, "AAPL"), (310, "OPT"), (6457, "265598"), (6320, "1"), (6994, "1")]),
        ("aapl no conid", vec![(55, "AAPL"), (310, "OPT"), (6320, "1"), (6994, "1")]),
        ("aapl +exch", vec![(55, "AAPL"), (310, "OPT"), (6346, "265598"), (6320, "1"), (6994, "1"), (6995, "SMART")]),
        ("aapl stk", vec![(55, "AAPL"), (310, "STK"), (6346, "265598"), (6320, "1"), (6994, "1")]),
    ];
    for (label, body) in variants {
        let now = ibx::protocol::datetime::chrono_free_timestamp();
        let mut fields: Vec<(u32, &str)> = vec![
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &now),
            (6040, "138"),
        ];
        fields.extend(body);
        ccp.send_fix(&fields).expect("send the chain request");
        let seen = tally(&mut ccp, 12);
        println!("  {label}: {seen:?}");
    }

    drop(farm);
    drop(hmds);
}

/// Live entry that validates `EClient::cancel_order_by_perm_id`.
/// Places a resting LMT GTC, captures the broker-assigned `permId` from
/// `order_status`-flavored OrderUpdate events, then cancels by permId (not by
/// local orderId) and asserts Cancelled.
/// Run: cargo test --test ib_paper_compat cancel_by_perm_id_phase_live -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn cancel_by_perm_id_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    println!("=== cancel_order_by_perm_id ===\n");
    let ibx::gateway::Session { gateway: gw, market_data: farm, trading: ccp, historical: hmds, .. } = Gateway::connect(&config)
        .expect("Gateway::connect failed");
    let account_id = gw.account_id.clone();
    drop(gw);

    let order_id = common::next_order_id();
    println!("  orderId = {order_id}");

    let shared = std::sync::Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        farm, ccp, hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).expect("send order failed");

    let join = run_hot_loop(hot_loop);

    // Wait for OrderUpdate(Submitted/PreSubmitted) to capture permId.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut perm_id: i64 = 0;
    let mut rejected = false;
    while Instant::now() < deadline && perm_id == 0 && !rejected {
        if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            match u.status {
                OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                    if u.perm_id != 0 {
                        perm_id = u.perm_id;
                    }
                }
                OrderStatus::Rejected => rejected = true,
                _ => {}
            }
        }
    }

    if rejected {
        let _ = control_tx.send(ControlCommand::Shutdown);
        let _ = join.join();
        panic!("order rejected — cleanup orderId={order_id} via GUI");
    }
    assert!(perm_id != 0,
        "permId never surfaced via OrderUpdate within 30s — cleanup orderId={order_id} via GUI");
    println!("  Captured permId={perm_id} for orderId={order_id}");

    // Look up the local orderId by permId (mirrors EClient::cancel_order_by_perm_id).
    // collect_open_orders merges shared.orders.order_cache (populated by 35=8 ack)
    // with ClientCore.open_orders.
    let core = ibx::client_core::ClientCore::new();
    let open = core.collect_open_orders(&shared);
    let found = open.iter().find(|(_, t)| t.order.perm_id == perm_id).map(|(oid, _)| *oid);
    let resolved_order_id = match found {
        Some(oid) => oid,
        None => {
            // Reports what the cache holds: a missing order and an order present
            // without its permId are different faults.
            let held = open.iter()
                .map(|(oid, t)| format!("{oid}:perm={} status={}", t.order.perm_id, t.status))
                .collect::<Vec<_>>()
                .join(", ");
            let direct = shared.orders.get_order_info(order_id)
                .map_or("absent from the cache entirely".to_string(),
                        |i| format!("cached with perm={} status={}", i.order.perm_id, i.order_state.status));
            let _ = control_tx.send(ControlCommand::Shutdown);
            let _ = join.join();
            panic!("permId {perm_id} not found among open orders — cleanup orderId={order_id} via GUI. \
                    The order itself is {direct}. Open orders collected: [{held}]");
        }
    };
    assert_eq!(resolved_order_id, order_id,
        "permId→orderId lookup resolved to {resolved_order_id} but expected {order_id}");
    println!("  Resolved permId={perm_id} → orderId={resolved_order_id}");

    println!("  Sending Cancel(orderId={resolved_order_id}) via permId-resolved path");
    let cancel_sent = Instant::now();
    control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: resolved_order_id }))
        .expect("send cancel failed");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut cancelled = false;
    let mut cancel_rejected = false;
    while Instant::now() < deadline && !cancelled && !cancel_rejected {
        if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            match u.status {
                OrderStatus::Cancelled => cancelled = true,
                OrderStatus::Rejected => cancel_rejected = true,
                _ => {}
            }
        }
    }
    let cancel_ms = cancel_sent.elapsed().as_millis();

    let _ = control_tx.send(ControlCommand::Shutdown);
    let _ = join.join();

    if cancel_rejected {
        panic!("cancel rejected — cleanup orderId={order_id} via GUI");
    }
    assert!(cancelled,
        "cancel not confirmed within 30s — cleanup orderId={order_id} via GUI");

    println!("  Cancel confirmed in {cancel_ms}ms");
    println!("\n  PASS — cancel_order_by_perm_id works\n");
}

/// Live entry that validates the `SubmitEx` wire path:
/// a bracket-style child (STP, GTC, parent_id + oca_group) placed against a
/// resting parent. Without them the attributes are dropped and the child
/// went live as an unlinked DAY stop.
///
/// Flow: parent LMT GTC BUY 1 SPY @ $1 (never fills) → child STP GTC SELL 1
/// @ $0.50 via `SubmitEx` linked with parent_id + oca_group → both must ack
/// (child must not be rejected) → cancel the parent → the child must cascade
/// to Cancelled without an explicit cancel, which proves the server saw the
/// parent link.
/// Run: cargo test --test ib_paper_compat submit_ex_bracket_child_phase_live -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn submit_ex_bracket_child_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    println!("=== SubmitEx bracket child ===\n");
    let ibx::gateway::Session { gateway: gw, market_data: farm, trading: ccp, historical: hmds, .. } = Gateway::connect(&config)
        .expect("Gateway::connect failed");
    let account_id = gw.account_id.clone();
    drop(gw);

    let parent_id = common::next_order_id();
    let child_id = common::next_order_id();
    assert_ne!(parent_id, child_id, "order id collision — parent/child tracking would be meaningless");
    println!("  parent orderId = {parent_id}, child orderId = {child_id}");

    let shared = std::sync::Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        farm, ccp, hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    // Parent: resting far-below-market entry (proven pattern from the
    // cross-session test).
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: parent_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).expect("send parent failed");

    // Child shape: STP + GTC + parent_id + oca_group. A sell
    // stop at $0.50 can never trigger even if something goes wrong.
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
        order_id: child_id, instrument: inst_id, side: Side::Sell, qty: ibx::types::QTY_SCALE,
        kind: ibx::types::OrderKind::Stop { stop_price: 50_000_000 },
        tif: b'1', // GTC
        attrs: ibx::types::OrderAttrs {
            parent_id,
            oca_group: parent_id,
            outside_rth: true,
            ..ibx::types::OrderAttrs::default()
        },
    })).expect("send child failed");

    let join = run_hot_loop(hot_loop);

    // Wait for both to ack.
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut parent_acked, mut child_acked) = (false, false);
    let mut rejected: Option<u64> = None;
    while Instant::now() < deadline && !(parent_acked && child_acked) && rejected.is_none() {
        if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            println!("  [update] oid={} status={:?} parentId={}", u.order_id, u.status, u.parent_id);
            match u.status {
                OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                    if u.order_id == parent_id { parent_acked = true; }
                    if u.order_id == child_id { child_acked = true; }
                }
                OrderStatus::Rejected => rejected = Some(u.order_id),
                _ => {}
            }
        }
    }

    if let Some(oid) = rejected {
        // Best-effort cleanup of whatever did rest before failing.
        let _ = control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: parent_id }));
        let _ = control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: child_id }));
        std::thread::sleep(Duration::from_secs(3));
        let _ = control_tx.send(ControlCommand::Shutdown);
        let _ = join.join();
        panic!("order {oid} rejected — SubmitEx encoding not accepted; check GUI for leftovers");
    }
    assert!(parent_acked, "parent never acked within 30s — cleanup {parent_id}/{child_id} via GUI");
    assert!(child_acked, "child never acked within 30s — the SubmitEx child likely vanished; cleanup {parent_id} via GUI");
    println!("  Both acked. Cancelling parent, expecting child to cascade...");

    // Cancel the parent only. A linked child must cascade to Cancelled.
    control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: parent_id }))
        .expect("send parent cancel failed");

    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut parent_cancelled, mut child_cancelled) = (false, false);
    while Instant::now() < deadline && !(parent_cancelled && child_cancelled) {
        if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            println!("  [update] oid={} status={:?}", u.order_id, u.status);
            if u.status == OrderStatus::Cancelled {
                if u.order_id == parent_id { parent_cancelled = true; }
                if u.order_id == child_id { child_cancelled = true; }
            }
        }
    }

    // Safety net: never leave the child resting, even on failure.
    if !child_cancelled {
        let _ = control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: child_id }));
        std::thread::sleep(Duration::from_secs(3));
    }
    let _ = control_tx.send(ControlCommand::Shutdown);
    let _ = join.join();

    assert!(parent_cancelled, "parent cancel not confirmed within 30s — check GUI");
    assert!(child_cancelled,
        "child did NOT cascade-cancel with its parent — parent link not honored on the wire");

    println!("\n  PASS — SubmitEx child linked, held, and cascade-cancelled\n");
}

/// Live entry that validates snap-to-tick on the outbound path.
/// Subscribes SPY market data (the subscribe ack carries the tick size the
/// engine stores), then places a resting LMT GTC BUY 1 SPY at the off-grid
/// price $1.001234. The engine must snap it to $1.00 before encoding; the
/// assertion reads the price back from the server-echoed open-order cache.
/// Run: cargo test --test ib_paper_compat snap_to_tick_phase_live -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn snap_to_tick_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    println!("=== snap-to-tick ===\n");
    let ibx::gateway::Session { gateway: gw, market_data: farm, trading: ccp, historical: hmds, .. } = Gateway::connect(&config)
        .expect("Gateway::connect failed");
    let account_id = gw.account_id.clone();
    drop(gw);

    let order_id = common::next_order_id();
    println!("  orderId = {order_id}");

    let shared = std::sync::Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        farm, ccp, hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    // Subscribe first: the subscribe ack is what populates the engine's
    // per-instrument tick size. Without it the snap is a no-op.
    control_tx.send(ControlCommand::Subscribe {
        contract: ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None,
    }).expect("send subscribe failed");

    let join = run_hot_loop(hot_loop);

    // Give the subscribe ack time to land so the tick size is known.
    std::thread::sleep(Duration::from_secs(5));

    // Off-grid on a $0.01 grid: must go out as $1.00.
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_123_400 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).expect("send order failed");

    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut acked, mut rejected) = (false, false);
    while Instant::now() < deadline && !acked && !rejected {
        if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            println!("  [update] oid={} status={:?}", u.order_id, u.status);
            match u.status {
                OrderStatus::Submitted | OrderStatus::PreSubmitted => acked = true,
                OrderStatus::Rejected => rejected = true,
                _ => {}
            }
        }
    }
    assert!(!rejected, "off-grid price was rejected — snap-to-tick did not apply");
    assert!(acked, "order never acked within 30s — cleanup orderId={order_id} via GUI");

    // Read the price back from the server-echoed open-order cache.
    std::thread::sleep(Duration::from_secs(2));
    let core = ibx::client_core::ClientCore::new();
    let echoed = core.collect_open_orders(&shared)
        .into_iter()
        .find(|(oid, _)| *oid == order_id)
        .map(|(_, t)| t.order.lmt_price);
    println!("  server-echoed lmt_price: {echoed:?}");

    // Cleanup before asserting on the echo.
    control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id }))
        .expect("send cancel failed");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut cancelled = false;
    while Instant::now() < deadline && !cancelled {
        if let Ok(Event::OrderUpdate(u)) = event_rx.recv_timeout(Duration::from_millis(100))
            && u.order_id == order_id && u.status == OrderStatus::Cancelled { cancelled = true; }
    }
    let _ = control_tx.send(ControlCommand::Shutdown);
    let _ = join.join();
    assert!(cancelled, "cancel not confirmed within 30s — cleanup orderId={order_id} via GUI");

    let echoed = echoed.expect("order missing from the open-order cache after ack");
    assert!((echoed - 1.00).abs() < 1e-9,
        "server echoed lmt_price {echoed} — expected the snapped 1.00");

    println!("\n  PASS — off-grid price snapped to the tick grid and accepted\n");
}

/// Live entry that validates that the deadline
/// sweeps do NOT fire on healthy traffic: a normal contract-details lookup
/// (by con_id and by symbol, including the fan-out) and a normal historical
/// request all still complete with rows/bars followed by their end signals,
/// well inside the sweep deadlines. The timeout paths themselves are
/// unit-tested; this guards the happy path against a premature sweep and
/// the changed end-ordering for by-symbol lookups.
/// Run: cargo test --test ib_paper_compat timeout_sweeps_phase_live -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn timeout_sweeps_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    println!("=== happy paths under the deadline sweeps ===\n");
    let ibx::gateway::Session { gateway: gw, market_data: farm, trading: ccp, historical: hmds, .. } = Gateway::connect(&config)
        .expect("Gateway::connect failed");
    let account_id = gw.account_id.clone();
    drop(gw);

    let shared = std::sync::Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        farm, ccp, hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");
    let join = run_hot_loop(hot_loop);

    // Helper: wait for rows + end on a req_id, in order.
    let wait_details = |req_id: u32, label: &str| -> (usize, bool, bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let (mut rows, mut end, mut row_after_end) = (0usize, false, false);
        // Reads on past the end callback: a row arriving after it is what this
        // reports, and stopping at the end makes that unobservable.
        let mut settle_by: Option<Instant> = None;
        while Instant::now() < deadline {
            if settle_by.is_some_and(|at| Instant::now() >= at) {
                break;
            }
            match event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::ContractDetails { req_id: r, .. }) if r == req_id => {
                    if end { row_after_end = true; }
                    rows += 1;
                }
                Ok(Event::ContractDetailsEnd(r)) if r == req_id => {
                    end = true;
                    settle_by = Some(Instant::now() + Duration::from_millis(500));
                }
                _ => {}
            }
        }
        println!("  {label}: rows={rows} end={end} row_after_end={row_after_end}");
        (rows, end, row_after_end)
    };

    // 1. By con_id — single record.
    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 756733, symbol: String::new(), sec_type: "STK".into(), exchange: String::new(), currency: String::new(), ..Default::default() }, req_id: 6001, filters: Default::default() }).expect("send details by con_id failed");
    let (rows, end, row_after_end) = wait_details(6001, "by-conId SPY");
    assert!(rows >= 1, "by-conId lookup returned no rows");
    assert!(end, "by-conId end never fired — sweep may have eaten the reply");
    assert!(!row_after_end, "a row arrived AFTER end — ordering regression");

    // 2. By symbol — exercises the fan-out counter and the deferred-end path.
    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 0, symbol: "AAPL".into(), sec_type: "STK".into(), exchange: String::new(), currency: "USD".into(), ..Default::default() }, req_id: 6002, filters: Default::default() }).expect("send details by symbol failed");
    let (rows, end, row_after_end) = wait_details(6002, "by-symbol AAPL fan-out");
    assert!(rows >= 1, "by-symbol lookup returned no rows");
    assert!(end, "by-symbol end never fired within 30s");
    assert!(!row_after_end, "a row arrived AFTER end — ordering regression");

    // 3. Historical bars — must complete without tripping the idle sweep.
    control_tx.send(ControlCommand::FetchHistorical { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "".to_string(), ..Default::default() }, req_id: 6003, end_date_time: String::new(), duration: "5 D".into(), bar_size: "1 day".into(), what_to_show: "TRADES".into(), use_rth: true, keep_up_to_date: false, include_expired: false, filters: Default::default() }).expect("send historical failed");
    let deadline = Instant::now() + Duration::from_secs(45);
    let (mut bars, mut complete, mut hist_err) = (0usize, false, None::<String>);
    while Instant::now() < deadline && !complete && hist_err.is_none() {
        if let Ok(Event::HistoricalData { req_id: 6003, data }) = event_rx.recv_timeout(Duration::from_millis(100)) {
            bars += data.bars.len();
            complete = data.is_complete;
        }
        for (rid, code, msg) in shared.reference.drain_historical_errors() {
            if rid == 6003 { hist_err = Some(format!("error {code}: {msg}")); }
        }
    }
    println!("  historical SPY 5D/1day: bars={bars} complete={complete} err={hist_err:?}");
    assert!(hist_err.is_none(), "healthy historical request errored: {hist_err:?}");
    assert!(complete && bars >= 3, "historical did not complete (bars={bars})");

    let _ = control_tx.send(ControlCommand::Shutdown);
    let _ = join.join();

    println!("\n  PASS — happy paths complete under the deadline sweeps\n");
}

/// Live entry, two parts.
/// Part 1: subscribe SPY, cancel, re-subscribe — the second
/// registration must REUSE the reclaimed slot id, proving unsubscribed
/// contracts no longer consume the instrument table.
/// Part 2: a symbol search with zero matches must still deliver
/// (an empty answer on the right req_id), and a following search must land
/// on ITS req_id — previously the empty result poisoned the queue and every
/// later reply was misattributed.
/// Run: cargo test --test ib_paper_compat reclaim_and_symbol_search_phase_live -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn reclaim_and_symbol_search_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    println!("=== slot reclaim + symbol search ===\n");
    let ibx::gateway::Session { gateway: gw, market_data: farm, trading: ccp, historical: hmds, .. } = Gateway::connect(&config)
        .expect("Gateway::connect failed");
    let account_id = gw.account_id.clone();
    drop(gw);

    let shared = std::sync::Arc::new(SharedState::new());
    let (event_tx, _event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        farm, ccp, hmds, None,
    );
    let join = run_hot_loop(hot_loop);

    let subscribe = |req: &str| {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        control_tx.send(ControlCommand::Subscribe {
            contract: ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0,
            reply_tx: Some(tx),
        }).expect("send subscribe failed");
        rx.recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("{req}: no registration reply"))
            .unwrap_or_else(|e| panic!("{req}: registration rejected: {e}"))
    };

    // ── Part 1: reclaim + reuse ──
    let id1 = subscribe("first subscribe");
    println!("  first subscribe: instrument id {id1}");
    std::thread::sleep(Duration::from_secs(2));
    control_tx.send(ControlCommand::Unsubscribe { instrument: id1 }).expect("send unsubscribe failed");
    std::thread::sleep(Duration::from_secs(2));
    let id2 = subscribe("re-subscribe");
    println!("  re-subscribe: instrument id {id2}");
    assert_eq!(id2, id1,
        "reclaimed slot must be reused — the cap would stay cumulative");

    // ── Part 2: symbol search ──
    control_tx.send(ControlCommand::FetchMatchingSymbols { req_id: 7001, pattern: "ZZZZQQXX".into() })
        .expect("send matching symbols failed");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut empty_delivered = false;
    while Instant::now() < deadline && !empty_delivered {
        for (rid, matches) in shared.reference.drain_matching_symbols() {
            println!("  symbol_samples: req_id={} matches={}", rid, matches.len());
            assert_eq!(rid, 7001, "reply misattributed");
            assert!(matches.is_empty(), "garbage pattern should have no matches");
            empty_delivered = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(empty_delivered,
        "empty symbol-search result was never delivered — queue poisoned");

    control_tx.send(ControlCommand::FetchMatchingSymbols { req_id: 7002, pattern: "AAPL".into() })
        .expect("send matching symbols failed");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut aapl_delivered = false;
    while Instant::now() < deadline && !aapl_delivered {
        for (rid, matches) in shared.reference.drain_matching_symbols() {
            println!("  symbol_samples: req_id={} matches={}", rid, matches.len());
            assert_eq!(rid, 7002, "reply landed on the wrong req_id");
            assert!(!matches.is_empty(), "AAPL search should match");
            aapl_delivered = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(aapl_delivered, "AAPL symbol search never delivered");

    let _ = control_tx.send(ControlCommand::Shutdown);
    let _ = join.join();

    println!("\n  PASS — slot reclaimed and reused; symbol search attributes correctly\n");
}

/// focused live entry — validates the on-demand RTT sample: send a
/// ping, expect a round-trip measurement to land in shared state.
/// Run: cargo test --test ib_paper_compat rtt_ping_phase_live -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn rtt_ping_phase_live() {
    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };

    println!("=== : RTT ping ===
");
    let ibx::gateway::Session { gateway: gw, market_data: farm, trading: ccp, historical: hmds, .. } = Gateway::connect(&config)
        .expect("Gateway::connect failed");
    let account_id = gw.account_id.clone();
    drop(gw);

    let shared = std::sync::Arc::new(SharedState::new());
    let (event_tx, _event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        farm, ccp, hmds, None,
    );
    let join = run_hot_loop(hot_loop);

    assert!(shared.last_ccp_rtt().is_none(), "no sample before any probe");
    control_tx.send(ControlCommand::Ping).expect("send ping failed");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut rtt = None;
    while Instant::now() < deadline && rtt.is_none() {
        rtt = shared.last_ccp_rtt();
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = control_tx.send(ControlCommand::Shutdown);
    let _ = join.join();

    let rtt = rtt.expect("RTT sample never arrived within 15s");
    println!("  measured RTT: {:.2} ms", rtt.as_secs_f64() * 1_000.0);
    assert!(rtt.as_millis() < 10_000, "implausible RTT: {rtt:?}");

    println!("
 PASS — on-demand RTT sample delivered
");
}

/// Whether the conditions on an order survive the trip back from the venue.
///
/// An order this session placed is held locally with its conditions, so reading
/// it back returns what was sent. An order placed by another session is rebuilt
/// from the venue's open-order report; the condition tags are written outbound
/// and not parsed inbound, so recovery depends on the venue restating them.
///
/// One session places a conditional order priced so it cannot fill and
/// conditioned so it cannot trigger, then exits. A second session asks what is
/// working. Needs no market: the order rests either way.
///
/// Run: cargo test --test ib_paper_compat conditions_round_trip_phase_live -- --ignored --nocapture
#[test]
#[ignore = "opens a session of its own, which the account allows one of, so it cannot run beside the suite; run it with --ignored"]
fn conditions_round_trip_phase_live() {
    use ibx::api::{EClientConfig, Wrapper};

    #[derive(Default)]
    struct Stated {
        orders: Vec<(i64, usize)>,
        finished: Vec<i64>,
    }
    impl Wrapper for Stated {
        fn open_order(
            &mut self, order_id: i64, _c: &ApiContract, order: &ApiOrder,
            _s: &ibx::api::types::OrderState,
        ) {
            self.orders.push((order_id, order.conditions.len()));
        }
        fn order_status(
            &mut self, order_id: i64, status: &str, _f: f64, _r: f64, _a: f64,
            _p: i64, _pi: i64, _l: f64, _c: i64, _w: &str, _m: f64,
        ) {
            if matches!(status, "Cancelled" | "Filled" | "Inactive") {
                self.finished.push(order_id);
            }
        }
    }

    let _ = tracing_subscriber::fmt::try_init();
    let config = match get_config() {
        Some(c) => c,
        None => { println!("Skipping: IB credentials not set"); return; }
    };
    let settings = || EClientConfig {
        username: config.username.clone(),
        password: config.password.to_string(),
        paper: config.paper,
        ..Default::default()
    };

    println!("=== conditions, read back on a session that did not place them ===");

    let placed_id = {
        let client = EClient::connect(&settings()).expect("connect failed");
        let spy = client.qualify_contract(&ApiContract {
            symbol: "SPY".into(), sec_type: "STK".into(), exchange: "SMART".into(),
            currency: "USD".into(), ..Default::default()
        }).expect("qualify failed");

        let mut order = ApiOrder {
            action: "BUY".into(), total_quantity: 1.0, order_type: "LMT".into(),
            lmt_price: 1.00, tif: "GTC".into(), ..Default::default()
        };
        // Cannot trigger: the contract below one cent. Cannot fill: a dollar.
        order.conditions = vec![OrderCondition::Price {
            // The exchange from the qualification reply, not the request: a
            // condition names the contract as the venue describes it.
            con_id: spy.con_id, exchange: spy.exchange.clone(),
            price: PRICE_SCALE / 100, is_more: false, trigger_method: 0,
        }];

        let id = next_order_id() as i64;
        client.place_order(id, &spy, &order).expect("place failed");
        std::thread::sleep(Duration::from_secs(4));
        println!("  placed {id} carrying {} condition(s)", order.conditions.len());
        client.disconnect();
        id
    };
    std::thread::sleep(Duration::from_secs(5));

    let client = EClient::connect(&settings()).expect("reconnect failed");
    let mut stated = Stated::default();
    client.req_all_open_orders(&mut stated);

    let stated_with = stated.orders.iter()
        .find(|(id, _)| *id == placed_id)
        .map(|(_, carried)| *carried);

    // Withdrawn before any assertion, and the withdrawal confirmed. This order is
    // GTC: an unconfirmed cancel, or an assertion returning first, leaves it
    // resting on the account.
    client.cancel_order(placed_id, "").expect("the resting order is withdrawn");
    let withdrawn_by = std::time::Instant::now() + Duration::from_secs(15);
    let cancelled = loop {
        // Each snapshot is read independently: a retained result would answer for
        // every snapshot after the first.
        stated.orders.clear();
        client.req_all_open_orders(&mut stated);
        if stated.finished.contains(&placed_id)
            || !stated.orders.iter().any(|(id, _)| *id == placed_id)
        {
            break true;
        }
        if std::time::Instant::now() >= withdrawn_by {
            break false;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    client.disconnect();
    assert!(cancelled, "order {placed_id} is still resting, and it is good-till-cancelled");

    match stated_with {
        Some(carried) => {
            println!("  the venue states it with {carried} condition(s)");
            assert_eq!(
                carried, 1,
                "the venue states the condition this order waits on, so a session \
                 reading it back has it: an order that came back stating none and \
                 was placed again went live at once where the first one waited",
            );
        }
        // Not a pass. A session that cannot see its own resting order has
        // nothing to say about what the order carries.
        None => panic!("the venue did not state the resting order at all"),
    }
}
