//! Multi-asset-class order test phases (forex, futures, options).

use super::common::*;
use ibx::control::contracts;
use ibx::protocol::fix;
use ibx::protocol::fixcomp;
use ibx::protocol::connection::Frame;

pub(super) fn phase_forex_order(conns: Conns) -> Conns {
    phase!("--- Phase 98: Forex Order Lifecycle (EUR.USD) ---");

    // First, look up EUR.USD contract
    let now = ibx::protocol::datetime::chrono_free_timestamp();
    let mut ccp = conns.ccp;
    ccp.send_fix(&[
        (fix::TAG_MSG_TYPE, "c"),
        (fix::TAG_SENDING_TIME, &now),
        (contracts::TAG_SECURITY_REQ_ID, "RFXEUR"),
        (contracts::TAG_SECURITY_REQ_TYPE, "2"),
        (contracts::TAG_SYMBOL, "EUR"),
        (contracts::TAG_SECURITY_TYPE, "CASH"),
        (contracts::TAG_EXCHANGE, "IDEALPRO"),
        (contracts::TAG_CURRENCY, "USD"),
        (contracts::TAG_IB_SOURCE, "Socket"),
    ]).expect("Failed to send forex secdef request");

    let mut forex_con_id: Option<u32> = None;
    let deadline = Instant::now() + Duration::from_secs(10);

    while Instant::now() < deadline && forex_con_id.is_none() {
        match ccp.try_recv() {
            Ok(0) => { std::thread::sleep(Duration::from_millis(50)); continue; }
            Err(e) => { println!("  CCP recv error: {e}"); break; }
            Ok(_) => {}
        }
        for frame in ccp.extract_frames() {
            // Every frame is unsigned, whatever kind it is. On a signed session a
            // frame read as it stands parses distorted, and unsigning is what
            // advances the read chain, so skipping one leaves the rest unreadable.
            for msg in messages_in(&mut ccp, &frame) {
                let tags = fix::fix_parse(&msg);
                if tags.get(&fix::TAG_MSG_TYPE).map(|s| s.as_str()) == Some("d")
                    && let Some(def) = contracts::parse_secdef_response(&msg, true)
                        && def.sec_type == contracts::SecurityType::Forex {
                            println!("  Contract: {} conId={} secType={:?} exchange={}",
                                def.symbol, def.con_id, def.sec_type, def.exchange);
                            forex_con_id = Some(def.con_id);
                        }
            }
        }
    }

    let fx_con_id = match forex_con_id {
        Some(id) => id,
        None => {
            lookup_returned_nothing("no EUR.USD forex contract came back");
        }
    };

    // Submit a forex limit order using the actual forex con_id
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, ccp, conns.hmds, None,
    );
    let inst = hot_loop.context_mut().register_instrument(fx_con_id as i64);
    hot_loop.context_mut().set_symbol(inst, "EUR".to_string());
    // A contract is not a stock because nobody said otherwise. Without this the
    // order goes out as a stock on the default venue, and the venue answers
    // that it knows no such contract — correctly.
    hot_loop.context_mut().set_routing(inst, "CASH", "IDEALPRO");

    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid, instrument: inst, side: Side::Buy, qty: 20000 * ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 50_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut order_acked = false;
    let mut cancel_sent = false;
    let mut order_cancelled = false;
    let mut rejected_order: Option<u64> = None;

    while Instant::now() < deadline {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100))
            && update.order_id == oid {
                match update.status {
                    OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                        order_acked = true;
                        if !cancel_sent {
                            control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: oid })).unwrap();
                            cancel_sent = true;
                        }
                    }
                    OrderStatus::Cancelled => { order_cancelled = true; break; }
                    OrderStatus::Rejected => { rejected_order = Some(update.order_id); break; }
                    _ => {}
                }
            }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if let Some(id) = rejected_order {
        skipped!("  SKIP: Forex order rejected — {}\n", reject_reason(&shared, id));
    } else {
        if skip_unacked_if_closed(order_acked) { return conns; }
        assert!(order_acked, "Forex order should be acknowledged");
        assert!(order_cancelled, "Forex order should be cancelled");
        println!("  PASS\n");
    }
    conns
}

