//! ibapi-compatible EClient class that wraps IbEngine.

mod market_data;
mod orders;
mod account;
mod reference;
mod ask;
mod dispatch;
mod stubs;
mod test_helpers;

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
    /// Reference to the EWrapper (which is typically `self` in the `App(EWrapper, EClient)` pattern).
    pub(crate) wrapper: Py<PyAny>,
    /// Set by connect(), cleared by disconnect.
    pub(crate) shared: Mutex<Option<Arc<SharedState>>>,
    /// Set by connect(), cleared by disconnect.
    pub(crate) control_tx: Mutex<Option<SyncSender<ControlCommand>>>,
    pub(crate) next_order_id: AtomicU64,
    /// Where the last id handed out is kept, and under which key. Empty when
    /// the caller asked for no file, which makes the counter this session's
    /// alone and lets it collide with what an earlier one used.
    pub(crate) order_id_store: Mutex<Option<(std::path::PathBuf, String)>>,
    pub(crate) _thread: Mutex<Option<thread::JoinHandle<()>>>,
    /// Set by connect(), cleared by disconnect.
    pub(crate) account_id: Mutex<Option<String>>,
    /// Every account this login holds, the first being `account_id`.
    pub(crate) accounts: Mutex<Vec<String>>,
    pub(crate) connected: AtomicBool,
    /// The number this session connected under, as the caller gave it.
    ///
    /// One session holds the account here, so this does not route anything.
    /// It is kept because the counterpart answers some calls by it — binding
    /// orders entered elsewhere is refused for any client but zero — and a
    /// caller that names one is answered the same way.
    pub(crate) client_id: AtomicI32,
    /// Receiver for engine events (disconnects, etc.).
    pub(crate) event_rx: Mutex<Option<std::sync::mpsc::Receiver<Event>>>,
    /// Sender for test-injected events (test-only).
    #[doc(hidden)]
    pub(crate) _test_event_tx: Mutex<Option<std::sync::mpsc::SyncSender<Event>>>,
    /// Holds the control channel's receiving end for a test-connected client.
    /// Dropping it closed the channel, so a client that reported itself
    /// connected failed every request that sends one.
    pub(crate) _test_control_rx: Mutex<Option<std::sync::mpsc::Receiver<ControlCommand>>>,
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
            // Dropping the client ends the session, so the venue is told.
            let _ = tx.send(ControlCommand::Logout);
            let _ = tx.send(ControlCommand::Shutdown);
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
/// refusing rather than truncating. `next_order_id()` starts at
/// milliseconds-since-epoch, so the ibapi idiom of one counter for orders and
/// requests is past `u32::MAX` on the first call, and a truncated id answers
/// under one the caller never used.
pub(crate) fn wire_req_id(req_id: i64) -> PyResult<u32> {
    crate::api::client::wire_req_id(req_id)
        .map_err(|refusal| PyRuntimeError::new_err(refusal.message))
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
            order_id_store: Mutex::new(None),
            _thread: Mutex::new(None),
            account_id: Mutex::new(None),
            accounts: Mutex::new(Vec::new()),
            connected: AtomicBool::new(false),
            event_rx: Mutex::new(None),
            _test_event_tx: Mutex::new(None),
            _test_control_rx: Mutex::new(None),
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
    /// is ``"ibkey"`` (return the 8-character code shown for ``display_id``) or
    /// ``"authenticator"`` (return the account's current code; ``display_id``
    /// and ``avth_url`` are empty). An authenticator account has no push to
    /// fall back to and cannot log in without this. It is called once, on a
    /// thread of its own, and holds the GIL while it runs — return the code,
    /// don't block on input. One wrong code ends the login; there is no retry.
    ///
    /// Multiple ``EClient`` instances can run concurrently in one process; each
    /// owns its own state, sockets, and engine thread, and ``connect()`` does
    /// not serialize across instances. If you pin engines via ``core_id``, give
    /// each a distinct value.
    ///
    /// `port` is taken and not applied. There is no local socket to name a port
    /// on: this client is the one the gateway would have been listening for.
    #[pyo3(signature = (host=crate::config::CCP_HOSTS[0].to_string(), port=0, client_id=0, username="".to_string(), password="".to_string(), paper=true, core_id=None, ib_key_timeout_secs=None, ib_key_token_sub_type=None, code_provider=None, readonly=false, settings=None, session_file=None, order_id_file=None))]
    #[allow(clippy::too_many_arguments)]
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
        // logon. The venue answers a request that names a session it still
        // holds with a challenge rather than a whole handshake, which is how a
        // program restarts without a person to approve it — and how a program
        // that starts often stops counting against an account as a new login
        // every time. Owner-only, sealed with the password, and bound to this
        // account and this kind of session.
        session_file: Option<String>,
        // Where the last order id handed out is kept. Beside the session file
        // by default, or under the caller's home where there is none.
        order_id_file: Option<String>,
    ) -> PyResult<()> {
        let username_for_resume = username.clone();
        let username_for_session = username.clone();
        let password_for_resume = password.clone();
        if self.connected.load(Ordering::Relaxed) {
            return Err(PyRuntimeError::new_err("Already connected"));
        }

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

        // Kept for the next start, where the caller asked for it. Best effort:
        // a session that cannot be written is a slower start next time, never
        // a failed connect now.
        if let Some(path) = session_file.as_ref() {
            let session = crate::auth::resume::ResumableSession {
                token: crate::auth::crypto::strip_leading_zeros(
                    &gw.session_token.to_bytes_be(),
                ).to_vec(),
                server_session_id: gw.server_session_id.clone(),
                hw_info: gw.hw_info.clone(),
                encoded: gw.encoded.clone(),
                username: username_for_session,
                paper,
            };
            if let Err(e) = crate::auth::resume::save(
                std::path::Path::new(path), &password_for_resume, &session,
            ) {
                log::warn!("session not saved to {path}: {e}");
            }
        }

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
        let (hot_loop, control_tx) = gw.into_hot_loop_with_farms(
            shared.clone(), Some(event_tx), farm_conn, ccp_conn, hmds_conn, secdef_conn, core_id,
            crate::gateway::CallerAuth {
                settings: Default::default(),
                host: connect_host,
                username: connect_username,
                password: connect_password,
                paper: connect_paper,
                code_provider: config.code_provider.clone(),
                ib_key_timeout_secs: config.ib_key_timeout_secs,
                ib_key_token_sub_type: config.ib_key_token_sub_type.clone(),
            },
        );

        // One past the last id this account handed out, read from the file
        // that remembers it. An id belongs to the account rather than to the
        // process: counting from one on every start collides with everything
        // placed yesterday, and the venue answers that with "Duplicate ID" and
        // places nothing.
        let id_store = order_id_file
            .map(std::path::PathBuf::from)
            .or_else(|| session_file.as_ref().and_then(|s| {
                std::path::Path::new(s).parent().map(|d| d.join("order-ids"))
            }))
            .unwrap_or_else(crate::order_ids::default_path);
        let id_key = crate::order_ids::key(&username_for_resume, paper, client_id);
        let start_id = crate::order_ids::next_after_last(&id_store, &id_key);
        *self.order_id_store.lock().unwrap() = Some((id_store, id_key));

        let handle = thread::Builder::new()
            .name("ib-engine-hotloop".into())
            .spawn(move || {
                hot_loop.run_with_panic_recovery();
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to spawn hot loop: {e}")))?;

        *self.shared.lock().unwrap() = Some(shared);
        *self.control_tx.lock().unwrap() = Some(control_tx);
        *self.event_rx.lock().unwrap() = Some(event_rx);
        self.next_order_id.store(start_id, Ordering::Relaxed);
        *self._thread.lock().unwrap() = Some(handle);
        self.connected.store(true, Ordering::Release);

        self.client_id.store(client_id, Ordering::Release);
        let _ = port; // kept for the reference client's signature

        // Fire initial callbacks synchronously, matching official Python ibapi
        // where connect_ack signals "socket ready" before run() is called.
        self.callback(py, "connect_ack", ())?;
        self.callback(py, "managed_accounts", (self.accounts_csv().as_str(),))?;
        self.callback(py, "next_valid_id", (start_id as i64,))?;

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
        let mut snake = String::with_capacity(name.len() + 4);
        for (i, c) in name.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i != 0 {
                    snake.push('_');
                }
                snake.extend(c.to_lowercase());
            } else {
                snake.push(c);
            }
        }
        if snake != name
            && let Ok(f) = slf.as_any().getattr(snake.as_str())
        {
            return Ok(f.unbind());
        }
        Err(pyo3::exceptions::PyAttributeError::new_err(format!(
            "'EClient' object has no attribute '{name}'"
        )))
    }

    /// Disconnect from IB.
    fn disconnect(&self, py: Python<'_>) -> PyResult<()> {
        // Taken out of their locks first, as in `Drop`: the sends are bounded
        // and the join waits on the engine, so a guard spanning either blocks
        // every other thread that needs the same lock.
        let tx = self.control_tx.lock().unwrap().clone();
        if let Some(tx) = tx {
            // The session is ending, so the venue is told before the engine stops.
            let _ = tx.send(ControlCommand::Logout);
            let _ = tx.send(ControlCommand::Shutdown);
        }
        let thread = self._thread.lock().unwrap().take();
        if let Some(h) = thread {
            // Same wedged-engine hazard as Drop: release the GIL
            // for the join so a stuck engine thread stalls only this call.
            py.detach(|| { let _ = h.join(); });
        }
        self.connected.store(false, Ordering::Release);
        // Reset per-session state so connect() can be called again.
        *self.shared.lock().unwrap() = None;
        *self.control_tx.lock().unwrap() = None;
        *self.event_rx.lock().unwrap() = None;
        *self.account_id.lock().unwrap() = None;
        self.accounts.lock().unwrap().clear();
        self.core.reset();
        Ok(())
    }

    /// Check if connected.
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Run the event loop.
    /// Deliver everything waiting, once, and return.
    ///
    /// `run` owns the thread it is called on, which a program with an event
    /// loop of its own cannot give it: an asyncio framework has to drive the
    /// callbacks from its own loop, and a blocking loop leaves it nowhere to
    /// stand. This is one pass of the same dispatch.
    fn poll(&self, py: Python<'_>) -> PyResult<()> {
        if !self.connected.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some(shared) = self.shared.lock().unwrap().clone() else {
            return Ok(());
        };
        self.dispatch_once(py, &shared)
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
        while self.connected.load(Ordering::Relaxed) {
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

        // Signal disconnection to wrapper
        self.callback(py, "connection_closed", ())?;

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

    /// Logical-name → host URL lookup from the gateway logon MiscUrls push
    /// (e.g. `region_dam`). None when the gateway did not push this key.
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
        && !is_base_noop(py, &f)
    {
        f.call1(py, args.clone())?;
        return Ok(());
    }
    wrapper.call_method1(py, name, args)?;
    Ok(())
}

/// The reference client's spelling of a callback name: its words run together
/// with each after the first capitalised.
fn ibapi_name(snake: &str) -> String {
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

/// Whether an attribute is this base class's own do-nothing default rather than
/// something the caller wrote. Calling the default would report the callback as
/// delivered when nobody received it.
fn is_base_noop(py: Python<'_>, f: &Py<PyAny>) -> bool {
    f.getattr(py, "__objclass__")
        .and_then(|c| c.getattr(py, "__name__"))
        .and_then(|n| n.extract::<String>(py))
        .map(|n| n == "EWrapper")
        .unwrap_or(false)
}

impl EClient {
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
        refusal: crate::api::error_codes::Refusal,
    ) -> PyResult<()> {
        let _ = self.wrapper.call_method(
            py,
            "error",
            (req_id, refusal.code, refusal.message, ""),
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
                        (req_id, NOT_CONNECTED_CODE, "Not connected", ""),
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

    /// Hand out the next order id, and remember it as used.
    ///
    /// Written where it was read from, for the reason it is read at all: an id
    /// this run used must not be handed out by the next one. Written as it is
    /// handed out rather than in a batch, because a run that ends between the
    /// two is exactly the run whose ids would be reused.
    pub(crate) fn take_order_id(&self) -> u64 {
        let id = self.next_order_id.fetch_add(1, Ordering::Relaxed);
        if let Some((path, key)) = self.order_id_store.lock().unwrap().as_ref()
            && let Err(e) = crate::order_ids::remember(path, key, id)
        {
            log::warn!("order id {id} not remembered in {}: {e}", path.display());
        }
        id
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
    /// The reference client names its callbacks in one style and this one names
    /// them in another. A caller who brought a wrapper written against the
    /// reference client defines the reference client's names, and every call
    /// here landed on the no-op this base class supplies instead — so their
    /// callbacks never ran and nothing said so. Silence is the whole of the
    /// fault: an exception would at least have been visible.
    ///
    /// So the reference name is tried first, and the name this client has
    /// always used second, which keeps every existing caller working.
    pub(crate) fn callback<'py, A>(
        &self,
        py: Python<'py>,
        name: &str,
        args: A,
    ) -> PyResult<()>
    where
        A: pyo3::call::PyCallArgs<'py> + Clone,
    {
        let alias = ibapi_name(name);
        if alias != name
            && let Ok(f) = self.wrapper.getattr(py, alias.as_str())
            && !is_base_noop(py, &f)
        {
            f.call1(py, args.clone())?;
            return Ok(());
        }
        self.wrapper.call_method1(py, name, args)?;
        Ok(())
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
        let identity = crate::client_core::ClientCore::contract_identity(
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::contract::TagValue;
    use pyo3::Python;

    #[test]
    fn eclient_default_state() {
        // Not constructible without Python; the parsing helpers stand alone
        let tv = [TagValue { tag: "maxPctVol".into(), value: "0.1".into() },
            TagValue { tag: "startTime".into(), value: "09:30:00".into() },
            TagValue { tag: "endTime".into(), value: "16:00:00".into() }];

        let get = |key: &str| -> String {
            tv.iter()
                .find(|t| t.tag == key)
                .map(|t| t.value.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("maxPctVol"), "0.1");
        assert_eq!(get("startTime"), "09:30:00");
        assert_eq!(get("missing"), "");
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

    use crate::api::types::{Contract as ApiContract, Order as ApiOrder};
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

    /// `next_order_id()` starts at milliseconds-since-epoch, so the ibapi idiom
    /// of one counter for orders and requests hands every request an id past
    /// `u32::MAX`. Truncating it answers under an id the caller never used.
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
            for (callback, index) in [("position", 2), ("position_multi", 4)] {
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
                    strikes: vec![140.0, 145.0],
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
