//! ibapi-compatible EClient class that wraps IbEngine.

mod market_data;
mod orders;
mod account;
mod reference;
mod ask;
mod dispatch;
mod stubs;
// Not in a wheel. See the `test-helpers` feature.
#[cfg(feature = "test-helpers")]
mod test_helpers;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use std::sync::mpsc::SyncSender;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::auth::session::{CodeProvider, IbKeyChallenge, SecondFactor};
use crate::bridge::{Event, SharedState};
use crate::client_core::ClientCore;

/// What the reference client reports for a request made before connecting.
const NOT_CONNECTED_CODE: i64 = 504;
use crate::gateway::{Gateway, GatewayConfig, Session};
use crate::types::*;
use super::contract::Contract;

/// ibapi-compatible EClient class.
/// Wraps the internal engine and dispatches events to an EWrapper subclass.
///
/// All methods take `&self` (shared borrow) so that `run()` can execute in a
/// daemon thread while the main thread calls req/cancel methods concurrently.
/// `frozen` tells PyO3 to skip RefCell borrow-checking, which is required
/// because `run()` holds a `&self` borrow for the lifetime of the event loop.
/// Interior mutability is provided by `Mutex`, `AtomicBool`, and atomics.
///
/// # Thread lifecycle
///
/// `connect()` spawns a single `ib-engine-hotloop` background thread.
/// The thread is **joined** on [`disconnect()`] and on [`Drop`].
/// Dropping an `EClient` without calling `disconnect()` first is safe:
/// the `Drop` impl sends `Shutdown` and joins the thread.
///
/// The client is **reconnectable**: calling `disconnect()` resets all session
/// state so that a subsequent `connect()` on the same instance works correctly.
#[pyclass(frozen, subclass)]
pub struct EClient {
    /// Reference to the EWrapper (which is typically `self` in the `App(EWrapper,
    /// EClient)` pattern).
    pub(crate) wrapper: Py<PyAny>,
    /// Set by connect(), cleared by disconnect.
    pub(crate) shared: Mutex<Option<Arc<SharedState>>>,
    /// Set by connect(), cleared by disconnect.
    pub(crate) control_tx: Mutex<Option<SyncSender<ControlCommand>>>,
    pub(crate) next_order_id: AtomicU64,
    /// Where the last id handed out is kept, and under which key. Empty when
    /// the caller asked for no file, which makes the counter this session's
    /// alone and lets it collide with what an earlier one used.
    pub(crate) _thread: Mutex<Option<thread::JoinHandle<()>>>,
    /// Set by connect(), cleared by disconnect.
    pub(crate) account_id: Mutex<Option<String>>,
    /// Every account this login holds, the first being `account_id`.
    pub(crate) accounts: Mutex<Vec<String>>,
    pub(crate) connected: AtomicBool,
    /// Whether the caller asked for positions and has not withdrawn the ask.
    ///
    /// `reqPositions` subscribes to a real-time feed, so a holding that moves
    /// afterwards is reported as it moves. Answering only the set held when
    /// the call was made left a caller tracking positions from a snapshot
    /// that went stale on the next fill.
    pub(crate) positions_requested: AtomicBool,
    /// Order records kept back because a fill for them was still queued.
    ///
    /// A fill is read against the record, so the record cannot be freed while
    /// one is waiting. Freed on the next read of the completed orders, which
    /// is when the fill has been delivered — so the deferral costs a pass and
    /// not the rest of the session.
    pub(crate) deferred_evictions: Mutex<std::collections::HashSet<u64>>,
    /// The requests watching holdings per account or model.
    ///
    /// `positionMulti` is the same live feed as `position`, asked for under a
    /// request id and withdrawn under it. Held apart from that flag because
    /// both may be watching at once and each is answered on its own callback.
    pub(crate) positions_multi_requested: Mutex<std::collections::HashSet<i64>>,
    /// Whether this session is finished rather than merely disconnected.
    ///
    /// The engine announces a loss it is still working on and a loss it has
    /// given up on with the same event, and only records a reason for the
    /// second. Kept apart from `connected` because the two answer different
    /// questions: a caller asks the first whether a request will reach the
    /// venue now, and the pumps ask this one whether there is any point
    /// waiting for it to.
    pub(crate) session_ended: AtomicBool,
    /// Whether the caller has been told this session closed.
    ///
    /// Delivered once, and from wherever the caller drives the dispatch from:
    /// a program with an event loop of its own drives it a pass at a time and
    /// never reaches the end of `run`, which is where the notice used to be
    /// sent — so it was never told the session had ended at all.
    pub(crate) close_notified: AtomicBool,
    /// Whether this session has already waited out the replay once.
    ///
    /// An account with nothing working never sees the replay end, so the wait
    /// runs to its bound every time it is entered. Entered per id, that was
    /// three seconds on every call for an account with no working orders —
    /// which is the ordinary way an id is asked for before each order.
    pub(crate) replay_waited: AtomicBool,
    /// The number this session connected under, as the caller gave it.
    ///
    /// One session holds the account here, so this does not route anything.
    /// It is kept because some calls are answered by client id: binding orders
    /// entered elsewhere is refused for any client id but 0.
    pub(crate) client_id: AtomicI32,
    /// Receiver for engine events (disconnects, etc.).
    pub(crate) event_rx: Mutex<Option<std::sync::mpsc::Receiver<Event>>>,
    /// What this session's event channel discarded because it was full.
    ///
    /// Kept rather than handed away: counted into a total nobody holds, a
    /// program that acted on every event it saw had no way to tell that from
    /// every event there was.
    pub(crate) events_lost: Arc<std::sync::atomic::AtomicU64>,
    /// Sender for test-injected events (test-only).
    #[doc(hidden)]
    pub(crate) _test_event_tx: Mutex<Option<std::sync::mpsc::SyncSender<Event>>>,
    /// Holds the control channel's receiving end for a test-connected client.
    /// Dropping it closed the channel, so a client that reported itself
    /// connected failed every request that sends one.
    pub(crate) _test_control_rx: Mutex<Option<std::sync::mpsc::Receiver<ControlCommand>>>,
    /// Which kind of trade stream each tick-by-tick request asked for, as the
    /// number the callback states it under: 1 for the exchange's own prints
    /// and 2 for every print including those reported away from it.
    ///
    /// The trade record does not carry the kind, so it is kept per request id.
    /// Without it a caller holding both subscriptions cannot tell the two
    /// streams apart.
    pub(crate) tbt_kind: Mutex<HashMap<i64, i32>>,
    /// Option calculations asked for before the venue had stated a model for
    /// the contract, kept until it does.
    ///
    /// The venue states a model only for a contract something is watching, so
    /// a question about one nobody watches opens the watch and waits rather
    /// than being refused for having been asked first. Answered on each
    /// dispatch pass and dropped when the caller withdraws it.
    pub(crate) pending_option_calcs:
        Mutex<HashMap<i64, crate::api::client::PendingOptionCalc>>,
    /// The orders this session has seen the venue finish with, kept.
    ///
    /// The queue they arrive on empties as it is read and the venue does not
    /// send them again, so without this a second request is answered with none
    /// of them and the account reads as having completed nothing. The Rust
    /// surface keeps the same
    /// archive for the same reason.
    #[allow(clippy::type_complexity)]
    pub(crate) completed: Mutex<Vec<(
        crate::types::model::Contract,
        crate::types::model::Order,
        crate::types::model::OrderState,
    )>>,
    /// Shared subscription tracking and dispatch preparation.
    pub(crate) core: ClientCore,
}

