//! ibapi-compatible EClient — Rust equivalent of C++ `EClientSocket`.
//!
//! Connects to IB, provides ibapi-matching method signatures, and dispatches
//! events to a [`Wrapper`](crate::api::wrapper::Wrapper) via `process_msgs()`.
//!
//! ```no_run
//! use ibx::api::{EClient, EClientConfig, Wrapper, Contract, Order};
//! use ibx::api::types::TickAttrib;
//!
//! struct MyWrapper;
//! impl Wrapper for MyWrapper {
//!     fn tick_price(&mut self, req_id: i64, tick_type: i32, price: f64, attrib: &TickAttrib) {
//!         println!("tick_price: req_id={req_id} type={tick_type} price={price}");
//!     }
//! }
//!
//! let mut client = EClient::connect(&EClientConfig {
//!     username: "user".into(),
//!     password: "pass".into(),
//!     host: "your_ib_host".into(),
//!     paper: true,
//!     core_id: None,
//!     ..Default::default()
//! }).unwrap();
//!
//! client.req_mkt_data(1, &Contract { con_id: 756733, symbol: "SPY".into(), ..Default::default() },
//!     "", false, false).unwrap();
//!
//! let mut wrapper = MyWrapper;
//! loop {
//!     client.process_msgs(&mut wrapper);
//! }
//! ```

pub(crate) mod ask;
mod simple;
pub use ask::{AccountValue, OptionChain, OrderReport, PositionRow, ScanRow, Schedule};
mod market_data;
mod orders;
mod account;
mod reference;
mod dispatch;
mod stubs;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::error_codes::Refusal;
use std::sync::{Arc, Mutex};
use std::thread;

use std::sync::mpsc::{Receiver, SyncSender};

use crate::types::model::{
    Contract as ApiContract, Order as ApiOrder, TagValue as ApiTagValue,
};
use crate::bridge::{Event, SharedState};
use crate::engine::hot_loop::EventSink;
use crate::client_core::ClientCore;
use crate::gateway::{Gateway, GatewayConfig, Session};
use crate::types::*;

// Re-export as public type names for the API surface
/// The contract type both surfaces share.
pub type Contract = ApiContract;
/// The order type both surfaces share.
pub type Order = ApiOrder;
/// One named option carried by a request.
pub type TagValue = ApiTagValue;

// Re-export public items from submodules
// Reads an order the caller composed, so it lives with the order model;
// reachable here because that is the path callers know it by.
pub use crate::client_core::parse_algo_params;

