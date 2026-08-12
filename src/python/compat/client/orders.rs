//! Order placement, cancellation, open orders, executions, completed orders.

use std::sync::atomic::Ordering;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::api::types::{
    Contract as ApiContract, ExecutionFilter,
};
use crate::api::error_codes::Refusal;
use crate::client_core::ClientCore;
use crate::types::*;
use super::EClient;
use super::super::contract::{Contract, Order, CommissionAndFeesReport, Execution};

#[pymethods]
impl EClient {
    /// Place an order.
    ///
    /// A request the client will not send is reported under the number the
    /// reference client reports it under, and the call returns. A program
    /// moved from that client has an `error` handler and no exception
    /// handling around a request, because nothing it was written against
    /// raises there.
    fn place_order(&self, py: Python<'_>, order_id: i64, contract: &Contract, order: &Order) -> PyResult<()> {
        self.core.refuse_if_readonly("an order").map_err(PyRuntimeError::new_err)?;
        let Some(tx) = self.tx_or_report(order_id) else { return Ok(()) };

        if let Err(why) = ClientCore::validate_order_destination(&contract.exchange) {
            return self.report_refusal(py, order_id, why.into());
        }

        // Convert and validate order params first (fail fast, no connection needed)
        let mut api_order = order.to_api();
        api_order.conditions = order.convert_conditions(py);
        // What the order path reads off a contract: where it is listed, its
        // legs, and the contract it hedges against. The legs and the hedge are
        // Python objects, so reading them needs the interpreter.
        let api_contract = crate::api::types::Contract {
            primary_exchange: contract.primary_exchange.clone(),
            combo_legs: contract.combo_legs_api(py).map_err(PyRuntimeError::new_err)?,
            delta_neutral_contract: contract.delta_neutral_contract.as_ref().map(|d| {
                let g = |n: &str| d.getattr(py, n).ok();
                crate::api::types::DeltaNeutralContract {
                    con_id: g("conId").and_then(|v| v.extract(py).ok()).unwrap_or(0),
                    delta: g("delta").and_then(|v| v.extract(py).ok()).unwrap_or(0.0),
                    price: g("price").and_then(|v| v.extract(py).ok()).unwrap_or(0.0),
                }
            }),
            ..Default::default()
        };
        // Empty before connect, so a named account cannot match and is refused.
        // That is the right answer either way: the field reaches no encoder, so
        // an order naming one would fill somewhere else whether or not a session
        // exists to compare against.
        let connected = self.account_id.lock().unwrap().clone().unwrap_or_default();
        if let Err(why) = ClientCore::validate_order(&api_order, &connected) {
            return self.report_refusal(py, order_id, why.into());
        }
        if let Err(why) = ClientCore::validate_supported_instructions(&api_order) {
            return self.report_refusal(py, order_id, why.into());
        }
        if let Err(why) = ClientCore::validate_combo_legs(
            &contract.sec_type, api_contract.combo_legs.len(),
        ) {
            return self.report_refusal(py, order_id, why.into());
        }
        if let Err(why) = ClientCore::validate_order_contract(
            contract.con_id,
            &contract.sec_type,
            &ClientCore::contract_identity(
                &contract.last_trade_date_or_contract_month, contract.strike,
                &contract.right, &contract.multiplier, &contract.currency,
            ),
        ) {
            return self.report_refusal(py, order_id, why.into());
        }
        // The same guard the Rust surface applies. Without it here, whether a
        // caller is protected from an order the venue will refuse in silence
        // depends on which language they wrote in.
        if let Ok(shared) = self.shared_state() {
            if let Err(why) = ClientCore::refuse_unpermitted_sec_type(
                &shared.reference.order_permissions(), &contract.sec_type,
            ) {
                return self.report_refusal(py, order_id, why.into());
            }
        }

        // An order names its contract by the venue's own id. A caller who
        // states a description instead — which every example written against
        // the reference client does — has it resolved here, once the order
        // itself is known to be one the venue would take: an order carrying no
        // id names nothing the venue can match, and is answered by silence.
        let named;
        let contract = if contract.con_id == 0 && !contract.symbol.is_empty() {
            match self.qualify_contract(py, contract) {
                Ok(found) => { named = found; &named }
                Err(why) => return self.report_refusal(
                    py, order_id, Refusal::no_definition(why.value(py).to_string()),
                ),
            }
        } else {
            contract
        };

        let oid = if order_id > 0 {
            order_id as u64
        } else {
            self.next_order_id.fetch_add(1, Ordering::Relaxed)
        };

        let instrument = self.find_or_register_instrument(py, contract)?;

        // If orderId is already tracked, this is a modification — emit Modify instead of Submit.
        let cmd = if self.core.is_order_tracked(oid) {
            // A replace states the order type, the limit price and the trigger.
            // An order defined by anything else cannot survive one.
            if let Some(refusal) = self.core.modify_refusal(oid, &api_order) {
                return self.report_refusal(py, order_id, refusal.into());
            }
            let price = (api_order.lmt_price * crate::api::types::PRICE_SCALE_F) as i64;
            let qty = api_order.total_quantity as u32;
            // A stop's trigger rides on aux_price, exactly as it does on the
            // submit path.
            let stop_price = (api_order.aux_price * crate::api::types::PRICE_SCALE_F) as i64;
            ControlCommand::Order(OrderRequest::Modify {
                new_order_id: oid,
                order_id: oid,
                price,
                qty,
                outside_rth: api_order.outside_rth,
                ord_type: api_order.ord_type_byte(),
                tif: api_order.tif_byte(),
                stop_price,
            })
        } else {
            match ClientCore::build_order_request(&api_order, oid, instrument, Some(&api_contract)) {
                Ok(built) => built,
                Err(why) => return self.report_refusal(py, order_id, why.into()),
            }
        };
        Self::send_control(py, &tx, cmd)?;

        // Track order in shared core
        let api_contract = ApiContract {
            con_id: contract.con_id,
            symbol: contract.symbol.clone(),
            sec_type: contract.sec_type.clone(),
            exchange: contract.exchange.clone(),
            currency: contract.currency.clone(),
            ..Default::default()
        };
        let mut tracked_order = api_order.clone();
        tracked_order.order_id = oid as i64;
        self.core.cache_contract(contract.con_id, api_contract.clone());
        self.core.track_order(oid, api_contract, tracked_order, instrument);

        Ok(())
    }

