use std::sync::Arc;
use std::time::Instant;

use crate::bridge::SharedState;
use crate::config::{chrono_free_timestamp, unix_to_ib_datetime, unix_to_ib_utc_dash};
use crate::engine::context::Context;
use crate::protocol::connection::Connection;
use crate::protocol::fix;
use crate::types::{AlgoParams, OrderCondition, OrderRequest, OrderStatus, OrderUpdate, Side};

use super::{HeartbeatState, format_price, format_qty, format_uint};

pub(crate) fn drain_and_send_orders(
    ccp_conn: &mut Option<Connection>,
    context: &mut Context,
    account_id: &str,
    hb: &mut HeartbeatState,
    disconnected: bool,
    shared: &Arc<SharedState>,
    // Whether a reconnect's recovery is still settling what the broker holds.
    recovery_pending: bool,
    event_tx: &Option<std::sync::mpsc::SyncSender<crate::bridge::Event>>,
) {
    // If CCP is disconnected, leave orders in the pending buffer for retry after reconnect.
    // See: https://github.com/deepentropy/ibx/issues/116
    if disconnected {
        return;
    }

    // Claim the buffer only once there is somewhere to send it. Draining first
    // and then finding no socket dropped every pending order on the floor.
    let conn = match ccp_conn.as_mut() {
        Some(c) => c,
        None => return,
    };
    let orders: Vec<OrderRequest> = context.drain_pending_orders().collect();
    let mut unsent: Vec<OrderRequest> = Vec::new();
    for mut order_req in orders {
        // Once a write has abandoned the transport nothing else can leave on
        // it, and the pre-write guard refuses the rest before they touch the
        // wire. Those are not in doubt the way the failed one is: they were
        // never sent, so they go back to wait for the reconnect rather than
        // being reported as orders of unknown state.
        if conn.write_failed() {
            unsent.push(order_req);
            continue;
        }
        // A cancel or a replace names a version of an order the recovery may be
        // about to correct — a replace that failed left its attempted ClOrdID
        // in front of the one the broker actually holds. Sent now they would
        // state that version and be refused, leaving the order live.
        //
        // Only where that order's state is actually in doubt. An order placed
        // since the reconnect is not, and holding its cancel for the length of
        // a recovery that has nothing to do with it can let it fill first.
        // Cancel-all is included: it sends the same per-order frames, so it
        // carries the same speculative versions.
        let waits_for_recovery = recovery_pending
            && match order_req {
                OrderRequest::Cancel { order_id } | OrderRequest::Modify { order_id, .. } => {
                    context.order(order_id).is_some_and(|o| o.status == OrderStatus::Uncertain)
                }
                OrderRequest::CancelAll { .. } => {
                    context.uncertain_orders().iter().any(|o| o.status == OrderStatus::Uncertain)
                }
                _ => false,
            };
        if waits_for_recovery {
            unsent.push(order_req);
            continue;
        }
        let oid = order_req.order_id();
        // What the engine believed before this request touched anything. A
        // replace writes its attempt into the tracked state ahead of the write,
        // and a write that fails must not leave that attempt standing as though
        // the broker had accepted it.
        let before = context.order(oid).copied();
        let speculative = match order_req {
            OrderRequest::Modify { new_order_id, .. } => Some(new_order_id),
            _ => None,
        };
        // Snap every price to the contract's tick grid before encoding
        // (ibx#216). The tick comes from the market-data subscription ack;
        // without one it is 0 and prices pass through unchanged.
        if let Some(instrument) = order_req.instrument() {
            order_req.snap_prices(context.market.min_tick_scaled(instrument));
        }
        let result = match order_req {
            OrderRequest::SubmitEx { order_id, instrument, side, qty, kind, tif, attrs } => {
                send_order_ex(
                    conn, context, account_id, order_id, instrument, side, qty, kind, tif, &attrs,
                )
            }
            OrderRequest::SubmitBracket {
                parent_id,
                tp_id,
                sl_id,
                instrument,
                side,
                qty,
                entry_price,
                take_profit,
                stop_loss,
            } => {
                let exit_side = match side {
                    Side::Buy => Side::Sell,
                    Side::Sell | Side::ShortSell => Side::Buy,
                };
                let exit_side_str = fix_side(exit_side);
                let side_str = fix_side(side);
                let qty_str = format_uint(qty as u64);
                let symbol = context.market.symbol(instrument).to_string();
                let (sec_type_str, destination) = context.market.order_routing(instrument);
                // Versioned ClOrdIDs like every other submit path: a cancel or
                // replace that has seen no echo yet computes `{id}.{ver}` for
                // OrigClOrdID, and a bare id would not match. The ids are freshly
                // allocated, so the version is 0. Tag 6107 below reads the same
                // string, which is the form `send_order_ex` sends a parent link on.
                let parent_str = format!("{parent_id}.0");
                let tp_str = format!("{tp_id}.0");
                let sl_str = format!("{sl_id}.0");
                let entry_str = format_price(entry_price);
                let tp_price_str = format_price(take_profit);
                let sl_price_str = format_price(stop_loss);
                let oca_group = format!("OCA_{parent_id}");

                // 1. Parent order: limit entry
                context.insert_order(crate::types::Order::new(
                    parent_id,
                    instrument,
                    side,
                    qty,
                    entry_price,
                    b'2',
                    b'0',
                    0,
                ));
                let now = chrono_free_timestamp();
                let parent_sent = conn.send_fix(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_NEW_ORDER),
                    (fix::TAG_SENDING_TIME, &now),
                    (11, &parent_str),
                    (1, account_id),
                    (55, &symbol),
                    (54, side_str),
                    (38, &qty_str),
                    (40, "2"), // Limit
                    (44, &entry_str),
                    (59, "0"), // DAY
                    (60, &now),
                    (167, &sec_type_str),
                    (100, &destination),
                    (6210, &destination),
                    (15, "USD"),
                    (204, CUSTOMER),
                ]);

                // 2. Take-profit child: limit exit, linked to parent, in OCA group
                context.insert_order(crate::types::Order::new(
                    tp_id,
                    instrument,
                    exit_side,
                    qty,
                    take_profit,
                    b'2',
                    b'1',
                    0,
                ));
                let now = chrono_free_timestamp();
                let tp_sent = conn.send_fix(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_NEW_ORDER),
                    (fix::TAG_SENDING_TIME, &now),
                    (11, &tp_str),
                    (1, account_id),
                    (55, &symbol),
                    (54, exit_side_str),
                    (38, &qty_str),
                    (40, "2"), // Limit
                    (44, &tp_price_str),
                    (59, "1"), // GTC
                    (60, &now),
                    (167, &sec_type_str),
                    (100, &destination),
                    (6210, &destination),
                    (15, "USD"),
                    (204, CUSTOMER),
                    (6107, &parent_str),            // ParentOrderID
                    (583, &oca_group),              // OCAGroup
                    (6209, "ReduceOnFillNonBlock"), // OCA type: gateway default 3 (ibx#215)
                ]);

                // 3. Stop-loss child: stop exit, linked to parent, in OCA group
                context.insert_order(crate::types::Order::new(
                    sl_id, instrument, exit_side, qty, stop_loss, b'3', b'1', stop_loss,
                ));
                let now = chrono_free_timestamp();
                // The legs go out as three messages and the arm reports one
                // outcome. Reporting only the last meant a parent that never
                // left was silence, with two children tracked against it.
                parent_sent.and(tp_sent).and(conn.send_fix(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_NEW_ORDER),
                    (fix::TAG_SENDING_TIME, &now),
                    (11, &sl_str),
                    (1, account_id),
                    (55, &symbol),
                    (54, exit_side_str),
                    (38, &qty_str),
                    (40, "3"), // Stop
                    (99, &sl_price_str),
                    (59, "1"), // GTC
                    (60, &now),
                    (167, &sec_type_str),
                    (100, &destination),
                    (6210, &destination),
                    (15, "USD"),
                    (204, CUSTOMER),
                    (6107, &parent_str),            // ParentOrderID
                    (583, &oca_group),              // OCAGroup
                    (6209, "ReduceOnFillNonBlock"), // OCA type: gateway default 3 (ibx#215)
                ]))
            }
            OrderRequest::SubmitLimitFractional { order_id, instrument, side, qty, price } => {
                context.insert_order(crate::types::Order::new(
                    order_id, instrument, side, 0, price, b'2', b'0', 0,
                ));
                let ver = *context.modify_versions.get(&order_id).unwrap_or(&0);
                let clord_str = format!("{order_id}.{ver}");
                let side_str = fix_side(side);
                let qty_str = format_qty(qty);
                let price_str = format_price(price);
                let symbol = context.market.symbol(instrument).to_string();
                let (sec_type_str, destination) = context.market.order_routing(instrument);
                let now = chrono_free_timestamp();
                conn.send_fix(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_NEW_ORDER),
                    (fix::TAG_SENDING_TIME, &now),
                    (11, &clord_str),
                    (1, account_id),
                    (55, &symbol),
                    (54, side_str),
                    (38, &qty_str), // Decimal qty (e.g., "0.5")
                    (40, "2"),      // OrdType = Limit
                    (44, &price_str),
                    (59, "0"),
                    (60, &now),
                    (167, &sec_type_str),
                    (100, &destination),
                    (6210, &destination),
                    (15, "USD"),
                    (204, CUSTOMER),
                ])
            }
            OrderRequest::Cancel { order_id } => {
                let result = send_cancel(conn, context, account_id, order_id);
                if result.is_ok() {
                    synthesize_pending_cancel(context, shared, order_id, event_tx);
                }
                result
            }
            OrderRequest::CancelAll { instrument } => {
                let open_ids: Vec<u64> =
                    context.open_orders_for(instrument).iter().map(|o| o.order_id).collect();
                let mut last_result = Ok(());
                for oid in open_ids {
                    last_result = send_cancel(conn, context, account_id, oid);
                    if last_result.is_ok() {
                        synthesize_pending_cancel(context, shared, oid, event_tx);
                    }
                }
                last_result
            }
            OrderRequest::Modify {
                new_order_id,
                order_id,
                price,
                qty,
                outside_rth,
                ord_type,
                tif,
                stop_price,
            } => {
                let orig = context.order(order_id).copied();
                let spec = context.submitted.get(&order_id).cloned();
                // What the replace states. A zero field states nothing, so the
                // resting order's value stays in force — which is what the
                // encoder used to do for every field, so a caller changing the
                // order type, the time-in-force or the trigger had the change
                // accepted, acknowledged, and dropped (ibx#349, ibx#372).
                // Whether the caller named the type, kept before the fallback
                // below overwrites it. A trigger on the request only means one
                // when the replace also states what it is replacing into.
                let ord_type_stated = ord_type != 0;
                let ord_type =
                    if ord_type != 0 { ord_type } else { orig.map_or(b'2', |o| o.ord_type) };
                let tif = if tif != 0 { tif } else { orig.map_or(b'0', |o| o.tif) };
                // Modify carries no instrument, so `snap_prices` cannot reach
                // it and both price-like fields are snapped here against the
                // tracked order's grid (ibx#216). The trigger needs it as much
                // as the limit does — a moved stop off the grid is rejected by
                // the gateway the same way a limit is.
                let (price, stop_price) = orig.map_or((price, stop_price), |o| {
                    let tick = context.market.min_tick_scaled(o.instrument);
                    (
                        crate::types::snap_to_tick(price, tick),
                        if stop_price != 0 {
                            crate::types::snap_to_tick(stop_price, tick)
                        } else {
                            0
                        },
                    )
                });
                // Which tag each price belongs on depends on the order type,
                // and the answer is needed twice: once for what the engine
                // records, once for what goes on the wire. A replacement that
                // recorded the old trigger would leave the next modify
                // restating a price this one just moved.
                let trigger_only = is_trigger_only(ord_type);
                let orig_stop = orig.map_or(0, |o| o.stop_price);
                let type_changed = orig.is_some_and(|o| o.ord_type != ord_type);
                // A two-legged type can have its trigger moved, but only if it
                // has one: b'K' is Limit-if-Touched and Market-to-Limit both,
                // and the tracked trigger is what separates them. Every other
                // type keeps the shape it had, so a trigger supplied on the
                // request cannot become a tag 99 for a limit order.
                //
                // A pegged or relative order tracks its offset in `stop_price`,
                // so its replace does restate that on 99 — unchanged from
                // before, and one of the reasons those types are refused a
                // modify outright (ibx#334).
                // A two-legged type carries a trigger when it has one, and it
                // has one either because the resting order did or because this
                // replace states it. Reading only the resting order sent a
                // stop-limit with no tag 99 at all when a limit was replaced
                // into one, which is not a stop-limit the gateway can accept.
                // A two-legged type carries a trigger when it has one: either
                // the resting order had one, or this replace states both the
                // type and the trigger. Reading only the resting order sent a
                // stop-limit with no tag 99 when a limit was replaced into one.
                // Reading the request alone is worse — the public client fills
                // it from aux_price, which on a market-to-limit is meaningless.
                let carries_trigger = trigger_only
                    || (matches!(ord_type, b'4' | b'K')
                        && if ord_type_stated {
                            // The replace names the type, so only what it also
                            // states belongs to it. The resting trigger was the
                            // old type's: carrying it into a market-to-limit
                            // turns the order into limit-if-touched.
                            stop_price != 0
                        } else {
                            orig_stop != 0
                        });
                let new_stop = if trigger_only && stop_price == 0 {
                    price
                } else if carries_trigger && stop_price != 0 {
                    stop_price
                } else if type_changed && !carries_trigger {
                    // The replace moved the order to a type with no trigger.
                    // The resting order's must not ride along on tag 99 —
                    // a limit order does not have one.
                    0
                } else {
                    orig_stop
                };

                if let Some(orig) = orig {
                    // The moved trigger is recorded too: a replacement that
                    // kept the old one would leave the next modify restating a
                    // price this one just moved.
                    context.insert_order(crate::types::Order::new(
                        new_order_id,
                        orig.instrument,
                        orig.side,
                        qty,
                        price,
                        ord_type,
                        tif,
                        new_stop,
                    ));
                }
                // Versioned ClOrdID chaining: orderId.0 → .1 → .2
                let prev_ver = *context.modify_versions.get(&order_id).unwrap_or(&0);
                let new_ver = prev_ver + 1;
                context.modify_versions.insert(order_id, new_ver);
                let clord_str = format!("{order_id}.{new_ver}");
                // OrigClOrdID matches whatever the server last recorded for
                // this order (which may pre-date the versioned scheme — ibx#179).
                let orig_clord = context
                    .last_clord
                    .get(&order_id)
                    .cloned()
                    .unwrap_or_else(|| format!("{order_id}.{prev_ver}"));
                // Pre-seed `last_clord` with what we're about to emit so a
                // subsequent cancel before the modify-ack still references the
                // right version.
                context.last_clord.insert(order_id, clord_str.clone());
                // The replacement is tracked under its own id, and a caller
                // that modifies it again names that one. The broker still knows
                // the order by the ClOrdID just sent, so the chain has to be
                // reachable from both: without this the next replace states an
                // OrigClOrdID the broker has never seen and is refused, which
                // is what a second modify in a row did.
                if new_order_id != order_id {
                    context.last_clord.insert(new_order_id, clord_str.clone());
                    context.modify_versions.insert(new_order_id, new_ver);
                    // The replacement is the order now, and the next replace
                    // restates from it. Without this the second modify of an
                    // order dropped everything the first one preserved.
                    if let Some(spec) = spec.clone() {
                        context.submitted.insert(new_order_id, spec);
                        // The order answers to the new id from here. Left
                        // behind, the old entry is retired by an id the caller
                        // no longer uses, so it stays for the session.
                        if new_order_id != order_id {
                            context.submitted.remove(&order_id);
                        }
                    }
                }

                let qty_str = format_uint(qty as u64);
                let price_str = format_price(price);
                let now = chrono_free_timestamp();
                let side_str = orig.map(|o| fix_side(o.side)).unwrap_or("1");
                let symbol = orig
                    .map(|o| context.market.symbol(o.instrument).to_string())
                    .unwrap_or_default();
                // A replace names the contract by the venue's own local symbol,
                // which is the same string as the symbol for a stock and a
                // different one for anything with an expiry or a strike. Naming
                // the family there says nothing about which member is being
                // replaced.
                let local_symbol = orig
                    .and_then(|o| context.market.order_identity(o.instrument))
                    .map(|id| id.local_symbol)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| symbol.clone());
                let (sec_type_str, _destination) = orig
                    .map(|o| context.market.order_routing(o.instrument))
                    .unwrap_or_else(|| ("STK".to_string(), "SMART".to_string()));
                let ord_type_str = crate::types::ord_type_fix_str(ord_type).to_string();
                // An order recovered without a stated time-in-force has none to
                // restate. Tag 59 carries a real instruction on a replace, so a
                // guess here would set what the gateway is holding — omitted
                // instead, leaving the resting order's own value in force.
                let tif_str = std::str::from_utf8(&[tif]).unwrap_or("0").to_string();
                let con_id_str = orig
                    .and_then(|o| context.market.con_id(o.instrument))
                    .map(|c| c.to_string())
                    .unwrap_or_default();

                // Lean modify message — omit identity tags (6121, 6119, 231, 100, 15, 204)
                let mut fields: Vec<(u32, &str)> = vec![
                    (fix::TAG_MSG_TYPE, fix::MSG_ORDER_REPLACE),
                    (fix::TAG_SENDING_TIME, &now),
                    (11, &clord_str),  // ClOrdID (versioned)
                    (41, &orig_clord), // OrigClOrdID (previous version)
                ];
                // Each price goes to the tag its order type uses. A
                // trigger-only type has no limit leg, so its price is the
                // trigger and tag 44 is left off entirely — the shape its own
                // submit has. Anything else keeps 44 for the limit.
                if !trigger_only {
                    fields.push((44, &price_str)); // Price
                }
                fields.push((1, account_id)); // Account
                fields.push((6122, "c")); // Client version
                // OutsideRTH, from the order the caller resubmitted rather than
                // hard-coded: the tracked record cannot express it, and asserting
                // 1 unconditionally opted every modified order into the extended
                // session (ibx#247). Same position it held in the capture.
                if outside_rth {
                    fields.push((6433, "1"));
                }
                let rest: [(u32, &str); 11] = [
                    (38, &qty_str),       // OrderQty
                    (54, side_str),       // Side
                    (40, &ord_type_str),  // OrdType
                    (55, &symbol),        // Symbol
                    (167, &sec_type_str), // SecurityType
                    (6035, &local_symbol), // the contract, not the family
                    (59, &tif_str),       // TIF — dropped below when unstated
                    (6008, &con_id_str),  // ConId
                    (6088, "Socket"),     // Connection type
                    (6211, ""),           // Empty (matches reference)
                    (6238, ""),           // Empty (matches reference)
                ];
                fields.extend(rest);
                if tif == crate::types::TIF_UNSTATED {
                    fields.retain(|(tag, _)| *tag != 59);
                }
                // The trigger the caller moved, or the one the order already had.
                let stop_str;
                if new_stop != 0 {
                    stop_str = format_price(new_stop);
                    fields.push((99, &stop_str));
                }
                // The gateway takes a replace as a full statement of the order,
                // not a difference against the resting one. This stated the
                // identity, the price and the quantity and stopped, so a
                // replaced order came back without the algo, the all-or-none
                // instruction, the good-till date or anything else it was
                // placed with — accepted, acknowledged, and quietly a different
                // order. What the submit said is restated here.
                let mut attr_fields: Vec<(u32, String)> = Vec::new();
                if let Some(spec) = spec.as_deref() {
                    push_order_attrs(
                        &mut attr_fields,
                        &spec.attrs,
                        &spec.kind,
                        orig.map(|o| o.side).unwrap_or(Side::Buy),
                        exec_inst_for(&spec.kind),
                    );
                    // Stated once. The lean message already names these, and the
                    // gateway reads a repeated tag as a second statement of it.
                    let stated: Vec<u32> = fields.iter().map(|(t, _)| *t).collect();
                    attr_fields.retain(|(tag, _)| !stated.contains(tag));
                }
                fields.extend(attr_fields.iter().map(|(t, v)| (*t, v.as_str())));
                conn.send_fix(&fields)
            }
        };
        match result {
            Ok(()) => hb.last_ccp_sent = Instant::now(),
            Err(e) => {
                // The caller is told, which is the whole of ibx#116 — it was
                // the silence that left a phantom position. What it is told is
                // that the state is not known: a write reporting failure may
                // already have put the frame on the wire, as the transport
                // says of TLS in as many words, so calling this a rejection
                // invited a resubmission of an order that may be working.
                //
                // Nothing is discarded and nothing is rolled back. The failure
                // abandons the transport, the reconnect that follows brings the
                // server's own account of what it holds, and `last_clord` is
                // re-recorded from that echo. Where the recovery accounts for
                // none of it, the sweep says so rather than this guessing.
                log::error!("Failed to send order {oid}: {e} — its state is not known");
                if oid != 0 {
                    // Back to the last thing the broker was known to hold, and
                    // marked as no longer known. The attempt is dropped: it was
                    // never accepted, and hydration would otherwise read it as
                    // the order's own truth for every field the recovery push
                    // does not state.
                    if let Some(new_id) = speculative {
                        context.remove_order(new_id);
                    }
                    if let Some(prior) = before {
                        context.insert_order(prior);
                    }
                    context.set_order_status_forced(oid, OrderStatus::Uncertain);
                    let update = OrderUpdate {
                        order_id: oid,
                        instrument: 0,
                        status: OrderStatus::Uncertain,
                        filled_qty: 0.0,
                        remaining_qty: 0.0, avg_price: 0,
                        perm_id: 0,
                        parent_id: 0,
                        timestamp_ns: 0,
                    };
                    shared.orders.push_order_update(update);
                    // An order whose state is no longer known is the one thing
                    // a caller must not have to ask for. It was recorded and
                    // not announced, so a caller reading the event channel —
                    // told this is a second delivery of everything, not a
                    // lesser one — was not told at all.
                    crate::engine::hot_loop::emit(
                        event_tx,
                        crate::bridge::Event::OrderUpdate(update),
                    );
                }
            }
        }
    }
    context.pending_orders.requeue_front(unsent);
}

