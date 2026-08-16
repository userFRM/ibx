use super::{CcpState, RECOVERY_TERMINATOR_GRACE};
use std::time::Instant;



use crate::bridge::{Event, RichOrderInfo, SharedState};
use crate::api::types as api;
use crate::engine::context::Context;
use crate::protocol::connection::{Connection, Frame};
use crate::protocol::fix;
use crate::protocol::fixcomp;
use crate::types::{
    CompletedOrder, Fill, InstrumentId, Price, Side, PRICE_SCALE,
};
use std::sync::mpsc::SyncSender;

use super::{HeartbeatState, emit, parse_price_tag, decode_tif};




/// Synthetic ibapi error code for a parked (39=I) order's reason, delivered
/// through `Wrapper::error` since ibapi has no callback dedicated to an order
/// held with a reason. Mirrors IB's generic order-message code (399) rather
/// than the reject code (201) — an Inactive order is not rejected, it can
/// still reactivate.
const ORDER_INACTIVE_ERROR_CODE: i32 = 399;


/// Convert a FIX OrderID hex string (e.g. "00cf16ed.000225ed.69ca0941.0001") to a stable i64 permId.
/// Uses FNV-1a hash of the first 3 dot-segments (the stable prefix) so that permId
/// remains constant across modifications (the last segment increments on each modify).
/// Extract the value of a single FIX tag from a raw message.
/// `prefix` should include the tag number and `=` (e.g. `b"6256="`).
/// Where to book a fill whose order this session does not track.
///
/// Both the contract and the side come off the report, and both are required:
/// a guessed side would move the position the wrong way, which is worse than
/// reporting that the fill could not be placed.
pub(crate) fn untracked_fill_target(
    context: &mut Context,
    parsed: &std::collections::HashMap<u32, String>,
) -> Option<(InstrumentId, Side)> {
    // A replayed execution restates history rather than reporting something
    // new. On a fresh process the gateway resends prior fills with 97=Y and
    // their original ExecIDs, for orders no session tracks; booking those
    // would build a position out of the past on top of the one the position
    // feed already reports. Within a process the ExecID window catches the
    // reconnect burst, so only the untracked case needs this.
    let replayed = |tag| parsed.get(&tag).map(|v| v.eq_ignore_ascii_case("Y")).unwrap_or(false);
    if replayed(97) || replayed(43) {
        log::debug!("Untracked fill is a replay, leaving the position alone");
        return None;
    }
    let con_id: i64 = parsed.get(&6008).and_then(|s| s.parse().ok()).unwrap_or(0);
    if con_id == 0 {
        log::warn!("Untracked fill carries no ContractID, position not updated");
        return None;
    }
    let side = match parsed.get(&54).map(|s| s.as_str()) {
        Some("1") => Side::Buy,
        Some("2") => Side::Sell,
        Some("5") => Side::ShortSell,
        other => {
            log::warn!("Untracked fill has Side={other:?}, position not updated");
            return None;
        }
    };
    // Fallible: a full instrument table must not abort the engine on an
    // inbound message.
    let Some(instrument) = context.try_register_instrument(con_id) else {
        log::warn!("Untracked fill for conId {con_id}: instrument table full, position not updated");
        return None;
    };
    if let Some(symbol) = parsed.get(&55) {
        context.set_symbol(instrument, symbol.clone());
    }
    Some((instrument, side))
}

/// The gateway's stated reason for a parked or rejected order: the tag 58 text
/// with the tag 103 reason code. Either alone is ambiguous — the text is often
/// generic and the code alone names no instrument — so both are reported when
/// the report carries both. Empty when it carries neither.
pub(crate) fn stated_reason(parsed: &std::collections::HashMap<u32, String>) -> String {
    let text = parsed.get(&58).map(|s| s.as_str()).unwrap_or("");
    let code = parsed.get(&103).map(|s| s.as_str()).unwrap_or("");
    match (text.is_empty(), code.is_empty()) {
        (false, false) => format!("{text} (reason code {code})"),
        (false, true) => text.to_string(),
        (true, false) => format!("reason code {code}"),
        (true, true) => String::new(),
    }
}

pub(crate) fn perm_id_from_fix_order_id(s: &str) -> i64 {
    // Hash only the stable prefix: "00cf16ed.000225ed.69ca0941" (drop ".0001")
    let stable = match s.rmatch_indices('.').next() {
        Some((idx, _)) if s[..idx].contains('.') => &s[..idx],
        _ => s, // no dots or only one segment — hash entire string
    };
    let mut h: u64 = 0xcbf29ce484222325;
    for b in stable.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h >> 1) as i64
}

/// The update that says an order's state is no longer known. Emitted when the
/// connection drops with it working, and again if the recovery does not
/// account for it.
pub(crate) fn uncertain_update(
    order: &crate::types::Order,
    cached: Option<crate::bridge::RichOrderInfo>,
) -> crate::types::OrderUpdate {
    crate::types::OrderUpdate {
                order_id: order.order_id,
                instrument: order.instrument,
                status: crate::types::OrderStatus::Uncertain,
                filled_qty: order.filled as f64,
                // A fractional order deliberately tracks `qty` as zero — the
                // decimal it was submitted with lives only in the enriched
                // record. Both quantity fields are floating point end to
                // end — the dispatchers already hand them to the callback
                // as f64 — so the fraction itself survives exactly here
                // rather than being rounded to a whole unit.
                remaining_qty: {
                    let outstanding = |total: f64| (total - order.filled as f64).max(0.0);
                    if order.qty > 0 {
                        outstanding(order.qty as f64)
                    } else if let Some(c) = cached.as_ref() {
                        outstanding(c.order.total_quantity)
                    } else {
                        // No exec report has reached this order yet, so
                        // neither its quantity nor a fill is known — both
                        // arrive on the same message — and there is no
                        // honest quantity to give. ibapi's own "value not
                        // set" sentinel, rather than a guessed number.
                        f64::MAX
                    }
                },
                // Nothing here states what it paid, and this update exists to
                // say what is no longer known.
                avg_price: 0,
                perm_id: cached.as_ref().map(|c| c.order.perm_id).unwrap_or(0),
                parent_id: cached.as_ref().map(|c| c.order.parent_id).unwrap_or(0),
                timestamp_ns: 0,
    }
}