    /// Exercise or lapse a long option position.
    ///
    /// `exercise_action` is 1 to exercise and 2 to lapse; anything else is
    /// refused. `_override` is taken for signature compatibility and is not
    /// sent: it is a validation bypass the venue's own front end applies before
    /// it builds the order, so there is no tag for it on the wire.
    #[pyo3(signature = (req_id, contract, exercise_action, exercise_quantity, account, _override))]
    fn exercise_options(
        &self, py: Python<'_>, req_id: i64, contract: &Contract, exercise_action: i32,
        exercise_quantity: i32, account: &str, _override: i32,
    ) -> PyResult<()> {
        self.core.refuse_if_readonly("an exercise").map_err(PyRuntimeError::new_err)?;
        // The session is what names the account an exercise would be taken on,
        // so it is established before the one the caller named is compared
        // against it. Without a session there is nothing to compare and nothing
        // to send, and the caller is told that rather than told about its
        // account.
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        let (action, qty) = match ClientCore::validate_exercise(
            exercise_action, exercise_quantity, account, &self.account(),
        ) {
            Ok(pair) => pair,
            Err(why) => return self.report_refusal(py, req_id, why.into()),
        };
        if let Err(why) = ClientCore::validate_order_contract(
            contract.con_id,
            &contract.sec_type,
            &ClientCore::contract_identity(
                &contract.last_trade_date_or_contract_month, contract.strike,
                &contract.right, &contract.multiplier, &contract.currency,
            ),
        ) {
            return self.report_refusal(py, req_id, why.into());
        }

        let oid = if req_id > 0 {
            req_id as u64
        } else {
            self.next_order_id.fetch_add(1, Ordering::Relaxed)
        };
        let instrument = self.find_or_register_instrument(py, contract)?;
        Self::send_control(py, &tx, ControlCommand::Order(
            ClientCore::build_exercise_request(oid, instrument, action, qty),
        ))
    }

    /// Cancel an order.
    #[pyo3(signature = (order_id, manual_order_cancel_time=""))]
    fn cancel_order(&self, py: Python<'_>, order_id: i64, manual_order_cancel_time: &str) -> PyResult<()> {
        self.core.refuse_if_readonly("a cancel").map_err(PyRuntimeError::new_err)?;
        let Some(tx) = self.tx_or_report(order_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::Order(OrderRequest::Cancel { order_id: order_id as u64 }))?;
        let _ = manual_order_cancel_time;
        Ok(())
    }