impl Drop for EClient {
    fn drop(&mut self) {
        // Both taken out of their locks first. A guard built in the scrutinee
        // is held for the whole body, and these bodies block: the sends are
        // bounded, and the join waits on the engine thread.
        let tx = self.control_tx.lock().unwrap().clone();
        if let Some(tx) = tx {
            // Dropping the client ends the session, so the venue is told. The
            // channel is bounded and these wait on a hot loop that is behind,
            // and dealloc runs with the GIL held — so detach for them, the same
            // way the join below does, and for the same reason.
            let sent = Python::try_attach(|py| {
                py.detach(|| {
                    let _ = tx.send(ControlCommand::Logout);
                    let _ = tx.send(ControlCommand::Shutdown);
                });
            });
            if sent.is_none() {
                let _ = tx.send(ControlCommand::Logout);
                let _ = tx.send(ControlCommand::Shutdown);
            }
        }
        let thread = self._thread.lock().unwrap().take();
        if let Some(h) = thread {
            // A wedged engine never returns from join. Detach so
            // that stall parks this thread, not the whole interpreter
            // try_attach, not attach: dealloc also runs during
            // interpreter shutdown (and `wrapper` commonly points back at
            // the object embedding this EClient, so the cyclic GC — not
            // just refcounting — can be the one calling drop), and attach
            // is unsound there. Fall back to a plain join in that case,
            // same as before this fix.
            let mut h = Some(h);
            Python::try_attach(|py| {
                if let Some(h) = h.take() {
                    py.detach(|| { let _ = h.join(); });
                }
            });
            if let Some(h) = h {
                let _ = h.join();
            }
        }
    }
}

/// Narrow a caller's req_id to the width the request carries on the wire,
/// refusing rather than truncating.
///
/// What need not fit is an id a caller numbers themselves, and
/// a truncated one answers under a number they never used.
pub(crate) fn wire_req_id(req_id: i64) -> PyResult<u32> {
    crate::api::client::wire_req_id(req_id)
        .map_err(|refusal| PyRuntimeError::new_err(refusal.message))
}

/// Narrow another signed value the caller stated to the width its request
/// carries, refusing rather than wrapping.
///
/// Same reason as `wire_req_id`, for the fields beside it. A contract id or a
/// count below zero wraps to a value above four billion, which asks about the
/// wrong contract or for every row the venue holds.
pub(crate) fn wire_u32(what: &str, value: i64) -> PyResult<u32> {
    u32::try_from(value).map_err(|_| {
        PyRuntimeError::new_err(format!(
            "{what} {value} is outside the range this request can carry (0..={})", u32::MAX,
        ))
    })
}

/// Adapt a Python callable to the second-factor [`CodeProvider`] the login gate
/// calls. The gate runs it on a thread of its own, so it takes the GIL itself;
/// `connect` has released it for the whole login. A raising callback becomes an
/// error the gate reports, not a panic.
fn code_provider_from_py(cb: Py<PyAny>) -> CodeProvider {
    Arc::new(move |challenge: IbKeyChallenge| {
        Python::attach(|py| {
            let factor = match challenge.factor {
                SecondFactor::IbKeyChallengeResponse => "ibkey",
                SecondFactor::AuthenticatorCode => "authenticator",
            };
            cb.call1(py, (factor, challenge.display_id, challenge.avth_url))
                .and_then(|code| code.extract::<String>(py))
                .map_err(|e| std::io::Error::other(e.to_string()))
        })
    })
}

#[pymethods]
impl EClient {

    #[new]
    #[pyo3(signature = (wrapper))]
    fn new(wrapper: Py<PyAny>) -> Self {
        Self {
            client_id: AtomicI32::new(0),
            wrapper,
            shared: Mutex::new(None),
            control_tx: Mutex::new(None),
            next_order_id: AtomicU64::new(0),
            _thread: Mutex::new(None),
            account_id: Mutex::new(None),
            accounts: Mutex::new(Vec::new()),
            connected: AtomicBool::new(false),
            positions_requested: AtomicBool::new(false),
            deferred_evictions: Mutex::new(std::collections::HashSet::new()),
            positions_multi_requested: Mutex::new(std::collections::HashSet::new()),
            session_ended: AtomicBool::new(false),
            close_notified: AtomicBool::new(false),
            replay_waited: AtomicBool::new(false),
            event_rx: Mutex::new(None),
            events_lost: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            _test_event_tx: Mutex::new(None),
            _test_control_rx: Mutex::new(None),
            tbt_kind: Mutex::new(HashMap::new()),
            pending_option_calcs: Mutex::new(HashMap::new()),
            completed: Mutex::new(Vec::new()),
            core: ClientCore::new(),
        }
    }

    /// Connect to IB and start the engine.
    ///
    /// Live logins (``paper=False``) enter a second-factor approval window and
    /// **block** until the factor is approved (mobile push) or the deadline
    /// fires (``ib_key_timeout_secs``, default ~18 min). This is a human
    /// approval gate, not a hang. To bound or avoid it: use ``paper=True``, pass
    /// a smaller ``ib_key_timeout_secs``, or run ``connect()`` on a worker
    /// thread with your own timeout. Paper logins skip the gate entirely. Set
    /// ``RUST_LOG=info`` to see a log line when the wait begins.
    ///
    /// ``code_provider`` answers that factor with a typed code instead:
    /// ``code_provider(factor, display_id, avth_url) -> str``, where ``factor``
    /// is ``"ibkey"`` (return the code shown for ``display_id``) or
    /// ``"authenticator"`` (return the account's current code; ``display_id``
    /// and ``avth_url`` are empty). An authenticator account has no push to
    /// fall back to and cannot log in without this. It is called once, on a
    /// thread of its own, and holds the GIL while it runs — return the code,
    /// don't block on input. It is asked once and the login carries whatever it
    /// returns; what the venue does with a wrong code has not been exercised
    /// from here.
    ///
    /// Multiple ``EClient`` instances can run concurrently in one process; each
    /// owns its own state, sockets, and engine thread, and ``connect()`` does
    /// not serialize across instances. If you pin engines via ``core_id``, give
    /// each a distinct value.
    ///
    /// `port` is taken and not applied. The session connects to the venue
    /// directly, so there is no local socket to name a port on.
    #[pyo3(signature = (host=crate::config::CCP_HOSTS[0].to_string(), port=0, client_id=0, username="".to_string(), password="".to_string(), paper=true, core_id=None, ib_key_timeout_secs=None, ib_key_token_sub_type=None, code_provider=None, readonly=false, settings=None, session_file=None))]
    fn connect(
        &self,
        py: Python<'_>,
        host: String,
        port: i32,
        client_id: i32,
        username: String,
        password: String,
        paper: bool,
        core_id: Option<usize>,
        ib_key_timeout_secs: Option<u64>,
        ib_key_token_sub_type: Option<String>,
        code_provider: Option<Py<PyAny>>,
        readonly: bool,
        // What this session runs under, by the names `ibx.configure` uses.
        // Stated here it belongs to this session; stated there it is the
        // process's, and is what a session that states nothing falls back to.
        settings: Option<std::collections::HashMap<String, String>>,
        // Where to keep this session so the next start does not need a new
        // logon. A request naming a session the venue still holds is answered
        // with a challenge rather than a full handshake, so a restart needs no
        // approval and does not count as a new login. Owner-only, sealed with
        // the password, and bound to this account and this kind of session.
        session_file: Option<String>,
    ) -> PyResult<()> {
        let username_for_resume = username.clone();
        let username_for_session = username.clone();
        let password_for_resume = password.clone();
        // Claimed, not read. Read and then set, two callers racing here both
        // find it clear and both build an engine — and the second replaces the
        // first, which goes on running with a live socket and a second logon
        // the account did not ask for. The venue bumps the older session when
        // that happens, so a caller racing itself knocks over its own.
        if self
            .connected
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(PyRuntimeError::new_err("Already connected"));
        }
        // Given back on every way out that is not a session, including the
        // ones a later edit adds.
        let mut claim = Connecting { flag: &self.connected, kept: false };
        // Past that refusal there can still be an engine running: a
        // market-data farm that went away flips this flag while the trading
        // connection is healthy and the ladder is still climbing, and
        // answering that notice by connecting again is how callers of the
        // reference client are written. Left running, the old engine loses its
        // sender and takes the path that sends no logout and withdraws nothing
        // it opened, so a session stays open at the venue while this one runs.
        // Stopped and joined before the state is reset, so the engine that is
        // going cannot write into what the new session has been given.
        self.stop_engine(py);
        self.forget_last_session();