/// Every tag the execution-report handler reads.
///
/// Derived from the handler itself so it cannot fall behind as fields are
/// added, the same way a definition's is.
pub fn tags_read_from_an_execution() -> Vec<u32> {
    // Both files, because the reading is done in both: the report handler is
    // here and the routing that reads a few tags of its own is next door. A
    // scan of one names fewer tags than are read, and every tag it misses is
    // then reported as a field the venue sent that nothing read.
    let source = concat!(include_str!("mod.rs"), include_str!("executions.rs"));
    let mut seen: Vec<u32> = Vec::new();
    for cap in source.split("parsed.get(&").skip(1) {
        let token: String = cap.chars().take_while(|c| *c != ')').collect();
        let token = token.trim();
        let tag = token.parse::<u32>().ok().or_else(|| {
            let needle = format!("pub const {token}: u32 = ");
            let at = crate::protocol::fix::SOURCE.find(&needle)? + needle.len();
            crate::protocol::fix::SOURCE[at..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        });
        if let Some(tag) = tag
            && !seen.contains(&tag)
        {
            seen.push(tag);
        }
    }
    seen.sort_unstable();
    seen
}

/// What a report stated that nothing here reads, in the order stated.
///
/// Read from the bytes rather than the parsed map: a map holds one value per
/// tag, and a report repeats them.
pub fn unnamed_execution_fields(data: &[u8]) -> Vec<(u32, String)> {
    let read = tags_read_from_an_execution();
    let mut out = Vec::new();
    for part in data.split(|&b| b == crate::protocol::fix::SOH) {
        if part.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(part);
        let Some((tag_str, value)) = text.split_once('=') else { continue };
        let Ok(tag) = tag_str.parse::<u32>() else { continue };
        // The message's own fields are not the fill's.
        if read.contains(&tag) || matches!(tag, 8 | 9 | 10 | 34 | 35 | 43 | 49 | 52 | 56 | 115) {
            continue;
        }
        out.push((tag, value.to_string()));
    }
    out
}

/// Answer a preview, and say whether it was the whole of this report.
///
/// The venue prices an order it has not placed on the same message as one it
/// has, and marks it with 6091. A preview it refuses carries no figures at all
/// — it arrives shaped exactly like the not-ready acknowledgement, every field
/// "n/a", and says why on 58 — so a refusal answers `false` and is reported
/// like any other report rather than read as a preview with nothing in it.
fn take_what_if(
    parsed: &std::collections::HashMap<u32, String>,
    clord_id: u64,
    context: &mut Context,
    shared: &SharedState,
    event_tx: &Option<SyncSender<Event>>,
) -> bool {
        const MARGIN_TAGS: [u32; 6] = [6826, 6827, 6828, 6092, 6093, 6094];
        let is_data_frame = MARGIN_TAGS.iter().any(|tag| {
            parsed.get(tag)
                .and_then(|s| s.parse::<f64>().ok())
                .is_some_and(|f| f.is_finite())
        });
        if is_data_frame
            && let Some(order) = context.order(clord_id).copied() {
                let response = crate::types::WhatIfResponse {
                    order_id: clord_id,
                    instrument: order.instrument,
                    init_margin_before: parse_price_tag(parsed.get(&6826)),
                    maint_margin_before: parse_price_tag(parsed.get(&6827)),
                    equity_with_loan_before: parse_price_tag(parsed.get(&6828)),
                    init_margin_after: parse_price_tag(parsed.get(&6092)),
                    maint_margin_after: parse_price_tag(parsed.get(&6093)),
                    equity_with_loan_after: parse_price_tag(parsed.get(&6094)),
                    commission: parse_price_tag(parsed.get(&6378)),
                    min_commission: parse_price_tag(parsed.get(&6379)),
                    max_commission: parse_price_tag(parsed.get(&6380)),
                    commission_currency: parsed.get(&6381).cloned().unwrap_or_default(),
                    // The venue's own warning, which rides its own tag and
                    // not the order's text.
                    warning_text: parsed.get(&6361).cloned().unwrap_or_default(),
                };
                log::info!("WhatIf response: clord={} initMargin={:.2}->{:.2} commission={:.2}",
                    clord_id,
                    response.init_margin_before as f64 / PRICE_SCALE as f64,
                    response.init_margin_after as f64 / PRICE_SCALE as f64,
                    response.commission as f64 / PRICE_SCALE as f64);
                context.retire_order(clord_id);
                shared.orders.push_what_if(response.clone());
                emit(event_tx, Event::WhatIf(response));
            }
    parsed.get(&39).map(|s| s.as_str()) != Some("8")
}

/// What a report says the order's state now is.
///
/// The wire's own status is not always the one the counterpart reports: 39=0
/// is New on the wire and PreSubmitted until an exchange has acknowledged the
/// order, which shows up on the same report as a destination and an exec
/// reference. Reading the wire value straight through told a caller an order
/// was working while it was still being routed.
fn status_of(
    ord_status: &str,
    clord_id: u64,
    parsed: &std::collections::HashMap<u32, String>,
) -> crate::types::OrderStatus {
    match ord_status {
        "0" => {
            // 39=0 is New on the wire, but the gateway reports PreSubmitted
            // until the order is actually routed to and acknowledged by an
            // exchange (for example a limit order resting pre-market). Routing
            // shows up on the same exec report as a non-empty ExDestination
            // (tag 100) plus an exec ref (tag 198) other than "NONE"; before
            // routing both are absent/"NONE". Captured in.
            let routed = parsed.get(&100).is_some_and(|s| !s.is_empty())
                || parsed.get(&198).is_some_and(|s| s != "NONE" && !s.is_empty());
            if routed {
                crate::types::OrderStatus::Submitted
            } else {
                crate::types::OrderStatus::PreSubmitted
            }
        }
        "5" => crate::types::OrderStatus::Submitted,
        "A" => crate::types::OrderStatus::PreSubmitted,
        "E" => crate::types::OrderStatus::PendingReplace,
        "6" => crate::types::OrderStatus::PendingCancel,
        "1" => crate::types::OrderStatus::PartiallyFilled,
        "2" => crate::types::OrderStatus::Filled,
        "4" | "C" => crate::types::OrderStatus::Cancelled,
        // Not cancelled. The terminal groups D with pending-cancel and its
        // own "is this terminal" test names only 2, 4, C and 8 — reading it
        // as cancelled retired an order that was still working.
        "D" => crate::types::OrderStatus::PendingCancel,
        "8" => crate::types::OrderStatus::Rejected,
        "I" => crate::types::OrderStatus::Inactive,
        other => {
            // A status this does not know is not a reason to drop the
            // report: it may carry a fill, and returning here threw the
            // fill away with it. Say so and carry on to the execution.
            log::warn!("Unknown order status 39={other} for order {clord_id} — \
                        the report is still read for its execution");
            crate::types::OrderStatus::Uncertain
        }
    }
}

impl CcpState {
    /// Book what a report says was filled.
    ///
    /// A fill can arrive for an order this session does not track: one that
    /// raced its own cancel-ack out of the book, one placed from another
    /// client, one left from an earlier session. The report names the contract
    /// and the side, so it is booked from those rather than dropped — a
    /// position the account actually holds is not this client's to forget.
    #[allow(clippy::too_many_arguments)]
    fn book_fill(
        &mut self,
        parsed: &std::collections::HashMap<u32, String>,
        clord_id: u64,
        dedup_key: &str,
        is_resend: bool,
        last_px: f64,
        last_shares: i64,
        report_cum_qty: i64,
        commission: f64,
        leaves_qty: i64,
        order_cum_qty: i64,
        order_avg_px: f64,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
    ) {
        // A fill can arrive for an order this session does not track: one
        // that raced its own cancel-ack out of the book, one placed from
        // another client, or one left from an earlier session. The report
        // names the contract and the side, so book it from that rather
        // than dropping a position the account actually holds. An untracked
        // order has nothing filled yet, so the arithmetic below reconciles
        // against zero.
        let target = match context.order(clord_id).copied() {
            Some(order) => Some((order.instrument, order.side, order.filled as i64)),
            None => untracked_fill_target(context, parsed).map(|(i, s)| (i, s, 0i64)),
        };
        if let Some((instrument, side, already_filled)) = target {
            let booked = if is_resend {
                // Recorded even though the cumulative figure is what decides
                // this copy: the same execution can arrive again without its
                // marker, and the window is what catches that one. Recorded
                // here rather than earlier so an execution that reaches this
                // handler before its order does is not spent on a delivery
                // that had nothing to book against.
                self.record_exec_id(dedup_key);
                if report_cum_qty <= 0 {
                    // Nothing to reconcile against. Booking the increment
                    // would double what the recovery record already seeded.
                    log::debug!("Resent execution for order {clord_id} carries no CumQty — not booked");
                    0
                } else {
                    let delta = (report_cum_qty - already_filled).max(0);
                    if delta != last_shares && delta > 0 {
                        // The report's own increment is not what this client
                        // is missing, so the fill that follows carries a
                        // reconciled quantity at this report's price rather
                        // than one execution's own terms. The order's total
                        // and the position are right; the execution record
                        // is approximate, and says so here.
                        log::warn!(
                            "Resent execution for order {clord_id}: booking {delta} to reach CumQty {report_cum_qty} \
                             (report states {last_shares}) — execution detail is reconciled, not exact",
                        );
                    }
                    delta
                }
            } else if !self.record_exec_id(dedup_key) {
                // A duplicate suppresses the fill and nothing else: the
                // report still carries a status to apply and terminal
                // bookkeeping to run, and returning here skipped both.
                log::warn!("Duplicate execution key={dedup_key} — the fill is already booked");
                0
            } else {
                last_shares
            };
            if booked > 0 {
                context.update_order_filled(clord_id, booked as u32);
                let fill = Fill {
                    instrument,
                    order_id: clord_id,
                    side,
                    price: (last_px * PRICE_SCALE as f64) as i64,
                    qty: booked,
                    remaining: leaves_qty,
                    commission: (commission * PRICE_SCALE as f64) as i64,
                    timestamp_ns: context.now_ns(),
                    cum_qty: order_cum_qty,
                    avg_price: (order_avg_px * PRICE_SCALE as f64) as i64,
                };
                let delta = match side {
                    Side::Buy => booked,
                    Side::Sell | Side::ShortSell => -booked,
                };
                context.update_position(instrument, delta as f64);
                // notify_fill inlined
                shared.orders.push_fill(fill);
                shared.portfolio.set_position(fill.instrument, context.position(fill.instrument));
                // The holding the caller reads is keyed by contract, and
                // the broker restates that feed on its own schedule — never
                // because an order filled. Left to that feed alone, a
                // position read back after a fill is the one the session
                // started with.
                // The report names the contract it filled. An order placed
                // by symbol registers an instrument that knows no contract
                // id, so taking it from the instrument attributed nothing.
                let filled_con_id = parsed.get(&6008)
                    .and_then(|s| s.parse::<i64>().ok())
                    .filter(|id| *id != 0)
                    .or_else(|| context.market.con_id(instrument));
                if let Some(con_id) = filled_con_id {
                    shared.portfolio.apply_fill(
                        con_id, delta as f64, (last_px * PRICE_SCALE as f64) as Price,
                    );
                }
                emit(event_tx, Event::Fill(fill));
            }
        }
    }


    /// Take an order this session never saw from the venue's own account of it.
    ///
    /// At session start the venue replays what it holds as ordinary
    /// acknowledgements, and a fresh process has nothing to match them against.
    /// The record built here is what every later report for that order is read
    /// against, so a field guessed here is wrong for the order's whole life — the
    /// side most of all, since a recovered buy recorded as a sell moves the
    /// position the wrong way by twice the fill.
    fn recover_order(
        &mut self,
        parsed: &std::collections::HashMap<u32, String>,
        clord_id: u64,
        prior: Option<crate::types::Order>,
        context: &mut Context,
        shared: &SharedState,
    ) {
        let con_id: i64 = parsed.get(&6008).and_then(|s| s.parse().ok()).unwrap_or(0);
        // The side has to be stated. A guess does not stay in the recovered
        // record: every later fill for the order books through the tracked
        // path and takes its side from here, so a recovered buy recorded as
        // a sell moves the position down by the filled quantity instead of
        // up — wrong by twice the fill, and indistinguishable afterwards
        // from a side the report actually carried.
        let side = match parsed.get(&54).map(|s| s.as_str()) {
            Some("1") => Some(Side::Buy),
            Some("2") => Some(Side::Sell),
            Some("5") => Some(Side::ShortSell),
            other => {
                // The sentinel that terminates a recovery burst, and the
                // mass-status echo, both parse to id 0 and carry no side.
                // Warning about those once per connect would cry wolf on
                // the one signal that matters when a real record is
                // refused.
                if clord_id != 0 {
                    log::warn!(
                        "Recovery record for order {clord_id} has Side={other:?}; not tracking it",
                    );
                }
                None
            }
        };
        let qty: u32 = parsed.get(&38).and_then(|s| s.parse::<f64>().ok()).map(|q| q as u32)
            .unwrap_or_else(|| prior.map_or(0, |o| o.qty));
        let limit_price_i64: i64 = parsed.get(&44)
            .and_then(|s| s.parse::<f64>().ok())
            .map(|p| (p * PRICE_SCALE as f64) as i64)
            .unwrap_or_else(|| prior.map_or(0, |o| o.price));
        let stop_price_i64: i64 = parsed.get(&99)
            .and_then(|s| s.parse::<f64>().ok())
            .map(|p| (p * PRICE_SCALE as f64) as i64)
            .unwrap_or_else(|| prior.map_or(0, |o| o.stop_price));
        let ord_type_byte: u8 = parsed.get(&40).and_then(|s| s.bytes().next())
            .unwrap_or_else(|| prior.map_or(b'2', |o| o.ord_type));
        // A recovery record with no tag 59 states no time-in-force, and this
        // order was not placed by this session, so there is nothing to
        // recover it from. Recorded as unstated rather than guessed: either
        // guess is restated as a real instruction on the next replace, and
        // an invented DAY would expire a resting GTC order at the close.
        let tif_byte: u8 = parsed.get(&59)
            .and_then(|s| s.bytes().next())
            .unwrap_or_else(|| prior.map_or(crate::types::TIF_UNSTATED, |o| o.tif));
        if let (Some(side), true) = (side, con_id != 0 && qty > 0) {
            // Recovery is fed by gateway frames, so a full instrument
            // table must degrade to a missing order rather than take the
            // engine down. The reconnect burst replays every
            // resting order, which is exactly when the table fills.
            // Skipping only the insert keeps the order in last_clord and
            // the rich-order cache, so req_open_orders still shows it —
            // but it is NOT in the engine book, so a later fill or
            // terminal status for it is dropped and no OrderUpdate
            // reaches the caller. A missing order beats taking
            // the engine down; it is not a complete answer.
            match context.try_register_instrument(con_id) {
                None => log::warn!(
                    "recovery: instrument table full, order clord={clord_id} con_id={con_id} not tracked in the engine book",
                ),
                Some(instrument) => {
            if let Some(sym) = parsed.get(&55) {
                context.set_symbol(instrument, sym.clone());
            }
            context.insert_order(crate::types::Order {
                order_id: clord_id,
                instrument,
                side,
                price: limit_price_i64,
                qty,
                // Seeded from the recovery push rather than assumed zero.
                // Without it a fresh process believes nothing has filled,
                // and the replayed executions behind this record all look
                // like new quantity.
                filled: parsed.get(&14)
                    .and_then(|s| s.parse::<f64>().ok())
                    .map(|c| c as u32)
                    .unwrap_or_else(|| prior.map_or(0, |o| o.filled)),
                // An order this session never saw is working by the fact of
                // being in the push. One whose state was not known stays
                // not known here, so the status this very message carries
                // moves it, and the caller who was told it was unknown is
                // told what it is.
                status: if prior.is_some() {
                    crate::types::OrderStatus::Uncertain
                } else {
                    crate::types::OrderStatus::Submitted
                },
                ord_type: ord_type_byte,
                tif: tif_byte,
                stop_price: stop_price_i64,
            });
            self.hydrated_any = true;
            log::info!("CCP recovery: inserted orderId={} sym={:?} side={:?} qty={} px={}",
                clord_id, parsed.get(&55), side, qty,
                limit_price_i64 as f64 / PRICE_SCALE as f64);
            // Published, not just tracked. The engine knowing an order is
            // working does the caller no good on its own: `req_open_orders`
            // reads what has been published, so an order the server named
            // at connect went unreported until some later message about it
            // happened to arrive. A caller asking what it already has on,
            // at the moment it starts, was told nothing.
            let sec_type_str = context.market.order_routing(instrument).0;
            shared.orders.push_order_info(clord_id, crate::bridge::RichOrderInfo {
                contract: api::Contract {
                    con_id,
                    symbol: parsed.get(&55).cloned().unwrap_or_default(),
                    sec_type: sec_type_str,
                    currency: parsed.get(&15).cloned().unwrap_or_default(),
                    ..Default::default()
                },
                order: api::Order {
                    order_id: clord_id as i64,
                    action: match side {
                        Side::Buy => "BUY".to_string(),
                        _ => "SELL".to_string(),
                    },
                    total_quantity: qty as f64,
                    order_type: crate::types::ord_type_fix_str(ord_type_byte).to_string(),
                    lmt_price: limit_price_i64 as f64 / PRICE_SCALE as f64,
                    aux_price: stop_price_i64 as f64 / PRICE_SCALE as f64,
                    account: parsed.get(&1).cloned().unwrap_or_default(),
                    ..Default::default()
                },
                order_state: api::OrderState {
                    status: "Submitted".to_string(),
                    ..Default::default()
                },
                last_exec: Default::default(),
            });
                }
            }
        }
    }

    pub(crate) fn poll_executions(
        &mut self,
        ccp_conn: &mut Option<Connection>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        hb: &mut HeartbeatState,
        account_id: &str,
    ) {
        if self.disconnected { return; }
        let messages = match ccp_conn.as_mut() {
            None => return,
            Some(conn) => {
                match conn.try_recv() {
                    Ok(0) if !conn.has_buffered_data() => return,
                    Ok(0) => {}
                    Err(e) => {
                        log::error!("CCP connection lost: {e}");
                        self.handle_disconnect(context, shared, event_tx);
                        return;
                    }
                    Ok(_) => {
                        hb.last_ccp_recv = Instant::now();
                        // RTT sample: interval from the test request
                        // to the first inbound traffic after it. On a quiet
                        // link (the ping use case) that is the echo itself.
                        if let Some((_, sent_at)) = hb.pending_ccp_test.take() {
                            shared.set_ccp_rtt(hb.last_ccp_recv.duration_since(sent_at));
                        }
                    }
                }
                let frames = conn.extract_frames();
                let mut msgs = Vec::new();
                for frame in frames {
                    match frame {
                        Frame::FixComp(raw) => {
                            let Some(unsigned) = conn.unsign(&raw) else { continue };
                            match fixcomp::fixcomp_decompress(&unsigned) {
                                Ok(inner) => {
                                    if log::log_enabled!(log::Level::Trace) {
                                        for m in &inner {
                                            log::trace!("WIRE< ccp/comp {}", fix::fmt_pipe(m));
                                        }
                                    }
                                    msgs.extend(inner);
                                }
                                Err(e) => {
                                    log::warn!(
                                        "CCP: dropping malformed FIXCOMP frame ({} bytes): {}",
                                        unsigned.len(), e,
                                    );
                                }
                            }
                        }
                        Frame::Fix(raw) => {
                            let Some(unsigned) = conn.unsign(&raw) else { continue };
                            if log::log_enabled!(log::Level::Trace) {
                                log::trace!("WIRE< ccp/fix {}", fix::fmt_pipe(&unsigned));
                            }
                            msgs.push(unsigned);
                        }
                        Frame::Binary(raw) => {
                            let Some(unsigned) = conn.unsign(&raw) else { continue };
                            if log::log_enabled!(log::Level::Trace) {
                                log::trace!("WIRE< ccp/bin {}", fix::fmt_pipe(&unsigned));
                            }
                            msgs.push(unsigned);
                        }
                        Frame::Control(_) => {
                        // 8=1 / 8=X control state — not consumed on the order path.
                        }
                    }
                }
                msgs
            }
        };
        for msg in &messages {
            self.process_ccp_message(msg, ccp_conn, context, shared, event_tx, hb, account_id);
        }
    }

    pub(crate) fn handle_exec_report(
        &mut self,
        parsed: &std::collections::HashMap<u32, String>,
        raw: &[u8],
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        account_id: &str,
    ) {
        // CCP recovery push format A (, captured against live):
        // 35=8 with 150=0/39=0, tag 11 carries `<permId>.0`, the originating
        // orderId is in tag 6121. For these, prefer 6121 as the local key so
        // cancel_order(<prior-session orderId>) finds the right ClOrdID.
        // Format B (paper account, observed live): tag 11 carries the
        // originating orderId directly with `.0` suffix, tags 6119/6121
        // absent — the existing tag-11 split below already gives the right
        // value. The unwrap_or_else fallback handles both.
        let recovery_origin_order_id: Option<u64> = if parsed.get(&150).map(|s| s.as_str()) == Some("0")
            && parsed.get(&39).map(|s| s.as_str()) == Some("0")
            && parsed.contains_key(&6121)
        {
            parsed.get(&6121).and_then(|s| s.parse::<u64>().ok())
        } else {
            None
        };

        let clord_id = recovery_origin_order_id.unwrap_or_else(|| {
            parsed.get(&11).and_then(|s| {
                // A cancel names the order with a leading C, and a position the
                // broker liquidated with a leading L. Only the first was taken
                // off, so every report on a liquidated position parsed to no
                // order at all and the fill reached nobody: a forced
                // liquidation was the one fill a caller could not
                let stripped = s.strip_prefix('C').or_else(|| s.strip_prefix('L')).unwrap_or(s);
                // Strip versioned suffix (.0, .1, .2) from modify-chained ClOrdIDs
                let base = stripped.split('.').next().unwrap_or(stripped);
                base.parse::<u64>().ok()
            }).unwrap_or(0)
        });

        // Recovery insert: a 35=8 with status New/New (150=0/39=0) for an order
        // that is NOT in this session's context is a cross-session recovery entry
        // pushed by CCP on session establishment. Insert into context.open_orders
        // so subsequent cancel/modify ACKs at ~line 668 can match via
        // context.order(clord_id) and emit OrderUpdate events to the user.
        let is_new_ack = parsed.get(&150).map(|s| s.as_str()) == Some("0")
            && parsed.get(&39).map(|s| s.as_str()) == Some("0");
        // The sentinel is dropped further down, but this recovery insert runs
        // first — without the guard, a `11='*'` terminator registers a conId
        // and inserts the reserved order id 0 before being "discarded".
        // An order whose state is not known is also hydrated from this echo,
        // not just an absent one. A replace overwrites the tracked record
        // before it goes out, so a replace that failed left the attempted
        // definition in place; the server's account of the order is the
        // authority and replaces it. Anything with a status the engine still
        // believes is left alone.
        // What the engine already holds for this order, where it holds
        // anything. The push states what the broker has and omits the rest —
        // tag 59 among them — so an unstated field keeps what was known rather
        // than taking a default meant for an order this session never saw.
        let prior = context.order(clord_id)
            .filter(|o| o.status == crate::types::OrderStatus::Uncertain)
            .copied();
        let unknown = prior.is_some();
        // An order that already finished this session is not brought back by a
        // frame that arrives behind it. The gateway echoes a working status
        // after a fill, and the tracked record is gone by then — retired when
        // the order finished — so its absence reads as "never seen" and the
        // echo would insert it as live, with none of the fill on it.
        let already_finished = shared.orders.recently_completed(clord_id);
        if is_new_ack && clord_id != 0 && !already_finished
            && (context.order(clord_id).is_none() || unknown)
        {
            self.recover_order(parsed, clord_id, prior, context, shared);
        }

        // Drop the sentinel/end-of-stream record (ClOrdID="*"/"0"/absent → parses
        // to 0). Real orders are assigned monotonic IDs via next_order_id and
        // never collide with 0. The recovery-push terminator (11='*') lands here.
        if clord_id == 0 {
            log::debug!("ExecReport: dropping sentinel record (ClOrdID=0/*) sym={:?} status={:?}",
                parsed.get(&55), parsed.get(&39));
            // Everything already working has now been named. The same record
            // shape also carries a mass-status echo that arrives before any
            // order, so this only counts once at least one has come through —
            // otherwise a caller is told the replay is over before it starts.
            if self.hydrated_any {
                shared.orders.set_replay_done();
            }
            // The push said everything it was going to say, so the orders it
            // left out can be judged without waiting out the whole grace.
            if self.recovery_sweep_at.is_some() {
                self.recovery_sweep_at = Some(Instant::now() + RECOVERY_TERMINATOR_GRACE);
            }
            return;
        }

        // Record the ClOrdID exactly as the server reports it so subsequent
        // cancel/modify can echo back the same string. Skip cancel-ack frames
        // (tag 11 starts with 'C' there) — those carry the cancel request's
        // own id, not the original order's.
        if let Some(raw_clord) = parsed.get(&11)
            && !raw_clord.starts_with('C') && raw_clord != "*" {
                context.last_clord.insert(clord_id, raw_clord.clone());
            }

        // What-If response: tag 6091=1 with margin data (tag 6092+).
        // The gateway emits a not-ready ack frame whose margin fields carry the
        // literal string "n/a" (parse fails), then a data frame with numbers.
        // Discriminate on parse-success, NOT positivity: a margin-reducing
        // preview (closing a position, cash-account sell) legitimately resolves
        // to init_margin_after == 0, and the gateway sends that as a numeric "0"
        // which must be delivered. Guarding on `> 0.0` silently dropped those
        // and left the caller's pending what-if to time out.
        // The not-ready ack is not always emitted — close/reject previews send a
        // single data frame — so accept the first data frame with no assumption
        // that an ack precedes it. A frame is the real preview when ANY of the
        // six margin fields (6826/6827/6828 before, 6092/6093/6094 after)
        // parses as a finite number, mirroring the gateway's own real-frame
        // test: each field is "set" when it parses, unset on nan/unparseable,
        // and the frame is real when any field is set. The
        // ack carries "n/a" in all six, so it never matches. Captured
        // byte-level in.
        if parsed.get(&6091).map(|s| s.as_str()) == Some("1")
            && take_what_if(parsed, clord_id, context, shared, event_tx)
        {
            return;
        }

        let ord_status = parsed.get(&39).map(|s| s.as_str()).unwrap_or("");
        let exec_type = parsed.get(&150).map(|s| s.as_str()).unwrap_or("");
        let exec_id = parsed.get(&17).map(|s| s.as_str()).unwrap_or("");
        let last_px = parsed.get(&31).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let last_shares = parsed.get(&32).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        // Absent is not zero. Without 151 the caller was told nothing was left
        // on an order that was still working; the terminal falls back to the
        // order quantity less what has filled, and so does this.
        let leaves_qty = parsed.get(&151).and_then(|s| s.parse::<i64>().ok()).unwrap_or_else(|| {
            let ordered = parsed.get(&38).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let done = parsed.get(&14).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            ((ordered - done).max(0.0)) as i64
        });
        // 14 CumQty and 6 AvgPx describe the order as a whole; 32 and 31
        // describe this print alone. The gateway sends all four on every
        // execution report.
        //
        // When the cumulative quantity is absent, the print alone is not a
        // substitute: on the second fill of an order it is smaller than what
        // was already reported, so `filled` would go backwards. Add the print
        // to what the order has already accumulated instead. The average price
        // is not reconstructible that way, so it falls back to the print — and
        // a negative average is a real value for a spread, so only an absent
        // or unparseable tag falls back at all.
        let order_cum_qty = parsed.get(&14).and_then(|s| s.parse::<f64>().ok())
            .map(|q| q as i64)
            .filter(|q| *q > 0)
            .unwrap_or_else(|| {
                context.order(clord_id).map_or(last_shares, |o| o.filled as i64 + last_shares)
            });
        let order_avg_px = parsed.get(&6)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(last_px);
        let commission = parsed.get(&12).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);

        if ord_status == "8" {
            // The venue says why it refused an order, and that was written to a
            // log where no caller could read it. A caller then saw only that
            // the order was not working, with nothing to act on, which is the
            // one thing the venue's own client never does.
            let reason = stated_reason(parsed);
            log::warn!("ExecReport REJECTED: clord={clord_id} reason='{reason}'");
            if !reason.is_empty() {
                shared.orders.push_order_inactive(clord_id, ORDER_INACTIVE_ERROR_CODE, reason);
            }
        } else {
            log::info!("ExecReport: 39={} 150={} 11={} 58={} 103={}",
                ord_status, exec_type, clord_id,
                parsed.get(&58).map(|s| s.as_str()).unwrap_or(""),
                parsed.get(&103).map(|s| s.as_str()).unwrap_or(""));
        }

        let status = status_of(ord_status, clord_id, parsed);


        // A replace is acknowledged as 39=5, and the gateway reaches it through
        // 39=6 first: captured live, a modify runs PendingCancel then Replaced.
        // The monotonic guard ranks PendingCancel above the working states, so
        // that acknowledgement reads as a stale frame and is dropped, leaving a
        // successfully modified order reported as stuck mid-cancel and never
        // confirmed. It is a deliberate transition, which is what the forced
        // path is for; the ranks are left alone because a partially filled
        // order must still be able to reach PendingCancel.
        // A report can carry the reason it restates the order, and two of those
        // reasons are refusals: the gateway answers a revision it would not make
        // and a cancel it would not make on the same message it answers a
        // successful one. Read as an acknowledgement, a refused revision left
        // the caller believing an order had been changed that had not been.
        let restatement_reason = parsed.get(&378).map(|s| s.as_str()).unwrap_or("");
        let revision_refused = matches!(restatement_reason, "102" | "103");
        let is_replace_ack = ord_status == "5" && !revision_refused;
        if revision_refused {
            // The order stands as it was, so it has no new status to report —
            // but the caller asked for a change and has to learn it did not
            // happen. Reported the way a refused order is, on the channel a
            // caller already watches, rather than only to a log.
            let reason = stated_reason(parsed);
            log::warn!(
                "Order {clord_id}: the gateway refused the request (378={restatement_reason}) — \
                 the order stands as it was: {reason}",
            );
            let told = if reason.is_empty() {
                "the venue refused the change and the order stands as it was".to_string()
            } else {
                reason
            };
            shared.orders.push_order_inactive(clord_id, ORDER_INACTIVE_ERROR_CODE, told);
        }
        if is_replace_ack {
            context.set_order_status_forced(clord_id, status);
        }

        // The guard's verdict doubles as the change flag: a stale
        // frame the guard rejects must not surface as an order_status either.
        // A refusal states no new status for the order — it says the request
        // was not carried out, and the order goes on under the terms it already
        // had. Any execution the report carries is still read below.
        let status_changed = !revision_refused
            && (is_replace_ack || context.update_order_status(clord_id, status));

        // The gateway marks a report that restates history: 97=Y is PossResend
        // and 43=Y is PossDupFlag. Neither was read anywhere, and the only
        // thing standing between a replayed execution and a second booking was
        // the ExecID window — which a fresh process does not have, because it
        // has never seen the ID. At session start the gateway replays
        // recent executions, so a restart with open partially-filled orders
        // emitted a fill for something that happened before it started.
        let is_resend = ["Y", "y"].contains(&parsed.get(&97).map(|v| v.as_str()).unwrap_or(""))
            || ["Y", "y"].contains(&parsed.get(&43).map(|v| v.as_str()).unwrap_or(""));
        // A report can also undo or restate an execution rather than announce a
        // new one: a busted trade and a corrected one both arrive as executions,
        // and adding their quantity booked a fill the account no longer has.
        // The cumulative figure is the truth on those, which is the same
        // arithmetic a replayed execution needs.
        let trans_type = parsed.get(&20).map(|s| s.as_str()).unwrap_or("");
        let is_resend = is_resend || matches!(trans_type, "1" | "2");

        // CumQty — the order's cumulative filled quantity as of this report.
        let report_cum_qty = parsed.get(&14)
            .and_then(|s| s.parse::<f64>().ok())
            .map(|c| c as i64)
            .unwrap_or(0);

        // Dedup key. An execution with no ExecID skipped the window entirely,
        // so a replayed copy booked a second time — and an absent tag 17 is the
        // shape a replay takes, which is precisely when the window matters
 //. Falling back to the fields that identify an execution
        // dedups it on its content instead of trusting it.
        //
        // CumQty is what separates two otherwise identical slices: it advances
        // with every execution on the order, including across a replacement
        // that raised the total, where LastShares, price, LeavesQty and the
        // timestamp tick can all repeat.
        let dedup_key = if exec_id.is_empty() {
            format!(
                "{}|{}|{}|{}|{}",
                clord_id,
                parsed.get(&60).map(|s| s.as_str()).unwrap_or(""),
                last_shares,
                last_px,
                report_cum_qty,
            )
        } else {
            exec_id.to_string()
        };

        if matches!(exec_type, "F" | "1" | "2") && last_shares > 0 {
            self.book_fill(
                parsed, clord_id, &dedup_key, is_resend, last_px,
                last_shares, report_cum_qty, commission, leaves_qty, order_cum_qty,
                order_avg_px, context, shared, event_tx,
            );
        }

        // A report that fills an order states its new status on the same
        // report, and suppressing the status because the fill was on it meant
        // the one transition that matters most was the one never announced: a
        // caller watching order status was told about the execution and left
        // believing the order was still working. The two are different
        // questions — what traded, and where the order stands — and a report
        // that answers both is not a reason to drop one.
        if status_changed
            && let Some(order) = context.order(clord_id).copied() {
                let perm_id: i64 = parsed.get(&37).map(|s| perm_id_from_fix_order_id(s)).unwrap_or(0);
                // Tag 583 is the link id this engine sends the OCA group on, not
                // a parent order. Hashing it produced a stable non-zero value
                // shared by every order in a group, none of which has a parent,
                // and nothing distinguished it from a real link.
                //
                // 6107 is not the way to recover one either, though an order
                // *sends* its parent there: the tag is message-scoped, and the
                // vendor's own audit renderer names the inbound one
                // ParentClientId. That is what the shared non-zero value above
                // was — one client id echoed to every order in the account.
                // Reading it back as a parent gives each of them a parent that
                // does not exist. Nothing on this report carries a parent order
                // id, so report none.
                let parent_id: i64 = 0;
                let update = crate::types::OrderUpdate {
                    order_id: clord_id,
                    instrument: order.instrument,
                    status,
                    filled_qty: order.filled as f64,
                    remaining_qty: leaves_qty as f64,
                    avg_price: (order_avg_px * PRICE_SCALE as f64) as Price,
                    perm_id,
                    parent_id,
                    timestamp_ns: context.now_ns(),
                };
                shared.orders.push_order_update(update);
                emit(event_tx, Event::OrderUpdate(update));

                // A parked (39=I) order carries its reason on the same tags
                // 58/103 as a reject, but OrderState.completedStatus stays
                // empty for Inactive — it is not completed and may
                // reactivate, so there is no snapshot field to carry the
                // reason on. Route it through the same error() path a
                // cancel/modify reject already uses instead.
                if status == crate::types::OrderStatus::Inactive {
                    let reason = stated_reason(parsed);
                    if !reason.is_empty() {
                        shared.orders.push_order_inactive(clord_id, ORDER_INACTIVE_ERROR_CODE, reason);
                    }
                }
            }

        // Enrich order/contract caches block
        {
            let account = parsed.get(&1).cloned().unwrap_or_default();
            let symbol = parsed.get(&55).cloned().unwrap_or_default();
            // Where the order is working. The report states it on 207 when it
            // says so at all, and on 6004 as the destination it was routed to;
            // failing both, this client knows where it sent the order and says
            // that. An empty exchange on a completed order is a contract a
            // caller cannot re-place, and the reference client never returns one.
            let exchange = parsed.get(&207).cloned()
                .filter(|e| !e.is_empty())
                .or_else(|| parsed.get(&6004).cloned().filter(|e| !e.is_empty()))
                .or_else(|| {
                    context.order(clord_id).copied()
                        .map(|o| context.market.order_routing(o.instrument).1)
                        .filter(|e| !e.is_empty())
                })
                .unwrap_or_default();
            let sec_type = parsed.get(&167).cloned().unwrap_or_default();
            let currency = parsed.get(&15).cloned().unwrap_or_default();
            let con_id: i64 = parsed.get(&6008).and_then(|s| s.parse().ok()).unwrap_or(0);
            let local_symbol = parsed.get(&6035).cloned().unwrap_or_default();
            let perm_id: i64 = parsed.get(&37).map(|s| perm_id_from_fix_order_id(s)).unwrap_or(0);
            let total_qty: f64 = parsed.get(&38).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let ord_type_tag = parsed.get(&40).map(|s| s.as_str()).unwrap_or("");
            let limit_price: f64 = parsed.get(&44).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let tif_tag = parsed.get(&59).map(|s| s.as_str()).unwrap_or("");
            let stop_px: f64 = parsed.get(&99).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let outside_rth = parsed.get(&6433).map(|s| s == "1").unwrap_or(false);
            let clearing_intent = parsed.get(&6419).cloned().unwrap_or_default();
            let auto_cancel_date = parsed.get(&6596).cloned().unwrap_or_default();
            let exec_exchange = parsed.get(&30).cloned().unwrap_or_default();
            let transact_time = parsed.get(&60).cloned().unwrap_or_default();
            let avg_px: f64 = parsed.get(&6).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            // Absent is not zero. This value is written into a row that
            // persists, so a later report that omits the tag — a pending
            // cancel, say — would otherwise wipe a real filled quantity back
            // to nothing, which is the symptom this is correcting.
            let cum_qty: Option<f64> = parsed.get(&14).and_then(|s| s.parse().ok());
            let last_liq: i32 = parsed.get(&851).and_then(|s| s.parse().ok()).unwrap_or(0);

            let sec_type_str = match sec_type.as_str() {
                "CS" | "COMMON" => "STK",
                "FUT" => "FUT",
                "OPT" => "OPT",
                "FOR" | "CASH" => "CASH",
                "IND" => "IND",
                "FOP" => "FOP",
                "WAR" => "WAR",
                "BAG" => "BAG",
                "BOND" => "BOND",
                "CMDTY" => "CMDTY",
                "NEWS" => "NEWS",
                "FUND" => "FUND",
                _ => &sec_type,
            };

            let order_type_str = match ord_type_tag {
                "1" => "MKT", "2" => "LMT", "3" => "STP", "4" => "STP LMT",
                "P" => "TRAIL", "5" => "MOC", "B" => "LOC", "J" => "MIT",
                "K" => "MTL", "R" => "REL", _ => ord_type_tag,
            };

            // Unknown maps to empty, which is what `decode_tif` means by it and
            // what makes the fallback below reachable. A catch-all of `DAY`
            // reported a perfectly ordinary value for a code this does not know
            // and for an absent tag alike, so a caller reconciling its own
            // orders saw a plausible answer that disagreed with what it sent
            // and nothing said so.
            //
            // The sibling above passes the raw tag through instead; that works
            // there because an absent tag leaves it empty, while any non-empty
            // TIF code would suppress the fallback that knows the real answer.
            let tif_str = match tif_tag {
                "0" => "DAY", "1" => "GTC", "3" => "IOC", "4" => "FOK",
                "2" => "OPG", "6" => "GTD", "8" => "AUC",
                // Stated but unmapped: reported as stated, like the order-type
                // sibling above. The gateway is authoritative when it says
                // anything, and a code this does not name is still better seen
                // than replaced by an unrelated local value.
                other => other,
            };

            let action = match parsed.get(&54).map(|s| s.as_str()) {
                Some("1") => "BUY",
                Some("2") => "SELL",
                Some("5") => "SSHORT",
                _ => if let Some(order) = context.order(clord_id) {
                    match order.side {
                        Side::Buy => "BUY",
                        Side::Sell => "SELL",
                        Side::ShortSell => "SSHORT",
                    }
                } else { "" },
            };

            let status_str = crate::client_core::order_status_str(status);

            let resolved_con_id = if con_id != 0 {
                con_id
            } else if let Some(order) = context.order(clord_id) {
                context.market.con_id(order.instrument).unwrap_or(0)
            } else {
                0
            };

            let contract = if resolved_con_id != 0 {
                if let Some(mut cached) = shared.reference.get_contract(resolved_con_id) {
                    if !symbol.is_empty() { cached.symbol = symbol.clone(); }
                    if !sec_type_str.is_empty() { cached.sec_type = sec_type_str.to_string(); }
                    if !exchange.is_empty() { cached.exchange = exchange.clone(); }
                    if !currency.is_empty() { cached.currency = currency.clone(); }
                    if !local_symbol.is_empty() { cached.local_symbol = local_symbol.clone(); }
                    cached
                } else {
                    api::Contract {
                        con_id: resolved_con_id,
                        symbol: symbol.clone(),
                        sec_type: sec_type_str.to_string(),
                        exchange: exchange.clone(),
                        currency: currency.clone(),
                        local_symbol: local_symbol.clone(),
                        ..Default::default()
                    }
                }
            } else {
                api::Contract {
                    symbol: symbol.clone(),
                    sec_type: sec_type_str.to_string(),
                    exchange: exchange.clone(),
                    currency: currency.clone(),
                    local_symbol: local_symbol.clone(),
                    ..Default::default()
                }
            };

            let (fb_action, fb_tif, fb_ord_type) = if let Some(ctx_order) = context.order(clord_id) {
                let a = match ctx_order.side {
                    crate::types::Side::Buy => "BUY",
                    crate::types::Side::Sell | crate::types::Side::ShortSell => "SELL",
                };
                let t = decode_tif(ctx_order.tif);
                let o = match ctx_order.ord_type {
                    b'1' => "MKT", b'2' => "LMT", b'3' => "STP", b'4' => "STP LMT",
                    b'P' => "TRAIL", _ => "",
                };
                (a, t, o)
            } else {
                ("", "", "")
            };

            // Derive 3 order-dependent fields from FIX tags
            let oca_type: i32 = match parsed.get(&6209).map(|s| s.as_str()) {
                Some("CancelOnFillWBlock") => 1,
                Some("ReduceOnFillWBlock") => 2,
                Some("ReduceOnFillNonBlock") => 3,
                Some("ReduceOnFillWBlockFromTotal") => 4,
                _ => 3, // default
            };
            let algo_strategy = parsed.get(&847).cloned().unwrap_or_default();
            let use_price_mgmt_algo: i32 = if algo_strategy == "Adaptive" { 1 } else { 0 };
            let trail_stop_price: f64 = parsed.get(&6117)
                .and_then(|s| s.parse().ok())
                .unwrap_or(f64::MAX);

            let order = api::Order {
                order_id: clord_id as i64,
                action: if action.is_empty() { fb_action.to_string() } else { action.to_string() },
                total_quantity: total_qty,
                order_type: if order_type_str.is_empty() { fb_ord_type.to_string() } else { order_type_str.to_string() },
                lmt_price: limit_price,
                aux_price: stop_px,
                tif: if tif_str.is_empty() { fb_tif.to_string() } else { tif_str.to_string() },
                account: if account.is_empty() { account_id.to_string() } else { account.clone() },
                perm_id,
                // Tag 14 (CumQty), not tag 151 (LeavesQty). The two are
                // complements, so reporting the remainder as the filled amount
                // makes a completed order read as entirely unfilled.
                filled_quantity: cum_qty.unwrap_or_else(|| {
                    shared.orders.get_order_info(clord_id)
                        .map_or(0.0, |info| info.order.filled_quantity)
                }),
                outside_rth,
                clearing_intent,
                auto_cancel_date,
                submitter: account_id.to_string(),
                oca_type,
                use_price_mgmt_algo,
                trail_stop_price,
                algo_strategy,
                // The report restates the order, and a caller asking what its
                // orders are is answered from it. Everything below arrived on
                // every report and was read from none of them, so an order came
                // back naming neither the reference the caller gave it, nor the
                // client that placed it, nor how it allocates.
                order_ref: parsed.get(&6010).cloned().unwrap_or_default(),
                rule80a: parsed.get(&47).cloned().unwrap_or_default(),
                good_till_date: parsed.get(&432).cloned().unwrap_or_default(),
                client_id: parsed.get(&109).and_then(|s| s.parse().ok()).unwrap_or(0),
                // How an advisor's order is divided, which is the whole of what
                // an advisor's order is.
                fa_group: parsed.get(&6160).cloned().unwrap_or_default(),
                fa_method: parsed.get(&6159).cloned().unwrap_or_default(),
                fa_percentage: parsed.get(&6164).cloned().unwrap_or_default(),
                ..Default::default()
            };

            let completed_time = if matches!(status,
                crate::types::OrderStatus::Filled |
                crate::types::OrderStatus::Cancelled |
                crate::types::OrderStatus::Rejected
            ) {
                parsed.get(&52).cloned().unwrap_or_default()
            } else {
                String::new()
            };
            let completed_status = match status {
                crate::types::OrderStatus::Filled => "Filled".to_string(),
                crate::types::OrderStatus::Cancelled => "Cancelled".to_string(),
                crate::types::OrderStatus::Rejected => {
                    parsed.get(&58).cloned().unwrap_or_else(|| "Rejected".to_string())
                }
                _ => String::new(),
            };

            let order_state = api::OrderState {
                status: status_str.to_string(),
                commission_and_fees: commission,
                completed_time,
                completed_status,
                // `completed_status` is the reject text alone, which is what
                // ibapi's field means. The reason code lives here, where a
                // caller telling a venue's refusal from a bad request can
                // reach it.
                reject_reason: if status == crate::types::OrderStatus::Rejected {
                    stated_reason(parsed)
                } else {
                    String::new()
                },
                ..Default::default()
            };

            let last_exec = api::Execution {
                // What the report stated that nothing above names. A report
                // carries far more than any one client reads, and what is not
                // read is kept rather than dropped.
                unnamed_fields: unnamed_execution_fields(raw),
                exec_id: exec_id.to_string(),
                time: transact_time,
                acct_number: account,
                exchange: exec_exchange,
                side: if let Some(o) = context.order(clord_id) {
                    match o.side { Side::Buy => "BOT", Side::Sell | Side::ShortSell => "SLD" }.to_string()
                } else { String::new() },
                shares: last_shares as f64,
                price: last_px,
                order_id: clord_id as i64,
                // The execution record describes this report, so an absent
                // cumulative is zero here rather than the cached total.
                cum_qty: cum_qty.unwrap_or(0.0),
                avg_price: avg_px,
                last_liquidity: last_liq,
                // Not a field of its own: the broker says it liquidated the
                // position by naming the order with a leading L rather than by
                // setting anything. Read as a flag it was never set at all, and
                // a caller could not tell a liquidation from any other fill.
                liquidation: i32::from(parsed.get(&11).is_some_and(|s| s.starts_with('L'))),
                // What the instrument's economic value is reckoned by, where it
                // has one, and what the reckoning is multiplied by.
                ev_rule: parsed.get(&6858).cloned().unwrap_or_default(),
                // The multiplier is the tag beside the rule, and the venue
                // states it as a number. It was read off 6892, which the venue
                // states as text — so it parsed to nothing and every fill
                // carried a multiplier of zero. A contract whose value follows
                // something other than its own price is then valued at nothing.
                ev_multiplier: parsed
                    .get(&crate::control::contracts::TAG_EV_MULTIPLIER)
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0.0),
                // The price on this report may yet be revised.
                pending_price_revision: parsed.get(&8497)
                    .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
                ..Default::default()
            };

            if con_id != 0 {
                // An execution report states a subset of a definition: it names
                // the contract, not its long name, its trading class or the
                // venues it may trade on. Caching it whole replaced a definition
                // already fetched with a poorer one, leaving a later reader a
                // contract missing fields. Fill, do not replace.
                let merged = match shared.reference.get_contract(con_id) {
                    Some(mut known) => {
                        if !contract.symbol.is_empty() { known.symbol = contract.symbol.clone(); }
                        if !contract.sec_type.is_empty() { known.sec_type = contract.sec_type.clone(); }
                        if !contract.exchange.is_empty() { known.exchange = contract.exchange.clone(); }
                        if !contract.currency.is_empty() { known.currency = contract.currency.clone(); }
                        if !contract.local_symbol.is_empty() {
                            known.local_symbol = contract.local_symbol.clone();
                        }
                        known
                    }
                    None => contract.clone(),
                };
                shared.reference.cache_contract(con_id, merged);
            }

            // A trade cancel (150=H) or trade correction (150=G) restates an
            // execution the gateway has already reported, so it may legitimately
            // return a completed order to a working quantity. Every other report
            // that would do that is a replay.
            let info = RichOrderInfo { contract, order, order_state, last_exec };
            if matches!(exec_type, "G" | "H") {
                shared.orders.push_order_correction(clord_id, info);
            } else {
                // A late duplicate of an earlier partial must not rewrite a
                // completed order back to open. The cache is what
                // `req_open_orders` reads, so a caller polling between the two
                // frames would see a finished order listed as working.
                let already_terminal = shared.orders.get_order_info(clord_id).is_some_and(|prev| {
                    matches!(prev.order_state.status.as_str(), "Filled" | "Cancelled" | "Inactive")
                });
                if !already_terminal || matches!(
                    status,
                    crate::types::OrderStatus::Filled
                        | crate::types::OrderStatus::Cancelled
                        | crate::types::OrderStatus::Rejected
                ) {
                    shared.orders.push_order_info(clord_id, info);
                }
            }
        }

        if matches!(status,
            crate::types::OrderStatus::Filled |
            crate::types::OrderStatus::Cancelled |
            crate::types::OrderStatus::Rejected
        ) {
            // Recorded whether or not the order was being tracked. A market
            // order can finish before its acknowledgement has been handled, so
            // requiring a tracked record meant the fastest orders — the ones
            // that fill immediately — left no memory of having finished, and
            // the working status the gateway echoes behind the fill had nothing
            // to be refused by.
            let tracked = context.order(clord_id).copied();
            shared.orders.push_completed_order(CompletedOrder {
                order_id: clord_id,
                instrument: tracked.map_or(0, |o| o.instrument),
                status,
                filled_qty: tracked.map_or(0, |o| o.filled as i64),
                timestamp_ns: context.now_ns(),
            });
            context.retire_order(clord_id);
        }
    }

    pub(crate) fn handle_cancel_reject(
        &mut self,
        parsed: &std::collections::HashMap<u32, String>,
        context: &mut Context,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
    ) {
        // Match handle_exec_report's tag-11 parsing: strip the gateway's
        // "C" prefix and any ".0/.1/.2" modify-chain suffix.
        let orig_clord = parsed.get(&41).and_then(|s| {
            let stripped = s.strip_prefix('C').unwrap_or(s);
            let base = stripped.split('.').next().unwrap_or(stripped);
            base.parse::<u64>().ok()
        });
        let reason = parsed.get(&58).map(|s| s.as_str()).unwrap_or("Cancel rejected");
        let reject_type: u8 = parsed.get(&434).and_then(|s| s.parse().ok()).unwrap_or(1);
        let reason_code: i32 = parsed.get(&102).and_then(|s| s.parse().ok()).unwrap_or(-1);
        log::warn!("CancelReject: origClOrd={orig_clord:?} type={reject_type} code={reason_code} reason={reason}");

        let Some(oid) = orig_clord else { return };

        // FIX CxlRejReason 1 = UnknownOrder: the gateway is stating that the
        // order does not exist on its side. Restoring it to working asserted
        // the opposite of the message being handled, and the engine's own view
        // governs subsequent cancels, modifies and reconnect bookkeeping — so a
        // phantom order persisted there while the cache row that would have
        // surfaced it was removed.
        //
        // Read as a positive statement, not as an absence: a missing or
        // unparseable tag 102 is synthesized as -1 here and says nothing, so it
        // takes the same path as the reasons that do mean the order is working.
        let unknown_order = reason_code == 1;

        // Update local context only for an order tracked in this session.
        let instrument = if let Some(order) = context.order(oid).copied() {
            if unknown_order {
                // Terminal and removed, which is what the gateway just said.
                // Holding the record in a non-working status instead is not an
                // option here: those are excluded from the open-order count
                // that guards instrument reclamation, so the slot could be
                // handed to another contract while a retained order still
                // pointed at it, and a late fill would move the wrong position.
                //
                // A fill that races the rejection is not lost with the order:
                // the untracked-fill path books it and moves the position
 //.
                context.set_order_status_forced(oid, crate::types::OrderStatus::Cancelled);
                context.retire_order(oid);
            } else {
                let restore_status = if order.filled > 0 {
                    crate::types::OrderStatus::PartiallyFilled
                } else {
                    crate::types::OrderStatus::Submitted
                };
                // Deliberate regression (PendingCancel back to working) — the
                // guard would rightly block it on the ordinary path.
                context.set_order_status_forced(oid, restore_status);
            }
            order.instrument
        } else {
            0
        };

        // Drop the stale cache entry so subsequent req_open_orders stops
        // returning it. Other reasons leave the cache alone; a follow-up exec
        // report will reconcile.
        //
        // No synthetic status update is queued alongside it. The cancel-reject
        // below is the report, and both dispatchers drain fills ahead of order
        // updates — so an update queued here would reach a caller after the
        // fill that raced it, stating the order was gone when it had just been
        // told the order filled.
        if unknown_order {
            shared.orders.remove_order_info(oid);
        }

        let reject = crate::types::CancelReject {
            order_id: oid,
            instrument,
            reject_type,
            reason_code,
            timestamp_ns: context.now_ns(),
        };
        shared.orders.push_cancel_reject(reject);
        emit(event_tx, Event::CancelReject(reject));
    }
}
