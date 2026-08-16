pub mod farm;
pub mod ccp;
pub mod hmds;
pub mod secdef;
pub mod order_builder;
pub(crate) mod retry;

/// How fast a reconnect may put its subscriptions back.
///
/// Taken from the caller's [`ReconnectConfig`](crate::api::reliability::ReconnectConfig).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReplayPacing {
    pub burst: usize,
    pub pace: std::time::Duration,
}

impl Default for ReplayPacing {
    fn default() -> Self {
        let d = crate::api::reliability::ReconnectConfig::default();
        Self { burst: d.replay_burst, pace: d.replay_pace }
    }
}

use std::sync::Arc;
use std::time::Instant;
use std::io;

use crate::bridge::{Event, SharedState};
use crate::engine::context::Context;
use crate::config::chrono_free_timestamp;
use crate::gateway::{connect_farm, reconnect_ccp, Farm, ReconnectAuth};
use crate::protocol::connection::Connection;
use crate::protocol::fix;
use crate::types::{ContractRef, ControlCommand, Fill, InstrumentId, Price, Qty, TbtQuote, TbtTrade, PRICE_SCALE, QTY_SCALE};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

use farm::FarmState;
use ccp::CcpState;
use hmds::HmdsState;

/// Auth server heartbeat interval — single source in config (
/// removed the duplicate definitions here).
const CCP_HEARTBEAT_SECS: u64 = crate::config::CCP_HEARTBEAT;
/// Farm heartbeat interval — single source in config.
const FARM_HEARTBEAT_SECS: u64 = crate::config::FARM_HEARTBEAT;
/// Liveness, aligned with the gateway's transport thresholds:
/// send a test request when nothing has been received for this long..
pub const LIVENESS_TEST_SECS: u64 = 15;
/// ...and declare the connection dead when nothing has been received for
/// this long. The old scheme declared death at ~21s — racing the server's
/// own ~35s reset and losing to transient stalls the server tolerates.
pub const LIVENESS_DEAD_SECS: u64 = 35;
/// Grace window after (re)connect before liveness is enforced:
/// early-connection jitter must not trigger a false disconnect during a
/// period the server itself treats as warm-up. Heartbeats are still sent.
pub const LIVENESS_WARMUP_SECS: u64 = 60;
// The ladder is ordered by construction, so it is checked by construction: a
// heartbeat that outlives the test window, or a test that outlives the dead
// window, fails the build rather than a test the optimizer folds away.
const _: () = assert!(CCP_HEARTBEAT_SECS < LIVENESS_TEST_SECS);
const _: () = assert!(LIVENESS_TEST_SECS < LIVENESS_DEAD_SECS);

/// The pinned-core hot loop. Pushes events to SharedState + optional event channel.
pub struct HotLoop {
    shared: Arc<SharedState>,
    event_tx: Option<SyncSender<Event>>,
    context: Context,
    /// Core ID to pin the hot loop thread to. None = no pinning.
    core_id: Option<usize>,
    /// Whether a recoverable loss was announced to the client. Gates the
    /// restore notice so a reconnect that nobody was told about stays quiet.
    loss_announced: bool,
    /// Set when a reconnect failed for a reason repeating cannot fix. The
    /// scheduler stops rather than climbing a ladder forever against a server
    /// that has already given its answer.
    reconnect_halted: Option<retry::DisconnectReason>,
    /// What the caller said about recovery. Defaults recover automatically and
    /// keep trying, which is what a process that must stay up wants.
    reconnect_cfg: crate::api::reliability::ReconnectConfig,
    budget: crate::api::reliability::RecoveryBudget,
    /// Slots a caller asked to free that were held open by a position. The
    /// table is bounded, so they are reconsidered once the account is flat
    /// rather than being lost until the process ends.
    pinned_by_position: Vec<InstrumentId>,
    /// Next scheduled CCP/farm reconnect attempt (jittered backoff).
    ccp_next_attempt_at: Option<Instant>,
    farm_next_attempt_at: Option<Instant>,
    /// Farm connection for market data (market data farm).
    pub farm_conn: Option<Connection>,
    /// Auth connection for order management.
    pub ccp_conn: Option<Connection>,
    /// Historical farm connection for historical data (optional).
    pub hmds_conn: Option<Connection>,
    /// Contract definitions and the calendar that rides with them. Absent
    /// where the venue stated no route for it at logon, which is a fact about
    /// the session rather than a failure.
    pub secdef_conn: Option<Connection>,
    pub secdef: secdef::SecDefState,
    /// SPSC channel receiver for control plane commands.
    control_rx: Option<Receiver<ControlCommand>>,
    /// Books asked for on no particular venue, held until the server has said
    /// which venues offer one.
    depth_awaiting_venues: Vec<ControlCommand>,
    /// Whether the hot loop should keep running.
    running: bool,
    /// Account ID for order submission.
    account_id: String,
    /// Heartbeat state.
    hb: HeartbeatState,
    /// Reusable buffer for control commands (avoids per-iteration allocation).
    cmd_buf: Vec<ControlCommand>,
    // ── Subsystems ──
    pub(crate) farm: FarmState,
    pub(crate) ccp: CcpState,
    pub(crate) hmds: HmdsState,
    // ── Auto-reconnect ──
    reconnect_auth: Option<ReconnectAuth>,
    pending_farm_reconnect: Option<Receiver<io::Result<Connection>>>,
    farm_reconnect_attempt: u32,
    pending_ccp_reconnect: Option<Receiver<io::Result<Connection>>>,
    ccp_reconnect_attempt: u32,
    /// HMDS reconnect state. Drives a background reconnect loop with
    /// exponential backoff when the historical-data farm is down — initial
    /// connect failed, or a future runtime disconnect detector trips it.
    pending_hmds_reconnect: Option<Receiver<io::Result<Connection>>>,
    hmds_reconnect_attempt: u32,
    /// The same, for the connection carrying the calendar. Rebuilt on the
    /// same terms as the others: a farm that goes is not a session that ends.
    secdef_reconnect_attempt: u32,
    secdef_next_attempt_at: Option<Instant>,
    pending_secdef_reconnect: Option<Receiver<io::Result<Connection>>>,
    /// Earliest instant the next HMDS reconnect attempt may spawn. `None`
    /// while HMDS is healthy.
    hmds_next_attempt_at: Option<Instant>,
}

/// Consecutive HMDS reconnect failures before the loss is logged as an error
///. Retries do not stop there: the servers go down for maintenance
/// nightly and come back on their own, and a client that gave up after ~2
/// minutes would stay dark until someone restarted the process — the same
/// failure the gateway has when its restart token is unset. Attempts continue
/// on the ladder below, which caps at 64s.
const HMDS_NOTIFY_AFTER_ATTEMPTS: u32 = 6;

/// Tracks last send/recv times and pending test requests for heartbeat management.
pub struct HeartbeatState {
    pub last_ccp_sent: Instant,
    pub last_ccp_recv: Instant,
    pub last_farm_sent: Instant,
    pub last_farm_recv: Instant,
    pub last_hmds_sent: Instant,
    pub last_hmds_recv: Instant,
    pub last_secdef_sent: Instant,
    pub last_secdef_recv: Instant,
    /// How often the venue said it expects to hear from this session, in
    /// seconds, as stated in its answer to the logon.
    ///
    /// Proposed by this client and answered by the venue, which may name a
    /// different number — so the proposal is not what the session is held to.
    /// Sending on the proposal regardless meant a venue asking for anything
    /// shorter was answered too slowly, and closed the connection for it.
    pub ccp_interval_secs: u64,
    /// Pending test request for auth: (test_req_id, sent_at).
    pub pending_ccp_test: Option<(String, Instant)>,
    /// When each connection (re)connected — liveness is not enforced during
    /// the warm-up window that follows.
    pub ccp_up_since: Instant,
    pub farm_up_since: Instant,
    pub hmds_up_since: Instant,
    /// Pending test request for farm: (test_req_id, sent_at).
    pub pending_farm_test: Option<(String, Instant)>,
    /// Pending test request for historical: (test_req_id, sent_at).
    pub pending_hmds_test: Option<(String, Instant)>,
    /// Counter for generating unique test request IDs.
    test_req_counter: u32,
}

/// Half an interval, and never nothing.
///
/// A session that answers on the deadline has no margin: one heartbeat delayed
/// by a slow link or a scheduling hiccup is late. Half leaves room for one to
/// be lost outright, which is what the counterpart allows itself.
fn half_of(interval_secs: u64) -> u64 {
    (interval_secs / 2).max(1)
}

impl HeartbeatState {
    /// How often to send, given what the venue asked for.
    ///
    /// Half the stated interval, which is what the counterpart sends at: a
    /// session that answers exactly on the deadline has no margin, and one
    /// heartbeat delayed by a scheduling hiccup or a slow link is late. Half
    /// leaves room for one to be lost entirely.
    ///
    /// Never zero, so a venue naming one second is answered every second
    /// rather than on every pass of the loop.
    pub fn ccp_send_every(&self) -> u64 {
        half_of(self.ccp_interval_secs)
    }

    /// How long of silence before asking whether the venue is still there.
    ///
    /// The fixed window, or a multiple of the stated interval where that is
    /// longer. A venue that speaks every thirty seconds is not silent at
    /// fifteen, and probing it on that schedule would ask a healthy session to
    /// prove itself on every gap between its own heartbeats.
    pub fn ccp_test_after(&self) -> u64 {
        LIVENESS_TEST_SECS.max(self.ccp_interval_secs.saturating_mul(3) / 2)
    }

    /// How long of silence before the connection is treated as gone.
    ///
    /// Three of the venue's own intervals at least: two heartbeats can be lost
    /// without the session being dead, and declaring it on a shorter window
    /// than the venue's own cadence kills connections that are working.
    pub fn ccp_dead_after(&self) -> u64 {
        LIVENESS_DEAD_SECS.max(self.ccp_interval_secs.saturating_mul(3))
    }

    /// The same two windows for a farm, from the interval that farm stated.
    ///
    /// A farm speaks on its own cadence, and the fixed windows are sized for
    /// the auth connection's. One that heartbeats every sixty seconds is not
    /// silent at thirty-five, so a fixed window declares a working connection
    /// dead between two of its own heartbeats.
    pub fn farm_test_after(stated: Option<u64>) -> u64 {
        let interval = stated.unwrap_or(FARM_HEARTBEAT_SECS);
        LIVENESS_TEST_SECS.max(interval.saturating_mul(3) / 2)
    }

    /// How long of silence before a farm is treated as gone.
    pub fn farm_dead_after(stated: Option<u64>) -> u64 {
        let interval = stated.unwrap_or(FARM_HEARTBEAT_SECS);
        LIVENESS_DEAD_SECS.max(interval.saturating_mul(3))
    }

    /// Hold this session to the interval the venue stated.
    pub fn set_ccp_interval(&mut self, secs: u64) {
        if secs > 0 {
            self.ccp_interval_secs = secs;
        }
    }

    fn new() -> Self {
        let now = Instant::now();
        Self {
            last_ccp_sent: now,
            last_ccp_recv: now,
            last_farm_sent: now,
            last_farm_recv: now,
            last_hmds_sent: now,
            last_secdef_sent: now,
            last_secdef_recv: now,
            last_hmds_recv: now,
            ccp_interval_secs: CCP_HEARTBEAT_SECS,
            pending_ccp_test: None,
            ccp_up_since: Instant::now(),
            farm_up_since: Instant::now(),
            hmds_up_since: Instant::now(),
            pending_farm_test: None,
            pending_hmds_test: None,
            test_req_counter: 0,
        }
    }

    fn next_test_id(&mut self) -> String {
        self.test_req_counter += 1;
        format!("T{}", self.test_req_counter)
    }
}

impl HotLoop {
    pub fn new(shared: Arc<SharedState>, event_tx: Option<SyncSender<Event>>, core_id: Option<usize>) -> Self {
        Self {
            shared,
            event_tx,
            context: Context::new(),
            core_id,
            farm_conn: None,
            ccp_conn: None,
            hmds_conn: None,
            secdef_conn: None,
            secdef: secdef::SecDefState::new(),
            control_rx: None,
            depth_awaiting_venues: Vec::new(),
            running: true,
            account_id: String::new(),
            hb: HeartbeatState::new(),
            cmd_buf: Vec::with_capacity(16),
            farm: FarmState::new(),
            ccp: CcpState::new(),
            hmds: HmdsState::new(),
            reconnect_auth: None,
            pending_farm_reconnect: None,
            ccp_next_attempt_at: None,
            farm_next_attempt_at: None,
            loss_announced: false,
            reconnect_halted: None,
            reconnect_cfg: Default::default(),
            budget: Default::default(),
            pinned_by_position: Vec::new(),
            farm_reconnect_attempt: 0,
            pending_ccp_reconnect: None,
            ccp_reconnect_attempt: 0,
            pending_hmds_reconnect: None,
            hmds_reconnect_attempt: 0,
            secdef_reconnect_attempt: 0,
            secdef_next_attempt_at: None,
            pending_secdef_reconnect: None,
            hmds_next_attempt_at: None,
        }
    }

    /// Set the control channel receiver. The caller keeps the sender.
    pub fn set_control_rx(&mut self, rx: Receiver<ControlCommand>) {
        self.control_rx = Some(rx);
    }

    /// Set the account ID for order submission.
    pub fn set_account_id(&mut self, account_id: String) {
        self.account_id = account_id;
    }

    /// Hold the auth connection to the heartbeat interval the venue stated.
    pub fn set_ccp_heartbeat_interval(&mut self, secs: u64) {
        self.hb.set_ccp_interval(secs);
    }

    /// Access the context (for pre-start configuration like registering instruments).
    pub fn context_mut(&mut self) -> &mut Context {
        &mut self.context
    }

    /// Process pending control commands once. For testing.
    pub fn poll_once(&mut self) {
        self.poll_control_commands();
    }

    /// Whether the hot loop is still running. For testing.
    #[doc(hidden)]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Build a HotLoop with connections and control channel, without requiring a Gateway.
    pub fn with_connections(
        shared: Arc<SharedState>,
        event_tx: Option<SyncSender<Event>>,
        account_id: String,
        farm_conn: Connection,
        ccp_conn: Connection,
        hmds_conn: Option<Connection>,
        core_id: Option<usize>,
    ) -> (Self, SyncSender<ControlCommand>) {
        let (tx, rx) = sync_channel(64);
        let mut hl = Self::new(shared, event_tx, core_id);
        hl.set_control_rx(rx);
        hl.set_account_id(account_id);
        hl.farm_conn = Some(farm_conn);
        hl.ccp_conn = Some(ccp_conn);
        hl.hmds_conn = hmds_conn;
        (hl, tx)
    }

    /// Run the hot loop under `catch_unwind`. On panic, log the payload and
    /// emit `Event::Disconnected` so consumers see the dead engine without
    /// having to wait for the next outbound call to fail. Use this from the
    /// engine-spawn site instead of `run()` directly.
    /// try_register + full-table rejection. On a full table the
    /// reply channel gets an Err — the caller's request fails loudly and the
    /// hot loop keeps running. Previously this was an assert! that killed
    /// the engine for the rest of the process.
    fn register_or_reject(
        &mut self,
        con_id: i64,
        symbol: String,
        sec_type: &str,
        exchange: &str,
        option_key: &str,
        // Answered without blocking: this runs on the thread driving all
        // three transports, and a caller's channel is the caller's to drain.
        // A reply that cannot be delivered is a caller that stopped listening.
        reply_tx: &Option<std::sync::mpsc::SyncSender<Result<InstrumentId, String>>>,
    ) -> Option<InstrumentId> {
        // Whether this call is what created the slot. Registration is also how
        // an already-live contract is looked up, and the account row is older
        // than any fill booked since: reapplying it on every call rolled a
        // filled position back to whatever the last account frame said.
        let is_new_slot = self.context.market.con_id(
            self.context.market.instrument_by_con_id(con_id).unwrap_or(0),
        ) != Some(con_id);
        match self.context.market.try_register_contract(con_id, &symbol, sec_type, exchange, option_key) {
            Some(id) => {
                self.context.market.set_symbol(id, symbol);
                self.context.market.set_routing(id, sec_type, exchange);
                // The account states what it holds before a caller subscribes
                // to anything, and that statement had nowhere to land: with no
                // slot yet, only the conId-keyed row was written. Taking it now
                // is what makes the engine's position table agree with the
                // account from the first callback, and what keeps the slot from
                // being reclaimed as unheld.
                if let Some(held) = self.shared.portfolio.position_info(con_id).filter(|_| is_new_slot)
                    && held.position != 0.0 {
                        self.context.update_position(id, held.position - self.context.position(id));
                        self.shared.portfolio.set_position(id, held.position);
                    }
                self.shared.market.set_instrument_count(self.context.market.count());
                if let Some(tx) = reply_tx { let _ = tx.try_send(Ok(id)); }
                Some(id)
            }
            None => {
                log::error!("Instrument table full: rejecting registration for con_id={con_id}");
                if let Some(tx) = reply_tx {
                    let _ = tx.try_send(Err(format!(
                        "instrument table full: {} contracts are live concurrently; \
                         cancel unused market-data subscriptions to free slots",
                        crate::types::MAX_INSTRUMENTS
                    )));
                }
                None
            }
        }
    }