        // Set before anything is sent, so there is no window in which the
        // session is open and the refusal is not yet in force.
        self.core.set_readonly(readonly);

        let code_provider = code_provider.map(code_provider_from_py);

        let config = GatewayConfig {
            settings: std::sync::Arc::new(
                crate::python::settings_from(settings.unwrap_or_default())
                    .map_err(PyRuntimeError::new_err)?,
            ),
            username,
            password: zeroize::Zeroizing::new(password),
            host,
            paper,
            accept_invalid_certs: false,
            ib_key_timeout_secs: ib_key_timeout_secs
                .unwrap_or(crate::auth::session::IB_KEY_DEFAULT_TIMEOUT_SECS),
            ib_key_token_sub_type: ib_key_token_sub_type
                .unwrap_or_else(|| crate::auth::session::IB_KEY_DEFAULT_TOKEN_SUB_TYPE.into()),
            code_provider,
            // Whatever was left in the file the caller named. A file that
            // cannot be read is a slower start, not a failed one: the password
            // is still here, and a file whose whole point is to avoid needing
            // a person defeats itself by raising at one.
            resume: session_file.as_ref().and_then(|path| {
                crate::auth::resume::load(
                    std::path::Path::new(path), &username_for_resume, &password_for_resume, paper,
                )
            }),
        };

        let result = py.detach(|| Gateway::connect(&config));
        let Session { gateway: gw, market_data: farm_conn, trading: ccp_conn, historical: hmds_conn, security_definition: secdef_conn } = result
            .map_err(|e| PyRuntimeError::new_err(format!("Connection failed: {e}")))?;

        crate::client_core::remember_session(
            session_file.as_deref().map(std::path::Path::new),
            &password_for_resume,
            &gw,
            &username_for_session,
            paper,
        );

        // Here rather than on the way in: a session can end without being
        // closed, so this is reached with the last one's state still held, but
        // until the connection above answered there was still a session in
        // recovery whose routing this would have taken away from it.
        self.forget_last_session();

        *self.account_id.lock().unwrap() = Some(gw.account_id.clone());
        *self.accounts.lock().unwrap() = gw.accounts.clone();
        let shared = Arc::new(SharedState::new());
        shared.set_settings(config.settings.clone());
        self.core.set_registration_timeout(config.settings.registration_timeout);
        gw.populate_init_data(&shared);

        let connect_host = config.host.clone();
        let connect_username = config.username.clone();
        let connect_password = config.password.clone();
        let connect_paper = config.paper;
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(256);
        let (hot_loop, control_tx) = crate::engine::hot_loop::HotLoop::for_session(
            gw,
            shared.clone(),
            // This session's own count of what it discarded, so a program with
            // two sessions is not told about the other one's — and held here,
            // so it can be asked.
            Some(crate::engine::hot_loop::EventSink::new(
                event_tx,
                Arc::clone(&self.events_lost),
            )),
            farm_conn, ccp_conn, hmds_conn, secdef_conn, core_id,
            crate::gateway::CallerAuth {
                // The settings the session opened under, not the defaults. Left
                // default here, a reconnect announced a different build, locale
                // and timezone than the connect did, and asked for every
                // execution the account holds where the caller asked for
                // today's. The Rust surface states the same thing in
                // `caller_auth`; this path had to state it too.
                settings: config.settings.clone(),
                host: connect_host,
                username: connect_username,
                password: connect_password,
                paper: connect_paper,
                code_provider: config.code_provider.clone(),
                ib_key_timeout_secs: config.ib_key_timeout_secs,
                ib_key_token_sub_type: config.ib_key_token_sub_type.clone(),
            },
        );

        let handle = thread::Builder::new()
            .name("ib-engine-hotloop".into())
            .spawn(move || {
                hot_loop.run_with_panic_recovery();
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to spawn hot loop: {e}")))?;

        *self.shared.lock().unwrap() = Some(shared);
        *self.control_tx.lock().unwrap() = Some(control_tx);
        *self.event_rx.lock().unwrap() = Some(event_rx);
        // Counted from whatever the venue names as working, once it has;
        // nothing is carried over from the last run.
        self.next_order_id.store(0, Ordering::Relaxed);
        *self._thread.lock().unwrap() = Some(handle);
        self.session_ended.store(false, Ordering::Release);
        self.close_notified.store(false, Ordering::Release);
        claim.kept = true;

        self.client_id.store(client_id, Ordering::Release);
        let _ = port; // kept for the reference client's signature

        // Fire initial callbacks synchronously, matching official Python ibapi
        // where connect_ack signals "socket ready" before run() is called.
        // Announcements, not permission. The session is up and its engine is
        // running; a handler that raises here — starting work in
        // `next_valid_id` is the ordinary way to write one — must not make
        // this report failure on a session that is live.
        self.notify(py, "connect_ack", ());
        self.notify(py, "managed_accounts", (self.accounts_csv().as_str(),));
        // The id is announced once and a program starts numbering from it, so
        // it is worth the wait: the venue names what the account has used just
        // after the connection is made, and announced before that lands this
        // is one a fill spent long ago. A program that trusts the announcement
        // — which is the ordinary way to write one — then has its first order
        // refused as a duplicate, and nothing about the refusal points here.
        self.wait_for_the_replay(py);
        self.notify(py, "next_valid_id", (self.stated_order_id() as i64,));

        Ok(())
    }

    /// Answer to the name the reference client gives a method as well as the
    /// name this one gives it.
    ///
    /// The reference client names its methods with the words run together, and
    /// code written against it calls them that way. Every method here is named
    /// with underscores, so that code stopped at its first call with the method
    /// simply not there. Rather than write a second name for each of ninety
    /// methods, the run-together name is translated back and the method this
    /// client already has is returned.
    ///
    /// Only reached when the attribute was not found, so it costs nothing on
    /// the name this client has always used.
    fn __getattr__(slf: Bound<'_, Self>, name: &str) -> PyResult<Py<PyAny>> {
        // The two the reference client opens with an `e`. Its `eConnect` takes
        // a host, a port and a client id, which are this call's first three
        // arguments in that order, so a program written against it reaches the
        // same call the same way.
        super::contract::by_reference_name(
            slf.as_any(),
            name,
            &[("eConnect", "connect"), ("eDisconnect", "disconnect")],
        )
    }

    /// Disconnect from IB.
    fn disconnect(&self, py: Python<'_>) -> PyResult<()> {
        self.stop_engine(py);
        self.connected.store(false, Ordering::Release);
        self.session_ended.store(true, Ordering::Release);
        // Reset per-session state so connect() can be called again.
        *self.shared.lock().unwrap() = None;
        *self.control_tx.lock().unwrap() = None;
        *self.event_rx.lock().unwrap() = None;
        self.forget_last_session();
        Ok(())
    }

    /// Check if connected.
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// How many engine events this session's channel discarded.
    ///
    /// The engine never waits on a reader — a session that stalled on one would
    /// stop carrying market data — so an event arriving at a full channel is
    /// dropped. A program that acted on every fill it saw needs to know the
    /// difference between that and every fill there was. Zero for a session
    /// whose reader kept up.
    fn events_lost(&self) -> u64 {
        self.events_lost.load(Ordering::Acquire)
    }

    /// Run the event loop.
    /// Deliver everything waiting, once, and return.
    ///
    /// `run` owns the thread it is called on, which a program with an event
    /// loop of its own cannot give it: an asyncio framework has to drive the
    /// callbacks from its own loop, and a blocking loop leaves it nowhere to
    /// stand. This is one pass of the same dispatch.
    fn poll(&self, py: Python<'_>) -> PyResult<()> {
        // Not gated on the connection. A session the engine is still
        // rebuilding is disconnected and not over, and the event saying it
        // came back arrives on this same pump — so a pump that stopped at the
        // loss could never deliver it, and the caller stayed stood down on a
        // session that had recovered.
        let Some(shared) = self.shared.lock().unwrap().clone() else {
            return Ok(());
        };
        let delivered = self.dispatch_once(py, &shared);
        self.tell_the_caller_it_closed(py);
        delivered
    }

    /// Deliver callbacks until the session ends.
    ///
    /// Blocks the calling thread. Everything a program receives arrives from
    /// here, so it runs on a thread of its own or is the last call a program
    /// makes. `poll` does one pass instead, for a program that owns its loop.
    fn run(&self, py: Python<'_>) -> PyResult<()> {
        if !self.connected.load(Ordering::Acquire) {
            return Err(PyRuntimeError::new_err("Not connected. Call connect() first."));
        }

        // Event loop — wake immediately on data, or check signals every 1ms.
        //
        // Ends when the session does, not when it goes quiet. The engine keeps
        // rebuilding a lost connection past the 1100 notice and announces 1102
        // when the transports carry again, so the loop must outlive 1100 to
        // deliver the recovery.
        while !self.session_ended.load(Ordering::Relaxed) {
            py.check_signals()?;

            let shared = match self.shared.lock().unwrap().clone() {
                Some(s) => s,
                None => break,
            };

            self.dispatch_once(py, &shared)?;

            // Wait for hot loop notification instead of fixed sleep.
            // Releases GIL while waiting; wakes immediately when data arrives.
            let shared_ref = shared.clone();
            py.detach(move || {
                shared_ref.wait_for_data(std::time::Duration::from_millis(1));
            });
        }

        // The same place `poll` says it from, so a caller driving this a pass
        // at a time hears it too, and neither hears it twice.
        self.tell_the_caller_it_closed(py);

        Ok(())
    }

    /// Get the account ID.
    fn get_account_id(&self) -> String {
        self.account()
    }

    /// Another session that already held this account when this one connected.
    ///
    /// `None` when this session is alone. Otherwise where the other one
    /// connected from, when it logged in, and whether this session is held to
    /// reading only because the other has the account.
    ///
    /// Worth asking before starting work: the venue permits one logon at a time
    /// and takes the account from the older session without saying which it
    /// dropped, so a second client reads as data that stops arriving.
    fn competing_session(&self) -> PyResult<Option<(String, String, bool)>> {
        Ok(self.shared_state()?.reference.competing_session())
    }

    /// Session ID surfaced to webapp REST clients as `x-ccp-session-id`.
    fn ccp_session_id(&self) -> PyResult<String> {
        Ok(self.shared_state()?.reference.ccp_session_id())
    }

    /// Logical-name → host URL lookup from the MiscUrls block of the logon
    /// response (e.g. `region_dam`). `None` when the logon did not carry the
    /// key.
    fn misc_url(&self, key: &str) -> PyResult<Option<String>> {
        Ok(self.shared_state()?.reference.misc_url(key))
    }
}

/// Call a callback on a wrapper under the reference client's name for it where
/// the caller defined one, and under this client's name otherwise.
pub(crate) fn call_named<'py, A>(
    py: Python<'py>,
    wrapper: &Py<PyAny>,
    name: &str,
    args: A,
) -> PyResult<()>
where
    A: pyo3::call::PyCallArgs<'py> + Clone,
{
    let alias = ibapi_name(name);
    if alias != name
        && let Ok(f) = wrapper.getattr(py, alias.as_str())
        && !answered_by_the_base(py, &f, &alias)
    {
        f.call1(py, args.clone())?;
        return Ok(());
    }
    wrapper.call_method1(py, name, args)?;
    Ok(())
}

