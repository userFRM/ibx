//! ibapi-compatible EClient class that wraps IbEngine.

mod market_data;
mod orders;
mod account;
mod reference;
mod dispatch;
mod stubs;
mod test_helpers;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crossbeam_channel::Sender;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::auth::session::{CodeProvider, IbKeyChallenge, SecondFactor};
use crate::bridge::{Event, SharedState};
use crate::client_core::ClientCore;
use crate::gateway::{Gateway, GatewayConfig};
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
    /// Set by connect(), cleared by disconnect().
    pub(crate) shared: Mutex<Option<Arc<SharedState>>>,
    /// Set by connect(), cleared by disconnect().
    pub(crate) control_tx: Mutex<Option<Sender<ControlCommand>>>,
    pub(crate) next_order_id: AtomicU64,
    pub(crate) _thread: Mutex<Option<thread::JoinHandle<()>>>,
    /// Set by connect(), cleared by disconnect().
    pub(crate) account_id: Mutex<Option<String>>,
    pub(crate) connected: AtomicBool,
    /// Receiver for engine events (disconnects, etc.).
    pub(crate) event_rx: Mutex<Option<crossbeam_channel::Receiver<Event>>>,
    /// Sender for test-injected events (test-only).
    #[doc(hidden)]
    pub(crate) _test_event_tx: Mutex<Option<crossbeam_channel::Sender<Event>>>,
    /// Shared subscription tracking and dispatch preparation.
    pub(crate) core: ClientCore,
}

impl Drop for EClient {
    fn drop(&mut self) {
        if let Some(tx) = self.control_tx.lock().unwrap().as_ref() {
            let _ = tx.send(ControlCommand::Shutdown);
        }
        if let Some(h) = self._thread.lock().unwrap().take() {
            // A wedged engine (ibx#254) never returns from join(). Detach so
            // that stall parks this thread, not the whole interpreter
            // (ibx#271). try_attach, not attach: dealloc also runs during
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
            wrapper,
            shared: Mutex::new(None),
            control_tx: Mutex::new(None),
            next_order_id: AtomicU64::new(0),
            _thread: Mutex::new(None),
            account_id: Mutex::new(None),
            connected: AtomicBool::new(false),
            event_rx: Mutex::new(None),
            _test_event_tx: Mutex::new(None),
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
    /// each a distinct value. See ibx#203 / ibx#207.
    #[pyo3(signature = (host="cdc1.ibllc.com".to_string(), port=0, client_id=0, username="".to_string(), password="".to_string(), paper=true, core_id=None, ib_key_timeout_secs=None, ib_key_token_sub_type=None, code_provider=None))]
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
    ) -> PyResult<()> {
        if self.connected.load(Ordering::Relaxed) {
            return Err(PyRuntimeError::new_err("Already connected"));
        }

        let code_provider = code_provider.map(code_provider_from_py);

        let config = GatewayConfig {
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
        };

        let result = py.detach(|| Gateway::connect(&config));
        let (gw, farm_conn, ccp_conn, hmds_conn) = result
            .map_err(|e| PyRuntimeError::new_err(format!("Connection failed: {e}")))?;

        *self.account_id.lock().unwrap() = Some(gw.account_id.clone());
        let shared = Arc::new(SharedState::new());
        gw.populate_init_data(&shared);

        let connect_host = config.host.clone();
        let connect_username = config.username.clone();
        let connect_password = config.password.clone();
        let connect_paper = config.paper;
        let (event_tx, event_rx) = crossbeam_channel::bounded(256);
        let (hot_loop, control_tx) = gw.into_hot_loop_with_farms(
            shared.clone(), Some(event_tx), farm_conn, ccp_conn, hmds_conn, core_id,
            crate::gateway::CallerAuth {
                host: connect_host,
                username: connect_username,
                password: connect_password,
                paper: connect_paper,
            },
        );

        let start_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() * 1000;

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

        let _ = (port, client_id); // unused but kept for ibapi signature compat

        // Fire initial callbacks synchronously, matching official Python ibapi
        // where connect_ack signals "socket ready" before run() is called.
        self.wrapper.call_method0(py, "connect_ack")?;
        self.wrapper.call_method1(py, "managed_accounts", (self.account().as_str(),))?;
        self.wrapper.call_method1(py, "next_valid_id", (start_id as i64,))?;

        Ok(())
    }

    /// Disconnect from IB.
    fn disconnect(&self, py: Python<'_>) -> PyResult<()> {
        if let Some(tx) = self.control_tx.lock().unwrap().as_ref() {
            let _ = tx.send(ControlCommand::Shutdown);
        }
        if let Some(h) = self._thread.lock().unwrap().take() {
            // Same wedged-engine hazard as Drop (ibx#254): release the GIL
            // for the join so a stuck engine thread stalls only this call.
            py.detach(|| { let _ = h.join(); });
        }
        self.connected.store(false, Ordering::Release);
        // Reset per-session state so connect() can be called again.
        *self.shared.lock().unwrap() = None;
        *self.control_tx.lock().unwrap() = None;
        *self.event_rx.lock().unwrap() = None;
        *self.account_id.lock().unwrap() = None;
        self.core.reset();
        Ok(())
    }

    /// Check if connected.
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Run the event loop.
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
        self.wrapper.call_method0(py, "connection_closed")?;

        Ok(())
    }

    /// Get the account ID.
    fn get_account_id(&self) -> String {
        self.account()
    }
}

impl EClient {
    /// Clone the control channel sender, or return "Not connected".
    pub(crate) fn tx(&self) -> PyResult<Sender<ControlCommand>> {
        self.control_tx.lock().unwrap().clone()
            .ok_or_else(|| PyRuntimeError::new_err("Not connected"))
    }

    /// Send a control command to the engine. `control_tx` is a bounded(64)
    /// channel: a full queue is normal backpressure (the hot loop is behind,
    /// not gone) and `send` is meant to wait for it to drain, so the send
    /// itself stays blocking. What must not happen is waiting with the GIL
    /// held, stalling every Python thread instead of just this call
    /// (ibx#271), so the wait runs detached, and only the actual send
    /// crosses that boundary; `cmd` must already be a plain owned value by
    /// the time it's built (never touching Python state once detached).
    pub(crate) fn send_control(py: Python<'_>, tx: &Sender<ControlCommand>, cmd: ControlCommand) -> PyResult<()> {
        py.detach(|| tx.send(cmd))
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

    /// Find instrument ID for a contract, registering if needed. The hot
    /// loop can take up to `REGISTRATION_TIMEOUT` to reply, so the round
    /// trip runs with the GIL released: otherwise a slow reply stalls every
    /// Python thread, not just this call (ibx#271).
    pub(crate) fn find_or_register_instrument(&self, py: Python<'_>, contract: &Contract) -> PyResult<u32> {
        let tx = self.tx()?;
        let con_id = contract.con_id;
        let symbol = contract.symbol.clone();
        let exchange = contract.exchange.clone();
        let sec_type = contract.sec_type.clone();
        py.detach(|| self.core.find_or_register_instrument(&tx, con_id, &symbol, &exchange, &sec_type))
            .map_err(PyRuntimeError::new_err)
    }
}

/// Register EClient on the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<EClient>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::contract::TagValue;

    #[test]
    fn eclient_default_state() {
        // Can't construct without Python, but we can test the parsing helpers
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
}
