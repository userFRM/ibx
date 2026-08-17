//! A session you look at, rather than one you interrogate.
//!
//! The reference client's shape is a request and a callback: everything a
//! program wants to know it has to have been listening for at the moment the
//! venue said it. That is the right shape for a program with its own event
//! loop and a poor one for everything else, and it is why
//! [`ib_async`](https://github.com/ib-api-reloaded/ib_async) exists in Python.
//! This is the same idea in Rust.
//!
//! One thread reads the session and keeps what arrives. A position, an order,
//! a fill and a quote are things you look at:
//!
//! ```no_run
//! # use ibx::{Client, Config};
//! # use ibx::types::model::{Contract, Order};
//! # use std::time::Duration;
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::connect(&Config {
//!     username: "user".into(), password: "pass".into(),
//!     paper: true, ..Default::default()
//! })?;
//!
//! let spy = client.qualify(Contract::stock("SPY"))?;
//! let order = client.place(&spy, &Order::limit("BUY", 100.0, 42.50))?;
//!
//! order.wait_done(Duration::from_secs(30));
//! println!("{} — {} filled", order.status(), order.fills().len());
//!
//! for position in client.positions() {
//!     println!("{} {}", position.contract.symbol, position.quantity);
//! }
//! client.disconnect();
//! # Ok(())
//! # }
//! ```
//!
//! It is the same session as [`EClient`] underneath — [`client`](Client::client)
//! reaches it — so nothing here is a second way to do what that one does. What
//! it adds is that the answers stay.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error_codes::Refusal;
use crate::types::model::{Contract, ContractDetails, Order};

use super::client::{EClient, EClientConfig};

mod fanout;
mod state;
pub use state::{AccountValue, Fill, LiveState, OrderStatus, Position, Trade};

#[cfg(feature = "async")]
mod asynchronous;
#[cfg(feature = "async")]
pub use asynchronous::AsyncClient;

#[cfg(test)]
mod tests;

/// Something a session tells its handlers about.
///
/// The same trait the reference client's callbacks arrive on, so a handler
/// written for one is a handler for the other. Implement the calls you want and
/// leave the rest: every one has a body that does nothing.
pub use crate::api::wrapper::Wrapper as Handler;

/// How long the reader waits before looking again when nothing arrived.
///
/// It holds the session's turn while it reads, and a question waits for a gap,
/// so this is also how long a question waits at worst before it can be asked.
const BETWEEN_READS: Duration = Duration::from_millis(1);

/// A session, and everything it has been told.
///
/// Cloning shares the session rather than opening another: the venue allows one
/// per login, and a second would take the first one's place.
#[derive(Clone)]
pub struct Client {
    client: Arc<EClient>,
    state: Arc<Mutex<LiveState>>,
    handlers: Arc<Mutex<Vec<Box<dyn Handler + Send>>>>,
    stop: Arc<AtomicBool>,
    reader: Arc<Mutex<Option<JoinHandle<()>>>>,
}

/// What the reader hands each message to: the session's own record of it, and
/// then every handler the caller registered, in the order they were added.
///
/// A message reaches all of them. Put on a queue instead, it would reach
/// whoever drained the queue first and nobody else, and a queue that filled
/// would drop it — which is why this is not a queue.
struct Fanout<'a> {
    kept: &'a mut LiveState,
    handlers: &'a mut Vec<Box<dyn Handler + Send>>,
}

impl Client {
    /// Open a session and start reading it.
    pub fn connect(config: &EClientConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let session = Self {
            client: Arc::new(EClient::connect(config)?),
            state: Arc::new(Mutex::new(LiveState::default())),
            handlers: Arc::new(Mutex::new(Vec::new())),
            stop: Arc::new(AtomicBool::new(false)),
            reader: Arc::new(Mutex::new(None)),
        };
        session.start_reading();
        // Asked for as the session opens, the way the reference client's own
        // wrapper does. Without it the account is silent until something asks,
        // and `positions()` and `account_values()` return empty lists — which
        // read as an account holding nothing rather than as nobody having
        // asked. Both are subscriptions: they answer once and then keep
        // answering, so this is the only place they are asked for.
        session.client.req_account_updates(true, "");

        // Into a record of its own and merged after, not straight into the
        // session's. The reader takes the session's turn and then the state;
        // filling the state here would take them the other way round, and two
        // locks taken in two orders is a session that stops at the first
        // moment both are wanted.
        let mut answered = LiveState::default();
        session.client.req_positions(&mut answered);
        // And what this account already has working, which may have been
        // placed by another session or on another day. Without asking, the
        // session knows only the orders it placed itself — and `open_trades()`
        // answers "none", which reads as an account with nothing working
        // rather than as nobody having asked.
        session.client.req_all_open_orders(&mut answered);
        session.kept().absorb(answered);
        Ok(session)
    }

    /// The session underneath, for every request this does not name.
    ///
    /// Reading it is free. Asking it a question takes the same turn the reader
    /// does, so the two do not compete.
    pub fn client(&self) -> &EClient {
        &self.client
    }

    /// One thread reads the session; everything else reads what it kept.
    ///
    /// It takes the session's turn while it reads and gives it back between
    /// reads. Reading without one, it would drain the answer to a question
    /// somebody else was waiting for and that question would time out on a
    /// reply that had already arrived.
    fn start_reading(&self) {
        let (client, state, handlers, stop) = (
            Arc::clone(&self.client),
            Arc::clone(&self.state),
            Arc::clone(&self.handlers),
            Arc::clone(&self.stop),
        );
        let handle = thread::Builder::new()
            .name("ibx-reader".to_string())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) && client.is_connected() {
                    {
                        let _turn = client.asking.lock().unwrap_or_else(|e| e.into_inner());
                        let mut kept = state.lock().unwrap_or_else(|e| e.into_inner());
                        let mut listening = handlers.lock().unwrap_or_else(|e| e.into_inner());
                        client.process_msgs(&mut Fanout {
                            kept: &mut kept,
                            handlers: &mut listening,
                        });
                    }
                    thread::sleep(BETWEEN_READS);
                }
            })
            .expect("a thread to read the session on");
        *self.reader.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    }

    fn kept(&self) -> MutexGuard<'_, LiveState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// End the session and stop reading it.
    pub fn disconnect(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.client.disconnect();
        if let Some(reader) = self.reader.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = reader.join();
        }
    }

    /// Whether the session is carrying traffic.
    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    /// Be told about everything the session is told, as it is told.
    ///
    /// The handler is called on the thread that reads the session, before the
    /// next message is read — so it sees every message, in order, with nothing
    /// buffered between the two and nothing to fall behind. It also means a
    /// handler that waits holds the session up: do the work somewhere else and
    /// return.
    ///
    /// More than one may be registered, and each is told everything. That is
    /// what a queue cannot do — a message taken off a queue is taken off it for
    /// everyone.
    pub fn on_event(&self, handler: impl Handler + Send + 'static) {
        self.handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Box::new(handler));
    }

    // ── what the session holds ──────────────────────────────────────────────

    /// Every order this session knows of.
    pub fn trades(&self) -> Vec<Trade> {
        self.kept().trades()
    }

    /// Every order the venue is still working.
    pub fn open_trades(&self) -> Vec<Trade> {
        self.kept().open_trades()
    }

    /// One order, by the number it was placed under.
    pub fn trade(&self, order_id: i64) -> Option<Trade> {
        self.kept().trade(order_id)
    }

    /// What the account holds.
    pub fn positions(&self) -> Vec<Position> {
        self.kept().positions()
    }

    /// What the account is worth, line by line.
    pub fn account_values(&self) -> Vec<AccountValue> {
        self.kept().account_values()
    }

    /// Every trade this session has been told about.
    pub fn fills(&self) -> Vec<Fill> {
        self.kept().fills()
    }

    /// How many times the session has been told something.
    ///
    /// For a caller waiting on the next change rather than on a length of time.
    pub fn changes(&self) -> u64 {
        self.kept().changes()
    }

    /// Every account this login holds.
    pub fn managed_accounts(&self) -> Vec<String> {
        let held = self.kept().accounts();
        if held.is_empty() { self.client.accounts.clone() } else { held }
    }

    /// The latest bid, ask and last for a contract being watched.
    ///
    /// Read without waiting on anything and without locking anything, so a
    /// program may read from any thread as often as it likes. `None` until the
    /// venue has sent a first tick, and for a contract nobody subscribed to —
    /// [`watch`](Client::watch) is what makes a quote exist.
    pub fn ticker(&self, contract: &Contract) -> Option<crate::types::Quote> {
        self.client.quote_of(contract)
    }

    // ── waiting ─────────────────────────────────────────────────────────────

    // ── asking ──────────────────────────────────────────────────────────────

    /// The one contract the venue means by this description.
    pub fn qualify(&self, contract: Contract) -> Result<Contract, Refusal> {
        self.client.qualify(contract)
    }

    /// Everything the venue lists under this description.
    pub fn lookup(&self, contract: &Contract) -> Result<Vec<ContractDetails>, Refusal> {
        self.client.lookup(contract)
    }

    /// Bars of trades during regular hours, ending now.
    pub fn bars(
        &self, contract: &Contract, duration: &str, bar_size: &str,
    ) -> Result<Vec<crate::types::model::BarData>, Refusal> {
        self.client.bars(contract, duration, bar_size)
    }

    /// Start a market-data subscription, and hand back the id that withdraws it.
    pub fn watch(&self, contract: &Contract) -> Result<i64, Refusal> {
        self.client.watch(contract)
    }

    /// Place an order, and hand back the order.
    ///
    /// Returns as soon as it is sent; the venue has not answered yet. What
    /// becomes of it is read through what this returns — the number it was
    /// placed under is bookkeeping a caller should not have to keep.
    pub fn place(&self, contract: &Contract, order: &Order) -> Result<PlacedOrder, Refusal> {
        let order_id = self.client.next_order_id();
        self.client.place_order(order_id, contract, order)?;
        let mut placed = order.clone();
        placed.order_id = order_id;
        // Held before it is returned, so a caller that reads it back before the
        // venue has said anything is told it is pending rather than told there
        // is no such order.
        self.kept().remember(order_id, Trade {
            contract: contract.clone(),
            order: placed,
            status: OrderStatus { status: "PendingSubmit".to_string(), ..Default::default() },
            state: None,
            fills: Vec::new(),
        });
        Ok(PlacedOrder { session: self.clone(), order_id })
    }

    /// An entry and the two exits that close it, placed as one instruction.
    ///
    /// The venue links them: whichever child fills withdraws the other, and
    /// neither reaches the market before the parent has a position for it to
    /// work against. Returns the three numbers, parent first — read each with
    /// [`trade`](Client::trade).
    pub fn place_bracket(
        &self, contract: &Contract, side: &str, quantity: f64,
        entry: f64, take_profit: f64, stop_loss: f64,
    ) -> Result<[i64; 3], Refusal> {
        self.client.place_bracket(contract, side, quantity, entry, take_profit, stop_loss)
    }

    /// Link orders so that a fill on one withdraws the rest.
    ///
    /// Stated on the orders, not sent: place them afterwards and the venue
    /// links them by the name they share.
    pub fn one_cancels_all(orders: &mut [Order], group: &str, kind: i32) {
        for order in orders {
            order.oca_group = group.to_string();
            order.oca_type = kind;
        }
    }

    /// Withdraw an order.
    pub fn cancel_order(&self, order_id: i64) -> Result<(), Refusal> {
        self.client.cancel_order(order_id, "")
    }

    /// Withdraw every order this account has working, on every connection.
    pub fn cancel_all(&self) -> Result<(), Refusal> {
        self.client.req_global_cancel()
    }

    /// What an order would cost and what it would do to the margin, without
    /// placing it.
    pub fn what_if(
        &self, contract: &Contract, order: &Order,
    ) -> Result<crate::types::model::OrderState, Refusal> {
        self.client.preview(contract, order)
    }
}