    /// Cancel an order identified by `permId` — stable across sessions, unlike
    /// the local order id. The cancel frame is orderId-only, so the local id is
    /// looked up from the open-order cache; fails if `perm_id` is not tracked.
    fn cancel_order_by_perm_id(&self, py: Python<'_>, perm_id: i64) -> PyResult<()> {
        self.core.refuse_if_readonly("a cancel").map_err(PyRuntimeError::new_err)?;
        if perm_id == 0 {
            return self.report_refusal(py, -1, Refusal::validation(
                "cancel_order_by_perm_id: perm_id must be non-zero",
            ));
        }
        let shared = self.shared_state()?;
        let found = self.core.collect_open_orders(&shared)
            .into_iter()
            .find(|(_, tracked)| tracked.order.perm_id == perm_id)
            .map(|(oid, _)| oid);
        let Some(order_id) = found else {
            return self.report_refusal(py, -1, Refusal::validation(
                format!("cancel_order_by_perm_id: permId {perm_id} not found in open orders"),
            ));
        };
        self.cancel_order(py, order_id as i64, "")
    }

    /// Cancel all orders globally.
    fn req_global_cancel(&self, py: Python<'_>) -> PyResult<()> {
        self.core.refuse_if_readonly("a global cancel").map_err(PyRuntimeError::new_err)?;
        let Some(tx) = self.tx_or_report(-1) else { return Ok(()) };
        let shared = self.shared_state()?;
        let count = shared.market.instrument_count();
        for instrument in 0..count {
            let _ = Self::send_control(py, &tx, ControlCommand::Order(OrderRequest::CancelAll { instrument }));
        }
        Ok(())
    }

    /// Request next valid order ID.
    #[pyo3(signature = (num_ids=1))]
    fn req_ids(&self, py: Python<'_>, num_ids: i32) -> PyResult<()> {
        let next_id = self.next_order_id.load(Ordering::Relaxed) as i64;
        self.callback(py, "next_valid_id", (next_id,))?;
        let _ = num_ids;
        Ok(())
    }

    /// Get the next order ID (local counter, auto-increments).
    fn next_order_id(&self) -> i64 {
        self.next_order_id.fetch_add(1, Ordering::Relaxed) as i64
    }

    /// Request all open orders for this client.
    fn req_open_orders(&self, py: Python<'_>) -> PyResult<()> {
        let shared = self.shared_state()?;
        let orders = self.core.collect_open_orders(&shared);
        for (order_id, tracked) in &orders {
            let c_py = Py::new(py, Contract::from_api(&tracked.contract))?.into_any();
            let o = Order {
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
            };
            let o_py = Py::new(py, o)?.into_any();
            let state = super::super::contract::OrderState {
                status: tracked.status.clone(),
                ..Default::default()
            };
            let state_py = Py::new(py, state)?.into_any();
            self.wrapper.call_method(
                py, "open_order",
                (*order_id as i64, &c_py, &o_py, &state_py),
                None,
            )?;
            self.wrapper.call_method(
                py, "order_status",
                (*order_id as i64, tracked.status.as_str(), tracked.filled, tracked.remaining,
                 0.0f64, tracked.order.perm_id, tracked.order.parent_id, 0.0f64, 0i64, "", 0.0f64),
                None,
            )?;
        }
        self.callback(py, "open_order_end", ())?;
        Ok(())
    }

    /// Request all open orders across all clients.
    fn req_all_open_orders(&self, py: Python<'_>) -> PyResult<()> {
        self.req_open_orders(py)
    }

    /// Binding an order placed elsewhere to this session.
    ///
    /// The reference client asks a local process to hand over orders a person
    /// entered by hand in front of it. There is no such process here and no
    /// such person, so there is nothing to hand over, and this reports that
    /// rather than returning as though the binding were in place.
    ///
    /// Returning quietly was worse than either alternative: a caller that asked
    /// to be given those orders and was told nothing waits for orders that are
    /// never coming, with nothing to say why.
    #[pyo3(signature = (b_auto_bind))]
    fn req_auto_open_orders(&self, b_auto_bind: bool) -> PyResult<()> {
        if b_auto_bind {
            crate::python::compat::client::report_unserviceable(
                self,
                -1,
                "binding orders entered outside this session: there is no other \
                 process holding them",
            );
        }
        Ok(())
    }

