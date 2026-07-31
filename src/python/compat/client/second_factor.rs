//! Python callables reached from inside the second-factor gate.
//!
//! `Gateway::connect` runs with the GIL released, so both callbacks re-attach
//! before touching Python and detach again on the way out. Neither can be
//! delivered as an engine event: the event channel and the loop that drains it
//! are created after `connect()` returns, and the gate is waiting now (ibx#208).

use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};

use pyo3::prelude::*;
use pyo3::types::PyString;

use crate::auth::session::{CodeProvider, IbKeyChallenge, WaitHook};

/// Render a Python exception as one line, for an `io::Error` message.
fn describe(py: Python<'_>, err: &PyErr) -> String {
    err.value(py).str().map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|_| err.to_string())
}

/// Adapt a Python callable to the Rust code provider.
///
/// The callable is handed `(display_id, approval_url)` and must return the
/// 8-character code as a string. Anything it raises becomes an `io::Error`, so
/// a provider that cannot produce a code fails the login rather than leaving
/// the gate waiting for a push that is not coming.
pub(crate) fn code_provider_bridge(callable: Py<PyAny>) -> CodeProvider {
    std::sync::Arc::new(move |challenge: IbKeyChallenge| -> io::Result<String> {
        let call = || {
            Python::attach(|py| -> PyResult<String> {
                let args = (
                    PyString::new(py, &challenge.display_id),
                    PyString::new(py, &challenge.avth_url),
                );
                let code = callable.call1(py, args)?;
                code.extract::<String>(py)
            })
        };
        // A panic crossing back into the gate would unwind through the auth
        // socket's borrow. Turned into an error here instead.
        match catch_unwind(AssertUnwindSafe(call)) {
            Ok(Ok(code)) => Ok(code),
            Ok(Err(e)) => {
                let msg = Python::attach(|py| describe(py, &e));
                Err(io::Error::other(format!("code_provider raised: {msg}")))
            }
            Err(_) => Err(io::Error::other("code_provider panicked")),
        }
    })
}

/// Adapt a Python callable to the wait hook.
///
/// Informational, so a callable that raises or panics is logged and the gate
/// carries on — a failing progress indicator is not a reason to fail a login
/// that is otherwise proceeding.
pub(crate) fn wait_hook_bridge(callable: Py<PyAny>) -> WaitHook {
    std::sync::Arc::new(move |challenge: &IbKeyChallenge| {
        let call = || {
            Python::attach(|py| {
                let args = (
                    PyString::new(py, &challenge.display_id),
                    PyString::new(py, &challenge.avth_url),
                );
                if let Err(e) = callable.call1(py, args) {
                    log::warn!("on_2fa_wait raised: {}", describe(py, &e));
                }
            })
        };
        if catch_unwind(AssertUnwindSafe(call)).is_err() {
            log::warn!("on_2fa_wait panicked; the second-factor gate is unaffected");
        }
    })
}