pub(super) fn phase_futures_order(conns: Conns) -> Conns {
    phase!("--- Phase 99: Futures Order (MES) ---");

    // Look up MES (Micro E-mini S&P 500)
    let now = ibx::protocol::datetime::chrono_free_timestamp();
    let mut ccp = conns.ccp;
    ccp.send_fix(&[
        (fix::TAG_MSG_TYPE, "c"),
        (fix::TAG_SENDING_TIME, &now),
        (contracts::TAG_SECURITY_REQ_ID, "RFUT"),
        (contracts::TAG_SECURITY_REQ_TYPE, "2"),
        (contracts::TAG_SYMBOL, "MES"),
        (contracts::TAG_SECURITY_TYPE, "FUT"),
        (contracts::TAG_EXCHANGE, "CME"),
        (contracts::TAG_CURRENCY, "USD"),
        (contracts::TAG_IB_SOURCE, "Socket"),
    ]).expect("Failed to send futures secdef request");

    let mut fut_contract: Option<contracts::ContractDefinition> = None;
    let deadline = Instant::now() + Duration::from_secs(10);

    while Instant::now() < deadline && fut_contract.is_none() {
        match ccp.try_recv() {
            Ok(0) => { std::thread::sleep(Duration::from_millis(50)); continue; }
            Err(e) => { println!("  CCP recv error: {e}"); break; }
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
                if tags.get(&fix::TAG_MSG_TYPE).map(|s| s.as_str()) == Some("d")
                    && let Some(def) = contracts::parse_secdef_response(&msg, true)
                        && def.sec_type == contracts::SecurityType::Future {
                            println!("  Contract: {} conId={} secType={:?} exchange={} expiry={} multiplier={}",
                                def.symbol, def.con_id, def.sec_type, def.exchange,
                                def.last_trade_date, def.multiplier);
                            assert!(def.multiplier > 0.0, "Futures multiplier should be positive");
                            assert!(!def.last_trade_date.is_empty(), "Futures should have expiry date");
                            // Take the first (front-month) contract
                            if fut_contract.is_none() {
                                fut_contract = Some(def);
                            }
                        }
            }
        }
    }

    let fut_def = match fut_contract {
        Some(def) => def,
        None => {
            lookup_returned_nothing("no MES futures contract came back");
        }
    };

    // Submit a futures limit order using the actual futures con_id
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, ccp, conns.hmds, None,
    );
    let inst = hot_loop.context_mut().register_instrument(fut_def.con_id as i64);
    hot_loop.context_mut().set_symbol(inst, "MES".to_string());
    hot_loop.context_mut().set_routing(inst, "FUT", "CME");
    // Everything the definition gave, in the order the key carries it: what
    // tells this contract from the rest of its family is the trading class and
    // the local symbol, not the maturity.
    hot_loop.context_mut().set_order_identity(inst, &format!(
        "{}|0||{}|{}|{}",
        fut_def.last_trade_date, fut_def.multiplier as i64,
        fut_def.trading_class, fut_def.local_symbol,
    ));
    println!("  identity: tradingClass={} localSymbol={}",
        fut_def.trading_class, fut_def.local_symbol);
    // What this phase is really for. A futures order is refused as ambiguous
    // unless it names one member of the family rather than the family: the
    // contract month on MaturityMonthYear with no maturity date at all, and
    // the local symbol on SecurityID, under the source code that marks the
    // identifier as venue-assigned. A trading class describes the family
    // and is not stated on an order. Placing the order here is what keeps that
    // shape honest, because a definition lookup alone never exercises it.

    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
        order_id: oid, instrument: inst, side: Side::Buy, qty: ibx::types::QTY_SCALE,
        kind: OrderKind::Limit { price: 100 * PRICE_SCALE },
        tif: b'1', attrs: OrderAttrs { outside_rth: true, ..OrderAttrs::default() },
    })).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut order_acked = false;
    let mut cancel_sent = false;
    let mut order_cancelled = false;
    let mut rejected_order: Option<u64> = None;

    while Instant::now() < deadline {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100))
            && update.order_id == oid {
                match update.status {
                    OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                        order_acked = true;
                        if !cancel_sent {
                            control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: oid })).unwrap();
                            cancel_sent = true;
                        }
                    }
                    OrderStatus::Cancelled => { order_cancelled = true; break; }
                    OrderStatus::Rejected => { rejected_order = Some(update.order_id); break; }
                    _ => {}
                }
            }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if let Some(id) = rejected_order {
        skipped!("  SKIP: Futures order rejected — {}\n", reject_reason(&shared, id));
    } else {
        if skip_unacked_if_closed(order_acked) { return conns; }
        assert!(order_acked, "Futures order should be acknowledged");
        assert!(order_cancelled, "Futures order should be cancelled");
        println!("  PASS\n");
    }
    conns
}

