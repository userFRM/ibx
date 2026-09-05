//! Event dispatch: drains SharedState queues and fires Python wrapper callbacks.

use crate::types::qty_to_f64;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use pyo3::prelude::*;

use crate::bridge::{Event, SharedState};
use crate::types::order_status::order_status_str;
use crate::types::*;

use crate::types::model::{
    Execution as ApiExecution,
    CommissionAndFeesReport as ApiCommissionAndFeesReport,
};
use super::EClient;
use super::super::contract::{Contract, ContractDescription, ContractDetails, BarData, CommissionAndFeesReport, DepthMktDataDescriptionPy, Execution, Order, OrderState};
use super::super::tick_types::*;
use super::super::super::types::PRICE_SCALE_F;

/// Tick type 13: the venue's model computation.
const MODEL_OPTION_COMPUTATION: i32 = 13;

/// Tick type 53: a computation this client was asked for.
///
/// The stream and the answer are two different things, and the venue names
/// them apart: a caller watching a contract reads the model on 13, and a
/// caller who asked what a volatility implies reads their answer on 53. Sent
/// under 13, an answer arrived indistinguishable from the stream.
const ASKED_OPTION_COMPUTATION: i32 = 53;

/// A figure the venue did not state, as the reference client states it.
///
/// This client holds an unstated double as `f64::MAX`, and so does the
/// reference stack — but only on its own side of the wire. What it sends a
/// caller is `-1` where a price or a volatility is unstated and `-2` where a
/// greek is, and the caller's library turns those two numbers back into
/// nothing. Handed `f64::MAX` instead, that test never fires and the number
/// goes into the caller's arithmetic.
fn unstated_as(value: f64, sentinel: f64) -> f64 {
    if value == f64::MAX || value.is_nan() { sentinel } else { value }
}

/// A price, a volatility or a dividend the venue did not state.
fn or_unstated_price(value: f64) -> f64 { unstated_as(value, -1.0) }

/// A greek the venue did not state.
fn or_unstated_greek(value: f64) -> f64 { unstated_as(value, -2.0) }