/// Convert Side to FIX tag 54 value.
/// Everything a cancel states, in one place because the two callers stated it
/// twice and drifted.
///
/// The terminal names five fields and stops: ClOrdID, OrigClOrdID, Side,
/// Account and Originator. It writes no TransactTime anywhere in the order
/// path, so neither does this.
fn send_cancel(
    conn: &mut Connection,
    context: &mut Context,
    account_id: &str,
    order_id: u64,
) -> std::io::Result<()> {
    // OrigClOrdID must match exactly what the server has on record. Prefer the
    // string last observed on the wire (see ibx#179 — legacy orders recorded
    // without a `.{ver}` suffix won't match a computed `{id}.0`). Fall back to
    // the versioned scheme when there is no observation yet (fresh-order
    // place->immediate-cancel before the ack round-trip).
    let orig_clord = context.last_clord.get(&order_id).cloned().unwrap_or_else(|| {
        let ver = *context.modify_versions.get(&order_id).unwrap_or(&0);
        format!("{order_id}.{ver}")
    });
    let attempt = context.cancel_attempts.entry(order_id).and_modify(|n| *n += 1).or_insert(0);
    let clord_str =
        if *attempt == 0 { format!("C{order_id}") } else { format!("C{order_id}.{attempt}") };
    let now = chrono_free_timestamp();
    let tracked = context.order(order_id).copied();
    let side = tracked.map(|o| fix_side(o.side));
    // A cancel names the order, and also states what it is cancelling: the
    // quantity and the contract. Naming only the order left the venue to look
    // both up.
    let qty_str = tracked.map(|o| format_uint(o.qty as u64).to_string());
    let con_id_str = tracked
        .and_then(|o| context.market.con_id(o.instrument))
        .filter(|c| *c != 0)
        .map(|c| c.to_string());
    let mut fields = vec![
        (fix::TAG_MSG_TYPE, fix::MSG_ORDER_CANCEL),
        (fix::TAG_SENDING_TIME, &now),
        (11, &clord_str),
        (41, &orig_clord),
        (1, account_id),
        (6088, "Socket"),
    ];
    // Stated when it is known rather than defaulted: the cancel is keyed by
    // OrigClOrdID, and a guessed side is a claim about someone's order that
    // nothing here can stand behind.
    if let Some(qty) = qty_str.as_deref() {
        fields.push((38, qty));
    }
    if let Some(side) = side {
        fields.push((54, side));
    }
    if let Some(con_id) = con_id_str.as_deref() {
        fields.push((6008, con_id));
    }
    conn.send_fix(&fields)
}

fn fix_side(side: Side) -> &'static str {
    match side {
        Side::Buy => "1",
        Side::Sell => "2",
        Side::ShortSell => "5",
    }
}

/// Synthesize the PendingCancel phase when a cancel request goes out
/// (ibx#211): the server acks a normal cancel with the terminal code only —
/// it never sends the pending-cancel code — so without this local
/// transition consumers jump straight from Submitted to Cancelled. The
/// server's ack (or a fill that raced the cancel) then advances the status;
/// a cancel reject restores the working status via the forced setter.
fn synthesize_pending_cancel(
    context: &mut Context,
    shared: &Arc<SharedState>,
    order_id: crate::types::OrderId,
    event_tx: &Option<std::sync::mpsc::SyncSender<crate::bridge::Event>>,
) {
    if !context.update_order_status(order_id, OrderStatus::PendingCancel) {
        return; // unknown order, already terminal, or already pending-cancel
    }
    if let Some(order) = context.order(order_id).copied() {
        let update = OrderUpdate {
            order_id,
            instrument: order.instrument,
            status: OrderStatus::PendingCancel,
            filled_qty: order.filled as f64,
            remaining_qty: order.qty as f64 - order.filled as f64, avg_price: 0,
            perm_id: 0,
            parent_id: 0,
            timestamp_ns: context.now_ns(),
        };
        shared.orders.push_order_update(update);
        crate::engine::hot_loop::emit(event_tx, crate::bridge::Event::OrderUpdate(update));
    }
}

/// Map the OCA type code (1..=4) to its tag 6209 wire label. 0/unset and
/// out-of-range coerce to 3 (ReduceOnFillNonBlock), the gateway default
/// (ibx#215).
/// Unit a trailing amount is expressed in, on tag 6268: percent, as against
/// an absolute amount (0) or ticks (1).
const TRAIL_UNIT_PERCENT: u32 = 100;

/// The SecurityIDSource an order states when the SecurityID it carries is the
/// venue's own local symbol rather than a public identifier. Not one of the
/// published sources, which are single characters.
const IB_LOCAL_SYMBOL_SOURCE: &str = "101";

/// What this client calls itself when a message asks who originated it.
const ORIGINATOR: &str = "Socket";

/// Who the order is for. The venue requires it stated and this client places
/// orders for the account that authenticated it.
const CUSTOMER: &str = "0";

fn oca_type_str(oca_type: u8) -> &'static str {
    match oca_type {
        1 => "CancelOnFillWBlock",
        2 => "ReduceOnFillWBlock",
        4 => "ReduceOnFillWBlockFromTotal",
        _ => "ReduceOnFillNonBlock",
    }
}

/// Order types whose price *is* the trigger: they have no limit leg, so a
/// replace states the price in tag 99 and sends no tag 44 at all — which is
/// the shape their own submit has.
fn is_trigger_only(ord_type: u8) -> bool {
    matches!(ord_type, b'3' | b'J') || ord_type == crate::types::ORD_STP_PRT
}

/// One shared encoder for every extended order submission (ibx#224): the
/// order-type-specific tags come from `kind`; the TIF and the full
/// `OrderAttrs` block are emitted identically for all kinds.
/// `SubmitLimitEx`, `SubmitTrailingStopPctEx` and `SubmitEx` all route
/// through here so the attrs emission cannot drift between order types.
#[allow(clippy::too_many_arguments)]
/// Restate the contract identity on an order for anything a symbol does not
/// name on its own. Without these an option order says nothing about which
/// strike, right or expiry it means and a future says nothing about its
/// contract month, which is why those types were refused outright rather than
/// sent under-specified (ibx#202).
fn push_contract_identity(
    fields: &mut Vec<(u32, String)>,
    context: &Context,
    instrument: crate::types::InstrumentId,
) {
    // Name the contract by its id where one is known — before anything else,
    // and for every kind of contract including a stock, which has no other
    // identity to state. The terminal writes this on every order it can, and it
    // is the only field that names a contract exactly: the rest describe one
    // and leave the venue to match, which is how a description that matches
    // nothing becomes "Unknown contract" and one that matches several becomes
    // "Ambiguous".
    if let Some(con_id) = context.market.con_id(instrument)
        && con_id != 0
    {
        fields.push((6008, con_id.to_string()));
    }
    let Some(id) = context.market.order_identity(instrument) else {
        return;
    };
    let crate::engine::market_state::OrderIdentity {
        expiry,
        strike,
        right,
        multiplier,
        trading_class,
        local_symbol,
        currency: _,
    } = id;
    let (sec_type, _) = context.market.order_routing(instrument);
    // How an order names one contract rather than a family.
    //
    // A stock is named by its symbol and nothing else. Every other kind is
    // named by the venue's own local symbol on SecurityID, under the source
    // that says the identifier is the venue's own, and states no trading
    // class: a class describes a family, which is what left a futures order
    // ambiguous. Options are the exception here only because they are accepted
    // as they stand and are left exactly as they were.
    let names_itself_by_local_symbol =
        matches!(sec_type.as_str(), "FUT" | "FWD" | "IND" | "BOND" | "CFD" | "CRYPTO" | "WAR");
    // Which kinds state a maturity, and in what form. A future and a warrant
    // state the contract month and carry no maturity date at all. An option
    // states what it has always stated, because that is accepted.
    let states_contract_month = matches!(sec_type.as_str(), "FUT" | "FWD" | "WAR");
    if states_contract_month {
        let month: String = expiry.chars().take(6).collect();
        if month.len() == 6 {
            fields.push((200, month));
        }
    } else if !names_itself_by_local_symbol
        && let Some(tag) = super::ccp::maturity_tag(&expiry)
    {
        fields.push((tag, expiry));
    }
    if names_itself_by_local_symbol {
        if !local_symbol.is_empty() {
            fields.push((48, local_symbol));
            fields.push((22, IB_LOCAL_SYMBOL_SOURCE.to_string()));
        }
    } else {
        if !trading_class.is_empty() {
            fields.push((6058, trading_class));
        }
        if !local_symbol.is_empty() {
            fields.push((6035, local_symbol));
        }
    }
    if strike.parse::<f64>().unwrap_or(0.0) > 0.0 {
        fields.push((202, strike));
    }
    // PutOrCall is a code on this wire, not the letter: Call = 1, Put = 0, the
    // same mapping the security-definition request uses. Sending "C" would name
    // no side the gateway recognises.
    match right.to_ascii_uppercase().as_str() {
        "C" | "CALL" | "1" => fields.push((201, "1".to_string())),
        "P" | "PUT" | "0" => fields.push((201, "0".to_string())),
        "" => {}
        other => {
            log::warn!("order for an option with an unrecognised right {other:?}; omitting tag 201")
        }
    }
    if !multiplier.is_empty() {
        fields.push((231, multiplier));
    }
}