/// Whether what a wrapper answered under the reference client's name is the
/// base class's own answer rather than the caller's.
///
/// The base answers to that name too, so that `super().tickPrice(...)` finds
/// something — `super` reads each class's own contents and asks no hook, so the
/// name has to be there. That answer also stands in front of a caller who
/// overrode only this client's spelling, and taking it would report the
/// callback delivered when nobody received it.
///
/// Known by `__func__`, which the base answers with a Python function to carry
/// and which the do-nothing's own bound form has none of — that missing link is
/// what an earlier guard here tried to read and never could. Anything else a
/// wrapper answers with, its own method or something put on the instance, is
/// the caller's.
fn answered_by_the_base(py: Python<'_>, f: &Py<PyAny>, alias: &str) -> bool {
    let Ok(func) = f.getattr(py, pyo3::intern!(py, "__func__")) else {
        return false;
    };
    py.get_type::<super::wrapper::EWrapper>()
        .getattr(alias)
        .is_ok_and(|base| func.is(&base))
}

/// The reference client's spelling of a callback name: its words run together
/// with each after the first capitalised.
fn ibapi_name(snake: &str) -> String {
    // Three it spells with its letters run together instead. Built the ordinary
    // way `real_time_bar` is `realTimeBar`, which that client does not declare,
    // so a wrapper written against it declares `realtimeBar`, nothing answers to
    // the name built here, and the bar goes to this crate's own do-nothing
    // rather than to the caller. Nothing is raised and nothing is logged.
    match snake {
        "real_time_bar" => return "realtimeBar".to_string(),
        "receive_fa" => return "receiveFA".to_string(),
        "replace_fa_end" => return "replaceFAEnd".to_string(),
        _ => {}
    }
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}


/// Holds the connected flag for the length of a connect that has not finished.
///
/// The refusal and the flag it reads are one step, so the flag is set before
/// the session exists. Anything short of a session gives it back.
struct Connecting<'a> {
    flag: &'a AtomicBool,
    kept: bool,
}

impl Drop for Connecting<'_> {
    fn drop(&mut self) {
        if !self.kept {
            self.flag.store(false, Ordering::Release);
        }
    }
}

impl EClient {
    /// Stop the engine this client is running and wait for it.
    ///
    /// Both taken out of their locks first, as in `Drop`: the sends are bounded
    /// and the join waits on the engine, so a guard spanning either blocks
    /// every other thread that needs the same lock. Detached for both — the
    /// channel is bounded and a wedged engine never returns from a join, so
    /// holding the GIL across either stalls every Python thread rather than
    /// this call alone.
    fn stop_engine(&self, py: Python<'_>) {
        let tx = self.control_tx.lock().unwrap().take();
        if let Some(tx) = tx {
            // The session is ending, so the venue is told before the engine
            // stops. Left to notice its sender went away, the loop takes the
            // path that sends no logout and withdraws nothing it opened.
            py.detach(|| {
                let _ = tx.send(ControlCommand::Logout);
                let _ = tx.send(ControlCommand::Shutdown);
            });
        }
        let thread = self._thread.lock().unwrap().take();
        if let Some(h) = thread {
            py.detach(|| {
                let _ = h.join();
            });
        }
    }

    /// Drop everything the last session held.
    ///
    /// Called both where a session is closed and where the next one is opened,
    /// because a session can end without being closed and what it held would
    /// otherwise be answered under request ids the next session never gave
    /// out: a model arriving for a contract nobody here watches, positions
    /// nobody here asked for, another account's completed orders, an eviction
    /// aimed at an order record this session now owns.
    fn forget_last_session(&self) {
        *self.account_id.lock().unwrap() = None;
        self.accounts.lock().unwrap().clear();
        self.positions_requested.store(false, Ordering::Release);
        self.positions_multi_requested.lock().unwrap().clear();
        self.deferred_evictions.lock().unwrap().clear();
        self.tbt_kind.lock().unwrap().clear();
        self.pending_option_calcs.lock().unwrap().clear();
        self.completed.lock().unwrap().clear();
        // Counted per session, so a caller reading it is told what this
        // session lost rather than a total carried over from the last.
        self.events_lost.store(0, Ordering::Relaxed);
        self.replay_waited.store(false, Ordering::Relaxed);
        self.core.reset();
    }

