//! Event dispatch: drains SharedState queues and fires Python wrapper callbacks.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pyo3::prelude::*;

use crate::bridge::{Event, SharedState};
use crate::client_core::order_status_str;
use crate::types::*;

use crate::api::types::{
    Execution as ApiExecution,
    CommissionAndFeesReport as ApiCommissionAndFeesReport,
};
use super::EClient;
use super::super::contract::{Contract, ContractDescription, ContractDetails, BarData, CommissionAndFeesReport, DepthMktDataDescriptionPy, Execution, Order, OrderState};
use super::super::tick_types::*;
use super::super::super::types::PRICE_SCALE_F;

/// Call a Python wrapper method, catching and logging an ordinary exception instead of
/// propagating it so one bad callback cannot kill the dispatch loop. `KeyboardInterrupt`,
/// `SystemExit`, and any other exception deriving from `BaseException` rather than
/// `Exception` are re-raised so Ctrl-C during a callback still stops `run()` and a
/// callback-raised `SystemExit` still terminates it, matching ibapi.
/// Fire a callback on the caller's wrapper.
///
/// Routed through the dispatcher that also tries the name the reference client
/// gives the callback: a wrapper written against that client defines those
/// names, and a call made only under this client's names lands on the base
/// class's do-nothing default instead of on the caller's code.
macro_rules! call_wrapper {
    ($wrapper:expr, $py:expr, $method:expr, $args:expr) => {
        if let Err(e) = crate::python::compat::client::call_named($py, &$wrapper, $method, $args) {
            if !e.is_instance_of::<pyo3::exceptions::PyException>($py) {
                return Err(e);
            }
            log::error!("Python callback {}() raised: {}", $method, e);
        }
    };
}