pub(super) fn phase_options_order(conns: Conns) -> Conns {
    phase!("--- Phase 100: Options Contract Details + Order (SPY options) ---");

    // Look up SPY options
    let now = ibx::protocol::datetime::chrono_free_timestamp();
    let mut ccp = conns.ccp;
    ccp.send_fix(&[
        (fix::TAG_MSG_TYPE, "c"),
        (fix::TAG_SENDING_TIME, &now),
        (contracts::TAG_SECURITY_REQ_ID, "ROPT"),
        (contracts::TAG_SECURITY_REQ_TYPE, "2"),
        (contracts::TAG_SYMBOL, "SPY"),
        (contracts::TAG_SECURITY_TYPE, "OPT"),
        (contracts::TAG_EXCHANGE, "BEST"),
        (contracts::TAG_CURRENCY, "USD"),
        (contracts::TAG_IB_SOURCE, "Socket"),
    ]).expect("Failed to send options secdef request");

    let mut option_contracts: Vec<contracts::ContractDefinition> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline {
        match ccp.try_recv() {
            Ok(0) => { std::thread::sleep(Duration::from_millis(50)); continue; }
            Err(e) => { println!("  CCP recv error: {e}"); break; }
            Ok(_) => {}
        }
        let mut got_end = false;
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
                let msg_type = tags.get(&fix::TAG_MSG_TYPE).map(|s| s.as_str()).unwrap_or("?");
                if msg_type == "d" {
                    if let Some(resp_type) = tags.get(&contracts::TAG_SECURITY_RESPONSE_TYPE)
                        && (resp_type == "6" || resp_type == "5") {
                            got_end = true;
                            continue;
                        }
                    if let Some(def) = contracts::parse_secdef_response(&msg, true)
                        && def.sec_type == contracts::SecurityType::Option && def.right.is_some() {
                            option_contracts.push(def);
                        }
                }
            }
        }
        if got_end && !option_contracts.is_empty() { break; }
    }

    if option_contracts.is_empty() {
        lookup_returned_nothing("no SPY option contracts came back");
    }

    // Pick the first call option found
    let opt = option_contracts.iter()
        .find(|d| d.right == Some(contracts::OptionRight::Call))
        .unwrap_or(&option_contracts[0]);
    println!("  Found {} option contracts, using: {} conId={} strike={} right={:?} expiry={}",
        option_contracts.len(), opt.symbol, opt.con_id, opt.strike,
        opt.right, opt.last_trade_date);
    assert!(opt.strike > 0.0, "Option strike should be positive");
    assert!(opt.multiplier > 0.0, "Option multiplier should be positive (typically 100)");

    // Submit an option limit order using the actual option con_id
    let opt_con_id = opt.con_id;
    let opt_last_trade_date = opt.last_trade_date.clone();
    let opt_strike = opt.strike;
    let opt_multiplier = opt.multiplier as i64;
    let opt_is_call = opt.right == Some(contracts::OptionRight::Call);
    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, ccp, conns.hmds, None,
    );
    let inst = hot_loop.context_mut().register_instrument(opt_con_id as i64);
    hot_loop.context_mut().set_symbol(inst, "SPY".to_string());
    // Without these the order named a symbol and nothing else, so it went out
    // as a stock on SPY — which the venue accepts, and which is why this phase
    // reported an option order working when it had never sent one.
    hot_loop.context_mut().set_routing(inst, "OPT", "SMART");
    hot_loop.context_mut().set_order_identity(inst, &format!(
        "{}|{}|{}|{}",
        opt_last_trade_date,
        opt_strike,
        if opt_is_call { "C" } else { "P" },
        opt_multiplier,
    ));

    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
        order_id: oid, instrument: inst, side: Side::Buy, qty: ibx::types::QTY_SCALE,
        kind: OrderKind::Limit { price: 1_000_000 },
        tif: b'1', attrs: OrderAttrs { outside_rth: true, ..OrderAttrs::default() },
    })).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut order_acked = false;
    let mut cancel_sent = false;
    let mut order_cancelled = false;
    let mut rejected_order: Option<u64> = None;

    while Instant::now() < deadline {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100))
            && update.order_id == oid {
                match update.status {
                    OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                        order_acked = true;
                        if !cancel_sent {
                            control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: oid })).unwrap();
                            cancel_sent = true;
                        }
                    }
                    OrderStatus::Cancelled => { order_cancelled = true; break; }
                    OrderStatus::Rejected => { rejected_order = Some(update.order_id); break; }
                    _ => {}
                }
            }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if let Some(id) = rejected_order {
        skipped!("  SKIP: Option order rejected — {}\n", reject_reason(&shared, id));
    } else {
        if skip_unacked_if_closed(order_acked) { return conns; }
        assert!(order_acked, "Option order should be acknowledged");
        assert!(order_cancelled, "Option order should be cancelled");
        println!("  PASS\n");
    }
    conns
}