    /// Reclaim an instrument slot if nothing references it any more
    /// Reclaimed only with nothing referring to the slot: no open orders, no
    /// market data, no tick-by-tick subscription, no news subscription. A
    /// reused id would repoint those references at the wrong contract, so a
    /// referenced slot stays resident until released.
    fn try_reclaim_instrument(&mut self, instrument: InstrumentId) {
        if !self.context.open_orders_for(instrument).is_empty() {
            return;
        }
        // Market data was missing from this list. Cancelling tick-by-tick or
        // news on an instrument that also holds an L1 subscription freed the
        // slot underneath it, and the resubscribe record then replayed the old
        // contract's descriptor against the id's new occupant.
        if self.farm.holds_market_data(instrument) {
            return;
        }
        if self.hmds.tbt_subscriptions.iter().any(|sub| sub.instrument == instrument) {
            return;
        }
        if self.ccp.news_subscriptions.iter().any(|(id, _, _)| *id == instrument) {
            return;
        }
        // A subscription waiting on the lookup that will name its contract is
        // as much a reference as a live one. The pending record carries the
        // slot, so a slot reclaimed while its lookup is out is handed to
        // another contract and then subscribed with the first one's id: the
        // quotes arrive, under the wrong contract, priced on its tick.
        if self.ccp.pending_md_subscribe.iter().any(|(_, p, _)| p.instrument == instrument)
            || self.ccp.resolved_md_subscribe.iter().any(|(_, p)| p.instrument == instrument)
        {
            return;
        }
        // A holding is a reference to the contract as much as a subscription
        // is. Dropping the last subscription on something the account still
        // owns handed the slot to the next contract, which then reported the
        // previous one's position as its own.
        if self.context.position(instrument) != 0.0 {
            if !self.pinned_by_position.contains(&instrument) {
                self.pinned_by_position.push(instrument);
            }
            return;
        }
        self.pinned_by_position.retain(|id| *id != instrument);
        if self.context.market.unregister(instrument).is_some() {
            // Zero the shared-side quote so a reused slot cannot serve the
            // previous contract's prices before its first tick.
            self.shared.market.push_quote(instrument, &crate::types::Quote::default());
            // Tick-by-tick rebuilds bid and ask from deltas against the last
            // pair it saw. Left in place, the next occupant's first delta
            // would be applied to the previous contract's prices.
            log::info!("Reclaimed instrument slot {instrument}");
        }
    }