impl EClient {
    /// Single iteration of event dispatch: drain all shared queues and fire Python callbacks.
    pub(crate) fn dispatch_once(&self, py: Python<'_>, shared: &Arc<SharedState>) -> PyResult<()> {
        // Drain engine events — surface disconnects as error callbacks.
        //
        // Collect under a short lock and dispatch after releasing it. Binding
        // `rx` from the guard would hold the mutex across the 1100 callback,
        // and a handler answering connectivity loss with disconnect() locks the
        // same mutex — non-reentrant, GIL held, so the interpreter freezes
        // rather than one call failing (, same shape as).
        let events: Vec<Event> = {
            let guard = self.event_rx.lock().unwrap();
            guard.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default()
        };
        // One batch is one session — the channel is replaced on reconnect — so
        // several `Disconnected` events in it are one loss, and a network cut
        // that takes both the farm and CCP down emits more than one. Firing per
        // event meant a handler that reconnected on the first would then take a
        // stale second 1100 into the new session, marking it disconnected.
        if events.iter().any(|e| matches!(e, Event::Disconnected)) {
            // Mark disconnected BEFORE the callback. A handler that answers
            // 1100 with disconnect() then connect() establishes a new session
            // and sets connected=true; storing false afterwards would clobber
            // the new session's state.
            self.connected.store(false, Ordering::Release);
            call_wrapper!(self.wrapper, py, "error", (-1i64, 1100i64, "Connectivity between client and server has been lost", ""));
        }
        // A session the caller ended is not a session that was lost, and is
        // not announced here: the dispatch loop ends and answers with
        // `connection_closed`, which is what the reference client answers
        // `disconnect()` with. Reported as 1100 as well, a program that stands
        // down on connectivity loss stood down on the session it had closed.
        if events.iter().any(|e| matches!(e, Event::Stopped)) {
            self.connected.store(false, Ordering::Release);
        }
        // 1102 rather than 1101: the reconnect re-establishes the
        // subscriptions itself, so the caller has nothing to re-request. A
        // client that stood down on 1100 and never saw this stayed down.
        if events.iter().any(|e| matches!(e, Event::Reconnected)) {
            self.connected.store(true, Ordering::Release);
            call_wrapper!(self.wrapper, py, "error", (-1i64, 1102i64, "Connectivity between client and server has been restored - data maintained", ""));
        }

        // Drain fills -> execDetails + orderStatus
        let fills = shared.orders.drain_fills();
        for fill in fills {
            let req_id = self.core.instrument_to_req.lock().unwrap()
                .get(&fill.instrument).copied().unwrap_or(-1);
            let side_str = match fill.side {
                Side::Buy => "BUY",
                Side::Sell => "SELL",
                Side::ShortSell => "SSHORT",
            };
            let price = fill.price as f64 / PRICE_SCALE_F;
            let commission = fill.commission as f64 / PRICE_SCALE_F;

            let status = if fill.remaining == 0 { "Filled" } else { "PartiallyFilled" };
            // A fill emits its own order_status and never reaches the branch
            // below, so the client's record has to be preferred here too.
            let (perm_id, engine_parent) = shared.orders.get_order_info(fill.order_id)
                .map(|info| (info.order.perm_id, info.order.parent_id))
                .unwrap_or((0, 0));
            let parent_id = self.core.tracked_parent_id(fill.order_id)
                .unwrap_or(engine_parent);
            // `filled` and `avgFillPrice` describe the order so far;
            // `lastFillPrice` describes this print.
            let avg_price = fill.avg_price as f64 / PRICE_SCALE_F;
            call_wrapper!(self.wrapper, py, "order_status", (fill.order_id as i64, status, fill.cum_qty as f64, fill.remaining as f64,
                 avg_price, perm_id, parent_id, price, 0i64, "", 0.0f64));

            // Track execution for req_executions.
            //
            // The venue states the execution's own id and the time it
            // happened, and both are held against the order. Neither is
            // composed here: an id built from an order number and a counter is
            // not the venue's, and the id is what a fill is reconciled against
            // a broker's own record by.
            let rich_info = shared.orders.get_order_info(fill.order_id);
            let exec_id = rich_info
                .as_ref()
                .map(|i| i.last_exec.exec_id.clone())
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| format!("{}.{}", fill.order_id, fill.timestamp_ns));
            let now_str = rich_info
                .as_ref()
                .map(|i| i.last_exec.time.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| format!("{}", fill.timestamp_ns));
            let exec_exchange = rich_info.as_ref()
                .map(|i| i.last_exec.exchange.as_str()).unwrap_or("").to_string();
            let cum_qty = rich_info.as_ref()
                .map(|i| i.last_exec.cum_qty).unwrap_or(fill.qty as f64);
            let avg_price = rich_info.as_ref()
                .map(|i| i.last_exec.avg_price).unwrap_or(price);
            // Build api-level contract for shared storage
            let api_contract = self.core.open_orders.lock().unwrap()
                .get(&fill.order_id).map(|o| o.contract.clone())
                .or_else(|| {
                    rich_info.map(|info| info.contract)
                })
                .unwrap_or_default();

            let api_exec = ApiExecution {
                exec_id: exec_id.clone(),
                time: now_str.clone(),
                acct_number: self.account(),
                exchange: exec_exchange.clone(),
                side: side_str.to_string(),
                shares: fill.qty as f64,
                price,
                order_id: fill.order_id as i64,
                cum_qty,
                avg_price,
                ..Default::default()
            };
            let api_commission = ApiCommissionAndFeesReport {
                exec_id: exec_id.clone(),
                commission_and_fees: commission,
                // As stated on the execution rather than assumed. See the
                // matching note on the other surface.
                currency: api_contract.currency.clone(),
                realized_pnl: f64::MAX,
                yield_amount: f64::MAX,
                yield_redemption_date: String::new(),
            };

            // Build Python contract for callback
            let exec_contract = Contract::from_api(&api_contract);

            // Store for req_executions replay via shared core
            self.core.push_execution(req_id, api_contract, api_exec, api_commission);

            let acct_name = self.account();
            let c_py = Py::new(py, exec_contract)?.into_any();
            let exec_obj = Execution {
                exec_id: exec_id.clone(),
                time: now_str.clone(),
                acct_number: acct_name,
                exchange: exec_exchange.clone(),
                side: side_str.to_string(),
                shares: fill.qty as f64,
                price,
                perm_id,
                client_id: self.client_id.load(Ordering::Acquire) as i64,
                order_id: fill.order_id as i64,
                liquidation: 0,
                cum_qty,
                avg_price,
                last_liquidity: 0,
                pending_price_revision: false,
                ..Default::default()
            };
            let exec_py = Py::new(py, exec_obj)?.into_any();
            call_wrapper!(self.wrapper, py, "exec_details", (req_id, &c_py, &exec_py));

            // Update open order tracking
            self.core.update_order_fill(fill.order_id, status, fill.cum_qty as f64, fill.remaining as f64);

            // Dispatch commission_and_fees_report
            let report = CommissionAndFeesReport {
                exec_id,
                commission_and_fees: commission,
                currency: "USD".to_string(),
                realized_pnl: f64::MAX,
                yield_amount: f64::MAX,
                yield_redemption_date: String::new(),
            };
            let report_py = Py::new(py, report)?.into_any();
            call_wrapper!(self.wrapper, py, "commission_and_fees_report", (&report_py,));
        }

