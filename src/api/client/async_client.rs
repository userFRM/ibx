//! The same session, for a program that is already running a reactor.
//!
//! This client's engine is a thread of its own, so a question asked of it
//! blocks the thread that asked and nothing else. That is what a program wants
//! when the asking thread is its own. It is the wrong thing inside an
//! asynchronous runtime, where the asking thread is one of a small pool shared
//! by everything else the program is doing, and holding one for the length of a
//! round trip to a venue stops work that has nothing to do with this session.
//!
//! [`AsyncClient`] is the same client with each question moved onto a thread
//! that is allowed to wait. Every method here has the same name and the same
//! answer as the one it stands in front of, and a test holds them to it.
//!
//! ```no_run
//! # use ibx::{AsyncClient, EClientConfig};
//! # use ibx::types::model::{Contract, Order};
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let ib = AsyncClient::connect(EClientConfig {
//!     username: "user".into(), password: "pass".into(),
//!     paper: true, ..Default::default()
//! }).await?;
//!
//! let spy = ib.qualify(Contract::stock("SPY")).await?;
//! let bars = ib.bars(&spy, "2 D", "1 hour").await?;
//! let preview = ib.preview(&spy, &Order::limit("BUY", 1.0, 1.0)).await?;
//! println!("{} bars, commission {}", bars.len(), preview.commission_and_fees);
//! # Ok(())
//! # }
//! ```
//!
//! There is no second engine and no second session behind this: it holds the
//! same [`EClient`], reachable through [`blocking`](AsyncClient::blocking) for
//! anything not covered here.

use std::sync::Arc;

use crate::error_codes::Refusal;
use crate::api::client::{AccountValue, EClient, EClientConfig, OrderReport, PositionRow};
use crate::types::model::{BarData, Contract, ContractDetails, Order, OrderState};

/// A session whose questions are asked off the runtime's own threads.
///
/// Cloning shares the session rather than opening another: the venue allows one
/// per login, and a second would take the first one's place.
#[derive(Clone)]
pub struct AsyncClient {
    inner: Arc<EClient>,
}

/// Ask a question on a thread that may wait, and hand the answer back.
///
/// The runtime keeps a pool for exactly this. Without it the question runs on a
/// worker, and everything else that worker was going to do waits on a venue.
macro_rules! off_the_reactor {
    ($self:expr, |$client:ident| $ask:expr) => {{
        let $client = Arc::clone(&$self.inner);
        tokio::task::spawn_blocking(move || $ask)
            .await
            .map_err(|e| Refusal::validation(format!("the question was cancelled: {e}")))?
    }};
}

impl AsyncClient {
    /// Open a session.
    ///
    /// Takes the configuration by value rather than by reference: the logon
    /// runs on another thread and has to own what it reads.
    pub async fn connect(config: EClientConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let client = tokio::task::spawn_blocking(move || {
            EClient::connect(&config).map_err(|e| e.to_string())
        })
        .await??;
        Ok(Self { inner: Arc::new(client) })
    }

    /// The session underneath, for everything this surface does not cover.
    ///
    /// Every call on it blocks the thread that makes it. Inside a runtime, wrap
    /// one in `spawn_blocking` as the methods here do.
    pub fn blocking(&self) -> &EClient {
        &self.inner
    }

    /// Bars of trades during regular hours, ending now.
    pub async fn bars(
        &self, contract: &Contract, duration: &str, bar_size: &str,
    ) -> Result<Vec<BarData>, Refusal> {
        let (contract, duration, bar_size) =
            (contract.clone(), duration.to_string(), bar_size.to_string());
        off_the_reactor!(self, |c| c.bars(&contract, &duration, &bar_size))
    }

    /// The one contract the venue means by this description.
    pub async fn qualify(&self, contract: Contract) -> Result<Contract, Refusal> {
        off_the_reactor!(self, |c| c.qualify(contract))
    }

    /// Everything the venue lists under this description.
    pub async fn lookup(&self, contract: &Contract) -> Result<Vec<ContractDetails>, Refusal> {
        let contract = contract.clone();
        off_the_reactor!(self, |c| c.lookup(&contract))
    }

    /// What an order would cost and what it would do to the margin, without
    /// placing it.
    pub async fn preview(
        &self, contract: &Contract, order: &Order,
    ) -> Result<OrderState, Refusal> {
        let (contract, order) = (contract.clone(), order.clone());
        off_the_reactor!(self, |c| c.preview(&contract, &order))
    }

    /// Place an order under the next id, and hand back what became of it.
    pub async fn place(
        &self, contract: &Contract, order: &Order,
    ) -> Result<OrderReport, Refusal> {
        let (contract, order) = (contract.clone(), order.clone());
        off_the_reactor!(self, |c| c.place(&contract, &order))
    }

    /// What the account is worth and what it can buy.
    pub async fn summary(&self) -> Result<Vec<AccountValue>, Refusal> {
        off_the_reactor!(self, |c| c.summary())
    }

    /// What the account holds.
    pub async fn positions(&self) -> Result<Vec<PositionRow>, Refusal> {
        off_the_reactor!(self, |c| c.positions())
    }

    /// Start a market-data subscription, and hand back the id that withdraws it.
    ///
    /// Not moved off the reactor: this sends and returns without waiting for an
    /// answer.
    pub fn watch(&self, contract: &Contract) -> Result<i64, Refusal> {
        self.inner.watch(contract)
    }

    /// The latest bid, ask and last for a contract being watched.
    ///
    /// Not moved off the reactor: this reads state already in memory and never
    /// waits.
    pub fn quote_of(&self, contract: &Contract) -> Option<crate::types::Quote> {
        self.inner.quote_of(contract)
    }

    /// Whether the session is carrying traffic.
    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// End the session.
    pub fn disconnect(&self) {
        self.inner.disconnect();
    }
}

#[cfg(test)]
mod tests {
    /// Every question this surface answers is a question the blocking one
    /// answers under the same name. A name that existed on one and not the
    /// other would be a program that cannot be moved between them, which is the
    /// only reason to have two.
    ///
    /// Read from the sources rather than from a list kept beside them: a list
    /// is a third thing to forget.
    #[test]
    fn every_async_call_is_a_blocking_call_under_the_same_name() {
        use std::collections::BTreeSet;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let names = |text: &str, prefix: &str| -> BTreeSet<String> {
            text.lines()
                .filter_map(|l| l.trim().strip_prefix(prefix))
                .filter_map(|l| l.split('(').next())
                .map(str::to_string)
                .collect()
        };
        let here = root.join("src/api/client");
        let mine = std::fs::read_to_string(here.join("async_client.rs")).expect("this file");
        // Every other file of the surface, so a call answered blockingly
        // somewhere unexpected still counts as answered. Read the directory
        // rather than list it: a list is a third thing to forget.
        let mut rest = String::new();
        for entry in std::fs::read_dir(&here).expect("the client's own directory") {
            let path = entry.expect("a readable entry").path();
            if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "async_client.rs")
            {
                rest.push_str(&std::fs::read_to_string(&path).expect("a readable file"));
            }
        }
        // The blocking surface is wider: it carries the reference client's
        // every request. What matters is that nothing here is missing there.
        let asynchronous = names(&mine, "pub async fn ");
        let blocking = names(&rest, "pub fn ");
        let missing: Vec<_> = asynchronous.difference(&blocking).collect();
        assert!(
            missing.is_empty(),
            "asked asynchronously and nowhere else: {missing:?}",
        );
        assert!(asynchronous.contains("bars"), "the reader found the methods");
    }
}