/// An order that has been placed, and what is becoming of it.
///
/// Held rather than looked up: the number an order was placed under is
/// bookkeeping the caller should not have to keep, and a status read through
/// one is a status read at the moment it is asked for rather than the moment
/// the order was placed.
#[derive(Clone)]
pub struct PlacedOrder {
    session: Client,
    order_id: i64,
}

impl PlacedOrder {
    /// The number the venue knows it by.
    pub fn id(&self) -> i64 {
        self.order_id
    }

    /// Everything the session has been told about it.
    pub fn trade(&self) -> Option<Trade> {
        self.session.trade(self.order_id)
    }

    /// The venue's own word for where it stands, as of now.
    pub fn status(&self) -> String {
        self.trade().map(|t| t.status.status).unwrap_or_default()
    }

    /// Whether it has stopped — filled, cancelled or refused.
    pub fn is_done(&self) -> bool {
        self.trade().is_some_and(|t| t.is_done())
    }

    /// Every trade against it so far.
    pub fn fills(&self) -> Vec<Fill> {
        self.trade().map(|t| t.fills).unwrap_or_default()
    }

    /// Wait until the venue has finished with it.
    ///
    /// `true` if it stopped, `false` if `timeout` passed first — in which case
    /// it is still working, and this said so rather than pretending otherwise.
    /// The waiting is here rather than on the session because this is the thing
    /// being waited for.
    pub fn wait_done(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.is_done() {
                return true;
            }
            thread::sleep(BETWEEN_READS);
        }
        self.is_done()
    }

    /// Withdraw it.
    pub fn cancel(&self) -> Result<(), Refusal> {
        self.session.cancel_order(self.order_id)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Only the last holder stops the session: cloning shares it, and a
        // clone going out of scope is not the caller finishing with it.
        if Arc::strong_count(&self.client) == 1 {
            self.stop.store(true, Ordering::Relaxed);
        }
    }
}