fn send_order_ex(
    conn: &mut Connection,
    context: &mut Context,
    account_id: &str,
    order_id: crate::types::OrderId,
    instrument: crate::types::InstrumentId,
    side: Side,
    qty: u32,
    kind: crate::types::OrderKind,
    tif: u8,
    attrs: &crate::types::OrderAttrs,
) -> std::io::Result<()> {
    use crate::types::OrderKind as K;

    // Engine-state entry: ord_type byte, tracked price, and tracked stop
    // price per kind — mirrors the corresponding plain variants exactly.
    let (ord_type_byte, track_price, track_stop) = match kind {
        K::Market => (b'1', 0, 0),
        K::Limit { price } => (b'2', price, 0),
        K::Stop { stop_price } => (b'3', stop_price, stop_price),
        K::StopLimit { price, stop_price } => (b'4', price, stop_price),
        K::TrailingStop { .. } => (b'P', 0, 0),
        K::TrailingStopLimit { lmt_offset, .. } => (b'P', lmt_offset, 0),
        K::TrailPct { .. } => (b'P', 0, 0),
        K::Moc => (b'5', 0, 0),
        K::Loc { price } => (b'B', price, 0),
        K::Mit { stop_price } => (b'J', stop_price, stop_price),
        K::Lit { price, stop_price } => (b'K', price, stop_price),
        K::PegBench { price, .. } => (crate::types::ORD_PEG_BENCH, price, 0),
        K::Mtl => (b'K', 0, 0),
        K::MktPrt => (b'U', 0, 0),
        K::StpPrt { stop_price } => (crate::types::ORD_STP_PRT, 0, stop_price),
        K::MidPrice { price_cap } => (crate::types::ORD_MIDPX, price_cap, 0),
        K::SnapMkt { offset } => (crate::types::ORD_SNAP_MKT, 0, offset),
        K::SnapMid { offset } => (crate::types::ORD_SNAP_MID, 0, offset),
        K::SnapPri { offset } => (crate::types::ORD_SNAP_PRI, 0, offset),
        K::PegMkt { offset, .. } => (crate::types::ORD_PEG_MKT, 0, offset),
        K::PegMid { offset, .. } => (crate::types::ORD_PEG_MID, 0, offset),
        K::Rel { offset } => (b'R', 0, offset),
        K::AdjustableStop { stop_price, .. } => (b'3', 0, stop_price),
        K::Adaptive { price, .. } | K::Algo { price, .. } => (b'2', price, 0),
        // Tracked under the what-if marker so the response is recognised as a
        // preview; it never becomes a live order.
        K::WhatIf { price, .. } => (crate::types::ORD_WHAT_IF, price, 0),
    };
    context.insert_order(crate::types::Order::new(
        order_id,
        instrument,
        side,
        qty,
        track_price,
        ord_type_byte,
        tif,
        track_stop,
    ));
    // Kept so a replace can restate it: the gateway takes a replace as a full
    // statement of the order, so an attribute this submit made and the replace
    // leaves out is an attribute the order loses.
    context.submitted.insert(
        order_id,
        Box::new(crate::types::OrderSpec { kind: kind.clone(), attrs: attrs.clone() }),
    );

    let ver = *context.modify_versions.get(&order_id).unwrap_or(&0);
    let symbol = context.market.symbol(instrument).to_string();
    let (sec_type_str, destination) = context.market.order_routing(instrument);
    let now = chrono_free_timestamp().to_string();
    let tif_byte = [tif];
    let tif_str = std::str::from_utf8(&tif_byte).unwrap_or("0");

    let mut fields: Vec<(u32, String)> = vec![
        (fix::TAG_MSG_TYPE, fix::MSG_NEW_ORDER.to_string()),
        (fix::TAG_SENDING_TIME, now.clone()),
        (11, format!("{order_id}.{ver}")),
        (1, account_id.to_string()),
        (55, symbol),
        (54, fix_side(side).to_string()),
        (38, format_uint(qty as u64).to_string()),
    ];

    // Order type (40) plus its price tags and type-specific companions —
    // identical values to the corresponding plain variants. Kinds that put
    // an instruction in tag 18 (TrailingStop/TrailPct = a, Rel = R) cannot
    // also carry all_or_none (18=G); validate_order rejects that
    // combination up front, and the emission below skips 18=G as a second
    // line of defense.
    // ExecInst is one field with the instructions concatenated, not one field
    // per instruction. The terminal builds it as the order type's own character
    // followed by "G" for all-or-none, and an order that had a character of its
    // own therefore lost its all-or-none entirely — silently, on every
    // trailing, relative, pegged and algo order.
    let exec_inst = exec_inst_for(&kind);
    match kind {
        K::Market => fields.push((40, "1".to_string())),
        K::Limit { price } => {
            fields.push((40, "2".to_string()));
            fields.push((44, format_price(price).to_string()));
        }
        K::Stop { stop_price } => {
            fields.push((40, "3".to_string()));
            fields.push((99, format_price(stop_price).to_string()));
        }
        K::StopLimit { price, stop_price } => {
            fields.push((40, "4".to_string()));
            fields.push((44, format_price(price).to_string()));
            fields.push((99, format_price(stop_price).to_string()));
        }
        K::AdjustableStop { stop_price, .. } => {
            // Base order type only. The 6257+ adjustable tags are appended after
            // the attribute block below, where the dedicated encoder this path
            // replaced put them.
            fields.push((40, "3".to_string())); // OrdType = Stop
            fields.push((99, format_price(stop_price).to_string())); // StopPx
        }
        K::TrailingStop { trail_amt, .. } => {
            // Per ib-agent#136 capture: amount-based trailing stop carries
            // the trail amount in both 99 and 211 and requires 18=a.
            let t = format_price(trail_amt).to_string();
            fields.push((40, "P".to_string()));
            fields.push((99, t.clone()));
            fields.push((211, t));
        }
        K::TrailingStopLimit { lmt_offset, trail_amt, .. } => {
            // Per ib-agent#136 capture: TRAIL LIMIT uses OrdType=TSL, no
            // tag 44, no tag 18; trail amount in both 99 and 211; 6370 is
            // the limit-vs-trail offset.
            let t = format_price(trail_amt).to_string();
            fields.push((40, "TSL".to_string()));
            fields.push((99, t.clone()));
            fields.push((6370, format_price(lmt_offset).to_string()));
            fields.push((211, t));
        }
        K::TrailPct { trail_pct, .. } => {
            // The percent itself goes on 99 and 211 in decimal form (1.00 for
            // 1%). Tag 6268 is not the amount but the unit the trail is
            // expressed in — 100 for percent, 0 for an amount, 1 for ticks —
            // and it was being filled with the percent in basis points. A one
            // percent trail is 100 basis points, which is also the code for
            // percent, so the one case anybody tried was right by coincidence
            // and every other percentage sent a unit that is not a unit.
            let pct_decimal = format!("{:.2}", trail_pct as f64 / 100.0);
            fields.push((40, "P".to_string()));
            fields.push((99, pct_decimal.clone()));
            fields.push((211, pct_decimal));
            fields.push((6268, TRAIL_UNIT_PERCENT.to_string()));
        }
        K::Moc => fields.push((40, "5".to_string())),
        K::Loc { price } => {
            fields.push((40, "B".to_string()));
            fields.push((44, format_price(price).to_string()));
        }
        K::Mit { stop_price } => {
            fields.push((40, "J".to_string()));
            fields.push((99, format_price(stop_price).to_string()));
        }
        K::Lit { price, stop_price } => {
            fields.push((40, "LT".to_string())); // per ib-agent#138
            fields.push((44, format_price(price).to_string()));
            fields.push((99, format_price(stop_price).to_string()));
        }
        K::Mtl => fields.push((40, "K".to_string())),
        K::MktPrt => fields.push((40, "U".to_string())),
        K::StpPrt { stop_price } => {
            fields.push((40, "SP".to_string()));
            fields.push((99, format_price(stop_price).to_string()));
        }
        K::MidPrice { .. } => fields.push((40, "MIDPX".to_string())),
        // The offset rides tag 211, which the gateway requires: without it the
        // order comes back "Message must contain field # 211" and is never
        // worked. Seen on all three against a paper account.
        K::SnapMkt { offset } => {
            fields.push((40, "SMKT".to_string()));
            fields.push((211, format_price(offset).to_string()));
        }
        K::SnapMid { offset } => {
            fields.push((40, "SMID".to_string()));
            fields.push((211, format_price(offset).to_string()));
        }
        K::SnapPri { offset } => {
            fields.push((40, "SREL".to_string()));
            fields.push((211, format_price(offset).to_string()));
        }
        // Both are OrdType "E" and are separated by ExecInst, which is what
        // ORD_PEG_MKT and ORD_PEG_MID state in types.rs. Emitting only the
        // OrdType sent the two as the same message, saying which peg neither.
        K::PegBench {
            ref_con_id,
            is_peg_decrease,
            pegged_change_amount,
            ref_change_amount,
            starting_price,
            stock_ref_price,
            ref ref_exchange,
            ..
        } => {
            fields.push((40, "PB".to_string()));
            fields.push((6941, ref_con_id.to_string()));
            // The change amount carries its own direction: there is no separate
            // field saying which way it moves, so a decrease is a negative one.
            let signed = if is_peg_decrease { -pegged_change_amount } else { pegged_change_amount };
            fields.push((6938, format_price(signed).to_string()));
            fields.push((6939, format_price(ref_change_amount).to_string()));
            fields.push((6942, ref_exchange.clone()));
            fields.push((6580, format_price(stock_ref_price).to_string()));
            fields.push((99, format_price(starting_price).to_string()));
        }
        // The venue names these back as PegToMkt and PegToMid under "P", and
        // named them something else entirely under "E" — so an order a caller
        // asked to peg was read as another type. The offset rides 211, which
        // the shared attrs block already writes for the pegged kinds.
        K::PegMkt { .. } => {
            fields.push((40, "P".to_string()));
        }
        K::PegMid { .. } => {
            fields.push((40, "P".to_string()));
        }
        K::Rel { offset } => {
            // Per ib-agent#138 capture: Relative shares OrdType=P and is
            // disambiguated by 18=R; peg offset on 211, no tag 44.
            fields.push((40, "P".to_string()));
            fields.push((211, format_price(offset).to_string()));
        }
        K::Adaptive { price, .. } => {
            // Per ib-agent#136 capture: Adaptive needs 18=e (ExecInst = adaptive
            // algo wrapper). Without it the gateway rejects with "Invalid value
            // in field # 18". The strategy and its one parameter are appended
            // after the attribute block, where the encoder this replaced put them.
            fields.push((40, "2".to_string()));
            fields.push((44, format_price(price).to_string()));
        }
        K::Algo { price, .. } => {
            fields.push((40, "2".to_string()));
            fields.push((44, format_price(price).to_string()));
            // Same marker the adaptive wrapper carries: an order handed to an
            // algo says so here, and the gateway refuses every one that does
            // not with "Invalid value in field # 18" — which it also answers
            // for a value that is merely the wrong one, so the six algo types
            // were refused identically whether the field was absent or wrong.
        }
        K::WhatIf { price, ord_type } => {
            fields.push((40, (ord_type as char).to_string()));
            // A market preview has no price to state, and stating one is how a
            // market-only security came to be refused as a limit.
            if ord_type != b'1' {
                fields.push((44, format_price(price).to_string()));
            }
        }
    }

    fields.push((59, tif_str.to_string()));
    fields.push((60, now));
    fields.push((167, sec_type_str.clone()));
    push_contract_identity(&mut fields, context, instrument);
    // Who placed the order. Every order states it, and a cancel and a market
    // data subscription already did; a new order was the one message that left
    // it out.
    fields.push((6088, ORIGINATOR.to_string()));
    // The venue refuses an order that does not state this: "Must specify
    // Customer Or Firm flag". The reference client's order writer does not
    // emit it, so it reaches the venue some other way there, but what this
    // client sends has to satisfy the venue rather than match the writer.
    fields.push((204, CUSTOMER.to_string()));
    // Stated with no value on every order the venue's own client sends: the
    // alert an order came from, and what that alert asked for. An order that
    // came from no alert still says so.
    fields.push((6211, String::new()));
    fields.push((6238, String::new()));
    // MIDPX / SNAP* / PEG* require a directed exchange; everything else
    // routes per the instrument's registered routing (ibx#217).
    let destination = match kind {
        K::MidPrice { .. }
        | K::SnapMkt { .. }
        | K::SnapMid { .. }
        | K::SnapPri { .. }
        | K::PegMkt { .. }
        | K::PegMid { .. } => "ISLAND".to_string(),
        _ => destination,
    };
    fields.push((100, destination.clone()));
    // Secondary routing field — the reference encoder always writes it
    // alongside the destination (ib-agent#165).
    fields.push((6210, destination));
    // What the contract is priced in, where the caller has said. It was a
    // constant, which is right for a US instrument and wrong for every other:
    // an order on a contract quoted in another currency named a contract that
    // is not the one it meant.
    fields.push((
        15,
        context
            .market
            .order_identity(instrument)
            .map_or_else(|| "USD".to_string(), |id| id.currency),
    ));

    push_order_attrs(&mut fields, attrs, &kind, side, exec_inst);

    let refs: Vec<(u32, &str)> = fields.iter().map(|(t, s)| (*t, s.as_str())).collect();
    conn.send_fix(&refs)
}

/// Everything an order states beyond its identity, contract and price, in the
/// tag order the reference encoder uses. A replace restates all of it, so this
/// is shared rather than spelled out twice.
/// The instruction characters an order type contributes to tag 18. They were
/// pushed from inside the type's own arm, which a replace does not run — so a
/// replaced algo or pegged order lost the instruction that made it one.
fn exec_inst_for(kind: &crate::types::OrderKind) -> String {
    use crate::types::OrderKind as K;
    match kind {
        K::TrailingStop { .. } | K::TrailPct { .. } => "a",
        K::PegMkt { .. } => "P",
        K::PegMid { .. } => "M",
        K::Rel { .. } => "R",
        // Pegged to a benchmark states the same instruction a relative order
        // does, beside its own order type. It had stated none.
        K::PegBench { .. } => "R",
        K::Adaptive { .. } | K::Algo { .. } => "e",
        _ => "",
    }
    .to_string()
}