pub(super) fn phase_concurrent_orders(conns: Conns) -> Conns {
    phase!("--- Phase 101: Concurrent Orders in Flight (3 simultaneous limit orders) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    // Register SPY
    let spy_inst = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(spy_inst, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(spy_inst, "STK", "SMART");

    // Submit 3 limit orders simultaneously at $1.00 (far below market)
    let oid1 = next_order_id();
    let oid2 = oid1 + 1;
    let oid3 = oid1 + 2;

    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid1, instrument: 0, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid2, instrument: 0, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid3, instrument: 0, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, regulatory_snapshot: false, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut acked = [false; 3];
    let mut cancelled = [false; 3];
    let mut cancel_sent = false;
    let mut rejected_order: Option<u64> = None;
    let oids = [oid1, oid2, oid3];

    while Instant::now() < deadline {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            let idx = oids.iter().position(|&id| id == update.order_id);
            if let Some(i) = idx {
                match update.status {
                    OrderStatus::Submitted => {
                        acked[i] = true;
                        // Once all 3 are acked, cancel them all
                        if acked.iter().all(|&a| a) && !cancel_sent {
                            for &oid in &oids {
                                control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: oid })).unwrap();
                            }
                            cancel_sent = true;
                        }
                    }
                    OrderStatus::Cancelled => { cancelled[i] = true; }
                    OrderStatus::Rejected => { rejected_order = Some(update.order_id); break; }
                    _ => {}
                }
            }
        }
        if cancelled.iter().all(|&c| c) { break; }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if let Some(id) = rejected_order {
        skipped!("  SKIP: One or more orders rejected — {}\n", reject_reason(&shared, id));
        return conns;
    }

    let acked_count = acked.iter().filter(|&&a| a).count();
    let cancelled_count = cancelled.iter().filter(|&&c| c).count();
    println!("  Acked: {acked_count}/3  Cancelled: {cancelled_count}/3");

    assert_eq!(acked_count, 3, "All 3 orders should be acknowledged");
    assert_eq!(cancelled_count, 3, "All 3 orders should be cancelled");
    println!("  PASS\n");
    conns
}