/// Configuration for connecting to IB via EClient.
///
/// # Live logins block on second-factor approval
///
/// With `paper: false`, [`connect()`](EClient::connect) enters a second-factor
/// approval window and **blocks** until the factor is approved (mobile push) or
/// the server-side deadline fires (~18 min). This is expected — it is a human
/// approval gate, not a hang. Bound or avoid it by using `paper: true`, lowering
/// the timeout (via [`GatewayConfig::ib_key_timeout_secs`] when building through
/// the lower-level API), or supplying a `code_provider`. Paper logins skip the
/// gate entirely. An `info`-level log line is emitted when the wait begins
/// (`RUST_LOG=info`).
///
/// # Multiple engines per process
///
/// Multiple `EClient` instances can run concurrently in one process. Each owns
/// its own state, sockets, and `ib-engine-hotloop` thread; nothing is shared
/// between them, and `connect()` does not serialize across instances. If you
/// pin engines with `core_id`, give each a **distinct** value — pinning two hot
/// loops to the same core makes them busy-poll the same CPU and starve each
/// other (degraded throughput, not a hang). With `core_id: None` (the default)
/// no pinning happens and there is no conflict.
#[derive(Default)]
pub struct EClientConfig {
    /// The login.
    pub username: String,
    /// Its password. Held only for the length of the logon.
    pub password: String,
    /// Where to start the session.
    ///
    /// Leave it empty. A login is enough: the venue answers the first message
    /// by naming which server this account belongs on, and the session moves
    /// there — so what is named here is only where to knock. Name one for a
    /// test, or to knock at a particular region.
    pub host: String,
    /// Refuse to send anything that places, changes or withdraws an order.
    ///
    /// The gateway had this as a setting of its own, and the other client here
    /// takes it on connect. A Rust caller could not state it at all, so a
    /// session meant to only look could still trade.
    pub readonly: bool,
    /// What a gateway holds in its own configuration file.
    ///
    /// A gateway is a process configured by a file beside it; this client is a
    /// library, so those settings are stated here instead of in a file nobody
    /// writes. Applied as the session opens, and for the whole process
    /// [`GatewaySettings`](crate::settings::GatewaySettings).
    pub gateway: crate::settings::GatewaySettings,
    /// `false` enters the live second-factor approval gate on connect (blocking).
    /// `true` skips it. See the type-level docs.
    pub paper: bool,
    /// CPU core to pin this engine's hot loop to. `None` = no pinning. When
    /// running multiple engines, use a **distinct** core per engine.
    pub core_id: Option<usize>,
    /// Supplies the second-factor code. Required for accounts whose factor is
    /// an authenticator code — those have no push to fall back to, and connect
    /// fails without it. For IBKey accounts it selects Challenge/Response over
    /// waiting for a mobile push, so `None` is fine there.
    pub code_provider: Option<crate::auth::session::CodeProvider>,
    /// Offer a session captured earlier, instead of logging in again.
    ///
    /// Take it from [`session()`](EClient::session). The connect names that
    /// session and the server chooses: a challenge it can answer from the
    /// session alone, or the ordinary login. Whatever it chooses, this connects
    /// — a session it will not take costs a login, never an error.
    ///
    /// **The servers reached from here have not yet chosen the challenge.**
    /// Every observed login, including one offering a session left by a process
    /// that was killed rather than closed, has been answered with the ordinary
    /// one. The client asks and handles both answers, because the protocol
    /// carries both and this library already answers the challenge when a
    /// dropped connection is rebuilt. Set this and lose nothing; do not plan
    /// around it skipping a second factor until you have seen it do so.
    pub resume: Option<crate::auth::resume::ResumableSession>,
    /// What to do about a dropped connection.
    ///
    /// The default recovers on its own and keeps trying, which is what a
    /// process that must stay up wants and what having no gateway makes this
    /// library's job. Set it to bound the effort, or to be told about a loss
    /// and decide yourself.
    /// [`ReconnectConfig`](crate::reliability::ReconnectConfig).
    pub reconnect: crate::reliability::ReconnectConfig,
    /// Keep the session in this file, so a restart can offer it without a
    /// person present.
    ///
    /// Off unless set, and worth leaving off for now. Nothing about the session
    /// touches disk otherwise: it is held in memory for the life of the
    /// process, which is all a reconnect needs. Setting this writes a
    /// credential to disk, sealed under the account password and readable only
    /// by its owner, and buys whatever [`resume`](EClientConfig::resume) buys —
    /// which today, on the servers reached from here, is nothing. That is a
    /// cost with no measured return, so it is a decision rather than a default.
    pub session_file: Option<std::path::PathBuf>,
    /// Where the last order id handed out is kept, so the next run does not
    /// hand out the same ones.
    ///
    /// An order id belongs to the account rather than to the process: the
    /// venue answers an order under an id it already holds with "Duplicate ID"
    /// and places nothing. The counterpart remembers its last id in its own
    /// settings and hands out that value plus one; this is the same, in a file
    /// keyed by account, kind of session and client id. Left unset the counter
    /// lives as long as the process, which is enough for one run and not for
    /// two.
    pub order_id_file: Option<std::path::PathBuf>,
}