        // Drain order updates -> orderStatus
        let updates = shared.orders.drain_order_updates();
        for update in updates {
            let status = order_status_str(update.status);
            // The engine reads no parent from the report, but this client
            // placed the order and was told. Prefer what it recorded; an order
            // it did not place keeps the engine's answer of none.
            let parent_id = self.core.tracked_parent_id(update.order_id)
                .unwrap_or(update.parent_id);
            let avg = update.avg_price as f64 / crate::types::PRICE_SCALE as f64;

            // The order as this client sent it, beside the status it is now
            // in. The reference client answers an order's every change with
            // both, from the order it holds — its own method for it sends the
            // pair — and a program that waits for the order to confirm what it
            // asked for waited on a callback that only arrived if it asked for
            // its open orders.
            // Copied out before the callback rather than read across it: a
            // guard built in the scrutinee is held for the whole body, and the
            // body calls user code. A wrapper that reaches the order cache from
            // that callback would wait on a lock its own caller holds, with the
            // GIL held behind it.
            let tracked = self.core.open_orders.lock().unwrap().get(&update.order_id).cloned();
            if let Some(tracked) = tracked {
                let contract_py = Py::new(py, Contract::from_api(&tracked.contract))?.into_any();
                let order_py = Py::new(py, Order {
                    // The number this session connected under. The reference
                    // client keys a trade by it with the order id, so an order
                    // reported under a client that did not place it is a second
                    // trade the caller never sees updated.
                    client_id: self.client_id.load(Ordering::Acquire),
                    order_id: tracked.order.order_id,
                    action: tracked.order.action.clone(),
                    total_quantity: tracked.order.total_quantity,
                    order_type: tracked.order.order_type.clone(),
                    lmt_price: tracked.order.lmt_price,
                    aux_price: tracked.order.aux_price,
                    tif: tracked.order.tif.clone(),
                    account: tracked.order.account.clone(),
                    perm_id: tracked.order.perm_id,
                    oca_type: tracked.order.oca_type,
                    use_price_mgmt_algo: tracked.order.use_price_mgmt_algo,
                    trail_stop_price: tracked.order.trail_stop_price,
                    algo_strategy: tracked.order.algo_strategy.clone(),
                    ..Default::default()
                })?.into_any();
                let state_py = Py::new(py, OrderState {
                    status: status.to_string(),
                    ..Default::default()
                })?.into_any();
                call_wrapper!(self.wrapper, py, "open_order",
                    (update.order_id as i64, &contract_py, &order_py, &state_py));
            }

            call_wrapper!(self.wrapper, py, "order_status", (update.order_id as i64, status, update.filled_qty,
                 update.remaining_qty, avg, update.perm_id, parent_id, 0.0f64,
                 self.client_id.load(Ordering::Acquire) as i64, "", 0.0f64));

            // Track open orders
            self.core.update_order_status(shared, update.order_id, update.status, update.filled_qty, update.remaining_qty);
        }

        for event in self.core.drain_group_events() {
            match event {
                crate::client_core::GroupEvent::List(req_id, groups) => {
                    call_wrapper!(self.wrapper, py, "display_group_list", (req_id, groups));
                }
                crate::client_core::GroupEvent::Updated(req_id, info) => {
                    call_wrapper!(self.wrapper, py, "display_group_updated", (req_id, info));
                }
            }
        }

        for text in shared.market.drain_venue_errors() {
            call_wrapper!(self.wrapper, py, "error", (-1i64, 321i64, text, ""));
        }

        for (instrument, reason) in shared.market.drain_subscription_failures() {
            let req_id = self.core.req_id_for_instrument(instrument);
            call_wrapper!(self.wrapper, py, "error", (req_id, 200i64, reason, ""));
        }

        for comp in shared.market.drain_option_computations() {
            let req_id = comp.answers
                .unwrap_or_else(|| self.core.req_id_for_instrument(comp.instrument));
            call_wrapper!(self.wrapper, py, "tick_option_computation",
                (req_id, 13i32, 0i32, comp.implied_vol, comp.delta, comp.opt_price,
                 comp.pv_dividend, comp.gamma, comp.vega, comp.theta, comp.und_price));
        }

        // Drain cancel rejects -> error
        let rejects = shared.orders.drain_cancel_rejects();
        for reject in rejects {
            let code = if reject.reject_type == 1 { 202i64 } else { 10147i64 };
            let msg = format!("Order {} cancel/modify rejected (reason: {})", reject.order_id, reject.reason_code);
            // Reason 1 is UnknownOrder: the gateway has said the order does not
            // exist, and the engine has already retired its record. The client's
            // own record has to go with it, or the open-order snapshot keeps
            // reporting the order the rejection was about.
            if reject.reason_code == 1 {
                self.core.untrack_order(reject.order_id);
            }
            call_wrapper!(self.wrapper, py, "error", (reject.order_id as i64, code, msg.as_str(), ""));
        }

        // Drain inactive-order reasons -> error
        for (order_id, code, msg) in shared.orders.drain_order_inactive() {
            call_wrapper!(self.wrapper, py, "error", (order_id as i64, code as i64, msg.as_str(), ""));
        }