fn push_order_attrs(
    fields: &mut Vec<(u32, String)>,
    attrs: &crate::types::OrderAttrs,
    kind: &crate::types::OrderKind,
    // The side, because a short sale states where the stock comes from even
    // when the caller names no slot.
    side: Side,
    // Composed from the order's own kind before this is reached: the pegged,
    // relative, trailing and algo types each contribute a character, and the
    // all-or-none instruction below joins them on one field.
    mut exec_inst: String,
) {
    use crate::types::OrderKind as K;
    // Extended attributes — same tag order as the historical SubmitLimitEx
    // block.
    if attrs.display_size > 0 {
        fields.push((111, format_uint(attrs.display_size as u64).to_string()));
    }
    if attrs.min_qty > 0 {
        fields.push((110, format_uint(attrs.min_qty as u64).to_string()));
    }
    if attrs.outside_rth {
        fields.push((6433, "1".to_string()));
    }
    if attrs.hidden {
        fields.push((6135, "1".to_string()));
    }
    if attrs.good_after > 0 {
        fields.push((168, unix_to_ib_datetime(attrs.good_after)));
    }
    // GTD expiry: date-only -> tag 432; time-precise -> tag 126 (UTC).
    // Mutually exclusive — never both (gateway rejects both together).
    if attrs.good_till_date_ymd > 0 {
        fields.push((432, format!("{:08}", attrs.good_till_date_ymd)));
    } else if attrs.good_till > 0 {
        fields.push((126, unix_to_ib_utc_dash(attrs.good_till)));
    }
    let oca_str = if !attrs.oca_group_str.is_empty() {
        attrs.oca_group_str.clone()
    } else if attrs.oca_group > 0 {
        format!("OCA_{}", attrs.oca_group)
    } else {
        String::new()
    };
    if !oca_str.is_empty() {
        fields.push((583, oca_str));
        fields.push((6209, oca_type_str(attrs.oca_type).to_string()));
    }
    if attrs.parent_id > 0 {
        // Match parent ClOrdID format: "{order_id}.{ver}" — assume ver=0
        // for initial submission.
        fields.push((6107, format!("{}.0", attrs.parent_id)));
    }
    if attrs.discretionary_amt > 0 {
        fields.push((9813, format_price(attrs.discretionary_amt).to_string()));
    }
    if attrs.sweep_to_fill {
        fields.push((6102, "1".to_string()));
    }
    if attrs.all_or_none {
        exec_inst.push('G');
    }
    if !exec_inst.is_empty() {
        fields.push((18, exec_inst));
    }
    // Instructions the caller set that used to reach no encoder. Each changes
    // what is traded, so each goes on the wire: a volatility order priced in
    // volatility, an offset the venue works from, a discretion the floor is
    // told about, the caller's own reference, and whether this opens a position
    // or closes one.
    if attrs.volatility > 0.0 {
        fields.push((9816, format!("{:.6}", attrs.volatility)));
    }
    if attrs.volatility_type > 0 {
        fields.push((6280, attrs.volatility_type.to_string()));
    }
    // What a volatility order does as the underlying moves: whether the venue
    // re-prices it, which price it references, and the band it stays inside.
    // A caller could state all four and have none of them sent.
    if attrs.seek_price_improvement {
        fields.push((6557, "1".to_string()));
    }
    // When a person entered the order by hand. A record the venue keeps, so an
    // order that states it and does not send it is recorded as something else.
    if !attrs.manual_order_time.is_empty() {
        fields.push((6532, attrs.manual_order_time.clone()));
    }
    // An error the caller has decided to send the order past anyway.
    if !attrs.advanced_error_override.is_empty() {
        fields.push((8229, attrs.advanced_error_override.clone()));
    }
    // The window an order is live in, and whether it may take liquidity.
    if !attrs.active_start_time.is_empty() {
        fields.push((6670, attrs.active_start_time.clone()));
    }
    if !attrs.active_stop_time.is_empty() {
        fields.push((6671, attrs.active_stop_time.clone()));
    }
    if attrs.post_only {
        fields.push((6605, "1".to_string()));
    }
    // Who asked for the order. A regulatory statement, not a preference, and
    // wrong by omission where it applies.
    if attrs.solicited {
        fields.push((6488, "1".to_string()));
    }
    if attrs.manual_order_indicator > 0 {
        fields.push((1028, attrs.manual_order_indicator.to_string()));
    }
    if attrs.route_marketable_to_bbo {
        fields.push((8265, "1".to_string()));
    }
    if attrs.imbalance_only {
        fields.push((6737, "1".to_string()));
    }
    if attrs.allow_pre_open {
        fields.push((6524, "1".to_string()));
    }
    if attrs.ignore_open_auction {
        fields.push((6562, "1".to_string()));
    }
    if attrs.is_oms_container {
        fields.push((6406, "1".to_string()));
    }
    if !attrs.ext_operator.is_empty() {
        fields.push((8089, attrs.ext_operator.clone()));
    }
    if !attrs.customer_account.is_empty() {
        fields.push((6207, attrs.customer_account.clone()));
    }
    if attrs.professional_customer {
        fields.push((6636, "1".to_string()));
    }
    if attrs.ref_futures_con_id > 0 {
        fields.push((6564, attrs.ref_futures_con_id.to_string()));
    }
    // Who decided the trade and who executed it. An order that states none of
    // these where the venue expects them is reported without them.
    if !attrs.mifid2_decision_maker.is_empty() {
        fields.push((8237, attrs.mifid2_decision_maker.clone()));
    }
    if !attrs.mifid2_decision_algo.is_empty() {
        fields.push((8243, attrs.mifid2_decision_algo.clone()));
    }
    if !attrs.mifid2_execution_trader.is_empty() {
        fields.push((8254, attrs.mifid2_execution_trader.clone()));
    }
    if !attrs.mifid2_execution_algo.is_empty() {
        fields.push((8255, attrs.mifid2_execution_algo.clone()));
    }
    // Letting the venue manage the price, how long the order runs, and what it
    // competes against. A caller states these and they reached nothing.
    if attrs.use_price_mgmt_algo > 0 {
        fields.push((8339, attrs.use_price_mgmt_algo.to_string()));
    }
    if attrs.duration != i32::MAX && attrs.duration > 0 {
        fields.push((8402, attrs.duration.to_string()));
    }
    if attrs.min_compete_size > 0 {
        fields.push((8411, attrs.min_compete_size.to_string()));
    }
    if attrs.compete_against_best_offset != f64::MAX {
        fields.push((8412, format!("{:.6}", attrs.compete_against_best_offset)));
    }
    if attrs.continuous_update {
        fields.push((6275, "1".to_string()));
    }
    if attrs.reference_price_type > 0 {
        fields.push((6279, attrs.reference_price_type.to_string()));
    }
    if attrs.stock_range_lower != f64::MAX {
        fields.push((6152, format!("{:.6}", attrs.stock_range_lower)));
    }
    if attrs.stock_range_upper != f64::MAX {
        fields.push((6153, format!("{:.6}", attrs.stock_range_upper)));
    }
    if attrs.percent_offset != f64::MAX {
        fields.push((9822, format!("{:.6}", attrs.percent_offset)));
    }
    if attrs.not_held {
        fields.push((6287, "1".to_string()));
    }
    if !attrs.order_ref.is_empty() {
        fields.push((6010, attrs.order_ref.clone()));
    }
    if !attrs.open_close.is_empty() {
        fields.push((77, attrs.open_close.clone()));
    }
    // Whether this order exercises the option it names or lapses it. The venue
    // has no exercise message: it reads the action off an ordinary order, and
    // it rides here so a replace restates it like every other attribute.
    if attrs.exercise_action != 0 {
        fields.push((6809, attrs.exercise_action.to_string()));
    }
    // The ladder. Sending the sizes and not the step, or the step and not the
    // sizes, describes no ladder at all, so an order that names one names all
    // of what it set.
    // The hedge. An order that asked for one and did not say so left the
    // position naked, which is the opposite of what it was for.
    // A combination states its legs on the order itself. There is no repeating
    // group for them and no standard leg tag: the count goes on 6079 and each
    // leg's contract, ratio and side on 6080, 6081 and 6082, with its venue,
    // position effect and short-sale slot after. The side is a flag, not the
    // letter the rest of the message uses.
    if !attrs.combo_legs.is_empty() {
        fields.push((6079, format_uint(attrs.combo_legs.len() as u64).to_string()));
        for leg in &attrs.combo_legs {
            fields.push((6080, leg.con_id.to_string()));
            fields.push((6081, format_uint(leg.ratio as u64).to_string()));
            // 1 buys the leg and 0 sells it, which is the opposite way round
            // from every other side field on the message. Sent the other way,
            // a long call spread priced at a debit came back "Guaranteed-to-Lose
            // combination orders are not allowed" — the venue had been given
            // the short spread — and reversing it previewed the long spread at
            // the margin a long spread carries.
            fields.push((6082, if leg.is_sell { "0" } else { "1" }.to_string()));
            // Empty where the leg routes with the combination rather than on a
            // venue of its own, which is what the terminal writes for SMART.
            fields.push((616, leg.exchange.clone()));
            if leg.open_close != 0 {
                fields.push((654, leg.open_close.to_string()));
            }
            if leg.short_sale_slot != 0 {
                fields.push((6086, leg.short_sale_slot.to_string()));
                if !leg.designated_location.is_empty() {
                    fields.push((6216, leg.designated_location.clone()));
                }
            }
            if leg.exempt_code != -1 {
                fields.push((1689, leg.exempt_code.to_string()));
            }
        }
        // Where the caller priced the legs separately rather than pricing the
        // combination, each leg's price follows the legs, one per leg and in
        // leg order — which is the only thing that says which leg a price
        // belongs to, since nothing on the wire names it.
        //
        // A caller who priced the legs and had the prices dropped got the
        // combination worked at whatever the venue made of it, which is not
        // the order that was placed.
        if attrs.combo_legs.iter().any(|leg| leg.price.is_some()) {
            for leg in &attrs.combo_legs {
                // A leg the caller left unpriced states nothing, the way a leg
                // with no venue of its own does.
                fields.push((
                    6879,
                    leg.price.map(|p| format_price(p).to_string()).unwrap_or_default(),
                ));
            }
        }
    }

    // Where the order clears, which is not the account it trades in. This
    // already read both of these back off the wire and sent neither.
    if !attrs.clearing_account.is_empty() {
        fields.push((440, attrs.clearing_account.clone()));
    }
    if !attrs.clearing_intent.is_empty() {
        fields.push((6419, attrs.clearing_intent.clone()));
    }

    // Lifecycle: whether the venue holds this order rather than working it,
    // whether it may work overnight, when it cancels itself, and what it takes
    // with it when it goes.
    if attrs.deactivate {
        fields.push((6521, "1".to_string()));
    }
    if attrs.deactivate_on_disconnect {
        fields.push((6661, "1".to_string()));
    }
    if attrs.include_overnight {
        fields.push((8534, "1".to_string()));
    }
    if attrs.auto_cancel_parent {
        fields.push((6965, "1".to_string()));
    }
    if attrs.min_trade_qty > 0 {
        fields.push((8415, format_uint(attrs.min_trade_qty as u64).to_string()));
    }
    if attrs.block_order {
        fields.push((9801, "1".to_string()));
    }
    if !attrs.auto_cancel_date.is_empty() {
        fields.push((6596, attrs.auto_cancel_date.clone()));
    }

    // Who the order is for, which the venue reads as a regulatory statement.
    if !attrs.rule80a.is_empty() {
        fields.push((47, attrs.rule80a.clone()));
    }
    if attrs.post_to_ats != 0 {
        fields.push((8405, format_uint(attrs.post_to_ats as u64).to_string()));
    }

    // Short-sale handling. The location is stated only for the slot that has
    // one, which is the rule the venue applies, and the exemption rides its own
    // tag rather than the slot.
    // A short sale states where the stock comes from, and states it whatever the
    // slot is. Both fields were written only for a slot the caller had named, so
    // a plain short sale — the ordinary case, no slot stated — went out as a
    // short with no allocation on it at all, which is not a short sale this
    // venue will work. The location rides its own tag, and only for the slot
    // that has one.
    if matches!(side, Side::ShortSell) {
        let located = matches!(attrs.designated_location.as_str(), "TMBR" | "IBKR");
        fields.push((114, if located { "Y" } else { "N" }.to_string()));
        if attrs.short_sale_slot == 2 && !attrs.designated_location.is_empty() {
            fields.push((5700, attrs.designated_location.clone()));
        }
        fields.push((6086, attrs.short_sale_slot.to_string()));
    }
    if attrs.exempt_code != -1 {
        fields.push((1688, attrs.exempt_code.to_string()));
    }
    // The hedge, as a number rather than the API's letter, with the parameter
    // the chosen kind takes: a beta or a pair ratio. Delta and FX take none.
    if attrs.hedge_type != 0 {
        fields.push((6665, attrs.hedge_type.to_string()));
        if attrs.hedge_beta != 0.0 {
            fields.push((6703, format!("{:.6}", attrs.hedge_beta)));
        }
        if attrs.hedge_ratio != 0.0 {
            fields.push((6666, format!("{:.6}", attrs.hedge_ratio)));
        }
    }
    // Where the contract is listed, which is not where the order routes. The
    // venue reads the two separately and this stated only the routing.
    if !attrs.primary_exchange.is_empty() {
        fields.push((207, attrs.primary_exchange.clone()));
    }
    // The contract the order hedges against: which one, its delta and its
    // price. Stated on the contract rather than the order, so an order that
    // named a hedging leg still said nothing about what to hedge with.
    // Which contract to hedge with is stated on the contract and again on the
    // order, and a caller written against the reference client sets both. Sent
    // twice the gateway reads it as a second statement of the same field, so
    // it is stated once. The contract's own answer is preferred: it is where
    // the hedging leg is named, and the order restates it.
    let hedge_con_id = attrs.delta_neutral_contract.as_deref()
        .map(|dnc| dnc.con_id)
        .filter(|id| *id != 0)
        .or_else(|| attrs.delta_neutral.as_deref().map(|dn| dn.con_id).filter(|id| *id != 0));
    if let Some(con_id) = hedge_con_id {
        fields.push((6150, con_id.to_string()));
    }
    if let Some(dnc) = attrs.delta_neutral_contract.as_deref() {
        fields.push((6148, format!("{:.6}", dnc.delta)));
        fields.push((6149, format!("{:.6}", dnc.price)));
    }
    if let Some(dn) = attrs.delta_neutral.as_deref() {
        fields.push((6290, dn.order_type.clone()));
        if dn.aux_price != 0 {
            fields.push((6291, format_price(dn.aux_price).to_string()));
        }
    }
    if let Some(scale) = attrs.scale.as_deref() {
        if scale.init_level_size > 0 {
            fields.push((6403, format_uint(scale.init_level_size as u64).to_string()));
        }
        if scale.subs_level_size > 0 {
            fields.push((6445, format_uint(scale.subs_level_size as u64).to_string()));
        }
        if scale.price_increment > 0 {
            fields.push((6405, format_price(scale.price_increment).to_string()));
        }
        if scale.profit_offset > 0 {
            fields.push((6446, format_price(scale.profit_offset).to_string()));
        }
        if scale.price_adjust_value != 0 {
            fields.push((6527, format_price(scale.price_adjust_value).to_string()));
        }
        if scale.price_adjust_interval > 0 {
            fields.push((6526, format_uint(scale.price_adjust_interval as u64).to_string()));
        }
        if scale.auto_reset {
            fields.push((6461, "1".to_string()));
        }
        if scale.random_percent {
            fields.push((6795, "1".to_string()));
        }
    }
    if attrs.trigger_method > 0 {
        fields.push((6115, attrs.trigger_method.to_string()));
    }
    if attrs.cash_qty > 0 {
        // 152, not 5920. The vendor's attribute declares `super(152, …, 5920, …)`
        // — the same shape as all-or-none's `super(18, …, 3570, …)`, where 18 is
        // the tag this already sends and 3570 is a selector for its own screens.
        // 5920 is that selector, and it is written by no encoder anywhere; 152
        // is CashOrderQty. An order by cash amount was naming a field the venue
        // does not read.
        fields.push((152, format_price(attrs.cash_qty).to_string()));
    }
    // Condition tags. The vendor's audit renderer names the whole set:
    // 6123 conid, 6124 exchange, 6125 price, 6126 operator, 6128 cancel-on-condition,
    // 6136 list size, 6137 conjunction, 6166 strike, 6168 expiry, 6169
    // security type, 6220 multiplier, 6222 type, 6223 time, 6224 send-email,
    // 6226 email text, 6227 TWS actions, 6241 inactive, 6245 percentage,
    // 6246 execution pattern, 6263 volume, 6151 ignore-RTH, 8569 amount,
    // 6947 a type discriminator (NOT a timezone).
    //
    // A time condition is still refused with every one of these read and the
    // relevant ones sent, including the timezone, so what it wants is not in
    // this list.
    if !attrs.conditions.is_empty() {
        let cond_strs = build_condition_strings(&attrs.conditions);
        fields.push((6136, cond_strs[0].clone())); // first element is count
        // 6128 cancels the order when its condition fails; 6151 lets the
        // conditions ignore regular hours. The audit renderer names 6128
        // "CondIgnoreRth" and 6151 "StockRefPrice" — both names belong to
        // other messages. The order serializer writes these two, for these two
        // flags, in this order. Swapping them to match the renderer was tried
        // and was wrong.
        if attrs.conditions_cancel_order {
            fields.push((6128, "1".to_string()));
        }
        if attrs.conditions_ignore_rth {
            fields.push((6151, "1".to_string()));
        }
        // Per-condition tags start at index 1, 11 strings per condition
        for i in 0..attrs.conditions.len() {
            let base = 1 + i * 11;
            fields.push((6222, cond_strs[base].clone())); // condType
            // Every slot, including the ones this condition has no use for.
            // The terminal writes only what applies, and following it here was
            // tried: it did not make the time condition acceptable, and it
            // broke the multi-condition order, which is refused for a missing
            // volume field the moment the price condition alongside it stops
            // stating an empty one. The gateway is reading these positionally.
            fields.push((6137, cond_strs[base + 1].clone())); // conjunction
            fields.push((6126, cond_strs[base + 2].clone())); // operator
            fields.push((6123, cond_strs[base + 3].clone())); // conId
            fields.push((6124, cond_strs[base + 4].clone())); // exchange
            fields.push((6127, cond_strs[base + 5].clone())); // triggerMethod
            fields.push((6125, cond_strs[base + 6].clone())); // price
            fields.push((6223, cond_strs[base + 7].clone())); // time
            fields.push((6245, cond_strs[base + 8].clone())); // percent
            fields.push((6263, cond_strs[base + 9].clone())); // volume
            fields.push((6246, cond_strs[base + 10].clone())); // execution

            // A time condition is still refused, and these were tried against a
            // live session to see whether the shape was the reason: writing the
            // condition's own fields first and the empty ones after, as the
            // terminal does, and adding the empty 6947 it pads with. Neither
            // changed the answer, and both are churn on a path that price,
            // volume and multi-condition orders already go through, so the
            // order here stays fixed
        }
    }

    // Adjustable-stop tags last, keeping the position they held in the encoder
    // this path replaced: after 204 and the attribute block, not in among the
    // order-type tags. Values and conditions are unchanged; only the encoder
    // they come from is new (ibx#240).
    if let K::AdjustableStop {
        trigger_price,
        adjusted_order_type,
        adjusted_stop_price,
        adjusted_stop_limit_price,
        adjusted_trailing_amount,
        adjustable_trailing_unit,
        ..
    } = &kind
    {
        fields.push((6257, "1".to_string())); // has adjustable params
        fields.push((6261, adjusted_order_type.fix_code().to_string()));
        fields.push((6258, format_price(*trigger_price).to_string()));
        fields.push((6259, format_price(*adjusted_stop_price).to_string()));
        if *adjusted_stop_limit_price > 0 {
            fields.push((6262, format_price(*adjusted_stop_limit_price).to_string()));
        }
        // Trailing amount + unit for a Trail/TrailLimit conversion
        // (ib-agent#167, ibx#225).
        if matches!(
            adjusted_order_type,
            crate::types::AdjustedOrderType::Trail | crate::types::AdjustedOrderType::TrailLimit
        ) {
            fields.push((6260, format_price(*adjusted_trailing_amount).to_string()));
            fields.push((6269, adjustable_trailing_unit.to_string()));
        }
    }

    // The optional tags each type appends last, in the position the per-type
    // encoders give them: after 204 and the attribute block, not in among the
    // order-type tags. The values and the conditions are unchanged.
    match &kind {
        K::MidPrice { price_cap } if *price_cap > 0 => {
            fields.push((44, format_price(*price_cap).to_string()));
        }
        // The offset is stated whether or not it is zero. Pegging at the price
        // with no offset is an ordinary order, and omitting the tag for it had
        // the gateway refuse the whole thing — seen against a paper account as
        // "Invalid value in field # 44", which is what an absent offset leaves
        // it looking for.
        K::PegMkt { offset, price_cap } => {
            fields.push((211, format_price(*offset).to_string()));
            if *price_cap > 0 {
                fields.push((44, format_price(*price_cap).to_string()));
            }
        }
        K::PegMid { offset, price_cap } => {
            // A midpoint peg states its offset one of two ways: as one
            // continuous number on tag 211, or as a whole-tick part and a
            // half-tick part together. The counterpart sends a different order
            // type for the second form, chosen when both parts are set, and
            // states no peg instruction beside it. Zero is how the first form
            // says it is not the second.
            //
            // Both parts were sent as zero whatever the caller asked for, so a
            // caller stating the two-part offset had it replaced with nothing
            // and got the continuous form with no offset in it.
            let whole = if attrs.mid_offset_at_whole == f64::MAX { 0.0 } else { attrs.mid_offset_at_whole };
            let half = if attrs.mid_offset_at_half == f64::MAX { 0.0 } else { attrs.mid_offset_at_half };
            fields.push((8403, format!("{whole:.6}")));
            fields.push((8404, format!("{half:.6}")));
            if whole != 0.0 && half != 0.0 {
                // The two-part form. The order type stated above is the one for
                // a continuous offset, so it is restated, and the instruction
                // that names the peg is dropped — the type carries it.
                for (tag, value) in fields.iter_mut() {
                    if *tag == 40 {
                        *value = "PMID2".to_string();
                    } else if *tag == 18 {
                        value.retain(|c| c != 'M');
                    }
                }
                fields.retain(|(tag, value)| *tag != 18 || !value.is_empty());
            }
            fields.push((211, format_price(*offset).to_string()));
            // The worst price the peg may reach, which IBKR documents as the
            // limit-price field for these types. A zero cap is no cap, and zero
            // is not a price, so it is left off rather than stated as one.
            if *price_cap > 0 {
                fields.push((44, format_price(*price_cap).to_string()));
            }
        }
        // Optional initial stop trigger (ib-agent#173).
        K::TrailingStop { trail_stop_price, .. }
        | K::TrailingStopLimit { trail_stop_price, .. }
        | K::TrailPct { trail_stop_price, .. }
            if *trail_stop_price > 0 =>
        {
            fields.push((6117, format_price(*trail_stop_price).to_string()));
        }
        _ => {}
    }

    // Strategy and preview tags last, in the position they held in the encoders
    // this path replaced: after 204 and the attribute block (ibx#318).
    match &kind {
        K::Adaptive { priority, .. } => {
            fields.push((847, "Adaptive".to_string()));
            fields.push((5957, "1".to_string()));
            fields.push((5958, "adaptivePriority".to_string()));
            fields.push((5960, priority.as_str().to_string()));
        }
        K::Algo { algo, .. } => {
            let (algo_name, param_strs) = build_algo_tags(algo);
            fields.push((847, algo_name.to_string()));
            // Tag 849 (maxPctVol) for the algos that use it.
            if let AlgoParams::Vwap { max_pct_vol, .. }
            | AlgoParams::ArrivalPx { max_pct_vol, .. }
            | AlgoParams::ClosePx { max_pct_vol, .. } = algo
            {
                fields.push((849, format!("{max_pct_vol}")));
            }
            fields.push((5957, (param_strs.len() / 2).to_string()));
            // Key/value pairs: 5958=key, 5960=value, repeated.
            for pair in param_strs.chunks_exact(2) {
                fields.push((5958, pair[0].clone()));
                fields.push((5960, pair[1].clone()));
            }
        }
        K::WhatIf { .. } => fields.push((6091, "1".to_string())),
        _ => {}
    }
}

fn build_algo_tags(algo: &AlgoParams) -> (&'static str, Vec<String>) {
    match algo {
        AlgoParams::Vwap { no_take_liq, allow_past_end_time, start_time, end_time, .. } => (
            "Vwap",
            vec![
                "noTakeLiq".into(),
                if *no_take_liq { "1" } else { "0" }.into(),
                "allowPastEndTime".into(),
                if *allow_past_end_time { "1" } else { "0" }.into(),
                "startTime".into(),
                start_time.clone(),
                "endTime".into(),
                end_time.clone(),
            ],
        ),
        AlgoParams::Twap { allow_past_end_time, start_time, end_time } => (
            "Twap",
            vec![
                "allowPastEndTime".into(),
                if *allow_past_end_time { "1" } else { "0" }.into(),
                "startTime".into(),
                start_time.clone(),
                "endTime".into(),
                end_time.clone(),
            ],
        ),
        AlgoParams::ArrivalPx {
            risk_aversion,
            allow_past_end_time,
            force_completion,
            start_time,
            end_time,
            ..
        } => (
            "ArrivalPx",
            vec![
                "riskAversion".into(),
                risk_aversion.as_str().into(),
                "allowPastEndTime".into(),
                if *allow_past_end_time { "1" } else { "0" }.into(),
                "forceCompletion".into(),
                if *force_completion { "1" } else { "0" }.into(),
                "startTime".into(),
                start_time.clone(),
                "endTime".into(),
                end_time.clone(),
            ],
        ),
        AlgoParams::ClosePx { risk_aversion, force_completion, start_time, .. } => (
            "ClosePx",
            vec![
                "riskAversion".into(),
                risk_aversion.as_str().into(),
                "forceCompletion".into(),
                if *force_completion { "1" } else { "0" }.into(),
                "startTime".into(),
                start_time.clone(),
            ],
        ),
        AlgoParams::DarkIce { allow_past_end_time, display_size, start_time, end_time } => (
            "DarkIce",
            vec![
                "allowPastEndTime".into(),
                if *allow_past_end_time { "1" } else { "0" }.into(),
                "displaySize".into(),
                display_size.to_string(),
                "startTime".into(),
                start_time.clone(),
                "endTime".into(),
                end_time.clone(),
            ],
        ),
        AlgoParams::PctVol { pct_vol, no_take_liq, start_time, end_time } => (
            "PctVol",
            vec![
                "noTakeLiq".into(),
                if *no_take_liq { "1" } else { "0" }.into(),
                "pctVol".into(),
                format!("{}", pct_vol),
                "startTime".into(),
                start_time.clone(),
                "endTime".into(),
                end_time.clone(),
            ],
        ),
    }
}