/// ibapi-compatible EClient. Matches C++ `EClientSocket` method signatures.
///
/// # Thread lifecycle
///
/// `connect()` spawns a single `ib-engine-hotloop` background thread.
/// The thread is **joined** on [`disconnect()`](EClient::disconnect) and on [`Drop`].
/// Dropping an `EClient` without calling `disconnect()` first is safe:
/// the `Drop` impl sends `Shutdown` and joins the thread.
///
/// # Losing the connection
///
/// When the engine stops — connection lost, reconnect exhausted, or the hot
/// loop panicked — the next [`process_msgs()`](EClient::process_msgs) call
/// fires [`connection_closed`](crate::api::wrapper::Wrapper::connection_closed) once
/// and
/// [`is_connected()`](EClient::is_connected) turns false. No error callback is
/// raised for this: the connectivity error codes are pushed by the server, not
/// synthesized locally.
pub struct EClient {
    pub(crate) shared: Arc<SharedState>,
    pub(crate) control_tx: SyncSender<ControlCommand>,
    pub(crate) thread: Mutex<Option<thread::JoinHandle<()>>>,
    /// The account this session acts for.
    pub account_id: String,
    /// Every account this login holds, the first being [`EClient::account_id`].
    pub accounts: Vec<String>,
    pub(crate) connected: AtomicBool,
    /// True once `connection_closed` has been delivered, so it fires at most
    /// once per session.
    pub(crate) close_notified: AtomicBool,
    pub(crate) next_order_id: AtomicU64,
    /// Where the last id handed out is kept, and under which key.
    pub(crate) order_id_store: Option<(std::path::PathBuf, String)>,
    /// One question at a time.
    ///
    /// A question drives the message pump itself, and the pump hands every
    /// message it drains to the collector that is running. A collector keeps
    /// what carries its own request id and discards the rest, so a second
    /// question asked while the first is pumping has its answer read and thrown
    /// away by the first, and waits out its timeout for a reply that already
    /// came. Held across the sending too, so the second question is not on the
    /// wire while the first is listening.
    pub(crate) asking: Mutex<()>,
    /// How many events the channel above discarded, if one is attached.
    pub(crate) discarded: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(crate) core: ClientCore,
    pub(crate) session_token_bytes: Vec<u8>,
    pub(crate) session: crate::auth::resume::ResumableSession,
    pub(crate) token_type: String,
}

impl Drop for EClient {
    fn drop(&mut self) {
        // Ensure the hot-loop thread is stopped and joined.
        let _ = self.control_tx.send(ControlCommand::Shutdown);
        if let Some(h) = self.thread.lock().unwrap().take() {
            let _ = h.join();
        }
    }
}

/// Narrow a caller's req_id to the width the request carries on the wire.
///
/// `EClient` takes req_id as `i64` for ibapi parity, but these requests encode
/// it as a `u32`, and the callbacks report back whatever was encoded. A cast
/// would answer under an id the caller never used — and `next_order_id()`
/// hands out ids well past `u32::MAX`, so the ibapi idiom of one counter for
/// orders and requests hit it on the first call. Refuse instead.
pub(crate) fn wire_req_id(req_id: i64) -> Result<u32, Refusal> {
    u32::try_from(req_id).map_err(|_| {
        Refusal::validation(format!(
            "req_id {req_id} is outside the range this request can carry (0..={})", u32::MAX,
        ))
    })
}

/// The gateway's view of an [`EClientConfig`].
///
/// Extracted so the forwarding is checkable without opening a socket: the
/// second-factor provider reaching the gateway is the whole of what makes the
/// feature usable from this client, and it is one line that a refactor can
/// drop silently.
fn gateway_config(config: &EClientConfig) -> GatewayConfig {
    GatewayConfig {
        // Settled here, once, on the caller's thread: everything downstream
        // reads a value rather than the process it happens to run in.
        settings: std::sync::Arc::new(config.gateway.resolve()),
        username: config.username.clone(),
        password: zeroize::Zeroizing::new(config.password.clone()),
        // A caller with a login should not have to know a hostname. The one
        // it would name is where every session starts anyway, and the venue
        // answers the first message by naming which server to go to — every
        // session in this codebase's logs is redirected within a second of
        // connecting. Naming one stays possible, for a test or a region.
        host: if config.host.trim().is_empty() {
            crate::config::CCP_HOSTS[0].to_string()
        } else {
            config.host.clone()
        },
        paper: config.paper,
        accept_invalid_certs: false,
        ib_key_timeout_secs: crate::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS,
        ib_key_token_sub_type: crate::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into(),
        code_provider: config.code_provider.clone(),
        // What the caller handed back, or what was left in the file they named.
        // A file that cannot be read is a slower start, not a failed one: the
        // password is still here, and the whole point of the file is to avoid
        // needing a person, which an error thrown at one defeats.
        resume: config.resume.clone().or_else(|| {
            config.session_file.as_ref().and_then(|path| {
                crate::auth::resume::load(path, &config.username, &config.password, config.paper)
            })
        }),
    }
}