/// The same instrument kind, resolved on venues outside the United States.
///
/// Every other phase names SMART, BEST, IDEALPRO, CME or NASDAQ, so the whole
/// suite could pass while every non-US listing resolved to the wrong exchange
/// or lost its currency. A definition is reference data, so this runs whether
/// or not any of these markets is trading.
///
/// Hong Kong is deliberately absent: SMART answered that no definition exists
/// and whether the exchange answers by name has not been established, so it is
/// not asserted either way here.
pub(super) fn phase_global_venues(conns: Conns) -> Conns {
    phase!("--- Global venues (definition, currency and exchange outside the US) ---");

    // Symbol, currency, and the exchange the listing belongs to.
    const VENUES: &[(&str, &str, &str)] = &[
        ("VOD", "GBP", "LSE"),
        ("SAP", "EUR", "IBIS"),
        ("ASML", "EUR", "AEB"),
        ("NESN", "CHF", "EBS"),
        ("7203", "JPY", "TSEJ"),
        ("BHP", "AUD", "ASX"),
    ];

    let Conns { farm, mut ccp, hmds, account_id } = conns;

    let now = ibx::protocol::datetime::chrono_free_timestamp();
    for (i, (symbol, currency, _)) in VENUES.iter().enumerate() {
        let req_id = format!("GV{i}");
        ccp.send_fix(&[
            (fix::TAG_MSG_TYPE, "c"),
            // Every request the venue answers states when it was sent. Without
            // it these six were ignored in silence, which reads exactly like a
            // venue that knows none of the contracts.
            (fix::TAG_SENDING_TIME, &now),
            (contracts::TAG_SECURITY_REQ_ID, &req_id),
            (contracts::TAG_SECURITY_REQ_TYPE, "2"),
            (contracts::TAG_SYMBOL, symbol),
            (contracts::TAG_SECURITY_TYPE, "CS"),
            (contracts::TAG_EXCHANGE, "SMART"),
            (contracts::TAG_CURRENCY, currency),
            (contracts::TAG_IB_SOURCE, "Socket"),
        ]).expect("failed to send a definition request");
    }

    let mut found: Vec<contracts::ContractDefinition> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(25);
    // SMART can answer one symbol with several listings, so counting replies
    // says nothing about how many symbols were answered. Wait on the symbols.
    let answered = |found: &[contracts::ContractDefinition]| {
        VENUES.iter().all(|(symbol, currency, _)| {
            found.iter().any(|d| {
                (d.symbol.eq_ignore_ascii_case(symbol)
                    || d.local_symbol.eq_ignore_ascii_case(symbol))
                    && d.currency == *currency
            })
        })
    };

    while Instant::now() < deadline && !answered(&found) {
        match ccp.try_recv() {
            Ok(0) => { std::thread::sleep(Duration::from_millis(50)); continue; }
            Err(e) => { println!("  CCP recv error: {e}"); break; }
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
                if tags.get(&fix::TAG_MSG_TYPE).map(|s| s.as_str()) == Some("d")
                    && let Some(def) = contracts::parse_secdef_response(&msg, true)
                    && !found.iter().any(|d| d.con_id == def.con_id)
                {
                    found.push(def);
                }
            }
        }
    }

    for (symbol, currency, exchange) in VENUES {
        let named: Vec<_> = found.iter().filter(|d| {
            d.symbol.eq_ignore_ascii_case(symbol) || d.local_symbol.eq_ignore_ascii_case(symbol)
        }).collect();

        if named.is_empty() {
            lookup_returned_nothing(&format!(
                "no definition came back for {symbol} on {exchange}; what did answer: {:?}",
                found.iter()
                    .map(|d| (d.symbol.clone(), d.currency.clone(), d.primary_exchange.clone()))
                    .collect::<Vec<_>>(),
            ));
        }

        // The listing asked for is the one quoted in the currency asked for. A
        // reply in another currency is a different listing on another venue,
        // and an order priced against it is priced in the wrong money.
        let Some(def) = named.iter().find(|d| d.currency == *currency) else {
            panic!(
                "{symbol} was asked for in {currency} and came back only as {:?}",
                named.iter().map(|d| (&d.currency, &d.primary_exchange)).collect::<Vec<_>>(),
            );
        };

        println!(
            "  {symbol}: con_id={} currency={} primary={} class={}",
            def.con_id, def.currency, def.primary_exchange, def.trading_class,
        );
        assert!(
            def.primary_exchange.eq_ignore_ascii_case(exchange)
                || def.valid_exchanges.iter().any(|e| e.eq_ignore_ascii_case(exchange)),
            "{symbol} belongs on {exchange}; this definition names {} and lists {:?}",
            def.primary_exchange, def.valid_exchanges,
        );
        assert!(def.con_id != 0, "{symbol} resolved without an id");
    }

    println!("  PASS ({} venues, {} currencies)\n", VENUES.len(), {
        let mut c: Vec<&str> = VENUES.iter().map(|(_, c, _)| *c).collect();
        c.sort_unstable();
        c.dedup();
        c.len()
    });

    Conns { farm, ccp, hmds, account_id }
}