fn build_condition_strings(conditions: &[OrderCondition]) -> Vec<String> {
    let mut out = Vec::with_capacity(1 + conditions.len() * 11);
    out.push(conditions.len().to_string());
    for (i, cond) in conditions.iter().enumerate() {
        let is_last = i == conditions.len() - 1;
        let conj = if is_last { "n" } else { "a" };
        let op = |is_more: bool| if is_more { ">=" } else { "<=" };
        match cond {
            OrderCondition::Price { con_id, exchange, price, is_more, trigger_method } => {
                out.push("1".into()); // condType
                out.push(conj.into()); // conjunction
                out.push(op(*is_more).into()); // operator
                out.push(con_id.to_string()); // conId
                out.push(exchange.clone()); // exchange
                out.push(trigger_method.to_string()); // triggerMethod
                out.push(format_price(*price).to_string()); // price
                out.push(String::new()); // time (unused)
                out.push(String::new()); // percent (unused)
                out.push(String::new()); // volume (unused)
                out.push(String::new()); // execution (unused)
            }
            // The venue refuses a time condition, and it is not this
            // encoding. Every field was checked against the terminal's own
            // encoder and agrees with it: the type is 3, the operators are
            // `>=` and `<=`, the conjunctions are `a`/`o`/`n`, the value is
            // `YYYYMMDD-HH:MM:SS` in GMT, and a time condition carries that
            // one field and no other — not the contract, exchange, trigger
            // method or price a price condition carries, and not the timezone
            // on tag 6947, which the terminal writes only for the condition
            // types that answer yes to carrying one, and this is not among
            // them. Sending the timezone anyway changes nothing, and neither
            // does any other shape of the value: eight were tried, including
            // both separators, with and without a zone, without seconds, date
            // alone, milliseconds, and epoch in seconds and milliseconds.
            //
            // So the refusal is about something other than the condition, and
            // price, volume and multi-condition orders are all accepted as
            // they stand. Left as the terminal writes it.
            OrderCondition::Time { time, is_more } => {
                out.push("3".into());
                out.push(conj.into());
                out.push(op(*is_more).into());
                out.push(String::new()); // conId (unused)
                out.push(String::new()); // exchange (unused)
                out.push(String::new()); // triggerMethod (unused)
                out.push(String::new()); // price (unused)
                out.push(time.clone()); // time
                out.push(String::new()); // percent (unused)
                out.push(String::new()); // volume (unused)
                out.push(String::new()); // execution (unused)
            }
            OrderCondition::Margin { percent, is_more } => {
                out.push("4".into());
                out.push(conj.into());
                out.push(op(*is_more).into());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(percent.to_string()); // percent
                out.push(String::new());
                out.push(String::new());
            }
            OrderCondition::Execution { symbol, exchange, sec_type } => {
                out.push("5".into());
                out.push(conj.into());
                out.push(String::new()); // operator (unused)
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                let exch = if exchange == "SMART" { "*" } else { exchange.as_str() };
                out.push(format!("symbol={symbol};exchange={exch};securityType={sec_type};"));
            }
            OrderCondition::Volume { con_id, exchange, volume, is_more } => {
                out.push("6".into());
                out.push(conj.into());
                out.push(op(*is_more).into());
                out.push(con_id.to_string());
                out.push(exchange.clone());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(volume.to_string()); // volume
                out.push(String::new());
            }
            OrderCondition::PercentChange { con_id, exchange, percent, is_more } => {
                out.push("7".into());
                out.push(conj.into());
                out.push(op(*is_more).into());
                out.push(con_id.to_string());
                out.push(exchange.clone());
                out.push(String::new());
                out.push(String::new());
                out.push(String::new());
                out.push(format!("{percent}")); // percent
                out.push(String::new());
                out.push(String::new());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {

    /// A stock is named by its symbol. Everything else names one contract by
    /// the venue's own identifier, which is what tells a member of a family
    /// from the family itself.
    #[test]
    fn only_a_stock_leaves_the_contract_unnamed() {
        for (sec_type, key, wants_id) in [
            // expiry|strike|right|multiplier|tradingClass|localSymbol
            ("STK", "|0|||XCLASS|XLOCAL", false),
            ("IND", "|0|||XCLASS|XLOCAL", true),
            ("CFD", "|0|||XCLASS|XLOCAL", true),
            ("CRYPTO", "|0|||XCLASS|XLOCAL", true),
        ] {
            let mut context = Context::new();
            let instrument = context
                .market
                .try_register_contract(1, "X", sec_type, "SMART", key)
                .expect("register a contract");
            context.set_symbol(instrument, "X".to_string());
            let mut fields: Vec<(u32, String)> = Vec::new();
            push_contract_identity(&mut fields, &context, instrument);
            let named = fields.iter().any(|(t, _)| *t == 48);
            assert_eq!(named, wants_id, "{sec_type} names the contract: {fields:?}");
            assert!(
                !(wants_id && fields.iter().any(|(t, _)| *t == 6058)),
                "{sec_type} states no trading class: {fields:?}",
            );
        }
    }

    /// A replace is a full statement of the order, so an attribute the submit
    /// made survives it. This one came back without its all-or-none instruction
    /// and was a different order to the one the caller had placed.
    #[test]
    fn a_replace_restates_the_attributes_the_order_was_placed_with() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());

        let attrs = crate::types::OrderAttrs { all_or_none: true, ..Default::default() };
        context.pending_orders.push(crate::types::OrderRequest::SubmitEx {
            order_id: 42,
            instrument,
            side: Side::Buy,
            qty: 100,
            kind: crate::types::OrderKind::Limit { price: 150 * crate::types::PRICE_SCALE },
            tif: b'0',
            attrs,
        });
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );
        let mut buf = [0u8; 8192];
        let n = peer.read(&mut buf).unwrap();
        let placed = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(placed.contains("\u{1}18=G\u{1}"), "the order was placed all-or-none: {placed}");

        context.pending_orders.push(crate::types::OrderRequest::Modify {
            new_order_id: 43,
            order_id: 42,
            price: 151 * crate::types::PRICE_SCALE,
            qty: 100,
            outside_rth: false,
            ord_type: 0,
            tif: 0,
            stop_price: 0,
        });
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]).to_string();
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(tag("35=").as_deref(), Some("G"), "a replace was sent: {msg}");
        assert_eq!(tag("6035=").as_deref(), Some("SPY"), "it names the contract: {msg}");
        assert_eq!(tag("18=").as_deref(), Some("G"), "it is still all-or-none: {msg}");
        assert_eq!(msg.matches("\u{1}38=").count(), 1, "the quantity is stated once: {msg}");
    }

    /// The five fields a cancel always names, and the one it never does. Two
    /// cancels of the same order must also name themselves differently, or the
    /// retry is a duplicate the server is free to drop.
    #[test]
    fn a_cancel_names_the_side_account_and_originator_but_no_transact_time() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        context.insert_order(crate::types::Order::new(
            42,
            instrument,
            Side::Sell,
            100,
            150 * crate::types::PRICE_SCALE,
            b'2',
            b'0',
            0,
        ));
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());

        let mut names = Vec::new();
        for _ in 0..2 {
            context.pending_orders.push(crate::types::OrderRequest::Cancel { order_id: 42 });
            drain_and_send_orders(
                &mut conn,
                &mut context,
                "DU1",
                &mut hb,
                false,
                &shared,
                false,
                &None,
            );
            let mut buf = [0u8; 4096];
            let n = peer.read(&mut buf).unwrap();
            let msg = String::from_utf8_lossy(&buf[..n]).to_string();
            let tag =
                |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

            assert_eq!(tag("35=").as_deref(), Some("F"), "a cancel was sent: {msg}");
            assert_eq!(tag("41=").as_deref(), Some("42.0"), "the order it cancels: {msg}");
            assert_eq!(tag("54=").as_deref(), Some("2"), "the side it carries: {msg}");
            assert_eq!(tag("1=").as_deref(), Some("DU1"), "the account: {msg}");
            assert_eq!(tag("6088=").as_deref(), Some("Socket"), "the originator: {msg}");
            assert_eq!(tag("38=").as_deref(), Some("100"), "what it cancels: {msg}");
            assert_eq!(tag("6008=").as_deref(), Some("756733"), "the contract it cancels: {msg}");
            assert_eq!(tag("60="), None, "no transact time is written in the order path: {msg}");
            names.push(tag("11=").expect("a cancel names itself"));
        }
        assert_ne!(names[0], names[1], "a retried cancel needs its own name: {names:?}");
    }

    /// ibx#349, ibx#372: the replace restated the tracked order's type,
    /// time-in-force and trigger, so a caller changing any of them had the
    /// change accepted, acknowledged and dropped. Asserted on the bytes,
    /// because the request-level tests passed throughout.
    #[test]
    fn a_modify_states_the_type_tif_and_trigger_it_carries() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        // Resting: a DAY limit with no trigger.
        context.insert_order(crate::types::Order::new(
            42,
            instrument,
            Side::Buy,
            100,
            150 * crate::types::PRICE_SCALE,
            b'2',
            b'0',
            0,
        ));

        // Modified to a GTC stop at 149.
        context.modify_ex(
            42,
            150 * crate::types::PRICE_SCALE,
            100,
            false,
            b'3',
            b'1',
            149 * crate::types::PRICE_SCALE,
        );
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

        assert_eq!(tag("35=").as_deref(), Some("G"), "a replace was sent: {msg}");
        assert_eq!(tag("40=").as_deref(), Some("3"), "the type the caller stated: {msg}");
        assert_eq!(tag("59=").as_deref(), Some("1"), "the tif the caller stated: {msg}");
        assert_eq!(
            tag("99="),
            Some(format_price(149 * crate::types::PRICE_SCALE).to_string()),
            "the trigger the caller stated: {msg}"
        );
    }

    /// The trigger is a price and lands on the instrument's grid like any
    /// other. `Modify` carries no instrument, so the generic snapping cannot
    /// reach it and both fields are snapped against the tracked order instead.
    #[test]
    fn a_moved_trigger_is_snapped_to_the_tick_grid() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        context.market.set_min_tick(instrument, 0.05);
        context.insert_order(crate::types::Order::new(
            42,
            instrument,
            Side::Sell,
            100,
            150 * crate::types::PRICE_SCALE,
            b'3',
            b'0',
            149 * crate::types::PRICE_SCALE,
        ));

        // 149.03 is off a five-cent grid.
        let off_grid = 149 * crate::types::PRICE_SCALE + 3 * crate::types::PRICE_SCALE / 100;
        context.modify_ex(42, 150 * crate::types::PRICE_SCALE, 100, false, b'3', b'0', off_grid);
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(
            tag("99="),
            Some(
                format_price(149 * crate::types::PRICE_SCALE + 5 * crate::types::PRICE_SCALE / 100)
                    .to_string()
            ),
            "the trigger must be on the grid: {msg}",
        );
    }

    /// A cancel names a version of an order the recovery may be about to
    /// correct. Sent against a reconnect that has not finished accounting for
    /// what the broker holds, it states a version that may not exist there and
    /// is refused, leaving the order live.
    #[test]
    fn a_cancel_waits_for_the_recovery_to_say_what_the_broker_holds() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        context.insert_order(crate::types::Order::new(
            42,
            instrument,
            Side::Buy,
            100,
            150 * crate::types::PRICE_SCALE,
            b'2',
            b'1',
            0,
        ));
        // This is the order in doubt: a write for it failed, so what the broker
        // holds for it is exactly what the recovery is about to say.
        context.set_order_status_forced(42, crate::types::OrderStatus::Uncertain);
        context.pending_orders.push(crate::types::OrderRequest::Cancel { order_id: 42 });
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());

        drain_and_send_orders(&mut conn, &mut context, "DU1", &mut hb, false, &shared, true, &None);
        peer.set_read_timeout(Some(std::time::Duration::from_millis(50))).unwrap();
        let mut buf = [0u8; 512];
        assert!(
            std::io::Read::read(&mut peer, &mut buf).unwrap_or(0) == 0,
            "nothing goes out while the recovery is still settling",
        );

        // An order placed since the reconnect is in no doubt, so its own cancel
        // is not made to wait on a recovery that has nothing to do with it.
        context.insert_order(crate::types::Order::new(
            43,
            instrument,
            Side::Buy,
            1,
            150 * crate::types::PRICE_SCALE,
            b'2',
            b'1',
            0,
        ));
        context.pending_orders.push(crate::types::OrderRequest::Cancel { order_id: 43 });
        drain_and_send_orders(&mut conn, &mut context, "DU1", &mut hb, false, &shared, true, &None);
        let n = std::io::Read::read(&mut peer, &mut buf).unwrap_or(0);
        assert!(
            String::from_utf8_lossy(&buf[..n]).contains("35=F"),
            "the cancel for the order that is not in doubt goes now",
        );

        // Once it has settled, the held one goes too.
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );
        let n = std::io::Read::read(&mut peer, &mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("35=F"), "and then it is sent",);
    }

    /// A write that fails has not established that the broker has nothing —
    /// the transport says as much of TLS. Calling it a rejection invited a
    /// resubmission of an order that may be working.
    #[test]
    fn an_order_whose_write_failed_is_unknown_rather_than_rejected() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        // Closing this side's write half makes the send fail on the call
        // rather than after a buffer fills.
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        context.insert_order(crate::types::Order::new(
            42,
            instrument,
            Side::Buy,
            100,
            150 * crate::types::PRICE_SCALE,
            b'2',
            b'1',
            0,
        ));
        context.last_clord.insert(42, "42.7".to_string());
        context.pending_orders.push(crate::types::OrderRequest::Modify {
            new_order_id: 43,
            order_id: 42,
            price: 151 * crate::types::PRICE_SCALE,
            qty: 100,
            outside_rth: false,
            ord_type: 0,
            tif: 0,
            stop_price: 0,
        });

        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &Some(tx),
        );

        // Both deliveries, because the event channel is documented as a second
        // delivery of everything rather than a lesser one — and an order whose
        // state is no longer known is the last thing to deliver only once.
        let events: Vec<_> = rx.try_iter().collect();
        assert!(
            events.iter().any(|e| matches!(e, crate::bridge::Event::OrderUpdate(u)
                if u.order_id == 42 && u.status == crate::types::OrderStatus::Uncertain)),
            "a caller reading events is told too: {events:?}",
        );

        let updates = shared.orders.drain_order_updates();
        assert!(
            updates
                .iter()
                .any(|u| u.order_id == 42 && u.status == crate::types::OrderStatus::Uncertain),
            "the caller is told, and told it is unknown: {updates:?}",
        );
        assert!(
            !updates.iter().any(|u| u.status == crate::types::OrderStatus::Rejected),
            "not that the broker refused it: {updates:?}",
        );
        assert!(
            context.order(42).is_some(),
            "and it stays tracked, for the recovery to account for",
        );
        let kept = context.order(42).unwrap();
        assert_eq!(
            kept.tif, b'1',
            "holding what the broker was last known to hold, not what the \
             replace tried to make it: the attempt was never accepted",
        );
        assert_eq!(kept.price, 150 * crate::types::PRICE_SCALE, "nor its price");
        assert!(context.order(43).is_none(), "and the attempt itself is not tracked");
    }

    /// A replace names the order the broker knows. Replacing a replacement
    /// left the chain behind under the previous id, so the second one stated an
    /// OrigClOrdID that had never existed and the broker refused it.
    #[test]
    fn a_replacement_can_itself_be_replaced() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        context.insert_order(crate::types::Order::new(
            7,
            instrument,
            Side::Buy,
            1,
            crate::types::PRICE_SCALE,
            b'2',
            b'0',
            0,
        ));
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());
        let mut buf = [0u8; 4096];

        // 7 -> 8, then 8 -> 9, as a caller stepping an order up twice does.
        context.pending_orders.push(crate::types::OrderRequest::Modify {
            new_order_id: 8,
            order_id: 7,
            price: 2 * crate::types::PRICE_SCALE,
            qty: 1,
            outside_rth: false,
            ord_type: 0,
            tif: 0,
            stop_price: 0,
        });
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );
        let n = peer.read(&mut buf).unwrap();
        let first = String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|");
        assert!(first.contains("|41=7.0|"), "the first replace names the original: {first}");

        context.pending_orders.push(crate::types::OrderRequest::Modify {
            new_order_id: 9,
            order_id: 8,
            price: 3 * crate::types::PRICE_SCALE,
            qty: 1,
            outside_rth: false,
            ord_type: 0,
            tif: 0,
            stop_price: 0,
        });
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );
        let n = peer.read(&mut buf).unwrap();
        let second = String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|");
        assert!(
            second.contains("|41=7.1|"),
            "the second names what the broker last acknowledged, not an id it never saw: {second}",
        );
    }

    /// What a pegged order actually puts on the wire.
    #[test]
    fn a_pegged_order_states_its_offset_and_no_limit_price() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        context.pending_orders.push(crate::types::OrderRequest::SubmitEx {
            order_id: 1,
            instrument,
            side: Side::Buy,
            qty: 1,
            kind: crate::types::OrderKind::PegMkt { offset: 0, price_cap: 0 },
            tif: b'0',
            attrs: Default::default(),
        });
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|");
        println!("PEGMKT WIRE: {msg}");
        assert!(msg.contains("|211=0|"), "the offset is stated: {msg}");
    }

    /// A replace may now change the order type, and a stop that becomes a
    /// limit has no trigger to state. Carrying the resting one anyway put a
    /// tag 99 on a limit order.
    #[test]
    fn a_replace_that_drops_the_trigger_does_not_carry_it() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        // A resting stop with a trigger at 149.
        context.insert_order(crate::types::Order::new(
            42,
            instrument,
            Side::Sell,
            100,
            150 * crate::types::PRICE_SCALE,
            b'3',
            b'1',
            149 * crate::types::PRICE_SCALE,
        ));

        // Replaced as a plain limit at 151.
        context.modify_ex(42, 151 * crate::types::PRICE_SCALE, 100, false, b'2', 0, 0);
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

        assert_eq!(tag("40=").as_deref(), Some("2"), "the stated type: {msg}");
        assert_eq!(
            tag("44=").as_deref(),
            Some(&*format_price(151 * crate::types::PRICE_SCALE)),
            "the limit price: {msg}"
        );
        assert_eq!(tag("99="), None, "and no trigger from the order it replaced: {msg}");
    }

    /// A modify that states none of them leaves what the resting order holds in
    /// force, which is every caller that only moves a price or a quantity.
    #[test]
    fn a_modify_that_states_nothing_keeps_the_resting_values() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let mut conn = Some(crate::protocol::connection::Connection::new_raw(stream).unwrap());
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        context.insert_order(crate::types::Order::new(
            42,
            instrument,
            Side::Sell,
            100,
            150 * crate::types::PRICE_SCALE,
            b'3',
            b'1',
            149 * crate::types::PRICE_SCALE,
        ));

        context.modify(42, 151 * crate::types::PRICE_SCALE, 100, false);
        let mut hb = crate::engine::hot_loop::HeartbeatState::new();
        let shared = std::sync::Arc::new(SharedState::new());
        drain_and_send_orders(
            &mut conn,
            &mut context,
            "DU1",
            &mut hb,
            false,
            &shared,
            false,
            &None,
        );

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

        assert_eq!(tag("40=").as_deref(), Some("3"), "the resting type: {msg}");
        assert_eq!(tag("59=").as_deref(), Some("1"), "the resting tif: {msg}");
        // A stop has one price and it is the trigger, so the single price the
        // caller passed can only have meant that. Leaving 149 in place would
        // put 151 on no tag at all and move nothing.
        assert_eq!(
            tag("99="),
            Some(format_price(151 * crate::types::PRICE_SCALE).to_string()),
            "the moved trigger: {msg}"
        );
        assert!(!msg.contains("\u{1}44="), "a stop states no limit price: {msg}");
    }
    use super::*;
    use crate::types::Order;

    fn order(oid: u64, filled: u32, status: OrderStatus) -> Order {
        Order {
            order_id: oid,
            instrument: 0,
            side: Side::Buy,
            price: 100,
            qty: 10,
            filled,
            status,
            ord_type: b'2',
            tif: b'0',
            stop_price: 0,
        }
    }

    // ibx#211: an outbound cancel synthesizes the PendingCancel phase the
    // server never sends for a normal cancel.
    #[test]
    fn synthesize_pending_cancel_updates_and_notifies() {
        let mut context = Context::new();
        let shared = Arc::new(SharedState::new());
        context.insert_order(order(7, 3, OrderStatus::PartiallyFilled));

        synthesize_pending_cancel(&mut context, &shared, 7, &None);

        assert_eq!(context.order(7).unwrap().status, OrderStatus::PendingCancel);
        let updates = shared.orders.drain_order_updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].status, OrderStatus::PendingCancel);
        assert_eq!(updates[0].filled_qty, 3.0);
        assert_eq!(updates[0].remaining_qty, 7.0);
    }

    #[test]
    fn synthesize_pending_cancel_skips_terminal_and_unknown_orders() {
        let mut context = Context::new();
        let shared = Arc::new(SharedState::new());
        // Late cancel racing a fill: the order is done, no phase to report.
        context.insert_order(order(8, 10, OrderStatus::Filled));

        synthesize_pending_cancel(&mut context, &shared, 8, &None);
        synthesize_pending_cancel(&mut context, &shared, 999, &None);

        assert_eq!(context.order(8).unwrap().status, OrderStatus::Filled);
        assert!(shared.orders.drain_order_updates().is_empty());
    }

    /// ibx#318: adaptive, algo and what-if orders returned early into their own
    /// encoders, which carried no attribute block at all — so outside-RTH, the
    /// parent link and the OCA group were accepted by the API and silently
    /// dropped, and the tif was hard-coded to DAY. Asserted on the bytes,
    /// because the enum-level tests passed throughout.
    #[test]
    fn adaptive_wire_carries_the_attributes_and_keeps_its_algo_tags() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::Adaptive {
                price: 100 * crate::types::PRICE_SCALE,
                priority: crate::types::AdaptivePriority::Urgent,
            },
            b'1',
            bracket_child_attrs(),
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

        assert_eq!(tag("6433=").as_deref(), Some("1"), "outside RTH missing: {msg}");
        assert_eq!(tag("6107=").as_deref(), Some("42.0"), "parent link missing: {msg}");
        assert_eq!(tag("583=").as_deref(), Some("bracket_1"), "OCA group missing: {msg}");
        assert_eq!(tag("59=").as_deref(), Some("1"), "tif must be GTC, not DAY: {msg}");

        // And everything the standalone encoder emitted is unchanged.
        assert_eq!(tag("40=").as_deref(), Some("2"));
        assert_eq!(tag("18=").as_deref(), Some("e"), "adaptive wrapper missing: {msg}");
        assert_eq!(tag("847=").as_deref(), Some("Adaptive"));
        assert_eq!(tag("5957=").as_deref(), Some("1"));
        assert_eq!(tag("5958=").as_deref(), Some("adaptivePriority"));
        assert_eq!(tag("5960=").as_deref(), Some("Urgent"));
        assert!(
            msg.find("15=").unwrap() < msg.find("847=").unwrap(),
            "the strategy tags keep their position after the contract block: {msg}"
        );
    }

    #[test]
    fn algo_wire_carries_the_attributes_and_keeps_its_algo_tags() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::Algo {
                price: 100 * crate::types::PRICE_SCALE,
                algo: AlgoParams::Vwap {
                    max_pct_vol: 0.25,
                    no_take_liq: true,
                    allow_past_end_time: false,
                    start_time: String::new(),
                    end_time: String::new(),
                },
            },
            b'1',
            bracket_child_attrs(),
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

        assert_eq!(tag("6433=").as_deref(), Some("1"), "outside RTH missing: {msg}");
        assert_eq!(tag("6107=").as_deref(), Some("42.0"), "parent link missing: {msg}");
        assert_eq!(tag("583=").as_deref(), Some("bracket_1"), "OCA group missing: {msg}");
        assert_eq!(tag("59=").as_deref(), Some("1"), "tif must be GTC, not DAY: {msg}");

        assert_eq!(tag("847=").as_deref(), Some("Vwap"));
        assert_eq!(tag("849=").as_deref(), Some("0.25"), "maxPctVol missing: {msg}");
        assert_eq!(tag("5957=").as_deref(), Some("4"), "param count: {msg}");
        assert_eq!(tag("5958=").as_deref(), Some("noTakeLiq"));
        assert_eq!(tag("5960=").as_deref(), Some("1"));
    }

    #[test]
    fn what_if_wire_carries_the_attributes_and_keeps_its_preview_flag() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::WhatIf { price: 100 * crate::types::PRICE_SCALE, ord_type: b'2' },
            b'1',
            bracket_child_attrs(),
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

        assert_eq!(tag("6433=").as_deref(), Some("1"), "outside RTH missing: {msg}");
        assert_eq!(tag("6107=").as_deref(), Some("42.0"), "parent link missing: {msg}");
        assert_eq!(tag("59=").as_deref(), Some("1"), "tif must be GTC, not DAY: {msg}");
        assert_eq!(tag("6091=").as_deref(), Some("1"), "what-if flag missing: {msg}");
        assert!(
            msg.find("15=").unwrap() < msg.find("6091=").unwrap(),
            "the preview flag keeps its position after the contract block: {msg}"
        );
        assert_eq!(tag("40=").as_deref(), Some("2"), "a limit preview: {msg}");
    }

    /// A caller written against the reference client names the hedging
    /// contract on the contract and again on the order. Sent twice, the
    /// gateway reads the second as a correction of the first.
    #[test]
    fn the_hedging_contract_is_named_once() {
        let attrs = crate::types::OrderAttrs {
            delta_neutral_contract: Some(Box::new(crate::types::DeltaNeutralContractSpec {
                con_id: 265598,
                delta: 0.5,
                price: 100.0,
            })),
            delta_neutral: Some(Box::new(crate::types::DeltaNeutralAttrs {
                order_type: "MKT".into(),
                aux_price: 0,
                con_id: 265598,
            })),
            ..crate::types::OrderAttrs::default()
        };
        let msg = send_kind_for_test(
            crate::types::OrderKind::Limit { price: 100 * crate::types::PRICE_SCALE },
            b'1',
            attrs,
        );
        let stated = msg.split('\u{1}').filter(|f| f.starts_with("6150=")).count();
        assert_eq!(stated, 1, "the hedging contract is named once: {msg}");
        assert!(msg.contains("6150=265598"), "{msg}");
    }

    #[test]
    fn a_market_preview_states_market_and_no_price() {
        // Previewing every order as a limit is refused outright by a security
        // that only trades at market.
        let msg = send_kind_for_test(
            crate::types::OrderKind::WhatIf { price: 0, ord_type: b'1' },
            b'1',
            bracket_child_attrs(),
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(tag("40=").as_deref(), Some("1"), "a market preview: {msg}");
        assert_eq!(tag("44=").as_deref(), None, "a market order states no price: {msg}");
        assert_eq!(tag("6091=").as_deref(), Some("1"), "still a preview: {msg}");
    }

    fn bracket_child_attrs() -> crate::types::OrderAttrs {
        crate::types::OrderAttrs {
            parent_id: 42,
            oca_group_str: "bracket_1".to_string(),
            oca_type: 1,
            outside_rth: true,
            ..Default::default()
        }
    }

    /// Encode one kind and return the frame as text.
    fn send_kind_for_test(
        kind: crate::types::OrderKind,
        tif: u8,
        attrs: crate::types::OrderAttrs,
    ) -> String {
        use std::io::Read;
        let (mut conn, mut peer) = crate::protocol::connection::Connection::for_test();
        let mut context = Context::new();
        send_order_ex(&mut conn, &mut context, "DU123456", 7, 0, Side::Buy, 1, kind, tif, &attrs)
            .unwrap();
        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        String::from_utf8_lossy(&buf[..n]).to_string()
    }

    /// A short sale states that side, distinctly from a plain sale.
    ///
    /// The venue refuses it — "sell short variant is not supported" — so no
    /// live phase can show the side is written correctly, and a caller shorting
    /// through this client depends on it being right the day a venue takes it.
    #[test]
    fn a_short_sale_states_its_own_side() {
        use std::io::Read;
        let (mut conn, mut peer) = crate::protocol::connection::Connection::for_test();
        let mut context = Context::new();
        send_order_ex(
            &mut conn, &mut context, "DU123456", 7, 0, Side::ShortSell, 1,
            crate::types::OrderKind::Limit { price: 100 * crate::types::PRICE_SCALE },
            b'1', &crate::types::OrderAttrs::default(),
        ).unwrap();
        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]).to_string();
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(tag("54=").as_deref(), Some("5"), "a short sale, not a sale: {msg}");
    }

    /// What a volatility order does as the underlying moves.
    ///
    /// A caller could state that the venue should keep re-pricing the order,
    /// which price to reference, and the band of underlying prices to stay
    /// inside — and the API accepted all four and sent none of them. An order
    /// asking to be managed arrived asking for nothing of the sort.
    #[test]
    fn a_volatility_order_carries_what_it_asked_to_be_managed_by() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::Limit { price: 100 * crate::types::PRICE_SCALE },
            b'0',
            crate::types::OrderAttrs {
                volatility: 0.25,
                volatility_type: 2,
                continuous_update: true,
                reference_price_type: 2,
                stock_range_lower: 100.0,
                stock_range_upper: 200.0,
                ..Default::default()
            },
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(tag("6280=").as_deref(), Some("2"), "the volatility kind: {msg}");
        assert_eq!(tag("6275=").as_deref(), Some("1"), "kept re-priced: {msg}");
        assert_eq!(tag("6279=").as_deref(), Some("2"), "the price it references: {msg}");
        assert!(
            tag("6152=").is_some_and(|v| v.starts_with("100.")),
            "the band it stays above: {msg}",
        );
        assert!(
            tag("6153=").is_some_and(|v| v.starts_with("200.")),
            "the band it stays below: {msg}",
        );
    }

    /// An order that asked the venue to manage its price, to run for a set
    /// time, and what to compete against.
    ///
    /// All four are on the order this API takes and on the Python one, where a
    /// caller coming from the reference client puts them, and none of them
    /// reached the wire.
    #[test]
    fn an_order_carries_what_it_competes_against_and_how_long_it_runs() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::Limit { price: 100 * crate::types::PRICE_SCALE },
            b'0',
            crate::types::OrderAttrs {
                use_price_mgmt_algo: 1,
                duration: 60,
                min_compete_size: 100,
                compete_against_best_offset: 0.02,
                ..Default::default()
            },
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(tag("8339=").as_deref(), Some("1"), "price managed by the venue: {msg}");
        assert_eq!(tag("8402=").as_deref(), Some("60"), "how long it runs: {msg}");
        assert_eq!(tag("8411=").as_deref(), Some("100"), "the smallest size worth competing for: {msg}");
        assert!(
            tag("8412=").is_some_and(|v| v.starts_with("0.02")),
            "how far past the best price: {msg}",
        );
    }

    /// A default order states none of them, so an order that asked for nothing
    /// does not arrive asking for something.
    #[test]
    fn a_default_order_competes_for_nothing() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::Limit { price: 100 * crate::types::PRICE_SCALE },
            b'0',
            crate::types::OrderAttrs::default(),
        );
        for t in ["8339=", "8402=", "8411=", "8412="] {
            assert!(!msg.contains(t), "{t} stated on an order that asked for nothing: {msg}");
        }
    }

    /// A midpoint peg whose offset is stated as two parts is the other form of
    /// the order, and says so by its type rather than by an instruction.
    #[test]
    fn a_two_part_midpoint_offset_is_the_other_peg() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::PegMid { offset: crate::types::PRICE_SCALE / 100, price_cap: 0 },
            b'0',
            crate::types::OrderAttrs {
                mid_offset_at_whole: 0.01,
                mid_offset_at_half: 0.005,
                ..Default::default()
            },
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(tag("40=").as_deref(), Some("PMID2"), "the two-part peg: {msg}");
        assert!(tag("18=").is_none(), "the type carries it, not an instruction: {msg}");
        assert!(tag("8403=").is_some_and(|v| v.starts_with("0.01")), "the whole part: {msg}");
        assert!(tag("8404=").is_some_and(|v| v.starts_with("0.005")), "the half part: {msg}");
    }

    /// One part alone is not the two-part form, and the ordinary peg still
    /// states its instruction.
    #[test]
    fn one_part_alone_is_still_the_ordinary_midpoint_peg() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::PegMid { offset: crate::types::PRICE_SCALE / 100, price_cap: 0 },
            b'0',
            crate::types::OrderAttrs { mid_offset_at_whole: 0.01, ..Default::default() },
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(tag("40=").as_deref(), Some("P"), "still the ordinary peg: {msg}");
        assert_eq!(tag("18=").as_deref(), Some("M"), "which states its instruction: {msg}");
    }

    /// A fill-or-kill order states that time in force on the wire.
    ///
    /// This venue refuses the order for the security types the live suite can
    /// reach — "the time-in-force FOK is invalid for this combination of
    /// exchange and security type", on the default destination and on ISLAND
    /// alike — so no live phase can show the encoding is right. What the venue
    /// accepts is its own; what this client writes is not, and it is checked
    /// here on the bytes.
    #[test]
    fn a_fill_or_kill_order_states_its_time_in_force() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::Limit { price: 100 * crate::types::PRICE_SCALE },
            b'4',
            crate::types::OrderAttrs::default(),
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(tag("59=").as_deref(), Some("4"), "fill or kill on the wire: {msg}");
    }

    /// An iceberg states how much of it is shown.
    ///
    /// Refused live as well — "iceberg orders not supported for this
    /// combination of exchange and security type" — and refused for every
    /// displayed quantity tried, so the field never reaches a venue that would
    /// act on it. It is still this client's job to write it.
    #[test]
    fn an_iceberg_order_states_the_quantity_it_shows() {
        let msg = send_kind_for_test(
            crate::types::OrderKind::Limit { price: 100 * crate::types::PRICE_SCALE },
            b'1',
            crate::types::OrderAttrs { display_size: 100, ..Default::default() },
        );
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));
        assert_eq!(tag("111=").as_deref(), Some("100"), "the displayed quantity: {msg}");
    }

    /// ibx#240: the tags a bracket child cannot ship without. Asserted on the
    /// bytes `send_order_ex` puts on the wire, not on the request enum — the
    /// enum-level tests passed throughout the period the child shipped naked.
    #[test]
    fn adjustable_stop_wire_carries_parent_oca_and_tif() {
        use std::io::Read;
        let (mut conn, mut peer) = crate::protocol::connection::Connection::for_test();
        let mut context = Context::new();
        let attrs = crate::types::OrderAttrs {
            parent_id: 42,
            oca_group_str: "bracket_1".to_string(),
            oca_type: 1,
            ..Default::default()
        };
        send_order_ex(
            &mut conn,
            &mut context,
            "DU123456",
            7,
            0,
            Side::Sell,
            1,
            crate::types::OrderKind::AdjustableStop {
                stop_price: 11 * crate::types::PRICE_SCALE,
                trigger_price: 12 * crate::types::PRICE_SCALE,
                adjusted_order_type: crate::types::AdjustedOrderType::Stop,
                adjusted_stop_price: 11 * crate::types::PRICE_SCALE + crate::types::PRICE_SCALE / 2,
                adjusted_stop_limit_price: 0,
                adjusted_trailing_amount: 0,
                adjustable_trailing_unit: 0,
            },
            b'1', // GTC
            &attrs,
        )
        .unwrap();

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

        assert_eq!(tag("6107=").as_deref(), Some("42.0"), "parent link missing: {msg}");
        assert_eq!(tag("583=").as_deref(), Some("bracket_1"), "OCA group missing: {msg}");
        assert_eq!(tag("59=").as_deref(), Some("1"), "tif must be GTC, not DAY: {msg}");
        // The adjustable-specific tags keep both the values and the position the
        // standalone arm gave them — after 204 and the attribute block — which
        // the sibling test pins by asserting 204 precedes 6257.
        assert_eq!(tag("40=").as_deref(), Some("3"));
        assert_eq!(tag("99="), Some(format_price(11 * crate::types::PRICE_SCALE).to_string()));
        assert_eq!(tag("6257=").as_deref(), Some("1"));
        assert_eq!(tag("6261=").as_deref(), Some(crate::types::AdjustedOrderType::Stop.fix_code()));
        assert_eq!(tag("6258="), Some(format_price(12 * crate::types::PRICE_SCALE).to_string()));
        assert_eq!(
            tag("6259="),
            Some(
                format_price(11 * crate::types::PRICE_SCALE + crate::types::PRICE_SCALE / 2)
                    .to_string()
            )
        );
    }

    /// A contract is priced in what it is priced in. The field was a constant,
    /// which names the right currency for a US instrument and the wrong one for
    /// every other, and an order naming the wrong currency names a different
    /// contract. A caller that says nothing still gets the constant.
    #[test]
    fn an_order_states_the_currency_the_contract_is_priced_in() {
        use std::io::Read;
        let sent = |key: Option<&str>| {
            let (mut conn, mut peer) = crate::protocol::connection::Connection::for_test();
            let mut context = Context::new();
            let id = context.market.try_register_contract(0, "BMW", "STK", "IBIS", "").unwrap();
            context.set_symbol(id, "BMW".to_string());
            if let Some(k) = key {
                context.set_order_identity(id, k);
            }
            send_order_ex(
                &mut conn,
                &mut context,
                "DU123456",
                12,
                id,
                Side::Buy,
                1,
                crate::types::OrderKind::Limit { price: crate::types::PRICE_SCALE },
                b'0',
                &crate::types::OrderAttrs::default(),
            )
            .unwrap();
            let mut buf = [0u8; 4096];
            let n = peer.read(&mut buf).unwrap();
            let msg = String::from_utf8_lossy(&buf[..n]).to_string();
            msg.split('\u{1}').find_map(|f| f.strip_prefix("15=").map(str::to_string)).unwrap()
        };

        assert_eq!(sent(Some("|0|||||EUR")), "EUR", "what the caller said");
        assert_eq!(sent(None), "USD", "and the old constant when nobody said");
    }

    /// The trail percentage and the unit it is expressed in are different
    /// fields, and a one percent trail is a hundred basis points while the code
    /// for percent is also a hundred — so the two agree for exactly one
    /// percentage and disagree for every other. Checked at two and a half.
    #[test]
    fn a_percent_trail_states_the_percent_and_the_unit_separately() {
        use std::io::Read;
        let (mut conn, mut peer) = crate::protocol::connection::Connection::for_test();
        let mut context = Context::new();
        send_order_ex(
            &mut conn,
            &mut context,
            "DU123456",
            9,
            0,
            Side::Sell,
            1,
            crate::types::OrderKind::TrailPct { trail_pct: 250, trail_stop_price: 0 },
            b'0',
            &crate::types::OrderAttrs::default(),
        )
        .unwrap();

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

        assert_eq!(tag("99=").as_deref(), Some("2.50"), "the percent, in decimal");
        assert_eq!(tag("211=").as_deref(), Some("2.50"), "and again where the peg carries it");
        assert_eq!(tag("6268=").as_deref(), Some("100"), "the unit is percent, not the percentage");
    }

    /// The conditional adjustable tags: 6262 only with a stop-limit conversion,
    /// 6260/6269 only with a trailing one. Same rules as the standalone arm.
    #[test]
    fn adjustable_stop_wire_carries_trail_and_limit_tags() {
        use std::io::Read;
        let (mut conn, mut peer) = crate::protocol::connection::Connection::for_test();
        let mut context = Context::new();
        send_order_ex(
            &mut conn,
            &mut context,
            "DU123456",
            8,
            0,
            Side::Sell,
            1,
            crate::types::OrderKind::AdjustableStop {
                stop_price: 11 * crate::types::PRICE_SCALE,
                trigger_price: 12 * crate::types::PRICE_SCALE,
                adjusted_order_type: crate::types::AdjustedOrderType::TrailLimit,
                adjusted_stop_price: 11 * crate::types::PRICE_SCALE,
                adjusted_stop_limit_price: 10 * crate::types::PRICE_SCALE,
                adjusted_trailing_amount: crate::types::PRICE_SCALE / 2,
                adjustable_trailing_unit: 0,
            },
            b'0',
            &crate::types::OrderAttrs::default(),
        )
        .unwrap();

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let tag = |t: &str| msg.split('\u{1}').find_map(|f| f.strip_prefix(t).map(str::to_string));

        assert_eq!(tag("6262="), Some(format_price(10 * crate::types::PRICE_SCALE).to_string()));
        assert_eq!(tag("6260="), Some(format_price(crate::types::PRICE_SCALE / 2).to_string()));
        assert_eq!(tag("6269=").as_deref(), Some("0"));
        // No parent, no OCA set: those tags must be absent, not empty.
        assert_eq!(tag("6107="), None);
        assert_eq!(tag("583="), None);

        // Order, not just presence: the adjustable tags sit after 204 and the
        // base type tags before 59, exactly where the dedicated encoder this
        // path replaced put them. Tag order is not supposed to carry meaning,
        // but this path had a shipped layout and there is no reason to change
        // it as a side effect (ibx#240).
        let pos = |t: &str| msg.split('\u{1}').position(|f| f.starts_with(t));
        assert!(pos("40=") < pos("59="), "base type tags precede tif: {msg}");
        assert!(pos("99=") < pos("59="), "stop price precedes tif: {msg}");
        assert!(pos("204=") < pos("6257="), "adjustable tags follow 204: {msg}");
        assert!(pos("6257=") < pos("6261="), "adjustable tags keep their order: {msg}");
        assert!(pos("6259=") < pos("6262="), "adjustable tags keep their order: {msg}");
        assert!(pos("6262=") < pos("6260="), "adjustable tags keep their order: {msg}");
    }
}

