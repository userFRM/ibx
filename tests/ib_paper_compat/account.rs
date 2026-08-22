//! Account data, PnL, summary, and position tracking test phases.

use super::common::*;
use ibx::api::types as api;
use ibx::api::wrapper::Wrapper;
use ibx::protocol::fix;

pub(super) fn phase_account_data(conns: Conns) -> Conns {
    phase!("--- Phase 4: Account Data Reception ---");

    let account_id = conns.account_id;

    let mut ccp = conns.ccp;
    let ts = ibx::protocol::datetime::chrono_free_timestamp();
    let _ = ccp.send_fix(&[
        (fix::TAG_MSG_TYPE, "U"),
        (fix::TAG_SENDING_TIME, &ts),
        (6040, "76"),
        (1, ""),
        (6565, "1"),
    ]);

    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 265598, symbol: "AAPL".into(), exchange: String::new(), sec_type: String::new(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut account_checked = false;
    let mut net_liq = 0i64;

    // Account figures arrive on their own, not behind a tick. Reading them
    // only when one arrives meant a quiet instrument reported the account
    // missing, which is a statement about the market rather than the session.
    while Instant::now() < deadline {
        let _ = event_rx.recv_timeout(Duration::from_millis(200));
        let acct = shared.portfolio.account();
        if acct.net_liquidation != 0 {
            net_liq = acct.net_liquidation;
            println!("  ACCOUNT: net_liq={:.2}", net_liq as f64 / PRICE_SCALE as f64);
            account_checked = true;
            break;
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if account_checked {
        assert!(net_liq > 0, "Paper account net liquidation should be > 0");
        println!("  net_liq=${:.2}", net_liq as f64 / PRICE_SCALE as f64);
        println!("  PASS\n");
    } else {
        session_owed(&shared, "no account data arrived");
    }
    conns
}

pub(super) fn phase_account_pnl(conns: Conns) -> Conns {
    phase!("--- Phase 14: Account PnL Reception ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    // Register SPY instrument so on_start order submission works
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    let order_id = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();

    let join = run_hot_loop(hot_loop);

    // The venue pushes account values on its own cadence off the logon
    // subscription; two consecutive pushes were 23s apart when measured. This
    // phase runs on a fresh SharedState, so nothing an earlier phase received
    // is here to short-circuit the wait, and a 15s deadline could expire
    // between two pushes and report the account as never arriving.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut account_received = false;
    let mut net_liq = 0i64;
    let mut probe_done = false;

    // Both conditions, not either. The $1 limit resolves in seconds while the
    // account push runs on the venue's cadence, so exiting as soon as the
    // order probe finished ended the wait before the account could arrive and
    // reported it as never sent.
    while Instant::now() < deadline && !(probe_done && account_received) {
        // Account state is written straight into SharedState by the account-summary
        // handler, which emits no Event — so poll it every iteration instead of only
        // when one arrives. Checking it inside the Tick/OrderUpdate arms meant a
        // Closed session (no tick to wake us, no ack for a $1 limit) never looked at
        // the account at all, and the phase then blamed tag 9806 for what was really
        // an absence of events.
        if !account_received {
            let acct = shared.portfolio.account();
            if acct.net_liquidation != 0 {
                net_liq = acct.net_liquidation;
                account_received = true;
            }
        }
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            if matches!(update.status, OrderStatus::Submitted | OrderStatus::PreSubmitted) {
                control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id })).unwrap();
            }
            if matches!(update.status, OrderStatus::Cancelled | OrderStatus::Rejected) {
                probe_done = true;
            }
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    assert!(account_received,
        "no account values in 60s — the logon subscription should have pushed at least once");
    assert!(net_liq > 0, "Paper account net liquidation should be > 0");
    println!("  NetLiq: ${:.2}", net_liq as f64 / PRICE_SCALE as f64);
    println!("  PASS\n");
    conns
}

pub(super) fn phase_position_tracking(conns: Conns) -> Conns {
    phase!("--- Phase 97: Position Tracking (SPY buy+sell round trip) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(), conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut phase = 0u8; // 0=wait ticks, 1=buy sent, 2=sell sent
    let mut tick_count = 0u32;
    let mut got_position_update = false;
    // What the account held before the buy: the round trip returns to this, not
    // to zero, since the account may already hold the contract.
    let mut held_before: Option<f64> = None;
    // The round trip ends on the sell filling, not on it being submitted.
    let mut sell_filled = false;
    // Set once the round trip is done, to keep draining for a moment rather
    // than leaving the moment the last fill lands.
    let mut settle_by: Option<Instant> = None;

    while Instant::now() < deadline {
        if settle_by.is_some_and(|at| Instant::now() >= at) {
            break;
        }
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Tick(instrument)) => {
                tick_count += 1;
                if phase == 0 && tick_count >= 5 {
                    held_before = Some(shared.portfolio.position(instrument));
                    let buy_oid = next_order_id();
                    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: buy_oid, instrument, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Market, tif: b'0', attrs: OrderAttrs::default() })).unwrap();
                    phase = 1;
                }
            }
            Ok(Event::Fill(fill)) => {
                if phase == 1 && fill.side == Side::Buy {
                    let sell_order_id = next_order_id() + 1;
                    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: sell_order_id, instrument: fill.instrument, side: Side::Sell, qty: ibx::types::QTY_SCALE, kind: OrderKind::Market, tif: b'0', attrs: OrderAttrs::default() })).unwrap();
                    phase = 2;
                } else if phase == 2 && fill.side == Side::Sell {
                    sell_filled = true;
                    // Keep receiving rather than sleeping through it: a position
                    // update the venue sends in this window arrives on this
                    // channel, and sleeping here then breaking is how it was
                    // missed. The deadline below ends the wait.
                    settle_by = Some(Instant::now() + Duration::from_secs(3));
                }
            }
            Ok(Event::PositionUpdate { instrument, con_id, position, avg_cost }) => {
                println!("  PositionUpdate: inst={} conId={} pos={} avgCost={:.4}",
                    instrument, con_id, position, avg_cost as f64 / ibx::types::PRICE_SCALE as f64);
                got_position_update = true;
            }
            Ok(Event::OrderUpdate(update))
                if update.status == OrderStatus::Rejected => {
                    // Shut down first: the engine records the reason after it
                    // emits the update, so reading it here would race the write.
                    let conns = shutdown_and_reclaim(&control_tx, join, account_id);
                    skipped!("  SKIP: Order rejected — {}\n", reject_reason(&shared, update.order_id));
                    return conns;
                }
            _ => {}
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if phase < 1 {
        no_market(&shared, "no ticks arrived");
    } else if sell_filled {
        // Read from what the session holds, which the engine moves on every fill.
        // The position event is the venue's feed and restates on its own
        // schedule, not on a fill, so it is not required here.
        let pos = shared.portfolio.position(0);
        let before = held_before.unwrap_or(0.0);
        println!(
            "  Final position: {pos}, held before: {before}{}",
            if got_position_update { " (the venue restated it too)" } else { "" },
        );
        // Both sides filled, so the account is back where it started. The
        // tolerance this replaces was the whole size of the trade: a share
        // bought and never sold satisfied it, and the phase reported the
        // position had returned while the account was still long it.
        assert_eq!(
            pos, before,
            "one share was bought and one sold, and the position did not come back to {before}",
        );
        println!("  PASS (position returned to {pos})\n");
    } else if phase == 2 {
        skipped!("  SKIP: the sell was sent and did not fill within the wait\n");
    } else {
        skipped!("  SKIP: Only reached phase {phase} (buy may not have filled)\n");
    }
    conns
}

