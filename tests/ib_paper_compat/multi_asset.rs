//! Multi-asset-class order test phases (forex, futures, options).

use super::common::*;
use ibx::control::contracts;
use ibx::protocol::fix;
use ibx::protocol::fixcomp;
use ibx::protocol::connection::Frame;

pub(super) fn phase_forex_order(conns: Conns) -> Conns {
    println!("--- Phase 98: Forex Order Lifecycle (EUR.USD) ---");

    // First, look up EUR.USD contract
    let now = ibx::gateway::chrono_free_timestamp();
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
                    && let Some(def) = contracts::parse_secdef_response(&msg)
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
        shared.clone(), Some(event_tx), account_id.clone(), conns.farm, ccp, conns.hmds, None,
    );
    let inst = hot_loop.context_mut().register_instrument(fx_con_id as i64);
    hot_loop.context_mut().set_symbol(inst, "EUR".to_string());
    // A contract is not a stock because nobody said otherwise. Without this the
    // order goes out as a stock on the default venue, and the venue answers
    // that it knows no such contract — correctly.
    hot_loop.context_mut().set_routing(inst, "CASH", "IDEALPRO");

    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid, instrument: inst, side: Side::Buy, qty: 20000, kind: OrderKind::Limit { price: 50_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();
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
        println!("  SKIP: Forex order rejected — {}\n", reject_reason(&shared, id));
    } else {
        if skip_unacked_if_closed(order_acked) { return conns; }
        assert!(order_acked, "Forex order should be acknowledged");
        assert!(order_cancelled, "Forex order should be cancelled");
        println!("  PASS\n");
    }
    conns
}

pub(super) fn phase_futures_order(conns: Conns) -> Conns {
    println!("--- Phase 99: Futures Order (MES) ---");

    // Look up MES (Micro E-mini S&P 500)
    let now = ibx::gateway::chrono_free_timestamp();
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
                    && let Some(def) = contracts::parse_secdef_response(&msg)
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
        shared.clone(), Some(event_tx), account_id.clone(), conns.farm, ccp, conns.hmds, None,
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
    // the venue's own local symbol on SecurityID under the source that says
    // the identifier is the venue's own. A trading class describes the family
    // and is not stated on an order. Placing the order here is what keeps that
    // shape honest, because a definition lookup alone never exercises it.

    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
        order_id: oid, instrument: inst, side: Side::Buy, qty: 1,
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
        println!("  SKIP: Futures order rejected — {}\n", reject_reason(&shared, id));
    } else {
        if skip_unacked_if_closed(order_acked) { return conns; }
        assert!(order_acked, "Futures order should be acknowledged");
        assert!(order_cancelled, "Futures order should be cancelled");
        println!("  PASS\n");
    }
    conns
}

pub(super) fn phase_options_order(conns: Conns) -> Conns {
    println!("--- Phase 100: Options Contract Details + Order (SPY options) ---");

    // Look up SPY options
    let now = ibx::gateway::chrono_free_timestamp();
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
                    if let Some(def) = contracts::parse_secdef_response(&msg)
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
        shared.clone(), Some(event_tx), account_id.clone(), conns.farm, ccp, conns.hmds, None,
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
        order_id: oid, instrument: inst, side: Side::Buy, qty: 1,
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
        println!("  SKIP: Option order rejected — {}\n", reject_reason(&shared, id));
    } else {
        if skip_unacked_if_closed(order_acked) { return conns; }
        assert!(order_acked, "Option order should be acknowledged");
        assert!(order_cancelled, "Option order should be cancelled");
        println!("  PASS\n");
    }
    conns
}

pub(super) fn phase_concurrent_orders(conns: Conns) -> Conns {
    println!("--- Phase 101: Concurrent Orders in Flight (3 simultaneous limit orders) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(event_tx), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    // Register SPY
    let spy_inst = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(spy_inst, "SPY".to_string());

    // Submit 3 limit orders simultaneously at $1.00 (far below market)
    let oid1 = next_order_id();
    let oid2 = oid1 + 1;
    let oid3 = oid1 + 2;

    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid1, instrument: 0, side: Side::Buy, qty: 1, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid2, instrument: 0, side: Side::Buy, qty: 1, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: oid3, instrument: 0, side: Side::Buy, qty: 1, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();

    control_tx.send(ControlCommand::Subscribe { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(), mode_9887: 0, reply_tx: None }).unwrap();
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
        println!("  SKIP: One or more orders rejected — {}\n", reject_reason(&shared, id));
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
    println!("--- Global venues (definition, currency and exchange outside the US) ---");

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

    let now = ibx::gateway::chrono_free_timestamp();
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
                    && let Some(def) = contracts::parse_secdef_response(&msg)
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
    println!("--- Non-dollar order (VOD, London, sterling) ---");

    let now = ibx::gateway::chrono_free_timestamp();
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
                    && let Some(def) = contracts::parse_secdef_response(&msg)
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
        shared.clone(), Some(event_tx), account_id.clone(), conns.farm, ccp, conns.hmds, None,
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
        &ibx::client_core::ClientCore::contract_identity("", 0.0, "", "", &def.currency),
    );

    // A tenth of a pound, against a share that trades near three quarters of
    // one. Nothing this order does can fill.
    let limit = ibx::types::PRICE_SCALE / 10;
    let oid = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx {
        order_id: oid, instrument: inst, side: Side::Buy, qty: 1,
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
        println!("  SKIP: the venue refused a sterling order — {}\n", reject_reason(&shared, id));
    } else {
        if !order_acked && !london_is_trading() {
            println!("  SKIP: London is not trading, so nothing works this order\n");
            return conns;
        }
        assert!(order_acked, "London is trading, so a sterling order should be acknowledged");
        assert!(order_cancelled, "a sterling order should be cancelled");
        println!("  PASS\n");
    }
    conns
}