    /// Request execution reports.
    #[pyo3(signature = (req_id, exec_filter=None))]
    fn req_executions(&self, py: Python<'_>, req_id: i64, exec_filter: Option<Py<PyAny>>) -> PyResult<()> {
        let filter = if let Some(ref fobj) = exec_filter {
            let get = |attr: &str| -> String {
                fobj.getattr(py, pyo3::types::PyString::new(py, attr))
                    .and_then(|v| v.extract::<String>(py))
                    .unwrap_or_default()
            };
            let get_i64 = |attr: &str| -> i64 {
                fobj.getattr(py, pyo3::types::PyString::new(py, attr))
                    .and_then(|v| v.extract::<i64>(py))
                    .unwrap_or_default()
            };
            ExecutionFilter {
                symbol: get("symbol"),
                sec_type: get("secType"),
                exchange: get("exchange"),
                side: get("side"),
                acct_code: get("acctCode"),
                // Dropping these silently replayed executions the caller had
                // filtered out — another client's fills, or ones before the
                // requested cutoff.
                client_id: get_i64("clientId"),
                time: get("time"),
            }
        } else {
            ExecutionFilter::default()
        };

        let snapshot = self.core.snapshot_executions(&filter);
        // Snapshot before any Python call: the callback runs with the GIL
        // held, and re-entering a path that locks `executions` would freeze
        // the interpreter, not just this thread (ibx#265).
        for se in snapshot {
            let c_py = Py::new(py, Contract::from_api(&se.contract))?.into_any();

            let exec_obj = Execution {
                unnamed_fields: se.execution.unnamed_fields.clone(),
                exec_id: se.execution.exec_id.clone(),
                time: se.execution.time.clone(),
                acct_number: se.execution.acct_number.clone(),
                exchange: se.execution.exchange.clone(),
                side: se.execution.side.clone(),
                shares: se.execution.shares,
                price: se.execution.price,
                perm_id: se.execution.perm_id,
                client_id: se.execution.client_id,
                order_id: se.execution.order_id,
                liquidation: se.execution.liquidation,
                cum_qty: se.execution.cum_qty,
                avg_price: se.execution.avg_price,
                order_ref: String::new(),
                ev_rule: se.execution.ev_rule.clone(),
                ev_multiplier: se.execution.ev_multiplier,
                model_code: se.execution.model_code.clone(),
                last_liquidity: se.execution.last_liquidity,
                pending_price_revision: se.execution.pending_price_revision,
            };
            let exec_py = Py::new(py, exec_obj)?.into_any();

            self.wrapper.call_method(
                py, "exec_details",
                (req_id, &c_py, &exec_py),
                None,
            )?;

            let report = CommissionAndFeesReport {
                exec_id: se.commission_and_fees.exec_id.clone(),
                commission_and_fees: se.commission_and_fees.commission_and_fees,
                currency: se.commission_and_fees.currency.clone(),
                realized_pnl: se.commission_and_fees.realized_pnl,
                yield_amount: se.commission_and_fees.yield_amount,
                yield_redemption_date: se.commission_and_fees.yield_redemption_date.clone(),
            };
            let report_py = Py::new(py, report)?.into_any();
            self.callback(py, "commission_and_fees_report", (&report_py,))?;
        }
        self.callback(py, "exec_details_end", (req_id,))?;
        Ok(())
    }