impl EClient {
    /// Connect to IB and start the engine.
    pub fn connect(config: &EClientConfig) -> Result<Self, Box<dyn std::error::Error>> {
        Self::connect_inner(config, None)
    }

    /// Connect to IB and start the engine with an event channel attached.
    ///
    /// A second, optional delivery path for a program that would rather own a
    /// queue than be called back. It is bounded, and an event arriving at a
    /// full one is discarded rather than made to wait — a session that stalled
    /// on a slow reader would stop carrying market data. Read
    /// [`events_lost`](EClient::events_lost) to learn whether that happened.
    ///
    /// One reader, and it is told what it drains and nothing else. For a
    /// program that wants more than one thing told about a message, or wants
    /// none of it dropped, hand a handler to [`Client`](crate::Client) instead: a
    /// handler is called with the message rather than sent a copy of it, so
    /// there is no queue to fill and no reader to be the only one. This is a second, optional delivery path that runs alongside
    /// [`process_msgs()`](EClient::process_msgs) — it does not replace it, and
    /// nothing is removed from the wrapper callbacks when it is in use.
    ///
    /// The channel is bounded by `capacity`; the engine never blocks on it, so
    /// a consumer that falls behind loses events rather than slowing the hot
    /// loop. Drain it from a thread that is not the one calling
    /// `process_msgs()`, or keep `capacity` generous.
    ///
    /// Attaching a channel makes the engine build events it would otherwise
    /// skip, which for bar batches and contract definitions means one deep copy
    /// each. Use [`connect()`](EClient::connect) when you only need the wrapper
    /// callbacks.
    pub fn connect_with_events(
        config: &EClientConfig,
        capacity: usize,
    ) -> Result<(Self, Receiver<Event>), Box<dyn std::error::Error>> {
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(capacity.max(1));
        let lost = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut client =
            Self::connect_inner(config, Some(EventSink::new(event_tx, std::sync::Arc::clone(&lost))))?;
        client.discarded = lost;
        Ok((client, event_rx))
    }

    fn connect_inner(
        config: &EClientConfig,
        event_tx: Option<EventSink>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let gw_config = gateway_config(config);

        let Session { gateway: gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, security_definition: secdef_conn } = Gateway::connect(&gw_config)?;
        let account_id = gw.account_id.clone();
        let accounts = gw.accounts.clone();
        let token_type = String::new();
        let session = crate::client_core::remember_session(
            config.session_file.as_deref(),
            &config.password,
            &gw,
            &config.username,
            config.paper,
        );
        let session_token_bytes = session.token.clone();

        let shared = Arc::new(SharedState::new());
        // Before the engine's threads exist, so nothing reads a setting that
        // can still change.
        shared.set_settings(gw_config.settings.clone());
        gw.populate_init_data(&shared);

        let (mut hot_loop, control_tx) = crate::engine::hot_loop::HotLoop::for_session(
            gw,
            shared.clone(), event_tx, farm_conn, ccp_conn, hmds_conn, secdef_conn, config.core_id,
            crate::gateway::CallerAuth {
                settings: Default::default(),
                host: config.host.clone(),
                username: config.username.clone(),
                password: zeroize::Zeroizing::new(config.password.clone()),
                paper: config.paper,
                code_provider: gw_config.code_provider.clone(),
                ib_key_timeout_secs: gw_config.ib_key_timeout_secs,
                ib_key_token_sub_type: gw_config.ib_key_token_sub_type.clone(),
            },
        );
        hot_loop.set_reconnect_config(config.reconnect.clone());

        let handle = thread::Builder::new()
            .name("ib-engine-hotloop".into())
            .spawn(move || { hot_loop.run_with_panic_recovery(); })?;

        // One past the last id this account handed out, where a file remembers
        // it. Otherwise the clock — seconds, not milliseconds: an id a
        // thousand times larger does not fit the width a request is carried
        // under, so every request built from one is refused before it leaves.
        let (start_id, order_id_store) = crate::client_core::order_ids_continue_from(
            config.order_id_file.clone(), &config.username, config.paper, 0,
        );

        let core = ClientCore::new();
        // Stated before the client is handed back, so a caller cannot place
        // anything between the session opening and the setting taking hold.
        core.set_readonly(config.readonly);
        core.set_registration_timeout(gw_config.settings.registration_timeout);

        Ok(Self {
            shared,
            control_tx,
            thread: Mutex::new(Some(handle)),
            account_id,
            accounts,
            connected: AtomicBool::new(true),
            close_notified: AtomicBool::new(false),
            next_order_id: AtomicU64::new(start_id),
            order_id_store,
            asking: Mutex::new(()),
            discarded: Default::default(),
            core,
            session_token_bytes,
            session,
            token_type,
        })
    }