pub(super) fn phase_account_summary(conns: Conns) -> Conns {
    phase!("--- Phase 106: Account Summary (verify individual tag values) ---");

    let account_id = conns.account_id.clone();
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    // Wait for account data to populate
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut has_account_data = false;

    while Instant::now() < deadline {
        // As in the reception phase: the figures do not arrive behind a tick.
        let _ = event_rx.recv_timeout(Duration::from_millis(200));
        {
            let acct = shared.portfolio.account();
            if acct.net_liquidation > 0 {
                // Verify individual fields
                println!("  NetLiquidation:    {:.2}", acct.net_liquidation as f64 / PRICE_SCALE as f64);
                println!("  BuyingPower:       {:.2}", acct.buying_power as f64 / PRICE_SCALE as f64);
                println!("  TotalCashValue:    {:.2}", acct.total_cash_value as f64 / PRICE_SCALE as f64);
                println!("  SettledCash:       {:.2}", acct.settled_cash as f64 / PRICE_SCALE as f64);
                println!("  AvailableFunds:    {:.2}", acct.available_funds as f64 / PRICE_SCALE as f64);
                println!("  ExcessLiquidity:   {:.2}", acct.excess_liquidity as f64 / PRICE_SCALE as f64);
                println!("  InitMarginReq:     {:.2}", acct.init_margin_req as f64 / PRICE_SCALE as f64);
                println!("  MaintMarginReq:    {:.2}", acct.maint_margin_req as f64 / PRICE_SCALE as f64);
                println!("  EquityWithLoan:    {:.2}", acct.equity_with_loan as f64 / PRICE_SCALE as f64);
                println!("  Cushion:           {:.4}", acct.cushion as f64 / PRICE_SCALE as f64);
                println!("  Leverage:          {:.4}", acct.leverage as f64 / PRICE_SCALE as f64);
                println!("  DayTradesRemain:   {}", acct.day_trades_remaining);
                println!("  UnrealizedPnL:     {:.2}", acct.unrealized_pnl as f64 / PRICE_SCALE as f64);
                println!("  RealizedPnL:       {:.2}", acct.realized_pnl as f64 / PRICE_SCALE as f64);
                println!("  DailyPnL:          {:.2}", acct.daily_pnl as f64 / PRICE_SCALE as f64);
                has_account_data = true;

                // Validate sanity
                assert!(acct.net_liquidation > 0, "NetLiquidation should be positive");
                assert!(acct.buying_power >= 0, "BuyingPower should be non-negative");
                assert!(acct.available_funds >= 0, "AvailableFunds should be non-negative");
                assert!(acct.excess_liquidity >= 0, "ExcessLiquidity should be non-negative");
                // EquityWithLoanValue should be close to NetLiquidation for paper accounts
                if acct.equity_with_loan > 0 {
                    let ratio = acct.equity_with_loan as f64 / acct.net_liquidation as f64;
                    assert!(ratio > 0.5 && ratio < 2.0,
                        "EquityWithLoan/NetLiq ratio {ratio:.2} seems wrong");
                }
                break;
            }
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if has_account_data {
        println!("  PASS\n");
    } else {
        session_owed(&shared, "no account data arrived");
    }
    conns
}

/// Phase: Completed Orders — submit an order, cancel it, verify it appears in drain_completed_orders.
pub(super) fn phase_completed_orders(conns: Conns) -> Conns {
    phase!("--- Phase 120: Completed Orders (submit+cancel → drain) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    let order_id = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: false, ..Default::default() } })).unwrap();

    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut cancel_sent = false;
    let mut terminal = false;
    let mut refused = false;

    while Instant::now() < deadline && !terminal {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            println!("  OrderUpdate: id={} status={:?}", update.order_id, update.status);
            if matches!(update.status, OrderStatus::Submitted | OrderStatus::PreSubmitted) && !cancel_sent {
                control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id })).unwrap();
                cancel_sent = true;
            }
            // A rejection is not this phase's outcome: the order never worked, so
            // the cancel under test is never sent.
            if matches!(update.status, OrderStatus::Rejected) {
                refused = true;
                terminal = true;
            }
            if matches!(update.status, OrderStatus::Cancelled | OrderStatus::Filled) {
                terminal = true;
            }
        }
    }

    // Give the hot loop a moment to archive the completed order
    std::thread::sleep(Duration::from_millis(200));

    let completed = shared.orders.drain_completed_orders();
    println!("  Completed orders drained: {}", completed.len());
    for co in &completed {
        println!("    order_id={} status={:?} filled_qty={}", co.order_id, co.status, co.filled_qty);
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if !terminal {
        skipped!("  SKIP: Order never reached terminal state\n");
        return conns;
    }
    if refused {
        skipped!(
            "  SKIP: the venue refused the order — {}; nothing was withdrawn, so the \
             cancel this phase exists to follow was never sent\n",
            reject_reason(&shared, order_id),
        );
        return conns;
    }

    assert!(cancel_sent, "the order reached a terminal state without a cancel being sent");
    assert!(!completed.is_empty(), "Expected at least one completed order after cancel");
    let co = completed.iter().find(|c| c.order_id == order_id);
    assert!(co.is_some(), "completed order for the placed order_id not found");
    let co = co.unwrap();
    assert!(
        matches!(co.status, OrderStatus::Cancelled),
        "the order was withdrawn, so the completed record should say so, not {:?}", co.status
    );
    println!("  PASS\n");
    conns
}

