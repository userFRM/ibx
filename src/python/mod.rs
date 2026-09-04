//! PyO3 bindings for ibx. Feature-gated behind `python`.
//!
//! Provides an ibapi-compatible API (callback-based):
//! ```python
//! from ibx import EClient, EWrapper, Contract, Order
//! class App(EWrapper):
//!     def next_valid_id(self, order_id):
//!         ..
//! app = App()
//! client = EClient(app)
//! client.connect(username="user", password="pass", paper=True)
//! client.run()
//! ```

/// What the Python side scales prices by, which is what the model scales them
/// by. A file of its own said nothing more than this line.
mod types {
    pub use crate::types::model::PRICE_SCALE_F;
}
pub mod compat;

/// What a session runs under, from the names the Python client states them by.
///
/// The same names `ibx.configure` uses, so a caller states a setting the same
/// way whether it is for one session or for the process.
pub(crate) fn settings_from(
    stated: std::collections::HashMap<String, String>,
) -> Result<crate::settings::SessionSettings, String> {
    use crate::settings::{ExecutionReportScope, GatewaySettings};
    let mut settings = GatewaySettings::default();
    for (name, value) in stated {
        match name.as_str() {
            "timezone" => settings.timezone = Some(value),
            "locale" => settings.locale = Some(value),
            "build" => settings.build = Some(value),
            "version" => settings.version = Some(value),
            "encoded" => settings.encoded = Some(value),
            "hardware_id" => settings.hardware_id = Some(value),
            "market_data_host" => settings.market_data_host = Some(value),
            "port" => settings.port = Some(value.parse().map_err(|_| format!("port: {value}"))?),
            "registration_timeout_ms" => {
                settings.registration_timeout_ms =
                    Some(value.parse().map_err(|_| format!("registration_timeout_ms: {value}"))?);
            }
            // One logger per process, so these are not settled per session:
            // taken here they were parsed, held and then dropped on the way to
            // the session, which reads as a log level that was set and did
            // nothing. Said plainly instead, naming where they do work — which
            // for this client is before it is imported, because importing it
            // installs the logger.
            "log_level" | "log_dir" | "log_queue" => {
                return Err(format!(
                    "{name} belongs to the process, not one session: importing ibx \
                     installs the logger, so set IBX_{} in the environment before that",
                    name.to_uppercase(),
                ));
            }
            // However it is spelled, matching what the same settings are read
            // as when they come from the environment. Matched against the
            // lowercase spelling alone, `Today` was refused here where the
            // environment accepts it, and `False` turned the Island setting on.
            "execution_reports" => {
                settings.execution_reports = Some(if value.eq_ignore_ascii_case("today") {
                    ExecutionReportScope::Today
                } else if value.eq_ignore_ascii_case("all") {
                    ExecutionReportScope::All
                } else {
                    return Err(format!("execution_reports: {value}"));
                });
            }
            "island_for_nasdaq" => {
                settings.island_for_nasdaq = Some(
                    !["0", "false", "no"].iter().any(|off| value.eq_ignore_ascii_case(off)),
                );
            }
            other => return Err(format!("no such setting: {other}")),
        }
    }
    Ok(settings.resolve())
}

use pyo3::prelude::*;

/// Python module definition.
#[pymodule]
fn ibx(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Forward Rust `log::*` records wherever the environment asks for them.
    // `IBX_LOG_DIR` is published to callers as a setting, so a wheel that
    // answered it with stderr was answering something else. Both paths are
    // no-ops when a logger is already installed, which is what a module
    // initialiser wants: it runs once per interpreter, not once per process.
    let settings = crate::logging::LogConfig::from_env();
    if settings.log_dir.is_some() {
        if let Some(guard) = crate::logging::try_init(&settings) {
            guard.keep_for_the_process();
        }
    } else {
        let _ = crate::logging::try_init_from_env("warn");
    }
    compat::register(m)?;
    Ok(())
}