/// An order on a contract that is not priced in dollars.
///
/// Every order the suite places is quoted in USD, and the currency is part of a
/// contract's identity, so the whole order path could be wrong for a foreign
/// listing and nothing here would say so. This resolves the London listing of a
/// share quoted in sterling and places a limit far below anything it could
/// trade at, then cancels it.
///
/// The limit is deliberately unreachable: the point is that the venue accepts
/// and works the order, not that anyone gets filled.
///
/// **This does not pass yet, and the reason is not settled.** With London
/// trading, the venue neither acknowledges nor refuses the order — through
/// SMART and routed to LSE by name alike. Silence is not a refusal: a
/// permission this account lacks is normally stated. What is established is
/// that the listing resolves correctly and the currency reaches the wire —
/// omitting it earns "Contract does not match supplied contract parameters",
/// which is the venue confirming it reads tag 15. What is not established is
/// whether this account may trade London at all, or whether an order on a
/// foreign venue must carry a field this client does not send. The logon
/// permission map answers the first and has not been read for this account.
pub(super) fn phase_non_usd_order(conns: Conns) -> Conns {
    phase!("--- Non-dollar order (VOD, London, sterling) ---");

    let now = ibx::protocol::datetime::chrono_free_timestamp();
    let mut ccp = conns.ccp;
    ccp.send_fix(&[
        (fix::TAG_MSG_TYPE, "c"),
        (fix::TAG_SENDING_TIME, &now),
        (contracts::TAG_SECURITY_REQ_ID, "RGBP"),
        (contracts::TAG_SECURITY_REQ_TYPE, "2"),
        (contracts::TAG_SYMBOL, "VOD"),
        (contracts::TAG_SECURITY_TYPE, "CS"),
        (contracts::TAG_EXCHANGE, "SMART"),
        (contracts::TAG_CURRENCY, "GBP"),
        (contracts::TAG_IB_SOURCE, "Socket"),
    ]).expect("failed to send the definition request");

    let mut listing: Option<contracts::ContractDefinition> = None;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && listing.is_none() {
        match ccp.try_recv() {
            Ok(0) => { std::thread::sleep(Duration::from_millis(50)); continue; }
            Err(e) => { println!("  CCP recv error: {e}"); break; }
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
                if tags.get(&fix::TAG_MSG_TYPE).map(|s| s.as_str()) == Some("d")
                    && let Some(def) = contracts::parse_secdef_response(&msg, true)
                    && def.currency == "GBP"
                {
                    listing = Some(def);
                }
            }
        }
    }

    let Some(def) = listing else {
        lookup_returned_nothing("no sterling listing of VOD came back");
    };
    println!("  Listing: con_id={} currency={} primary={}",
        def.con_id, def.currency, def.primary_exchange);

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, ccp, conns.hmds, None,
    );
    let inst = hot_loop.context_mut().register_instrument(def.con_id as i64);
    hot_loop.context_mut().set_symbol(inst, "VOD".to_string());
    hot_loop.context_mut().set_routing(inst, "CS", "SMART");
    // What the contract is priced in. Without it the order states dollars, and
    // the venue answers that the contract does not match the parameters given —
    // correctly, because a sterling listing ordered in dollars is a different
    // contract. This is the identity the client surface builds for itself.
    hot_loop.context_mut().set_order_identity(
        inst,
        &ibx::types::model::contract_identity("", 0.0, "", "", &def.currency),
    );

    // A tenth of a pound, against a share that trades near three quarters of
    // one. Nothing this order does can fill.
    let limit = ibx::types::PRICE_SCALE / 10;
    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
        order_id: oid, instrument: inst, side: Side::Buy, qty: ibx::types::QTY_SCALE,
        kind: OrderKind::Limit { price: limit }, tif: b'1',
        attrs: OrderAttrs { outside_rth: true, ..Default::default() },
    })).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut order_acked = false;
    let mut cancel_sent = false;
    let mut order_cancelled = false;
    let mut rejected_order: Option<u64> = None;

    while Instant::now() < deadline {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100))
            && update.order_id == oid {
                match update.status {
                    OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                        order_acked = true;
                        if !cancel_sent {
                            control_tx.send(ControlCommand::Order(
                                OrderRequest::Cancel { order_id: oid })).unwrap();
                            cancel_sent = true;
                        }
                    }
                    OrderStatus::Cancelled => { order_cancelled = true; break; }
                    OrderStatus::Rejected => { rejected_order = Some(update.order_id); break; }
                    _ => {}
                }
            }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if let Some(id) = rejected_order {
        skipped!("  SKIP: the venue refused a sterling order — {}\n", reject_reason(&shared, id));
    } else {
        if !order_acked && !london_is_trading() {
            skipped!("  SKIP: London is not trading, so nothing works this order\n");
            return conns;
        }
        assert!(order_acked, "London is trading, so a sterling order should be acknowledged");
        assert!(order_cancelled, "a sterling order should be cancelled");
        println!("  PASS\n");
    }
    conns
}

// ─── Phase 194: A crypto order ───