    pub fn run_with_panic_recovery(mut self) {
        let event_tx = self.event_tx.clone();
        let shared = self.shared.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run();
        }));
        if let Err(payload) = result {
            let msg: &str = payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| payload.downcast_ref::<&'static str>().copied())
                .unwrap_or("<non-string panic payload>");
            log::error!("Engine hot loop panicked, emitting Disconnected: {msg}");
            shared.set_connection_lost();
            emit(&event_tx, Event::Disconnected);
        }
    }

    /// Run the hot loop. Blocks until Shutdown command received.
    pub fn run(&mut self) {
        if let Some(core) = self.core_id {
            Self::pin_to_core(core);
        }

        self.running = true;

        while self.running {
            self.context.loop_iterations += 1;

            // 1. Busy-poll market data farm socket (non-blocking recv)
            let farm_was_ok = !self.farm.disconnected;
            self.farm.poll_market_data(
                &mut self.farm_conn, &mut self.context, &self.shared,
                &self.event_tx, &mut self.hb,
            );
            let _ = farm_was_ok; // reconnects are scheduled below

            // 1b. Busy-poll historical socket for tick-by-tick data
            self.hmds.poll(
                &mut self.hmds_conn, &self.shared,
                &self.event_tx, &mut self.hb,
            );

            // 1c. The security definition farm, which carries the calendar.
            self.secdef.poll(
                &mut self.secdef_conn, &self.shared,
                &self.event_tx, &mut self.hb,
            );

            // 1c. Hand off any scanner results with cache-miss con_ids to CCP for
            // contract-detail fan-out. Mirrors what the gateway does
            // internally for binary-API scanner clients.
            for (req_id, result) in self.hmds.cold_scanner_results.drain(..).collect::<Vec<_>>() {
                self.ccp.start_scanner_enrichment(
                    req_id, result, &mut self.ccp_conn, &self.shared, &mut self.hb,
                );
            }

            // 2. Drain pending orders → build → sign → send to auth
            //    Skip if CCP is disconnected — orders stay in buffer for retry after reconnect.
            order_builder::drain_and_send_orders(
                &mut self.ccp_conn, &mut self.context, &self.account_id, &mut self.hb,
                self.ccp.disconnected, &self.shared,
                self.ccp.recovery_sweep_at.is_some(), &self.event_tx,
            );

            // A write that abandoned the transport leaves it unable to carry
            // anything out while the peer may still be sending, so nothing
            // read-side would ever notice. Without this the liveness deadline
            // never fires, no reconnect is scheduled, and the session sits
            // write-dead while reporting itself connected.
            self.disconnect_write_dead_transports();

            // 3. Busy-poll auth socket for execution reports
            let ccp_was_ok = !self.ccp.disconnected;
            self.ccp.poll_executions(
                &mut self.ccp_conn, &mut self.context, &self.shared,
                &self.event_tx, &mut self.hb, &self.account_id,
            );
            // A subscription the venue can now be asked for: its contract was
            // named by symbol and the lookup has come back with an id.
            if !self.ccp.resolved_md_subscribe.is_empty() {
                for (con_id, p) in std::mem::take(&mut self.ccp.resolved_md_subscribe) {
                    // The slot keeps the id so a reconnect resubscribes by it
                    // rather than starting the lookup again.
                    self.context.market.adopt_con_id(p.instrument, con_id);
                    self.farm.send_mktdata_subscribe(
                        con_id, &p.symbol, &p.exchange, &p.sec_type,
                        &p.last_trade_date, p.strike, &p.right, &p.multiplier,
                        p.instrument, p.mode_9887,
                        &mut self.farm_conn,
                        &mut self.hb,
                    );
                }
            }

            // A holding that has since been closed releases the slot the
            // caller already asked to free.
            if !self.pinned_by_position.is_empty() {
                for instrument in std::mem::take(&mut self.pinned_by_position) {
                    if self.context.position(instrument) == 0.0 {
                        // Reclaiming may still be refused for a reason that has
                        // nothing to do with the position — a working order, a
                        // subscription taken out since. `try_reclaim_instrument`
                        // records it again where that reason is the position,
                        // and this keeps it where the reason is anything else,
                        // so the request to free the slot is not lost either way.
                        self.try_reclaim_instrument(instrument);
                        if self.context.market.con_id(instrument).is_some()
                            && !self.pinned_by_position.contains(&instrument)
                        {
                            self.pinned_by_position.push(instrument);
                        }
                    } else {
                        self.pinned_by_position.push(instrument);
                    }
                }
            }
            self.ccp.sweep_recovery(&mut self.context, &self.shared, &self.event_tx);
            self.ccp.sweep_pending_matching_symbols();
            self.ccp.sweep_pending_option_params(&self.shared);
            self.ccp.sweep_pending_schedule_pairs(&self.shared, &self.event_tx);
            self.ccp.sweep_scanner_enrichments(&self.shared);
            self.ccp.sweep_contract_details(&self.shared, &self.event_tx);
            self.ccp.sweep_pending_subscribes(&self.shared);
            self.ccp.sweep_pending_named(&self.shared);
            self.hmds.sweep_pending_historical(&self.shared);
            let _ = ccp_was_ok; // reconnects are scheduled below

            // 4. Check control_plane_rx (SPSC) for commands
            self.poll_control_commands();

            // 5. Heartbeat check (auth 10s, farm 30s)
            self.check_heartbeats();

            // 5b. Poll pending reconnects and schedule the next attempts
            // (jittered backoff instead of immediate re-dials)
            self.poll_farm_reconnect();
            // What a reconnect has still to put back. Paced, and paced on the
            // passes the loop is already making rather than by holding it.
            self.drive_replay();
            self.poll_ccp_reconnect();
            self.poll_hmds_reconnect();
            self.poll_secdef_reconnect();
            self.budget.settle(Instant::now(), self.reconnect_cfg.stable_window);
            self.maybe_spawn_farm_reconnect();
            self.maybe_spawn_ccp_reconnect();
            self.maybe_spawn_hmds_reconnect();
            self.maybe_spawn_secdef_reconnect();

            // 6. Wake any waiting consumers (e.g. Python event loop)
            self.shared.notify();

            // 7. With every transport down there is no socket to poll and no
            //    socket to be quick for, so the spin above buys nothing and
            //    costs a core — on a laptop that is the battery and the fans,
            // which is how this gets noticed. Parking only in that
            //    state leaves the connected path exactly as it was; a reconnect
            //    is scheduled on a backoff measured in seconds, so a millisecond
            //    here delays nothing, including shutdown.
            if self.farm.disconnected && self.ccp.disconnected && self.hmds.disconnected {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    fn emit_hmds_unavailable(&self, req_id: u32, from_historical: bool) {
        push_hmds_unavailable(&self.shared, req_id, from_historical);
    }

    fn poll_control_commands(&mut self) {
        let rx = match self.control_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };

        self.cmd_buf.clear();
        self.cmd_buf.extend(rx.try_iter());

        // try_iter() stops on both Empty and Disconnected — do one extra
        // try_recv() to distinguish.  If a straggler command arrived between
        // try_iter() finishing and this call, push it into the batch.
        let sender_dropped = match rx.try_recv() {
            Ok(cmd)  => { self.cmd_buf.push(cmd); false }
            Err(std::sync::mpsc::TryRecvError::Empty)        => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => true,
        };

        // Drain the buffer first, so the loop body can mutably borrow self.
        // Requests held for want of a contract id come first: they were asked
        // for before anything still in the buffer.
        // A book held for want of the list of venues that offer one, now that
        // the server has named them.
        if !self.depth_awaiting_venues.is_empty()
            && !self.shared.reference.depth_exchanges().is_empty()
        {
            let held = std::mem::take(&mut self.depth_awaiting_venues);
            self.cmd_buf.extend(held);
        }
        let mut cmds: Vec<ControlCommand> = std::mem::take(&mut self.ccp.resolved_named);
        cmds.append(&mut self.cmd_buf);
        for cmd in cmds {
            // A caller who passed the contract it wrote down rather than the
            // venue's id for it gets the lookup made on its behalf, and the
            // request arrives here again once the venue has named it.
            let Some(cmd) = self.ccp.hold_until_named(cmd, &mut self.ccp_conn, &mut self.hb)
            else {
                continue;
            };
            match cmd {
                ControlCommand::Subscribe { contract, mode_9887, reply_tx } => {
                    let ContractRef { con_id, symbol, exchange, sec_type, currency, last_trade_date, strike, right, multiplier } = contract;
                    // What tells two conId-less contracts on one underlying apart.
                    // Built by the same function an order uses, or the two
                    // describe one contract differently: the slot a
                    // subscription took would not be found again by an order,
                    // which would take a second one — with no quote on it, and
                    // stating the wrong currency because the slot it did take
                    // never recorded one.
                    let option_key = crate::client_core::ClientCore::contract_identity(
                        &last_trade_date, strike, &right, &multiplier, &currency,
                    );
                    // Registered without answering yet: a contract with no conId
                    // has no client-side identity, so whether this is a duplicate
                    // can only be settled here, against the slot the engine just
                    // resolved. Refusing after the subscribe had already gone out
                    // left the caller told it failed while a live subscription
                    // bound the second contract's tag and minTick onto the first,
                    // with no id to cancel it by.
                    match self.register_or_reject(con_id, symbol.clone(), &sec_type, &exchange, &option_key, &None) {
                        None => {
                            if let Some(tx) = &reply_tx {
                                let _ = tx.try_send(Err(format!(
                                    "instrument table full: cannot subscribe to {symbol}"
                                )));
                            }
                        }
                        // Already subscribed, so nothing goes to the venue
                        // again: one contract holds one subscription on the
                        // wire, and the caller watches the one that is up, so
                        // two parts of one program may watch one contract.
                        Some(id) if self.farm.holds_market_data(id) => {
                            if let Some(tx) = &reply_tx {
                                let _ = tx.try_send(Ok(id));
                            }
                        }
                        Some(id) => {
                            if let Some(tx) = &reply_tx {
                                let _ = tx.try_send(Ok(id));
                            }
                            if con_id == 0 {
                                // The venue answers a subscription only when it
                                // is named by contract id, and says nothing at
                                // all — no tick and no refusal — to one named
                                // by symbol. Ask it to name the contract first.
                                self.ccp.resolve_for_subscribe(
                                    crate::engine::hot_loop::ccp::PendingSubscribe {
                                        instrument: id,
                                        symbol: symbol.clone(),
                                        exchange: exchange.clone(),
                                        sec_type: sec_type.clone(),
                                        currency: currency.clone(),
                                        last_trade_date: last_trade_date.clone(),
                                        strike,
                                        right: right.clone(),
                                        multiplier: multiplier.clone(),
                                        mode_9887,
                                    },
                                    &mut self.ccp_conn,
                                    &mut self.hb,
                                );
                            } else {
                                self.farm.send_mktdata_subscribe(
                                    con_id, &symbol, &exchange, &sec_type,
                                    &last_trade_date, strike, &right, &multiplier,
                                    id, mode_9887,
                                    &mut self.farm_conn,
                                    &mut self.hb,
                                );
                            }
                        }
                    }
                }
                ControlCommand::Unsubscribe { instrument } => {
                    self.farm.send_mktdata_unsubscribe(
                        instrument,
                        &mut self.farm_conn,
                        &mut self.hb,
                    );
                    // The tags are dead with the requests that earned them, and
                    // `try_reclaim_instrument` below only drops them when the
                    // slot itself goes — so a pinned instrument accumulated one
                    // per ack until the next farm drop. News is the
                    // one reader that outlives the L1 request: ticker setup
                    // registers into the same map and news routes on it, so a
                    // live news subscription keeps them.
                    if !self.ccp.news_subscriptions.iter().any(|(id, _, _)| *id == instrument) {
                        self.context.market.clear_server_tags_for(instrument);
                    }
                    self.try_reclaim_instrument(instrument);
                }
                ControlCommand::SubscribeTbt { contract, req_id, tbt_type, reply_tx } => {
                    let ContractRef { con_id, symbol, sec_type, exchange, .. } = contract;
                    // A stream is asked for by the venue's id for the contract.
                    // Sent with none, the venue answers "Unknown contract"
                    // against a query nothing here has told the caller about,
                    // and the caller waits on a stream that was refused before
                    // it began. Told here instead — the surfaces resolve a
                    // description before it reaches this point, and this is
                    // what catches the one that does not.
                    if con_id == 0 {
                        let reason = format!(
                            "a {sec_type} trade stream on {symbol} was asked for without the \
                             venue's id for the contract, which is what a stream is asked for by",
                        );
                        log::error!("{reason}");
                        push_hmds_error(&self.shared, req_id.max(0) as u32, reason.clone(), false);
                        if let Some(tx) = reply_tx.as_ref() {
                            let _ = tx.try_send(Err(reason));
                        }
                        continue;
                    }
                    // Registered with what the contract is, so the slot carries
                    // it and the subscription can state it.
                    if let Some(id) = self.register_or_reject(con_id, symbol, &sec_type, &exchange, "", &reply_tx) {
                        let mts = self.context.market.min_tick_scaled(id);
                        self.hmds.send_tbt_subscribe(
                            req_id, con_id, id, tbt_type, &sec_type, &exchange, mts,
                            &mut self.hmds_conn, &mut self.hb,
                        );
                    }
                }
                ControlCommand::UnsubscribeTbt { req_id, instrument } => {
                    self.hmds.send_tbt_unsubscribe(req_id, instrument, &mut self.hmds_conn, &mut self.hb);
                    self.try_reclaim_instrument(instrument);
                }
                ControlCommand::SubscribeNews { con_id, symbol, providers, reply_tx } => {
                    if let Some(id) = self.register_or_reject(con_id, symbol, "", "", "", &reply_tx) {
                        // Allocate req_id from farm's counter (shared ID space)
                        let req_id = self.farm.next_md_req_id;
                        self.farm.next_md_req_id += 1;
                        self.ccp.send_news_subscribe(con_id, id, &providers, req_id, &mut self.ccp_conn, &mut self.hb);
                    }
                }
                ControlCommand::UnsubscribeNews { instrument } => {
                    self.ccp.send_news_unsubscribe(instrument, &mut self.ccp_conn, &mut self.hb);
                    self.try_reclaim_instrument(instrument);
                }
                ControlCommand::UpdateParam { key, value } => {
                    let _ = (key, value);
                }
                ControlCommand::Ping => {
                    // On-demand RTT sample. Reuses the liveness
                    // test-request machinery; a pending liveness test is
                    // already a measurement in flight, so don't stomp it.
                    if self.hb.pending_ccp_test.is_none()
                        && let Some(conn) = self.ccp_conn.as_mut() {
                            let ts = chrono_free_timestamp();
                            let test_id = self.hb.next_test_id();
                            let _ = conn.send_fix(&[
                                (fix::TAG_MSG_TYPE, fix::MSG_TEST_REQUEST),
                                (fix::TAG_SENDING_TIME, &ts),
                                (fix::TAG_TEST_REQ_ID, &test_id),
                            ]);
                            self.hb.pending_ccp_test = Some((test_id, Instant::now()));
                            self.hb.last_ccp_sent = Instant::now();
                        }
                }
                ControlCommand::Order(req) => {
                    self.context.pending_orders.push(req);
                }
                ControlCommand::RegisterInstrument { contract, identity, reply_tx } => {
                    let ContractRef { con_id, symbol, sec_type, exchange, .. } = contract;
                    self.register_or_reject(con_id, symbol, &sec_type, &exchange, &identity, &reply_tx);
                }
                ControlCommand::FetchHistorical { contract, req_id, end_date_time, duration, bar_size, what_to_show, use_rth, keep_up_to_date, .. } => {
                    let ContractRef { con_id, symbol, sec_type, exchange, .. } = contract;
                    // keepUpToDate sends via CCP but bars/end arrive on HMDS — both
                    // paths require an authed HMDS socket to deliver a completion.
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, true);
                    } else if keep_up_to_date {
                        // The bars so far, then the stream that keeps them
                        // current. The venue answers a request to keep bars up
                        // to date with the bars and closes the query, on either
                        // connection it can be sent over; what it keeps sending
                        // is five-second bars, and the bar still forming is
                        // folded from those.
                        self.hmds.send_historical_request_ex(
                            req_id, con_id, &end_date_time, &duration, &bar_size, &what_to_show,
                            use_rth, false, &symbol, &sec_type, &exchange,
                            &mut self.hmds_conn, &mut self.hb, &self.shared,
                        );
                        if let Ok(size) = crate::control::historical::BarSize::from_api_str(&bar_size) {
                            self.hmds.keep_up_to_date_reqs.insert(req_id);
                            self.hmds.forming_bars.retain(|f| f.req_id != req_id);
                            self.hmds.forming_bars.push(crate::engine::hot_loop::hmds::FormingBar {
                                req_id,
                                seconds: size.seconds(),
                                opened_at: 0,
                                bar: Default::default(),
                                weighted: 0.0,
                            });
                            self.hmds.kut_resub.retain(|k| k.req_id != req_id);
                            self.hmds.kut_resub.push(crate::engine::hot_loop::hmds::KutRequest {
                                req_id, con_id, end_date_time, duration, bar_size,
                                what_to_show: what_to_show.clone(), use_rth,
                                symbol: symbol.clone(), sec_type: sec_type.clone(),
                                exchange: exchange.clone(),
                            });
                            self.hmds.send_realtime_bar_subscribe(
                                req_id, con_id, &symbol, &sec_type, &exchange, &what_to_show,
                                use_rth, &mut self.hmds_conn, &mut self.hb,
                            );
                        }
                    } else {
                        self.hmds.send_historical_request_ex(req_id, con_id, &end_date_time, &duration, &bar_size, &what_to_show, use_rth, false, &symbol, &sec_type, &exchange, &mut self.hmds_conn, &mut self.hb, &self.shared);
                    }
                }
                ControlCommand::CancelHistorical { req_id } => {
                    self.hmds.keep_up_to_date_reqs.remove(&req_id);
                    self.hmds.kut_resub.retain(|k| k.req_id != req_id);
                    self.hmds.rtbar_subs.retain(|(_, rid, _, _)| *rid != req_id);
                    self.hmds.forming_bars.retain(|f| f.req_id != req_id);
                    // A keep-up-to-date request rides the five-second stream,
                    // and its routing and half-built bar are held apart from
                    // the request itself. Left behind, a later request under
                    // the same id folds its first bars into the cancelled
                    // one's partial.

                    if let Some(pos) = self.hmds.pending_historical.iter().position(|(_, rid, _)| *rid == req_id) {
                        let (query_id, _, _) = self.hmds.pending_historical.remove(pos);
                        self.hmds.send_historical_cancel(&query_id, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::FetchHeadTimestamp { contract, req_id, what_to_show, use_rth, .. } => {
                    let ContractRef { con_id, .. } = contract;
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, false);
                    } else {
                        self.hmds.send_head_timestamp_request(req_id, con_id, &what_to_show, use_rth, &mut self.hmds_conn, &mut self.hb, &self.shared);
                    }
                }
                ControlCommand::FetchContractDetails { contract, req_id, filters } => {
                    let ContractRef { con_id, symbol, sec_type, exchange, currency, .. } = contract;
                    if con_id > 0 {
                        self.ccp.send_secdef_request(req_id, con_id, &mut self.ccp_conn, &mut self.hb);
                    } else {
                        self.ccp.send_secdef_request_by_symbol(req_id, &symbol, &sec_type, &exchange, &currency, &filters, &mut self.ccp_conn, &mut self.hb);
                    }
                }
                ControlCommand::CancelHeadTimestamp { req_id } => {
                    if let Some(pos) = self.hmds.pending_head_ts.iter().position(|(_, rid)| *rid == req_id) {
                        self.hmds.pending_head_ts.remove(pos);
                    }
                }
                ControlCommand::FetchMatchingSymbols { req_id, pattern } => {
                    self.ccp.send_matching_symbols_request(req_id, &pattern, &mut self.ccp_conn, &mut self.hb);
                }
                ControlCommand::FetchCalendarMetaData { req_id } => {
                    self.secdef.send_calendar_meta_data_request(
                        req_id, &mut self.secdef_conn, &mut self.hb, &self.shared,
                    );
                }
                ControlCommand::FetchCalendarEvents { req_id, query } => {
                    self.secdef.send_calendar_events_request(
                        req_id, &query, &mut self.secdef_conn, &mut self.hb, &self.shared,
                    );
                }
                ControlCommand::CancelCalendar { req_id } => {
                    if !self.secdef.withdraw_calendar_request(req_id) {
                        push_hmds_error(
                            &self.shared, req_id,
                            "no calendar request is waiting under this id".to_string(),
                            false,
                        );
                    }
                }
                ControlCommand::FetchOptionParams { req_id, symbol, fut_fop_exchange, underlying_sec_type, underlying_con_id } => {
                    self.ccp.send_option_params_request(
                        req_id, &symbol, &fut_fop_exchange, &underlying_sec_type, underlying_con_id,
                        &mut self.ccp_conn, &mut self.hb, &self.shared,
                    );
                }
                ControlCommand::FetchMktDepthExchanges => {
                    self.ccp.send_mkt_depth_exchanges_request(&mut self.ccp_conn, &mut self.hb, &self.shared);
                }
                ControlCommand::FetchScannerParams => {
                    self.hmds.send_scanner_params_request(&mut self.hmds_conn, &mut self.hb);
                }
                ControlCommand::SubscribeScanner { req_id, instrument, location_code, scan_code, max_items, filters } => {
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, false);
                    } else {
                        self.hmds.send_scanner_subscribe(req_id, &instrument, &location_code, &scan_code, max_items, filters, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::CancelScanner { req_id } => {
                    if let Some(pos) = self.hmds.pending_scanner.iter().position(|(_, rid)| *rid == req_id) {
                        let (scan_id, _) = self.hmds.pending_scanner.remove(pos);
                        self.hmds.send_scanner_cancel(&scan_id, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::FetchHistoricalNews { req_id, con_id, provider_codes, start_time, end_time, max_results } => {
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, false);
                    } else {
                        self.hmds.send_historical_news_request(req_id, con_id, &provider_codes, &start_time, &end_time, max_results, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::FetchNewsArticle { req_id, provider_code, article_id } => {
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, false);
                    } else {
                        self.hmds.send_news_article_request(req_id, &provider_code, &article_id, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::FetchFundamentalData { req_id, con_id, report_type } => {
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, false);
                    } else {
                        self.hmds.send_fundamental_data_request(req_id, con_id, &report_type, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::CancelFundamentalData { req_id } => {
                    self.hmds.send_fundamental_cancel(req_id, &mut self.hmds_conn, &mut self.hb);
                }
                ControlCommand::FetchHistogramData { req_id, con_id, use_rth, period } => {
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, false);
                    } else {
                        self.hmds.send_histogram_request(req_id, con_id, use_rth, &period, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::CancelHistogramData { req_id } => {
                    if let Some(pos) = self.hmds.pending_histogram.iter().position(|(_, rid)| *rid == req_id) {
                        self.hmds.pending_histogram.remove(pos);
                    }
                }
                ControlCommand::FetchHistoricalTicks { contract, req_id, start_date_time, end_date_time, number_of_ticks, what_to_show, use_rth, .. } => {
                    let ContractRef { con_id, sec_type, exchange, .. } = contract;
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, false);
                    } else {
                        self.hmds.send_historical_ticks_request(req_id, con_id, &sec_type, &exchange, &start_date_time, &end_date_time, number_of_ticks, &what_to_show, use_rth, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::SubscribeRealTimeBar { contract, req_id, what_to_show, use_rth, .. } => {
                    let ContractRef { con_id, symbol, sec_type, exchange, .. } = contract;
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, false);
                    } else {
                        self.hmds.send_realtime_bar_subscribe(req_id, con_id, &symbol, &sec_type, &exchange, &what_to_show, use_rth, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::CancelRealTimeBar { req_id } => {
                    self.hmds.rtbar_resub.retain(|r| r.req_id != req_id);
                    if let Some(pos) = self.hmds.rtbar_subs.iter().position(|(_, rid, _, _)| *rid == req_id) {
                        let (query_id, _, ticker_id, _) = self.hmds.rtbar_subs.remove(pos);
                        let cancel_id = ticker_id.map(|t| t.to_string()).unwrap_or(query_id);
                        self.hmds.send_historical_cancel(&cancel_id, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::FetchHistoricalSchedule { contract, req_id, end_date_time, duration, use_rth, .. } => {
                    let ContractRef { con_id, sec_type, exchange, .. } = contract;
                    if self.hmds_conn.is_none() {
                        self.emit_hmds_unavailable(req_id, false);
                    } else {
                        self.hmds.send_schedule_request(req_id, con_id, &sec_type, &exchange, &end_date_time, &duration, use_rth, &mut self.hmds_conn, &mut self.hb);
                    }
                }
                ControlCommand::SubscribeDepth { contract, req_id, num_rows, is_smart_depth, filters, .. } => {
                    let ContractRef { con_id, exchange, sec_type, .. } = contract;
                    // Which venues offer a book is the server's to say, and it
                    // is asked once. Asked here rather than at logon so a
                    // session that never wants a book never asks.
                    //
                    // A book on no particular venue waits for the answer
                    // rather than going out to the one venue the contract is
                    // listed on: sent early it gathers from one venue where it
                    // was meant to gather from all of them, and a caller sees
                    // a thin book with nothing to say it is thin.
                    let on_no_venue =
                        is_smart_depth || matches!(exchange.as_str(), "SMART" | "BEST" | "");
                    if on_no_venue && self.shared.reference.depth_exchanges().is_empty() {
                        self.ccp.send_mkt_depth_exchanges_request(
                            &mut self.ccp_conn, &mut self.hb, &self.shared,
                        );
                        self.depth_awaiting_venues.push(ControlCommand::SubscribeDepth {
                            req_id, num_rows, is_smart_depth, filters,
                            contract: ContractRef { con_id, exchange, sec_type, ..Default::default() },
                        });
                        continue;
                    }
                    self.farm.send_depth_subscribe(
                        req_id, con_id, &exchange, &filters.primary_exchange, &sec_type,
                        num_rows, is_smart_depth,
                        &mut self.farm_conn,
                        &mut self.hb,
                        &self.shared,
                    );
                }
                ControlCommand::UnsubscribeDepth { req_id } => {
                    self.farm.send_depth_unsubscribe(
                        req_id,
                        &mut self.farm_conn,
                        &mut self.hb,
                    );
                    // Purge any already-buffered depth updates so callers never see stale data
                    self.shared.market.purge_depth_updates(req_id);
                }
                ControlCommand::SubscribePnl { req_id, account } => {
                    self.ccp.send_pnl_subscribe(req_id, &account, &mut self.ccp_conn, &mut self.hb);
                }
                ControlCommand::AdvisorConfig { command, partition, document } => {
                    self.ccp.send_advisor_config(
                        command, &partition, document.as_deref(),
                        &mut self.ccp_conn, &mut self.hb,
                    );
                }
                ControlCommand::CancelPnl { req_id } => {
                    let _ = req_id; // Server auto-cancels on disconnect; no explicit cancel message needed
                }
                ControlCommand::FetchNewsProviders { .. }
                | ControlCommand::FetchSmartComponents { .. }
                | ControlCommand::FetchSoftDollarTiers { .. }
                | ControlCommand::FetchUserInfo { .. } => {
                    // Gateway-local data — handled synchronously in Python EClient.
                    // These variants exist for future CCP round-trip support.
                }
                ControlCommand::Logout => {
                    // Tell the venue the session is going rather than leaving it
                    // to notice. This ends the session, so it is not part of
                    // stopping the loop: a caller that stops the engine and keeps
                    // its connections — reusing them for the next piece of work —
                    // must not have the session logged out from under it.
                    self.ccp.send_logout(&mut self.ccp_conn, &mut self.hb);
                }
                ControlCommand::ForceDisconnect => {
                    // What a maintenance window does, on demand. The recovery
                    // that follows is the engine's own: nothing here helps it
                    // along, which is the point.
                    log::warn!("both transports taken away on request");
                    self.force_farm_disconnect();
                    self.force_ccp_disconnect();
                }
                ControlCommand::Shutdown => {
                    // Unsubscribe all active market data before stopping
                    let instruments: Vec<InstrumentId> = self.farm.instrument_md_reqs
                        .iter().map(|(id, _)| *id).collect();
                    for instrument in instruments {
                        self.farm.send_mktdata_unsubscribe(
                            instrument,
                            &mut self.farm_conn,
                            &mut self.hb,
                        );
                    }
                    // Every tick stream withdrawn before stopping, each named
                    // by the request that opened it: a contract can carry
                    // several, and withdrawing by contract leaves the rest.
                    let open: Vec<(i64, InstrumentId)> = self.hmds.tbt_subscriptions
                        .iter().map(|sub| (sub.caller_req_id, sub.instrument)).collect();
                    for (req_id, instrument) in open {
                        self.hmds.send_tbt_unsubscribe(
                            req_id, instrument, &mut self.hmds_conn, &mut self.hb,
                        );
                    }
                    // Unsubscribe all news subscriptions before stopping
                    let news_instruments: Vec<InstrumentId> = self.ccp.news_subscriptions
                        .iter().map(|(id, _, _)| *id).collect();
                    for instrument in news_instruments {
                        self.ccp.send_news_unsubscribe(instrument, &mut self.ccp_conn, &mut self.hb);
                    }
                    self.running = false;
                    self.shared.set_connection_lost();
                    emit(&self.event_tx, Event::Stopped);
                }
            }
        }

        // All senders dropped — treat as implicit shutdown.
        if sender_dropped && self.running {
            log::warn!("Control channel disconnected — shutting down hot loop");
            self.running = false;
            self.shared.set_connection_lost();
            emit(&self.event_tx, Event::Stopped);
        }
    }

    fn check_heartbeats(&mut self) {
        let now = Instant::now();
        let ts = chrono_free_timestamp();

        // --- Auth heartbeat (skip if already disconnected) ---
        if !self.ccp.disconnected
        && let Some(conn) = self.ccp_conn.as_mut() {
            let since_sent = now.duration_since(self.hb.last_ccp_sent).as_secs();
            let since_recv = now.duration_since(self.hb.last_ccp_recv).as_secs();

            if since_sent >= self.hb.ccp_send_every() {
                let _ = conn.send_fix(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_HEARTBEAT),
                    (fix::TAG_SENDING_TIME, &ts),
                ]);
                self.hb.last_ccp_sent = now;
            }

            let warmed_up = now.duration_since(self.hb.ccp_up_since).as_secs() >= LIVENESS_WARMUP_SECS;
            if warmed_up && since_recv > self.hb.ccp_test_after() {
                if since_recv > self.hb.ccp_dead_after() {
                    log::error!("CCP liveness timeout ({since_recv}s silent) — connection lost");
                    self.ccp.handle_disconnect(&mut self.context, &self.shared, &self.event_tx);
                } else if self.hb.pending_ccp_test.is_none() {
                    let test_id = self.hb.next_test_id();
                    let _ = conn.send_fix(&[
                        (fix::TAG_MSG_TYPE, fix::MSG_TEST_REQUEST),
                        (fix::TAG_SENDING_TIME, &ts),
                        (fix::TAG_TEST_REQ_ID, &test_id),
                    ]);
                    self.hb.pending_ccp_test = Some((test_id, now));
                    self.hb.last_ccp_sent = now;
                }
            }
        }

        // --- Farm heartbeat (skip if already disconnected) ---
        if !self.farm.disconnected
        && let Some(conn) = self.farm_conn.as_mut() {
            let since_sent = now.duration_since(self.hb.last_farm_sent).as_secs();
            let since_recv = now.duration_since(self.hb.last_farm_recv).as_secs();

            // Half of what this farm said it expects, or half of what was
            // proposed where it said nothing. Answering exactly on the
            // deadline leaves no room for one heartbeat to be late, which is
            // the same reason the auth connection sends at half.
            if since_sent >= half_of(conn.heartbeat_secs.unwrap_or(FARM_HEARTBEAT_SECS)) {
                let _ = conn.send_fix(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_HEARTBEAT),
                    (fix::TAG_SENDING_TIME, &ts),
                ]);
                self.hb.last_farm_sent = now;
            }

            let stated = conn.heartbeat_secs;
            let warmed_up = now.duration_since(self.hb.farm_up_since).as_secs() >= LIVENESS_WARMUP_SECS;
            if warmed_up && since_recv > HeartbeatState::farm_test_after(stated) {
                if since_recv > HeartbeatState::farm_dead_after(stated) {
                    log::error!("Farm liveness timeout ({since_recv}s silent) — connection lost");
                    self.farm.handle_disconnect(&mut self.context, &self.event_tx);
                } else if self.hb.pending_farm_test.is_none() {
                    let test_id = self.hb.next_test_id();
                    let _ = conn.send_fix(&[
                        (fix::TAG_MSG_TYPE, fix::MSG_TEST_REQUEST),
                        (fix::TAG_SENDING_TIME, &ts),
                        (fix::TAG_TEST_REQ_ID, &test_id),
                    ]);
                    self.hb.pending_farm_test = Some((test_id, now));
                    self.hb.last_farm_sent = now;
                }
            }
        }

        // --- Historical heartbeat (skip if disconnected or no historical activity) ---
        if !self.hmds.disconnected && self.hmds_conn.is_some()
        && let Some(conn) = self.hmds_conn.as_mut() {
            let since_sent = now.duration_since(self.hb.last_hmds_sent).as_secs();
            let since_recv = now.duration_since(self.hb.last_hmds_recv).as_secs();

            // Half of what this farm said it expects, as on the farm channel
            // above. Sending on a fixed thirty is late for a farm that asked
            // for ten, and the venue closes a session it stopped hearing from.
            if since_sent >= half_of(conn.heartbeat_secs.unwrap_or(FARM_HEARTBEAT_SECS)) {
                let _ = conn.send_fix(&[
                    (fix::TAG_MSG_TYPE, fix::MSG_HEARTBEAT),
                    (fix::TAG_SENDING_TIME, &ts),
                ]);
                self.hb.last_hmds_sent = now;
            }

            let stated = conn.heartbeat_secs;
            let warmed_up = now.duration_since(self.hb.hmds_up_since).as_secs() >= LIVENESS_WARMUP_SECS;
            if warmed_up && since_recv > HeartbeatState::farm_test_after(stated) {
                if since_recv > HeartbeatState::farm_dead_after(stated) {
                    log::error!("HMDS liveness timeout ({since_recv}s silent) — connection lost");
                    self.hmds.disconnect(&mut self.hmds_conn);
                } else if self.hb.pending_hmds_test.is_none() {
                    let test_id = self.hb.next_test_id();
                    let _ = conn.send_fix(&[
                        (fix::TAG_MSG_TYPE, fix::MSG_TEST_REQUEST),
                        (fix::TAG_SENDING_TIME, &ts),
                        (fix::TAG_TEST_REQ_ID, &test_id),
                    ]);
                    self.hb.pending_hmds_test = Some((test_id, now));
                    self.hb.last_hmds_sent = now;
                }
            }
        }
    }

    fn pin_to_core(core: usize) {
        let core_ids = core_affinity::get_core_ids().unwrap_or_default();
        if let Some(id) = core_ids.get(core) {
            core_affinity::set_for_current(*id);
        }
    }

    /// Whether the farm connection has been lost.
    pub fn is_farm_disconnected(&self) -> bool {
        self.farm.disconnected
    }

    /// Whether the auth connection has been lost.
    pub fn is_ccp_disconnected(&self) -> bool {
        self.ccp.disconnected
    }

    /// Put back as much of a reconnect's subscription book as the pacing allows.
    fn drive_replay(&mut self) {
        let replay = ReplayPacing {
            burst: self.reconnect_cfg.replay_burst,
            pace: self.reconnect_cfg.replay_pace,
        };
        self.farm.drive_replay(replay, &mut self.farm_conn, &mut self.hb);
    }

    /// Replace the farm connection (after reconnection) and re-subscribe to all instruments.
    pub fn reconnect_farm(&mut self, conn: Connection) {
        let replay = ReplayPacing {
            burst: self.reconnect_cfg.replay_burst,
            pace: self.reconnect_cfg.replay_pace,
        };
        self.farm.reconnect(
            conn,
            &mut self.farm_conn,
            &mut self.context, &mut self.hb,
            replay,
            &self.shared,
        );
    }

    /// Forget a halt, unless it was one nothing can undo.
    ///
    /// One transport coming back says nothing about the other. A session the
    /// venue took away stays taken away while the market-data side reconnects
    /// happily, and clearing the halt on that success let the auth side be
    /// retried — straight back into taking the account from whoever holds it.
    fn clear_halt_if_it_was_not_settled(&mut self) {
        if matches!(&self.reconnect_halted, Some(reason) if reason.is_terminal()) {
            return;
        }
        self.reconnect_halted = None;
        self.shared.reference.clear_session_over();
    }

    /// Replace the auth connection (after reconnection) and reconcile order state.
    pub fn reconnect_ccp(&mut self, conn: Connection) {
        // Whoever held the account when this reconnect arrived, and the
        // interval the venue stated on it. Both are answers to this
        // connection's logon and belong to this connection: kept from the one
        // that went, a session would be held to an interval nobody agreed and
        // a takeover would be reported once and never again.
        // Set from this connection whether or not it names anybody. A logon
        // that names no other session is the venue saying the account is this
        // client's alone, and keeping the previous answer reports a holder
        // who has since gone — with their address and the time they took it —
        // for as long as the process runs.
        self.shared.reference.set_competing_session(conn.competing.clone());
        // What this connection stated, or what its own logon proposed where it
        // stated nothing — an ack that echoes no interval has accepted the one
        // it was offered. Either way it is this connection's, and leaving the
        // previous one in place holds the session to an interval neither end
        // of it agreed.
        self.hb.set_ccp_interval(conn.heartbeat_secs.unwrap_or(crate::config::CCP_HEARTBEAT));
        // This session's logon is now the newer one. Left at the first, every
        // later reconnect would find its own previous logon listed as a
        // competing session and give the account up to itself.
        if let Some(stamped) = conn.logged_in_at.clone()
            && let Some(auth) = self.reconnect_auth.as_mut()
        {
            auth.logged_in_at = stamped;
        }
        // Where this attempt landed. A reconnect can be redirected, and a
        // session that does not remember it dials the door again every time
        // and never learns the one host it knows answers for this account.
        if let Some(landed) = conn.connected_host.clone()
            && let Some(auth) = self.reconnect_auth.as_mut()
            && auth.host != landed
        {
            log::info!("this session is now on {landed}");
            auth.alternate_hosts.retain(|host| *host != landed);
            if !auth.alternate_hosts.contains(&auth.host) {
                auth.alternate_hosts.push(auth.host.clone());
            }
            auth.host = landed;
        }
        self.ccp.reconnect(conn, &mut self.ccp_conn, &mut self.hb, &self.account_id, &self.context.market);
    }

    /// Give up any transport a write has abandoned.
    fn disconnect_write_dead_transports(&mut self) {
        if !self.ccp.disconnected
            && self.ccp_conn.as_ref().is_some_and(|c| c.write_failed())
        {
            log::error!("CCP transport can no longer be written to — giving it up");
            self.ccp.handle_disconnect(&mut self.context, &self.shared, &self.event_tx);
        }
        if !self.farm.disconnected
            && self.farm_conn.as_ref().is_some_and(|c| c.write_failed())
        {
            log::error!("Farm transport can no longer be written to — giving it up");
            self.farm.handle_disconnect(&mut self.context, &self.event_tx);
        }
        if !self.hmds.disconnected
            && self.hmds_conn.as_ref().is_some_and(|c| c.write_failed())
        {
            log::error!("HMDS transport can no longer be written to — giving it up");
            self.hmds.disconnect(&mut self.hmds_conn);
        }
        // The calendar's connection, on the same terms as the other three. Its
        // read path gives it up when the socket goes, but a write that fails
        // without a read error after it leaves the connection installed — and
        // the reconnect declines to build another while one is, so the calendar
        // stays on a socket nothing can be sent through.
        if self.secdef_conn.as_ref().is_some_and(|c| c.write_failed()) {
            log::error!("Security-definition transport can no longer be written to — giving it up");
            self.secdef.give_up(&mut self.secdef_conn, &self.shared);
        }
    }

    /// Say once that recovery has stopped, so a caller waiting on a connection
    /// that is never coming back is told rather than left waiting.
    fn report_recovery_exhausted(&mut self, which: &str) {
        if self.reconnect_halted.is_some() {
            return;
        }
        log::error!(
            "{which} recovery abandoned after {} attempts — the limits the caller set are spent",
            self.budget.attempts(),
        );
        self.reconnect_halted = Some(retry::DisconnectReason::ByDesign);
        // Said once, where every request can read it: a session nothing is
        // trying to rebuild answers nothing, and a caller that is not told
        // waits out a timeout per call for an answer that cannot come.
        self.shared.reference.set_session_over(retry::DisconnectReason::ByDesign.as_str());
        self.shared.set_connection_lost();
        emit(&self.event_tx, Event::Disconnected);
    }

    /// Tell the client a transport it was told about is carrying traffic
    /// again. Silent unless a loss was announced, so the routine drops a
    /// reconnect handles on its own stay invisible.
    fn announce_reconnected(&mut self) {
        if !self.loss_announced { return; }
        // Both transports answer to the one notice, so the one that comes back
        // first must not speak for the other: a network cut takes down both,
        // and announcing on the first recovery told the caller everything was
        // back while half of it still was not.
        if self.farm.disconnected || self.ccp.disconnected { return; }
        self.loss_announced = false;
        log::info!("Connection restored — subscriptions re-established");
        self.shared.set_connection_restored();
        emit(&self.event_tx, Event::Reconnected);
    }

    /// Take the caller's recovery settings.
    pub fn set_reconnect_config(&mut self, cfg: crate::api::reliability::ReconnectConfig) {
        self.reconnect_cfg = cfg;
    }

    /// Set cached auth credentials for farm auto-reconnect.
    pub fn set_reconnect_auth(&mut self, auth: ReconnectAuth) {
        self.reconnect_auth = Some(auth);
    }


    /// Schedule-then-spawn farm reconnects on the jittered backoff ladder
 ///. Called every loop iteration; no-op while connected or an
    /// attempt is in flight.
    fn maybe_spawn_farm_reconnect(&mut self) {
        if !self.farm.disconnected || self.pending_farm_reconnect.is_some() {
            return;
        }
        // A reason the server has already given does not change by being asked
        // again, and a ladder climbed against one is just noise on someone
        // else's server.
        if self.reconnect_halted.is_some() {
            return;
        }
        if !self.budget.may_retry(&self.reconnect_cfg, Instant::now()) {
            self.report_recovery_exhausted("farm");
            return;
        }
        match self.farm_next_attempt_at {
            None => {
                let delay = reconnect_backoff(self.farm_reconnect_attempt);
                log::info!("Farm reconnect attempt {} scheduled in {:?}",
                    self.farm_reconnect_attempt + 1, delay);
                self.farm_next_attempt_at = Some(Instant::now() + delay);
            }
            Some(due) if Instant::now() >= due => {
                self.farm_next_attempt_at = None;
                self.budget.record_attempt(Instant::now());
                self.spawn_farm_reconnect();
                if self.pending_farm_reconnect.is_none() {
                    // Could not spawn (no cached credentials): re-check in a
                    // minute instead of warn-spamming every iteration.
                    self.farm_next_attempt_at =
                        Some(Instant::now() + std::time::Duration::from_secs(60));
                }
            }
            _ => {}
        }
    }

    /// See `maybe_spawn_farm_reconnect`.
    fn maybe_spawn_ccp_reconnect(&mut self) {
        if !self.ccp.disconnected || self.pending_ccp_reconnect.is_some() {
            return;
        }
        if self.reconnect_halted.is_some() {
            return;
        }
        if !self.budget.may_retry(&self.reconnect_cfg, Instant::now()) {
            self.report_recovery_exhausted("ccp");
            return;
        }
        match self.ccp_next_attempt_at {
            None => {
                let delay = reconnect_backoff(self.ccp_reconnect_attempt);
                log::info!("CCP reconnect attempt {} scheduled in {:?}",
                    self.ccp_reconnect_attempt + 1, delay);
                self.ccp_next_attempt_at = Some(Instant::now() + delay);
            }
            Some(due) if Instant::now() >= due => {
                self.ccp_next_attempt_at = None;
                self.budget.record_attempt(Instant::now());
                self.spawn_ccp_reconnect();
                if self.pending_ccp_reconnect.is_none() {
                    self.ccp_next_attempt_at =
                        Some(Instant::now() + std::time::Duration::from_secs(60));
                }
            }
            _ => {}
        }
    }

    /// Spawn a background thread to reconnect the farm using cached credentials.
    fn spawn_farm_reconnect(&mut self) {
        if self.pending_farm_reconnect.is_some() { return; } // already in progress
        let auth = match self.reconnect_auth.clone() {
            Some(a) if !a.host.is_empty() => a,
            _ => {
                log::error!(
                    "Farm is down and cannot be reconnected: no credentials were cached. \
                     The connection has to be rebuilt by the caller.",
                );
                self.reconnect_halted = Some(retry::DisconnectReason::AuthorizationFailed);
                // Said once, where every request can read it: a session nothing is
                // trying to rebuild answers nothing, and a caller that is not told
                // waits out a timeout per call for an answer that cannot come.
                self.shared.reference.set_session_over(retry::DisconnectReason::AuthorizationFailed.as_str());
                self.shared.set_connection_lost();
                emit(&self.event_tx, Event::Disconnected);
                return;
            }
        };
        self.farm_reconnect_attempt += 1;
        let attempt = self.farm_reconnect_attempt;
        log::info!("Farm auto-reconnect attempt {} starting (host={}, user={})", attempt, auth.host, auth.username);

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name(format!("farm-reconnect-{attempt}"))
            .spawn(move || {
                let (farm_host, farm_name) =
                    crate::gateway::reconnect_trading_route(&auth);
                let result = connect_farm(
                    &auth.settings, &farm_host, &farm_name,
                    &auth.username, &auth.password, auth.paper,
                    &auth.server_session_id, &auth.session_key,
                    &auth.hw_info, &auth.encoded, Farm::MarketData, auth.trading_port,
                );
                let _ = tx.send(result);
            })
            .ok();
        self.pending_farm_reconnect = Some(rx);
    }

    /// Poll for a completed farm reconnect. Non-blocking.
    fn poll_farm_reconnect(&mut self) {
        let rx = match self.pending_farm_reconnect.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        match rx.try_recv() {
            Ok(Ok(conn)) => {
                log::info!("Farm auto-reconnect succeeded (attempt {})", self.farm_reconnect_attempt);
                self.reconnect_farm(conn);
                self.farm_reconnect_attempt = 0;
                self.clear_halt_if_it_was_not_settled();
                self.budget.record_connected(Instant::now());
                self.announce_reconnected();
                self.farm_next_attempt_at = None;
                self.hb.farm_up_since = Instant::now();
                self.pending_farm_reconnect = None;
            }
            Ok(Err(e)) => {
                let reason = retry::DisconnectReason::from_error(&e);
                log::error!(
                    "Farm auto-reconnect failed (attempt {}): {} — {}",
                    self.farm_reconnect_attempt, e, reason.as_str(),
                );
                if reason.is_terminal() {
                    log::error!(
                        "Farm reconnect stopped: {}. Retrying cannot change this; \
                         the caller has to act.",
                        reason.as_str(),
                    );
                    self.reconnect_halted = Some(reason);
                    self.shared.reference.set_session_over(reason.as_str());
                    self.pending_farm_reconnect = None;
                    self.shared.set_connection_lost();
                    emit(&self.event_tx, Event::Disconnected);
                    return;
                }
                self.farm_next_attempt_at = Some(
                    Instant::now()
                        + retry::delay_for(reason, reconnect_backoff(self.farm_reconnect_attempt)),
                );
                self.pending_farm_reconnect = None;
                // Notify once after three straight failures; retries continue
                // on the backoff ladder — the old 3-attempt hard cap gave up
                // sooner than the gateway would.
                if self.farm_reconnect_attempt == 3 {
                    log::error!("Farm auto-reconnect failed 3 times — notifying (retries continue)");
                    self.loss_announced = true;
                    self.shared.set_connection_lost();
                    emit(&self.event_tx, Event::Disconnected);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("Farm reconnect thread dropped without result");
                self.pending_farm_reconnect = None;
            }
        }
    }

    /// Spawn a background thread to reconnect CCP using cached credentials.
    fn spawn_ccp_reconnect(&mut self) {
        if self.pending_ccp_reconnect.is_some() { return; }
        let auth = match self.reconnect_auth.clone() {
            Some(a) if !a.host.is_empty() => a,
            _ => {
                // Nothing to reconnect with, and nothing about waiting changes
                // that. Retrying it quietly every minute leaves a caller on a
                // dead trading connection with no way to learn it is dead —
                // which is the state this whole path exists to avoid.
                log::error!(
                    "CCP is down and cannot be reconnected: no credentials were cached. \
                     The connection has to be rebuilt by the caller.",
                );
                self.reconnect_halted = Some(retry::DisconnectReason::AuthorizationFailed);
                // Said once, where every request can read it: a session nothing is
                // trying to rebuild answers nothing, and a caller that is not told
                // waits out a timeout per call for an answer that cannot come.
                self.shared.reference.set_session_over(retry::DisconnectReason::AuthorizationFailed.as_str());
                self.shared.set_connection_lost();
                emit(&self.event_tx, Event::Disconnected);
                return;
            }
        };
        self.ccp_reconnect_attempt += 1;
        let attempt = self.ccp_reconnect_attempt;
        log::info!("CCP auto-reconnect attempt {} starting (host={})", attempt, auth.host);

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name(format!("ccp-reconnect-{attempt}"))
            .spawn(move || {
                let _ = tx.send(reconnect_ccp(&auth));
            })
            .ok();
        self.pending_ccp_reconnect = Some(rx);
    }

    /// Poll for a completed CCP reconnect. Non-blocking.
    fn poll_ccp_reconnect(&mut self) {
        let rx = match self.pending_ccp_reconnect.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        match rx.try_recv() {
            Ok(Ok(conn)) => {
                log::info!("CCP auto-reconnect succeeded (attempt {})", self.ccp_reconnect_attempt);
                self.reconnect_ccp(conn);
                self.ccp_reconnect_attempt = 0;
                self.clear_halt_if_it_was_not_settled();
                self.budget.record_connected(Instant::now());
                // The request for these goes out on this socket even though
                // the bars come back on the historical one, so an HMDS that
                // recovered first could not send them and left them recorded
                // but silent.
                self.resubscribe_keep_up_to_date();
                self.announce_reconnected();
                self.ccp_next_attempt_at = None;
                self.hb.ccp_up_since = Instant::now();
                self.pending_ccp_reconnect = None;
            }
            Ok(Err(e)) => {
                let reason = retry::DisconnectReason::from_error(&e);
                log::error!(
                    "CCP auto-reconnect failed (attempt {}): {} — {}",
                    self.ccp_reconnect_attempt, e, reason.as_str(),
                );
                if reason.is_terminal() {
                    log::error!(
                        "CCP reconnect stopped: {}. Retrying cannot change this; \
                         the caller has to act.",
                        reason.as_str(),
                    );
                    self.reconnect_halted = Some(reason);
                    self.shared.reference.set_session_over(reason.as_str());
                    self.pending_ccp_reconnect = None;
                    self.shared.set_connection_lost();
                    emit(&self.event_tx, Event::Disconnected);
                    return;
                }
                self.ccp_next_attempt_at = Some(
                    Instant::now()
                        + retry::delay_for(reason, reconnect_backoff(self.ccp_reconnect_attempt)),
                );
                self.pending_ccp_reconnect = None;
                // See the farm path: notify once, keep retrying.
                if self.ccp_reconnect_attempt == 3 {
                    log::error!("CCP auto-reconnect failed 3 times — notifying (retries continue)");
                    self.loss_announced = true;
                    self.shared.set_connection_lost();
                    emit(&self.event_tx, Event::Disconnected);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("CCP reconnect thread dropped without result");
                self.pending_ccp_reconnect = None;
            }
        }
    }

    /// If HMDS is down and a backoff window has elapsed, spawn the next attempt.
    /// Auto-schedules the first attempt when the engine starts with no HMDS
    /// connection — covers the case where initial soft-token returned
    /// FAILED and the gateway dropped the socket.
    fn maybe_spawn_hmds_reconnect(&mut self) {
        if self.hmds_conn.is_some() { return; }
        if self.pending_hmds_reconnect.is_some() { return; }
        let auth = match self.reconnect_auth.as_ref() {
            Some(a) if !a.host.is_empty() && !a.hmds_host.is_empty() => a,
            _ => return,
        };
        // Schedule the first attempt if not already scheduled.
        if self.hmds_next_attempt_at.is_none() {
            self.hmds_next_attempt_at = Some(Instant::now() + hmds_reconnect_backoff(self.hmds_reconnect_attempt + 1));
            return;
        }
        let due = self.hmds_next_attempt_at.unwrap();
        if Instant::now() < due { return; }
        let auth = auth.clone();
        self.hmds_reconnect_attempt += 1;
        let attempt = self.hmds_reconnect_attempt;
        log::info!(
            "HMDS reconnect attempt {} starting (host={}/{})",
            attempt, auth.hmds_host, auth.hmds_farm,
        );
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name(format!("hmds-reconnect-{attempt}"))
            .spawn(move || {
                let result = connect_farm(
                    &auth.settings, &auth.hmds_host, &auth.hmds_farm,
                    &auth.username, &auth.password, auth.paper,
                    &auth.server_session_id, &auth.session_key,
                    &auth.hw_info, &auth.encoded, Farm::Historical, auth.hmds_port,
                );
                let _ = tx.send(result);
            })
            .ok();
        self.pending_hmds_reconnect = Some(rx);
    }

    /// Rebuild the connection carrying the calendar, on the same terms as the
    /// others: a farm that goes is not a session that ends.
    fn maybe_spawn_secdef_reconnect(&mut self) {
        if self.secdef_conn.is_some() { return; }
        if self.pending_secdef_reconnect.is_some() { return; }
        let auth = match self.reconnect_auth.as_ref() {
            Some(a) if !a.secdef_host.is_empty() && !a.secdef_farm.is_empty() => a,
            // A session the venue named no such farm for has none to rebuild.
            _ => return,
        };
        if self.secdef_next_attempt_at.is_none() {
            self.secdef_next_attempt_at =
                Some(Instant::now() + hmds_reconnect_backoff(self.secdef_reconnect_attempt + 1));
            return;
        }
        if Instant::now() < self.secdef_next_attempt_at.unwrap() { return; }
        let auth = auth.clone();
        self.secdef_reconnect_attempt += 1;
        let attempt = self.secdef_reconnect_attempt;
        log::info!(
            "Security definition farm reconnect attempt {} starting ({}/{})",
            attempt, auth.secdef_host, auth.secdef_farm,
        );
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name(format!("secdef-reconnect-{attempt}"))
            .spawn(move || {
                let result = connect_farm(
                    &auth.settings, &auth.secdef_host, &auth.secdef_farm,
                    &auth.username, &auth.password, auth.paper,
                    &auth.server_session_id, &auth.session_key,
                    &auth.hw_info, &auth.encoded, Farm::SecurityDefinition, auth.secdef_port,
                );
                let _ = tx.send(result);
            })
            .ok();
        self.pending_secdef_reconnect = Some(rx);
    }

    /// Take the rebuilt connection, or schedule another attempt.
    fn poll_secdef_reconnect(&mut self) {
        let rx = match self.pending_secdef_reconnect.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        match rx.try_recv() {
            Ok(Ok(conn)) => {
                log::info!(
                    "Security definition farm reconnected (attempt {})",
                    self.secdef_reconnect_attempt,
                );
                self.secdef_conn = Some(conn);
                self.hb.last_secdef_recv = Instant::now();
                self.hb.last_secdef_sent = Instant::now();
                self.secdef_reconnect_attempt = 0;
                self.secdef_next_attempt_at = None;
                self.pending_secdef_reconnect = None;
            }
            Ok(Err(e)) => {
                log::warn!(
                    "Security definition farm reconnect attempt {} failed: {e}",
                    self.secdef_reconnect_attempt,
                );
                self.pending_secdef_reconnect = None;
                self.secdef_next_attempt_at = Some(
                    Instant::now() + hmds_reconnect_backoff(self.secdef_reconnect_attempt + 1),
                );
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending_secdef_reconnect = None;
            }
        }
    }

    /// Ask again for the keepUpToDate streams the dead HMDS socket was
    /// carrying. The request goes out on CCP and the bars arrive on HMDS, so
    /// only this side owns both and only this side can put them back.
    fn resubscribe_keep_up_to_date(&mut self) {
        if self.hmds.kut_resub.is_empty() { return; }
        // The request goes out on the auth socket and the bars come back on
        // the historical one, so both have to be up. Whichever recovers second
        // is the one that sends, and the other's call does nothing — asking
        // twice would leave two streams upstream for one subscription.
        if self.ccp_conn.is_none() || self.ccp.disconnected
            || self.hmds_conn.is_none() || self.hmds.disconnected
        {
            log::info!(
                "{} keepUpToDate stream(s) wait for both transports before being asked for again",
                self.hmds.kut_resub.len(),
            );
            return;
        }
        let wanted: Vec<_> = self.hmds.kut_resub.clone();
        let mut back = 0;
        for k in &wanted {
            self.hmds.pending_historical.retain(|(_, rid, _)| *rid != k.req_id);
            if self.hmds.send_historical_request_via_ccp(
                k.req_id, k.con_id, &k.end_date_time, &k.duration, &k.bar_size,
                &k.what_to_show, k.use_rth, &k.symbol, &k.sec_type, &k.exchange,
                &mut self.ccp_conn, &mut self.hb, &self.ccp.ccp_sign_key,
                &self.ccp.ccp_sign_iv, &self.shared,
            ) {
                back += 1;
            }
        }
        log::info!("HMDS reconnected, re-requested {}/{} keepUpToDate streams", back, wanted.len());
    }

    /// Poll for a completed HMDS reconnect. Non-blocking.
    fn poll_hmds_reconnect(&mut self) {
        let rx = match self.pending_hmds_reconnect.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        match rx.try_recv() {
            Ok(Ok(conn)) => {
                log::info!("HMDS reconnect succeeded (attempt {})", self.hmds_reconnect_attempt);
                self.hmds.reconnect(conn, &mut self.hmds_conn, &self.context.market, &mut self.hb);
                self.hb.last_hmds_recv = Instant::now();
                self.hb.last_hmds_sent = Instant::now();
                // The probe that went unanswered belonged to the dead session.
                // Leaving it pending suppressed the next one — the liveness
                // check only sends a TestRequest when none is outstanding — so
                // a fresh connection went silent-to-dead without ever being
                // probed. The warm-up starts here too, or the new session is
                // already past it.
                self.hb.pending_hmds_test = None;
                self.hb.hmds_up_since = Instant::now();
                self.hmds_reconnect_attempt = 0;
                self.hmds_next_attempt_at = None;
                self.pending_hmds_reconnect = None;
                self.resubscribe_keep_up_to_date();
            }
            Ok(Err(e)) => {
                log::warn!(
                    "HMDS reconnect failed (attempt {}): {}",
                    self.hmds_reconnect_attempt, e,
                );
                self.pending_hmds_reconnect = None;
                if self.hmds_reconnect_attempt == HMDS_NOTIFY_AFTER_ATTEMPTS {
                    log::error!(
                        "HMDS still down after {HMDS_NOTIFY_AFTER_ATTEMPTS} attempts — historical data is unavailable until it answers; retries continue",
                    );
                }
                self.hmds_next_attempt_at = Some(Instant::now() + hmds_reconnect_backoff(self.hmds_reconnect_attempt + 1));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("HMDS reconnect thread dropped without result");
                self.pending_hmds_reconnect = None;
            }
        }
    }

    /// Access heartbeat state for testing.
    pub fn heartbeat_state(&self) -> &HeartbeatState {
        &self.hb
    }

    /// Test-only: force farm into disconnected state.
    pub fn force_farm_disconnect(&mut self) {
        self.farm.handle_disconnect_for_test();
    }

    /// Test-only: force the auth transport into disconnected state.
    ///
    /// Sets the flag and nothing else, so recovery is exercised without
    /// invalidating the session server-side — which is what happens if the
    /// connection is taken away by logging in again from elsewhere, and why
    /// that is not a way to test this.
    pub fn force_ccp_disconnect(&mut self) {
        self.ccp.disconnected = true;
    }

    /// Test-only: trigger farm reconnect spawn.
    pub fn spawn_farm_reconnect_for_test(&mut self) {
        self.spawn_farm_reconnect();
    }

    /// Test-only: poll pending farm reconnect.
    pub fn poll_farm_reconnect_for_test(&mut self) {
        self.poll_farm_reconnect();
    }

    /// Mutably access heartbeat state for testing (e.g., setting timestamps).
    pub fn heartbeat_state_mut(&mut self) -> &mut HeartbeatState {
        &mut self.hb
    }

    /// Inject a raw farm message for testing. Processes it through the full decode pipeline.
    pub fn inject_farm_message(&mut self, msg: &[u8]) {
        self.farm.process_farm_message(msg, &mut self.farm_conn, &mut self.context, &self.shared, &self.event_tx, &mut self.hb);
    }

    /// Inject a raw auth message for testing. Processes execution reports, etc.
    pub fn inject_ccp_message(&mut self, msg: &[u8]) {
        self.ccp.process_ccp_message(msg, &mut self.ccp_conn, &mut self.context, &self.shared, &self.event_tx, &mut self.hb, &self.account_id);
    }

    /// Inject a raw HMDS message for testing. Processes historical data, news, etc.
    pub fn inject_hmds_message(&mut self, msg: &[u8]) {
        self.hmds.process_hmds_message(msg, &mut self.hmds_conn, &self.shared, &self.event_tx, &mut self.hb);
    }

    /// Inject a TBT trade for testing. Pushes to SharedState and emits event.
    pub fn inject_tbt_trade(&mut self, trade: &TbtTrade) {
        self.shared.market.push_tbt_trade(trade.clone());
        emit(&self.event_tx, Event::TbtTrade(trade.clone()));
    }

    /// Inject a TBT quote for testing. Pushes to SharedState.
    pub fn inject_tbt_quote(&mut self, quote: &TbtQuote) {
        self.shared.market.push_tbt_quote(*quote);
    }

    /// Inject a simulated tick for testing.
    pub fn inject_tick(&mut self, instrument: InstrumentId) {
        self.shared.market.push_quote(instrument, self.context.quote(instrument));
        emit(&self.event_tx, Event::Tick(instrument));
    }

    /// Simulate a fill for testing. Updates position and notifies.
    pub fn inject_fill(&mut self, fill: &Fill) {
        let delta = match fill.side {
            crate::types::Side::Buy => fill.qty,
            crate::types::Side::Sell | crate::types::Side::ShortSell => -fill.qty,
        };
        self.context.update_position(fill.instrument, delta as f64);
        self.shared.orders.push_fill(*fill);
        self.shared.portfolio.set_position(fill.instrument, self.context.position(fill.instrument));
        emit(&self.event_tx, Event::Fill(*fill));
    }
}

// ── Helper functions used by subsystems ──

/// Stack-allocated string (up to 24 bytes). Zero heap allocations.
pub(crate) struct StackStr {
    buf: [u8; 24],
    len: u8,
}

impl StackStr {
    #[inline]
    fn new() -> Self {
        Self { buf: [0; 24], len: 0 }
    }

    #[inline]
    fn push(&mut self, b: u8) {
        self.buf[self.len as usize] = b;
        self.len += 1;
    }

    /// Write an i64 in decimal. Returns number of bytes written.
    fn write_i64(&mut self, val: i64) {
        if val < 0 {
            self.push(b'-');
            self.write_u64((-val) as u64);
        } else {
            self.write_u64(val as u64);
        }
    }

    fn write_u64(&mut self, val: u64) {
        if val == 0 {
            self.push(b'0');
            return;
        }
        // Write digits in reverse, then reverse them in-place.
        let start = self.len as usize;
        let mut v = val;
        while v > 0 {
            self.push(b'0' + (v % 10) as u8);
            v /= 10;
        }
        self.buf[start..self.len as usize].reverse();
    }
}

impl std::ops::Deref for StackStr {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        // SAFETY: only ASCII digits, '.', '-' and ':' are written
        unsafe { std::str::from_utf8_unchecked(&self.buf[..self.len as usize]) }
    }
}

impl std::fmt::Display for StackStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

impl std::fmt::Debug for StackStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

/// Format an unsigned integer to a stack string. Zero alloc.
#[inline]
pub(crate) fn format_uint(val: u64) -> StackStr {
    let mut s = StackStr::new();
    s.write_u64(val);
    s
}

/// Emit an event to the channel (if connected). Non-blocking — drops event if full.
#[inline]
pub(crate) fn emit(event_tx: &Option<SyncSender<Event>>, event: Event) {
    if let Some(tx) = event_tx {
        let _ = tx.try_send(event);
    }
}

/// Clone a payload for the event channel, but only when one is attached.
///
/// Use this wherever the payload is a deep copy (bar batches, contract
/// definitions): the value goes to `SharedState` by move and the clone is paid
/// for only when someone is listening. With no channel — the default for the
/// Rust client — nothing is copied at all.
///
/// Clone first, push second, emit last, so the event never becomes visible
/// before the same data is readable from `SharedState`.
#[inline]
pub(crate) fn clone_for_event<T: Clone>(event_tx: &Option<SyncSender<Event>>, value: &T) -> Option<T> {
    event_tx.as_ref().map(|_| value.clone())
}

/// Backoff schedule for HMDS reconnect attempts.
/// `min(64, 3 * 2^(attempt-1))` seconds — approximates the captured cadence
/// of 3.2 / 11.4 / 18.5 / 42.7 / 63.7 s the official client uses.
#[inline]
/// Jittered reconnect backoff for CCP/farm, mirroring the
/// gateway's ladder: delay = min(2s floor + ladder + jitter, 82s). The
/// ladder climbs 0/5/15/30/50/60s per consecutive failure and the jitter
/// range grows 5s -> 20s (+5s per failure). Immediate rapid-fire re-dials
/// risk server-side rate limiting and eviction. (HMDS keeps its own
/// capture-matched doubling schedule below.)
pub(crate) fn reconnect_backoff(failures: u32) -> std::time::Duration {
    const LADDER_MS: [u64; 6] = [0, 5_000, 15_000, 30_000, 50_000, 60_000];
    let ladder = LADDER_MS[(failures as usize).min(LADDER_MS.len() - 1)];
    let jitter_max = (5_000 + 5_000 * failures as u64).min(20_000);
    let jitter = rand::random::<u64>() % jitter_max;
    std::time::Duration::from_millis((2_000 + ladder + jitter).min(82_000))
}

pub(crate) fn hmds_reconnect_backoff(attempt: u32) -> std::time::Duration {
    let n = attempt.saturating_sub(1).min(31);
    let secs = (3u64.saturating_mul(1u64 << n)).min(64);
    std::time::Duration::from_secs(secs)
}

/// Surface an "HMDS unavailable" error for `req_id` when the historical-data
/// socket isn't connected. Mirrors the QueryError surface: code 162
/// via `push_historical_error` for the consumer's `error()` callback, plus —
/// for historical-bar requests only — a terminal empty-bars response so
/// `historical_data_end` fires. Without this, requests issued while HMDS is
/// down hang silently.
pub(crate) fn push_hmds_unavailable(shared: &SharedState, req_id: u32, from_historical: bool) {
    push_hmds_error(
        shared, req_id,
        "Historical data service connection is not available".to_string(),
        from_historical,
    );
}

/// Surface an HMDS-side request failure: error 162 plus, for bar requests,
/// the terminal completion sentinel so a blocked wait unblocks.
pub(crate) fn push_hmds_error(shared: &SharedState, req_id: u32, message: String, from_historical: bool) {
    const HMDS_ERROR_CODE: i32 = 162;
    shared.reference.push_historical_error(
        req_id,
        HMDS_ERROR_CODE,
        message,
    );
    if from_historical {
        shared.reference.push_historical_data(
            req_id,
            crate::control::historical::HistoricalResponse {
                query_id: String::new(),
                timezone: String::new(),
                is_complete: true,
                bars: Vec::new(),
            },
        );
    }
}

/// Format a fixed-point Price as a decimal string for FIX tags. Zero alloc.
pub(crate) fn format_price(price: Price) -> StackStr {
    let whole = price / PRICE_SCALE;
    let frac = (price % PRICE_SCALE).unsigned_abs();
    let mut s = StackStr::new();
    // Division truncates toward zero, so a price in (-1, 0) has a whole part
    // of 0 and carries its sign only in the fraction. Writing the integer
    // alone would turn -0.35 into 0.35.
    if price < 0 && whole == 0 {
        s.push(b'-');
    }
    s.write_i64(whole);
    if frac != 0 {
        s.push(b'.');
        // Write 8-digit zero-padded fraction, then trim trailing zeros.
        let frac_start = s.len as usize;
        let digits = [
            b'0' + (frac / 10_000_000 % 10) as u8,
            b'0' + (frac / 1_000_000 % 10) as u8,
            b'0' + (frac / 100_000 % 10) as u8,
            b'0' + (frac / 10_000 % 10) as u8,
            b'0' + (frac / 1_000 % 10) as u8,
            b'0' + (frac / 100 % 10) as u8,
            b'0' + (frac / 10 % 10) as u8,
            b'0' + (frac % 10) as u8,
        ];
        // Find last non-zero digit.
        let mut end = 8;
        while end > 0 && digits[end - 1] == b'0' { end -= 1; }
        s.buf[frac_start..frac_start + end].copy_from_slice(&digits[..end]);
        s.len = (frac_start + end) as u8;
    }
    s
}

/// Parse a FIX tag value as a Price (fixed-point). Returns 0 if absent,
/// unparseable, or non-finite. Rust's f64 parser accepts "nan"/"inf", but on
/// the wire those are not-available sentinels, not values: the gateway's own
/// field parser maps nan/unparseable to unset. Without the finite
/// filter, "nan" saturated to 0 and "inf" to i64::MAX.
pub(crate) fn parse_price_tag(val: Option<&String>) -> Price {
    val.and_then(|s| s.parse::<f64>().ok())
        .filter(|f| f.is_finite())
        .map(|f| (f * PRICE_SCALE as f64) as Price)
        .unwrap_or(0)
}

/// Decode a wire TIF byte to the API TIF string. Exact inverse of
/// `api::types::Order::tif_byte` (DTC also encodes to '6' and decodes as GTD).
/// The old inline map decoded '7' (never emitted) as OPG and dropped
/// OPG ('2') and AUC ('8') to "".
pub(crate) fn decode_tif(tif: u8) -> &'static str {
    match tif {
        b'0' => "DAY", b'1' => "GTC", b'2' => "OPG", b'3' => "IOC",
        b'4' => "FOK", b'6' => "GTD", b'8' => "AUC", _ => "",
    }
}

/// Format a fixed-point `Qty` as a decimal string. Zero alloc.
///
/// The number of places comes from `QTY_SCALE` rather than being written out,
/// so a quantity held more finely is written more finely. Fixed at four
/// places while the scale said a hundred-millionth, half a share went out as
/// something else entirely.
pub(crate) fn format_qty(qty: Qty) -> StackStr {
    /// How many decimal places `QTY_SCALE` holds.
    const PLACES: usize = {
        let mut places = 0;
        let mut scale = QTY_SCALE;
        while scale > 1 {
            scale /= 10;
            places += 1;
        }
        places
    };
    const _: () = assert!(
        10i64.pow(PLACES as u32) == QTY_SCALE,
        "the quantity scale is a power of ten, or a fraction of it cannot be written out",
    );

    let whole = qty / QTY_SCALE;
    let frac = (qty % QTY_SCALE).unsigned_abs();
    let mut s = StackStr::new();
    s.write_i64(whole);
    if frac != 0 {
        s.push(b'.');
        let frac_start = s.len as usize;
        let mut digits = [b'0'; PLACES];
        let mut rest = frac;
        for slot in digits.iter_mut().rev() {
            *slot = b'0' + (rest % 10) as u8;
            rest /= 10;
        }
        let mut end = PLACES;
        while end > 0 && digits[end - 1] == b'0' { end -= 1; }
        s.buf[frac_start..frac_start + end].copy_from_slice(&digits[..end]);
        s.len = (frac_start + end) as u8;
    }
    s
}

/// Fast extraction of FIX tag 35 (MsgType) value via byte scan.
pub(crate) fn fast_extract_msg_type(msg: &[u8]) -> Option<&[u8]> {
    let limit = msg.len().min(48);
    let mut i = 0;
    while i + 3 < limit {
        if msg[i] == b'3' && msg[i + 1] == b'5' && msg[i + 2] == b'='
            && (i == 0 || msg[i - 1] == 0x01)
        {
            let val_start = i + 3;
            let mut j = val_start;
            while j < msg.len() && msg[j] != 0x01 {
                j += 1;
            }
            if j > val_start {
                return Some(&msg[val_start..j]);
            }
        }
        i += 1;
    }
    None
}

pub(crate) fn find_body_after_tag<'a>(msg: &'a [u8], tag_marker: &[u8]) -> Option<&'a [u8]> {
    msg.windows(tag_marker.len())
        .position(|w| w == tag_marker)
        .map(|pos| &msg[pos + tag_marker.len()..])
}

/// Extract the raw bytes of a binary FIX tag value using a length tag.
pub(crate) fn extract_raw_tag(msg: &[u8], tag: u32) -> Option<Vec<u8>> {
    let len_tag = tag - 1;
    if let Some(len_val) = extract_text_tag(msg, len_tag)
        && let Ok(data_len) = len_val.parse::<usize>() {
            let needle = format!("{tag}=");
            let needle_bytes = needle.as_bytes();
            if let Some(idx) = msg.windows(needle_bytes.len()).position(|w| w == needle_bytes) {
                let val_start = idx + needle_bytes.len();
                let val_end = (val_start + data_len).min(msg.len());
                return Some(msg[val_start..val_end].to_vec());
            }
        }
    let needle = format!("{tag}=");
    let needle_bytes = needle.as_bytes();
    let mut pos = 0;
    while pos < msg.len() {
        let remaining = &msg[pos..];
        if let Some(idx) = remaining.windows(needle_bytes.len()).position(|w| w == needle_bytes) {
            let abs_idx = pos + idx;
            if abs_idx == 0 || msg[abs_idx - 1] == 0x01 {
                let val_start = abs_idx + needle_bytes.len();
                let val_end = msg[val_start..].iter().position(|&b| b == 0x01)
                    .map(|p| val_start + p)
                    .unwrap_or(msg.len());
                return Some(msg[val_start..val_end].to_vec());
            }
            pos = abs_idx + 1;
        } else {
            break;
        }
    }
    None
}

/// Extract a text FIX tag value (SOH-delimited) from raw message bytes.
fn extract_text_tag(msg: &[u8], tag: u32) -> Option<String> {
    let needle = format!("{tag}=");
    let needle_bytes = needle.as_bytes();
    let mut pos = 0;
    while pos < msg.len() {
        let remaining = &msg[pos..];
        if let Some(idx) = remaining.windows(needle_bytes.len()).position(|w| w == needle_bytes) {
            let abs_idx = pos + idx;
            if abs_idx == 0 || msg[abs_idx - 1] == 0x01 {
                let val_start = abs_idx + needle_bytes.len();
                let val_end = msg[val_start..].iter().position(|&b| b == 0x01)
                    .map(|p| val_start + p)
                    .unwrap_or(msg.len());
                return Some(String::from_utf8_lossy(&msg[val_start..val_end]).into_owned());
            }
            pos = abs_idx + 1;
        } else {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {

    /// The session is held to the interval the venue named, not the one this
    /// client proposed at logon.
    ///
    /// The proposal is 10 seconds and the venue may answer with anything. Sent
    /// on the proposal regardless, a venue asking for 6 was answered every 10
    /// and closed the connection for being slow — which reads as a disconnect
    /// with no cause, minutes into a session that was working.
    #[test]
    fn the_heartbeat_follows_the_interval_the_venue_stated() {
        let mut hb = HeartbeatState::new();
        assert_eq!(hb.ccp_interval_secs, CCP_HEARTBEAT_SECS, "the proposal, until answered");
        assert_eq!(hb.ccp_send_every(), CCP_HEARTBEAT_SECS / 2);

        hb.set_ccp_interval(6);
        assert_eq!(hb.ccp_send_every(), 3, "half of what the venue asked for");

        // Half, because answering on the deadline leaves no room for one
        // heartbeat to be late. The counterpart sends at half for the same
        // reason.
        hb.set_ccp_interval(30);
        assert_eq!(hb.ccp_send_every(), 15);

        // A venue naming one second is answered every second, not on every
        // pass of the loop.
        hb.set_ccp_interval(1);
        assert_eq!(hb.ccp_send_every(), 1);

        // Nothing stated leaves the session on what it proposed, rather than
        // sending on every pass forever.
        hb.set_ccp_interval(0);
        assert_eq!(hb.ccp_send_every(), 1, "the last stated value stands");
    }

    /// Half of whatever a connection is held to, and never nothing.
    ///
    /// The farms carry their own interval and may name one this client did not
    /// propose, so the rule cannot live inside the auth side's state.
    #[test]
    fn half_an_interval_is_never_nothing() {
        assert_eq!(super::half_of(30), 15);
        assert_eq!(super::half_of(10), 5);
        assert_eq!(super::half_of(6), 3);
        // A venue naming one second is answered every second rather than on
        // every pass of the loop.
        assert_eq!(super::half_of(1), 1);
        assert_eq!(super::half_of(0), 1);
    }

    /// A reconnect that names nobody says the account is this client's alone.
    ///
    /// Keeping the previous connection's answer reports a holder who has since
    /// gone — their address and the time they took it — for the life of the
    /// process, and a caller reading it gives up an account nobody is holding.
    #[test]
    fn a_reconnect_that_names_nobody_clears_the_one_that_did() {
        let shared = SharedState::new();
        shared.reference.set_competing_session(Some((
            "10.0.0.7".to_string(), "20260815-09:30:00".to_string(), false,
        )));
        assert!(shared.reference.competing_session().is_some());

        // What `reconnect_ccp` does with a connection naming no other session.
        shared.reference.set_competing_session(None);

        assert!(
            shared.reference.competing_session().is_none(),
            "the account is this client's, and stays that way until told otherwise",
        );
    }

    /// A farm states its own cadence, and it is not the auth connection's.
    ///
    /// The fixed windows are sized against ten seconds. A farm asking for
    /// sixty is declared dead at thirty-five — between two of its own
    /// heartbeats — and a farm asking for ten is answered on a thirty-second
    /// schedule, which is the venue closing a session it stopped hearing from.
    #[test]
    fn a_farm_is_measured_against_the_cadence_it_stated() {
        // Nothing stated: the farm default, and the fixed windows with it.
        assert_eq!(HeartbeatState::farm_test_after(None), LIVENESS_TEST_SECS.max(45));
        assert_eq!(HeartbeatState::farm_dead_after(None), LIVENESS_DEAD_SECS.max(90));

        // A slower farm is not a silent one.
        assert_eq!(HeartbeatState::farm_test_after(Some(60)), 90);
        assert_eq!(HeartbeatState::farm_dead_after(Some(60)), 180);
        assert!(
            HeartbeatState::farm_dead_after(Some(60)) > LIVENESS_DEAD_SECS,
            "a farm on a sixty-second cadence outlives the fixed window",
        );

        // A faster one keeps the fixed windows, which are already generous.
        assert_eq!(HeartbeatState::farm_test_after(Some(10)), LIVENESS_TEST_SECS);
        assert_eq!(HeartbeatState::farm_dead_after(Some(10)), LIVENESS_DEAD_SECS);

        for stated in [None, Some(10), Some(30), Some(60)] {
            assert!(
                HeartbeatState::farm_test_after(stated) < HeartbeatState::farm_dead_after(stated),
                "probe before declaring, at {stated:?}",
            );
        }
    }

    /// An ack that echoes no interval has accepted the one it was offered.
    ///
    /// Keeping the previous connection's holds the new session to a cadence
    /// neither end of it agreed to.
    #[test]
    fn a_reconnect_that_states_no_interval_takes_the_one_it_proposed() {
        let mut hb = HeartbeatState::new();
        hb.set_ccp_interval(30);
        assert_eq!(hb.ccp_send_every(), 15);

        // An ack carrying no tag 108 leaves `reconnect_ccp` with the interval
        // its own logon proposed, which is what it sets.
        hb.set_ccp_interval(crate::config::CCP_HEARTBEAT);
        assert_eq!(
            hb.ccp_send_every(), crate::config::CCP_HEARTBEAT / 2,
            "the new connection's proposal, not the dead one's interval",
        );
    }

    /// A venue that speaks less often is not a venue that has gone away.
    ///
    /// The silence windows are fixed numbers chosen against a ten-second
    /// interval. Left fixed, a venue stating thirty would be probed while it
    /// was still inside its own cadence, and declared dead at thirty-five
    /// having missed one heartbeat.
    #[test]
    fn silence_is_measured_against_the_venues_own_cadence() {
        let mut hb = HeartbeatState::new();
        assert_eq!(hb.ccp_test_after(), LIVENESS_TEST_SECS);
        assert_eq!(hb.ccp_dead_after(), LIVENESS_DEAD_SECS);

        hb.set_ccp_interval(30);
        assert_eq!(hb.ccp_test_after(), 45, "not silent inside its own interval");
        assert_eq!(hb.ccp_dead_after(), 90, "three of its own heartbeats");
        assert!(hb.ccp_send_every() < hb.ccp_test_after(), "send before probing");
        assert!(hb.ccp_test_after() < hb.ccp_dead_after(), "probe before declaring");

        // A shorter interval keeps the fixed windows: they are already more
        // generous than the venue's cadence requires.
        hb.set_ccp_interval(5);
        assert_eq!(hb.ccp_test_after(), LIVENESS_TEST_SECS);
        assert_eq!(hb.ccp_dead_after(), LIVENESS_DEAD_SECS);
    }

    /// A reconnected transport must not inherit the dead session's outstanding
    /// probe. The liveness check only sends a TestRequest when none is pending,
    /// so a stale one suppressed every probe on the new connection and it went
    /// from silent to declared dead without being asked anything. The warm-up
    /// has to restart with it, or the new session is already past it.
    #[test]
    fn an_hmds_reconnect_does_not_inherit_the_dead_session_probe() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        hl.hb.pending_hmds_test = Some(("t-1".to_string(), Instant::now()));
        hl.hb.hmds_up_since = Instant::now() - Duration::from_secs(600);
        hl.hmds.disconnected = true;

        // A reconnect that has just succeeded.
        let listener = std::net::TcpListener::bind("127.0.1:0").unwrap();
        let sock = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(Ok(crate::protocol::connection::Connection::new_raw(sock).unwrap())).unwrap();
        hl.pending_hmds_reconnect = Some(rx);

        hl.poll_hmds_reconnect();

        assert!(hl.hmds_conn.is_some(), "the connection is installed");
        assert!(!hl.hmds.disconnected, "and the transport is live again");
        assert!(
            hl.hb.pending_hmds_test.is_none(),
            "the dead session's probe is dropped, or it suppresses every probe on the new one",
        );
        assert!(
            hl.hb.hmds_up_since.elapsed() < Duration::from_secs(LIVENESS_WARMUP_SECS),
            "and the new session starts inside its warm-up",
        );
    }

    /// The liveness timeout clears the connection as well as setting the
    /// disconnected flag. The reconnect scheduler returns early while a
    /// connection is present, so leaving the dead socket in place makes the
    /// first HMDS liveness timeout permanent for the life of the process, with
    /// every later historical request targeting the abandoned socket.
    #[test]
    fn an_hmds_disconnect_lets_its_reconnect_run() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        hl.set_reconnect_auth(crate::gateway::ReconnectAuth {
            trading_port: None,
            hmds_port: None,
            secdef_port: None,
            logged_in_at: String::new(),
            alternate_hosts: Vec::new(),
            settings: Default::default(),
            host: "gw.example".into(),
            username: "u".into(),
            password: zeroize::Zeroizing::new(String::new()),
            paper: true,
            code_provider: None,
            ib_key_timeout_secs: crate::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
            ib_key_token_sub_type: crate::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
            session_key: Default::default(),
            session_token: Default::default(),
            server_session_id: String::new(),
            hw_info: String::new(),
            encoded: String::new(),
            hmds_host: "hmds.example".into(),
            hmds_farm: "hfarm".into(),
            trading_host: "trade.example".into(),
            trading_farm: "tfarm".into(),
            secdef_host: String::new(),
            secdef_farm: String::new(),
        });

        // A live transport, so releasing it is observable.
        let listener = std::net::TcpListener::bind("127.0.1:0").unwrap();
        let sock = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        hl.hmds_conn = Some(crate::protocol::connection::Connection::new_raw(sock).unwrap());
        hl.hmds.disconnected = false;
        assert!(hl.hmds_next_attempt_at.is_none(), "nothing scheduled while it is up");

        // Driven through the receive path rather than by calling the helper:
        // the peer is gone, so `try_recv` errors and the poll gives up on the
        // transport. This is the ordinary way HMDS dies.
        drop(_peer);
        hl.hmds.poll(&mut hl.hmds_conn, &SharedState::new(), &None, &mut hl.hb);

        assert!(hl.hmds.disconnected, "the transport is marked dead");
        assert!(hl.hmds_conn.is_none(), "and the socket is released with it");

        // The scheduler now gets past its early return and arms the first attempt.
        hl.maybe_spawn_hmds_reconnect();
        assert!(
            hl.hmds_next_attempt_at.is_some(),
            "an HMDS reconnect is scheduled rather than skipped forever",
        );
    }

    /// A cancelled keep-up-to-date request leaves nothing behind under its id.
    ///
    /// The stream's routing and its half-built bar are held apart from the
    /// request, so cancelling the request alone leaves them. A later request
    /// numbered the same then folds its first bars into the cancelled one's
    /// partial and reports a bar built from two contracts.
    #[test]
    fn cancelling_a_kept_up_to_date_request_takes_its_stream_with_it() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        let req_id = 7u32;

        hl.hmds.keep_up_to_date_reqs.insert(req_id);
        hl.hmds.rtbar_subs.push(("hist_1".to_string(), req_id, Some(99), 0.01));
        hl.hmds.forming_bars.push(crate::engine::hot_loop::hmds::FormingBar {
            req_id,
            seconds: 60,
            opened_at: 0,
            bar: Default::default(),
            weighted: 0.0,
        });

        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        hl.set_control_rx(rx);
        tx.send(crate::types::ControlCommand::CancelHistorical { req_id }).unwrap();
        hl.poll_control_commands();


        assert!(!hl.hmds.keep_up_to_date_reqs.contains(&req_id));
        assert!(
            !hl.hmds.rtbar_subs.iter().any(|(_, rid, _, _)| *rid == req_id),
            "the stream's routing goes with the request",
        );
        assert!(
            !hl.hmds.forming_bars.iter().any(|f| f.req_id == req_id),
            "and so does the bar it was part way through",
        );
    }

    /// A subscription still waiting to be told which contract it is for.
    ///
    /// The pending record carries the slot, not the contract — the contract is
    /// what the lookup is for. Reclaim it and the slot goes to the next
    /// registration; the lookup then comes back, adopts the first contract's
    /// id onto the second contract's slot, and subscribes it there. The quotes
    /// arrive under the wrong contract, priced on its tick increment.
    #[test]
    fn a_slot_waiting_on_its_lookup_is_not_reclaimed() {
        for stage in ["asked", "answered"] {
            let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
            let instrument = hl.context.market.register(0);
            let pending = crate::engine::hot_loop::ccp::PendingSubscribe {
                instrument,
                symbol: "SPY".into(),
                exchange: "SMART".into(),
                sec_type: "STK".into(),
                currency: "USD".into(),
                last_trade_date: String::new(),
                strike: 0.0,
                right: String::new(),
                multiplier: String::new(),
                mode_9887: 0,
            };
            match stage {
                // Out on the wire, no answer yet.
                "asked" => hl.ccp.pending_md_subscribe.push((1, pending, Instant::now())),
                // Answered, not yet sent.
                _ => hl.ccp.resolved_md_subscribe.push((756733, pending)),
            }

            hl.try_reclaim_instrument(instrument);

            // The slot is reclaimed by being handed to the next contract that
            // asks, which is exactly what must not happen here.
            let next = hl.context.market.register(265598);
            assert_ne!(
                next, instrument,
                "the slot was handed on while its lookup was {stage}",
            );
        }
    }

    /// The slot guard lists what still refers to a contract, and a holding was
    /// not on it. Dropping the last subscription on something the account owns
    /// freed the slot, and the contract that took it inherited the position.
    #[test]
    fn a_slot_the_account_still_holds_is_not_reclaimed() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        let instrument = hl.context.register_instrument(265598);
        hl.context.update_position(instrument, 100.0);

        hl.try_reclaim_instrument(instrument);
        assert_eq!(
            hl.context.market.con_id(instrument), Some(265598),
            "the slot stays with the contract the account is long",
        );

        assert_eq!(
            hl.pinned_by_position, vec![instrument],
            "and the request to free it is remembered rather than lost",
        );

        // Half a share is a holding. A whole-number table read this as flat and
        // handed the slot to the next contract.
        hl.context.update_position(instrument, -99.5);
        assert_eq!(hl.context.position(instrument), 0.5);
        hl.try_reclaim_instrument(instrument);
        assert_eq!(
            hl.context.market.con_id(instrument), Some(265598),
            "a fractional holding keeps its slot",
        );

        // Flat, and nothing else refers to it: now it may go.
        hl.context.update_position(instrument, -0.5);
        hl.try_reclaim_instrument(instrument);
        assert_eq!(hl.context.market.con_id(instrument), None, "the freed slot holds no contract");
        assert!(
            hl.hmds.tbt_subscriptions.iter().all(|sub| sub.instrument != instrument),
            "and no stream left for the next occupant to inherit prices from",
        );
        assert!(hl.pinned_by_position.is_empty(), "and nothing is still waiting on it");
    }

    /// A caller who would rather be told about a loss than have it handled
    /// gets exactly that: nothing is scheduled and nothing is attempted.
    #[test]
    fn a_manual_policy_leaves_the_reconnect_to_the_caller() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        hl.set_reconnect_config(crate::api::reliability::ReconnectConfig::manual());
        hl.farm.disconnected = true;

        hl.maybe_spawn_farm_reconnect();
        assert!(hl.farm_next_attempt_at.is_none(), "nothing is scheduled");
        assert!(hl.pending_farm_reconnect.is_none(), "and nothing is in flight");
    }

    /// A budget the caller set is spent and then recovery stops, rather than
    /// climbing forever against a connection that will not come back.
    #[test]
    fn a_spent_budget_stops_the_reconnect() {
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        let mut hl = HotLoop::new(shared, Some(tx), None);
        hl.set_reconnect_config(
            crate::api::reliability::ReconnectConfig::default().with_max_attempts(2),
        );
        hl.farm.disconnected = true;

        // Spend it.
        for _ in 0..2 {
            hl.budget.record_attempt(Instant::now());
        }
        hl.maybe_spawn_farm_reconnect();

        assert!(hl.farm_next_attempt_at.is_none(), "no further attempt is scheduled");
        assert!(
            matches!(rx.try_recv(), Ok(Event::Disconnected)),
            "and the caller is told recovery has stopped",
        );
    }

    /// A transport that is down and cannot be rebuilt is not a transport that
    /// is recovering. Retrying it quietly leaves the caller on a dead trading
    /// connection with nothing to tell them so.
    #[test]
    fn an_unrecoverable_transport_is_reported_rather_than_retried_in_silence() {
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        let mut hl = HotLoop::new(shared.clone(), Some(tx), None);
        hl.ccp.disconnected = true;
        // No credentials were ever cached, so there is nothing to reconnect with.

        hl.spawn_ccp_reconnect();

        assert!(
            matches!(rx.try_recv(), Ok(Event::Disconnected)),
            "the caller is told the connection is gone",
        );
        assert!(shared.take_connection_lost(), "and a caller without an event channel too");
        assert!(hl.reconnect_halted.is_some(), "and nothing keeps retrying what cannot work");
        // And every request made from here on is answered at once instead of
        // waiting out a timeout apiece for a session that has ended.
        assert!(
            shared.reference.session_over().is_some(),
            "a caller that asks for data is told the session is over, not left to time out",
        );
    }

    /// A login the server refused is refused the same way next time. Climbing
    /// a ladder against that answer forever is noise on someone else's server
    /// and silence to the caller, who is the only one who can fix it.
    #[test]
    fn a_refused_login_stops_the_reconnect_rather_than_looping() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        hl.farm.disconnected = true;
        hl.reconnect_halted = Some(retry::DisconnectReason::AuthorizationFailed);

        hl.maybe_spawn_farm_reconnect();
        assert!(hl.farm_next_attempt_at.is_none(), "nothing is scheduled against a settled answer");
        assert!(hl.pending_farm_reconnect.is_none(), "and no attempt is in flight");

        // Whatever the server objected to may have been changed since.
        hl.reconnect_halted = None;
        hl.maybe_spawn_farm_reconnect();
        assert!(hl.farm_next_attempt_at.is_some(), "an ordinary loss still retries");
    }

    /// A client that stood down on the loss needs the other edge to come back
    /// up. Without it an overnight outage the engine recovered from on its own
    /// still ended the session, from the caller's side.
    #[test]
    fn a_recovered_loss_is_announced_once_and_only_after_a_loss() {
        let shared = Arc::new(SharedState::new());
        let mut hl = HotLoop::new(shared.clone(), None, None);

        // Reconnects the client was never told about stay quiet.
        hl.announce_reconnected();
        assert!(!shared.take_connection_restored(), "nothing was announced to recover from");

        // One transport back is not the connection back.
        hl.loss_announced = true;
        hl.ccp.disconnected = true;
        hl.announce_reconnected();
        assert!(!shared.take_connection_restored(), "the other transport is still down");

        hl.ccp.disconnected = false;
        hl.announce_reconnected();
        assert!(shared.take_connection_restored(), "the recovery reaches the client");

        hl.announce_reconnected();
        assert!(!shared.take_connection_restored(), "and reaches it once");
    }

    /// The servers go down for maintenance most nights and come back on their
    /// own. Stopping after six attempts — about two and a half minutes on the
    /// ladder — left historical data dead until someone restarted the process,
    /// which is the failure the gateway has when its restart token is unset.
    #[test]
    fn hmds_keeps_retrying_through_an_outage_longer_than_the_ladder() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        hl.set_reconnect_auth(crate::gateway::ReconnectAuth {
            trading_port: None,
            hmds_port: None,
            secdef_port: None,
            logged_in_at: String::new(),
            alternate_hosts: Vec::new(),
            settings: Default::default(),
            host: "gw.example".into(),
            username: "u".into(),
            password: zeroize::Zeroizing::new(String::new()),
            paper: true,
            code_provider: None,
            ib_key_timeout_secs: crate::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
            ib_key_token_sub_type: crate::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
            session_key: Default::default(),
            session_token: Default::default(),
            server_session_id: String::new(),
            hw_info: String::new(),
            encoded: String::new(),
            hmds_host: "hmds.example".into(),
            hmds_farm: "hfarm".into(),
            trading_host: "trade.example".into(),
            trading_farm: "tfarm".into(),
            secdef_host: String::new(),
            secdef_farm: String::new(),
        });

        // Well past the old cap, with the socket still down.
        hl.hmds_reconnect_attempt = 60;
        hl.hmds_next_attempt_at = None;
        hl.maybe_spawn_hmds_reconnect();

        let due = hl.hmds_next_attempt_at.expect("another attempt is armed");
        assert!(
            due.saturating_duration_since(Instant::now()) <= Duration::from_secs(64),
            "and the ladder stays capped rather than drifting out to nothing",
        );
    }
    use super::*;
    use std::sync::Arc;
    use crate::bridge::{Event, SharedState};
    use crate::types::*;
    use std::time::Duration;

    /// A contract specified the ordinary ibapi way — symbol, secType,
    /// exchange, no conId — arrives here with conId 0. Keyed on that alone,
    /// every such contract resolves to whichever one registered first: quotes
    /// land in one slot and an order built from it goes out under the first
    /// contract's symbol.
    /// Two conId-less options on one underlying differ only by strike and right,
    /// which the descriptor did not carry: both landed in one slot, so the put's
    /// quotes and its minTick overwrote the call's — and minTick is what snaps an
    /// order's price.
    #[test]
    fn two_conid_less_options_on_one_underlying_do_not_share_a_slot() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        let call = hl.register_or_reject(
            0, "AAPL".into(), "OPT", "SMART", "20260619|230|C|100", &None).expect("call");
        let put = hl.register_or_reject(
            0, "AAPL".into(), "OPT", "SMART", "20260619|240|P|100", &None).expect("put");
        assert_ne!(call, put, "the call and the put must not resolve to one slot");

        // The same option still resolves to its own slot rather than a third.
        assert_eq!(
            hl.register_or_reject(0, "AAPL".into(), "OPT", "SMART", "20260619|230|C|100", &None),
            Some(call), "the same contract keeps its slot",
        );
    }

    /// A slot allocated before any identity was stated — which is what the
    /// pre-flight registration does — must be adopted by the first caller that
    /// states one, not stranded while a second slot is allocated for the same
    /// contract.
    #[test]
    fn a_slot_with_no_identity_is_adopted_by_the_first_that_states_one() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        let pre = hl.register_or_reject(0, "AAPL".into(), "OPT", "SMART", "", &None).expect("pre");
        assert_eq!(
            hl.register_or_reject(0, "AAPL".into(), "OPT", "SMART", "20260619|230|C|100", &None),
            Some(pre), "the identity-less slot is adopted, not stranded",
        );
        // And once adopted it belongs to that contract alone.
        assert_ne!(
            hl.register_or_reject(0, "AAPL".into(), "OPT", "SMART", "20260619|240|P|100", &None),
            Some(pre), "a different contract does not inherit it",
        );
    }

    #[test]
    fn con_id_less_contracts_do_not_share_one_slot() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        let aapl = hl.register_or_reject(0, "AAPL".into(), "STK", "SMART", "", &None).expect("AAPL");
        let qqq = hl.register_or_reject(0, "QQQ".into(), "STK", "SMART", "", &None).expect("QQQ");

        assert_ne!(aapl, qqq, "two symbols must not resolve to one instrument");
        assert_eq!(hl.context.market.symbol(aapl), "AAPL");
        assert_eq!(hl.context.market.symbol(qqq), "QQQ", "an order on this slot names QQQ");

        // The same contract again is the same slot, or every re-registration
        // burns another one.
        assert_eq!(hl.register_or_reject(0, "AAPL".into(), "STK", "SMART", "", &None), Some(aapl));
        // Tick-by-tick and news register with neither secType nor exchange,
        // and must land on the slot the L1 subscription already has.
        assert_eq!(hl.register_or_reject(0, "QQQ".into(), "", "", "", &None), Some(qqq));
    }

    // F64::from_str accepts "nan"/"inf", so a not-available sentinel
    // of "nan" collapsed to price 0 (and "inf" saturated to i64::MAX) instead
    // of being treated as unset.
    #[test]
    fn parse_price_tag_rejects_non_finite_sentinels() {
        let s = |v: &str| v.to_string();
        assert_eq!(parse_price_tag(Some(&s("nan"))), 0);
        assert_eq!(parse_price_tag(Some(&s("NaN"))), 0);
        assert_eq!(parse_price_tag(Some(&s("inf"))), 0);
        assert_eq!(parse_price_tag(Some(&s("-inf"))), 0);
        assert_eq!(parse_price_tag(Some(&s("n/a"))), 0);
        assert_eq!(parse_price_tag(None), 0);
        // Genuine numbers still parse, including a true zero.
        assert_eq!(parse_price_tag(Some(&s("0"))), 0);
        assert_eq!(parse_price_tag(Some(&s("434.71"))), (434.71 * PRICE_SCALE as f64) as Price);
        assert_eq!(parse_price_tag(Some(&s("-1.5"))), (-1.5 * PRICE_SCALE as f64) as Price);
    }

    #[test]
    fn inject_tick_emits_events() {
        let shared = Arc::new(SharedState::new());
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
        let mut engine = HotLoop::new(shared.clone(), Some(event_tx), None);
        engine.context_mut().market.register(265598);

        engine.inject_tick(0);
        engine.inject_tick(0);

        let events: Vec<Event> = event_rx.try_iter().collect();
        let tick_count = events.iter().filter(|e| matches!(e, Event::Tick(_))).count();
        assert_eq!(tick_count, 2);
    }

    #[test]
    fn inject_tick_multiple_instruments() {
        let shared = Arc::new(SharedState::new());
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
        let mut engine = HotLoop::new(shared.clone(), Some(event_tx), None);
        engine.context_mut().market.register(265598); // 0: AAPL
        engine.context_mut().market.register(272093); // 1: MSFT

        engine.inject_tick(0);
        engine.inject_tick(1);

        let events: Vec<Event> = event_rx.try_iter().collect();
        let tick_events: Vec<_> = events.iter().filter_map(|e| match e {
            Event::Tick(id) => Some(*id),
            _ => None,
        }).collect();
        assert_eq!(tick_events, vec![0, 1]);
    }

    #[test]
    fn inject_fill_updates_position() {
        let shared = Arc::new(SharedState::new());
        let (event_tx, _event_rx) = std::sync::mpsc::sync_channel(4096);
        let mut engine = HotLoop::new(shared.clone(), Some(event_tx), None);
        engine.context_mut().market.register(265598);

        let fill = Fill {
            instrument: 0,
            order_id: 1001,
            side: Side::Buy,
            price: 150_00000000,
            qty: 100,
            remaining: 0,
            commission: 1_00000000,
            timestamp_ns: 0,
            cum_qty: 100, avg_price: 150_00000000,
        };
        engine.inject_fill(&fill);
        assert_eq!(engine.context_mut().position(0), 100.0);
    }

    #[test]
    fn heartbeat_state_accessible() {
        let shared = Arc::new(SharedState::new());
        let mut engine = HotLoop::new(shared, None, None);
        let hb = engine.heartbeat_state_mut();
        hb.last_farm_sent = Instant::now() - Duration::from_secs(60);
        assert!(engine.heartbeat_state().last_farm_sent.elapsed().as_secs() >= 59);
    }

    #[test]
    fn shutdown_sets_running_false() {
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut engine = HotLoop::new(shared, None, None);
        engine.set_control_rx(rx);
        engine.running = true;
        tx.send(ControlCommand::Shutdown).unwrap();
        engine.poll_once();
        assert!(!engine.is_running());
    }

    #[test]
    fn channel_disconnect_stops_loop() {
        let shared = Arc::new(SharedState::new());
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(4096);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut engine = HotLoop::new(shared, Some(event_tx), None);
        engine.set_control_rx(rx);
        engine.running = true;

        // Drop sender — simulates EClient being dropped without disconnect.
        drop(tx);

        engine.poll_once();
        assert!(!engine.is_running(), "hot loop should stop when control channel disconnects");

        // The session ended because the side that owned it went away, which
        // is a stop and not a loss: nothing is coming back, and a consumer
        // told it lost connectivity would stand by for a reconnect.
        let events: Vec<Event> = event_rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, Event::Stopped)));
    }

    #[test]
    fn shutdown_sets_connection_lost_flag_without_event_channel() {
        // The flag path must work with no event channel attached,
        // which is the default for the Rust client.
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut engine = HotLoop::new(shared.clone(), None, None);
        engine.set_control_rx(rx);
        engine.running = true;
        tx.send(ControlCommand::Shutdown).unwrap();
        engine.poll_once();

        assert!(shared.take_connection_lost(), "shutdown must signal connection lost");
        assert!(!shared.take_connection_lost(), "flag must clear after being read");
    }

    #[test]
    fn channel_disconnect_sets_connection_lost_flag() {
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut engine = HotLoop::new(shared.clone(), None, None);
        engine.set_control_rx(rx);
        engine.running = true;
        drop(tx);
        engine.poll_once();

        assert!(shared.take_connection_lost());
    }

    /// A fully disconnected engine parks rather than running the loop flat
    /// out. Every poll returns immediately with nothing, so an unparked loop is
    /// bounded only by clock speed and holds one core at full occupancy.
    ///
    /// Counted in iterations rather than CPU time, because that is what tells a
    /// parked loop from a spinning one on any machine. A free spin manages
    /// millions in this window; parked it is one per millisecond.
    #[test]
    fn a_loop_with_every_transport_down_does_not_spin() {
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut engine = HotLoop::new(shared, None, None);
        engine.set_control_rx(rx);
        engine.farm.disconnected = true;
        engine.ccp.disconnected = true;
        engine.hmds.disconnected = true;

        let handle = std::thread::spawn(move || {
            engine.run();
            engine.context.loop_iterations
        });
        std::thread::sleep(std::time::Duration::from_millis(60));
        tx.send(ControlCommand::Shutdown).unwrap();
        let iterations = handle.join().expect("the loop must exit on Shutdown");

        assert!(
            iterations < 10_000,
            "a parked loop runs on the order of 60 iterations in 60ms; got {iterations}",
        );
    }

    #[test]
    fn clone_for_event_skips_the_copy_when_no_channel() {
    // With no listener the deep copy must not happen at all.
        let payload = vec![1u8, 2, 3];
        assert!(clone_for_event(&None, &payload).is_none());

        let (tx, _rx) = std::sync::mpsc::sync_channel::<Event>(1);
        assert_eq!(clone_for_event(&Some(tx), &payload), Some(payload));
    }

    #[test]
    fn run_exits_on_shutdown() {
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut engine = HotLoop::new(shared, None, None);
        engine.set_control_rx(rx);

        // Send Shutdown before run() starts — run() should drain it and exit.
        tx.send(ControlCommand::Shutdown).unwrap();

        // run() should return (not hang).
        engine.run();
        assert!(!engine.is_running());
    }

    #[test]
    fn run_exits_on_channel_disconnect() {
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut engine = HotLoop::new(shared, None, None);
        engine.set_control_rx(rx);

        // Drop sender — run() should detect disconnect and exit.
        drop(tx);

        engine.run();
        assert!(!engine.is_running());
    }

    #[test]
    fn push_hmds_unavailable_historical_emits_error_and_terminal_sentinel() {
        let shared = SharedState::new();
        push_hmds_unavailable(&shared, 7, true);

        let errors = shared.reference.drain_historical_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, 7);
        assert_eq!(errors[0].1, 162);
        assert!(errors[0].2.contains("not available"));

        let hist = shared.reference.drain_historical_data();
        assert_eq!(hist.len(), 1, "terminal sentinel required so historical_data_end fires");
        assert_eq!(hist[0].0, 7);
        assert!(hist[0].1.is_complete);
        assert!(hist[0].1.bars.is_empty());
    }

    /// A price between -1 and 0 has a whole part of 0, so the sign lives only
    /// in the fraction. Writing the integer alone turns a credit into a debit —
    /// the same number with the opposite meaning, and nothing rejects it.
    #[test]
    fn format_price_keeps_the_sign_below_one() {
        let p = |v: f64| format_price((v * PRICE_SCALE as f64) as Price).to_string();
        assert_eq!(p(-0.35), "-0.35");
        assert_eq!(p(-0.5), "-0.5");
        assert_eq!(p(-0.01), "-0.01");
        // Either side of the boundary, and the cases that already worked.
        assert_eq!(p(-1.5), "-1.5");
        assert_eq!(p(-1.0), "-1");
        assert_eq!(p(0.0), "0");
        assert_eq!(p(0.35), "0.35");
        assert_eq!(p(1.5), "1.5");
        assert_eq!(p(737.53), "737.53");

        // Raw fixed-point units, so the boundary is pinned at the smallest
        // representable tick rather than only at prices a float can express.
        // A guard that fires slightly too late still formats these positive.
        let raw = |v: Price| format_price(v).to_string();
        assert_eq!(raw(-1), "-0.00000001", "the smallest negative tick keeps its sign");
        assert_eq!(raw(-PRICE_SCALE + 1), "-0.99999999", "just inside the interval");
        assert_eq!(raw(-PRICE_SCALE), "-1", "the boundary itself");
        assert_eq!(raw(-PRICE_SCALE - 1), "-1.00000001", "just outside it");
        assert_eq!(raw(0), "0");
        assert_eq!(raw(1), "0.00000001");
    }

    // The CCP/farm ladder — floor 2s, ladder 0/5/15/30/50/60s,
    // jitter range growing 5s -> 20s, ceiling 82s.
    #[test]
    fn reconnect_backoff_ladder_bounds() {
        use std::time::Duration;
        let expect = |failures: u32, lo: u64, hi: u64| {
            for _ in 0..50 {
                let d = reconnect_backoff(failures);
                assert!(d >= Duration::from_millis(lo) && d < Duration::from_millis(hi),
                    "failures={failures} got {d:?}, expected [{lo}ms, {hi}ms)");
            }
        };
        expect(0, 2_000, 7_000);    // 2s + 0 + jitter(0..5s)
        expect(1, 7_000, 17_000);   // 2s + 5s + jitter(0..10s)
        expect(2, 17_000, 32_000);  // 2s + 15s + jitter(0..15s)
        expect(3, 32_000, 52_000);  // 2s + 30s + jitter(0..20s)
        expect(5, 62_000, 82_001);  // 2s + 60s + jitter(0..20s), capped 82s
        expect(50, 62_000, 82_001); // ladder index capped
    }

    // The liveness ladder must sit inside the server's own thresholds
    // (test at 15s, dead at 35s, warm-up 60s). Ordering is a `const _` assert.
    #[test]
    fn liveness_thresholds_ordered() {
        assert_eq!(LIVENESS_TEST_SECS, 15);
        assert_eq!(LIVENESS_DEAD_SECS, 35);
        assert_eq!(LIVENESS_WARMUP_SECS, 60);
    }

    #[test]
    fn hmds_reconnect_backoff_matches_captured_cadence() {
        use std::time::Duration;
        // Captured cadence: 3.2 / 11.4 / 18.5 / 42.7 / 63.7 s.
        // This schedule: 3 / 6 / 12 / 24 / 48 / 64 s — follows the doubling
        // shape and caps at the 64 s ceiling.
        assert_eq!(hmds_reconnect_backoff(1), Duration::from_secs(3));
        assert_eq!(hmds_reconnect_backoff(2), Duration::from_secs(6));
        assert_eq!(hmds_reconnect_backoff(3), Duration::from_secs(12));
        assert_eq!(hmds_reconnect_backoff(4), Duration::from_secs(24));
        assert_eq!(hmds_reconnect_backoff(5), Duration::from_secs(48));
        assert_eq!(hmds_reconnect_backoff(6), Duration::from_secs(64));
        // Cap holds for any further attempts.
        assert_eq!(hmds_reconnect_backoff(7), Duration::from_secs(64));
        assert_eq!(hmds_reconnect_backoff(100), Duration::from_secs(64));
        // Saturating math survives degenerate inputs.
        assert_eq!(hmds_reconnect_backoff(0), Duration::from_secs(3));
        assert_eq!(hmds_reconnect_backoff(u32::MAX), Duration::from_secs(64));
    }

    #[test]
    fn push_hmds_unavailable_non_historical_emits_error_without_sentinel() {
        let shared = SharedState::new();
        push_hmds_unavailable(&shared, 42, false);

        let errors = shared.reference.drain_historical_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, 42);
        assert_eq!(errors[0].1, 162);
        // Head-ts / histogram / ticks / schedule / scanner / news / fundamental:
        // no bar-stream consumer waiting for historical_data_end.
        assert!(shared.reference.drain_historical_data().is_empty());
    }

    /// The tags a `35=Q` ack bound are dead the moment the subscription is
    /// cancelled, but `try_reclaim_instrument` was the only path that dropped
    /// them and it returns early while an open order, a tick-by-tick or a news
    /// subscription pins the slot. Every subscribe/unsubscribe cycle on a
    /// pinned instrument then left one behind until the next farm drop
 ///.
    #[test]
    fn an_l1_unsubscribe_hands_back_its_tags_on_a_pinned_instrument() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        let (tx, rx) = sync_channel(4);
        hl.set_control_rx(rx);

        let id = hl.context.market.register(4001);
        hl.context.market.register_server_tag(910_001, id);
        hl.farm.instrument_md_reqs.push((id, vec![7]));
        hl.hmds.tbt_subscriptions.push(crate::engine::hot_loop::hmds::TbtSubscription {
            instrument: id,
            query_id: "AAPL".to_string(),
            kind: TbtType::AllLast,
            caller_req_id: 0,
            venue_id: 0,
            min_tick: 0,
            size_tick: 0.0,
            running: Default::default(),
        });

        tx.send(ControlCommand::Unsubscribe { instrument: id }).unwrap();
        hl.poll_once();

        assert_eq!(
            hl.context.market.instrument_by_server_tag(910_001), None,
            "the cancelled subscription's tag is handed back, not held to the next farm drop",
        );
        assert_eq!(
            hl.context.market.con_id(id), Some(4001),
            "and the tick-by-tick subscription still pins the slot itself",
        );
    }

    /// The map is not L1-only: `35=L` ticker setup registers into it too and
    /// news resolves against it, so an unsubscribe that clears an instrument's
    /// tags while its news subscription is live ends the news feed silently.
    #[test]
    fn an_l1_unsubscribe_keeps_the_tags_a_live_news_subscription_routes_on() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        let (tx, rx) = sync_channel(4);
        hl.set_control_rx(rx);

        let id = hl.context.market.register(4002);
        hl.context.market.register_server_tag(910_002, id);
        hl.farm.instrument_md_reqs.push((id, vec![8]));
        hl.ccp.news_subscriptions.push((id, 55, "BRFG".to_string()));

        tx.send(ControlCommand::Unsubscribe { instrument: id }).unwrap();
        hl.poll_once();

        assert_eq!(
            hl.context.market.instrument_by_server_tag(910_002), Some(id),
            "news routes through this map, so its tag outlives the L1 request",
        );
    }
}

