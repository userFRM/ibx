//! What the session currently holds.
//!
//! The other Rust client here answers a question and forgets it: a caller who
//! wants to know what an order did asks again, and a caller who wants to know
//! what happened while they were not asking finds out that nothing did. That
//! is the reference client's shape, and it is the right one for a program with
//! its own event loop.
//!
//! This keeps what arrives. One thread reads the session and writes here;
//! everything below reads what it wrote. A position, an order, a fill and a
//! quote are things a program looks at, not questions it asks.

use std::collections::BTreeMap;
use std::sync::mpsc::SyncSender;

use crate::api::wrapper::Wrapper;
use crate::types::model::{Contract, Execution, Order, OrderState};

/// Where an order stands, as the venue last said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrderStatus {
    /// The venue's own word for it.
    pub status: String,
    /// How much has filled.
    pub filled: f64,
    /// How much has not.
    pub remaining: f64,
    /// What the filled part averaged.
    pub average_price: f64,
    /// The venue's own number for the order, stable across sessions.
    pub perm_id: i64,
    /// What the venue said when it would not take the order.
    pub why_held: String,
}

/// Statuses under which an order is still working.
///
/// Taken from what the venue sends rather than inferred: anything not named
/// here has stopped. Inferring the other way round — treating an unrecognised
/// status as still working — leaves a program waiting on an order that is gone.
const WORKING: [&str; 5] = [
    "PendingSubmit", "PendingCancel", "PreSubmitted", "Submitted", "ApiPending",
];

/// One trade against an order.
#[derive(Debug, Clone)]
pub struct Fill {
    /// What filled.
    pub contract: Contract,
    /// The venue's report of the trade.
    pub execution: Execution,
}

/// An order and what has become of it.
///
/// The status changes under a caller holding this, which is what makes it a
/// trade rather than a receipt.
#[derive(Debug, Clone)]
pub struct Trade {
    /// What is being traded.
    pub contract: Contract,
    /// The order as it was placed.
    pub order: Order,
    /// Where it stands.
    pub status: OrderStatus,
    /// What the venue said about it when it was placed.
    pub state: Option<OrderState>,
    /// Every trade against it, in the order they happened.
    pub fills: Vec<Fill>,
}

impl Trade {
    /// Whether the venue is still working it.
    pub fn is_active(&self) -> bool {
        WORKING.contains(&self.status.status.as_str())
    }

    /// Whether it has stopped — filled, cancelled or refused.
    pub fn is_done(&self) -> bool {
        !self.is_active()
    }
}

/// One trade printed on a contract being watched tick by tick.
#[derive(Debug, Clone)]
pub struct Tick {
    /// When it printed, in nanoseconds since the epoch.
    pub time: i64,
    /// What it printed at.
    pub price: f64,
    /// How much printed.
    pub size: f64,
    /// Which venue printed it.
    pub exchange: String,
}

/// Something that happened to an order.
#[derive(Debug, Clone)]
pub struct OrderEvent {
    /// The order it happened to.
    pub order_id: i64,
    /// Where it stands now.
    pub status: String,
    /// How much has filled.
    pub filled: f64,
    /// How much has not.
    pub remaining: f64,
    /// The trade, where this event is one. A status change carries none.
    pub fill: Option<Fill>,
}

/// A holding, and what it is worth now.
///
/// [`Position`] is what the account has and what it cost. This is the same
/// holding priced: a caller asking "how am I doing" wants this one.
#[derive(Debug, Clone)]
pub struct Holding {
    /// What is held.
    pub contract: Contract,
    /// How much, negative when short.
    pub quantity: f64,
    /// What the venue last marked it at.
    pub market_price: f64,
    /// What that makes the holding worth.
    pub market_value: f64,
    /// What it cost, on average, per unit.
    pub average_cost: f64,
    /// What it is up or down, unsold.
    pub unrealized: f64,
    /// What it has made or lost, sold.
    pub realized: f64,
    /// Whose.
    pub account: String,
}

/// What the account has made or lost.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pnl {
    /// Today.
    pub daily: f64,
    /// On what is still held.
    pub unrealized: f64,
    /// On what has been closed.
    pub realized: f64,
}

/// One five-second bar, as the venue closed it.
#[derive(Debug, Clone)]
pub struct LiveBar {
    /// The request this arrived under, which is what tells one subscription's
    /// bars from another's.
    pub req_id: i64,
    /// When the bar closed, in seconds since the epoch.
    pub time: i64,
    /// Where it opened.
    pub open: f64,
    /// Its highest.
    pub high: f64,
    /// Its lowest.
    pub low: f64,
    /// Where it closed.
    pub close: f64,
    /// How much traded in it.
    pub volume: f64,
    /// The volume-weighted average price across it.
    pub wap: f64,
    /// How many trades made it up.
    pub count: i32,
}