    /// Tell the caller a request will not be sent, under the number the
    /// reference client reports that class of refusal under.
    ///
    /// Reported rather than raised: the reference client answers a request it
    /// refuses on the error callback and returns, so a program written against
    /// it handles refusals there and nowhere else.
    pub(crate) fn report_refusal(
        &self,
        py: Python<'_>,
        req_id: i64,
        refusal: crate::error_codes::Refusal,
    ) -> PyResult<()> {
        let _ = self.wrapper.call_method(
            py,
            "error",
            (req_id, raised_now(), refusal.code, refusal.message, ""),
            None,
        );
        Ok(())
    }

    /// The control channel, or nothing and the caller told why.
    ///
    /// A request issued before connecting is answered on the error callback
    /// and the call returns normally, which is what the reference client does.
    /// Raising instead made a caller written against that client take a
    /// different path here than it takes there.
    pub(crate) fn tx_or_report(&self, req_id: i64) -> Option<SyncSender<ControlCommand>> {
        // Taken before the arms run. The `None` arm calls user code, and a
        // handler that disconnects or issues another request would wait on
        // this same lock while holding the GIL.
        let tx = self.control_tx.lock().unwrap().clone();
        match tx {
            Some(tx) => Some(tx),
            None => {
                Python::attach(|py| {
                    let _ = self.wrapper.call_method(
                        py,
                        "error",
                        (req_id, raised_now(), NOT_CONNECTED_CODE, "Not connected", ""),
                        None,
                    );
                });
                None
            }
        }
    }

    pub(crate) fn tx(&self) -> PyResult<SyncSender<ControlCommand>> {
        self.control_tx.lock().unwrap().clone()
            .ok_or_else(|| PyRuntimeError::new_err("Not connected"))
    }

    /// Hold until the venue has named what the account already has.
    ///
    /// The working orders and the executions behind the rest arrive unprompted
    /// after a connect, and both raise the mark an id is floored at. Asked
    /// before they land, the floor is nothing and the id handed out is one a
    /// fill spent long ago, which the venue refuses as a duplicate — so the
    /// first order of a session is the one that cannot be placed. Bounded the
    /// same way and for the same reason as the open-order replay beside it: an
    /// account with nothing working never sees the replay end.
    pub(crate) fn wait_for_the_replay(&self, py: Python<'_>) {
        // Once per session, however many ids are asked for. What the wait is
        // for is the floor being raised before the first id goes out, and that
        // has either happened by the time the first wait returns or it is not
        // going to: an account with nothing working never sees the replay end,
        // so waiting again only spends the bound again.
        if self.replay_waited.load(Ordering::Acquire) {
            return;
        }
        let Ok(shared) = self.shared_state() else { return };
        for _ in 0..300 {
            if shared.orders.replay_done() {
                break;
            }
            py.detach(|| std::thread::sleep(std::time::Duration::from_millis(10)));
        }
        self.replay_waited.store(true, Ordering::Release);
    }

    /// The id a caller may next place under, without taking it.
    ///
    /// One past the highest id the venue has named an order under. Stated
    /// rather than reserved, as the reference client states it: the caller
    /// places under it, and the taking happens then.
    ///
    /// An id is spent for good once a fill has spent it — only a withdrawn one
    /// comes free. The venue names the live orders at connect and replays the
    /// executions behind the rest, and both raise the mark, so what this
    /// answers is safe once that has arrived. Asked before it has, it answers
    /// from a mark of nothing and names an id a fill spent long ago, which the
    /// venue refuses as a duplicate. Seen on a session that connected and
    /// asked in the same breath.
    pub(crate) fn stated_order_id(&self) -> u64 {
        let floor = self.shared.lock().unwrap().as_ref()
            .map(|shared| shared.orders.working_id_watermark() + 1)
            .unwrap_or(1);
        let id = self.next_order_id.load(Ordering::Acquire).max(floor);
        crate::bridge::say_if_past_a_request_id(id);
        id
    }

    /// Hand out the next order id.
    ///
    /// Floored at one past the highest id the venue has named an order under,
    /// from any session. Nothing is kept between runs, because there is
    /// nothing a run knows that the next one will not be told — but it is told
    /// after connecting rather than during it, so an id asked for in the same
    /// breath as the connection is floored at nothing. See `stated_order_id`.
    pub(crate) fn take_order_id(&self, py: Python<'_>) -> u64 {
        self.wait_for_the_replay(py);
        let floor = self.stated_order_id();
        let mut held = self.next_order_id.load(Ordering::Acquire);
        loop {
            let id = held.max(floor);
            match self.next_order_id.compare_exchange_weak(
                held, id + 1, Ordering::AcqRel, Ordering::Acquire,
            ) {
                // Against what is handed out, not against the floor it was
                // taken from: the counter can already be past the floor, and
                // two callers racing here take ids either side of the line.
                Ok(_) => {
                    crate::bridge::say_if_past_a_request_id(id);
                    return id;
                }
                Err(seen) => held = seen,
            }
        }
    }

    /// Send a control command to the engine. `control_tx` is a sync_channel(64)
    /// channel: a full queue is normal backpressure (the hot loop is behind,
    /// not gone) and `send` is meant to wait for it to drain, so the send
    /// itself stays blocking. What must not happen is waiting with the GIL
    /// held, stalling every Python thread instead of just this call
 ///, so the wait runs detached, and only the actual send
    /// crosses that boundary; `cmd` must already be a plain owned value by
    /// the time it's built (never touching Python state once detached).
    pub(crate) fn send_control(py: Python<'_>, tx: &SyncSender<ControlCommand>, cmd: ControlCommand) -> PyResult<()> {
        // The error carries the command back, which is the whole command by
        // value. Nothing here wants it returned, so it is described and dropped
        // while still detached rather than moved across the boundary.
        py.detach(|| tx.send(cmd).map_err(|e| e.to_string()))
            .map_err(|e| PyRuntimeError::new_err(format!("Engine stopped: {e}")))
    }

    /// Clone the shared state Arc, or return "Not connected".
    pub(crate) fn shared_state(&self) -> PyResult<Arc<SharedState>> {
        self.shared.lock().unwrap().clone()
            .ok_or_else(|| PyRuntimeError::new_err("Not connected"))
    }

    /// Return the account id (empty string if not connected).
    pub(crate) fn account(&self) -> String {
        self.account_id.lock().unwrap().clone().unwrap_or_default()
    }