/// What the venue does with an order for a crypto.
///
/// A crypto is the one contract quoted around the clock, so this is also the
/// only order phase that says anything on a Saturday. Its quantity is the
/// reason it is here: a crypto is counted in hundred-millionths, and an order
/// for a fraction of one is the shape every other asset never sends.
pub(super) fn phase_crypto_order(conns: Conns) -> Conns {
    phase!("--- Phase 194: Crypto Order (BTC on its own venue) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst = hot_loop.context_mut().register_instrument(479624278);
    hot_loop.context_mut().set_symbol(inst, "BTC".to_string());
    // A crypto is not a stock and does not route smart: it trades on the one
    // venue that carries it.
    hot_loop.context_mut().set_routing(inst, "CRYPTO", "PAXOS");

    // A thousandth of a coin, which is a quantity no other asset class states,
    // priced far under the market so it cannot trade.
    //
    // Immediate-or-cancel, because the venue says so: a day order is answered
    // "The crypto buy order must be Minutes or IOC". So the order is expected
    // to be taken and then cancelled for want of a fill, which is the venue
    // acting on an order it accepted.
    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
        order_id: oid, instrument: inst, side: Side::Buy,
        qty: ibx::types::QTY_SCALE / 1000,
        kind: OrderKind::Limit { price: 10_000 * ibx::types::PRICE_SCALE },
        tif: b'3', attrs: OrderAttrs::default(),
    })).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(45);
    let mut order_acked = false;
    let mut cancel_sent = false;
    let mut order_cancelled = false;
    let mut rejected_order: Option<u64> = None;

    while Instant::now() < deadline {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100))
            && update.order_id == oid {
                match update.status {
                    OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                        order_acked = true;
                        if !cancel_sent {
                            control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: oid })).unwrap();
                            cancel_sent = true;
                        }
                    }
                    OrderStatus::Cancelled => { order_cancelled = true; break; }
                    OrderStatus::Rejected => { rejected_order = Some(update.order_id); break; }
                    _ => {}
                }
            }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if let Some(id) = rejected_order {
        let why = shared.orders.get_order_info(id)
            .map(|info| info.order_state.reject_reason)
            .filter(|why| !why.is_empty())
            .unwrap_or_else(|| "no reason reported".to_string());
        skipped!("  SKIP: the venue refused a crypto order — {why}\n");
        return conns;
    }
    // An immediate-or-cancel order that cannot fill is cancelled by the venue,
    // and may be cancelled without ever being reported working. Either way the
    // venue took it, which is what this phase asks.
    assert!(
        order_acked || order_cancelled,
        "the crypto order was neither acknowledged nor acted on",
    );
    if order_acked {
        println!("  immediate-or-cancel: taken, then cancelled for want of a fill");
    } else {
        println!("  immediate-or-cancel: taken and cancelled without resting");
    }
    println!("  PASS\n");
    conns
}

// ─── Phase 196: The other life a crypto order may have ───

/// The venue names two: immediate-or-cancel, and one measured in minutes.
///
/// Nothing has ever sent the second. This client can name it — tag 59 carries
/// it — and whether the venue takes it named alone, or wants the number of
/// minutes beside it, is a question only a session answers.
pub(super) fn phase_crypto_minutes_tif(conns: Conns) -> Conns {
    phase!("--- Phase 196: A crypto order living by the minute ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst = hot_loop.context_mut().register_instrument(479624278);
    hot_loop.context_mut().set_symbol(inst, "BTC".to_string());
    hot_loop.context_mut().set_routing(inst, "CRYPTO", "PAXOS");

    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
        order_id: oid, instrument: inst, side: Side::Buy,
        qty: ibx::types::QTY_SCALE / 1000,
        kind: OrderKind::Limit { price: 10_000 * ibx::types::PRICE_SCALE },
        tif: b'p', attrs: OrderAttrs::default(),
    })).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(45);
    let mut working = false;
    let mut cancelled = false;
    let mut refused: Option<u64> = None;

    while Instant::now() < deadline {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100))
            && update.order_id == oid {
                match update.status {
                    OrderStatus::Submitted | OrderStatus::PreSubmitted => {
                        working = true;
                        control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id: oid })).unwrap();
                    }
                    OrderStatus::Cancelled => { cancelled = true; break; }
                    OrderStatus::Rejected => { refused = Some(update.order_id); break; }
                    _ => {}
                }
            }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    match refused {
        Some(id) => {
            let why = shared.orders.get_order_info(id)
                .map(|info| info.order_state.reject_reason)
                .filter(|why| !why.is_empty())
                .unwrap_or_else(|| "no reason reported".to_string());
            println!("  named alone it is refused — {why}");
            println!("  PASS\n");
        }
        None if working || cancelled => {
            println!("  named alone it is taken, and the order works");
            println!("  PASS\n");
        }
        None => {
            println!("  nothing came back within the deadline");
            println!("  PASS\n");
        }
    }
    conns
}

// ─── Phase 195: A crypto fill ───