    /// Construct from pre-built components (for testing or custom setups).
    #[doc(hidden)]
    pub fn from_parts(
        shared: Arc<SharedState>,
        control_tx: SyncSender<ControlCommand>,
        handle: thread::JoinHandle<()>,
        account_id: String,
    ) -> Self {
        let start_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() * 1000;
        Self {
            shared,
            control_tx,
            thread: Mutex::new(Some(handle)),
            accounts: vec![account_id.clone()],
            account_id,
            connected: AtomicBool::new(true),
            close_notified: AtomicBool::new(false),
            next_order_id: AtomicU64::new(start_id),
            // Built from parts, so nothing is remembered anywhere.
            order_id_store: None,
            asking: Mutex::new(()),
            discarded: Default::default(),
            core: ClientCore::new(),
            session_token_bytes: Vec::new(),
            session: Default::default(),
            token_type: String::new(),
        }
    }

    /// Map a reqId to an InstrumentId (for testing without a live engine).
    #[doc(hidden)]
    pub fn map_req_instrument(&self, req_id: i64, instrument: InstrumentId) {
        self.core.req_to_instrument.lock().unwrap().insert(req_id, instrument);
        self.core.instrument_to_req.lock().unwrap().insert(instrument, req_id);
    }

    /// Pre-populate the order tracker (for testing the dispatcher path
    /// without going through the engine's place-order flow).
    #[doc(hidden)]
    pub fn track_order_for_test(
        &self,
        order_id: u64,
        contract: ApiContract,
        order: ApiOrder,
        instrument: InstrumentId,
    ) {
        self.core.track_order(order_id, contract, order, instrument);
    }

    /// Pre-seed a con_id → InstrumentId mapping (for testing without a live engine).
    #[doc(hidden)]
    pub fn seed_instrument(&self, con_id: i64, instrument: InstrumentId) {
        self.core.con_id_to_instrument.lock().unwrap().insert(con_id, instrument);
    }

    /// Send a control command to the engine. Returns `Err` if the engine has shut down.
    pub(crate) fn send(&self, cmd: ControlCommand) -> Result<(), Refusal> {
        self.control_tx
            .send(cmd)
            .map_err(|e| Refusal::not_connected(format!("Engine stopped: {e}")))
    }

    // ── Connection ──

    /// How many events the channel from
    /// [`connect_with_events`](EClient::connect_with_events) discarded.
    ///
    /// The engine never waits on a reader — a session that stalled on one
    /// would stop carrying market data — so an event arriving at a full
    /// channel is dropped. A program that acted on every fill it saw needs to
    /// know the difference between that and every fill there was. Zero for a
    /// session with no channel attached, and for one whose reader kept up.
    pub fn events_lost(&self) -> u64 {
        self.discarded.load(Ordering::Relaxed)
    }

    /// False after [`disconnect()`](EClient::disconnect), and after a
    /// `process_msgs()` call that observed the engine stopping.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Disconnect from IB.  Sends `Shutdown` to the hot loop, waits for the
    /// background thread to exit, and marks the client as disconnected.
    pub fn disconnect(&self) {
        // The session is ending, so the venue is told before the engine stops.
        let _ = self.control_tx.send(ControlCommand::Logout);
        let _ = self.control_tx.send(ControlCommand::Shutdown);
        if let Some(h) = self.thread.lock().unwrap().take() {
            let _ = h.join();
        }
        self.connected.store(false, Ordering::Release);
        self.core.reset();
    }
}