#[cfg(test)]
mod qty_formatting_tests {
    use super::format_qty;
    use crate::types::QTY_SCALE;

    /// A quantity goes out as the number it is. The places were written out as
    /// four while the scale held a hundred-millionth, so half a share left
    /// this machine as something else.
    #[test]
    fn a_fraction_of_a_share_is_written_as_itself() {
        for (held, written) in [
            (QTY_SCALE, "1"),
            (QTY_SCALE / 2, "0.5"),
            (QTY_SCALE / 4, "0.25"),
            (100 * QTY_SCALE, "100"),
            (-3 * QTY_SCALE / 2, "-1.5"),
            (1, "0.00000001"),
        ] {
            let out = format_qty(held);
            assert_eq!(&*out, written, "{held} was written as {}", &*out);
        }
    }

    /// Whatever the scale holds is written, and nothing beyond it.
    #[test]
    fn every_place_the_scale_holds_survives() {
        let smallest = format_qty(1);
        let places = smallest.split_once('.').expect("a fraction").1.len();
        assert_eq!(10i64.pow(places as u32), QTY_SCALE, "a place the scale holds went unwritten");
    }
}

#[cfg(test)]
mod calendar_farm_reconnect_tests {
    use super::*;

    /// A farm that goes is not a session that ends. The connection carrying
    /// the calendar is rebuilt on the same terms as the others, so a caller
    /// that asks again after one drops is served rather than told forever
    /// that this session has no such connection.
    #[test]
    fn a_calendar_connection_that_went_is_scheduled_to_come_back() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        hl.set_reconnect_auth(crate::gateway::ReconnectAuth {
            trading_port: None,
            hmds_port: None,
            secdef_port: None,
            logged_in_at: String::new(),
            alternate_hosts: Vec::new(),
            settings: Default::default(),
            host: "gw.example".into(),
            username: "u".into(),
            password: zeroize::Zeroizing::new(String::new()),
            paper: true,
            code_provider: None,
            ib_key_timeout_secs: crate::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
            ib_key_token_sub_type: crate::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
            session_key: Default::default(),
            session_token: Default::default(),
            server_session_id: String::new(),
            hw_info: String::new(),
            encoded: String::new(),
            hmds_host: String::new(),
            hmds_farm: String::new(),
            trading_host: String::new(),
            trading_farm: String::new(),
            secdef_host: "sd.example".into(),
            secdef_farm: "secdefeu".into(),
        });
        assert!(hl.secdef_conn.is_none());