    /// Request completed orders.
    #[pyo3(signature = (api_only=false))]
    fn req_completed_orders(&self, py: Python<'_>, api_only: bool) -> PyResult<()> {
        let _ = api_only;
        // Bind the clone out of the guard first. A MutexGuard temporary in an
        // if-let scrutinee lives to the end of the body, so cloning alone does
        // not release it — a callback re-entering disconnect() would deadlock
        // on this same mutex (ibx#268).
        let shared = self.shared.lock().unwrap().clone();
        if let Some(shared) = shared {
            let completed = shared.orders.drain_completed_orders();
            for co in &completed {
                let status_str = crate::client_core::order_status_str(co.status);
                let rich_info = shared.orders.get_order_info(co.order_id);

                // Build OrderState iso with Rust API path (api/client/orders.rs:101-125):
                // start from rich_info.order_state when available, override status with the
                // canonical status_str, fall back to defaults otherwise.
                let state = if let Some(info) = rich_info.as_ref() {
                    let s = &info.order_state;
                    let allocations: Vec<super::super::contract::OrderAllocation> = s
                        .order_allocations.iter().map(|a| {
                            super::super::contract::OrderAllocation {
                                account: a.account.clone(),
                                position: a.position.clone(),
                                position_desired: a.position_desired.clone(),
                                position_after: a.position_after.clone(),
                                desired_alloc_qty: a.desired_alloc_qty.clone(),
                                allowed_alloc_qty: a.allowed_alloc_qty.clone(),
                                is_monetary: a.is_monetary,
                            }
                        }).collect();
                    super::super::contract::OrderState {
                        status: status_str.into(),
                        init_margin_before: s.init_margin_before.clone(),
                        maint_margin_before: s.maint_margin_before.clone(),
                        equity_with_loan_before: s.equity_with_loan_before.clone(),
                        init_margin_change: s.init_margin_change.clone(),
                        maint_margin_change: s.maint_margin_change.clone(),
                        equity_with_loan_change: s.equity_with_loan_change.clone(),
                        init_margin_after: s.init_margin_after.clone(),
                        maint_margin_after: s.maint_margin_after.clone(),
                        equity_with_loan_after: s.equity_with_loan_after.clone(),
                        commission_and_fees: s.commission_and_fees,
                        min_commission_and_fees: s.min_commission_and_fees,
                        max_commission_and_fees: s.max_commission_and_fees,
                        commission_and_fees_currency: s.commission_and_fees_currency.clone(),
                        warning_text: s.warning_text.clone(),
                        completed_time: s.completed_time.clone(),
                        completed_status: s.completed_status.clone(),
                        margin_currency: s.margin_currency.clone(),
                        init_margin_before_outside_rth: s.init_margin_before_outside_rth,
                        maint_margin_before_outside_rth: s.maint_margin_before_outside_rth,
                        equity_with_loan_before_outside_rth: s.equity_with_loan_before_outside_rth,
                        init_margin_change_outside_rth: s.init_margin_change_outside_rth,
                        maint_margin_change_outside_rth: s.maint_margin_change_outside_rth,
                        equity_with_loan_change_outside_rth: s.equity_with_loan_change_outside_rth,
                        init_margin_after_outside_rth: s.init_margin_after_outside_rth,
                        maint_margin_after_outside_rth: s.maint_margin_after_outside_rth,
                        equity_with_loan_after_outside_rth: s.equity_with_loan_after_outside_rth,
                        suggested_size: s.suggested_size.clone(),
                        reject_reason: s.reject_reason.clone(),
                        order_allocations: allocations,
                    }
                } else {
                    super::super::contract::OrderState {
                        status: status_str.into(),
                        ..Default::default()
                    }
                };
                let state_py = Py::new(py, state)?.into_any();

                let tracked = self.core.open_orders.lock().unwrap().get(&co.order_id).map(|o| {
                    (Contract::from_api(&o.contract), {
                        Order {
                            order_id: o.order.order_id,
                            action: o.order.action.clone(),
                            total_quantity: o.order.total_quantity,
                            order_type: o.order.order_type.clone(),
                            lmt_price: o.order.lmt_price,
                            aux_price: o.order.aux_price,
                            tif: o.order.tif.clone(),
                            account: o.order.account.clone(),
                            perm_id: o.order.perm_id,
                            // The value ibx#309 corrected; without it a completed
                            // order reads as entirely unfilled on this surface.
                            filled_quantity: o.order.filled_quantity,
                            ..Default::default()
                        }
                    })
                });
                if let Some((c, o)) = tracked {
                    let c_py = Py::new(py, c)?.into_any();
                    let o_py = Py::new(py, o)?.into_any();
                    self.callback(py, "completed_order", (&c_py, &o_py, &state_py))?;
                } else if let Some(info) = rich_info {
                    let c = Contract::from_api(&info.contract);
                    let o = Order {
                        order_id: info.order.order_id,
                        action: info.order.action,
                        total_quantity: info.order.total_quantity,
                        order_type: info.order.order_type,
                        lmt_price: info.order.lmt_price,
                        aux_price: info.order.aux_price,
                        tif: info.order.tif,
                        account: info.order.account,
                        perm_id: info.order.perm_id,
                        filled_quantity: info.order.filled_quantity,
                        ..Default::default()
                    };
                    let c_py = Py::new(py, c)?.into_any();
                    let o_py = Py::new(py, o)?.into_any();
                    self.callback(py, "completed_order", (&c_py, &o_py, &state_py))?;
                } else {
                    let c_py = Py::new(py, Contract::default())?.into_any();
                    let o_py = Py::new(py, Order::default())?.into_any();
                    self.callback(py, "completed_order", (&c_py, &o_py, &state_py))?;
                }
                // Bound `order_cache` growth: terminal entries are no longer
                // needed once delivered through `completed_order`.
                shared.orders.remove_order_info(co.order_id);
            }
            self.callback(py, "completed_orders_end", ())?;
        }
        Ok(())
    }
}