    /// Call a callback on the caller's wrapper, under the name the reference
    /// client gives it as well as the name this one does.
    ///
    /// The two clients spell callback names differently. A wrapper written
    /// against the reference client defines only that client's names, and a
    /// call under this client's names reaches the base-class no-op instead.
    ///
    /// The reference name is tried first and this client's name second, so a
    /// wrapper written against either is reached.
    pub(crate) fn callback<'py, A>(
        &self,
        py: Python<'py>,
        name: &str,
        args: A,
    ) -> PyResult<()>
    where
        A: pyo3::call::PyCallArgs<'py> + Clone,
    {
        call_named(py, &self.wrapper, name, args)
    }

    /// Say the session closed, once, however the caller drives the dispatch.
    ///
    /// Not a method on the Python object: it is this client telling the
    /// caller, not something the caller calls.
    fn tell_the_caller_it_closed(&self, py: Python<'_>) {
        if self.session_ended.load(Ordering::Acquire)
            && !self.close_notified.swap(true, Ordering::AcqRel)
        {
            self.notify(py, "connection_closed", ());
        }
    }

    /// Tell the caller something, and do not let what it raises decide the
    /// call that is telling it.
    ///
    /// For the notices a session sends on its way up. They are announcements,
    /// not permission: the session is already open and its engine already
    /// running by the time they go out, so a handler that raises must not make
    /// opening the session report failure. A caller told that would not close
    /// what it believes it never opened, and would find the next attempt
    /// refused for being connected already.
    ///
    /// The dispatch loop holds the same rule for the same reason, so one
    /// raising handler cannot take the loop down with it.
    /// Hand a caller one of the answers to a call it made, and let the rest of
    /// the answer through whatever it does with it.
    ///
    /// The dispatch loop already treats a raising callback this way: what the
    /// caller wrote is the caller's problem, and the batch behind it is still
    /// owed. Delivered with `?` instead, one raising callback abandoned the
    /// answers behind it and the end-of-answer that closes it — so a program
    /// waiting to be told the answer was complete waited for good, and the
    /// exception came back out of a request call, which is somewhere the
    /// reference client never raises one.
    ///
    /// Not an ordinary exception — an interrupt, the interpreter going down —
    /// still ends the call, because nothing here can carry on through that.
    pub(crate) fn deliver<'py, A>(&self, py: Python<'py>, name: &str, args: A) -> PyResult<()>
    where
        A: pyo3::call::PyCallArgs<'py> + Clone,
    {
        if let Err(e) = self.callback(py, name, args) {
            if !e.is_instance_of::<pyo3::exceptions::PyException>(py) {
                return Err(e);
            }
            log::error!("Python callback {name}() raised: {e}");
        }
        Ok(())
    }

    pub(crate) fn notify<'py, A>(&self, py: Python<'py>, name: &str, args: A)
    where
        A: pyo3::call::PyCallArgs<'py> + Clone,
    {
        if let Err(e) = self.callback(py, name, args) {
            if e.is_instance_of::<pyo3::exceptions::PyException>(py) {
                log::error!("Python callback {name}() raised: {e}");
            } else {
                // Not an ordinary exception — an interrupt, or the interpreter
                // going down. Nothing here can carry on through that.
                e.restore(py);
            }
        }
    }

    /// Every account this login holds, comma separated, which is the shape
    /// the reference client answers `managed_accounts` in. Falls back to the
    /// default account so a client built by hand still answers something.
    pub(crate) fn accounts_csv(&self) -> String {
        let accounts = self.accounts.lock().unwrap();
        if accounts.is_empty() { self.account() } else { accounts.join(",") }
    }

    /// Find instrument ID for a contract, registering if needed. The hot
    /// loop can take up to `REGISTRATION_TIMEOUT` to reply, so the round
    /// trip runs with the GIL released: otherwise a slow reply stalls every
    /// Python thread, not just this call.
    pub(crate) fn find_or_register_instrument(&self, py: Python<'_>, contract: &Contract) -> PyResult<u32> {
        let tx = self.tx()?;
        let con_id = contract.con_id;
        let symbol = contract.symbol.clone();
        let exchange = contract.exchange.clone();
        let sec_type = contract.sec_type.clone();
        let identity = crate::types::model::contract_identity(
            &contract.last_trade_date_or_contract_month, contract.strike,
            &contract.right, &contract.multiplier, &contract.currency,
        );
        py.detach(|| self.core.find_or_register_instrument(
            &tx, con_id, &symbol, &exchange, &sec_type, &identity,
        ))
        .map_err(|refusal| PyRuntimeError::new_err(refusal.message))
    }
}

/// Register EClient on the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<EClient>()?;
    Ok(())
}

