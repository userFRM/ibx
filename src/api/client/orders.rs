//! Order placement, cancellation, execution replay, and algo parsing.

use std::sync::atomic::Ordering;

use crate::api::error_codes::Refusal;
use crate::api::types::{ExecutionFilter, PRICE_SCALE_F};
use crate::api::wrapper::Wrapper;
use crate::client_core::ClientCore;
use crate::types::*;

use super::{Contract, Order, TagValue, EClient};

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

    /// Place an order. Matches `placeOrder` in C++.
    ///
    /// An order names its contract by the venue's own id. A caller who states
    /// a description instead of an id — which every example written against
    /// the reference client does — has it resolved here, once the order itself
    /// is known to be one the venue would take: an order that names no
    /// contract is one the venue has nothing to match, and answers with
    /// nothing at all.
    ///
    /// Resolving it costs a request and an answer the first time, so this call
    /// does not return until the venue has named the contract — up to the
    /// answer timeout. Once per description: the answer is kept, and later
    /// orders on the same contract are sent without asking again. The reference client never waits here, because a gateway
    /// resolved the contract before the order reached it; this client is the
    /// gateway, so the work happens somewhere, and today it happens on the
    /// caller's thread. A caller placing orders from inside a callback stalls
    /// its own dispatch loop for that time. Pass a contract carrying `con_id`
    /// — from `qualify_contract`, or from any contract-details answer — and
    /// nothing is resolved and nothing waits.
    pub fn place_order(&self, order_id: i64, contract: &Contract, order: &Order) -> Result<(), Refusal> {
        ClientCore::validate_order_destination(&contract.exchange)?;

        // Validate order params and contract before registering instrument (fail fast).
        ClientCore::validate_order(order, &self.account_id)?;
        ClientCore::validate_supported_instructions(order)?;
        ClientCore::validate_combo_legs(&contract.sec_type, contract.combo_legs.len())?;
        ClientCore::validate_order_contract(
            contract.con_id,
            &contract.sec_type,
            &ClientCore::contract_identity(
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
            self.next_order_id.fetch_add(1, Ordering::Relaxed)
        };

        let instrument = self.core.find_or_register_instrument(
            &self.control_tx,
            contract.con_id, &contract.symbol, &contract.exchange, &contract.sec_type,
            &ClientCore::contract_identity(
                &contract.last_trade_date_or_contract_month, contract.strike,
                &contract.right, &contract.multiplier, &contract.currency,
            ),
        )?;

        // If orderId is already tracked, this is a modification — emit Modify instead of Submit.
        let cmd = if self.core.is_order_tracked(oid) {
            // A replace states the order type, the limit price and the trigger.
            // An order defined by anything else cannot survive one, so refuse
            // rather than send a message that destroys it.
            if let Some(refusal) = self.core.modify_refusal(oid, order) {
                return Err(refusal.into());
            }
            let price = (order.lmt_price * PRICE_SCALE_F) as i64;
            let qty = order.total_quantity as u32;
            // A stop's trigger rides on aux_price, exactly as it does on the
            // submit path. Reading only lmt_price left a stop order modifying
            // itself to a limit price of zero.
            let stop_price = (order.aux_price * PRICE_SCALE_F) as i64;
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
    /// refused. `override_` is taken for signature compatibility and is not
    /// sent: it is a validation bypass the venue's own front end applies before
    /// it builds the order, so there is no tag for it on the wire.
    pub fn exercise_options(
        &self, req_id: i64, contract: &Contract, exercise_action: i32,
        exercise_quantity: i32, account: &str, override_: bool,
    ) -> Result<(), Refusal> {
        let _ = override_;
        let (action, qty) = ClientCore::validate_exercise(
            exercise_action, exercise_quantity, account, &self.account_id,
        )?;
        let identity = ClientCore::contract_identity(
            &contract.last_trade_date_or_contract_month, contract.strike,
            &contract.right, &contract.multiplier, &contract.currency,
        );
        ClientCore::validate_order_contract(contract.con_id, &contract.sec_type, &identity)?;

        let oid = if req_id > 0 {
            req_id as u64
        } else {
            self.next_order_id.fetch_add(1, Ordering::Relaxed)
        };
        let instrument = self.core.find_or_register_instrument(
            &self.control_tx,
            contract.con_id, &contract.symbol, &contract.exchange, &contract.sec_type,
            &identity,
        )?;
        self.send(ControlCommand::Order(
            ClientCore::build_exercise_request(oid, instrument, action, qty),
        ))
    }

    /// Cancel an order. Matches `cancelOrder` in C++.
    pub fn cancel_order(&self, order_id: i64, _manual_order_cancel_time: &str) -> Result<(), Refusal> {
        self.send(ControlCommand::Order(OrderRequest::Cancel {
            order_id: order_id as u64,
        }))
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
        // Use global instrument count (not just locally-tracked ones)
        let count = self.shared.market.instrument_count();
        for instrument in 0..count {
            self.send(ControlCommand::Order(OrderRequest::CancelAll { instrument }))?;
        }
        Ok(())
    }

    /// Request next valid order ID. Matches `reqIds` in C++.
    pub fn req_ids(&self, wrapper: &mut impl Wrapper) {
        let next_id = self.next_order_id.load(Ordering::Relaxed) as i64;
        wrapper.next_valid_id(next_id);
    }

    /// Get the next order ID (local counter).
    pub fn next_order_id(&self) -> i64 {
        let id = self.next_order_id.fetch_add(1, Ordering::Relaxed);
        // Remembered as it is handed out rather than in a batch: a run that
        // ends between the two is exactly the run whose ids would be reused.
        if let Some((path, key)) = self.order_id_store.as_ref()
            && let Err(e) = crate::order_ids::remember(path, key, id)
        {
            log::warn!("order id {id} not remembered in {}: {e}", path.display());
        }
        id as i64
    }

    // ── Open Orders ──

    /// Request open orders for this client. Matches `reqOpenOrders` in C++.
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
        for (order_id, tracked) in self.core.collect_open_orders(&self.shared) {
            let state = crate::api::types::OrderState {
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
        for order in self.shared.orders.drain_completed_orders() {
            let status_str = crate::client_core::order_status_str(order.status);
            if let Some(info) = self.shared.orders.get_order_info(order.order_id) {
                let mut state = info.order_state;
                state.status = status_str.into();
                // Enrich contract with secdef cache at read time
                let contract = if info.contract.con_id != 0 {
                    self.core.get_contract(info.contract.con_id, &self.shared).unwrap_or(info.contract)
                } else {
                    info.contract
                };
                wrapper.completed_order(&contract, &info.order, &state);
            } else {
                let contract = Contract::default();
                let api_order = Order { order_id: order.order_id as i64, ..Default::default() };
                let state = crate::api::types::OrderState {
                    status: status_str.into(),
                    ..Default::default()
                };
                wrapper.completed_order(&contract, &api_order, &state);
            }
            // Bound `order_cache` growth: terminal entries are no longer needed
            // once delivered through `completed_order`.
            self.shared.orders.remove_order_info(order.order_id);
        }
        wrapper.completed_orders_end();
    }

    // ── Executions ──

    /// Automatically bind future orders to this client. Matches `reqAutoOpenOrders` in C++.
    /// Bind orders entered elsewhere to this client.
    ///
    /// Nothing goes to the venue: the counterpart answers this itself, setting
    /// a property of its own and refusing it for any client but the one those
    /// orders bind to. What that property gates does not arise here — this
    /// session is told about every order on the account, whether it placed
    /// them or not — and this surface names no client, so there is nothing to
    /// refuse and nothing left to do.
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

/// Parse algo strategy and TagValue params into internal AlgoParams.
///
/// A key the caller never set defaults the way IB's own algos do (0.0,
/// false, or the documented default enum value). A key the caller *did*
/// set — even to an empty string — is refused if it does not parse, rather
/// than taking that same default: `riskAversion="Aggresive"` would otherwise
/// submit a Neutral algo with no error, and `maxPctVol=""` would submit 0.0.
pub fn parse_algo_params(strategy: &str, params: &[TagValue]) -> Result<AlgoParams, Refusal> {
    let get = |key: &str| -> Option<String> {
        params.iter().find(|tv| tv.tag == key).map(|tv| tv.value.clone())
    };
    let get_str = |key: &str| -> String { get(key).unwrap_or_default() };
    let get_f64 = |key: &str| -> Result<f64, Refusal> {
        let raw = match get(key) {
            None => return Ok(0.0),
            Some(raw) => raw,
        };
        let v: f64 = raw.parse()
            .map_err(|_| Refusal::validation(format!("Invalid {key} '{raw}': expected a number")))?;
        if !v.is_finite() {
            return Err(Refusal::validation(
                format!("Invalid {key} '{raw}': must be a finite number"),
            ));
        }
        Ok(v)
    };
    let get_bool = |key: &str| -> Result<bool, Refusal> {
        let raw = match get(key) {
            None => return Ok(false),
            Some(raw) => raw,
        };
        match raw.to_lowercase().as_str() {
            "0" | "false" => Ok(false),
            "1" | "true" => Ok(true),
            _ => Err(Refusal::validation(
                format!("Invalid {key} '{raw}': expected true/false or 1/0"),
            )),
        }
    };

    match strategy.to_lowercase().as_str() {
        "vwap" => Ok(AlgoParams::Vwap {
            max_pct_vol: get_f64("maxPctVol")?,
            no_take_liq: get_bool("noTakeLiq")?,
            allow_past_end_time: get_bool("allowPastEndTime")?,
            start_time: get_str("startTime"),
            end_time: get_str("endTime"),
        }),
        "twap" => Ok(AlgoParams::Twap {
            allow_past_end_time: get_bool("allowPastEndTime")?,
            start_time: get_str("startTime"),
            end_time: get_str("endTime"),
        }),
        "arrivalpx" | "arrival_price" => Ok(AlgoParams::ArrivalPx {
            max_pct_vol: get_f64("maxPctVol")?,
            risk_aversion: parse_risk_aversion(get("riskAversion").as_deref())?,
            allow_past_end_time: get_bool("allowPastEndTime")?,
            force_completion: get_bool("forceCompletion")?,
            start_time: get_str("startTime"),
            end_time: get_str("endTime"),
        }),
        "closepx" | "close_price" => Ok(AlgoParams::ClosePx {
            max_pct_vol: get_f64("maxPctVol")?,
            risk_aversion: parse_risk_aversion(get("riskAversion").as_deref())?,
            force_completion: get_bool("forceCompletion")?,
            start_time: get_str("startTime"),
        }),
        "darkice" | "dark_ice" => {
            let display_size = match get("displaySize") {
                None => 100,
                Some(raw) => raw.parse().map_err(|_| format!("Invalid displaySize '{raw}': expected a non-negative integer"))?,
            };
            Ok(AlgoParams::DarkIce {
                allow_past_end_time: get_bool("allowPastEndTime")?,
                display_size,
                start_time: get_str("startTime"),
                end_time: get_str("endTime"),
            })
        }
        "pctvol" | "pct_vol" => Ok(AlgoParams::PctVol {
            pct_vol: get_f64("pctVol")?,
            no_take_liq: get_bool("noTakeLiq")?,
            start_time: get_str("startTime"),
            end_time: get_str("endTime"),
        }),
        // Anything else goes as the caller wrote it.
        //
        // Refused here instead, a caller could use only the algorithms this
        // match happens to name — five of the thirteen an ordinary session is
        // offered. Which ones an account may use is the venue's answer, stated
        // at logon and enforced by it, and the reference client does not
        // interpret these either.
        // The caller's own spelling, not the one folded for matching: the
        // venue is handed this name and does not know a lower-cased one.
        _ => Ok(AlgoParams::Named {
            strategy: strategy.to_string(),
            params: params
                .iter()
                .flat_map(|tv| [tv.tag.clone(), tv.value.clone()])
                .collect(),
        }),
    }
}

/// Parse a `riskAversion` tag value (used by ArrivalPx and ClosePx). A
/// missing tag defaults to Neutral, matching IB's own algo default; a
/// present value — including an empty string — that isn't a recognized
/// member is refused rather than silently defaulting to Neutral.
fn parse_risk_aversion(raw: Option<&str>) -> Result<RiskAversion, Refusal> {
    let raw = match raw {
        None => return Ok(RiskAversion::Neutral),
        Some(raw) => raw,
    };
    match raw.to_lowercase().as_str() {
        "neutral" => Ok(RiskAversion::Neutral),
        "get_done" | "getdone" => Ok(RiskAversion::GetDone),
        "aggressive" => Ok(RiskAversion::Aggressive),
        "passive" => Ok(RiskAversion::Passive),
        _ => Err(Refusal::validation(
            "Unknown riskAversion '{raw}': expected Get_Done, Aggressive, Neutral or Passive"
                .replace("{raw}", raw),
        )),
    }
}
