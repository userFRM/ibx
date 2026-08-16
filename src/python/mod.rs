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
            "log_level" => settings.log_level = Some(value),
            "log_dir" => settings.log_dir = Some(value),
            "log_queue" => settings.log_queue = Some(value == "true" || value == "1"),
            "execution_reports" => {
                settings.execution_reports = Some(match value.as_str() {
                    "today" => ExecutionReportScope::Today,
                    "all" => ExecutionReportScope::All,
                    other => return Err(format!("execution_reports: {other}")),
                });
            }
            "island_for_nasdaq" => {
                settings.island_for_nasdaq = Some(value != "false" && value != "0");
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
    // Forward Rust `log::*` macros to stderr when RUST_LOG is set.
    // `try_init` is no-op if a logger is already installed (e.g. by tests).
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
    compat::register(m)?;
    Ok(())
}