/// When this client raised something itself, in milliseconds.
///
/// The reference client stamps the trouble it raises before anything reaches
/// the venue — a call made with no session, a request it will not send — and
/// leaves the field at zero for trouble the venue stated.
pub(crate) fn raised_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::Python;

    /// A callback reaches the caller under the name the reference client gives
    /// it, including the three that client spells with its letters run together.
    ///
    /// Built by capitalising after each underscore, `real_time_bar` is
    /// `realTimeBar`. That client declares `realtimeBar`, so a wrapper written
    /// against it declares `realtimeBar` too, and a bar sent to the name built
    /// here reaches nobody — no exception, no log line, no bar.
    #[test]
    fn a_callback_is_named_the_way_the_reference_client_names_it() {
        assert_eq!(ibapi_name("real_time_bar"), "realtimeBar");
        assert_eq!(ibapi_name("receive_fa"), "receiveFA");
        assert_eq!(ibapi_name("replace_fa_end"), "replaceFAEnd");

        // The ordinary rule still holds for every other callback.
        assert_eq!(ibapi_name("tick_price"), "tickPrice");
        assert_eq!(ibapi_name("connection_closed"), "connectionClosed");
        assert_eq!(ibapi_name("order_status"), "orderStatus");
        assert_eq!(ibapi_name("error"), "error");
    }

    /// A client that has not connected holds no session, and says so on every
    /// question about one.
    #[test]
    fn eclient_default_state() {
        Python::initialize();
        Python::attach(|py| {
            let client = Py::new(py, EClient::new(recording_wrapper(py))).unwrap();
            assert!(!client.get().connected.load(Ordering::Relaxed));
            assert_eq!(client.get().account(), "");
            assert!(client.get().accounts.lock().unwrap().is_empty());
            assert!(client.get().shared.lock().unwrap().is_none());
            assert_eq!(client.get().events_lost.load(Ordering::Acquire), 0);

            // And a pump on it does nothing rather than dispatching against a
            // session that is not there.
            client.call_method0(py, "poll").unwrap();
            let err = client.call_method0(py, "run").unwrap_err();
            assert!(err.to_string().contains("Not connected"), "got {err}");
        });
    }

    /// The gate hands the callback the factor it is asking for, and an
    /// authenticator account has no push to fall back on if the answer is
    /// wrong, so the label and the argument order are the whole contract.
    #[test]
    fn code_provider_passes_the_challenge_and_returns_the_code() {
        Python::initialize();
        Python::attach(|py| {
            let echo = py
                .eval(c"lambda factor, display_id, avth_url: f'{factor}/{display_id}/{avth_url}'", None, None)
                .unwrap()
                .unbind();
            let provider = code_provider_from_py(echo);

            let code = provider(IbKeyChallenge {
                factor: SecondFactor::AuthenticatorCode,
                display_id: String::new(),
                avth_url: String::new(),
            }).unwrap();
            assert_eq!(code, "authenticator//");

            let code = provider(IbKeyChallenge {
                factor: SecondFactor::IbKeyChallengeResponse,
                display_id: "AB12".into(),
                avth_url: "https://clientam.com/x".into(),
            }).unwrap();
            assert_eq!(code, "ibkey/AB12/https://clientam.com/x");

            // A raising callback comes back as an error carrying its message.
            // Escaping as a panic instead would leave the gate reporting only
            // that the provider died.
            let boom = py.eval(c"lambda *a: (_ for _ in ()).throw(ValueError('no code'))", None, None).unwrap().unbind();
            let err = code_provider_from_py(boom)(IbKeyChallenge::default()).unwrap_err();
            assert!(err.to_string().contains("no code"), "got {err}");
        });
    }

    // ── Parity with the Rust EClient ──
    //
    // Most callers reach this engine through Python, so a request the Rust
    // surface carries and this one drops is a defect the majority of users hit.

    use crate::types::model::{Contract as ApiContract, Order as ApiOrder};
    use crate::types::PositionInfo;

    /// A wrapper that keeps every callback it is handed, so a test can read
    /// back exactly what crossed the boundary.
    fn recording_wrapper(py: Python<'_>) -> Py<PyAny> {
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
        ns.get_item("w").unwrap().unwrap().unbind()
    }

    /// A connected client whose engine is a channel the test reads.
    fn wired_client(
        py: Python<'_>,
    ) -> (Py<EClient>, std::sync::mpsc::Receiver<ControlCommand>, Arc<SharedState>, Py<PyAny>) {
        let w = recording_wrapper(py);
        let client = EClient::new(w.clone_ref(py));
        let shared = Arc::new(SharedState::new());
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        *client.shared.lock().unwrap() = Some(shared.clone());
        *client.control_tx.lock().unwrap() = Some(tx);
        *client.account_id.lock().unwrap() = Some("DU123".into());
        client.connected.store(true, Ordering::Release);
        (Py::new(py, client).unwrap(), rx, shared, w)
    }

    /// The engine writes down why a session finished; the notice saying so
    /// goes out on a channel that drops what it cannot hold. A loop ending
    /// only on the notice waits for ever on a session that is already over.
    #[test]
    fn a_session_recorded_as_over_ends_the_loop_without_the_notice() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, shared, _w) = wired_client(py);
            // Nothing is queued: this is the notice having been dropped.
            shared.reference.set_session_over(
                crate::reliability::retry::DisconnectReason::EngineStopped.as_str(),
            );
            let client = client.borrow(py);
            client.dispatch_once(py, &shared).unwrap();
            assert!(
                client.session_ended.load(Ordering::Relaxed),
                "run() would go on waiting on a session the engine has finished",
            );
            assert!(
                !client.connected.load(Ordering::Relaxed),
                "a finished session still reads as connected",
            );
        });
    }

    /// An account with nothing working counts from one, and one working order
    /// puts the count past it. The venue holds an id only while its order is
    /// live, so this is the whole of what a new id has to clear.
    #[test]
    fn the_counter_starts_past_what_is_working_and_no_further() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, shared, _w) = wired_client(py);
            let client = client.borrow(py);
            assert_eq!(client.stated_order_id(), 1, "nothing is working");

            let working = || crate::bridge::RichOrderInfo {
                contract: Default::default(),
                order: Default::default(),
                order_state: crate::types::model::OrderState {
                    status: "Submitted".into(), ..Default::default()
                },
                last_exec: Default::default(),
            };
            shared.orders.push_order_info(41, working());
            assert_eq!(
                client.stated_order_id(), 42,
                "an order named under 41 is what the next id has to clear",
            );
            assert_eq!(client.take_order_id(py), 42);
            assert_eq!(client.take_order_id(py), 43, "and the count goes on from there");

            shared.orders.push_order_info(7, working());
            assert_eq!(
                client.take_order_id(py), 44,
                "an order named under a lower id does not move the count back",
            );

            // A fill spends its id as surely as a working order holds it: the
            // venue refuses the second placement under it either way.
            let filled = crate::bridge::RichOrderInfo {
                contract: Default::default(),
                order: Default::default(),
                order_state: crate::types::model::OrderState {
                    status: "Filled".into(), ..Default::default()
                },
                last_exec: Default::default(),
            };
            shared.orders.push_order_info(90, filled);
            assert_eq!(
                client.take_order_id(py), 91,
                "an id a fill has spent is counted past, not handed out again",
            );
        });
    }

    /// An id a caller numbers themselves need not fit the width the request
    /// carries. Truncating it answers under an id they never used, so it is
    /// refused instead.
    #[test]
    fn a_req_id_past_u32_is_refused_rather_than_truncated() {
        Python::initialize();
        Python::attach(|py| {
            let (client, rx, _shared, _w) = wired_client(py);
            let big = u32::MAX as i64 + 1;
            for method in [
                "cancel_historical_data", "cancel_head_time_stamp", "cancel_scanner_subscription",
                "cancel_fundamental_data", "cancel_histogram_data", "cancel_mkt_depth",
                "cancel_real_time_bars",
            ] {
                let Err(err) = client.call_method1(py, method, (big,)) else {
                    panic!("{method} accepted a req_id it cannot carry");
                };
                assert!(err.to_string().contains("outside the range"), "{method}: got {err}");
            }
            assert!(rx.try_recv().is_err(), "a refused req_id must reach no engine command");
        });
    }

    /// The reference client sends a book request's secType and exchange
    /// straight off the contract. Filling them in here subscribes a contract
    /// naming only an id to the book of a US stock on SMART, under the
    /// caller's own request id.
    #[test]
    fn a_book_request_states_the_contract_it_was_given() {
        Python::initialize();
        Python::attach(|py| {
            let (client, rx, _shared, _w) = wired_client(py);
            let contract = Py::new(py, Contract {
                con_id: 495512563,
                sec_type: String::new(),
                exchange: String::new(),
                ..Default::default()
            }).unwrap();
            client.call_method1(py, "req_mkt_depth", (1i64, &contract, 5i32, false)).unwrap();

            let sent = rx.try_recv().expect("the request reached no engine command");
            let ControlCommand::SubscribeDepth { contract, .. } = sent else {
                panic!("a book request sent something else");
            };
            assert_eq!(contract.exchange, "", "an exchange was invented");
            assert_eq!(contract.sec_type, "", "a security type was invented");
            assert_eq!(contract.con_id, 495512563);
        });
    }

    /// One pass can carry two reports for the same order: an acknowledgement
    /// and then a fill. Keeping one report per order dropped the earlier one,
    /// and the caller was never told the order had been acknowledged.
    // Drives the client through the methods that push state into it, which
    // a wheel does not carry: see the `test-helpers` feature.
    #[cfg(feature = "test-helpers")]
    #[test]
    fn an_acknowledgement_survives_a_fill_in_the_same_pass() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, _shared, w) = wired_client(py);

            // The venue acknowledges the order, then fills half of it, both
            // before the caller pumps the queue again.
            client.call_method1(py, "_test_push_order_update",
                (88u64, 0u32, "Submitted", 0.0f64, 100.0f64)).unwrap();
            client.call_method1(py, "_test_push_fill",
                (0u32, 88u64, "BUY", 150.0f64, 50i64, 50i64, 0.0f64)).unwrap();
            client.call_method1(py, "_test_push_order_update",
                (88u64, 0u32, "PartiallyFilled", 50.0f64, 50.0f64)).unwrap();

            client.call_method0(py, "_test_dispatch_once").unwrap();

            let g = pyo3::types::PyDict::new(py);
            g.set_item("w", &w).unwrap();
            let reported: usize = py.eval(
                c"len([c for c in w.calls if c[0] in ('order_status', 'orderStatus')])",
                Some(&g), None,
            ).unwrap().extract().unwrap();
            assert_eq!(
                reported, 2,
                "the acknowledgement was dropped by the fill that followed it",
            );
        });
    }

    /// `reqPositions` subscribes to a real-time feed: a holding that moves
    /// after the call is reported as it moves. Answering only the set held
    /// when the call was made left a caller tracking its positions from a
    /// snapshot that went stale on the next fill.
    // Drives the client through the methods that push state into it, which
    // a wheel does not carry: see the `test-helpers` feature.
    #[cfg(feature = "test-helpers")]
    #[test]
    fn a_holding_that_moves_after_the_request_is_reported() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, shared, w) = wired_client(py);
            shared.portfolio.set_account_download_complete();
            let held = |qty: f64| PositionInfo {
                con_id: 756733, position: qty, symbol: "SPY".into(),
                sec_type: "STK".into(), currency: "USD".into(), ..Default::default()
            };
            shared.portfolio.set_position_info(held(1.0));
            client.call_method0(py, "req_positions").unwrap();

            let g = pyo3::types::PyDict::new(py);
            g.set_item("w", &w).unwrap();
            let reported = || -> usize {
                py.eval(c"len([c for c in w.calls if c[0] == 'position'])", Some(&g), None)
                    .unwrap().extract().unwrap()
            };
            assert_eq!(reported(), 1, "the holding held when it was asked for");

            shared.portfolio.set_position_info(held(3.0));
            client.call_method0(py, "_test_dispatch_once").unwrap();
            assert_eq!(reported(), 2, "and the holding once it moves");

            // Withdrawn, so what moves after is no longer reported.
            client.call_method0(py, "cancel_positions").unwrap();
            shared.portfolio.set_position_info(held(5.0));
            client.call_method0(py, "_test_dispatch_once").unwrap();
            assert_eq!(reported(), 2, "a withdrawn ask is not answered further");
        });
    }

    /// The Rust surface hands `position` the whole contract. An options
    /// position whose strike, right and expiry are dropped names no contract
    /// the caller can act on.
    #[test]
    fn a_position_carries_the_contract_it_is_a_position_in() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, shared, w) = wired_client(py);
            client.get().core.cache_contract(756733, ApiContract {
                con_id: 756733, symbol: "SPY".into(), sec_type: "OPT".into(),
                exchange: "SMART".into(), currency: "USD".into(),
                last_trade_date_or_contract_month: "20260320".into(),
                strike: 600.0, right: "C".into(), multiplier: "100".into(),
                ..Default::default()
            });
            shared.portfolio.set_position_info(PositionInfo {
                con_id: 756733, position: 1.0, ..Default::default()
            });
            shared.portfolio.set_account_download_complete();
            shared.portfolio.set_account(&Default::default());

            client.call_method0(py, "req_positions").unwrap();
            client.call_method1(py, "req_positions_multi", (1i64, "DU123", "")).unwrap();

            let g = pyo3::types::PyDict::new(py);
            g.set_item("w", &w).unwrap();
            // Under the reference client's own spelling, which is what a
            // wrapper written against it defines and so what these answers are
            // delivered as.
            for (callback, index) in [("position", 2), ("positionMulti", 4)] {
                let expr = format!("[c[{index}] for c in w.calls if c[0] == '{callback}'][0]");
                let c = py.eval(&std::ffi::CString::new(expr).unwrap(), Some(&g), None).unwrap();
                for (field, want) in [
                    ("symbol", "SPY"), ("sec_type", "OPT"), ("right", "C"),
                    ("multiplier", "100"), ("last_trade_date_or_contract_month", "20260320"),
                ] {
                    let got: String = c.getattr(field).unwrap().extract().unwrap();
                    assert_eq!(got, want, "{callback} dropped {field}");
                }
                let strike: f64 = c.getattr("strike").unwrap().extract().unwrap();
                assert_eq!(strike, 600.0, "{callback} dropped strike");
            }
        });
    }

    /// `permId` is what survives a restart; the local order id does not.
    #[test]
    fn an_order_can_be_cancelled_by_its_perm_id() {
        Python::initialize();
        Python::attach(|py| {
            let (client, rx, _shared, _w) = wired_client(py);
            client.get().core.track_order(
                77,
                ApiContract { con_id: 756733, symbol: "SPY".into(), ..Default::default() },
                ApiOrder { order_id: 77, total_quantity: 1.0, perm_id: 91011, ..Default::default() },
                0,
            );

            client.call_method1(py, "cancel_order_by_perm_id", (91011i64,)).unwrap();
            match rx.try_recv().expect("a cancel must reach the engine") {
                ControlCommand::Order(OrderRequest::Cancel { order_id }) => assert_eq!(order_id, 77),
                other => panic!("expected a Cancel, got {other:?}"),
            }
        });
    }

    /// The per-request market-data mode is what keeps a thinly-traded name
    /// streaming after hours; without it this surface can only ask for realtime.
    #[test]
    fn req_mkt_data_ex_carries_the_market_data_mode() {
        Python::initialize();
        Python::attach(|py| {
            let (client, rx, _shared, _w) = wired_client(py);
            let contract = Py::new(py, Contract {
                con_id: 756733, symbol: "SPY".into(), ..Default::default()
            }).unwrap();
            // The registration reply comes from an engine no test has, so the
            // call itself fails; the commands are on the channel either way.
            let _ = client.call_method1(py, "req_mkt_data_ex", (1i64, &contract, "", false, false, 2i32));

            let mut mode = None;
            while let Ok(cmd) = rx.try_recv() {
                if let ControlCommand::Subscribe { mode_9887, .. } = cmd {
                    mode = Some(mode_9887);
                }
            }
            assert_eq!(mode, Some(2), "the requested market-data mode must reach the subscribe");
        });
    }

    /// Both are read off the same logon push the Rust surface exposes; a webapp
    /// REST call from Python needs them.
    #[test]
    fn the_session_id_and_url_map_from_logon_are_readable() {
        Python::initialize();
        Python::attach(|py| {
            let (client, _rx, shared, _w) = wired_client(py);
            shared.reference.set_ccp_session_id("abc.0001".into());
            shared.reference.set_misc_urls(
                [("region_dam".to_string(), "api.ibkr.com".to_string())].into_iter().collect(),
            );

            let id: String = client.call_method0(py, "ccp_session_id").unwrap().extract(py).unwrap();
            assert_eq!(id, "abc.0001");
            let url: Option<String> = client.call_method1(py, "misc_url", ("region_dam",))
                .unwrap().extract(py).unwrap();
            assert_eq!(url.as_deref(), Some("api.ibkr.com"));
            let missing: Option<String> = client.call_method1(py, "misc_url", ("nope",))
                .unwrap().extract(py).unwrap();
            assert_eq!(missing, None);
        });
    }

    /// The chain callback takes seven arguments in an order nothing on this
    /// side of the boundary checks, and a caller reads the strikes it is
    /// handed by position.
    // Drives the client through the methods that push state into it, which
    // a wheel does not carry: see the `test-helpers` feature.
    #[cfg(feature = "test-helpers")]
    #[test]
    fn an_option_chain_crosses_the_boundary_in_the_order_a_caller_reads_it() {
        Python::initialize();
        Python::attach(|py| {
            let (client, rx, shared, w) = wired_client(py);

            client.call_method1(py, "req_sec_def_opt_params", (9i64, "AAPL", "", "STK", 265598i64)).unwrap();
            match rx.try_recv().expect("the request must reach the engine") {
                ControlCommand::FetchOptionParams { req_id, symbol, underlying_con_id, .. } => {
                    assert_eq!((req_id, symbol.as_str(), underlying_con_id), (9, "AAPL", 265598));
                }
                other => panic!("expected a chain request, got {other:?}"),
            }

            shared.reference.push_option_params(9, 265598, vec![
                crate::control::contracts::OptionChainScope {
                    symbol: "AAPL".into(), exchange: "SMART".into(), trading_class: "AAPL".into(),
                    multiplier: "100".into(), expirations: vec!["20260116".into(), "20260320".into()],
                    strikes: vec![140.0, 145.0], underlying_con_id: 265598,
                },
            ]);
            client.call_method0(py, "_test_dispatch_once").unwrap();

            let g = pyo3::types::PyDict::new(py);
            g.set_item("w", &w).unwrap();
            // This wrapper answers to any name at all, so the callback reaches
            // it under the reference client's spelling as readily as under this
            // one's. Either is correct here; a wrapper that names the callback
            // explicitly is what the naming tests cover.
            let call = py.eval(
                c"[c for c in w.calls if c[0] in ('security_definition_option_parameter', 'securityDefinitionOptionParameter')][0]",
                Some(&g), None,
            ).unwrap();
            let arg = |n: usize| call.get_item(n).unwrap();
            assert_eq!(arg(1).extract::<i64>().unwrap(), 9);
            assert_eq!(arg(2).extract::<String>().unwrap(), "SMART");
            assert_eq!(arg(3).extract::<i64>().unwrap(), 265598);
            assert_eq!(arg(4).extract::<String>().unwrap(), "AAPL");
            assert_eq!(arg(5).extract::<String>().unwrap(), "100");
            assert_eq!(arg(6).extract::<Vec<String>>().unwrap(), ["20260116", "20260320"]);
            assert_eq!(arg(7).extract::<Vec<f64>>().unwrap(), [140.0, 145.0]);

            let ended: usize = py.eval(
                c"len([c for c in w.calls if c[0] in ('security_definition_option_parameter_end', 'securityDefinitionOptionParameterEnd')])",
                Some(&g), None,
            ).unwrap().extract().unwrap();
            assert_eq!(ended, 1, "the request ends once");
        });
    }
}