#[cfg(test)]
mod modify_wire_tests {
    use super::*;
    use crate::protocol::connection::Connection;
    use std::io::Read;

    /// Drive the order queue and read what actually reaches the socket.
    fn drain(context: &mut Context) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(addr).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut conn = Some(Connection::new_raw(client).unwrap());

        let shared = std::sync::Arc::new(SharedState::new());
        let mut hb = HeartbeatState::new();
        drain_and_send_orders(
            &mut conn, context, "DU111111", &mut hb, false, &shared, false, &None,
        );

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|")
    }

    /// A plain limit modified with the flag in each polarity.
    fn replace_bytes(outside_rth: bool) -> String {
        let mut context = Context::new();
        context.insert_order(crate::types::Order::new(
            7,
            0,
            Side::Buy,
            1,
            100 * crate::types::PRICE_SCALE,
            b'2',
            b'0',
            0,
        ));
        context.modify(7, 200 * crate::types::PRICE_SCALE, 50, outside_rth);
        drain(&mut context)
    }

    /// ibx#247: the replace asserted 6433=1 unconditionally, so an RTH-only
    /// order was opted into the extended session by its first modify. Pins the
    /// 6122/6433/38 neighbourhood in both polarities — presence and captured
    /// position when the caller sets the flag, absence when it does not.
    #[test]
    fn modify_emits_outside_rth_only_when_the_caller_set_it() {
        let on = replace_bytes(true);
        assert!(
            on.contains("|6122=c|6433=1|38=50|"),
            "6433 must keep its captured position between 6122 and 38: {on}"
        );

        let off = replace_bytes(false);
        assert!(!off.contains("|6433="), "an RTH-only order must not assert 6433: {off}");
        assert!(off.contains("|6122=c|38=50|"), "the rest of the message is unchanged: {off}");
    }

    /// ibx#324: a stop has no limit leg, so the price a caller supplies to a
    /// modify can only mean the trigger. Writing it to tag 44 and restating the
    /// original trigger in 99 leaves the stop where it was — and on a live
    /// gateway the replace is rejected outright, so the order the caller meant
    /// to move ends up Inactive.
    #[test]
    fn modifying_a_stop_moves_its_trigger() {
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.insert_order(crate::types::Order::new(
            7,
            instrument,
            Side::Sell,
            1,
            600 * crate::types::PRICE_SCALE,
            b'3',
            b'0',
            600 * crate::types::PRICE_SCALE,
        ));

        context.modify(7, 610 * crate::types::PRICE_SCALE, 1, false);
        let sent = drain(&mut context);

        assert!(sent.contains("|99=610|"), "the trigger moves to the new price: {sent}");
        assert!(!sent.contains("|99=600|"), "and does not restate the old one: {sent}");
        assert!(!sent.contains("|44="), "a stop has no limit leg to state: {sent}");
    }

    /// Every trigger-only type, not just the plain stop. Market-if-touched and
    /// stop-with-protection have no limit leg either.
    #[test]
    fn every_trigger_only_type_moves_its_trigger() {
        for (ord_type, name) in
            [(b'3', "STP"), (b'J', "MIT"), (crate::types::ORD_STP_PRT, "STP PRT")]
        {
            let mut context = Context::new();
            let instrument = context.register_instrument(756733);
            context.insert_order(crate::types::Order::new(
                7,
                instrument,
                Side::Sell,
                1,
                600 * crate::types::PRICE_SCALE,
                ord_type,
                b'0',
                600 * crate::types::PRICE_SCALE,
            ));

            context.modify(7, 610 * crate::types::PRICE_SCALE, 1, false);
            let sent = drain(&mut context);

            assert!(sent.contains("|99=610|"), "{name}: trigger moves: {sent}");
            assert!(!sent.contains("|44="), "{name}: no limit leg to state: {sent}");
        }

        // The bucket is pinned from above as well: a type that is not
        // trigger-only must keep its limit leg.
        for ord_type in *b"U21" {
            let mut context = Context::new();
            let instrument = context.register_instrument(756733);
            context.insert_order(crate::types::Order::new(
                7,
                instrument,
                Side::Sell,
                1,
                100 * crate::types::PRICE_SCALE,
                ord_type,
                b'0',
                0,
            ));
            context.modify(7, 610 * crate::types::PRICE_SCALE, 1, false);
            let sent = drain(&mut context);
            assert!(
                sent.contains("|44=610|"),
                "{ord_type} is not trigger-only and keeps tag 44: {sent}",
            );
        }
    }

    /// The other side of the same rule: a replace that states both the type
    /// and the trigger is stating a real one. Deciding from the resting order
    /// alone sent a stop-limit with no tag 99, which is not a stop-limit.
    #[test]
    fn a_replace_into_a_stop_limit_states_the_trigger_it_was_given() {
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        // A plain limit, so there is no resting trigger to fall back on.
        context.insert_order(crate::types::Order::new(
            7,
            instrument,
            Side::Sell,
            1,
            100 * crate::types::PRICE_SCALE,
            b'2',
            b'0',
            0,
        ));
        context.pending_orders.push(crate::types::OrderRequest::Modify {
            new_order_id: 8,
            order_id: 7,
            ord_type: b'4',
            tif: 0,
            price: 101 * crate::types::PRICE_SCALE,
            qty: 1,
            outside_rth: false,
            stop_price: 99 * crate::types::PRICE_SCALE,
        });
        let sent = drain(&mut context);

        assert!(sent.contains("|40=4|"), "it is a stop-limit now: {sent}");
        assert!(sent.contains("|44=101|"), "with its limit leg: {sent}");
        assert!(sent.contains("|99=99|"), "and the trigger it was given: {sent}");
    }

    /// A type that carries no trigger must not acquire one. The public client
    /// fills the request's trigger from `aux_price`, which is meaningless on a
    /// limit and is the offset on a pegged order — neither belongs in tag 99.
    #[test]
    fn a_type_without_a_trigger_never_gains_one() {
        for (ord_type, name) in [
            (b'2', "LMT"),
            (b'1', "MKT"),
            (b'P', "TRAIL"),
            (b'K', "MTL"),
            (crate::types::ORD_PEG_MID, "PEG MID"),
        ] {
            let mut context = Context::new();
            let instrument = context.register_instrument(756733);
            // Tracked with no trigger. A pegged or relative order tracks its
            // offset in this field, so this pins the request-supplied path
            // rather than claiming those types never emit a 99.
            context.insert_order(crate::types::Order::new(
                7,
                instrument,
                Side::Sell,
                1,
                100 * crate::types::PRICE_SCALE,
                ord_type,
                b'0',
                0,
            ));

            // A trigger arrives on the request anyway.
            context.pending_orders.push(crate::types::OrderRequest::Modify {
                new_order_id: 8,
                order_id: 7,
                ord_type: 0,
                tif: 0,
                price: 101 * crate::types::PRICE_SCALE,
                qty: 1,
                outside_rth: false,
                stop_price: 610 * crate::types::PRICE_SCALE,
            });
            let sent = drain(&mut context);

            assert!(!sent.contains("|99="), "{name} must not gain a trigger: {sent}");
            assert!(sent.contains("|44=101|"), "{name} keeps its limit leg: {sent}");
        }
    }

    /// A two-legged type can have its trigger moved when it has one.
    /// Pegged-to-market and pegged-to-midpoint share OrdType "E" and are told
    /// apart by ExecInst, exactly as `ORD_PEG_MKT` and `ORD_PEG_MID` document
    /// in types.rs. Neither emitted tag 18, so the two went on the wire byte
    /// for byte identical and neither said which peg it was — every other
    /// shared-OrdType pair in this encoder does emit its disambiguator.
    /// An option order that does not restate expiry, strike and right names no
    /// particular contract: the symbol alone is the whole chain. That is why
    /// non-stock orders were refused rather than sent, and carrying the identity
    /// is what makes sending one safe (ibx#202).
    #[test]
    fn an_option_order_names_its_contract() {
        let mut context = Context::new();
        let instrument = context
            .market
            .try_register_contract(0, "AAPL", "OPT", "SMART", "20260619|230|C|100")
            .expect("slot");
        context.market.set_symbol(instrument, "AAPL".into());
        context.market.set_routing(instrument, "OPT", "SMART");

        context.pending_orders.push(crate::types::OrderRequest::SubmitEx {
            order_id: 7,
            instrument,
            side: Side::Buy,
            qty: 1,
            kind: crate::types::OrderKind::Limit { price: 5 * crate::types::PRICE_SCALE },
            tif: b'0',
            attrs: crate::types::OrderAttrs::default(),
        });
        let sent = drain(&mut context);

        assert!(sent.contains("|167=OPT|"), "the security type: {sent}");
        assert!(sent.contains("|541=20260619|"), "a full date on the maturity-date tag: {sent}");
        assert!(sent.contains("|202=230|"), "the strike: {sent}");
        assert!(sent.contains("|201=1|"), "the right, as the wire code for a call: {sent}");
        assert!(sent.contains("|231=100|"), "the multiplier: {sent}");
    }

    /// An exercise and a lapse are new orders carrying the action, and nothing
    /// else tells them apart from each other or from an ordinary order. Read
    /// off the socket, because the request that carries the action and the
    /// message that states it are two different things.
    #[test]
    fn an_exercise_and_a_lapse_go_out_as_new_orders_carrying_the_action() {
        for (action, stated) in [(1u8, "1"), (2, "2")] {
            let mut context = Context::new();
            let instrument = context
                .market
                .try_register_contract(0, "AAPL", "OPT", "SMART", "20260619|230|C|100")
                .expect("slot");
            context.market.set_symbol(instrument, "AAPL".into());
            context.market.set_routing(instrument, "OPT", "SMART");

            context.pending_orders.push(
                crate::client_core::ClientCore::build_exercise_request(7, instrument, action, 3),
            );
            let sent = drain(&mut context);

            assert!(sent.contains("|35=D|"), "an exercise is a new order: {sent}");
            assert!(
                sent.contains(&format!("|6809={stated}|")),
                "carrying the action it was asked for: {sent}",
            );
            assert!(sent.contains("|38=3|"), "for the contracts named: {sent}");
            assert!(sent.contains("|54=1|"), "on the buy side: {sent}");
            assert!(sent.contains("|541=20260619|"), "and naming the option: {sent}");
        }
    }

    /// A stock names itself with its symbol, so none of those tags belong on it.
    /// A contract known by conId still has to restate its identity on the wire.
    /// Recording it only on the conId-less path sent a future naming its
    /// exchange and not its month, which the gateway parked.
    #[test]
    fn a_future_known_by_con_id_still_names_its_month() {
        let mut context = Context::new();
        let instrument = context
            .market
            .try_register_contract(793_356_217, "MES", "FUT", "CME", "202609|0||5")
            .expect("slot");
        context.market.set_symbol(instrument, "MES".into());
        context.market.set_routing(instrument, "FUT", "CME");

        context.pending_orders.push(crate::types::OrderRequest::SubmitEx {
            order_id: 7,
            instrument,
            side: Side::Buy,
            qty: 1,
            kind: crate::types::OrderKind::Limit { price: 3827 * crate::types::PRICE_SCALE },
            tif: b'0',
            attrs: crate::types::OrderAttrs::default(),
        });
        let sent = drain(&mut context);
        assert!(sent.contains("|167=FUT|"), "the security type: {sent}");
        assert!(sent.contains("|200=202609|"), "and the contract month: {sent}");
        assert!(sent.contains("|231=5|"), "and the multiplier: {sent}");
    }

    #[test]
    fn a_stock_order_carries_no_option_identity() {
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.market.set_symbol(instrument, "SPY".into());
        context.pending_orders.push(crate::types::OrderRequest::SubmitEx {
            order_id: 7,
            instrument,
            side: Side::Buy,
            qty: 1,
            kind: crate::types::OrderKind::Limit { price: 5 * crate::types::PRICE_SCALE },
            tif: b'0',
            attrs: crate::types::OrderAttrs::default(),
        });
        let sent = drain(&mut context);
        for tag in ["|200=", "|201=", "|202=", "|231="] {
            assert!(!sent.contains(tag), "a stock carries no {tag}: {sent}");
        }
    }

    #[test]
    fn the_two_pegs_are_told_apart_on_the_wire() {
        let mut sent = Vec::new();
        for kind in [
            crate::types::OrderKind::PegMkt { offset: 5 * crate::types::PRICE_SCALE, price_cap: 0 },
            crate::types::OrderKind::PegMid { offset: 5 * crate::types::PRICE_SCALE, price_cap: 0 },
        ] {
            let mut context = Context::new();
            let instrument = context.register_instrument(756733);
            context.pending_orders.push(crate::types::OrderRequest::SubmitEx {
                order_id: 7,
                instrument,
                side: Side::Buy,
                qty: 1,
                kind,
                tif: b'0',
                attrs: crate::types::OrderAttrs::default(),
            });
            sent.push(drain(&mut context));
        }
        // Asked live, the venue names these back as PegToMkt and PegToMid under
        // "P". Sent as "E" it named them something else entirely and refused
        // them under that other name, so a caller asking to peg had an order
        // the venue read as a different type — which is worse than a refusal.
        assert!(sent[0].contains("|40=P|"), "pegged to market is OrdType P: {}", sent[0]);
        assert!(sent[1].contains("|40=P|"), "pegged to midpoint is OrdType P: {}", sent[1]);
        assert!(sent[0].contains("|18=P|"), "pegged to market states its peg: {}", sent[0]);
        assert!(sent[1].contains("|18=M|"), "pegged to midpoint states its peg: {}", sent[1]);
        // The offset is stated once. Written twice the venue read the second.
        assert_eq!(sent[0].matches("|211=").count(), 1, "one offset: {}", sent[0]);
        assert_ne!(sent[0], sent[1], "the two pegs must not be the same message");
    }

    #[test]
    fn a_supplied_trigger_moves_a_two_legged_order() {
        for (ord_type, name) in [(b'4', "STP LMT"), (b'K', "LIT")] {
            let mut context = Context::new();
            let instrument = context.register_instrument(756733);
            context.insert_order(crate::types::Order::new(
                7,
                instrument,
                Side::Sell,
                1,
                605 * crate::types::PRICE_SCALE,
                ord_type,
                b'0',
                600 * crate::types::PRICE_SCALE,
            ));

            context.pending_orders.push(crate::types::OrderRequest::Modify {
                new_order_id: 8,
                order_id: 7,
                ord_type: 0,
                tif: 0,
                price: 610 * crate::types::PRICE_SCALE,
                qty: 1,
                outside_rth: false,
                stop_price: 590 * crate::types::PRICE_SCALE,
            });
            let sent = drain(&mut context);

            assert!(sent.contains("|44=610|"), "{name}: the limit moves: {sent}");
            assert!(sent.contains("|99=590|"), "{name}: and so does the trigger: {sent}");
        }
    }

    /// The replacement carries the trigger forward, so a second modify still
    /// has one to restate.
    #[test]
    fn the_replacement_keeps_the_trigger() {
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.insert_order(crate::types::Order::new(
            7,
            instrument,
            Side::Sell,
            1,
            600 * crate::types::PRICE_SCALE,
            b'3',
            b'0',
            600 * crate::types::PRICE_SCALE,
        ));

        let second = context.modify(7, 610 * crate::types::PRICE_SCALE, 1, false);
        drain(&mut context);
        assert_eq!(
            context.order(second).expect("tracked").stop_price,
            610 * crate::types::PRICE_SCALE,
            "the replacement records the trigger it just asked for",
        );

        context.modify(second, 620 * crate::types::PRICE_SCALE, 1, false);
        let sent = drain(&mut context);
        assert!(sent.contains("|99=620|"), "and the next modify moves it again: {sent}");
    }

    /// A type with both legs keeps the limit on 44 and holds its trigger.
    #[test]
    fn modifying_a_stop_limit_moves_the_limit_and_keeps_the_trigger() {
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.insert_order(crate::types::Order::new(
            7,
            instrument,
            Side::Sell,
            1,
            605 * crate::types::PRICE_SCALE,
            b'4',
            b'0',
            600 * crate::types::PRICE_SCALE,
        ));

        context.modify(7, 610 * crate::types::PRICE_SCALE, 1, false);
        let sent = drain(&mut context);

        assert!(sent.contains("|44=610|"), "the limit moves: {sent}");
        assert!(sent.contains("|99=600|"), "the trigger is restated unchanged: {sent}");
    }

    /// ibx#311: the bracket was the last submit path emitting a bare ClOrdID.
    /// A cancel that has seen no echo yet computes `{id}.{ver}` for OrigClOrdID,
    /// so a leg cancelled before its first execution report named an id the
    /// gateway is not holding — and tag 6107 disagreed with the parent link
    /// `send_order_ex` puts on a child of the same order.
    #[test]
    fn a_bracket_leg_is_submitted_under_the_id_its_cancel_will_name() {
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        let (parent, tp, sl) = context.submit_bracket(
            instrument,
            Side::Buy,
            1,
            100 * crate::types::PRICE_SCALE,
            110 * crate::types::PRICE_SCALE,
            90 * crate::types::PRICE_SCALE,
        );
        let submitted = drain(&mut context);

        for id in [parent, tp, sl] {
            assert!(
                submitted.contains(&format!("|11={id}.0|")),
                "leg {id} is submitted versioned: {submitted}"
            );
        }
        assert_eq!(
            submitted.matches(&format!("|6107={parent}.0|")).count(),
            2,
            "both children link the parent by the id it was submitted under: {submitted}"
        );

        // Nothing has echoed, so the cancel computes the OrigClOrdID.
        context.cancel(tp);
        let cancelled = drain(&mut context);
        assert!(
            cancelled.contains(&format!("|41={tp}.0|")),
            "the cancel names the submitted id: {cancelled}"
        );
    }
}