/// Phase: Enriched API output — submit+cancel an order, then call the Rust API Wrapper
/// callbacks (completed_order, position) and verify the Contract/Order/OrderState fields
/// match the ibapi GT capture format.
pub(super) fn phase_enriched_order_cache(conns: Conns) -> Conns {
    phase!("--- Phase 130: Enriched API Wrapper Output (submit+cancel → req_completed_orders) ---");

    // Recording wrapper that captures full field data from completed_order/position callbacks
    struct GtWrapper {
        completed: Vec<(api::Contract, api::Order, api::OrderState)>,
        positions: Vec<(String, api::Contract, f64, f64)>,
    }
    impl Wrapper for GtWrapper {
        fn completed_order(&mut self, contract: &api::Contract, order: &api::Order, state: &api::OrderState) {
            self.completed.push((contract.clone(), order.clone(), state.clone()));
        }
        fn completed_orders_end(&mut self) {}
        fn position(&mut self, account: &str, contract: &api::Contract, pos: f64, avg_cost: f64) {
            self.positions.push((account.to_string(), contract.clone(), pos, avg_cost));
        }
        fn position_end(&mut self) {}
        fn open_order(&mut self, _: i64, _: &api::Contract, _: &api::Order, _: &api::OrderState) {}
        fn open_order_end(&mut self) {}
    }

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    // Fetch secdef first to populate contract cache with exchange/localSymbol/tradingClass
    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 756733, symbol: String::new(), sec_type: "STK".into(), exchange: String::new(), currency: String::new(), ..Default::default() }, req_id: 9999, filters: Default::default() }).unwrap();

    let order_id = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: false, ..Default::default() } })).unwrap();

    let join = run_hot_loop(hot_loop);

    // Wait for secdef + order lifecycle
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut cancel_sent = false;
    let mut terminal = false;

    while Instant::now() < deadline && !terminal {
        if let Ok(Event::OrderUpdate(update)) = event_rx.recv_timeout(Duration::from_millis(100)) {
            if matches!(update.status, OrderStatus::Submitted | OrderStatus::PreSubmitted) && !cancel_sent {
                control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id })).unwrap();
                cancel_sent = true;
            }
            if matches!(update.status, OrderStatus::Cancelled | OrderStatus::Rejected | OrderStatus::Filled) {
                terminal = true;
            }
        }
    }

    std::thread::sleep(Duration::from_millis(500));

    // ── Exercise the real API path: req_completed_orders + req_positions ──
    let mut wrapper = GtWrapper { completed: Vec::new(), positions: Vec::new() };

    // Manually do what EClient::req_completed_orders does
    for co in shared.orders.drain_completed_orders() {
        let status_str = match co.status {
            OrderStatus::Filled => "Filled",
            OrderStatus::Cancelled => "Cancelled",
            OrderStatus::Rejected => "Inactive",
            _ => "Unknown",
        };
        if let Some(info) = shared.orders.get_order_info(co.order_id) {
            let mut state = info.order_state;
            state.status = status_str.into();
            // What EClient::req_completed_orders does, and this loop did not:
            // the contract on the order record is what the execution report
            // stated, which is a subset of the definition. The definition is
            // what the caller is given, so a field the report omits — the
            // trading class among them — is filled from it at read time.
            let mut contract = info.contract.clone();
            if contract.con_id != 0
                && let Some(def) = shared.reference.get_contract(contract.con_id)
            {
                if contract.exchange.is_empty() { contract.exchange = def.exchange; }
                if contract.trading_class.is_empty() { contract.trading_class = def.trading_class; }
                if contract.local_symbol.is_empty() { contract.local_symbol = def.local_symbol; }
                if contract.primary_exchange.is_empty() { contract.primary_exchange = def.primary_exchange; }
            }
            wrapper.completed_order(&contract, &info.order, &state);
        } else {
            let c = api::Contract::default();
            let o = api::Order { order_id: co.order_id as i64, ..Default::default() };
            let s = api::OrderState { status: status_str.into(), ..Default::default() };
            wrapper.completed_order(&c, &o, &s);
        }
    }

    // Manually do what EClient::req_positions does
    for pi in shared.portfolio.position_infos() {
        // The same construction `req_positions` uses: the definition where one
        // has been fetched, and the holding's own fields where it has not. The
        // feed names the contract beside the quantity, so a definition that has
        // not landed does not leave the holding unnamed.
        let c = shared.reference.get_contract(pi.con_id).unwrap_or_else(|| api::Contract {
            con_id: pi.con_id,
            symbol: pi.symbol.clone(),
            sec_type: pi.sec_type.clone(),
            currency: pi.currency.clone(),
            multiplier: pi.multiplier.clone(),
            ..Default::default()
        });
        let avg_cost = pi.avg_cost as f64 / PRICE_SCALE as f64;
        wrapper.position(&account_id, &c, pi.position, avg_cost);
    }

    let gt_account = account_id.clone();
    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if !terminal {
        skipped!("  SKIP: Order never reached terminal state\n");
        return conns;
    }

    // ── Compare Wrapper output against GT expectations ──
    // GT: completedOrder has contract with conId/symbol/secType/currency,
    //     order with action/totalQuantity/orderType/tif/account,
    //     orderState with status
    let mut pass = true;
    if let Some((c, o, s)) = wrapper.completed.first() {
        println!("  completed_order callback received:");

        // Contract fields (GT: conId=756733, symbol=SPY, secType=STK, currency=USD)
        println!("    contract.conId      = {} (GT: 756733)", c.con_id);
        println!("    contract.symbol     = '{}' (GT: 'SPY')", c.symbol);
        println!("    contract.secType    = '{}' (GT: 'STK')", c.sec_type);
        println!("    contract.currency   = '{}' (GT: 'USD')", c.currency);
        println!("    contract.exchange   = '{}' (GT: 'SMART')", c.exchange);
        println!("    contract.localSym   = '{}' (GT: 'SPY')", c.local_symbol);
        println!("    contract.tradClass  = '{}' (GT: 'SPY')", c.trading_class);

        // Order fields (GT: action=BUY, totalQuantity=1, orderType=LMT, tif=GTC)
        println!("    order.action        = '{}' (GT: 'BUY')", o.action);
        println!("    order.totalQuantity = {} (GT: 1.0)", o.total_quantity);
        println!("    order.orderType     = '{}' (GT: 'LMT')", o.order_type);
        println!("    order.tif           = '{}' (GT: 'GTC')", o.tif);
        println!(
            "    order.account       = '{}' (GT: '{}')",
            super::common::redacted(&o.account),
            super::common::redacted(&gt_account),
        );

        // OrderState fields (GT: status=Cancelled)
        println!("    orderState.status   = '{}' (GT: 'Cancelled')", s.status);

        // Validate
        if c.con_id != 756733 { println!("    FAIL: conId"); pass = false; }
        if c.symbol != "SPY" { println!("    FAIL: symbol"); pass = false; }
        if c.sec_type != "STK" { println!("    FAIL: secType"); pass = false; }
        if c.currency != "USD" { println!("    FAIL: currency"); pass = false; }
        if c.local_symbol.is_empty() { println!("    FAIL: localSymbol empty"); pass = false; }
        if c.trading_class.is_empty() { println!("    FAIL: tradingClass empty"); pass = false; }
        if o.action != "BUY" { println!("    FAIL: action"); pass = false; }
        if (o.total_quantity - 1.0).abs() > 0.01 { println!("    FAIL: totalQuantity"); pass = false; }
        if o.order_type != "LMT" { println!("    FAIL: orderType"); pass = false; }
        if o.tif != "GTC" { println!("    FAIL: tif"); pass = false; }
        if o.account.is_empty() { println!("    FAIL: account empty"); pass = false; }
        if s.status != "Cancelled" { println!("    FAIL: status"); pass = false; }
    } else {
        println!("  No completed_order callback — CCP may not have sent enriched exec report");
        pass = false;
    }

    if pass {
        println!("  PASS (all fields match GT)\n");
    } else {
        println!("  FAIL\n");
    }
    assert!(pass, "Enriched API output did not match GT expectations");

    conns
}

