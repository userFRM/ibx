//! Order placement, cancellation, open orders, executions, completed orders.

use std::sync::atomic::Ordering;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::types::model::{
    ExecutionFilter,
};
use crate::error_codes::Refusal;
use crate::client_core::ClientCore;
use crate::types::*;
use super::EClient;
use super::super::contract::{Contract, Order, OrderState, CommissionAndFeesReport, Execution};

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
        api_order.conditions = match order.convert_conditions(py) {
            Ok(conditions) => conditions,
            Err(why) => return self.report_refusal(py, order_id, Refusal::validation(why)),
        };
        // The three fields whose Python value is an object: the conversion
        // cannot read one without the interpreter, so they are filled here.
        // An object this client cannot read is a refusal, not an empty value:
        // read as absent, a leg goes out unpriced and a tag the protocol does
        // not carry stops being refused for stating it.
        api_order.order_combo_legs = match order.convert_order_combo_legs(py) {
            Ok(legs) => legs,
            Err(why) => return self.report_refusal(py, order_id, Refusal::validation(why)),
        };
        api_order.order_misc_options = match order.convert_misc_options(py) {
            Ok(options) => options,
            Err(why) => return self.report_refusal(py, order_id, Refusal::validation(why)),
        };
        // What the order path reads off a contract: where it is listed, its
        // legs, and the contract it hedges against. The legs and the hedge are
        // Python objects, so reading them needs the interpreter.
        let api_contract = crate::types::model::Contract {
            primary_exchange: contract.primary_exchange.clone(),
            combo_legs: contract.combo_legs_api(py).map_err(PyRuntimeError::new_err)?,
            // Every field is read from the caller's object. A delta or price
            // defaulted to zero hedges the order against nothing.
            delta_neutral_contract: match contract.delta_neutral_contract.as_ref() {
                None => None,
                Some(d) => {
                    let read = |name: &str| -> Result<f64, String> {
                        d.getattr(py, name)
                            .and_then(|v| v.extract(py))
                            .map_err(|e| format!("the hedging contract states no readable {name}: {e}"))
                    };
                    let hedge = (|| {
                        Ok::<_, String>(crate::types::model::DeltaNeutralContract {
                            con_id: d
                                .getattr(py, "conId")
                                .and_then(|v| v.extract(py))
                                .map_err(|e| format!("the hedging contract states no readable conId: {e}"))?,
                            delta: read("delta")?,
                            price: read("price")?,
                        })
                    })();
                    match hedge {
                        Ok(hedge) => Some(hedge),
                        Err(why) => {
                            return self.report_refusal(py, order_id, Refusal::validation(why))
                        }
                    }
                }
            },
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
        for (at, leg) in api_contract.combo_legs.iter().enumerate() {
            if let Err(why) = ClientCore::validate_leg(at, leg) {
                return self.report_refusal(py, order_id, Refusal::validation(why));
            }
        }
        if let Err(why) = ClientCore::validate_order_contract(
            contract.con_id,
            &contract.sec_type,
            &crate::types::model::contract_identity(
                &contract.last_trade_date_or_contract_month, contract.strike,
                &contract.right, &contract.multiplier, &contract.currency,
            ),
        ) {
            return self.report_refusal(py, order_id, why.into());
        }
        // The same guard the Rust surface applies. Without it here, whether a
        // caller is protected from an order the venue will refuse in silence
        // depends on which language they wrote in.
        if let Ok(shared) = self.shared_state()
            && let Err(why) = ClientCore::refuse_unpermitted_sec_type(
                &shared.reference.order_permissions(), &contract.sec_type,
            )
        {
            return self.report_refusal(py, order_id, why.into());
        }

        // An order names its contract by venue contract id. A caller who
        // states a description instead, as the reference client's examples do,
        // has it resolved here once the order itself validates. An order
        // carrying no id matches nothing and is answered by silence.
        //
        // That resolution is a request and an answer the first time, so this
        // call does not return until the venue has named the contract. The GIL
        // is released for the wait, so nothing else in the interpreter is held
        // up — but a caller placing an order from inside a wrapper callback
        // stalls its own dispatch loop, because that is the thread it is on. A
        // contract carrying conId resolves nothing and waits for nothing, and
        // so does one whose description the venue has already named: a program
        // placing a hundred orders on one contract asks about it once.
        let named;
        let contract = if contract.con_id == 0 && !contract.symbol.is_empty() {
            let key = crate::client_core::ClientCore::description_key(&contract.to_api());
            match self.qualify_once(py, contract, &key) {
                Ok(found) => { named = found; &named }
                // Reported under the code for the cause. An order refused
                // because the session ended is not code 200, which names a
                // contract the venue does not hold and invites a retry.
                Err(why) => return self.report_refusal(py, order_id, why),
            }
        } else {
            contract
        };

        // The number the caller stated, or a refusal. An id at or below zero
        // names no order the venue will hold, and one was handed out in its
        // place: the order went to the market under a number the caller had
        // never seen, so every status about it arrived under an id they were
        // not watching and their own cancel named nothing. The reference
        // client sends what it is given and the venue answers for it.
        let Some(oid) = u64::try_from(order_id).ok().filter(|id| *id > 0) else {
            return self.report_refusal(py, order_id, Refusal::validation(format!(
                "place_order: order_id {order_id} is not an order number;                  ask for one with next_order_id() or reqIds()",
            )));
        };

        let instrument = self.find_or_register_instrument(py, contract)?;

        // If orderId is already tracked, this is a modification — emit Modify instead
        // of Submit.
        let cmd = if self.core.is_order_tracked(oid) {
            // A replace carries the order id and its fields, not the contract, so
            // the order stays on the instrument it was placed on. A contract naming
            // a different instrument is refused rather than recorded.
            let placed_on = self.core.open_orders.lock().unwrap()
                .get(&oid)
                .map(|tracked| tracked.instrument);
            if placed_on.is_some_and(|placed_on| placed_on != instrument) {
                return self.report_refusal(py, order_id, Refusal::validation(format!(
                    "order {oid} is working on another contract, and a replace names \
                     the order rather than the contract: withdraw it and place a new \
                     order to trade {}",
                    contract.symbol,
                )));
            }
            // A replace states the order type, the limit price and the trigger.
            // An order defined by anything else cannot survive one.
            if let Some(refusal) = self.core.modify_refusal(oid, &api_order) {
                return self.report_refusal(py, order_id, refusal.into());
            }
            let price = crate::types::price_from_f64(api_order.lmt_price);
            let qty = crate::types::qty_from_f64(api_order.total_quantity);
            // A stop's trigger rides on aux_price, exactly as it does on the
            // submit path.
            let stop_price = crate::types::price_from_f64(api_order.aux_price);
            ControlCommand::Order(OrderRequest::Modify {
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
        // As on the other surface: an order that does not transmit is built
        // and kept, and one that does sends whatever of its family was kept
        // before sending itself.
        if api_order.transmit {
            for waiting in self.core.release_before(oid, api_order.parent_id) {
                Self::send_control(py, &tx, waiting)?;
            }
            Self::send_control(py, &tx, cmd)?;
        } else {
            self.core.hold_until_transmitted(oid, api_order.parent_id, cmd);
        }

        // Track order in shared core
        let api_contract = contract.to_api();
        let mut tracked_order = api_order.clone();
        tracked_order.order_id = oid as i64;
        self.core.cache_contract(contract.con_id, api_contract.clone());
        self.core.track_order(oid, api_contract, tracked_order, instrument);

        Ok(())
    }

    /// Exercise or lapse a long option position.
    ///
    /// `exercise_action` is 1 to exercise and 2 to lapse; anything else is
    /// refused.
    ///
    /// `_override` is taken and not sent, because no tag carries it: it names
    /// a check made before the order is built, not one the venue makes. The
    /// check it names is real — it is what stops an exercise of an option out
    /// of the money and a lapse of one in it — and this client does not make
    /// it, because what it rests on is the venue's word on where the option
    /// stands, which this client does not ask for. An instruction is sent as
    /// given; passing `0` says so in the log and changes nothing else.
    #[pyo3(signature = (req_id, contract, exercise_action, exercise_quantity, account, _override))]
    fn exercise_options(
        &self, py: Python<'_>, req_id: i64, contract: &Contract, exercise_action: i32,
        exercise_quantity: i32, account: &str, _override: i32,
    ) -> PyResult<()> {
        self.core.refuse_if_readonly("an exercise").map_err(PyRuntimeError::new_err)?;
        if _override == 0 {
            log::warn!(
                "exercise of {} asked to stop short of an option out of the money, and \
                 this client does not know where the option stands: the instruction is \
                 sent as given",
                contract.symbol,
            );
        }
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
            &crate::types::model::contract_identity(
                &contract.last_trade_date_or_contract_month, contract.strike,
                &contract.right, &contract.multiplier, &contract.currency,
            ),
        ) {
            return self.report_refusal(py, req_id, why.into());
        }

        let oid = if req_id > 0 {
            req_id as u64
        } else {
            self.take_order_id(py)
        };
        let instrument = self.find_or_register_instrument(py, contract)?;
        Self::send_control(py, &tx, ControlCommand::Order(
            ClientCore::build_exercise_request(oid, instrument, action, crate::types::qty_from_wire(qty as i64)),
        ))
    }

    /// Cancel an order.
    ///
    /// `manual_order_cancel_time` is taken and not applied. A cancel on this
    /// wire names five fields and no time among them.
    #[pyo3(signature = (order_id, manual_order_cancel_time=""))]
    fn cancel_order(&self, py: Python<'_>, order_id: i64, manual_order_cancel_time: &str) -> PyResult<()> {
        self.core.refuse_if_readonly("a cancel").map_err(PyRuntimeError::new_err)?;
        let Some(tx) = self.tx_or_report(order_id) else { return Ok(()) };
        // As `place_order`. A negative id read as unsigned is a number above
        // nine quintillion, and the cancel names it.
        let Some(oid) = u64::try_from(order_id).ok().filter(|id| *id > 0) else {
            return self.report_refusal(py, order_id, Refusal::validation(format!(
                "cancel_order: order_id {order_id} is not an order number",
            )));
        };
        Self::send_control(py, &tx, ControlCommand::Order(OrderRequest::Cancel { order_id: oid }))?;
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
        // Every failed send is counted and reported. A caller answered without
        // an error takes the account to be flat, and a send that did not reach
        // the engine withdrew nothing.
        let mut unsent = 0usize;
        for instrument in 0..count {
            if Self::send_control(py, &tx, ControlCommand::Order(OrderRequest::CancelAll { instrument })).is_err() {
                unsent += 1;
            }
        }
        if unsent > 0 {
            return Err(PyRuntimeError::new_err(format!(
                "a global cancel reached the engine for {} of {count} instruments; \
                 the rest were not sent, so orders on them are still working",
                count as usize - unsent,
            )));
        }
        Ok(())
    }

    /// Request next valid order ID.
    ///
    /// `num_ids` is taken and not applied. Ids are handed out one at a time
    /// here, as the reference client does whatever number is asked for.
    ///
    /// Before a session exists there is no counter to answer from: the id an
    /// account may next use is the venue's to state. Answering announces zero,
    /// which names no order the venue will hold and is refused on placement.
    /// Reported the way the reference client reports every request made before
    /// connecting.
    #[pyo3(signature = (num_ids=1))]
    fn req_ids(&self, py: Python<'_>, num_ids: i32) -> PyResult<()> {
        let Some(_connected) = self.tx_or_report(-1) else { return Ok(()) };
        // As `take_order_id` does: the mark this is read off is raised by a
        // replay that lands after the connection does.
        self.wait_for_the_replay(py);
        self.deliver(py, "next_valid_id", (self.stated_order_id() as i64,))?;
        let _ = num_ids;
        Ok(())
    }

    /// Get the next order ID (local counter, auto-increments).
    fn next_order_id(&self, py: Python<'_>) -> i64 {
        self.take_order_id(py) as i64
    }

    /// The first id past everything the account has used that a request can
    /// also carry.
    ///
    /// A caller that numbers its orders and its requests out of one counter —
    /// which is how the client this one stands in for is written — needs both
    /// at once: clear of every id an order has spent, and inside the four
    /// billion a request is carried in. An account that has been given a wider
    /// order id than that has no such number above it, so this answers with
    /// the widest the account has used that a request can carry, and the
    /// counting goes on from there.
    fn next_shared_id(&self, py: Python<'_>) -> PyResult<i64> {
        self.wait_for_the_replay(py);
        let Ok(shared) = self.shared_state() else { return Ok(1) };
        let next = shared.orders.narrow_id_watermark() + 1;
        // One past the widest carryable id is not itself carryable, and nor is
        // anything this client has reserved. Handing one back would number a
        // request that is answered to nobody, so the caller is told instead.
        crate::api::client::wire_req_id(next as i64)
            .map(|_| next as i64)
            .map_err(|refusal| PyRuntimeError::new_err(format!(
                "this account has no order id left that a request can also carry: {}",
                refusal.message,
            )))
    }

    /// Request all open orders for this client.
    fn req_open_orders(&self, py: Python<'_>) -> PyResult<()> {
        let shared = self.shared_state()?;
        // The venue names the working orders unprompted after a connect.
        // Answering before that replay lands reports none of them, and a
        // caller that reads "nothing" places the same order twice. Bounded at
        // 3s: an account with nothing working never sees the replay end.
        for _ in 0..300 {
            if shared.orders.replay_done() { break; }
            py.detach(|| std::thread::sleep(std::time::Duration::from_millis(10)));
        }
        let orders = self.core.collect_open_orders(&shared);
        for (order_id, tracked) in &orders {
            let c_py = Py::new(py, Contract::from_api(&tracked.contract))?.into_any();
            let o = Order::from_api(py, &tracked.order, self.client_id.load(Ordering::Acquire))?;
            let o_py = Py::new(py, o)?.into_any();
            let state = super::super::contract::OrderState {
                status: tracked.status.clone(),
                ..Default::default()
            };
            let state_py = Py::new(py, state)?.into_any();
            self.deliver(py, "open_order", (*order_id as i64, &c_py, &o_py, &state_py))?;
            self.deliver(py, "order_status",
                (*order_id as i64, tracked.status.as_str(), tracked.filled, tracked.remaining,
                 0.0f64, tracked.order.perm_id, tracked.order.parent_id, 0.0f64,
                 self.client_id.load(Ordering::Acquire) as i64, "", 0.0f64))?;
        }
        self.deliver(py, "open_order_end", ())?;
        Ok(())
    }

    /// Request all open orders across all clients.
    ///
    /// The same answer as `req_open_orders`. The reference client splits the
    /// two by client id; this wire carries no client id on an order, so the
    /// venue names the orders on the account without stating who entered them.
    /// A subset would be an attribution the venue does not supply.
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
    /// Reported rather than returning silently: a caller told nothing waits
    /// for orders that will not arrive.
    ///
    /// `b_auto_bind` is taken and not applied. Whether it asks to bind or to
    /// stop binding, the answer is the same: this session hears about every
    /// order on the account either way.
    #[pyo3(signature = (b_auto_bind))]
    fn req_auto_open_orders(&self, b_auto_bind: bool) -> PyResult<()> {
        // Nothing goes to the wire. The request is refused for any client id
        // but 0, and otherwise sets state that does not apply here: this
        // session is told about every order on the account whether or not it
        // placed them. The refusal is the only observable part.
        if self.client_id.load(std::sync::atomic::Ordering::Acquire) != 0 {
            crate::python::compat::client::stubs::report_unserviceable_with(
                self,
                -1,
                crate::error_codes::Refusal::AUTO_BIND_NOT_THIS_CLIENT,
                "orders entered elsewhere are bound to the client numbered zero",
            );
        }
        let _ = b_auto_bind;
        Ok(())
    }

    /// Request execution reports.
    ///
    /// Before a session exists this is reported on the error callback, as
    /// every other request made before connecting is. Answered instead, the
    /// answer waits for a dispatch pass no session is there to make, and the
    /// caller hears nothing at all.
    #[pyo3(signature = (req_id, exec_filter=None))]
    fn req_executions(&self, py: Python<'_>, req_id: i64, exec_filter: Option<Py<PyAny>>) -> PyResult<()> {
        let Some(_connected) = self.tx_or_report(req_id) else { return Ok(()) };
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
                // Stored executions carry the venue's word for the side, and a
                // filter states the order action. Compared as written, a filter
                // for buys matches nothing.
                side: match get("side").to_ascii_uppercase().as_str() {
                    "BUY" => "BOT".to_string(),
                    "SELL" | "SSHORT" => "SLD".to_string(),
                    other => other.to_string(),
                },
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
        // the interpreter, not just this thread.
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
                order_ref: se.execution.order_ref.clone(),
                ev_rule: se.execution.ev_rule.clone(),
                ev_multiplier: se.execution.ev_multiplier,
                model_code: se.execution.model_code.clone(),
                last_liquidity: se.execution.last_liquidity,
                pending_price_revision: se.execution.pending_price_revision,
            };
            let exec_py = Py::new(py, exec_obj)?.into_any();

            self.deliver(py, "exec_details", (req_id, &c_py, &exec_py))?;

            let report = CommissionAndFeesReport {
                exec_id: se.commission_and_fees.exec_id.clone(),
                commission_and_fees: se.commission_and_fees.commission_and_fees,
                currency: se.commission_and_fees.currency.clone(),
                realized_pnl: se.commission_and_fees.realized_pnl,
                yield_amount: se.commission_and_fees.yield_amount,
                yield_redemption_date: se.commission_and_fees.yield_redemption_date.clone(),
            };
            let report_py = Py::new(py, report)?.into_any();
            self.deliver(py, "commission_and_fees_report", (&report_py,))?;
        }
        self.deliver(py, "exec_details_end", (req_id,))?;
        Ok(())
    }

    /// Request completed orders.
    ///
    /// `api_only` is taken and not applied. It asks for orders entered through
    /// an API rather than by hand, and nothing this client holds says which an
    /// order was: the completed orders are the ones this session saw, and the
    /// venue states no origin on them. Passing `true` is answered with all of
    /// them rather than with a guess at which were typed.
    #[pyo3(signature = (api_only=false))]
    fn req_completed_orders(&self, py: Python<'_>, api_only: bool) -> PyResult<()> {
        let _ = api_only;
        // Bind the clone out of the guard first. A MutexGuard temporary in an
        // if-let scrutinee lives to the end of the body, so cloning alone does
        // not release it — a callback re-entering disconnect() would deadlock
        // on this same mutex.
        let shared = self.shared.lock().unwrap().clone();
        if let Some(shared) = shared {
            // Read off the queue once and kept. It empties as it is read and
            // the venue does not send these again, so a second request would
            // otherwise be answered with none of them, and with default objects
            // for any whose record has been retired.
            {
                let mut archive = self.completed.lock().unwrap();
                for co in shared.orders.drain_completed_orders() {
                    let status_str = crate::types::order_status::order_status_str(co.status);
                    let rich_info = shared.orders.get_order_info(co.order_id);
                    let tracked = self.core.open_orders.lock().unwrap().get(&co.order_id).cloned();
                    // The state the venue stated where it stated one, under the
                    // status this client names it by, which is canonical rather
                    // than whatever the stored state last held.
                    let state = crate::types::model::OrderState {
                        status: status_str.into(),
                        ..rich_info.as_ref().map(|i| i.order_state.clone()).unwrap_or_default()
                    };
                    let (contract, order) = match (tracked, rich_info) {
                        (Some(o), _) => (o.contract, o.order),
                        (None, Some(info)) => (info.contract, info.order),
                        (None, None) => (
                            crate::types::model::Contract::default(),
                            crate::types::model::Order {
                                order_id: co.order_id as i64,
                                ..Default::default()
                            },
                        ),
                    };
                    archive.push((contract, order, state));
                    // Bound `order_cache` growth: terminal entries are no
                    // longer needed once what they carried has been read out.
                    // Handed to the side that reads the fills rather than
                    // freed here. That side is the only one that can tell when
                    // a record is finished with: a fill taken off the queue but
                    // not yet reported still needs it, and from here looks like
                    // no fill at all.
                    self.deferred_evictions.lock().unwrap().insert(co.order_id);
                }
            }
            // Copied before anything is called back: a callback may ask for
            // these again, and the lock is not re-entrant.
            let completed = self.completed.lock().unwrap().clone();
            for (contract, order, state) in &completed {
                let c_py = Py::new(py, Contract::from_api(contract))?.into_any();
                let o_py = Py::new(
                    py,
                    Order::from_api(py, order, self.client_id.load(Ordering::Acquire))?,
                )?.into_any();
                let state_py = Py::new(py, OrderState::from_api(state))?.into_any();
                self.deliver(py, "completed_order", (&c_py, &o_py, &state_py))?;
            }
            self.deliver(py, "completed_orders_end", ())?;
        }
        Ok(())
    }
}