#[cfg(test)]
mod outside_rth_polarity_tests {
    use super::*;
    use crate::protocol::connection::Connection;
    use std::io::Read;

    /// Drive the queued orders and read what actually reaches the socket.
    fn drain(context: &mut Context) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(addr).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut conn = Some(Connection::new_raw(client).unwrap());

        let shared = std::sync::Arc::new(SharedState::new());
        let mut hb = HeartbeatState::new();
        drain_and_send_orders(
            &mut conn, context, "DU111111", &mut hb, false, &shared, false, &None,
        );

        let mut buf = vec![0u8; 8192];
        let n = peer.read(&mut buf).unwrap();
        String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|")
    }

    /// Every submit path guards tag 6433, and nothing asserted the guard: the
    /// assertions checked that the flag is present when the caller asked for
    /// it, never that it is absent when they did not. Making every encoder emit
    /// it unconditionally therefore failed no test.
    ///
    /// That is the shape ibx#247 took on the replace path, where a hard-coded
    /// 6433 opted every modified order into the extended session. An order
    /// widened to outside regular hours fills at prices the caller never meant
    /// to trade at, and no callback distinguishes it (ibx#352).
    /// A named submit path, invoked as (context, instrument, outside_rth).
    type SubmitCase = (&'static str, fn(&mut Context, u32, bool) -> crate::types::OrderId);

    #[test]
    fn every_submit_path_emits_outside_rth_only_when_it_was_asked_for() {
        let cases: Vec<SubmitCase> = vec![
            ("limit gtc", |c, i, o| {
                c.submit_limit_gtc(i, Side::Buy, 1, 100 * crate::types::PRICE_SCALE, o)
            }),
            ("stop gtc", |c, i, o| {
                c.submit_stop_gtc(i, Side::Sell, 1, 90 * crate::types::PRICE_SCALE, o)
            }),
            ("stop limit gtc", |c, i, o| {
                c.submit_stop_limit_gtc(
                    i,
                    Side::Sell,
                    1,
                    89 * crate::types::PRICE_SCALE,
                    90 * crate::types::PRICE_SCALE,
                    o,
                )
            }),
            ("extended encoder", |c, i, o| {
                c.submit_limit_ex(
                    i,
                    Side::Buy,
                    1,
                    100 * crate::types::PRICE_SCALE,
                    b'0',
                    crate::types::OrderAttrs { outside_rth: o, ..Default::default() },
                )
            }),
        ];

        for (label, submit) in cases {
            for asked in [true, false] {
                let mut context = Context::new();
                let instrument = context.register_instrument(756733);
                submit(&mut context, instrument, asked);
                let sent = drain(&mut context);

                assert_eq!(
                    sent.contains("|6433=1|"),
                    asked,
                    "{label}, outside_rth={asked}: {sent}",
                );
            }
        }
    }

    /// A connection, a context and an instrument, for a combination order.
    fn combo_test_state() -> (
        crate::protocol::connection::Connection,
        std::net::TcpStream,
        Context,
        crate::types::InstrumentId,
    ) {
        let (conn, peer) = crate::protocol::connection::Connection::for_test();
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        (conn, peer, context, instrument)
    }

    /// A combination names its legs on the order. There is no repeating group
    /// for them: a count, then a contract, a ratio and a side per leg. The side
    /// is a flag rather than the letter the order itself uses, and a leg that
    /// routes with the combination states no venue of its own.
    #[test]
    fn a_combination_names_each_of_its_legs() {
        use std::io::Read;
        let (mut conn, mut peer) = crate::protocol::connection::Connection::for_test();
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        let attrs = crate::types::OrderAttrs {
            combo_legs: vec![
                crate::types::ComboLegSpec {
                    con_id: 265598,
                    ratio: 1,
                    is_sell: false,
                    exchange: String::new(),
                    open_close: 1,
                    short_sale_slot: 0,
                    designated_location: String::new(),
                    exempt_code: -1,
                    price: None,
                },
                crate::types::ComboLegSpec {
                    con_id: 272093,
                    ratio: 2,
                    is_sell: true,
                    exchange: "ARCA".into(),
                    open_close: 0,
                    short_sale_slot: 0,
                    designated_location: String::new(),
                    exempt_code: -1,
                    price: None,
                },
            ],
            ..Default::default()
        };
        send_order_ex(
            &mut conn,
            &mut context,
            "DU123456",
            31,
            instrument,
            Side::Buy,
            1,
            crate::types::OrderKind::Limit { price: crate::types::PRICE_SCALE },
            b'0',
            &attrs,
        )
        .unwrap();

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let f: Vec<&str> = msg.split('\u{1}').collect();
        assert!(f.contains(&"6079=2"), "the leg count: {msg}");
        assert!(f.contains(&"6080=265598") && f.contains(&"6080=272093"), "each contract: {msg}");
        assert!(f.contains(&"6081=1") && f.contains(&"6081=2"), "each ratio: {msg}");
        assert!(f.contains(&"6082=0") && f.contains(&"6082=1"), "each side, as a flag: {msg}");
        // The buying leg comes first, and it is the one carrying 1.
        let sides: Vec<&&str> = f.iter().filter(|t| t.starts_with("6082=")).collect();
        assert_eq!(*sides[0], "6082=1", "a bought leg: {msg}");
        assert_eq!(*sides[1], "6082=0", "a sold leg: {msg}");
        assert!(
            f.contains(&"616=") && f.contains(&"616=ARCA"),
            "a venue only where the leg has its own: {msg}"
        );
        assert!(f.contains(&"654=1"), "the position effect where set: {msg}");
    }

    /// A caller can price the legs separately rather than pricing the
    /// combination. Dropped, the combination is worked at whatever the venue
    /// makes of it, which is not the order that was placed.
    #[test]
    fn legs_priced_separately_go_out_with_their_prices() {
        use std::io::Read;
        let (mut conn, mut peer, mut context, instrument) = combo_test_state();
        let leg = |con_id: i64, is_sell: bool, price: Option<crate::types::Price>| {
            crate::types::ComboLegSpec {
                con_id, ratio: 1, is_sell, exchange: String::new(),
                open_close: 0, short_sale_slot: 0, designated_location: String::new(),
                exempt_code: -1, price,
            }
        };
        let attrs = crate::types::OrderAttrs {
            combo_legs: vec![
                leg(265598, false, Some(2 * crate::types::PRICE_SCALE)),
                leg(272093, true, None),
            ],
            ..Default::default()
        };
        send_order_ex(
            &mut conn, &mut context, "DU123456", 32, instrument, Side::Buy, 1,
            crate::types::OrderKind::Limit { price: crate::types::PRICE_SCALE },
            b'0', &attrs,
        )
        .unwrap();

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let f: Vec<&str> = msg.split('\u{1}').collect();
        let priced: Vec<&&str> = f.iter().filter(|x| x.starts_with("6879=")).collect();
        assert_eq!(priced.len(), 2, "one price a leg, in leg order: {msg}");
        assert_eq!(*priced[0], "6879=2", "the leg the caller priced: {msg}");
        assert_eq!(*priced[1], "6879=", "and the one it left alone: {msg}");
    }

    /// Nothing goes out where the caller priced the combination itself, which
    /// is what most callers do.
    #[test]
    fn an_unpriced_combination_states_no_leg_prices() {
        use std::io::Read;
        let (mut conn, mut peer, mut context, instrument) = combo_test_state();
        let attrs = crate::types::OrderAttrs {
            combo_legs: vec![crate::types::ComboLegSpec {
                con_id: 265598, ratio: 1, is_sell: false, exchange: String::new(),
                open_close: 0, short_sale_slot: 0, designated_location: String::new(),
                exempt_code: -1, price: None,
            }],
            ..Default::default()
        };
        send_order_ex(
            &mut conn, &mut context, "DU123456", 33, instrument, Side::Buy, 1,
            crate::types::OrderKind::Limit { price: crate::types::PRICE_SCALE },
            b'0', &attrs,
        )
        .unwrap();

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        assert!(
            !String::from_utf8_lossy(&buf[..n]).contains("6879="),
            "a price nobody stated",
        );
    }

    /// A ladder and a hedge each go out under the tags the vendor's own
    /// attributes declare for them. Both used to reach no encoder, so an order
    /// that asked for either got a plain one instead — one order for the whole
    /// size, or a position with nothing against it.
    #[test]
    fn a_scale_and_a_hedge_go_out_under_their_own_tags() {
        use std::io::Read;
        let (mut conn, mut peer) = crate::protocol::connection::Connection::for_test();
        let mut context = Context::new();
        let instrument = context.register_instrument(756733);
        context.set_symbol(instrument, "SPY".to_string());
        let attrs = crate::types::OrderAttrs {
            scale: Some(Box::new(crate::types::ScaleAttrs {
                init_level_size: 100,
                subs_level_size: 50,
                price_increment: crate::types::PRICE_SCALE / 20,
                profit_offset: crate::types::PRICE_SCALE / 10,
                price_adjust_interval: 60,
                auto_reset: true,
                random_percent: true,
                ..Default::default()
            })),
            delta_neutral: Some(Box::new(crate::types::DeltaNeutralAttrs {
                order_type: "MKT".into(),
                aux_price: 0,
                con_id: 265598,
            })),
            ..Default::default()
        };
        send_order_ex(
            &mut conn,
            &mut context,
            "DU123456",
            21,
            instrument,
            Side::Buy,
            100,
            crate::types::OrderKind::Limit { price: 100 * crate::types::PRICE_SCALE },
            b'0',
            &attrs,
        )
        .unwrap();

        let mut buf = [0u8; 4096];
        let n = peer.read(&mut buf).unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        let has = |t: &str| msg.split('\u{1}').any(|f| f.starts_with(t));
        for tag in [
            "6403=100",
            "6445=50",
            "6405=0.05",
            "6446=0.1",
            "6526=60",
            "6461=1",
            "6795=1",
            "6290=MKT",
            "6150=265598",
        ] {
            assert!(has(tag), "{tag} is on the order: {msg}");
        }
    }

    /// A contract that is not a stock is named by more than its symbol, and an
    /// order that states only the symbol names a whole family — which the venue
    /// answers as ambiguous, or as a contract it does not know. One submit path
    /// restated the identity and the rest did not, so which of them an order
    /// went through decided whether it could be placed at all.
    #[test]
    fn every_submit_path_names_the_contract_and_not_just_its_symbol() {
        let cases: Vec<SubmitCase> = vec![
            ("limit gtc", |c, i, o| {
                c.submit_limit_gtc(i, Side::Buy, 1, 100 * crate::types::PRICE_SCALE, o)
            }),
            ("stop gtc", |c, i, o| {
                c.submit_stop_gtc(i, Side::Sell, 1, 90 * crate::types::PRICE_SCALE, o)
            }),
            ("stop limit gtc", |c, i, o| {
                c.submit_stop_limit_gtc(
                    i,
                    Side::Sell,
                    1,
                    89 * crate::types::PRICE_SCALE,
                    90 * crate::types::PRICE_SCALE,
                    o,
                )
            }),
            ("limit ioc", |c, i, _| {
                c.submit_limit_ioc(i, Side::Buy, 1, 100 * crate::types::PRICE_SCALE)
            }),
            ("limit fok", |c, i, _| {
                c.submit_limit_fok(i, Side::Buy, 1, 100 * crate::types::PRICE_SCALE)
            }),
        ];

        for (label, submit) in cases {
            let mut context = Context::new();
            let instrument = context
                .market
                .try_register_contract(893091670, "MES", "FUT", "CME", "20270917|0||5|MES|MESU7")
                .expect("register a future");
            context.set_symbol(instrument, "MES".to_string());
            submit(&mut context, instrument, false);
            let sent = drain(&mut context);

            // A future states its contract month. The order path carries no
            // MaturityDate at all, and a full date on the month tag named a
            // contract the venue could not settle on.
            assert!(sent.contains("|200=202709|"), "{label} states the contract month: {sent}");
            assert!(!sent.contains("|541="), "{label} states no maturity date: {sent}");
            assert!(sent.contains("|231=5|"), "{label} states the multiplier: {sent}");
            assert!(sent.contains("|167=FUT|"), "{label} states the security type: {sent}");
            // The member, not the family: the local symbol under the source
            // that says the identifier is the venue's own.
            assert!(!sent.contains("|6058="), "{label} states no trading class: {sent}");
            assert!(sent.contains("|48=MESU7|"), "{label} names the contract: {sent}");
            assert!(sent.contains("|22=101|"), "{label} says what the identifier is: {sent}");
            // An order that asked for neither states neither. A zero percent
            // offset is a relative order and a zero exempt code is a short
            // sale exemption, so a derived default put both on every order.
            assert!(!sent.contains("|9822="), "{label} claims no percent offset: {sent}");
            assert!(!sent.contains("|1688="), "{label} claims no exemption: {sent}");
            assert!(!sent.contains("|21="), "{label} states no handling instruction: {sent}");
            assert!(sent.contains("|204=0|"), "{label} says who the order is for: {sent}");
        }
    }
}