/// Phase: req_all_open_orders — submit a limit order, call open_order Wrapper callback,
/// verify Contract/Order/OrderState fields match GT, then cancel.
pub(super) fn phase_enriched_open_orders(conns: Conns) -> Conns {
    phase!("--- Phase 131: Enriched open_order Wrapper Output (submit → req_all_open_orders → cancel) ---");

    struct OoWrapper {
        orders: Vec<(i64, api::Contract, api::Order, api::OrderState)>,
    }
    impl Wrapper for OoWrapper {
        fn open_order(&mut self, order_id: i64, contract: &api::Contract, order: &api::Order, state: &api::OrderState) {
            self.orders.push((order_id, contract.clone(), order.clone(), state.clone()));
        }
        fn open_order_end(&mut self) {}
    }

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    // Fetch secdef to populate contract cache
    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 756733, symbol: String::new(), sec_type: "STK".into(), exchange: String::new(), currency: String::new(), ..Default::default() }, req_id: 9998, filters: Default::default() }).unwrap();

    let order_id = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: false, ..Default::default() } })).unwrap();

    let join = run_hot_loop(hot_loop);

    // Wait for Submitted
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut submitted = false;
    while Instant::now() < deadline && !submitted {
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::OrderUpdate(u)) if matches!(u.status, OrderStatus::Submitted | OrderStatus::PreSubmitted) => { submitted = true; }
            _ => {}
        }
    }

    if !submitted {
        let conns = shutdown_and_reclaim(&control_tx, join, account_id);
        skipped!("  SKIP: Order never submitted\n");
        return conns;
    }

    // Wait a bit for CCP exec report to populate the enriched cache
    std::thread::sleep(Duration::from_millis(500));

    // Exercise req_all_open_orders path
    let mut wrapper = OoWrapper { orders: Vec::new() };
    for (oid, info) in shared.orders.drain_open_orders() {
        if !matches!(info.order_state.status.as_str(), "Filled" | "Cancelled" | "Inactive") {
            wrapper.open_order(oid as i64, &info.contract, &info.order, &info.order_state);
        }
    }

    // Cancel the order
    control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id })).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::OrderUpdate(u)) if matches!(u.status, OrderStatus::Cancelled | OrderStatus::Rejected) => break,
            _ => {}
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    // Validate — GT: contract has conId/symbol/secType/currency, order has action/qty/type/tif/account/lmtPrice
    let mut pass = true;
    if let Some((oid, c, o, s)) = wrapper.orders.first() {
        println!("  open_order callback received (orderId={oid}):");
        println!("    contract.conId      = {} (GT: 756733)", c.con_id);
        println!("    contract.symbol     = '{}' (GT: 'SPY')", c.symbol);
        println!("    contract.secType    = '{}' (GT: 'STK')", c.sec_type);
        println!("    contract.currency   = '{}' (GT: 'USD')", c.currency);
        println!("    contract.exchange   = '{}' (GT: 'SMART')", c.exchange);
        println!("    contract.localSym   = '{}' (GT: 'SPY')", c.local_symbol);
        println!("    contract.tradClass  = '{}' (GT: 'SPY')", c.trading_class);
        println!("    order.action        = '{}' (GT: 'BUY')", o.action);
        println!("    order.totalQuantity = {} (GT: 1.0)", o.total_quantity);
        println!("    order.orderType     = '{}' (GT: 'LMT')", o.order_type);
        println!("    order.tif           = '{}' (GT: 'GTC')", o.tif);
        println!("    order.account       = '{}'", super::common::redacted(&o.account));
        println!("    order.lmtPrice      = {} (GT: 1.0)", o.lmt_price);
        println!("    orderState.status   = '{}' (GT: 'Submitted')", s.status);

        if c.con_id != 756733 { println!("    FAIL: conId"); pass = false; }
        if c.symbol != "SPY" { println!("    FAIL: symbol"); pass = false; }
        if c.sec_type != "STK" { println!("    FAIL: secType"); pass = false; }
        if c.currency != "USD" { println!("    FAIL: currency"); pass = false; }
        if c.local_symbol.is_empty() { println!("    FAIL: localSymbol empty"); pass = false; }
        if c.trading_class.is_empty() { println!("    FAIL: tradingClass empty"); pass = false; }
        if o.action != "BUY" { println!("    FAIL: action"); pass = false; }
        if (o.total_quantity - 1.0).abs() > 0.01 { println!("    FAIL: totalQuantity"); pass = false; }
        if o.order_type != "LMT" { println!("    FAIL: orderType"); pass = false; }
        if o.tif != "GTC" { println!("    FAIL: tif"); pass = false; }
        if o.account.is_empty() { println!("    FAIL: account empty"); pass = false; }
        if !matches!(s.status.as_str(), "Submitted" | "PreSubmitted") {
            println!("    FAIL: status should be Submitted/PreSubmitted"); pass = false;
        }
    } else {
        println!("  No open_order callback received");
        pass = false;
    }

    if pass { println!("  PASS (all fields match GT)\n"); }
    else { println!("  FAIL\n"); }
    assert!(pass, "open_order Wrapper output did not match GT");
    conns
}