/// One headline, as it was published.
#[derive(Debug, Clone)]
pub struct NewsTick {
    /// The subscription it arrived on.
    pub req_id: i64,
    /// When it was published.
    pub time: i64,
    /// Who published it.
    pub provider: String,
    /// Their reference for the article, which fetches the body.
    pub article_id: String,
    /// What it says.
    pub headline: String,
}

/// One notice the venue broadcast to everybody.
#[derive(Debug, Clone)]
pub struct Bulletin {
    /// The venue's number for it.
    pub id: i64,
    /// What kind of notice it is.
    pub kind: i32,
    /// What it says.
    pub message: String,
    /// Which exchange it came from.
    pub exchange: String,
}

/// A holding.
///
/// A futures position arrives carrying the venue's id for the contract and
/// nothing else — no symbol, no security type, no currency. The definition
/// service does not answer a query keyed on that id for a future, though it
/// does for a share, and it holds the contract perfectly well when asked by
/// name. So the id and the quantity are what is known, until something in the
/// program names that contract for another reason and the definition is
/// cached. Asked and answered against a session: `793356217` came back
/// "no security definition has been found" by id, and by name came back as
/// `MESU6`, September 2026, under that same id.
#[derive(Debug, Clone)]
pub struct Position {
    /// Which account holds it.
    pub account: String,
    /// What is held.
    pub contract: Contract,
    /// How much, negative when short.
    pub quantity: f64,
    /// What it cost, on average, per unit.
    pub average_cost: f64,
}

/// One line of what an account is worth.
#[derive(Debug, Clone)]
pub struct AccountValue {
    /// What it names.
    pub tag: String,
    /// What it says.
    pub value: String,
    /// In what.
    pub currency: String,
    /// Whose.
    pub account: String,
}

/// Everything one session has been told, kept as it arrives.
#[derive(Debug, Default)]
pub struct LiveState {
    trades: BTreeMap<i64, Trade>,
    positions: Vec<Position>,
    values: BTreeMap<(String, String, String), AccountValue>,
    executions: Vec<Fill>,
    accounts: Vec<String>,
    /// Bumped whenever anything below changes, so a caller can wait for the
    /// next change rather than for a length of time.
    changes: u64,
    /// Where a caller asked for the ticks on one contract to be sent, by the
    /// request the subscription was asked for under.
    holdings: Vec<Holding>,
    pnl: Option<Pnl>,
    bulletins: Vec<Bulletin>,
    live_bars: Vec<LiveBar>,
    news: Vec<NewsTick>,
    bar_streams: Vec<(i64, SyncSender<LiveBar>)>,
    news_streams: Vec<SyncSender<NewsTick>>,
    tick_streams: Vec<(i64, SyncSender<Tick>)>,
    /// Where a caller asked for what happens to orders to be sent.
    order_streams: Vec<SyncSender<OrderEvent>>,
}

impl LiveState {
    /// Every order this session knows of, oldest first.
    pub fn trades(&self) -> Vec<Trade> {
        self.trades.values().cloned().collect()
    }

    /// Every order the venue is still working.
    pub fn open_trades(&self) -> Vec<Trade> {
        self.trades.values().filter(|t| t.is_active()).cloned().collect()
    }

    /// One order, by the number it was placed under.
    pub fn trade(&self, order_id: i64) -> Option<Trade> {
        self.trades.get(&order_id).cloned()
    }

    /// What the account holds.
    pub fn positions(&self) -> Vec<Position> {
        self.positions.clone()
    }

    /// What the account is worth, line by line.
    pub fn account_values(&self) -> Vec<AccountValue> {
        self.values.values().cloned().collect()
    }

    /// Every trade this session has been told about.
    pub fn fills(&self) -> Vec<Fill> {
        self.executions.clone()
    }

    /// What the account holds, priced.
    pub fn holdings(&self) -> Vec<Holding> {
        self.holdings.clone()
    }

    /// What the account has made or lost, if the venue has said.
    pub fn pnl(&self) -> Option<Pnl> {
        self.pnl
    }

    /// Every notice the venue has broadcast this session.
    pub fn bulletins(&self) -> Vec<Bulletin> {
        self.bulletins.clone()
    }

    /// Every five-second bar this session has been sent.
    pub fn live_bars(&self) -> Vec<LiveBar> {
        self.live_bars.clone()
    }

    /// Every headline this session has been sent.
    pub fn news(&self) -> Vec<NewsTick> {
        self.news.clone()
    }

