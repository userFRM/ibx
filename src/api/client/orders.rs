//! Order placement, cancellation, execution replay, and algo parsing.

use std::sync::atomic::Ordering;

use crate::error_codes::Refusal;
use crate::types::model::ExecutionFilter;
use crate::api::wrapper::Wrapper;
use crate::client_core::ClientCore;
use crate::types::*;

use super::{Contract, Order, EClient};

impl EClient {
    // ── Orders ──

    /// Refuse a security type the venue does not permit this account to trade.
    ///
    /// The venue states its permissions at logon, keyed by security type, and it
    /// refuses an order on an unpermitted type by returning it Inactive with no
    /// text at all — so without this check the caller is told nothing. Silence
    /// here is not permission: when the venue stated no permissions, there is
    /// nothing to enforce and the order goes.
    fn check_sec_type_permitted(&self, sec_type: &str) -> Result<(), Refusal> {
        // A validator's own words, under the number a request the client will
        // not send is reported with.
        ClientCore::refuse_unpermitted_sec_type(
            &self.shared.reference.order_permissions(), sec_type,
        )
        .map_err(Refusal::validation)
    }

    /// Security type → the order types the venue permits for it, as stated at
    /// logon. Empty until the session is up.
    pub fn order_permissions(&self) -> std::collections::HashMap<String, Vec<String>> {
        self.shared.reference.order_permissions()
    }

    /// The order types permitted for one security type, or `None` when the type
    /// is not permitted at all. A combination is named `COMB`.
    pub fn permitted_order_types(&self, sec_type: &str) -> Option<Vec<String>> {
        self.shared.reference.permitted_order_types(&sec_type.to_ascii_uppercase())
    }

    /// Feature tokens the venue enables for this account: those stated at
    /// logon, and those the account configuration adds afterwards.
    pub fn enabled_features(&self) -> Vec<String> {
        self.shared.reference.enabled_features()
    }

    /// Which algorithms the venue offers, keyed `PROVIDER/SECTYPE`.
    ///
    /// Stated on the session rather than per contract. An algorithm absent
    /// here is one this account may not use, and an order naming it is
    /// refused by the venue.
    pub fn algorithms(&self) -> std::collections::HashMap<String, Vec<String>> {
        self.shared.reference.algorithms()
    }

    /// The algorithms offered for one security type, across every provider.
    pub fn algorithms_for(&self, sec_type: &str) -> Vec<String> {
        self.shared.reference.algorithms_for(sec_type)
    }

    /// Refuse where the trading connection has stopped being retried.
    ///
    /// Gated on that connection's own state rather than the session's: the
    /// session flag is set by any transport ending, the market-data farm
    /// included, and refusing on it would refuse what a live trading
    /// connection would have carried. Where the trading connection itself has
    /// stopped, anything given here is recorded and buffered and never sent,
    /// and the caller is told it did something the venue never saw.
    fn refuse_if_trading_is_over(&self, what: &str) -> Result<(), Refusal> {
        match self.shared.reference.trading_over() {
            Some(why) => Err(Refusal::validation(format!(
                "the trading connection has ended and is not being retried ({why}), so \
                 {what} given here would be recorded and never sent: open a session again",
            ))),
            None => Ok(()),
        }
    }