/// Phase: req_positions — verify position callback delivers enriched Contract
/// with symbol/secType/currency from the contract cache.
pub(super) fn phase_enriched_positions(conns: Conns) -> Conns {
    phase!("--- Phase 132: Enriched position Wrapper Output (req_positions) ---");

    struct PosWrapper {
        positions: Vec<(String, api::Contract, f64, f64)>,
    }
    impl Wrapper for PosWrapper {
        fn position(&mut self, account: &str, contract: &api::Contract, pos: f64, avg_cost: f64) {
            self.positions.push((account.to_string(), contract.clone(), pos, avg_cost));
        }
        fn position_end(&mut self) {}
    }

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    let join = run_hot_loop(hot_loop);

    // Wait for position updates to arrive (UP messages from CCP)
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got_pos = false;
    while Instant::now() < deadline {
        match event_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::PositionUpdate { con_id, position, .. }) if con_id == 756733 => {
                println!("  PositionUpdate: con_id={con_id} position={position}");
                got_pos = true;
                break;
            }
            _ => {}
        }
    }

    // Give time for contract cache to be populated from exec reports
    std::thread::sleep(Duration::from_millis(500));

    // Exercise req_positions path
    let mut wrapper = PosWrapper { positions: Vec::new() };
    for pi in shared.portfolio.position_infos() {
        // The same construction `req_positions` uses: the definition where one
        // has been fetched, and the holding's own fields where it has not. The
        // feed names the contract beside the quantity, so a definition that has
        // not landed does not leave the holding unnamed.
        let c = shared.reference.get_contract(pi.con_id).unwrap_or_else(|| api::Contract {
            con_id: pi.con_id,
            symbol: pi.symbol.clone(),
            sec_type: pi.sec_type.clone(),
            currency: pi.currency.clone(),
            multiplier: pi.multiplier.clone(),
            ..Default::default()
        });
        let avg_cost = pi.avg_cost as f64 / PRICE_SCALE as f64;
        wrapper.position(&account_id, &c, pi.position, avg_cost);
    }

    let gt_account = account_id.clone();
    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if !got_pos && wrapper.positions.is_empty() {
        skipped!("  SKIP: No positions in account (need prior fills)\n");
        return conns;
    }

    // Validate — GT: contract has conId=756733, symbol=SPY, secType=STK
    let mut pass = true;
    if let Some((acct, c, pos, avg_cost)) = wrapper.positions.iter().find(|(_, c, _, _)| c.con_id == 756733) {
        println!("  position callback received:");
        println!("    account             = '{acct}' (GT: '{gt_account}')");
        println!("    contract.conId      = {} (GT: 756733)", c.con_id);
        println!("    contract.symbol     = '{}' (GT: 'SPY')", c.symbol);
        println!("    contract.secType    = '{}' (GT: 'STK')", c.sec_type);
        println!("    position            = {pos} (GT: dynamic)");
        println!("    avgCost             = {avg_cost:.3} (GT: dynamic)");

        if c.con_id != 756733 { println!("    FAIL: conId"); pass = false; }
        if c.symbol != "SPY" { println!("    FAIL: symbol"); pass = false; }
        if c.sec_type != "STK" { println!("    FAIL: secType"); pass = false; }
    } else {
        // May not have SPY position — check any position has enriched contract
        if let Some((acct, c, pos, avg_cost)) = wrapper.positions.first() {
            println!("  position callback (no SPY, checking first):");
            println!("    account             = '{acct}'");
            println!("    contract.conId      = {}", c.con_id);
            println!("    contract.symbol     = '{}'", c.symbol);
            println!("    contract.secType    = '{}'", c.sec_type);
            println!("    position            = {pos}");
            println!("    avgCost             = {avg_cost:.3}");
            if c.con_id == 0 { println!("    FAIL: conId is 0"); pass = false; }
        } else {
            println!("  No position callbacks at all");
            pass = false;
        }
    }

    if pass { println!("  PASS\n"); }
    else { println!("  FAIL\n"); }
    assert!(pass, "position Wrapper output did not match GT");
    conns
}

