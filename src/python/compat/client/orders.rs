//! Order placement, cancellation, open orders, executions, completed orders.

use std::sync::atomic::Ordering;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::types::model::{
    ExecutionFilter,
};
use crate::error_codes::{DUPLICATE_ORDER_ID, Refusal};
use crate::client_core::ClientCore;
use crate::types::*;
use super::EClient;
use super::super::contract::{Contract, Order, OrderState, CommissionAndFeesReport, Execution};

/// What a withdrawal states about itself that this wire cannot carry.
///
/// `None` where it states nothing — no object, or one left as it comes. The
/// reference client's object holds three fields and a cancel on this wire names
/// five, none of them these, so anything stated has nowhere to go. Read by
/// attribute, as every object a caller fills in is read here, and a plain
/// string is taken as the time on its own.
fn withdrawal_states(py: Python<'_>, order_cancel: Option<&Py<PyAny>>) -> Option<String> {
    let held = order_cancel?;
    if let Ok(time) = held.extract::<String>(py) {
        return (!time.is_empty()).then(|| WITHDRAWAL_CARRIES.replace("{field}", "a time"));
    }
    let says = |attr: &str| -> bool {
        held.getattr(py, attr)
            .ok()
            .filter(|v| !v.is_none(py))
            .and_then(|v| v.extract::<String>(py).ok())
            .is_some_and(|text| !text.is_empty())
    };
    for (attr, named) in [
        ("manualOrderCancelTime", "a time"),
        ("extOperator", "an operator"),
    ] {
        if says(attr) {
            return Some(WITHDRAWAL_CARRIES.replace("{field}", named));
        }
    }
    // The indicator is a number, and the one it carries when nobody set it is
    // the number this protocol writes for an integer nobody set.
    let indicator = held.getattr(py, "manualOrderIndicator").ok()
        .and_then(|v| v.extract::<i32>(py).ok());
    match indicator {
        Some(i) if i != i32::MAX => Some(WITHDRAWAL_CARRIES.replace("{field}", "who entered it")),
        _ => None,
    }
}

/// Said whichever of the three the caller stated.
const WITHDRAWAL_CARRIES: &str = "a withdrawal states {field}, and this protocol \
     carries no field for it: the cancel names five and none of them is that one, so the \
     order would be withdrawn without it. Withdraw it without stating one, or state it \
     where the order was placed.";