        // Poll quotes for changes -> tickPrice/tickSize
        // Poll quotes via shared ClientCore (same logic as Rust dispatch)
        let instruments = self.core.snapshot_instruments();
        let mut snapshot_done: Vec<i64> = Vec::new();
        for (iid, req_id) in instruments {
            let result = self.core.poll_instrument_ticks(shared, iid, req_id);
            // The same quote, once per caller watching this contract. One
            // contract holds one subscription on the wire, and everyone who
            // asked for it hears it under their own request.
            let watchers = self.core.followers_of(iid);

            // Fire market_data_type once per subscription on first tick delivery
            if let Some(mdt) = self.core.check_mdt_needed(req_id, result.delivered) {
                call_wrapper!(self.wrapper, py, "market_data_type", (req_id, mdt));
            }

            let attrib = TickAttrib::default();
            let attrib_obj = Py::new(py, attrib)?.into_any();
            for tick in &result.ticks {
                for id in std::iter::once(tick.req_id).chain(watchers.iter().copied()) {
                    if let Some(mdt) = self.core.check_mdt_needed(id, result.delivered) {
                        call_wrapper!(self.wrapper, py, "market_data_type", (id, mdt));
                    }
                    if tick.is_price {
                        call_wrapper!(self.wrapper, py, "tick_price", (id, tick.tick_type, tick.value, &attrib_obj));
                    } else {
                        call_wrapper!(self.wrapper, py, "tick_size", (id, tick.tick_type, tick.value));
                    }
                }
            }
            for st in &result.string_ticks {
                for id in std::iter::once(st.req_id).chain(watchers.iter().copied()) {
                    call_wrapper!(self.wrapper, py, "tick_string", (id, st.tick_type, st.value.as_str()));
                }
            }
            if let Some(ts) = &result.timestamp {
                let ts_secs = ts.timestamp_ns / 1_000_000_000;
                call_wrapper!(self.wrapper, py, "tick_string", (ts.req_id, TICK_LAST_TIMESTAMP, ts_secs.to_string().as_str()));
            }
            if self.core.check_snapshot_done(
                req_id, result.delivered,
                crate::client_core::ClientCore::is_quoted(&shared.market.quote(iid)),
            ) {
                call_wrapper!(self.wrapper, py, "tick_snapshot_end", (req_id,));
                snapshot_done.push(req_id);
            }
        }
        for req_id in snapshot_done {
            self.cancel_mkt_data(py, req_id)?;
        }

        // Drain TBT trades -> tickByTickAllLast
        let tbt_trades = shared.market.drain_tbt_trades();
        for trade in tbt_trades {
            // As the caller numbered it, from the record itself: a contract
            // can carry several streams and the contract alone does not say
            // which one this came from.
            let req_id = trade.req_id;
            let price = trade.price as f64 / PRICE_SCALE_F;
            let size = trade.size as f64 / crate::types::QTY_SCALE as f64;
            // What the venue said about this print, not what a default says.
            let attrib = super::super::tick_types::TickAttribLast {
                past_limit: trade.past_limit,
                unreported: trade.unreported,
            };
            let attrib_obj = Py::new(py, attrib)?.into_any();
            call_wrapper!(self.wrapper, py, "tick_by_tick_all_last", (req_id, 1i32, trade.timestamp as i64, price, size,
                 &attrib_obj, trade.exchange.as_str(), trade.conditions.as_str()));
        }

        // Drain TBT quotes -> tickByTickBidAsk
        let tbt_quotes = shared.market.drain_tbt_quotes();
        for quote in tbt_quotes {
            let req_id = quote.req_id;
            let attrib = super::super::tick_types::TickAttribBidAsk {
                bid_past_low: quote.bid_past_low,
                ask_past_high: quote.ask_past_high,
            };
            let attrib_obj = Py::new(py, attrib)?.into_any();
            call_wrapper!(self.wrapper, py, "tick_by_tick_bid_ask", (req_id, quote.timestamp as i64,
                 quote.bid as f64 / PRICE_SCALE_F, quote.ask as f64 / PRICE_SCALE_F,
                 quote.bid_size as f64 / crate::types::QTY_SCALE as f64,
                 quote.ask_size as f64 / crate::types::QTY_SCALE as f64, &attrib_obj));
        }

        // Drain depth updates -> updateMktDepth / updateMktDepthL2
        let depth_updates = shared.market.drain_depth_updates();
        if !depth_updates.is_empty() {
            log::debug!("delivering {} book level(s)", depth_updates.len());
        }
        for du in depth_updates {
            if du.market_maker.is_empty() {
                call_wrapper!(self.wrapper, py, "update_mkt_depth", (du.req_id as i64, du.position, du.operation, du.side, du.price, du.size));
            } else {
                call_wrapper!(self.wrapper, py, "update_mkt_depth_l2", (du.req_id as i64, du.position, du.market_maker.as_str(),
                     du.operation, du.side, du.price, du.size, du.is_smart_depth));
            }
        }