        // The first pass schedules rather than dials, the way the others do.
        hl.maybe_spawn_secdef_reconnect();
        assert!(hl.secdef_next_attempt_at.is_some(), "no attempt was scheduled");
        assert!(hl.pending_secdef_reconnect.is_none(), "it dialled immediately");
    }

    /// A session the venue named no such farm for has none to rebuild, and
    /// does not sit trying to.
    #[test]
    fn a_session_without_that_farm_does_not_try() {
        let mut hl = HotLoop::new(Arc::new(SharedState::new()), None, None);
        hl.set_reconnect_auth(crate::gateway::ReconnectAuth {
            trading_port: None,
            hmds_port: None,
            secdef_port: None,
            logged_in_at: String::new(),
            alternate_hosts: Vec::new(),
            settings: Default::default(),
            host: "gw.example".into(),
            username: "u".into(),
            password: zeroize::Zeroizing::new(String::new()),
            paper: true,
            code_provider: None,
            ib_key_timeout_secs: crate::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
            ib_key_token_sub_type: crate::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
            session_key: Default::default(),
            session_token: Default::default(),
            server_session_id: String::new(),
            hw_info: String::new(),
            encoded: String::new(),
            hmds_host: String::new(),
            hmds_farm: String::new(),
            trading_host: String::new(),
            trading_farm: String::new(),
            secdef_host: String::new(),
            secdef_farm: String::new(),
        });
        hl.maybe_spawn_secdef_reconnect();
        assert!(hl.secdef_next_attempt_at.is_none());
        assert!(hl.pending_secdef_reconnect.is_none());
    }
}