impl EClient {
    /// Which slot a contract holds on this session, if it holds one.
    pub fn instrument_of(&self, con_id: i64) -> Option<crate::types::InstrumentId> {
        self.core.con_id_to_instrument.lock().unwrap().get(&con_id).copied()
    }

    /// The session's own state, for reading what has arrived.
    pub fn shared_state(&self) -> &Arc<SharedState> {
        &self.shared
    }

    /// Frames this session kept exactly as the venue sent them, by connection.
    ///
    /// Empty unless `IBX_CAPTURE_WIRE` is set. A reading checked only against
    /// frames this client made up says nothing about the ones that arrive.
    pub fn unread_wire(&self) -> Vec<(&'static str, String)> {
        self.shared.market.unread_wire()
    }

    /// Session ID surfaced to webapp REST clients as `x-ccp-session-id`.
    pub fn ccp_session_id(&self) -> String {
        self.shared.reference.ccp_session_id()
    }

    /// Logical-name → host URL lookup from the gateway logon MiscUrls push
    /// (e.g. `region_dam`). Returns `None` when the gateway did not push this key.
    pub fn misc_url(&self, key: &str) -> Option<String> {
        self.shared.reference.misc_url(key)
    }

    /// Canonical big-endian session-token bytes (leading zeros stripped) captured
    /// at connect. Round-trips through `BigUint::from_bytes_be` to the SRP shared
    /// secret K and is the second SHA-1 input for SSO `Authenticate-TWS` bodies.
    pub fn session_token_bytes(&self) -> &[u8] {
        &self.session_token_bytes
    }

    /// The session this connection established, for a caller that wants to
    /// resume from it later.
    ///
    /// Hand it back through [`EClientConfig::resume`] on a subsequent connect.
    /// Keep it wherever the process keeps secrets — it is a credential, and
    /// where it lives is the caller's decision, which is why nothing here
    /// writes it anywhere by default.
    pub fn session(&self) -> &crate::auth::resume::ResumableSession {
        &self.session
    }

    /// `stoken_type` discriminator captured at connect (`"st"`, `"tst"`, `"zenith"`,
    /// or empty for the SRP-only path). Sent verbatim in SSO authenticator bodies.
    pub fn token_type(&self) -> &str {
        &self.token_type
    }
}

#[cfg(test)]
mod host_default_tests {
    use super::*;

    /// A caller with a login and nothing else gets a session. The hostname it
    /// would otherwise have to name is where every session starts anyway, and
    /// the venue redirects from there.
    #[test]
    fn a_config_without_a_host_still_knows_where_to_knock() {
        let config = EClientConfig {
            username: "someone".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        assert!(config.host.is_empty(), "a caller stated none");
        let resolved = gateway_config(&config);
        assert_eq!(resolved.host, crate::config::CCP_HOSTS[0]);
    }

    /// One that is named is used as given.
    #[test]
    fn a_host_that_is_named_is_used() {
        let config = EClientConfig {
            username: "someone".to_string(),
            password: "secret".to_string(),
            host: "ndc1.ibllc.com".to_string(),
            ..Default::default()
        };
        assert_eq!(gateway_config(&config).host, "ndc1.ibllc.com");
    }
}

#[cfg(test)]
mod readonly_tests {
    use super::*;

    /// A session meant only to look refuses to place, change or withdraw an
    /// order. The other client here has taken this on connect all along; a
    /// Rust caller could not state it, so a read-only session could still
    /// trade.
    #[test]
    fn a_read_only_session_refuses_to_trade() {
        let core = ClientCore::new();
        core.set_readonly(true);
        assert!(core.refuse_if_readonly("place an order").is_err());
        assert!(
            core.refuse_if_readonly("place an order")
                .unwrap_err()
                .to_lowercase()
                .contains("read"),
            "the refusal does not say why",
        );
    }

    /// And one that was not asked for does not.
    #[test]
    fn an_ordinary_session_is_not_refused() {
        let core = ClientCore::new();
        core.set_readonly(false);
        assert!(core.refuse_if_readonly("place an order").is_ok());
    }
}