        // Drain news -> tickNews
        let news_items = shared.market.drain_tick_news();
        for news in news_items {
            let req_id = self.core.req_id_for_instrument(news.instrument);
            call_wrapper!(self.wrapper, py, "tick_news", (req_id, news.timestamp as i64, news.provider_code.as_str(),
                 news.article_id.as_str(), news.headline.as_str(), ""));
        }

        // Drain news bulletins -> updateNewsBulletin
        if self.core.bulletin_subscribed.load(Ordering::Acquire) {
            let bulletins = shared.market.drain_news_bulletins();
            for b in bulletins {
                call_wrapper!(self.wrapper, py, "update_news_bulletin", (b.msg_id as i64, b.msg_type, b.message.as_str(), b.exchange.as_str()));
            }
        }

        // Drain what-if responses -> open_order(contract, order, OrderState) + order_status
        // (iso with official ibapi: server delivers margin via openOrder.orderState)
        let what_ifs = shared.orders.drain_what_if_responses();
        for wi in what_ifs {
            let fmt = |p: Price| format!("{:.2}", p as f64 / PRICE_SCALE_F);
            let state = OrderState {
                status: "PreSubmitted".into(),
                init_margin_before: fmt(wi.init_margin_before),
                maint_margin_before: fmt(wi.maint_margin_before),
                equity_with_loan_before: fmt(wi.equity_with_loan_before),
                init_margin_change: fmt(wi.init_margin_after - wi.init_margin_before),
                maint_margin_change: fmt(wi.maint_margin_after - wi.maint_margin_before),
                equity_with_loan_change: fmt(wi.equity_with_loan_after - wi.equity_with_loan_before),
                init_margin_after: fmt(wi.init_margin_after),
                maint_margin_after: fmt(wi.maint_margin_after),
                equity_with_loan_after: fmt(wi.equity_with_loan_after),
                commission_and_fees: wi.commission as f64 / PRICE_SCALE_F,
                ..Default::default()
            };

            let tracked = self.core.open_orders.lock().unwrap().get(&wi.order_id).cloned();
            let (contract_py, order_py) = if let Some(t) = tracked {
                let c = Contract::from_api(&t.contract);
                let o = Order {
                    order_id: t.order.order_id,
                    action: t.order.action,
                    total_quantity: t.order.total_quantity,
                    order_type: t.order.order_type,
                    lmt_price: t.order.lmt_price,
                    aux_price: t.order.aux_price,
                    tif: t.order.tif,
                    what_if: t.order.what_if,
                    // What the child was given. On the order, which is where
                    // the reference client carries it — a preview is not an
                    // order, so a status naming its parent is a status for an
                    // order that was never placed.
                    parent_id: self.core.tracked_parent_id(wi.order_id).unwrap_or(0),
                    // As on the update path: the reference client keys a trade
                    // by the client that placed it.
                    client_id: self.client_id.load(Ordering::Acquire),
                    ..Default::default()
                };
                (Py::new(py, c)?.into_any(), Py::new(py, o)?.into_any())
            } else {
                (Py::new(py, Contract::default())?.into_any(),
                 Py::new(py, Order::default())?.into_any())
            };
            let state_py = Py::new(py, state)?.into_any();
            // A preview and nothing else. The venue answers what an order
            // would cost on the order itself, and a status besides it is a
            // status for an order that was never placed — which is what the
            // reference client's own wrapper says when it receives one.
            call_wrapper!(self.wrapper, py, "open_order",
                (wi.order_id as i64, &contract_py, &order_py, &state_py));
            self.core.open_orders.lock().unwrap().remove(&wi.order_id);
        }

        // Drain HMDS query errors -> error. Surface gateway-side validation
        // failures (e.g. "Invalid time length") that previously vanished silently.
        for (req_id, code, msg) in shared.reference.drain_historical_errors_for_dispatch() {
            call_wrapper!(self.wrapper, py, "error", (req_id as i64, code as i64, msg.as_str(), ""));
        }

        // Drain historical data -> historicalData + historicalDataEnd / historicalDataUpdate
        let hist_data = shared.reference.drain_historical_data_for_dispatch();
        for (req_id, response) in hist_data {
            let is_update = self.core.hist_initial_complete.lock().unwrap().contains(&req_id);
            for bar in &response.bars {
                let bar_obj = BarData::new(
                    self.core.bar_time_for(req_id as i64, &bar.time),
                    bar.open, bar.high, bar.low, bar.close,
                    bar.volume, bar.wap, bar.count as i32,
                    response.timezone.clone(),
                );
                let bar_py = Py::new(py, bar_obj)?.into_any();
                if is_update {
                    call_wrapper!(self.wrapper, py, "historical_data_update", (req_id as i64, &bar_py));
                } else {
                    call_wrapper!(self.wrapper, py, "historical_data", (req_id as i64, &bar_py));
                }
            }
            if response.is_complete && !is_update {
                self.core.hist_initial_complete.lock().unwrap().insert(req_id);
                call_wrapper!(self.wrapper, py, "historical_data_end", (req_id as i64, "", ""));
            }
        }

