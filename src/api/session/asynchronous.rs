//! [`Client`] for a program already running an asynchronous runtime.
//!
//! What waits is awaited; what does not is not. A question goes to the venue
//! and comes back, so it is moved onto a thread that may wait — holding a
//! runtime thread for a round trip stops work that has nothing to do with this
//! session. Reading what the session holds waits for nothing: it is a lock and
//! a copy, and making it a future would be ceremony over a memory read.
//!
//! ```no_run
//! # use ibx::{AsyncClient, Config};
//! # use ibx::types::model::{Contract, Order};
//! # use std::time::Duration;
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let client = AsyncClient::connect(Config {
//!     username: "user".into(), password: "pass".into(),
//!     paper: true, ..Default::default()
//! }).await?;
//!
//! let spy = client.qualify(Contract::stock("SPY")).await?;
//! let order = client.place(&spy, &Order::limit("BUY", 100.0, 42.50)).await?;
//! client.wait_done(&order, Duration::from_secs(30)).await;
//! println!("{}", order.status());
//! # Ok(())
//! # }
//! ```
//!
//! There is no second session behind this: it holds a [`Client`], reachable
//! through [`blocking`](AsyncClient::blocking) for anything not named here.

use std::time::{Duration, Instant};

use crate::error_codes::Refusal;
use crate::types::model::{BarData, Contract, ContractDetails, Order, OrderState};

use super::{AccountValue, Client, Fill, PlacedOrder, Position, Trade};
use crate::api::client::EClientConfig;

/// A session read from a runtime.
///
/// Cloning shares the session rather than opening another.
#[derive(Clone)]
pub struct AsyncClient {
    inner: Client,
}

/// Ask on a thread that may wait, and hand the answer back.
macro_rules! off_the_reactor {
    ($self:expr, |$client:ident| $ask:expr) => {{
        let $client = $self.inner.clone();
        tokio::task::spawn_blocking(move || $ask)
            .await
            .map_err(|e| Refusal::validation(format!("the question was cancelled: {e}")))?
    }};
}

impl AsyncClient {
    /// Open a session and start reading it.
    ///
    /// Takes the configuration by value: the logon runs on another thread and
    /// has to own what it reads.
    pub async fn connect(config: EClientConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let inner = tokio::task::spawn_blocking(move || {
            Client::connect(&config).map_err(|e| e.to_string())
        })
        .await??;
        Ok(Self { inner })
    }

    /// The session underneath, for everything this does not name.
    pub fn blocking(&self) -> &Client {
        &self.inner
    }

    // ── what the session holds; none of this waits ──────────────────────────

    /// Every order this session knows of.
    pub fn trades(&self) -> Vec<Trade> {
        self.inner.trades()
    }

    /// Every order the venue is still working.
    pub fn open_trades(&self) -> Vec<Trade> {
        self.inner.open_trades()
    }

    /// One order, by the number it was placed under.
    pub fn trade(&self, order_id: i64) -> Option<Trade> {
        self.inner.trade(order_id)
    }

    /// What the account holds.
    pub fn positions(&self) -> Vec<Position> {
        self.inner.positions()
    }

    /// What the account is worth, line by line.
    pub fn account_values(&self) -> Vec<AccountValue> {
        self.inner.account_values()
    }

    /// Every trade this session has been told about.
    pub fn fills(&self) -> Vec<Fill> {
        self.inner.fills()
    }

    /// Every account this login holds.
    pub fn managed_accounts(&self) -> Vec<String> {
        self.inner.managed_accounts()
    }

    /// How many times the session has been told something.
    pub fn changes(&self) -> u64 {
        self.inner.changes()
    }

    /// What the account holds, priced.
    pub fn holdings(&self) -> Vec<super::Holding> {
        self.inner.holdings()
    }

    /// What the account has made or lost, if the venue has said.
    pub fn pnl(&self) -> Option<super::Pnl> {
        self.inner.pnl()
    }

    /// Every notice the venue has broadcast this session.
    pub fn bulletins(&self) -> Vec<super::Bulletin> {
        self.inner.bulletins()
    }

    /// What the venue has said about requests since this was last asked.
    ///
    /// Nothing waits on these, so unread they read as nothing having happened:
    /// a stream that never prints looks like a quiet market. Reading them
    /// clears them.
    pub fn notices(&self) -> Vec<super::Notice> {
        self.inner.notices()
    }

    /// Every five-second bar this session has been sent.
    pub fn live_bars(&self) -> Vec<super::LiveBar> {
        self.inner.live_bars()
    }

    /// Every headline this session has been sent.
    pub fn news(&self) -> Vec<super::NewsTick> {
        self.inner.news()
    }

    /// Five-second bars on a contract, as the venue closes them.
    ///
    /// Not moved off the reactor: subscribing sends and returns. Reading the
    /// stream blocks, so read it from a task of its own.
    pub fn live_bar_stream(&self, contract: &Contract) -> Result<super::LiveBars, Refusal> {
        self.inner.live_bar_stream(contract)
    }

    /// Every headline the session is subscribed to, as it is published.
    pub fn news_stream(&self) -> super::News {
        self.inner.news_stream()
    }

    /// One quote each for several contracts, now.
    pub async fn quotes(
        &self, contracts: &[Contract], timeout: Duration,
    ) -> Result<Vec<Option<crate::types::Quote>>, Refusal> {
        let contracts = contracts.to_vec();
        off_the_reactor!(self, |client| client.quotes(&contracts, timeout))
    }

    /// The latest bid, ask and last for a contract being watched.
    pub fn ticker(&self, contract: &Contract) -> Option<crate::types::Quote> {
        self.inner.ticker(contract)
    }

    /// Whether the session is carrying traffic.
    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// End the session and stop reading it.
    ///
    /// Awaited, because it waits: ending a session sends a logout and then
    /// joins the reading threads. Run inline, that join holds a runtime thread
    /// for as long as the engine takes to stop.
    pub async fn disconnect(&self) {
        let client = self.inner.clone();
        // A panic in the shutdown is not something a call that returns nothing
        // can report, and the session is ending either way.
        let _ = tokio::task::spawn_blocking(move || client.disconnect()).await;
    }

    /// Every trade printed on a contract, as it prints.
    ///
    /// Subscribing does not always only send: a contract named by symbol
    /// rather than by the venue's id has to be asked about first, and that
    /// waits on the venue and on this session's turn to ask. Done on the
    /// reactor, that stalls every other task on it, so it is done off the
    /// reactor like every other call here that can wait.
    ///
    /// Reading the stream blocks the thread that reads it, so read it from a
    /// task of its own — `spawn_blocking`, or a thread.
    pub async fn ticks(&self, contract: &Contract) -> Result<super::Ticks, Refusal> {
        let contract = contract.clone();
        off_the_reactor!(self, |client| client.ticks(&contract))
    }

    /// Everything that happens to this session's orders, as it happens.
    pub fn order_events(&self) -> super::OrderEvents {
        self.inner.order_events()
    }

    /// Wait until the venue has finished with an order.
    ///
    /// `true` if it stopped, `false` if `timeout` passed first. Awaited rather
    /// than blocked on: the order is read from memory, so what this waits for
    /// is the venue, and a runtime thread is not the thing that should do the
    /// waiting.
    pub async fn wait_done(&self, order: &PlacedOrder, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if order.is_done() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        order.is_done()
    }

    // ── asking ──────────────────────────────────────────────────────────────

    /// The one contract the venue means by this description.
    pub async fn qualify(&self, contract: Contract) -> Result<Contract, Refusal> {
        off_the_reactor!(self, |client| client.qualify(contract))
    }

    /// Everything the venue lists under this description.
    pub async fn lookup(&self, contract: &Contract) -> Result<Vec<ContractDetails>, Refusal> {
        let contract = contract.clone();
        off_the_reactor!(self, |client| client.lookup(&contract))
    }

    /// Bars of trades during regular hours, ending now.
    pub async fn bars(
        &self, contract: &Contract, duration: &str, bar_size: &str,
    ) -> Result<Vec<BarData>, Refusal> {
        let (contract, duration, bar_size) =
            (contract.clone(), duration.to_string(), bar_size.to_string());
        off_the_reactor!(self, |client| client.bars(&contract, &duration, &bar_size))
    }

    /// Start a market-data subscription, and hand back the id that withdraws it.
    ///
    /// Not moved off the reactor: this sends and returns without waiting.
    pub fn watch(&self, contract: &Contract) -> Result<i64, Refusal> {
        self.inner.watch(contract)
    }

    /// Place an order, and hand back what is known of it so far.
    pub async fn place(&self, contract: &Contract, order: &Order) -> Result<PlacedOrder, Refusal> {
        let (contract, order) = (contract.clone(), order.clone());
        off_the_reactor!(self, |client| client.place(&contract, &order))
    }

    /// An entry and the two exits that close it, placed as one instruction.
    pub async fn place_bracket(
        &self, contract: &Contract, side: &str, quantity: f64,
        entry: f64, take_profit: f64, stop_loss: f64,
    ) -> Result<[i64; 3], Refusal> {
        let (contract, side) = (contract.clone(), side.to_string());
        off_the_reactor!(self, |client| client.place_bracket(
            &contract, &side, quantity, entry, take_profit, stop_loss
        ))
    }

    /// Link orders so that a fill on one withdraws the rest.
    ///
    /// Not moved off the reactor: this writes two fields and sends nothing.
    pub fn one_cancels_all(orders: &mut [Order], group: &str, kind: i32) {
        Client::one_cancels_all(orders, group, kind);
    }

    /// Withdraw an order.
    pub fn cancel_order(&self, order_id: i64) -> Result<(), Refusal> {
        self.inner.cancel_order(order_id)
    }

    /// Withdraw every order this account has working, on every connection.
    pub fn cancel_all(&self) -> Result<(), Refusal> {
        self.inner.cancel_all()
    }

    /// Run a scan and hand back what it found.
    pub async fn scan(
        &self, instrument: &str, location: &str, scan_code: &str, most: u32,
    ) -> Result<Vec<crate::api::client::ScanRow>, Refusal> {
        let (instrument, location, scan_code) =
            (instrument.to_string(), location.to_string(), scan_code.to_string());
        off_the_reactor!(self, |client| client.scan(&instrument, &location, &scan_code, most))
    }

    /// When a contract trades, over a window ending now.
    pub async fn schedule(
        &self, contract: &Contract, duration: &str,
    ) -> Result<crate::api::client::Schedule, Refusal> {
        let (contract, duration) = (contract.clone(), duration.to_string());
        off_the_reactor!(self, |client| client.schedule(&contract, &duration))
    }

    /// What the corporate-events calendar says it carries.
    pub async fn calendar_schema(&self) -> Result<String, Refusal> {
        off_the_reactor!(self, |client| client.calendar_schema())
    }

    /// The calendar's events for one contract.
    pub async fn calendar_events(&self, con_id: i64) -> Result<String, Refusal> {
        off_the_reactor!(self, |client| client.calendar_events(con_id))
    }

    /// What an order would cost and what it would do to the margin, without
    /// placing it.
    pub async fn what_if(&self, contract: &Contract, order: &Order) -> Result<OrderState, Refusal> {
        let (contract, order) = (contract.clone(), order.clone());
        off_the_reactor!(self, |client| client.what_if(&contract, &order))
    }
}

#[cfg(test)]
mod tests {
    /// Every question this surface answers is one the blocking session answers
    /// under the same name, and every reading of state is on both.
    ///
    /// A name on one and not the other is a program that cannot be moved
    /// between them, which is the only reason to have two.
    #[test]
    fn the_two_sessions_answer_the_same_names() {
        use std::collections::BTreeSet;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api/session");
        // Only what the session itself answers. The file also holds the order
        // a caller gets back from placing one, and its methods are not the
        // session's — compared whole, the two lists differ by things that were
        // never meant to be on both.
        let names = |text: &str, of: &str| -> BTreeSet<String> {
            let at = text.find(of).unwrap_or_else(|| panic!("no {of}"));
            let block = &text[at..text[at..].find("\n}\n").map_or(text.len(), |e| at + e)];
            block
                .lines()
                .filter_map(|l| l.trim().strip_prefix("pub fn ").or(l.trim().strip_prefix("pub async fn ")))
                .filter_map(|l| l.split('(').next())
                .map(str::to_string)
                .collect()
        };
        let here = std::fs::read_to_string(root.join("asynchronous.rs")).expect("this file");
        let there = std::fs::read_to_string(root.join("mod.rs")).expect("the blocking one");
        // `blocking` reaches the session underneath and has nothing to reach on
        // the session itself; `client` is that reach, under the other name.
        // `blocking` reaches the session underneath and has nothing to reach
        // on the session itself. `wait_done` is on the order on the blocking
        // side, where waiting is a thread doing nothing; here it is on the
        // session, because a future needs a runtime to poll it and the order
        // has none.
        let not_on_both = BTreeSet::from(["blocking".to_string(), "wait_done".to_string()]);
        let mine: BTreeSet<String> = names(&here, "impl AsyncClient {")
            .difference(&not_on_both)
            .cloned().collect();
        // What the blocking session reaches, not only what it names: it
        // dereferences to the client, so a question answered there is answered
        // on it. This surface has no such fall-through on purpose — a blocking
        // call reached by accident from a runtime is the bug the surface exists
        // to prevent — so anything it offers, it names.
        //
        // That makes the two sets different sizes and the parity two separate
        // statements: nothing here is unreachable there, and nothing the
        // blocking session names is missing here. The raw protocol calls the
        // client carries are reached through `blocking()`, so they are not in
        // the second set.
        let session_names = names(&there, "impl Client {");
        let mut theirs = session_names.clone();
        for entry in std::fs::read_dir(root.parent().expect("api").join("client")).expect("the client") {
            let path = entry.expect("a readable entry").path();
            if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "tests.rs") {
                let text = std::fs::read_to_string(&path).expect("a readable file");
                theirs.extend(
                    text.lines()
                        .filter_map(|l| l.trim().strip_prefix("pub fn "))
                        .filter_map(|l| l.split('(').next())
                        .map(str::to_string),
                );
            }
        }
        assert!(mine.len() >= 15, "the reader found the methods: {mine:?}");
        assert!(session_names.len() >= 15, "and the blocking session's: {session_names:?}");

        let unreachable: Vec<_> = mine.difference(&theirs).collect();
        assert!(
            unreachable.is_empty(),
            "asked on this session and reachable on neither: {unreachable:?}",
        );
        // The second direction. Comparing the first set with itself narrowed
        // to the second cannot fail, and would pass a question dropped from
        // this surface or added only to the blocking one.
        let absent: Vec<_> = session_names.difference(&mine).collect();
        assert!(
            absent.is_empty(),
            "named on the blocking session and not on this one: {absent:?}",
        );
    }
}