/// Phase: exec_details — submit a market order (fills immediately), verify exec_details
/// callback has enriched Contract with conId/symbol/secType.
pub(super) fn phase_enriched_exec_details(conns: Conns) -> Conns {
    phase!("--- Phase 133: Enriched exec_details Wrapper Output (market order → fill) ---");

    struct ExecWrapper {
        execs: Vec<(i64, api::Contract, api::Execution)>,
    }
    impl Wrapper for ExecWrapper {
        fn exec_details(&mut self, req_id: i64, contract: &api::Contract, execution: &api::Execution) {
            self.execs.push((req_id, contract.clone(), execution.clone()));
        }
        fn order_status(&mut self, _: i64, _: &str, _: f64, _: f64, _: f64, _: i64, _: i64, _: f64, _: i64, _: &str, _: f64) {}
        fn open_order(&mut self, _: i64, _: &api::Contract, _: &api::Order, _: &api::OrderState) {}
    }

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    // Fetch secdef to populate contract cache
    control_tx.send(ControlCommand::FetchContractDetails { contract: ibx::types::ContractRef { con_id: 756733, symbol: String::new(), sec_type: "STK".into(), exchange: String::new(), currency: String::new(), ..Default::default() }, req_id: 9997, filters: Default::default() }).unwrap();

    let order_id = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Market, tif: b'0', attrs: OrderAttrs::default() })).unwrap();

    let join = run_hot_loop(hot_loop);

    // Wait for fill
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut filled = false;
    while Instant::now() < deadline {
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Fill(f)) if f.order_id == order_id => {
                println!("  Fill received: qty={} price={:.2}", ibx::types::qty_to_f64(f.qty), f.price as f64 / PRICE_SCALE as f64);
                filled = true;
                break;
            }
            Ok(Event::OrderUpdate(u)) if u.status == OrderStatus::Rejected => {
                let conns = shutdown_and_reclaim(&control_tx, join, account_id);
                skipped!("  SKIP: Order rejected — {}\n", reject_reason(&shared, u.order_id));
                return conns;
            }
            _ => {}
        }
    }

    if !filled {
        let conns = shutdown_and_reclaim(&control_tx, join, account_id);
        no_market(&shared, "no fill arrived");
        return conns;
    }

    std::thread::sleep(Duration::from_millis(300));

    // Exercise exec_details path (same as process_msgs)
    let mut wrapper = ExecWrapper { execs: Vec::new() };
    for fill in shared.orders.drain_fills() {
        let price_f = fill.price as f64 / api::PRICE_SCALE_F;
        let side_str = match fill.side {
            Side::Buy => "BOT",
            Side::Sell | Side::ShortSell => "SLD",
        };
        let exec = api::Execution {
            side: side_str.into(),
            shares: ibx::types::qty_to_f64(fill.qty),
            price: price_f,
            order_id: fill.order_id as i64,
            ..Default::default()
        };
        let c = shared.orders.get_order_info(fill.order_id)
            .map(|info| info.contract)
            .unwrap_or_default();
        wrapper.exec_details(-1, &c, &exec);
    }

    // Sell back to flatten
    let sell_id = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id: sell_id, instrument: inst_id, side: Side::Sell, qty: ibx::types::QTY_SCALE, kind: OrderKind::Market, tif: b'0', attrs: OrderAttrs::default() })).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Fill(f)) if f.order_id == sell_id => break,
            _ => {}
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    // Validate
    let mut pass = true;
    if let Some((_, c, exec)) = wrapper.execs.first() {
        println!("  exec_details callback received:");
        println!("    contract.conId      = {} (GT: 756733)", c.con_id);
        println!("    contract.symbol     = '{}' (GT: 'SPY')", c.symbol);
        println!("    contract.secType    = '{}' (GT: 'STK')", c.sec_type);
        println!("    contract.currency   = '{}' (GT: 'USD')", c.currency);
        println!("    contract.exchange   = '{}' (GT: 'ARCA')", c.exchange);
        println!("    contract.localSym   = '{}' (GT: 'SPY')", c.local_symbol);
        println!("    contract.tradClass  = '{}' (GT: 'SPY')", c.trading_class);
        println!("    execution.side      = '{}' (GT: 'BOT')", exec.side);
        println!("    execution.shares    = {} (GT: 1.0)", exec.shares);
        println!("    execution.price     = {:.2}", exec.price);

        if c.con_id != 756733 { println!("    FAIL: conId"); pass = false; }
        if c.symbol != "SPY" { println!("    FAIL: symbol"); pass = false; }
        if c.sec_type != "STK" { println!("    FAIL: secType"); pass = false; }
        if c.currency != "USD" { println!("    FAIL: currency"); pass = false; }
        if c.local_symbol.is_empty() { println!("    FAIL: localSymbol empty"); pass = false; }
        if exec.side != "BOT" { println!("    FAIL: side"); pass = false; }
        if (exec.shares - 1.0).abs() > 0.01 { println!("    FAIL: shares"); pass = false; }
    } else {
        println!("  No exec_details callback");
        pass = false;
    }

    if pass { println!("  PASS (all fields match GT)\n"); }
    else { println!("  FAIL\n"); }
    assert!(pass, "exec_details Wrapper output did not match GT");
    conns
}