        // Drain head timestamps -> headTimestamp
        let head_ts = shared.reference.drain_head_timestamps_for_dispatch();
        for (req_id, response) in head_ts {
            call_wrapper!(self.wrapper, py, "head_timestamp",
                (req_id as i64, response.head_timestamp.as_str()));
        }

        // Drain contract details -> contractDetails + contractDetailsEnd
        let contract_defs = shared.reference.drain_contract_details_for_dispatch();
        for (req_id, def) in contract_defs {
            let details = ContractDetails::from_definition(py, &def);
            let details_py = Py::new(py, details)?.into_any();
            call_wrapper!(self.wrapper, py, "contract_details",
                (req_id as i64, &details_py));
        }
        let contract_ends = shared.reference.drain_contract_details_end_for_dispatch();
        for req_id in contract_ends {
            call_wrapper!(self.wrapper, py, "contract_details_end", (req_id as i64,));
        }

        // The calendar's answers, as the venue wrote them.
        for (req_id, json) in shared.reference.drain_calendar_meta_data_for_dispatch() {
            call_wrapper!(self.wrapper, py, "wsh_meta_data", (req_id as i64, json.as_str()));
        }
        for (req_id, json) in shared.reference.drain_calendar_events_for_dispatch() {
            call_wrapper!(self.wrapper, py, "wsh_event_data", (req_id as i64, json.as_str()));
        }

        // Drain matching symbols -> symbolSamples
        let symbol_results = shared.reference.drain_matching_symbols_for_dispatch();
        for (req_id, matches) in symbol_results {
            let descriptions: Vec<Py<ContractDescription>> = matches.iter().map(|m| {
                Py::new(py, ContractDescription {
                    con_id: m.con_id as i64,
                    symbol: m.symbol.clone(),
                    sec_type: m.sec_type.to_fix().to_string(),
                    currency: m.currency.clone(),
                    primary_exchange: m.primary_exchange.clone(),
                    derivative_sec_types: m.derivative_types.clone(),
                }).unwrap()
            }).collect();
            let list = pyo3::types::PyList::new(py, &descriptions)?;
            call_wrapper!(self.wrapper, py, "symbol_samples", (req_id as i64, list.as_any()));
        }

        // Drain option chains -> securityDefinitionOptionParameter + ...End
        let option_params = shared.reference.drain_option_params_for_dispatch();
        for (req_id, underlying_con_id, scopes) in option_params {
            for scope in &scopes {
                let expirations = pyo3::types::PyList::new(py, &scope.expirations)?;
                let strikes = pyo3::types::PyList::new(py, &scope.strikes)?;
                call_wrapper!(self.wrapper, py, "security_definition_option_parameter",
                    (req_id as i64, scope.exchange.as_str(), underlying_con_id,
                     scope.trading_class.as_str(), scope.multiplier.as_str(),
                     expirations.as_any(), strikes.as_any()));
            }
            call_wrapper!(self.wrapper, py, "security_definition_option_parameter_end", (req_id as i64,));
        }

        // Drain depth exchanges -> mktDepthExchanges
        let depth_exchanges = shared.reference.drain_depth_exchanges();
        if !depth_exchanges.is_empty() {
            let descriptions: Vec<Py<DepthMktDataDescriptionPy>> = depth_exchanges.iter().map(|d| {
                Py::new(py, DepthMktDataDescriptionPy {
                    exchange: d.exchange.clone(),
                    sec_type: d.sec_type.clone(),
                    listing_exch: d.listing_exch.clone(),
                    service_data_type: d.service_data_type.clone(),
                    agg_group: d.agg_group,
                }).unwrap()
            }).collect();
            let list = pyo3::types::PyList::new(py, &descriptions)?;
            call_wrapper!(self.wrapper, py, "mkt_depth_exchanges", (list.as_any(),));
        }

        // Drain scanner params -> scannerParameters
        let scanner_params = shared.reference.drain_scanner_params();
        for xml in scanner_params {
            call_wrapper!(self.wrapper, py, "scanner_parameters", (xml.as_str(),));
        }

        // Drain scanner data -> scannerData + scannerDataEnd
        let scanner_results = shared.reference.drain_scanner_data();
        for (req_id, result) in scanner_results {
            for (rank, entry) in result.entries.iter().enumerate() {
                let cd = ContractDetails::new_default(py);
                {
                    let mut contract = cd.contract.borrow_mut(py);
                    contract.con_id = entry.con_id as i64;
                    // Look up cached contract for symbol info
                    if let Some(ac) = self.core.get_contract(entry.con_id as i64, shared) {
                        contract.symbol = ac.symbol;
                        contract.sec_type = ac.sec_type;
                        contract.exchange = ac.exchange;
                        contract.currency = ac.currency;
                        contract.local_symbol = ac.local_symbol;
                        contract.primary_exchange = ac.primary_exchange;
                        contract.trading_class = ac.trading_class;
                    }
                }
                let cd_py = Py::new(py, cd)?.into_any();
                call_wrapper!(self.wrapper, py, "scanner_data", (req_id as i64, rank as i32, &cd_py, "", "", "", ""));
            }
            call_wrapper!(self.wrapper, py, "scanner_data_end", (req_id as i64,));
        }