    /// Place an order. Matches `placeOrder` in C++.
    ///
    /// An order names its contract by the venue's id. A caller who states
    /// a description instead of an id — which every example written against
    /// the reference client does — has it resolved here, once the order itself
    /// is known to be one the venue would take: an order that names no
    /// contract is one the venue has nothing to match, and answers with
    /// nothing at all.
    ///
    /// Resolving it costs a request and an answer the first time, so this call
    /// does not return until the venue has named the contract — up to the
    /// answer timeout. Once per description: the answer is kept, and later
    /// orders on the same contract are sent without asking again. The reference client
    /// never waits here, because a gateway
    /// resolved the contract before the order reached it; this client is the
    /// gateway, so the work happens somewhere, and today it happens on the
    /// caller's thread. A caller placing orders from inside a callback stalls
    /// its own dispatch loop for that time. Pass a contract carrying `con_id`
    /// — from `qualify_contract`, or from any contract-details answer — and
    /// nothing is resolved and nothing waits.
    pub fn place_order(&self, order_id: i64, contract: &Contract, order: &Order) -> Result<(), Refusal> {
        // Gated on the trading connection's own state rather than the
        // session's. The session flag is set by any transport ending, the
        // market-data farm included, and refusing an order on that takes the
        // trading connection down with the quote feed — on a connection that
        // would have carried it. This one is set only where the trading
        // connection itself has stopped being retried, which is where an order
        // accepted here would join a buffer nothing drains and be reported as
        // sent while never reaching the venue.
        self.refuse_if_trading_is_over("an order")?;
        self.core.refuse_if_readonly("an order").map_err(Refusal::validation)?;
        ClientCore::validate_order_destination(&contract.exchange)?;

        // Validate order params and contract before registering instrument (fail fast).
        ClientCore::validate_order(order, &self.account_id)?;
        ClientCore::validate_supported_instructions(order)?;
        ClientCore::validate_combo_legs(&contract.sec_type, contract.combo_legs.len())?;
        for (at, leg) in contract.combo_legs.iter().enumerate() {
            ClientCore::validate_leg(at, leg)?;
        }
        ClientCore::validate_order_contract(
            contract.con_id,
            &contract.sec_type,
            &crate::types::model::contract_identity(
                &contract.last_trade_date_or_contract_month, contract.strike,
                &contract.right, &contract.multiplier, &contract.currency,
            ),
        )?;
        self.check_sec_type_permitted(&contract.sec_type)?;

        let named;
        let contract = if contract.con_id == 0 && !contract.symbol.is_empty() {
            let key = ClientCore::description_key(contract);
            named = match self.core.named_for(&key) {
                Some(already) => already,
                None => {
                    // Under the code the lookup failed with. Rewritten to
                    // "no security definition" whatever happened, an order
                    // refused because the session ended reads as an order for
                    // a contract that does not exist, and a caller that
                    // branches on the code retries the description for ever.
                    let answer = self.qualify_contract(contract)?;
                    self.core.remember_named(key, answer.clone());
                    answer
                }
            };
            &named
        } else {
            contract
        };

        let oid = if order_id > 0 {
            order_id as u64
        } else {
            // Through the reservation, which persists the id. An id taken from
            // the counter alone is not recorded, and a restart reuses it; the
            // venue rejects a repeated tag 11.
            self.reserve_order_ids(1) as u64
        };

        let instrument = self.core.find_or_register_instrument(
            &self.control_tx,
            contract.con_id, &contract.symbol, &contract.exchange, &contract.sec_type,
            &crate::types::model::contract_identity(
                &contract.last_trade_date_or_contract_month, contract.strike,
                &contract.right, &contract.multiplier, &contract.currency,
            ),
        )?;

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
                return Err(Refusal::validation(format!(
                    "order {oid} is working on another contract, and a replace names \
                     the order rather than the contract: withdraw it and place a new \
                     order to trade {}",
                    contract.symbol,
                )));
            }
            // A replace states the order type, the limit price and the trigger.
            // An order defined by anything else cannot survive one, so refuse
            // rather than send a message that destroys it.
            if let Some(refusal) = self.core.modify_refusal(oid, order) {
                return Err(refusal.into());
            }
            let price = crate::types::price_from_f64(order.lmt_price);
            let qty = crate::types::qty_from_f64(order.total_quantity);
            // A stop's trigger rides on aux_price, exactly as it does on the
            // submit path. Reading only lmt_price left a stop order modifying
            // itself to a limit price of zero.
            let stop_price = crate::types::price_from_f64(order.aux_price);
            ControlCommand::Order(OrderRequest::Modify {
                order_id: oid,
                price,
                qty,
                outside_rth: order.outside_rth,
                ord_type: order.ord_type_byte(),
                tif: order.tif_byte(),
                stop_price,
            })
        } else {
            ClientCore::build_order_request(order, oid, instrument, Some(contract))?
        };
        self.send(cmd)?;
        self.core.cache_contract(contract.con_id, contract.clone());
        self.core.track_order(oid, contract.clone(), order.clone(), instrument);
        Ok(())
    }

    /// Exercise or lapse a long option position. Matches `exerciseOptions` in C++.
    ///
    /// `exercise_action` is 1 to exercise and 2 to lapse; anything else is
    /// refused.
    ///
    /// `override_` is taken and not sent, because there is no tag for it: it
    /// names a check made before the order is built, not one the venue makes.
    /// The check it names is a real one — it is what stops an exercise of an
    /// option that is out of the money and a lapse of one that is in it — and
    /// this client does not make it, because what it rests on is the venue's
    /// word on where the option stands, which this client does not ask for.
    /// So an instruction is sent as given, and `override_ = false` buys no
    /// protection here. Passing `true` is the honest description of what
    /// happens either way; passing `false` says so in the log.
    pub fn exercise_options(
        &self, req_id: i64, contract: &Contract, exercise_action: i32,
        exercise_quantity: i32, account: &str, override_: bool,
    ) -> Result<(), Refusal> {
        self.core.refuse_if_readonly("an exercise").map_err(Refusal::validation)?;
        if !override_ {
            log::warn!(
                "exercise of {} asked to stop short of an option out of the money, and \
                 this client does not know where the option stands: the instruction is \
                 sent as given",
                contract.symbol,
            );
        }
        let (action, qty) = ClientCore::validate_exercise(
            exercise_action, exercise_quantity, account, &self.account_id,
        )?;
        let identity = crate::types::model::contract_identity(
            &contract.last_trade_date_or_contract_month, contract.strike,
            &contract.right, &contract.multiplier, &contract.currency,
        );
        ClientCore::validate_order_contract(contract.con_id, &contract.sec_type, &identity)?;

        let oid = if req_id > 0 {
            req_id as u64
        } else {
            // Written down, as on `place_order` above.
            self.reserve_order_ids(1) as u64
        };
        let instrument = self.core.find_or_register_instrument(
            &self.control_tx,
            contract.con_id, &contract.symbol, &contract.exchange, &contract.sec_type,
            &identity,
        )?;
        self.send(ControlCommand::Order(
            ClientCore::build_exercise_request(oid, instrument, action, crate::types::qty_from_wire(qty as i64)),
        ))
    }

    /// Cancel an order. Matches `cancelOrder` in C++.
    ///
    /// `manual_order_cancel_time` is taken and not applied. A cancel names five
    /// fields on this wire and no time among them, as the protocol's
    /// cancel does.
    pub fn cancel_order(&self, order_id: i64, _manual_order_cancel_time: &str) -> Result<(), Refusal> {
        self.refuse_if_trading_is_over("a withdrawal")?;
        self.core.refuse_if_readonly("a cancel").map_err(Refusal::validation)?;
        // Tag 11 order ids start at 1. A negative id cast unchecked becomes a
        // large unsigned one, which the venue answers "no such order".
        let order_id = u64::try_from(order_id).ok().filter(|id| *id > 0).ok_or_else(|| {
            Refusal::validation(format!(
                "order_id {order_id} is not an order number: they start at one",
            ))
        })?;
        self.send(ControlCommand::Order(OrderRequest::Cancel { order_id }))
    }

    /// Cancel an order identified by `permId` — stable across sessions.
    ///
    /// `permId` is the broker-assigned identifier returned in `order_status`
    /// callbacks and surfaced in account tools. Useful for cancelling an order
    /// placed in a prior session, where the local `order_id` is not retained.
    ///
    /// the CCP cancel frame is orderId-only, so ibx looks up
    /// the local `order_id` from `permId` in the open-order cache (populated by
    /// `place_order` callbacks or by the CCP session-recovery push hydrated in
    /// `handle_exec_report`). Fails if `perm_id` is not currently tracked.
    pub fn cancel_order_by_perm_id(&self, perm_id: i64) -> Result<(), Refusal> {
        self.refuse_if_trading_is_over("a withdrawal")?;
        self.core.refuse_if_readonly("a cancel").map_err(Refusal::validation)?;
        if perm_id == 0 {
            return Err("cancel_order_by_perm_id: perm_id must be non-zero".into());
        }
        let order_id = self.core.collect_open_orders(&self.shared)
            .into_iter()
            .find(|(_, tracked)| tracked.order.perm_id == perm_id)
            .map(|(oid, _)| oid)
            .ok_or_else(|| format!("cancel_order_by_perm_id: permId {perm_id} not found in open orders"))?;
        self.cancel_order(order_id as i64, "")
    }

    /// Cancel all orders. Matches `reqGlobalCancel` in C++.
    pub fn req_global_cancel(&self) -> Result<(), Refusal> {
        self.refuse_if_trading_is_over("a withdrawal of every order")?;
        self.core.refuse_if_readonly("a global cancel").map_err(Refusal::validation)?;
        // Use global instrument count (not just locally-tracked ones)
        let count = self.shared.market.instrument_count();
        for instrument in 0..count {
            self.send(ControlCommand::Order(OrderRequest::CancelAll { instrument }))?;
        }
        Ok(())
    }

    /// Request next valid order ID. Matches `reqIds` in C++.
    pub fn req_ids(&self, wrapper: &mut impl Wrapper) {
        // Stated without being taken, as the reference client states it: the
        // caller places under it, and the reservation happens then.
        let stated = self.next_order_id.load(Ordering::Acquire).max(self.next_id_base());
        crate::bridge::say_if_past_a_request_id(stated);
        wrapper.next_valid_id(stated as i64);
    }

    /// One past the highest id the account is working an order under.
    ///
    /// The venue refuses an order that names an id it is still working, and
    /// takes one whose order has been withdrawn or filled: an id is spent only
    /// while its order is live. So what an id has to clear is the working set,
    /// which the venue names unprompted at every connect — from every session,
    /// not just this one — and not every id the account has ever used.
    ///
    /// Read on each reservation rather than settled once, so an order the
    /// venue names later still raises the floor. Nothing is waited for: an
    /// account with nothing working counts from one, and the wait that would
    /// tell that apart from a naming still in flight costs every such account
    /// its first order.
    fn next_id_base(&self) -> u64 {
        self.shared.orders.working_id_watermark() + 1
    }

    /// Get the next order ID (local counter).
    pub fn next_order_id(&self) -> i64 {
        self.reserve_order_ids(1)
    }

    /// Take `n` consecutive ids in one step.
    ///
    /// A bracket occupies three consecutive ids: parent, parent+1, parent+2.
    /// Reserving them in one step keeps a concurrent placement from taking a
    /// child's id or moving the counter back over ids already handed out.
    fn reserve_order_ids(&self, n: u64) -> i64 {
        let floor = self.next_id_base();
        let mut held = self.next_order_id.load(Ordering::Acquire);
        loop {
            let first = held.max(floor);
            match self.next_order_id.compare_exchange_weak(
                held, first + n, Ordering::AcqRel, Ordering::Acquire,
            ) {
                Ok(_) => {
                    // Said where the id is handed out rather than only where
                    // one is stated, so a caller that never asks what the next
                    // one is still hears it.
                    crate::bridge::say_if_past_a_request_id(first);
                    return first as i64;
                }
                Err(seen) => held = seen,
            }
        }
    }

    // ── Open Orders ──

    /// Request open orders for this client. Matches `reqOpenOrders` in C++.
    ///
    /// Answers with every order working on the account, as
    /// [`req_all_open_orders`](EClient::req_all_open_orders) does. The protocol
    /// carries no client number on an order, so this session cannot tell which
    /// orders it placed; reporting fewer would omit working orders.
    pub fn req_open_orders(&self, wrapper: &mut impl Wrapper) {
        self.req_all_open_orders(wrapper);
    }

    /// Request all open orders. Matches `reqAllOpenOrders` in C++.
    pub fn req_all_open_orders(&self, wrapper: &mut impl Wrapper) {
        // The orders already working are named by the server unprompted after a
        // connect, and answering before that lands reports none of them. A
        // strategy asking what it already has on, at the moment it starts, is
        // exactly who asks this first, and telling it "nothing" is how the same
        // order gets placed twice. Bounded: an account with nothing working
        // never sees the replay end, and waiting forever for it would be worse
        // than answering.
        for _ in 0..300 {
            if self.shared.orders.replay_done() { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !self.shared.orders.replay_done() {
            // An incomplete replay is reported: what follows is what has arrived,
            // which is otherwise indistinguishable from an account with nothing
            // working.
            log::warn!(
                "the venue had not finished naming this account's working orders within \
                 the wait, so what follows is what had arrived rather than what is working",
            );
        }
        for (order_id, tracked) in self.core.collect_open_orders(&self.shared) {
            let state = crate::types::model::OrderState {
                status: tracked.status,
                ..Default::default()
            };
            wrapper.open_order(order_id as i64, &tracked.contract, &tracked.order, &state);
        }
        wrapper.open_order_end();
    }

    // ── Completed Orders ──

    /// Request completed orders. Matches `reqCompletedOrders` in C++.
    ///
    /// Immediately delivers every completed order this session archived, then
    /// calls `completed_orders_end`.
    ///
    /// `api_only` is taken and not applied. It asks for orders entered through
    /// an API rather than by hand, and nothing this client holds says which an
    /// order was: the completed orders are the ones this session saw, and the
    /// venue states no origin on them. Passing `true` is answered with all of
    /// them rather than with a guess at which were typed.
    pub fn req_completed_orders(&self, api_only: bool, wrapper: &mut impl Wrapper) {
        let _ = api_only;
        // Drained once and retained. The queue empties on read and the venue does
        // not resend completed orders, so later calls answer from this archive.
        {
            let mut archive = self.completed.lock().unwrap();
            for order in self.shared.orders.drain_completed_orders() {
                let status_str = crate::types::order_status::order_status_str(order.status);
                let entry = if let Some(info) = self.shared.orders.get_order_info(order.order_id) {
                    let mut state = info.order_state;
                    state.status = status_str.into();
                    // Enrich contract with secdef cache at read time
                    let contract = if info.contract.con_id != 0 {
                        self.core.get_contract(info.contract.con_id, &self.shared)
                            .unwrap_or(info.contract)
                    } else {
                        info.contract
                    };
                    (contract, info.order, state)
                } else {
                    (
                        Contract::default(),
                        Order { order_id: order.order_id as i64, ..Default::default() },
                        crate::types::model::OrderState {
                            status: status_str.into(),
                            ..Default::default()
                        },
                    )
                };
                archive.push(entry);
                // Bound `order_cache` growth: terminal entries are no longer
                // needed once what they carried has been read out of them.
                // Handed to the side that reads the fills rather than freed
                // here. That side is the only one that can tell when a record
                // is finished with: a fill taken off the queue but not yet
                // reported still needs it, and from here looks like no fill at
                // all. Freed here, the report it belonged to arrived with no
                // contract and no execution id — which is also the id its
                // commission is reported under.
                self.deferred_evictions.lock().unwrap().insert(order.order_id);
            }
        }
        // Copied before anything is called back: a callback may ask for these
        // again, and the lock is not re-entrant.
        let completed = self.completed.lock().unwrap().clone();
        for (contract, order, state) in &completed {
            wrapper.completed_order(contract, order, state);
        }
        wrapper.completed_orders_end();
    }

    // ── Executions ──

    /// Automatically bind future orders to this client. Matches `reqAutoOpenOrders` in
    /// C++.
    /// Bind orders entered elsewhere to this client.
    ///
    /// Nothing goes to the venue; this is answered locally, setting
    /// a property of its own and refusing it for any client but the one those
    /// orders bind to. What that property gates does not arise here — this
    /// session is told about every order on the account, whether it placed
    /// them or not — and this surface names no client, so there is nothing to
    /// refuse and nothing left to do.
    ///
    /// `b_auto_bind` is taken and not applied. Whether it asks to bind or to
    /// stop binding, the answer is the same: this session hears about every
    /// order on the account either way.
    pub fn req_auto_open_orders(&self, _b_auto_bind: bool) {}

    /// Request execution reports. Matches `reqExecutions` in C++.
    /// Replays stored executions (optionally filtered), firing `exec_details` +
    /// `commission_and_fees_report` for each, then `exec_details_end`.
    pub fn req_executions(&self, req_id: i64, filter: &ExecutionFilter, wrapper: &mut impl Wrapper) {
        // Snapshot first: a callback may re-enter a path that locks
        // `executions`, and the dispatch thread pushes fills through the same
        // mutex — holding it across user code deadlocks one and stalls the
        // other.
        for se in self.core.snapshot_executions(filter) {
            wrapper.exec_details(req_id, &se.contract, &se.execution);
            wrapper.commission_and_fees_report(&se.commission_and_fees);
        }
        wrapper.exec_details_end(req_id);
    }
}

impl EClient {
    /// Send a bracket as the one instruction the engine has for it.
    ///
    /// Three orders under three numbers, linked by the venue: the children are
    /// held until the parent has a position, and whichever fills withdraws the
    /// other. `place_bracket` is the call that states this in a caller's terms;
    /// this is the part that reaches the engine.
    pub(crate) fn submit_bracket(
        &self, contract: &Contract, side: crate::types::Side, quantity: f64,
        entry: f64, take_profit: f64, stop_loss: f64,
    ) -> Result<[i64; 3], Refusal> {
        // Checked here rather than in `place_bracket`: every surface that places a
        // bracket routes through this.
        self.core.refuse_if_readonly("a bracket").map_err(Refusal::validation)?;
        // The checks `place_order` applies. An order on a security type the account
        // is not permitted is returned Inactive with tag 58 empty, so the reason is
        // stated here instead.
        ClientCore::validate_order_destination(&contract.exchange)?;
        ClientCore::validate_order_contract(
            contract.con_id,
            &contract.sec_type,
            &crate::types::model::contract_identity(
                &contract.last_trade_date_or_contract_month, contract.strike,
                &contract.right, &contract.multiplier, &contract.currency,
            ),
        )?;
        self.check_sec_type_permitted(&contract.sec_type)?;
        let instrument = self.core.find_or_register_instrument(
            &self.control_tx,
            contract.con_id, &contract.symbol, &contract.exchange, &contract.sec_type,
            &crate::types::model::contract_identity(
                &contract.last_trade_date_or_contract_month, contract.strike,
                &contract.right, &contract.multiplier, &contract.currency,
            ),
        )?;
        // Consecutive, because the venue reads the children's numbers as the
        // parent's plus one and two. Taken apart, a bracket links to whatever
        // happened to be placed in between.
        let parent_id = self.reserve_order_ids(3);
        let (tp_id, sl_id) = (parent_id + 1, parent_id + 2);

        let scaled = |price: f64| crate::types::price_from_f64(price);
        self.send(ControlCommand::Order(OrderRequest::SubmitBracket {
            parent_id: parent_id as u64,
            tp_id: tp_id as u64,
            sl_id: sl_id as u64,
            instrument,
            side,
            qty: crate::types::qty_from_f64(quantity),
            entry_price: scaled(entry),
            take_profit: scaled(take_profit),
            stop_loss: scaled(stop_loss),
        }))?;
        Ok([parent_id, tp_id, sl_id])
    }
}