    /// Every account this login holds.
    pub fn accounts(&self) -> Vec<String> {
        self.accounts.clone()
    }

    /// How many times anything here has changed.
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// Keep an order the caller has just placed, before the venue has said
    /// anything about it.
    ///
    /// Without this, a caller who places an order and asks for it back is told
    /// there is no such order — the venue has not answered yet, and the answer
    /// is what creates it.
    pub(crate) fn remember(&mut self, order_id: i64, trade: Trade) {
        self.trades.entry(order_id).or_insert(trade);
        self.changed();
    }

    /// Take what another record was told.
    ///
    /// For an answer collected outside this one — a subscription asked for as
    /// the session opens, whose reply arrives before anything is reading.
    pub(crate) fn absorb(&mut self, other: Self) {
        for held in other.positions {
            if !self.positions.iter().any(|p| {
                p.account == held.account && p.contract.con_id == held.contract.con_id
            }) {
                self.positions.push(held);
            }
        }
        for (key, value) in other.values {
            self.values.entry(key).or_insert(value);
        }
        for (id, trade) in other.trades {
            self.trades.entry(id).or_insert(trade);
        }
        if self.accounts.is_empty() {
            self.accounts = other.accounts;
        }
        self.changed();
    }

    /// Send the ticks on one subscription to a caller who asked for them.
    pub(crate) fn stream_ticks(&mut self, req_id: i64, to: SyncSender<Tick>) {
        self.tick_streams.push((req_id, to));
    }

    /// Send the bars on one subscription to a caller who asked for them.
    pub(crate) fn stream_bars(&mut self, req_id: i64, to: SyncSender<LiveBar>) {
        self.bar_streams.push((req_id, to));
    }

    /// Send the headlines to a caller who asked for them.
    pub(crate) fn stream_news(&mut self, to: SyncSender<NewsTick>) {
        self.news_streams.push(to);
    }

    /// Send what happens to orders to a caller who asked for it.
    pub(crate) fn stream_order_events(&mut self, to: SyncSender<OrderEvent>) {
        self.order_streams.push(to);
    }

    /// Hand an event to everyone still listening, and forget the ones who
    /// stopped: a caller who dropped their stream is not a caller to keep
    /// sending to, and a send that fails is how that is known.
    fn tell<T: Clone>(streams: &mut Vec<SyncSender<T>>, what: &T) {
        streams.retain(|to| to.try_send(what.clone()).is_ok());
    }

    fn changed(&mut self) {
        self.changes = self.changes.wrapping_add(1);
    }
}

impl Wrapper for LiveState {
    fn open_order(&mut self, order_id: i64, contract: &Contract, order: &Order, state: &OrderState) {
        let trade = self.trades.entry(order_id).or_insert_with(|| Trade {
            contract: contract.clone(),
            order: order.clone(),
            status: OrderStatus::default(),
            state: None,
            fills: Vec::new(),
        });
        // The venue's own statement of the order outranks what was sent: a
        // field it did not take comes back as what it did take, and a caller
        // reading this back is reading what is working, not what was asked.
        trade.contract = contract.clone();
        trade.order = order.clone();
        trade.state = Some(state.clone());
        if trade.status.status.is_empty() {
            trade.status.status = state.status.clone();
        }
        self.changed();
    }

    fn order_status(
        &mut self, order_id: i64, status: &str, filled: f64, remaining: f64,
        average_price: f64, perm_id: i64, _parent_id: i64, _last_fill_price: f64,
        _client_id: i64, why_held: &str, _mkt_cap_price: f64,
    ) {
        let trade = self.trades.entry(order_id).or_insert_with(|| Trade {
            contract: Contract::default(),
            order: Order { order_id, ..Default::default() },
            status: OrderStatus::default(),
            state: None,
            fills: Vec::new(),
        });
        trade.status = OrderStatus {
            status: status.to_string(),
            filled,
            remaining,
            // A status arriving after a fill can state no average; the one
            // already reported is the better answer.
            average_price: if average_price > 0.0 { average_price } else { trade.status.average_price },
            perm_id,
            why_held: why_held.to_string(),
        };
        Self::tell(&mut self.order_streams, &OrderEvent {
            order_id, status: status.to_string(), filled, remaining, fill: None,
        });
        self.changed();
    }

    fn tick_by_tick_all_last(
        &mut self, req_id: i64, _tick_type: i32, time: i64, price: f64, size: f64,
        _attrib: &crate::types::model::TickAttribLast, exchange: &str, _conditions: &str,
    ) {
        let tick = Tick { time, price, size, exchange: exchange.to_string() };
        // Only to whoever asked for this contract's ticks. Everyone gets
        // everyone else's otherwise, and a caller watching one thing has to
        // filter out the rest.
        self.tick_streams
            .retain(|(id, to)| *id != req_id || to.try_send(tick.clone()).is_ok());
    }