/// Phase: PnL Subscription Lifecycle — verify daily/unrealized/realized PnL populated.
pub(super) fn phase_pnl_subscription(conns: Conns) -> Conns {
    phase!("--- Phase 121: PnL Subscription (verify all 3 PnL fields) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (mut hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );
    let inst_id = hot_loop.context_mut().register_instrument(756733);
    hot_loop.context_mut().set_symbol(inst_id, "SPY".to_string());
    // A US stock routed smart. Registered by id alone it states no
    // security type, and the venue answers an order carrying an empty
    // tag 167 with "Unsupported type".
    hot_loop.context_mut().set_routing(inst_id, "STK", "SMART");

    // Submit a far-from-market order to trigger account updates
    let order_id = next_order_id();
    control_tx.send(ControlCommand::Order(OrderRequest::SubmitEx { order_id, instrument: inst_id, side: Side::Buy, qty: ibx::types::QTY_SCALE, kind: OrderKind::Limit { price: 1_00_000_000 }, tif: b'1', attrs: OrderAttrs { outside_rth: true, ..Default::default() } })).unwrap();

    let join = run_hot_loop(hot_loop);

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut pnl_checked = false;
    let mut order_submitted = false;

    while Instant::now() < deadline {
        match event_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::OrderUpdate(update)) => {
                if matches!(update.status, OrderStatus::Submitted | OrderStatus::PreSubmitted) {
                    order_submitted = true;
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }

        // Account values are pushed, not answered, so nothing says which event
        // arrives first or whether any arrives at all. Reading them only inside
        // a tick meant a phase that subscribes to no market data never looked:
        // the figures were there and went unread, which is indistinguishable
        // from the venue never sending them.
        if !pnl_checked {
            let acct = shared.portfolio.account();
            // The profit and loss figures are legitimately zero for an account
            // holding nothing. Net liquidation is what says the values arrived.
            if acct.net_liquidation > 0 {
                println!("  DailyPnL:      {:.2}", acct.daily_pnl as f64 / PRICE_SCALE as f64);
                println!("  UnrealizedPnL: {:.2}", acct.unrealized_pnl as f64 / PRICE_SCALE as f64);
                println!("  RealizedPnL:   {:.2}", acct.realized_pnl as f64 / PRICE_SCALE as f64);
                pnl_checked = true;
                control_tx.send(ControlCommand::Order(OrderRequest::Cancel { order_id })).unwrap();
            }
        }
        if pnl_checked && order_submitted { break; }
    }

    // Wait for cancel to complete
    let cancel_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < cancel_deadline {
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::OrderUpdate(u)) if matches!(u.status, OrderStatus::Cancelled | OrderStatus::Rejected) => break,
            _ => {}
        }
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    if pnl_checked {
        // PnL fields populated (even if 0 — that's valid for no-position accounts)
        println!("  PASS\n");
    } else {
        session_owed(&shared, "no profit and loss figures arrived");
    }
    conns
}