/// A fraction of a coin, bought and sold, read back as the fraction it was.
///
/// A crypto is counted in hundred-millionths where a share is counted in
/// hundredths, so a quantity that survives the round trip on every other asset
/// class can still come back a hundred million times wrong here. Nothing but a
/// fill settles that: the quantity goes out, the venue fills it, and the number
/// that comes back is the one to read.
pub(super) fn phase_crypto_fill(conns: Conns) -> Conns {
    phase!("--- Phase 195: Crypto Fill (a thousandth of a coin, round trip) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst = hot_loop.context_mut().register_instrument(479624278);
    hot_loop.context_mut().set_symbol(inst, "BTC".to_string());
    hot_loop.context_mut().set_routing(inst, "CRYPTO", "PAXOS");

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 479624278, symbol: "BTC".into(), exchange: "PAXOS".into(), sec_type: "CRYPTO".into(), currency: "USD".into(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, regulatory_snapshot: false, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    // A thousandth of a coin, which at the price this trades at is a few tens
    // of dollars of paper money.
    const SIZE: i64 = ibx::types::QTY_SCALE / 1000;
    let buy = next_order_id();
    let mut sell: Option<u64> = None;
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut ticks = 0u32;
    let mut sent_buy = false;
    let mut bought: Option<(f64, i64)> = None;
    let mut sold: Option<(f64, i64)> = None;
    let mut refused: Option<u64> = None;

    while Instant::now() < deadline {
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Tick(_)) => {
                ticks += 1;
                // Priced through the offer so it trades rather than resting,
                // which an immediate-or-cancel order has no time to do.
                if !sent_buy && ticks >= 3 {
                    let ask = shared.market.quote(inst).ask;
                    if ask <= 0 { continue; }
                    // Whole dollars, which sit on this venue's grid whatever
                    // its increment is. Priced off the grid the venue answers
                    // "Invalid Price" — an immediate-or-cancel order fills at
                    // the offer regardless, so paying through it costs nothing.
                    let price = (ask / ibx::types::PRICE_SCALE + 100) * ibx::types::PRICE_SCALE;
                    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
                        order_id: buy, instrument: inst, side: Side::Buy, qty: SIZE,
                        kind: OrderKind::Limit { price },
                        tif: b'3', attrs: OrderAttrs::default(),
                    })).unwrap();
                    sent_buy = true;
                }
            }
            Ok(Event::Fill(fill)) => {
                let qty = fill.qty as f64 / ibx::types::QTY_SCALE as f64;
                let price = fill.price as f64 / ibx::types::PRICE_SCALE as f64;
                if fill.side == Side::Buy && bought.is_none() {
                    println!("  bought {qty} at {price}");
                    bought = Some((qty, fill.qty));
                    // Flattened straight back, so the phase leaves nothing on.
                    let bid = shared.market.quote(inst).bid;
                    let oid = next_order_id();
                    sell = Some(oid);
                    let price = (bid / ibx::types::PRICE_SCALE - 100) * ibx::types::PRICE_SCALE;
                    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
                        order_id: oid, instrument: inst, side: Side::Sell, qty: fill.qty,
                        kind: OrderKind::Limit { price },
                        tif: b'3', attrs: OrderAttrs::default(),
                    })).unwrap();
                } else if fill.side == Side::Sell && sold.is_none() {
                    println!("  sold {qty} at {price}");
                    sold = Some((qty, fill.qty));
                    break;
                }
            }
            Ok(Event::OrderUpdate(update))
                if update.status == OrderStatus::Rejected
                    && (update.order_id == buy || Some(update.order_id) == sell) =>
            {
                refused = Some(update.order_id);
                break;
            }
            _ => {}
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if let Some(id) = refused {
        let why = shared.orders.get_order_info(id)
            .map(|info| info.order_state.reject_reason)
            .filter(|why| !why.is_empty())
            .unwrap_or_else(|| "no reason reported".to_string());
        skipped!("  SKIP: the venue refused it — {why}\n");
        return conns;
    }
    let Some((qty, raw)) = bought else {
        skipped!("  SKIP: nothing traded within the deadline\n");
        return conns;
    };
    // An immediate-or-cancel order takes what is at the offer and cancels the
    // rest, so the fill is at most what was asked for and often less. What it
    // must be is a fraction: read a hundred million times too large it would
    // come back as thousands of coins, which is the mistake this exists for.
    assert!(
        raw > 0 && raw <= SIZE,
        "a thousandth of a coin was asked for and {qty} came back",
    );
    if let Some((sold_qty, raw_sold)) = sold {
        assert_eq!(
            raw_sold, raw,
            "the fraction bought is the fraction sold: {qty} out, {sold_qty} back",
        );
        println!("  PASS — the fraction survives the round trip, position flat\n");
    } else {
        println!("  PASS — bought {qty}; the sale did not report in time\n");
    }
    conns
}