    fn exec_details(&mut self, _req_id: i64, contract: &Contract, execution: &Execution) {
        let fill = Fill { contract: contract.clone(), execution: execution.clone() };
        let (status, filled, remaining) = self
            .trades
            .get(&execution.order_id)
            .map(|t| (t.status.status.clone(), t.status.filled, t.status.remaining))
            .unwrap_or_default();
        Self::tell(&mut self.order_streams, &OrderEvent {
            order_id: execution.order_id,
            status,
            filled,
            remaining,
            fill: Some(fill.clone()),
        });
        // Against the order it belongs to as well as the session's own list,
        // so a caller holding one trade sees its fills without matching them
        // up themselves.
        if let Some(trade) = self.trades.get_mut(&execution.order_id) {
            trade.fills.push(fill.clone());
        }
        self.executions.push(fill);
        self.changed();
    }

    fn position(&mut self, account: &str, contract: &Contract, quantity: f64, average_cost: f64) {
        let held = Position {
            account: account.to_string(),
            contract: contract.clone(),
            quantity,
            average_cost,
        };
        // The venue reports a holding as it stands, so the same contract
        // arriving again replaces what was there. Appended, a position closed
        // and reopened would be counted twice.
        let same = |p: &Position| {
            p.account == held.account && p.contract.con_id == held.contract.con_id
        };
        match self.positions.iter_mut().find(|p| same(p)) {
            Some(existing) => *existing = held,
            None => self.positions.push(held),
        }
        // A holding the venue reports as zero is one the account no longer has.
        self.positions.retain(|p| p.quantity != 0.0);
        self.changed();
    }

    fn real_time_bar(
        &mut self, req_id: i64, time: i64, open: f64, high: f64, low: f64,
        close: f64, volume: f64, wap: f64, count: i32,
    ) {
        let bar = LiveBar { req_id, time, open, high, low, close, volume, wap, count };
        // Kept as well as streamed: a caller who subscribed and then looked
        // rather than iterating still finds the bars that arrived.
        self.live_bars.push(bar.clone());
        self.bar_streams
            .retain(|(id, to)| *id != req_id || to.try_send(bar.clone()).is_ok());
        self.changed();
    }

    fn tick_news(
        &mut self, req_id: i64, time: i64, provider: &str, article_id: &str,
        headline: &str, _extra: &str,
    ) {
        let tick = NewsTick {
            req_id, time, provider: provider.to_string(),
            article_id: article_id.to_string(), headline: headline.to_string(),
        };
        self.news.push(tick.clone());
        Self::tell(&mut self.news_streams, &tick);
        self.changed();
    }

    fn update_portfolio(
        &mut self, contract: &Contract, quantity: f64, market_price: f64,
        market_value: f64, average_cost: f64, unrealized: f64, realized: f64,
        account: &str,
    ) {
        let priced = Holding {
            contract: contract.clone(), quantity, market_price, market_value,
            average_cost, unrealized, realized, account: account.to_string(),
        };
        // Priced as it stands, so the same contract replaces itself rather
        // than accumulating a row per mark.
        match self.holdings.iter_mut().find(|h| {
            h.account == priced.account && h.contract.con_id == priced.contract.con_id
        }) {
            Some(existing) => *existing = priced,
            None => self.holdings.push(priced),
        }
        self.holdings.retain(|h| h.quantity != 0.0);
        self.changed();
    }

    fn pnl(&mut self, _req_id: i64, daily: f64, unrealized: f64, realized: f64) {
        self.pnl = Some(Pnl { daily, unrealized, realized });
        self.changed();
    }

    fn update_news_bulletin(&mut self, id: i64, kind: i32, message: &str, exchange: &str) {
        self.bulletins.push(Bulletin {
            id, kind, message: message.to_string(), exchange: exchange.to_string(),
        });
        self.changed();
    }

    fn update_account_value(&mut self, key: &str, value: &str, currency: &str, account: &str) {
        // Keyed by all three: the same tag arrives once per currency, and one
        // login can hold more than one account.
        self.values.insert(
            (account.to_string(), key.to_string(), currency.to_string()),
            AccountValue {
                tag: key.to_string(),
                value: value.to_string(),
                currency: currency.to_string(),
                account: account.to_string(),
            },
        );
        self.changed();
    }

    fn managed_accounts(&mut self, accounts: &str) {
        self.accounts = accounts.split(',').filter(|a| !a.is_empty()).map(str::to_string).collect();
        self.changed();
    }
}