/// Phase: explicit whole-account P&L subscribe/cancel lifecycle (CCP 6040=142 / cancel).
///
/// Distinct from `phase_pnl_subscription`, which only reads account state after an
/// order. This exercises the `SubscribePnl`/`CancelPnl` ControlCommands directly.
/// Session-independent: P&L is account data, available whether or not the market is open.
pub(super) fn phase_pnl_subscribe_command(conns: Conns) -> Conns {
    phase!("--- Phase 134: PnL Subscribe/Cancel Command (CCP 6040=142) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, _event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    let req_id: i64 = 6142;
    // .unwrap() doubles as a liveness check: a closed channel means the hot loop died.
    control_tx
        .send(ControlCommand::SubscribePnl { req_id, account: account_id.clone() })
        .unwrap();
    let join = run_hot_loop(hot_loop);

    // The server pushes 6040=143 midnight seeds — one repeating group per open
    // position. A flat paper account yields no seeds, which is a valid outcome, so
    // a bounded window is waited and whatever arrived is logged, rather than requiring seeds.
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut seeds_seen = 0usize;
    while Instant::now() < deadline {
        let seeds = shared.portfolio.midnight_seeds();
        if !seeds.is_empty() {
            seeds_seen = seeds.len();
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    control_tx.send(ControlCommand::CancelPnl { req_id }).unwrap();
    std::thread::sleep(Duration::from_millis(500));

    // Read before the engine is stopped: stopping it sets the same flag a lost
    // connection does. A malformed subscribe drops the session, so this phase
    // asserts on it rather than leaving it to the phases that follow.
    let survived = !lost_unasked(&shared);

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    println!("  midnight seeds received: {seeds_seen}");
    assert!(
        survived,
        "the session went away across the subscribe and the withdrawal, which is \
         what the venue does with a message it cannot read",
    );
    println!("  PASS\n");
    conns
}

/// Phase: News Bulletins — drain news bulletins from SharedState.
pub(super) fn phase_news_bulletins(conns: Conns) -> Conns {
    phase!("--- Phase 122: News Bulletins (drain from SharedState) ---");

    let account_id = conns.account_id;
    let shared = Arc::new(SharedState::new());
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
    let (hot_loop, control_tx) = HotLoop::with_connections(
        shared.clone(), Some(ibx::engine::hot_loop::EventSink::new(event_tx, Default::default())), account_id.clone(),
        conns.farm, conns.ccp, conns.hmds, None,
    );

    control_tx.send(ControlCommand::Subscribe { contract: ibx::types::ContractRef { con_id: 756733, symbol: "SPY".into(), exchange: String::new(), sec_type: "STK".into(), currency: String::new(), last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new() }, mode_9887: 0, reply_tx: None }).unwrap();
    let join = run_hot_loop(hot_loop);

    // Wait for any events to flow, checking for bulletins periodically
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut total_bulletins = 0usize;

    while Instant::now() < deadline {
        // Whatever the wait returns, drain: a bulletin is published to the
        // store, not to this channel, so the wait is only the interval.
        let _ = event_rx.recv_timeout(Duration::from_millis(500));
        let bulletins = shared.market.drain_news_bulletins();
        for b in &bulletins {
            println!("  Bulletin: id={} type={} exchange={} msg={}",
                b.msg_id, b.msg_type, b.exchange,
                &b.message[..std::cmp::min(80, b.message.len())]);
        }
        total_bulletins += bulletins.len();
    }

    let conns = shutdown_and_reclaim(&control_tx, join, account_id);

    // News bulletins are sporadic; none may arrive in the window.
    // The test validates the drain mechanism works without panicking.
    println!("  Total bulletins received: {total_bulletins}");
    if total_bulletins > 0 {
        println!("  PASS (received {total_bulletins} bulletins)\n");
    } else {
        println!("  PASS (no bulletins during test window — drain mechanism verified)\n");
    }
    conns
}