#[pymethods]
impl EClient {
    /// Place an order.
    ///
    /// A request the client will not send is reported under the number the
    /// reference client reports it under, and the call returns. A program
    /// moved from that client has an `error` handler and no exception
    /// handling around a request, because nothing it was written against
    /// raises there. A send the engine can no longer take is reported the
    /// same way, stating what has already reached the engine and what has
    /// not.
    fn place_order(&self, py: Python<'_>, order_id: i64, contract: &Contract, order: &Order) -> PyResult<()> {
        self.core.refuse_if_readonly("an order").map_err(PyRuntimeError::new_err)?;
        let Some(tx) = self.tx_or_report_for_trading(order_id)? else { return Ok(()) };

        if let Err(why) = ClientCore::validate_order_destination(&contract.exchange) {
            return self.report_refusal(py, order_id, why.into());
        }

        // Convert and validate order params first (fail fast, no connection needed)
        let mut api_order = order.to_api();
        api_order.conditions = match order.convert_conditions(py) {
            Ok(conditions) => conditions,
            Err(why) => return self.report_refusal(py, order_id, Refusal::validation(why)),
        };
        // The fields whose Python value is a list of objects: the conversion
        // cannot read one without the interpreter, so they are filled here.
        // An object this client cannot read is a refusal, not an empty value:
        // read as absent, a leg goes out unpriced, an algo runs on the venue's
        // defaults, and a tag the protocol does not carry stops being refused
        // for stating it.
        api_order.order_combo_legs = match order.convert_order_combo_legs(py) {
            Ok(legs) => legs,
            Err(why) => return self.report_refusal(py, order_id, Refusal::validation(why)),
        };
        api_order.order_misc_options = match order.convert_misc_options(py) {
            Ok(options) => options,
            Err(why) => return self.report_refusal(py, order_id, Refusal::validation(why)),
        };
        api_order.algo_params = match order.convert_algo_params(py) {
            Ok(params) => params,
            Err(why) => return self.report_refusal(py, order_id, Refusal::validation(why)),
        };
        api_order.smart_combo_routing_params = match order.convert_smart_combo_routing_params(py) {
            Ok(params) => params,
            Err(why) => return self.report_refusal(py, order_id, Refusal::validation(why)),
        };
        // What the order path reads off a contract: where it is listed, its
        // legs, and the contract it hedges against. The legs and the hedge are
        // Python objects, so reading them needs the interpreter.
        let api_contract = crate::types::model::Contract {
            primary_exchange: contract.primary_exchange.clone(),
            // A leg this client cannot read is a refusal, like the other
            // fields read off the caller's objects above, and is reported
            // the same way.
            combo_legs: match contract.combo_legs_api(py) {
                Ok(legs) => legs,
                Err(why) => return self.report_refusal(py, order_id, Refusal::validation(why)),
            },
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

        // The number this call is spending, so the allocator does not hand it
        // out again. It counts from the highest the venue has named, and the
        // venue has not named this one yet — a program keeping its own numbers
        // off `next_valid_id`, which is the reference client's own idiom, then
        // asked for one and was given a number it had put on the market
        // moments before.
        self.next_order_id.fetch_max(oid + 1, Ordering::AcqRel);

        // If orderId already names a working order, this is a modification —
        // emit Modify instead of Submit. Settled before anything is
        // registered, as on the other surface: asking for a slot first spent
        // one of the table's on a contract this call then refused, and the
        // table does not grow.
        let venue = self.shared_state().ok();
        // Before the slot a tracked order holds is compared with the one this
        // contract is cached under: the engine may have given that slot back.
        if let Some(shared) = venue.as_deref() {
            self.core.forget_released_slots(shared);
        }
        let replacing = self.core.is_working_at_the_venue(oid, venue.as_deref());
        // A number the venue has already worked an order under names nothing
        // now, so this placement is not a revision -- and the venue refuses a
        // repeated number only while it is still working one, so after a fill
        // it takes it as a new order. A caller retrying what it believed had
        // failed was given a second live order.
        if !replacing && self.core.the_number_is_spent(oid) {
            return self.report_refusal(py, oid as i64, Refusal::stated(
                DUPLICATE_ORDER_ID,
                format!(
                    "order {oid} has already been worked and finished: place a new \
                     order under a number of its own",
                ),
            ));
        }
        // A replace carries the order id and its fields, not the contract, so
        // the order stays on the instrument it was placed on. A contract
        // naming a different instrument is refused rather than recorded.
        let placed_on = if replacing { self.core.tracked_instrument(oid) } else { None };
        let venue_now = venue.as_deref();
        let wrong_contract = || Refusal::validation(format!(
            "order {oid} is working on another contract, and a replace names \
             the order rather than the contract: withdraw it and place a new \
             order to trade {}",
            contract.symbol,
        ));
        // A registration the engine does not answer — a wait that ran out or
        // an engine that went away mid-request — is a refusal, and reported
        // as one: nothing this call sends has anywhere to go.
        let instrument = match placed_on {
            Some(placed_on) if contract.con_id != 0 && venue_now.is_some() => {
                if self.core.cached_instrument(venue_now.unwrap(), contract.con_id)
                    != Some(placed_on)
                {
                    return self.report_refusal(py, order_id, wrong_contract());
                }
                placed_on
            }
            _ => {
                // An order the venue replayed holds no slot here, and the
                // check above has nothing to compare. The venue's own book
                // names the contract it is on, and a replace names the order
                // rather than the contract — so one naming another contract is
                // refused rather than recorded against it, and rather than
                // spending a slot on a contract nothing needs.
                if replacing
                    && let Some(known) =
                        venue_now.and_then(|v| v.orders.get_order_info(oid))
                    && !ClientCore::names_the_same_contract(&known.contract, &api_contract)
                {
                    return self.report_refusal(py, order_id, wrong_contract());
                }
                match self.find_or_register_instrument(py, contract) {
                    Ok(instrument) => instrument,
                    Err(why) => return self.report_refusal(py, order_id, why),
                }
            }
        };

        let cmd = if replacing {
            if placed_on.is_some_and(|placed_on| placed_on != instrument) {
                return self.report_refusal(py, order_id, wrong_contract());
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
        //
        // The family goes out as one thing or not at all, as far as the
        // engine will allow: it stays held while it is sent, and each member
        // leaves the hold only once its send is accounted for. A send that
        // fails partway otherwise left the parent live at the venue with its
        // protective children never sent, and the caller saw only that the
        // call had failed. What reached the engine and what did not is said
        // on the error callback, under the number a lost session is reported
        // under, and the call returns.
        // Track order in shared core. The record is the contract as the venue
        // was told it: the description, and the legs and the hedge read off
        // the caller's objects above. Converted afresh, the record held a BAG
        // with nothing in it, and that is what every callback handed back.
        let api_contract = crate::types::model::Contract {
            combo_legs: api_contract.combo_legs,
            delta_neutral_contract: api_contract.delta_neutral_contract,
            ..contract.to_api()
        };
        let mut tracked_order = api_order.clone();
        tracked_order.order_id = oid as i64;
        // The client this order goes out under. Left at nought, the order
        // read back could not be told from one placed under client zero, and
        // restating one as the other collides with whatever is held there.
        tracked_order.client_id = self.client_id.load(Ordering::Acquire);
        self.core.cache_contract(contract.con_id, api_contract.clone());
        // The record of a placement goes down before the command, as on the
        // other surface. Written afterwards, the engine could acknowledge the
        // order — or refuse and retire it — while there was nothing here to
        // record it against, and the insertion behind that put a fresh
        // PendingSubmit over the venue's own word, or brought back an order
        // the refusal had already taken out. A held placement takes its record
        // in the same step as the hold: the two apart were read between, and a
        // withdrawal that found the hold, took it and found no record reported
        // the order gone while the record was written behind it.
        //
        // A replace restates an order the venue is already working, so it
        // states new terms and not a new order: recorded as one, a partly
        // filled order came back as pending with nothing filled and its whole
        // quantity outstanding. Its record goes down first for the same
        // reason a placement's does — the venue can answer the replace before
        // this call returns, and a restatement written behind that answer put
        // the attempted terms over a refusal that had already put back the
        // real ones.
        if replacing {
            // Whether or not the session state is still here. Skipped where it
            // was not, the record went unchanged for a change that did go out
            // — and the undo on the send-failure path below still ran, putting
            // back terms from before a change the venue may have taken.
            self.core.restate_order(
                venue_now, oid, api_contract.clone(), tracked_order.clone(), instrument,
            );
        }
        if api_order.transmit {
            if !replacing {
                self.core.track_order(
                    oid, api_contract.clone(), tracked_order.clone(), instrument,
                );
            }
            let sent = self.core.transmit_family(oid, api_order.parent_id, cmd, |c| {
                Self::send_control(py, &tx, c).is_ok()
            });
            if let Err(why) = sent {
                // Nothing left, so nothing was restated.
                if replacing { self.core.undo_restatement(oid); }
                return self.report_refusal(py, order_id, Refusal::not_connected(why));
            }
        } else if replacing {
            self.core.hold_until_transmitted(oid, api_order.parent_id, cmd);
        } else {
            self.core.hold_and_track(
                oid, api_order.parent_id, cmd,
                api_contract.clone(), tracked_order.clone(), instrument,
            );
        }

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
    #[pyo3(signature = (req_id, contract, exercise_action, exercise_quantity, account, _override,
                        manual_order_time="", customer_account="", professional_customer=false))]
    fn exercise_options(
        &self, py: Python<'_>, req_id: i64, contract: &Contract, exercise_action: i32,
        exercise_quantity: i32, account: &str, _override: i32,
        manual_order_time: &str, customer_account: &str, professional_customer: bool,
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
        let Some(tx) = self.tx_or_report_for_trading(req_id)? else { return Ok(()) };
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
        let instrument = self.find_or_register_instrument(py, contract)
            .map_err(|why| PyRuntimeError::new_err(why.message))?;
        Self::send_control(py, &tx, ControlCommand::Order(
            ClientCore::build_exercise_request(
                oid, instrument, action, crate::types::qty_from_wire(qty as i64),
                crate::client_core::ExerciseStates {
                    manual_order_time: manual_order_time.to_string(),
                    customer_account: customer_account.to_string(),
                    professional_customer,
                },
            ),
        ))
    }

    /// Cancel an order.
    ///
    /// The second argument is what the reference client states about the
    /// withdrawal itself — when a person entered it, on whose authority, and
    /// whether a person entered it at all. It is taken as that object or as
    /// the time alone, which is how this client took it before.
    ///
    /// A cancel on this wire names five fields and none of those is among
    /// them, so what the caller stated cannot travel. The cancel goes anyway
    /// and the caller is told the annotation did not: refused outright, a live
    /// order was left standing over a record the wire has no room for, and the
    /// client this one stands in for withdraws it — it states all three on
    /// every cancel it sends. Taken silently it would be withdrawn under
    /// nobody's name while the caller had given one, so it is said.
    #[pyo3(signature = (order_id, order_cancel=None))]
    fn cancel_order(&self, py: Python<'_>, order_id: i64, order_cancel: Option<Py<PyAny>>) -> PyResult<()> {
        self.core.refuse_if_readonly("a cancel").map_err(PyRuntimeError::new_err)?;
        let uncarried = withdrawal_states(py, order_cancel.as_ref());
        let Some(tx) = self.tx_or_report_for_trading(order_id)? else { return Ok(()) };
        // As `place_order`. A negative id read as unsigned is a number above
        // nine quintillion, and the cancel names it.
        let Some(oid) = u64::try_from(order_id).ok().filter(|id| *id > 0) else {
            return self.report_refusal(py, order_id, Refusal::validation(format!(
                "cancel_order: order_id {order_id} is not an order number",
            )));
        };
        // Said before the cancel goes, so a caller reading its callbacks in
        // order learns what will not travel before it is told the order went.
        if let Some(stated) = uncarried {
            self.say_the_annotation_did_not_travel(py, order_id, stated)?;
        }
        // An order still held never reached the venue, so withdrawing it is
        // forgetting a command rather than sending one, as it is on the other
        // surface. Sent, the venue answers that it knows no such order and the
        // command stays queued to go out behind the next thing that transmits:
        // a caller that cancelled a parent and then sent its stop-loss had the
        // parent it had cancelled placed for it. The record goes with the
        // command, so the id stops reading as a working order's.
        // Only where what is held would have placed the order. A revision of
        // an order the venue is already working can also be waiting to be
        // transmitted, and forgetting that and returning left the live order
        // working while the caller had been told it was withdrawn: the staged
        // revision goes, and the cancel still travels.
        if self.core.withdraw_held_placement(oid) {
            return Ok(());
        }
        Self::send_control(py, &tx, ControlCommand::Order(OrderRequest::Cancel { order_id: oid }))
    }

    /// Say that a withdrawal's annotation has nowhere to go, without stopping
    /// the withdrawal.
    ///
    /// The order still comes back. A record of who withdrew it and when is a
    /// regulatory one, and losing it matters — but not as much as a live order
    /// left working because the record could not be filed, which is what
    /// refusing did.
    fn say_the_annotation_did_not_travel(
        &self, py: Python<'_>, order_id: i64, stated: String,
    ) -> PyResult<()> {
        self.report_refusal(py, order_id, Refusal::validation(stated))
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
        let Some(_tx) = self.tx_or_report_for_trading(-1)? else { return Ok(()) };
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
        self.cancel_order(py, order_id as i64, None)
    }

    /// Cancel every order the account is working.
    ///
    /// This wire carries no request to withdraw everything, so it is composed
    /// here: one cancel for each order held, which is what a caller asking for
    /// everything back is asking for. What is held is what the venue named as working at connect and
    /// what this session placed since. The venue names the former after the
    /// connect returns, so a global cancel issued straight away waits for that
    /// naming, as asking for the open orders does, and covers what was named.
    /// Where the naming does not finish within the wait, what had been named
    /// is still withdrawn and the call says so rather than returning as
    /// though every order were covered: a partial cancel that reads as one
    /// beats the same cancel in silence, which reads as a complete answer.
    /// The same where the naming did finish and an order it named could not be
    /// given a slot in this client's instrument table — the engine holds no
    /// record of such an order, so no cancel here names it and it goes on
    /// working at the venue.
    #[pyo3(signature = (order_cancel=None))]
    fn req_global_cancel(&self, py: Python<'_>, order_cancel: Option<Py<PyAny>>) -> PyResult<()> {
        self.core.refuse_if_readonly("a global cancel").map_err(PyRuntimeError::new_err)?;
        if let Some(stated) = withdrawal_states(py, order_cancel.as_ref()) {
            self.say_the_annotation_did_not_travel(py, -1, stated)?;
        }
        let Some(tx) = self.tx_or_report_for_trading(-1)? else { return Ok(()) };
        // Everything held goes with everything working: an order the venue was
        // never given is withdrawn by forgetting it, and left queued it would
        // go out behind the next thing that transmits — after the caller had
        // asked for every order to be taken back.
        self.core.withdraw_all_held();
        let named = self.wait_for_the_replay(py);
        let shared = self.shared_state()?;
        // One request per instrument the engine holds an order on. The count
        // is the engine's, mirrored: a contract the venue named an order on
        // counts whether or not this session ever subscribed to it.
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
        // An order the venue named that could not be given a slot in this
        // client's instrument table is in none of those requests: the engine
        // holds no record of it to compose a cancel from, and it is still
        // working there. Said before the naming is judged, because this is an
        // omission that happened rather than one that may have.
        let unheld = shared.orders.orders_without_a_slot();
        if unheld > 0 {
            return self.report_refusal(py, -1, Refusal::no_answer(format!(
                "{unheld} of this account's working orders have no slot in this client's \
                 instrument table, so no cancel was composed for them and they are still \
                 working; the {count} that were sent cover the rest",
            )));
        }
        // Only where the venue had begun naming and not finished. An account
        // working nothing is named with nothing, and the record that ends the
        // naming cannot be told from the one that precedes it, so an empty
        // account never sees it finish — warning there would cry wolf on every
        // withdrawal against an idle account.
        if !named && shared.orders.naming_began() {
            // Said to the caller rather than the log: what had been named
            // went, and what had not been named is not covered. A silent
            // partial cancel is the worst thing this call can do. Reported
            // on the error callback and the call returns, the way a refusal
            // is reported.
            return self.report_refusal(py, -1, Refusal::no_answer(format!(
                "the venue had not finished naming this account's working orders within \
                 the wait: {count} cancels were sent for what had been named, and what had \
                 not been named is not covered and may still be working",
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
        let Some(_connected) = self.tx_or_report(-1)? else { return Ok(()) };
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
        let Some(_tx) = self.tx_or_report(-1)? else { return Ok(()) };
        // The venue names the working orders unprompted after a connect.
        // Answering before that replay lands reports none of them, and a
        // caller that reads "nothing" places the same order twice.
        let named = self.wait_for_the_replay(py);
        let shared = self.shared_state()?;
        // Only where the venue had begun naming and not finished. An account
        // working nothing is named with nothing, and the record that ends the
        // naming cannot be told from the one that precedes it, so an empty
        // account never sees it finish — reporting there would cry wolf on
        // every reading of an idle account.
        if !named && shared.orders.naming_began() {
            // Said to the caller rather than only to the log, and said ahead of
            // the orders: what follows is what had arrived, which is otherwise
            // indistinguishable from an account with nothing working, and a
            // caller reading it as the whole set places what it already has on.
            self.report_refusal(py, -1, Refusal::no_answer(
                "the venue had not finished naming this account's working orders within \
                 the wait, so what follows is what had arrived rather than what is working",
            ))?;
        }
        // And where an order the venue named could not be given a slot in the
        // instrument table. It is listed below, from the order cache, and the
        // engine holds no record of it: no fill on it is booked, no status
        // change on it is announced, and a withdrawal of everything does not
        // reach it. Listed without that said, it reads as an order this
        // session is following.
        let unheld = shared.orders.orders_without_a_slot();
        if unheld > 0 {
            self.report_refusal(py, -1, Refusal::no_answer(format!(
                "{unheld} of the orders below have no slot in this client's instrument \
                 table and are absent from the engine's book: their fills are not booked, \
                 their status changes are not announced, and a withdrawal of every order \
                 does not reach them",
            )))?;
        }
        let orders = self.core.collect_open_orders(&shared);
        for (order_id, tracked) in &orders {
            let c_py = Py::new(py, Contract::from_api(py, &tracked.contract)?)?.into_any();
            let o = Order::from_api(py, &tracked.order)?;
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
                 // The client the order was placed under, as the accompanying
                 // order object already states. Read off this client instead,
                 // the two callbacks for one order disagree and the status
                 // attributes the order to whoever happened to ask.
                 tracked.order.client_id as i64, "", 0.0f64))?;
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
    /// `order_bound` is never fired here: the permanent id an order was given
    /// arrives on its status and its fills, and the reference client no longer
    /// gates anything on the message that would carry it.
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
        let Some(_tx) = self.tx_or_report(-1)? else { return Ok(()) };
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
    ///
    /// `lastNDays` and `specificDates` on the filter are refused when stated:
    /// the executions answered are this session's, filtered by the other
    /// fields, and a window this client cannot apply would go unapplied.
    #[pyo3(signature = (req_id, exec_filter=None))]
    fn req_executions(&self, py: Python<'_>, req_id: i64, exec_filter: Option<Py<PyAny>>) -> PyResult<()> {
        let Some(_connected) = self.tx_or_report(req_id)? else { return Ok(()) };
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
            // Two filters this session cannot apply: it answers from the
            // executions it has seen and does not ask the venue, so a window
            // stated in days or dates would be dropped rather than applied.
            // The reference leaves `lastNDays` at UNSET_INTEGER and
            // `specificDates` at None; an object without them reads as 0.
            let last_n_days = get_i64("lastNDays");
            let dates_stated = fobj
                .getattr(py, pyo3::types::PyString::new(py, "specificDates"))
                .ok()
                .is_some_and(|v| !v.is_none(py) && v.bind(py).len().is_ok_and(|n| n > 0));
            if (last_n_days != 0 && last_n_days != i64::from(i32::MAX)) || dates_stated {
                return self.report_refusal(py, req_id, Refusal::validation(
                    "req_executions: lastNDays and specificDates are not applied here; \
                     executions are this session's, filtered by the other fields",
                ));
            }
            ExecutionFilter {
                symbol: get("symbol"),
                sec_type: get("secType"),
                exchange: get("exchange"),
                // Either vocabulary: the comparison reads the order action
                // and the venue's word for the side alike, so this surface
                // sends what it was given.
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
        // the interpreter, not just this thread.
        for se in snapshot {
            let c_py = Py::new(py, Contract::from_api(py, &se.contract)?)?.into_any();

            let exec_obj = Execution::from_api(&se.execution);
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
        let Some(_tx) = self.tx_or_report(-1)? else { return Ok(()) };
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
                // What the venue has taken back goes first. A trade cancel or
                // correction returns a finished order to a working quantity,
                // and the bridge can only drop the completion it still holds —
                // this is the copy it cannot reach. Applied before the
                // arrivals below, an order taken back and then finished again
                // keeps the new record and loses the superseded one.
                for order_id in shared.orders.drain_order_corrections() {
                    archive.retain(|(_, order, _)| order.order_id != order_id as i64);
                    // And the eviction armed for it when it finished. A bust or a
                    // correction puts the order back to a working quantity, and an
                    // eviction still standing took its record away on the next pass —
                    // after which the order reads as one the venue is not working.
                    self.deferred_evictions.lock().unwrap().remove(&order_id);
                }
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
                    // Filled out from what the venue has said about the
                    // contract, as the open-order answer already is on both
                    // surfaces and as the other surface's completed answer is.
                    // Taken verbatim here, an order lost its exchange, its
                    // multiplier and its local symbol the moment it finished,
                    // and only on this binding.
                    let contract = if contract.con_id != 0 {
                        self.core.get_contract(contract.con_id, &shared).unwrap_or(contract)
                    } else {
                        contract
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
                let c_py = Py::new(py, Contract::from_api(py, contract)?)?.into_any();
                let o_py = Py::new(py, Order::from_api(py, order)?)?.into_any();
                let state_py = Py::new(py, OrderState::from_api(state))?.into_any();
                self.deliver(py, "completed_order", (&c_py, &o_py, &state_py))?;
            }
            self.deliver(py, "completed_orders_end", ())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::SharedState;
    use std::sync::Arc;

    /// A connected client whose engine is a channel the test reads, and a
    /// wrapper that keeps every callback it is handed.
    fn wired_client(
        py: Python<'_>,
    ) -> (EClient, std::sync::mpsc::Receiver<ControlCommand>, Arc<SharedState>, Py<PyAny>) {
        let client = EClient::__new__(&pyo3::types::PyTuple::empty(py), None);
        let ns = pyo3::types::PyDict::new(py);
        py.run(
            c"class W:
    def __init__(self): self.calls = []
    def __getattr__(self, name):
        return lambda *args: self.calls.append((name,) + args)
w = W()",
            None,
            Some(&ns),
        ).unwrap();
        let wrapper = ns.get_item("w").unwrap().unwrap().unbind();
        client.__init__(wrapper.clone_ref(py)).unwrap();
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(64);
        *client.shared.lock().unwrap() = Some(shared.clone());
        *client.control_tx.lock().unwrap() = Some(tx);
        *client.account_id.lock().unwrap() = Some("DU123".into());
        client.connected.store(true, Ordering::Release);
        (client, rx, shared, wrapper)
    }

    /// The venue names the working orders after the connect returns, and a
    /// global cancel is composed from what has been named. Issued before the
    /// naming lands, it waits for it — without the wait it counted no
    /// instruments, sent nothing, and returned without an error.
    #[test]
    fn a_global_cancel_waits_for_the_venue_to_name_the_working_orders() {
        Python::initialize();
        Python::attach(|py| {
            let (client, rx, shared, _wrapper) = wired_client(py);
            let venue = shared.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                venue.market.set_instrument_count(1);
                venue.orders.set_replay_done();
            });
            client.req_global_cancel(py, None).unwrap();
            let sent: Vec<ControlCommand> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert!(
                matches!(sent.as_slice(), [ControlCommand::Order(OrderRequest::CancelAll { instrument: 0 })]),
                "the order the venue named is withdrawn: {sent:?}",
            );
        });
    }

    /// An order held back for a later transmit was never given to the venue,
    /// so a withdrawal of everything forgets it rather than sending a cancel
    /// the venue would refuse — left held, it would go out behind the next
    /// thing that transmits, after the caller had asked for everything back.
    #[test]
    fn a_global_cancel_forgets_the_orders_held_for_a_later_transmit() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, shared, _wrapper) = wired_client(py);
            shared.orders.set_replay_done();
            client.core.hold_until_transmitted(
                7, 0, ControlCommand::Order(OrderRequest::Cancel { order_id: 7 }),
            );
            client.req_global_cancel(py, None).unwrap();
            assert!(!client.core.withdraw_held(7), "nothing is still held");
        });
    }

    /// A global cancel issued before the venue has finished naming the
    /// working orders is not answered in silence: what had been named is
    /// still withdrawn, and the caller is told on the error callback that
    /// the naming had not finished — a partial cancel that reads as one,
    /// rather than as a complete answer that is not.
    #[test]
    fn a_global_cancel_says_when_the_venue_has_not_finished_naming() {
        Python::initialize();
        Python::attach(|py| {
            let (client, rx, shared, wrapper) = wired_client(py);
            shared.market.set_instrument_count(1);
            // The venue began naming and never said it had finished: the wait
            // runs out with something named and something possibly not. An
            // account named nothing at all is the other case, and is told
            // nothing — there is no uncovered order to warn about.
            shared.orders.note_naming_began();
            client.req_global_cancel(py, None).unwrap();
            let sent: Vec<ControlCommand> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert!(
                matches!(sent.as_slice(), [ControlCommand::Order(OrderRequest::CancelAll { instrument: 0 })]),
                "what had been named is still withdrawn: {sent:?}",
            );
            let calls = wrapper.bind(py).getattr("calls").unwrap();
            let heard = calls
                .extract::<Vec<(String, i64, i64, i64, String, String)>>()
                .unwrap();
            assert!(
                heard.iter().any(|(name, req_id, _, code, message, _)| {
                    name == "error"
                        && *req_id == -1
                        && *code == crate::error_codes::Refusal::NO_ANSWER as i64
                        && message.contains("had not finished naming")
                }),
                "the caller is told on the error callback what was and was not covered: {heard:?}",
            );
        });
    }

    /// Asking what the account is working before the venue has finished
    /// naming it answers with what had arrived, and that is what an account
    /// working nothing looks like. The caller is told which of the two it is
    /// reading, so a strategy does not take a partial snapshot for a flat
    /// account and place again what it already has on.
    #[test]
    fn open_orders_say_when_the_snapshot_is_not_known_to_be_whole() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, shared, wrapper) = wired_client(py);
            // The venue began naming and never said it had finished. An
            // account named nothing at all is the other case, and is told
            // nothing — there is no missing order to warn about.
            shared.orders.note_naming_began();
            client.req_open_orders(py).unwrap();
            let calls = wrapper.bind(py).getattr("calls").unwrap();
            let told: Vec<(String, i64, i64, i64, String, String)> = (0..calls.len().unwrap())
                .filter_map(|i| calls.get_item(i).unwrap().extract().ok())
                .collect();
            assert!(
                told.iter().any(|(name, req_id, _, code, message, _)| {
                    name == "error"
                        && *req_id == -1
                        && *code == crate::error_codes::Refusal::NO_ANSWER as i64
                        && message.contains("had not finished naming")
                }),
                "the caller is told the snapshot is not known to be whole: {told:?}",
            );
        });
    }

    /// An order placed under client zero keeps that client id when another
    /// session reads it back, and an order this session placed reads under
    /// the client it went out under.
    ///
    /// Zero is a client, not an absence: restated as the observer's own, an
    /// order placed elsewhere read as one this session held under the same
    /// id, and whatever this session held under that id read as another's.
    #[test]
    fn an_order_placed_by_client_zero_keeps_that_client_id_when_read_back() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, shared, wrapper) = wired_client(py);
            shared.orders.set_replay_done();
            client.client_id.store(7, Ordering::Release);
            client.core.con_id_to_instrument.lock().unwrap().insert(756733, 0);
            // Placed by this session, held back from the venue.
            client.place_order(py, 3, &bracket_contract(), &bracket_order(false, 0)).unwrap();
            // Placed under client zero, as the venue reports one.
            client.core.track_order(
                4,
                crate::types::model::Contract {
                    con_id: 756733, symbol: "SPY".into(), ..Default::default()
                },
                crate::types::model::Order {
                    order_id: 4, action: "BUY".into(), total_quantity: 1.0,
                    order_type: "LMT".into(), lmt_price: 10.0, ..Default::default()
                },
                0,
            );
            client.req_open_orders(py).unwrap();
            client.hand_over_what_is_waiting(py).unwrap();
            let calls = wrapper.bind(py).getattr("calls").unwrap();
            let mut read_back = std::collections::BTreeMap::new();
            for i in 0..calls.len().unwrap() {
                let call = calls.get_item(i).unwrap();
                let name: String = call.get_item(0).unwrap().extract().unwrap();
                if name != "openOrder" {
                    continue;
                }
                let order_id: i64 = call.get_item(1).unwrap().extract().unwrap();
                let client_id: i64 = call.get_item(3).unwrap()
                    .getattr("clientId").unwrap().extract().unwrap();
                read_back.insert(order_id, client_id);
            }
            assert_eq!(
                read_back.get(&3), Some(&7),
                "the order this session placed reads under the client it went out under: {read_back:?}",
            );
            assert_eq!(
                read_back.get(&4), Some(&0),
                "the order placed under client zero is not restated as this session's: {read_back:?}",
            );
        });
    }

    /// An order the venue named that could not be given a slot in the
    /// instrument table is in none of the cancels a withdrawal of everything
    /// composes: those are composed from the engine's book, and the order
    /// never reached it. The caller is told on the error callback rather than
    /// answered as though the account had been flattened.
    #[test]
    fn a_global_cancel_says_when_an_order_has_no_slot_in_the_instrument_table() {
        Python::initialize();
        Python::attach(|py| {
            let (client, rx, shared, wrapper) = wired_client(py);
            // The naming finished. One of the orders it carried arrived with
            // the table full, so this is the other case from a naming that
            // never ended: what was left out is established, not suspected.
            shared.orders.set_replay_done();
            shared.market.set_instrument_count(1);
            shared.orders.note_an_order_without_a_slot();
            client.req_global_cancel(py, None).unwrap();
            let sent: Vec<ControlCommand> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert!(
                matches!(sent.as_slice(), [ControlCommand::Order(OrderRequest::CancelAll { instrument: 0 })]),
                "what the book does hold is still withdrawn: {sent:?}",
            );
            let calls = wrapper.bind(py).getattr("calls").unwrap();
            let heard: Vec<(String, i64, i64, i64, String, String)> = (0..calls.len().unwrap())
                .filter_map(|i| calls.get_item(i).unwrap().extract().ok())
                .collect();
            assert!(
                heard.iter().any(|(name, req_id, _, code, message, _)| {
                    name == "error"
                        && *req_id == -1
                        && *code == crate::error_codes::Refusal::NO_ANSWER as i64
                        && message.contains("no slot in this client's instrument table")
                }),
                "the caller is told what the withdrawal did not reach: {heard:?}",
            );
        });
    }

    /// An order left out of the engine's book for want of a slot is still
    /// listed from the order cache, and nothing this session does follows it:
    /// its fills are not booked, its status changes are not announced, and a
    /// withdrawal of everything does not reach it. The caller is told so,
    /// rather than reading the list as the orders this session is following.
    #[test]
    fn open_orders_say_when_one_of_them_has_no_slot_in_the_instrument_table() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, shared, wrapper) = wired_client(py);
            // The naming finished, and one of the orders it carried has no slot.
            shared.orders.set_replay_done();
            shared.orders.note_an_order_without_a_slot();
            client.req_open_orders(py).unwrap();
            let calls = wrapper.bind(py).getattr("calls").unwrap();
            let told: Vec<(String, i64, i64, i64, String, String)> = (0..calls.len().unwrap())
                .filter_map(|i| calls.get_item(i).unwrap().extract().ok())
                .collect();
            assert!(
                told.iter().any(|(name, req_id, _, code, message, _)| {
                    name == "error"
                        && *req_id == -1
                        && *code == crate::error_codes::Refusal::NO_ANSWER as i64
                        && message.contains("no slot in this client's instrument table")
                }),
                "the caller is told which of the orders it lists it cannot act on: {told:?}",
            );
        });
    }

    /// A quote feed the engine has given up on does not stop this surface
    /// withdrawing a live order.
    ///
    /// Every request here read the session's flag, which any transport being
    /// given up on raises — so an order the trading connection would have
    /// carried was refused because the prices had stopped, and a caller could
    /// not withdraw what it already had working. The other surface reads the
    /// trading connection's own state before it takes an order, and this one
    /// now reads the same thing.
    #[test]
    fn a_quote_feed_that_ended_does_not_stop_an_order_being_withdrawn() {
        Python::initialize();
        Python::attach(|py| {
            let (client, shared, wrapper) = placed_client(py);
            let (tx, rx) = std::sync::mpsc::sync_channel::<ControlCommand>(4);
            *client.control_tx.lock().unwrap() = Some(tx);

            shared.reference.set_session_over("the market data farm");
            client.cancel_order(py, 11, None).unwrap();
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(ControlCommand::Order(OrderRequest::Cancel { order_id: 11 })),
                ),
                "the trading connection still carries the withdrawal",
            );
            assert!(error_calls(py, &wrapper).is_empty(), "and nothing is reported");

            // Once that connection is the one that has ended, it is refused.
            shared.reference.set_trading_over("the trading connection");
            client.cancel_order(py, 12, None).unwrap();
            assert!(rx.try_recv().is_err(), "nothing reaches the engine");
            assert_eq!(
                error_calls(py, &wrapper).len(), 1,
                "and the caller is told, on the callback a refusal is reported on",
            );
        });
    }

    /// A connected client whose engine is a channel the test drives, with a
    /// wrapper that keeps every callback it is handed.
    fn placed_client(py: Python<'_>) -> (EClient, Arc<SharedState>, Py<PyAny>) {
        let ns = pyo3::types::PyDict::new(py);
        py.run(
            c"class W:
    def __init__(self): self.calls = []
    def __getattr__(self, name):
        return lambda *args: self.calls.append((name,) + args)
w = W()",
            None,
            Some(&ns),
        )
        .unwrap();
        let wrapper = ns.get_item("w").unwrap().unwrap().unbind();
        let client = EClient::__new__(&pyo3::types::PyTuple::empty(py), None);
        client.__init__(wrapper.clone_ref(py)).unwrap();
        let shared = Arc::new(SharedState::new());
        *client.shared.lock().unwrap() = Some(shared.clone());
        *client.account_id.lock().unwrap() = Some("DU123".into());
        client.connected.store(true, Ordering::Release);
        (client, shared, wrapper)
    }

    /// The contract the bracket tests place their orders on.
    fn bracket_contract() -> Contract {
        Contract {
            con_id: 756733,
            symbol: "IBM".into(),
            sec_type: "STK".into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
            ..Default::default()
        }
    }

    /// One order of the bracket tests.
    fn bracket_order(transmit: bool, parent_id: i64) -> Order {
        Order {
            action: "BUY".into(),
            total_quantity: 100.0,
            order_type: "LMT".into(),
            lmt_price: 10.0,
            transmit,
            parent_id,
            ..Default::default()
        }
    }

    /// The error callbacks the wrapper was handed, as id, code and message.
    fn error_calls(py: Python<'_>, wrapper: &Py<PyAny>) -> Vec<(i64, i64, String)> {
        let all: Vec<(String, i64, i64, i64, String, String)> = wrapper
            .getattr(py, "calls")
            .unwrap()
            .extract(py)
            .unwrap();
        all.into_iter()
            .filter(|(name, _, _, _, _, _)| name == "error")
            .map(|(_, id, _stamp, code, message, _)| (id, code, message))
            .collect()
    }

    /// A bracket whose engine takes the whole family: all three go out, in
    /// the order they were placed, and nothing is left held behind them.
    #[test]
    fn a_bracket_whose_engine_takes_the_family_sends_all_three_in_order() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _shared, wrapper) = placed_client(py);
            let (tx, rx) = std::sync::mpsc::sync_channel::<ControlCommand>(0);
            *client.control_tx.lock().unwrap() = Some(tx);
            client.core.set_registration_timeout(std::time::Duration::from_secs(2));
            let engine = std::thread::spawn(move || {
                let mut received = Vec::new();
                match rx.recv() {
                    Ok(ControlCommand::RegisterInstrument { reply_tx, .. }) => {
                        let _ = reply_tx.expect("a registration asks for an answer").send(Ok(7));
                    }
                    other => panic!("a registration goes first, got {other:?}"),
                }
                while received.len() < 3 {
                    match rx.recv() {
                        Ok(cmd) => received.push(cmd),
                        Err(_) => break,
                    }
                }
                received
            });
            client.place_order(py, 3, &bracket_contract(), &bracket_order(false, 0)).unwrap();
            client.place_order(py, 4, &bracket_contract(), &bracket_order(false, 3)).unwrap();
            client.place_order(py, 5, &bracket_contract(), &bracket_order(true, 3)).unwrap();
            let received = engine.join().unwrap();
            assert!(
                matches!(
                    received.as_slice(),
                    [
                        ControlCommand::Order(OrderRequest::SubmitEx { con_id: 0, order_id: 3, .. }),
                        ControlCommand::Order(OrderRequest::SubmitEx { con_id: 0, order_id: 4, .. }),
                        ControlCommand::Order(OrderRequest::SubmitEx { con_id: 0, order_id: 5, .. }),
                    ],
                ),
                "the family goes in the order it was placed: {received:?}",
            );
            assert!(error_calls(py, &wrapper).is_empty(), "a family that went is not reported");
            assert!(!client.core.withdraw_held(3), "nothing is held after the transmit");
            assert!(!client.core.withdraw_held(4), "nothing is held after the transmit");
        });
    }

    /// The engine stops once the parent of a bracket is in hand: the parent
    /// is on its way to the venue and the protective children are not. The
    /// call is answered on the error callback, stating what reached the
    /// engine and what did not — raised instead, a caller written against
    /// the reference client learned only that something failed, and had an
    /// unhedged position it believed did not exist.
    #[test]
    fn a_bracket_whose_engine_stops_partway_states_what_reached_the_engine() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _shared, wrapper) = placed_client(py);
            // A rendezvous channel: nothing reaches the engine until the
            // engine takes it, so the engine controls exactly how much of
            // the family it has in hand when it stops.
            let (tx, rx) = std::sync::mpsc::sync_channel::<ControlCommand>(0);
            *client.control_tx.lock().unwrap() = Some(tx);
            client.core.set_registration_timeout(std::time::Duration::from_secs(2));
            let engine = std::thread::spawn(move || {
                let mut received = Vec::new();
                // Answer the registration the first order asks for, take the
                // parent of the bracket, then stop.
                match rx.recv() {
                    Ok(ControlCommand::RegisterInstrument { reply_tx, .. }) => {
                        let _ = reply_tx.expect("a registration asks for an answer").send(Ok(7));
                    }
                    other => panic!("a registration goes first, got {other:?}"),
                }
                if let Ok(order) = rx.recv() {
                    received.push(order);
                }
                received
            });
            client.place_order(py, 3, &bracket_contract(), &bracket_order(false, 0)).unwrap();
            client.place_order(py, 4, &bracket_contract(), &bracket_order(false, 3)).unwrap();
            client.place_order(py, 5, &bracket_contract(), &bracket_order(true, 3)).unwrap();
            let received = engine.join().unwrap();
            assert!(
                matches!(
                    received.as_slice(),
                    [ControlCommand::Order(OrderRequest::SubmitEx { con_id: 0, order_id: 3, .. })],
                ),
                "only the parent reached the engine: {received:?}",
            );
            let errors = error_calls(py, &wrapper);
            let (id, code, message) = errors.last().expect("the caller is told on the error callback");
            assert_eq!(*id, 5);
            assert_eq!(*code, 504);
            assert!(message.contains("order 3 reached the engine"), "{message}");
            assert!(message.contains("orders 4, 5 did not reach it"), "{message}");
            // What did not go is not left queued to slip out behind the next
            // thing that transmits.
            assert!(!client.core.withdraw_held(4), "the child that did not go is withdrawn");
            assert!(!client.core.withdraw_held(3), "the parent went with its send");
        });
    }

    /// The engine stops before it takes anything of a bracket. Nothing is
    /// sent and nothing is lost: the family is still held for a later
    /// transmit, and the caller is told that on the error callback rather
    /// than handed an exception.
    #[test]
    fn a_bracket_whose_engine_stops_first_sends_nothing_and_keeps_its_family() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _shared, wrapper) = placed_client(py);
            let (tx, rx) = std::sync::mpsc::sync_channel::<ControlCommand>(0);
            *client.control_tx.lock().unwrap() = Some(tx);
            client.core.set_registration_timeout(std::time::Duration::from_secs(2));
            let engine = std::thread::spawn(move || {
                let received: Vec<ControlCommand> = Vec::new();
                // Answer the registration, then stop before taking an order.
                match rx.recv() {
                    Ok(ControlCommand::RegisterInstrument { reply_tx, .. }) => {
                        let _ = reply_tx.expect("a registration asks for an answer").send(Ok(7));
                    }
                    other => panic!("a registration goes first, got {other:?}"),
                }
                received
            });
            client.place_order(py, 3, &bracket_contract(), &bracket_order(false, 0)).unwrap();
            client.place_order(py, 4, &bracket_contract(), &bracket_order(false, 3)).unwrap();
            client.place_order(py, 5, &bracket_contract(), &bracket_order(true, 3)).unwrap();
            let received = engine.join().unwrap();
            assert!(received.is_empty(), "no order reached the engine: {received:?}");
            let errors = error_calls(py, &wrapper);
            let (id, code, message) = errors.last().expect("the caller is told on the error callback");
            assert_eq!(*id, 5);
            assert_eq!(*code, 504);
            assert!(message.contains("nothing was sent"), "{message}");
            assert!(message.contains("orders 3, 4 are still held"), "{message}");
            assert!(client.core.withdraw_held(3), "the parent is still held");
            assert!(client.core.withdraw_held(4), "the child is still held");
        });
    }

    /// Cancelling an order the venue was never given forgets it, rather than
    /// asking the venue to withdraw something it does not have.
    ///
    /// Sent, the venue answers that it knows no such order and the command
    /// stays queued to go out behind the next thing that transmits: a caller
    /// that cancelled a parent and then sent its stop-loss had the parent it
    /// had cancelled placed for it.
    #[test]
    fn cancelling_a_held_order_forgets_it_rather_than_placing_it_later() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _shared, _wrapper) = placed_client(py);
            let (tx, rx) = std::sync::mpsc::sync_channel::<ControlCommand>(64);
            *client.control_tx.lock().unwrap() = Some(tx);
            client.core.con_id_to_instrument.lock().unwrap().insert(756733, 0);

            client.place_order(py, 3, &bracket_contract(), &bracket_order(false, 0)).unwrap();
            client.cancel_order(py, 3, None).unwrap();
            client.place_order(py, 4, &bracket_contract(), &bracket_order(true, 0)).unwrap();

            let sent: Vec<ControlCommand> = std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|c| matches!(c, ControlCommand::Order(_)))
                .collect();
            for cmd in &sent {
                if let ControlCommand::Order(OrderRequest::SubmitEx { order_id, .. }) = cmd {
                    assert_ne!(
                        *order_id, 3,
                        "the order that was cancelled is not placed later: {sent:?}",
                    );
                }
                assert!(
                    !matches!(cmd, ControlCommand::Order(OrderRequest::Cancel { order_id: 3 })),
                    "and the venue is not asked to withdraw one it never had: {sent:?}",
                );
            }
            assert!(!client.core.is_held(3), "nothing of it is left queued");
            assert!(!client.core.is_order_tracked(3), "and its id no longer reads as an order");
        });
    }

    /// The engine is asked for a contract and says nothing before the wait
    /// runs out. Reported under this client's own number for silence, and
    /// the call returns: the exception this used to raise was somewhere a
    /// caller written against the reference client has no handling.
    #[test]
    fn a_registration_that_times_out_is_reported_and_the_call_returns() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _shared, wrapper) = placed_client(py);
            let (tx, rx) = std::sync::mpsc::sync_channel::<ControlCommand>(8);
            *client.control_tx.lock().unwrap() = Some(tx);
            client.core.set_registration_timeout(std::time::Duration::from_millis(20));
            let engine = std::thread::spawn(move || {
                // Take the registration and sit on the answer past the wait.
                if let Ok(cmd) = rx.recv() {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    drop(cmd);
                }
            });
            client.place_order(py, 3, &bracket_contract(), &bracket_order(true, 0)).unwrap();
            engine.join().unwrap();
            let errors = error_calls(py, &wrapper);
            let (id, code, message) = errors.last().expect("the caller is told on the error callback");
            assert_eq!(*id, 3);
            assert_eq!(*code, Refusal::NO_ANSWER as i64);
            assert!(message.contains("Registration timed out"), "{message}");
        });
    }

    /// The engine takes a registration and stops before answering it.
    /// Reported under the number for a lost session, and the call returns.
    #[test]
    fn a_registration_the_engine_stops_in_is_reported_and_the_call_returns() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _shared, wrapper) = placed_client(py);
            let (tx, rx) = std::sync::mpsc::sync_channel::<ControlCommand>(8);
            *client.control_tx.lock().unwrap() = Some(tx);
            client.core.set_registration_timeout(std::time::Duration::from_secs(2));
            let engine = std::thread::spawn(move || {
                // Take the registration and stop without answering.
                let _ = rx.recv();
            });
            client.place_order(py, 3, &bracket_contract(), &bracket_order(true, 0)).unwrap();
            engine.join().unwrap();
            let errors = error_calls(py, &wrapper);
            let (id, code, message) = errors.last().expect("the caller is told on the error callback");
            assert_eq!(*id, 3);
            assert_eq!(*code, 504);
            assert!(message.contains("Engine stopped"), "{message}");
        });
    }

    /// A combination leg the caller states and this client cannot read is a
    /// refusal, as the other unreadable fields are, and is answered on the
    /// error callback: the exception this used to raise was somewhere a
    /// caller written against the reference client has no handling.
    #[test]
    fn an_unreadable_combination_leg_is_reported_and_the_call_returns() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _shared, wrapper) = placed_client(py);
            let (tx, _rx) = std::sync::mpsc::sync_channel::<ControlCommand>(8);
            *client.control_tx.lock().unwrap() = Some(tx);
            let contract = bracket_contract();
            // A leg with no contract id: it names no contract, and the list
            // is refused.
            let leg = py.eval(c"type('ComboLeg', (), {})()", None, None).unwrap();
            contract.combo_legs.bound(py).append(leg).unwrap();
            client.place_order(py, 3, &contract, &bracket_order(true, 0)).unwrap();
            let errors = error_calls(py, &wrapper);
            let (id, code, message) = errors.last().expect("the caller is told on the error callback");
            assert_eq!(*id, 3);
            assert_eq!(*code, Refusal::VALIDATION as i64);
            assert!(message.contains("combo leg 0 has no conId"), "{message}");
        });
    }
}