/// Call a Python wrapper method, catching and logging an ordinary exception instead of
/// propagating it so one bad callback cannot kill the dispatch loop.
/// `KeyboardInterrupt`,
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
    /// Single iteration of event dispatch: drain all shared queues and fire Python
    /// callbacks.
    pub(crate) fn dispatch_once(&self, py: Python<'_>, shared: &Arc<SharedState>) -> PyResult<()> {
        // What requests answered on the caller's thread, handed over here.
        // The reference client answers every request from its own loop, so a
        // program written against it may hold a lock across a request and take
        // it again in the callback; answered inside the request, that program
        // stops there. Oldest first, and before the engine's own events, so a
        // caller reads them in the order it asked.
        self.hand_over_what_is_waiting(py)?;

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
        // Which way the connection last went, as the engine wrote it down
        // rather than as it tried to announce it. Both transitions are
        // recorded in flags that are always set, while the channel above is
        // bounded and drops what it cannot hold — so a program far enough
        // behind lost the transition itself and then read the session as
        // connected right through an outage, or as disconnected after it had
        // come back. The request surface reads these same two flags. Read
        // before any callback runs, because a handler that answers the loss by
        // connecting again leaves a session whose state is not this one's.
        // The events stay the announcement: a loss the caller asked for sets
        // the flag too, and is not something to report as connectivity gone.
        let went = shared.take_connection_lost();
        let came_back = shared.take_connection_restored();
        if went {
            self.connected.store(false, Ordering::Release);
        }
        if came_back {
            self.connected.store(true, Ordering::Release);
        }
        // Whether the session is down as of this pass, by the flags rather
        // than by what is queued behind them. A restore still in the backlog
        // is then history, and must not be read as the state.
        let down_now = went && !came_back;
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
            // A loss the engine is still working on and one it has abandoned
            // are the same event; only the second records why the session
            // finished. Taken as the end either way, a caller lost the
            // recovery the engine was in the middle of.
            if shared.reference.session_over().is_some() {
                self.session_ended.store(true, Ordering::Release);
                // A question kept for a model belongs to the session that
                // asked it, and this session is finished. `connect()` may be
                // called again without `disconnect()`, so a question left here
                // would be answered in the next one under a request id nobody
                // there ever used.
                self.pending_option_calcs.lock().unwrap().clear();
            }
            call_wrapper!(self.wrapper, py, "error", (-1i64, 0i64, 1100i64, "Connectivity between client and server has been lost", ""));
        }
        // A session the caller ended is not a session that was lost, and is
        // not announced here: the dispatch loop ends and answers with
        // `connection_closed`, which is what the reference client answers
        // `disconnect()` with. Reported as 1100 as well, a program that stands
        // down on connectivity loss stood down on the session it had closed.
        if events.iter().any(|e| matches!(e, Event::Stopped)) {
            self.connected.store(false, Ordering::Release);
            self.session_ended.store(true, Ordering::Release);
        }
        // What the engine wrote down, rather than the notice it tried to send.
        // The event channel is bounded and drops what it cannot hold, so a
        // program far enough behind loses the one event that ends `run()` and
        // then waits on a session that finished — while the reason for it has
        // been recorded the whole time.
        //
        // Read against the session still held, not whichever one this pass
        // began with: a handler answering the loss above by connecting again
        // leaves a new session in place by the time this runs, and the
        // finished one read here would otherwise end it before it had done
        // anything at all.
        let still_current = self
            .shared
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|held| Arc::ptr_eq(held, shared));
        if still_current
            && !self.session_ended.load(Ordering::Relaxed)
            && shared.reference.session_over().is_some()
        {
            self.connected.store(false, Ordering::Release);
            self.session_ended.store(true, Ordering::Release);
            self.pending_option_calcs.lock().unwrap().clear();
        }
        // 1102 rather than 1101: the reconnect re-establishes the
        // subscriptions itself, so the caller has nothing to re-request. A
        // client that stood down on 1100 and never saw this stayed down.
        if events.iter().any(|e| matches!(e, Event::Reconnected)) {
            // Announced, because it happened; not stored where the flags say
            // the session went again behind it. A restore left in the backlog
            // used to overwrite the loss that followed it, and the caller read
            // a dead session as up while the other surface read it as down.
            if !down_now {
                self.connected.store(true, Ordering::Release);
            }
            call_wrapper!(self.wrapper, py, "error", (-1i64, 0i64, 1102i64, "Connectivity between client and server has been restored - data maintained", ""));
        }

        // One of the connections the venue keeps data on went away or came
        // back. Said as it happens, under the number the venue reports it
        // under: a caller reading quotes has nothing else to tell it that the
        // last price it holds stopped being a price, and the quotes it can
        // read do not go anywhere when the connection carrying them does.
        for event in &events {
            if let Event::VenueData { which, up } = event {
                let (broken, ok) = which.codes();
                call_wrapper!(self.wrapper, py, "error",
                    (-1i64, 0i64, if *up { ok } else { broken }, which.says(*up), ""));
            }
        }

        // The status that came with the same report, so one execution report
        // produces one `orderStatus` and not two. The engine announces the fill
        // and the order's resulting status together; emitting them separately
        // reports one execution twice, the second at no price.
        // Records held back while a fill for them was queued, freed once that
        // fill has been read. Freed here and nowhere else: this is the side
        // that reads the fills, so a record cannot be freed between a fill
        // being taken off the queue and the report that is built from it.
        if !self.deferred_evictions.lock().unwrap().is_empty() {
            self.deferred_evictions.lock().unwrap().retain(|oid| {
                if shared.orders.has_pending_fill(*oid) {
                    return true;
                }
                shared.orders.remove_order_info(*oid);
                false
            });
        }
        let mut paired: Vec<crate::types::OrderUpdate> = shared.orders.drain_order_updates();

        // A holding that moved since the caller last heard, where the caller
        // asked for positions and has not withdrawn the ask. The feed is
        // real-time, so a fill that changes what the account holds is followed
        // by the holding it changed. Nothing is drained while no ask stands.
        // `req_positions` takes what stood before its own answer and hands it
        // to the per-request watchers itself, so what is left here is what
        // moved after that answer — reported once, rather than replayed on the
        // pass that carries the answer.
        let on_position = self.positions_requested.load(Ordering::Acquire);
        let per_request: Vec<i64> = {
            let watching = self.positions_multi_requested.lock().unwrap();
            let mut ids: Vec<i64> = watching.iter().copied().collect();
            ids.sort_unstable();
            ids
        };
        if on_position || !per_request.is_empty() {
            // Drained once and given to everyone watching. Drained per
            // watcher, the first would take the move and the rest would never
            // hear of it.
            let moved = shared.portfolio.drain_position_changes();
            for pi in &moved {
                let c_py = Py::new(py, self.position_contract(py, pi, shared)?)?.into_any();
                let avg_cost = pi.avg_cost as f64 / crate::types::PRICE_SCALE as f64;
                if on_position {
                    call_wrapper!(
                        self.wrapper, py, "position",
                        (self.account().as_str(), &c_py, pi.position, avg_cost)
                    );
                }
                // The account this session opened under, whatever the request
                // named, as the answer to the request itself states.
                for req_id in &per_request {
                    call_wrapper!(
                        self.wrapper, py, "position_multi",
                        (*req_id, self.account().as_str(), "", &c_py, pi.position, avg_cost)
                    );
                }
            }
        }

        // Drain fills -> execDetails + orderStatus
        let fills = shared.orders.drain_fills();
        for (fill, booked_off) in fills {
            // A fill nobody asked for is numbered -1. The reference wrapper
            // decides a fill is live by the request id not matching one it is
            // waiting on, so any other id files the fill as the answer to that
            // request and suppresses the fill event.
            let req_id = -1i64;
            // The venue's two words for a side, which is what the reference
            // client hands a caller and what the rest of this client already
            // reports. A short sale is sold: the venue states no third word.
            let side_str = match fill.side {
                Side::Buy => "BOT",
                Side::Sell | Side::ShortSell => "SLD",
            };
            let price = fill.price as f64 / PRICE_SCALE_F;

            // Same report, not merely the same order. A pass can carry an
            // acknowledgement and a fill for one order, and those are two
            // reports: paired on the order alone, the fill took the
            // acknowledgement's status and the order never reported as filled.
            // The report they share is the one whose quantities agree.
            let with_it = paired
                .iter()
                .position(|u| {
                    u.order_id == fill.order_id
                        && u.remaining_qty == qty_to_f64(fill.remaining)
                        && u.filled_qty == qty_to_f64(fill.cum_qty)
                })
                .map(|at| paired.remove(at));
            // The status the report carries. Derived from the remaining
            // quantity only when the report states none.
            let status = with_it
                .map(|u| order_status_str(u.status))
                .unwrap_or(if fill.remaining == 0 { "Filled" } else { "Submitted" });
            if let Some(u) = with_it {
                self.core.update_order_status(
                    shared, u.order_id, u.status, u.filled_qty, u.remaining_qty, u.instrument,
                );
            }
            let (perm_id, parent_id) = self.core.perm_and_parent(shared, fill.order_id);
            // `filled` and `avgFillPrice` describe the order so far;
            // `lastFillPrice` describes this print.
            let avg_price = fill.avg_price as f64 / PRICE_SCALE_F;
            call_wrapper!(self.wrapper, py, "order_status", (fill.order_id as i64, status, qty_to_f64(fill.cum_qty), qty_to_f64(fill.remaining),
                 avg_price, perm_id, parent_id, price,
                 // The client the order was placed under, as the other surface
                 // reports it. Read off this client instead, a status about an
                 // order this one did not place named whoever happened to be
                 // watching.
                 self.core.placing_client(shared, fill.order_id) as i64, "", 0.0f64));

            // Track execution for req_executions.
            //
            // The venue states the execution's own id and the time it
            // happened, and both are held against the order. Neither is
            // composed here: an id built from an order number and a counter is
            // not the venue's, and the id is what a fill is reconciled against
            // a broker's own record by.
            // The report this fill was booked off, not whatever the order's
            // record says now: one pass can carry two prints of one order, and
            // the record holds only the later.
            let rich_info = booked_off.or_else(|| shared.orders.get_order_info(fill.order_id));
            // What the report stated beyond the print, taken before anything
            // consumes the record.
            let from_the_report = rich_info
                .as_ref()
                .map(|info| info.last_exec.clone())
                .unwrap_or_default();
            // Left as the report stated them, which is what the comment above
            // says and what the other surface does. Composed from the order
            // number and the clock instead, a caller reconciling against the
            // broker's own record was handed an id the broker never issued —
            // and the time, which `req_executions` filters on by comparing
            // digits, read as a count of nanoseconds and put every such fill
            // before every bound a caller can state.
            let exec_id = rich_info
                .as_ref()
                .map(|i| i.last_exec.exec_id.clone())
                .unwrap_or_default();
            let now_str = rich_info
                .as_ref()
                .map(|i| i.last_exec.time.clone())
                .unwrap_or_default();
            let exec_exchange = rich_info.as_ref()
                .map(|i| i.last_exec.exchange.as_str()).unwrap_or("").to_string();
            let cum_qty = rich_info.as_ref()
                .map(|i| i.last_exec.cum_qty).unwrap_or(qty_to_f64(fill.qty));
            let avg_price = rich_info.as_ref()
                .map(|i| i.last_exec.avg_price).unwrap_or(price);
            // Build api-level contract for shared storage
            let api_contract = self.core.open_orders.lock().unwrap()
                .get(&fill.order_id).map(|o| o.contract.clone())
                .or_else(|| {
                    rich_info.map(|info| info.contract)
                })
                .unwrap_or_default();

            // Everything the report stated, with the print's own numbers over
            // it. Built from nothing instead, the record kept for a replay
            // dropped what only the report carries — the caller's own label for
            // the order among it — so a fill answered under `req_executions`
            // was blank where the live callback had stated it.
            let api_exec = ApiExecution {
                exec_id: exec_id.clone(),
                time: now_str.clone(),
                acct_number: self.account(),
                exchange: exec_exchange.clone(),
                side: side_str.to_string(),
                shares: qty_to_f64(fill.qty),
                price,
                order_id: fill.order_id as i64,
                // The order's own permanent number and the client that placed
                // it, as the callback below carries them. Stored as zero, a
                // request filtered by client matched nothing at all, and the
                // same fill replayed named no client and no permanent id.
                perm_id,
                // What the report stated, and where it stated none, the client
                // the order was placed under. Read off this client instead, a
                // fill on an order somebody else placed was labelled with
                // whoever happened to be asking — and `req_executions` filters
                // on exactly that field.
                client_id: if from_the_report.client_id != 0 {
                    from_the_report.client_id
                } else {
                    i64::from(self.core.placing_client(shared, fill.order_id))
                },
                cum_qty,
                avg_price,
                ..from_the_report
            };
            // What it cost is not stated on this report. It arrives on a
            // record of its own, after this, and is reported from there — see
            // the drain below. Stored unstated so a replay of this execution
            // says the charge is unknown rather than that it was nothing.
            let api_commission = ApiCommissionAndFeesReport::default();

            let c_py = Py::new(py, Contract::from_api(py, &api_contract)?)?.into_any();
            // The same record the replay keeps, in the shape a caller reads,
            // so the two cannot state different things about one fill.
            let exec_py = Py::new(py, Execution::from_api(&api_exec))?.into_any();
            // Kept for `req_executions` to answer from.
            self.core.push_execution(req_id, api_contract, api_exec, api_commission);
            call_wrapper!(self.wrapper, py, "exec_details", (req_id, &c_py, &exec_py));

            // Update open order tracking
            self.core.update_order_fill(fill.order_id, status, qty_to_f64(fill.cum_qty), qty_to_f64(fill.remaining));

        }

        // Executions the venue restated rather than announced. Filed for
        // `req_executions` and reported to nobody: a caller that asks is
        // answered, and one that did not hears nothing.
        self.core.record_restated_executions(shared);

        // What the venue says its fills cost, each naming the execution it
        // belongs to. Reported after the executions above, which is the order
        // they arrive in and the order a caller reads them in.
        for charge in shared.orders.drain_charges() {
            self.core.record_charge(&charge);
            let report = CommissionAndFeesReport {
                exec_id: charge.exec_id.clone(),
                commission_and_fees: charge.commission_and_fees,
                currency: charge.currency.clone(),
                realized_pnl: charge.realized_pnl,
                yield_amount: charge.yield_amount,
                yield_redemption_date: charge.yield_redemption_date.clone(),
            };
            let report_py = Py::new(py, report)?.into_any();
            call_wrapper!(self.wrapper, py, "commission_and_fees_report", (&report_py,));
        }

        // What is left: a status change with no fill on the same report.
        // In the order they arrived. They were sorted by order only because
        // they had been collected into a map, which left them in no order at
        // all; kept as they came, the order the venue reported them in is the
        // order the caller reads them in.
        for update in paired {
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
                let contract_py = Py::new(py, Contract::from_api(py, &tracked.contract)?)?.into_any();
                let order_py = Py::new(py, Order::from_api(py, &tracked.order)?)?.into_any();
                let state_py = Py::new(py, OrderState {
                    status: status.to_string(),
                    ..Default::default()
                })?.into_any();
                call_wrapper!(self.wrapper, py, "open_order",
                    (update.order_id as i64, &contract_py, &order_py, &state_py));
            }

            call_wrapper!(self.wrapper, py, "order_status", (update.order_id as i64, status, update.filled_qty,
                 update.remaining_qty, avg, update.perm_id, parent_id, 0.0f64,
                 self.core.placing_client(shared, update.order_id) as i64, "", 0.0f64));

            // Track open orders
            self.core.update_order_status(shared, update.order_id, update.status, update.filled_qty, update.remaining_qty, update.instrument);
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
            call_wrapper!(self.wrapper, py, "error", (-1i64, super::raised_now(), 321i64, text, ""));
        }

        // A lookup that named a contract another slot already holds. One
        // subscription per contract exists on the wire, so the callers given
        // the second slot read the first — otherwise their quotes arrive on a
        // slot nothing is watching.
        for (from, into) in shared.market.drain_subscription_moves() {
            self.core.move_watchers(from, into);
        }
        for (instrument, reason) in shared.market.drain_subscription_failures() {
            let req_id = self.core.req_id_for_instrument(instrument);
            call_wrapper!(self.wrapper, py, "error", (req_id, 0i64, 200i64, reason, ""));
        }

        // A book this client could not keep whole, on the request that asked
        // for it. Nothing further is kept for it, so a caller not told reads a
        // subscription that is up and a book that has stopped moving. 354 is
        // what the reference client reports when data asked for is not served.
        for (req_id, reason) in shared.market.drain_depth_drops() {
            call_wrapper!(self.wrapper, py, "error",
                (i64::from(req_id), super::raised_now(), 354i64, reason, ""));
        }

        // A calculation asked for before the venue had stated a model waited on
        // the watch that asking opened. Answer it here, before the drain, so
        // the caller gets the question they asked rather than only the model.
        if !self.pending_option_calcs.lock().unwrap().is_empty() {
            self.answer_kept_option_calcs();
        }

        for comp in shared.market.drain_option_computations() {
            // A locally solved calculation answers the request that asked for
            // it. The venue's model belongs to the contract, so it goes to
            // every request watching that contract.
            let (to, tick_type): (Vec<i64>, i32) = match comp.answers {
                Some(asked) => (vec![asked], ASKED_OPTION_COMPUTATION),
                None => {
                    let owner = self.core.req_id_for_instrument(comp.instrument);
                    (
                        std::iter::once(owner)
                            .chain(self.core.followers_of(comp.instrument))
                            .collect(),
                        MODEL_OPTION_COMPUTATION,
                    )
                }
            };
            for req_id in to {
                call_wrapper!(self.wrapper, py, "tick_option_computation",
                    (req_id, tick_type, 0i32,
                     or_unstated_price(comp.implied_vol), or_unstated_greek(comp.delta),
                     or_unstated_price(comp.opt_price), or_unstated_price(comp.pv_dividend),
                     or_unstated_greek(comp.gamma), or_unstated_greek(comp.vega),
                     or_unstated_greek(comp.theta), or_unstated_price(comp.und_price)));
            }
        }

        // A replacement the venue has taken spends the terms kept against a
        // refusal of it. Before the refusals below, as on the other surface.
        for order_id in shared.orders.drain_replacements_taken() {
            self.core.settle_replacement(order_id);
        }

        // Drain cancel rejects -> error
        let rejects = shared.orders.drain_cancel_rejects();
        for reject in rejects {
            let (code, msg) = self.core.retire_rejected(&reject);
            call_wrapper!(self.wrapper, py, "error", (reject.order_id as i64, 0i64, code, msg.as_str(), ""));
        }

        // Drain inactive-order reasons -> error
        for (order_id, code, msg) in shared.orders.drain_order_inactive() {
            // A refusal is the end of a preview: it states what an order would
            // have cost, and nothing reached the book. Left standing, as it
            // was on this surface alone, the record read as a working order —
            // so the next placement under that number went out as a change to
            // an order the venue had never been given.
            if self.core.tracked_order(order_id).is_some_and(|o| o.what_if) {
                self.core.untrack_order(order_id);
            }
            call_wrapper!(self.wrapper, py, "error", (order_id as i64, 0i64, code as i64, msg.as_str(), ""));
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
            // Which kinds the venue has stated, for anything waiting on a
            // snapshot of this contract.
            for tick in &result.ticks {
                for id in std::iter::once(tick.req_id).chain(watchers.iter().copied()) {
                    self.core.note_snapshot_tick(id, tick.tick_type);
                }
            }
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
            for tick in &result.generic_ticks {
                for id in std::iter::once(tick.req_id).chain(watchers.iter().copied()) {
                    call_wrapper!(self.wrapper, py, "tick_generic", (id, tick.tick_type, tick.value));
                }
            }
            for st in &result.string_ticks {
                for id in std::iter::once(st.req_id).chain(watchers.iter().copied()) {
                    call_wrapper!(self.wrapper, py, "tick_string", (id, st.tick_type, st.value.as_str()));
                }
            }
            if let Some(ts) = &result.timestamp {
                let ts_secs = ts.timestamp_ns / 1_000_000_000;
                // To everyone watching this contract, as the prices and the
                // other strings above are. Sent to the request that made the
                // subscription alone, a second caller on the same contract had
                // every tick but the one that says when the last trade
                // happened, and could not tell a live print from a stale one.
                for id in std::iter::once(ts.req_id).chain(watchers.iter().copied()) {
                    call_wrapper!(self.wrapper, py, "tick_string", (id, TICK_LAST_TIMESTAMP, ts_secs.to_string().as_str()));
                }
            }
            // The holder and everyone watching it, for the reason the ticks
            // above go to both: a caller that asked for a snapshot of a
            // contract somebody was already watching is recorded as a
            // follower, and naming only the holder left its snapshot never
            // completed and never withdrawn.
            for id in std::iter::once(req_id).chain(watchers.iter().copied()) {
                if self.core.check_snapshot_done(id) {
                    call_wrapper!(self.wrapper, py, "tick_snapshot_end", (id,));
                    snapshot_done.push(id);
                }
            }
        }
        // Withdrawn by this client, not by the caller, so nothing is reported:
        // a handler that disconnected on `tick_snapshot_end` is not told 504
        // about a snapshot that completed, and an engine that has gone is not
        // an exception out of `run` — the session was recorded as over above.
        for req_id in snapshot_done {
            let Ok(tx) = self.tx() else { break };
            if let Err(why) = self.withdraw_mkt_data(py, &tx, req_id) {
                log::debug!("withdrawing finished snapshot {req_id}: {why}");
            }
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
            // The stream this request asked for. Stated as 1 whichever it was,
            // a caller subscribed to every print was told each of them came
            // from the exchange, and one holding both subscriptions could not
            // tell the two apart.
            let kind = self.tbt_kind.lock().unwrap().get(&req_id).copied().unwrap_or(1);
            call_wrapper!(self.wrapper, py, "tick_by_tick_all_last", (req_id, kind, trade.timestamp as i64, price, size,
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
        let depth_updates = shared.market.drain_depth_updates_for_dispatch(
            |id| shared.reference.is_ours(crate::bridge::RecordKind::Depth, i64::from(id)),
        );
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
            // Once per caller watching the contract, as its quotes already
            // are. The owner alone was told, so a second subscription on the
            // same contract heard no news at all.
            let watchers = self.core.followers_of(news.instrument);
            for id in std::iter::once(req_id).chain(watchers.iter().copied()) {
                call_wrapper!(self.wrapper, py, "tick_news", (id, news.timestamp as i64, news.provider_code.as_str(),
                     news.article_id.as_str(), news.headline.as_str(), ""));
            }
        }

        // Drain news bulletins -> updateNewsBulletin
        if self.core.bulletin_subscribed.load(Ordering::Acquire) {
            let bulletins = shared.market.drain_news_bulletins();
            for b in bulletins {
                call_wrapper!(self.wrapper, py, "update_news_bulletin", (b.msg_id as i64, b.msg_type, b.message.as_str(), b.exchange.as_str()));
            }
        }

        // Drain what-if responses -> open_order(contract, order, OrderState) +
        // order_status
        // (iso with official ibapi: server delivers margin via openOrder.orderState)
        let what_ifs = shared.orders.drain_what_if_responses();
        for wi in what_ifs {
            let state = OrderState::from_api(&crate::types::model::OrderState::from(&wi));

            let tracked = self.core.open_orders.lock().unwrap().get(&wi.order_id).cloned();
            let (contract_py, order_py) = if let Some(t) = tracked {
                let c = Contract::from_api(py, &t.contract)?;
                let o = Order::from_api(py, &t.order)?;
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
        for (req_id, code, msg) in shared.reference.drain_historical_errors_for_dispatch(
            |id| shared.reference.held_under_any_kind(i64::from(id)),
        ) {
            let req_id = crate::bridge::ReferenceState::request_id_reported(req_id);
            call_wrapper!(self.wrapper, py, "error", (req_id, 0i64, code as i64, msg.as_str(), ""));
        }

        // Drain historical data -> historicalData + historicalDataEnd /
        // historicalDataUpdate
        let hist_data = shared.reference.drain_historical_data_for_dispatch();
        for (req_id, response) in hist_data {
            let is_update = self.core.hist_initial_complete.lock().unwrap().contains(&req_id);
            for bar in &response.bars {
                let bar_obj = BarData::new(
                    self.core.bar_time_for(req_id as i64, &bar.time, &response.timezone),
                    bar.open, bar.high, bar.low, bar.close,
                    bar.volume, bar.wap, bar.count,
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
                // The range the request covered. A caller paging backwards
                // feeds the start in as its next end; given two empty strings,
                // as it was, every page it asked for was the page it had.
                let (from, to) =
                    self.core.historical_range_for(req_id as i64, &response.timezone);
                call_wrapper!(self.wrapper, py, "historical_data_end",
                    (req_id as i64, from.as_str(), to.as_str()));
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
                    contract: Py::new(py, Contract {
                        con_id: m.con_id as i64,
                        symbol: m.symbol.clone(),
                        // The user-visible spelling, the same one the Rust
                        // surface hands back: a stock reaches this as CS, the
                        // wire name for it, which no request accepts.
                        sec_type: m.sec_type.to_api_str().to_string(),
                        currency: m.currency.clone(),
                        primary_exchange: m.primary_exchange.clone(),
                        ..Default::default()
                    }).unwrap(),
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
            // A refused scan arrives in the shape of a completed one and
            // carries the reason, as on the other surface. Reported against
            // the requesting id, so a refusal is not delivered as an empty
            // result.
            if !result.error_text.is_empty() {
                call_wrapper!(self.wrapper, py, "error",
                    (req_id as i64, 0i64, 321i64, result.error_text.as_str(), ""));
            }
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
        // Each tick as the reference client hands it over: a record with names
        // on it. Handed over as a tuple it carries the same numbers and answers
        // to none of the names, so a program reading `tick.price` off what it
        // was given finds nothing there.
        let hist_ticks = shared.reference.drain_historical_ticks();
        for (req_id, data, _what, done) in hist_ticks {
            // The venue states the moment as it spells it; the reference client
            // states it in seconds, and a record's moment is an integer there
            // with no room to say "unreadable". A stamp that cannot be read
            // back leaves the tick out rather than putting it in 1970: a
            // caller charting the series would otherwise see a print half a
            // century before the market it asked about.
            let at = crate::protocol::datetime::ib_datetime_to_unix;
            let dropped = std::cell::Cell::new(0usize);
            match data {
                crate::types::HistoricalTickData::Midpoint(ticks) => {
                    let py_ticks: Vec<crate::python::compat::tick_types::HistoricalTick> = ticks.iter().filter_map(|t| Some(crate::python::compat::tick_types::HistoricalTick {
                        time: at(&t.time).or_else(|| { dropped.set(dropped.get() + 1); None })?,
                        price: t.price,
                        // A midpoint has no size, and the reference client
                        // states zero for it.
                        size: 0.0,
                    })).collect();
                    let list = pyo3::types::PyList::new(py, py_ticks)?;
                    call_wrapper!(self.wrapper, py, "historical_ticks", (req_id as i64, list, done));
                }
                crate::types::HistoricalTickData::Last(ticks) => {
                    let py_ticks: Vec<crate::python::compat::tick_types::HistoricalTickLast> = ticks.iter().filter_map(|t| Some(crate::python::compat::tick_types::HistoricalTickLast {
                        time: at(&t.time).or_else(|| { dropped.set(dropped.get() + 1); None })?,
                        tick_attrib_last: Default::default(),
                        price: t.price,
                        size: t.size,
                        exchange: t.exchange.clone(),
                        special_conditions: t.special_conditions.clone(),
                    })).collect();
                    let list = pyo3::types::PyList::new(py, py_ticks)?;
                    call_wrapper!(self.wrapper, py, "historical_ticks_last", (req_id as i64, list, done));
                }
                crate::types::HistoricalTickData::BidAsk(ticks) => {
                    let py_ticks: Vec<crate::python::compat::tick_types::HistoricalTickBidAsk> = ticks.iter().filter_map(|t| Some(crate::python::compat::tick_types::HistoricalTickBidAsk {
                        time: at(&t.time).or_else(|| { dropped.set(dropped.get() + 1); None })?,
                        tick_attrib_bid_ask: Default::default(),
                        price_bid: t.bid_price,
                        price_ask: t.ask_price,
                        size_bid: t.bid_size,
                        size_ask: t.ask_size,
                    })).collect();
                    let list = pyo3::types::PyList::new(py, py_ticks)?;
                    call_wrapper!(self.wrapper, py, "historical_ticks_bid_ask", (req_id as i64, list, done));
                }
            }
            if dropped.get() > 0 {
                // Told to the caller, not only to the log. A shortened series
                // and a complete one look the same to a program charting it,
                // and the difference is prints that happened and are not
                // there. Under the number this client reports a request it
                // could not make sense of.
                let why = format!(
                    "{} historical tick(s) state a moment that cannot be read back, and are \
                     left out of this answer rather than dated to 1970",
                    dropped.get(),
                );
                call_wrapper!(self.wrapper, py, "error",
                    (req_id as i64, super::raised_now(), crate::error_codes::Refusal::VALIDATION as i64, why.as_str(), ""));
            }
        }

        // Drain real-time bars -> real_time_bar or historical_data_update
        // (keepUpToDate)
        let rtbars = shared.market.drain_real_time_bars_for_dispatch(
            |id| shared.reference.is_ours(crate::bridge::RecordKind::Bars, i64::from(id)),
        );
        for (req_id, bar) in rtbars {
            if self.core.hist_initial_complete.lock().unwrap().contains(&req_id) {
                // keepUpToDate bar → dispatch as historical_data_update
                // Bar open time, in seconds since the epoch. Bars of the
                // initial answer carry the venue's stamp and zone; a bar still
                // forming is stamped locally and states no zone.
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
                let c = match self.core.get_contract(entry.con_id, shared) {
                    Some(ac) => Contract::from_api(py, &ac)?,
                    None => Contract { con_id: entry.con_id, ..Default::default() },
                };
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
                // The account type rides the batch with every other figure, as
                // the venue stated it. A fixed "INDIVIDUAL" misreports the
                // advisor and institutional accounts, which are the ones the
                // answer decides something for.
                for entry in &batch.entries {
                    call_wrapper!(self.wrapper, py, "account_summary", (batch.req_id, acct_name.as_str(), entry.tag.as_str(), entry.value.as_str(), entry.currency.as_str()));
                }
                call_wrapper!(self.wrapper, py, "account_summary_end", (batch.req_id,));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod unstated_tests {
    use super::{or_unstated_greek, or_unstated_price};

    /// The reference stack turns `-1` and `-2` back into nothing, and turns
    /// every other number into itself. A figure this client holds as unstated
    /// has to arrive as one of those two or the caller reads it as a price.
    #[test]
    fn an_unstated_figure_arrives_as_the_number_that_means_nothing() {
        for unstated in [f64::MAX, f64::NAN] {
            assert_eq!(or_unstated_price(unstated), -1.0);
            assert_eq!(or_unstated_greek(unstated), -2.0);
        }
        // Everything else is the venue's own figure and passes through, a
        // negative delta and a zero among them.
        for stated in [0.0, -0.42, 1.0, 775.4] {
            assert_eq!(or_unstated_price(stated), stated);
            assert_eq!(or_unstated_greek(stated), stated);
        }
    }

    /// A solve states a volatility against a price and computes no greek.
    #[test]
    fn a_solved_computation_states_no_greek() {
        let solved = crate::types::OptionComputation::solved(7);
        assert_eq!(solved.answers, Some(7));
        for greek in [solved.delta, solved.gamma, solved.vega, solved.theta, solved.pv_dividend] {
            assert_eq!(or_unstated_greek(greek), -2.0, "a greek nobody computed reads as one");
        }
    }
}

#[cfg(test)]
mod withdrawal_tests {
    use super::*;

    /// A snapshot that ends once the engine has gone is withdrawn quietly.
    ///
    /// The pass reaches the withdrawal with the session already recorded as
    /// over. A send that fails there is not an exception out of `run`, which
    /// ends the way the loop means it to, with `connection_closed`.
    #[test]
    fn a_snapshot_ending_after_the_engine_has_gone_does_not_end_the_pass_with_an_error() {
        Python::initialize();
        Python::attach(|py| {
            let client = EClient::__new__(&pyo3::types::PyTuple::empty(py), None);
            let wrapper = py
                .eval(c"type('W', (), {'__getattr__': lambda s, n: (lambda *a: None)})()", None, None)
                .unwrap()
                .unbind();
            client.__init__(wrapper).unwrap();
            let shared = Arc::new(SharedState::new());
            shared.market.set_instrument_count(1);
            // The engine's end of the channel is gone.
            let (tx, rx) = std::sync::mpsc::sync_channel(4);
            drop(rx);
            *client.shared.lock().unwrap() = Some(shared.clone());
            *client.control_tx.lock().unwrap() = Some(tx);
            client.connected.store(true, Ordering::Release);
            // A snapshot asked for long enough ago to be swept on this pass.
            client.core.req_to_instrument.lock().unwrap().insert(1, 0);
            client.core.instrument_to_req.lock().unwrap().insert(0, 1);
            client.core.snapshot_reqs.lock().unwrap().insert(
                1, (std::time::Instant::now() - std::time::Duration::from_secs(12), 0),
            );

            client.dispatch_once(py, &shared).expect("the pass ends, saying nothing of the withdrawal");
        });
    }
}

#[cfg(test)]
mod scanner_tests {
    use super::*;

    /// A scan the venue refused arrives in the shape of a completed one.
    /// Handed over as an empty result, the caller was told the market held
    /// no matches where the venue declined the question — told instead, as
    /// on the other surface.
    #[test]
    fn a_refused_scan_is_reported_not_delivered_as_an_empty_result() {
        Python::initialize();
        Python::attach(|py| {
            let client = EClient::__new__(&pyo3::types::PyTuple::empty(py), None);
            let wrapper = py
                .eval(c"__import__('builtins').type('W', (), {'__init__': lambda s: setattr(s, 'calls', []), '__getattr__': lambda s, n: (lambda *a: s.calls.append((n, a)))})()", None, None)
                .unwrap()
                .unbind();
            client.__init__(wrapper.clone_ref(py)).unwrap();
            let shared = Arc::new(SharedState::new());
            shared.reference.push_scanner_data(3, crate::control::scanner::ScannerResult {
                con_ids: Vec::new(),
                entries: Vec::new(),
                scan_time: String::new(),
                error_text: "Scanner subscription not allowed".to_string(),
            });

            client.dispatch_once(py, &shared).expect("the pass ends");

            let calls = wrapper.getattr(py, "calls").unwrap();
            let list = calls.cast_bound::<pyo3::types::PyList>(py).unwrap();
            let names: Vec<String> = list.iter()
                .map(|c| c.get_item(0).unwrap().extract::<String>().unwrap())
                .collect();
            assert!(names.contains(&"error".to_string()),
                "the refusal is reported: {names:?}");
            assert!(!names.contains(&"scanner_data".to_string()),
                "and nothing is delivered as a row: {names:?}");
            let error_call = list.iter()
                .find(|c| c.get_item(0).unwrap().extract::<String>().unwrap() == "error")
                .unwrap();
            let args = error_call.get_item(1).unwrap();
            assert_eq!(args.get_item(0).unwrap().extract::<i64>().unwrap(), 3,
                "against the requesting id");
            assert_eq!(args.get_item(3).unwrap().extract::<String>().unwrap(),
                "Scanner subscription not allowed", "in the venue's own words");
        });
    }
}