        // Drain historical news -> historicalNews + historicalNewsEnd
        let news_results = shared.reference.drain_historical_news_for_dispatch();
        for (req_id, headlines, has_more) in news_results {
            for h in &headlines {
                call_wrapper!(self.wrapper, py, "historical_news", (req_id as i64, h.time.as_str(), h.provider_code.as_str(),
                     h.article_id.as_str(), h.headline.as_str()));
            }
            call_wrapper!(self.wrapper, py, "historical_news_end", (req_id as i64, has_more));
        }

        // Drain news articles -> newsArticle
        let articles = shared.reference.drain_news_articles();
        for (req_id, article_type, text) in articles {
            call_wrapper!(self.wrapper, py, "news_article", (req_id as i64, article_type, text.as_str()));
        }

        // Drain fundamental data -> fundamentalData
        let fundamentals = shared.reference.drain_fundamental_data_for_dispatch();
        for (req_id, data) in fundamentals {
            call_wrapper!(self.wrapper, py, "fundamental_data", (req_id as i64, data.as_str()));
        }

        // Drain histogram data -> histogram_data
        let histograms = shared.reference.drain_histogram_data_for_dispatch();
        for (req_id, entries) in histograms {
            let tuples: Vec<Bound<'_, pyo3::types::PyTuple>> = entries.iter().map(|e| {
                pyo3::types::PyTuple::new(py, &[e.price.into_pyobject(py).unwrap().into_any(), e.count.into_pyobject(py).unwrap().into_any()]).unwrap()
            }).collect();
            let py_list = pyo3::types::PyList::new(py, tuples)?;
            call_wrapper!(self.wrapper, py, "histogram_data", (req_id as i64, py_list));
        }

        // Drain historical ticks
        let hist_ticks = shared.reference.drain_historical_ticks();
        for (req_id, data, _what, done) in hist_ticks {
            match data {
                crate::types::HistoricalTickData::Midpoint(ticks) => {
                    let py_ticks: Vec<Bound<'_, pyo3::types::PyTuple>> = ticks.iter().map(|t| {
                        pyo3::types::PyTuple::new(py, &[
                            t.time.as_str().into_pyobject(py).unwrap().into_any(),
                            t.price.into_pyobject(py).unwrap().into_any(),
                        ]).unwrap()
                    }).collect();
                    let list = pyo3::types::PyList::new(py, py_ticks)?;
                    call_wrapper!(self.wrapper, py, "historical_ticks", (req_id as i64, list, done));
                }
                crate::types::HistoricalTickData::Last(ticks) => {
                    let py_ticks: Vec<Bound<'_, pyo3::types::PyTuple>> = ticks.iter().map(|t| {
                        pyo3::types::PyTuple::new(py, &[
                            t.time.as_str().into_pyobject(py).unwrap().into_any(),
                            t.price.into_pyobject(py).unwrap().into_any(),
                            t.size.into_pyobject(py).unwrap().into_any(),
                            t.exchange.as_str().into_pyobject(py).unwrap().into_any(),
                            t.special_conditions.as_str().into_pyobject(py).unwrap().into_any(),
                        ]).unwrap()
                    }).collect();
                    let list = pyo3::types::PyList::new(py, py_ticks)?;
                    call_wrapper!(self.wrapper, py, "historical_ticks_last", (req_id as i64, list, done));
                }
                crate::types::HistoricalTickData::BidAsk(ticks) => {
                    let py_ticks: Vec<Bound<'_, pyo3::types::PyTuple>> = ticks.iter().map(|t| {
                        pyo3::types::PyTuple::new(py, &[
                            t.time.as_str().into_pyobject(py).unwrap().into_any(),
                            t.bid_price.into_pyobject(py).unwrap().into_any(),
                            t.ask_price.into_pyobject(py).unwrap().into_any(),
                            t.bid_size.into_pyobject(py).unwrap().into_any(),
                            t.ask_size.into_pyobject(py).unwrap().into_any(),
                        ]).unwrap()
                    }).collect();
                    let list = pyo3::types::PyList::new(py, py_ticks)?;
                    call_wrapper!(self.wrapper, py, "historical_ticks_bid_ask", (req_id as i64, list, done));
                }
            }
        }

        // Drain real-time bars -> real_time_bar or historical_data_update (keepUpToDate)
        let rtbars = shared.market.drain_real_time_bars();
        for (req_id, bar) in rtbars {
            if self.core.hist_initial_complete.lock().unwrap().contains(&req_id) {
                // keepUpToDate bar → dispatch as historical_data_update
                // The moment the bar opened, in seconds since the epoch. The
                // bars of the initial answer carry the venue's own stamp in
                // the venue's own zone; a bar still forming is stamped here,
                // and a zone this client invented for it would be a claim
                // about a time zone rather than a time.
                let bar_obj = BarData::new(
                    format!("{}", bar.timestamp), bar.open, bar.high, bar.low, bar.close,
                    bar.volume as i64, bar.wap, bar.count,
                    String::new(), // streaming bars carry no timezone
                );
                let bar_py = Py::new(py, bar_obj)?.into_any();
                call_wrapper!(self.wrapper, py, "historical_data_update", (req_id as i64, &bar_py));
            } else {
                call_wrapper!(self.wrapper, py, "real_time_bar", (
                    req_id as i64,
                    bar.timestamp as i64,
                    bar.open, bar.high, bar.low, bar.close,
                    bar.volume, bar.wap, bar.count,
                ));
            }
        }

        // Drain historical schedules -> historical_schedule
        let schedules = shared.reference.drain_historical_schedules_for_dispatch();
        for (req_id, resp) in schedules {
            let sessions: Vec<Bound<'_, pyo3::types::PyTuple>> = resp.sessions.iter().map(|s| {
                pyo3::types::PyTuple::new(py, &[
                    s.ref_date.as_str().into_pyobject(py).unwrap().into_any(),
                    s.open_time.as_str().into_pyobject(py).unwrap().into_any(),
                    s.close_time.as_str().into_pyobject(py).unwrap().into_any(),
                ]).unwrap()
            }).collect();
            let py_sessions = pyo3::types::PyList::new(py, sessions)?;
            call_wrapper!(self.wrapper, py, "historical_schedule", (
                req_id as i64,
                resp.start_date_time.as_str(),
                resp.end_date_time.as_str(),
                resp.timezone.as_str(),
                py_sessions,
            ));
        }

        // Account updates (via ClientCore)
        if let Some(batch) = self.core.prepare_account_updates(shared) {
            let account_name = self.account();
            for field in &batch.fields {
                call_wrapper!(self.wrapper, py, "update_account_value", (field.key.as_str(), field.value.as_str(), field.currency.as_str(), account_name.as_str()));
            }

            // Portfolio updates (position entries)
            let portfolio = self.core.prepare_portfolio_updates(shared);
            for entry in &portfolio {
                let contract = self.core.get_contract(entry.con_id, shared);
                let c = contract.map(|ac| Contract::from_api(&ac))
                    .unwrap_or_else(|| Contract { con_id: entry.con_id, ..Default::default() });
                let c_py = pyo3::Py::new(py, c).unwrap().into_any();
                call_wrapper!(self.wrapper, py, "update_portfolio",
                    (&c_py, entry.position, entry.market_price, entry.market_value,
                     entry.avg_cost, entry.unrealized_pnl, entry.realized_pnl, account_name.as_str()));
            }

            if batch.finished {
                call_wrapper!(self.wrapper, py, "update_account_time", ("",));
                call_wrapper!(self.wrapper, py, "account_download_end", (account_name.as_str(),));
            }
        }

        // P&L dispatch (via ClientCore)
        if let Some(update) = self.core.poll_pnl(shared) {
            call_wrapper!(self.wrapper, py, "pnl", (update.req_id, update.daily_pnl, update.unrealized_pnl, update.realized_pnl));
        }

        // Per-position P&L dispatch (via ClientCore)
        for update in self.core.poll_pnl_single(shared) {
            call_wrapper!(self.wrapper, py, "pnl_single", (update.req_id, update.pos, update.daily_pnl,
                 update.unrealized_pnl, update.realized_pnl, update.value));
        }

        // Account summary dispatch (via ClientCore)
        {
            let acct_name = self.account();
            if let Some(batch) = self.core.prepare_account_summary(shared, acct_name.as_str()) {
                let tags_orig = self.core.account_summary_req.lock().unwrap().clone();
                let tags_list = tags_orig.map(|(_, t)| t).unwrap_or_default();
                if tags_list.is_empty() || tags_list.iter().any(|t| t == "AccountType") {
                    call_wrapper!(self.wrapper, py, "account_summary", (batch.req_id, acct_name.as_str(), "AccountType", "INDIVIDUAL", ""));
                }
                for entry in &batch.entries {
                    call_wrapper!(self.wrapper, py, "account_summary", (batch.req_id, acct_name.as_str(), entry.tag, entry.value.as_str(), entry.currency.as_str()));
                }
                call_wrapper!(self.wrapper, py, "account_summary_end", (batch.req_id,));
            }
        }

        Ok(())
    }
}
