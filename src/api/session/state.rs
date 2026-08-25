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
    /// Status as the venue reports it.
    pub status: String,
    /// How much has filled.
    pub filled: f64,
    /// How much has not.
    pub remaining: f64,
    /// What the filled part averaged.
    pub average_price: f64,
    /// Order id assigned by the venue. Stable across sessions, unlike the
    /// client order id.
    pub perm_id: i64,
    /// What the venue said when it would not take the order.
    pub why_held: String,
    /// The order this one is a child of — the parent of a bracket, or 0.
    pub parent_id: i64,
    /// What the last print went off at, as opposed to what the filled part
    /// averaged.
    pub last_fill_price: f64,
    /// Which client placed it, as the venue states it. An order this session
    /// did not place still reports here, under the client that did; zero where
    /// the venue named none.
    pub client_id: i64,
}

/// What the venue said about a request, under the number it said it with.
///
/// A session answers a question with the answer or a refusal, but the venue
/// also speaks about requests already made — a subscription it will not serve,
/// a contract it does not recognise, a connection coming and going. Those
/// arrive with no call waiting on them. Unread, a stream that never prints and
/// an iterator that never ends are indistinguishable from a quiet market.
#[derive(Debug, Clone)]
pub struct Notice {
    /// The request it answers, or -1 for one that answers no request.
    pub req_id: i64,
    /// Error code as the venue reports it.
    pub code: i64,
    /// What it said.
    pub message: String,
}

impl Notice {
    /// Whether this remarks on the connection rather than refusing anything.
    /// The venue numbers those from 2100 to 2200.
    pub fn is_notice(&self) -> bool {
        (2100..=2200).contains(&self.code)
    }
}

/// One trade against an order.
#[derive(Debug, Clone)]
pub struct Fill {
    /// What filled.
    pub contract: Contract,
    /// The venue's report of the trade.
    pub execution: Execution,
    /// What the trade cost, once the venue reports it.
    ///
    /// The trade and its cost arrive as separate reports matched by execution
    /// id, in either order. `None` until the second one arrives.
    pub commission: Option<crate::types::model::CommissionAndFeesReport>,
}

/// An order and what has become of it, as of the moment it was read.
///
/// A copy: the session goes on being told about the order, and this does not
/// change with it. Read it again for where the order stands now.
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
    ///
    /// The same statuses the open-order snapshot is built from, because they
    /// answer the same question and a second list of them answered it
    /// differently: a partial fill is reported as its own status, and it was
    /// missing here, so an order half filled and still working read as
    /// finished and `wait_done` returned on it.
    ///
    /// An order whose status this session lost — a disconnect while it was
    /// working — is not one that has stopped. It is one nothing can currently
    /// say has, and saying so would end a caller's wait on an order that may
    /// still be live. It resolves when the session does.
    pub fn is_active(&self) -> bool {
        // A parked order is not a finished one. `Inactive` is where an order
        // the venue refused and an order it is holding both land, and only the
        // completed status beside it tells the two apart: the venue states one
        // for a refusal and leaves it empty for a hold. Reading both as
        // refusals drops an order the venue may still work from `open_trades`
        // and returns `wait_done` on it. The open-order snapshot reads the
        // same way.
        crate::types::order_status::is_open_or_reactivatable(
            &self.status.status,
            self.state.as_ref().map_or("", |state| state.completed_status.as_str()),
        ) || self.status.status == "Unknown"
    }

    /// Whether it has stopped — filled, cancelled or refused.
    pub fn is_done(&self) -> bool {
        !self.is_active()
    }
}

/// One trade printed on a contract being watched tick by tick.
#[derive(Debug, Clone)]
pub struct Tick {
    /// Print time, in seconds since the epoch.
    ///
    /// Passed on unscaled, which is the unit the reference client's
    /// tick-by-tick callbacks carry.
    pub time: i64,
    /// What it printed at.
    pub price: f64,
    /// How much printed.
    pub size: f64,
    /// Which venue printed it.
    pub exchange: String,
    /// Whether the trade was outside the venue's limits.
    pub past_limit: bool,
    /// Whether it goes unreported to the tape, which is what separates every
    /// trade from the ones that print.
    pub unreported: bool,
    /// Sale condition codes as the venue reports them.
    pub conditions: String,
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
    /// What the provider states alongside it — the tone and the strength of
    /// it, where the provider states them. Carried as the venue writes it.
    pub extra: String,
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
    pub(crate) order_streams: Vec<SyncSender<OrderEvent>>,
    /// What the venue has said about requests, oldest first.
    notices: Vec<Notice>,
    /// What each fill cost, by the venue's id for the execution. Held for the
    /// fill that has not arrived yet, and read by the one that has.
    commissions: BTreeMap<String, crate::types::model::CommissionAndFeesReport>,
    /// Where a fill is, by the venue's id for it: the order it belongs to, and
    /// its place in the list below. Both lists are only ever appended to, so a
    /// place recorded here stays that fill's.
    ///
    /// Costs arrive as their own reports, one per fill, and each was matched by
    /// walking every fill the session holds — twice. Over a session that is the
    /// square of the number of fills in string comparisons, on the path that
    /// tells a caller what its trading cost.
    exec_at: std::collections::HashMap<String, (i64, usize)>,
}

/// How many of the venue's remarks are kept before the oldest is let go.
///
/// A caller that never asks is not a reason to grow without limit, and the
/// newest are the ones that explain what is happening now.
const NOTICES_KEPT: usize = 256;

/// How much of a stream is kept for a caller that looks rather than iterates.
///
/// The same reasoning as the remarks above, and the same answer: a caller that
/// subscribed and then looked still finds what arrived, and one that never
/// looks is not a reason to grow without limit. A session left running for
/// days on one bar subscription takes several thousand bars a day, and every
/// read copies the whole of it while the lock the reader wants is held.
const STREAM_KEPT: usize = 4_096;

/// How much of a full stream is let go at once.
///
/// Dropping the oldest one at a time moves everything behind it on every
/// arrival, for the whole life of a session that has reached the cap — which
/// on a one-second bar is every second, for ever. Dropping a run of them
/// leaves that cost once per run, and the run is small enough that what is
/// kept is still most of the cap.
const STREAM_RELEASED: usize = 512;

impl LiveState {
    /// What the venue has said about requests, oldest first, and clears them.
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

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
        match self.trades.get_mut(&order_id) {
            // The venue can answer before the caller's own record is written:
            // an order placed and reported inside the same millisecond leaves
            // a status here first, and a status names neither the contract nor
            // the order. Overwriting the status-only record instead leaves the
            // trade naming no instrument. What the venue has said stands; what
            // only the caller knows is filled in.
            Some(held) => {
                held.contract = trade.contract;
                held.order = trade.order;
            }
            None => {
                self.trades.insert(order_id, trade);
            }
        }
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
            match self.trades.get_mut(&id) {
                // As in `remember`: the snapshot names the contract and the
                // order, and a status arriving while it is being asked for
                // names neither. Keeping the status-only record discards the
                // answer that has both.
                Some(held) => {
                    held.contract = trade.contract;
                    held.order = trade.order;
                    if held.state.is_none() {
                        held.state = trade.state;
                    }
                    if held.status.status.is_empty() {
                        held.status = trade.status;
                    }
                }
                None => {
                    self.trades.insert(id, trade);
                }
            }
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
    ///
    /// A caller who is merely behind has not stopped. A full buffer and a
    /// dropped receiver both fail the send, and treating them alike ended the
    /// stream of anyone who read slower than the venue printed — their iterator
    /// finished, which is the same thing it does when the session closes, so
    /// there was no telling a busy moment from a closed session. Behind, the
    /// event is dropped and the caller kept.
    fn tell<T: Clone>(streams: &mut Vec<SyncSender<T>>, what: &T) {
        streams.retain(|to| !matches!(
            to.try_send(what.clone()),
            Err(std::sync::mpsc::TrySendError::Disconnected(_)),
        ));
    }

    /// Let go of everyone the session was sending to.
    ///
    /// A receiver whose senders are gone ends its iterator, which is what a
    /// stream on a session that has closed should do. Kept, the sender leaves
    /// a caller in `recv()` waiting on a session that will never speak again.
    pub(crate) fn close_streams(&mut self) {
        self.tick_streams.clear();
        self.bar_streams.clear();
        self.news_streams.clear();
        self.order_streams.clear();
    }

    fn changed(&mut self) {
        self.changes = self.changes.wrapping_add(1);
    }
}

impl Wrapper for LiveState {
    fn error(&mut self, req_id: i64, error_code: i64, error_string: &str, _advanced: &str) {
        let notice = Notice { req_id, code: error_code, message: error_string.to_string() };
        // Logged as well as kept: a caller who never asks still has this in the
        // session's own log, and the thing being explained — a stream that
        // never prints, a subscription that answers nothing — is exactly what a
        // caller looks in the log for.
        if notice.is_notice() {
            log::info!("venue notice {error_code} on request {req_id}: {error_string}");
        } else {
            log::warn!("venue refused request {req_id} with {error_code}: {error_string}");
        }
        if self.notices.len() == NOTICES_KEPT {
            self.notices.remove(0);
        }
        self.notices.push(notice);
        self.changed();
    }


    fn open_order(&mut self, order_id: i64, contract: &Contract, order: &Order, state: &OrderState) {
        let trade = self.trades.entry(order_id).or_insert_with(|| Trade {
            contract: contract.clone(),
            order: order.clone(),
            status: OrderStatus::default(),
            state: None,
            fills: Vec::new(),
        });
        // The venue's report replaces what was sent. A field the venue did not
        // accept comes back as the value it did accept, so this holds the
        // working order rather than the requested one.
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
        average_price: f64, perm_id: i64, parent_id: i64, last_fill_price: f64,
        client_id: i64, why_held: &str, _mkt_cap_price: f64,
    ) {
        let trade = self.trades.entry(order_id).or_insert_with(|| Trade {
            contract: Contract::default(),
            order: Order { order_id, ..Default::default() },
            status: OrderStatus::default(),
            state: None,
            fills: Vec::new(),
        });
        // The venue assigns the permanent id, so an order states none when it
        // is placed and learns it from the first report. It belongs on the
        // order as well as on the status: it is how an order is named across
        // sessions, and `cancel_order_by_perm_id` could not address an order
        // this session had placed itself while it sat only on the status.
        if trade.order.perm_id == 0 && perm_id != 0 {
            trade.order.perm_id = perm_id;
        }
        trade.status = OrderStatus {
            status: status.to_string(),
            filled,
            remaining,
            // A status arriving after a fill can state no average; the one
            // already reported is the better answer. Nothing is what zero
            // means here, and only zero: an instrument can trade at a negative
            // price, and reading anything below zero as unstated reported the
            // previous average, or none, for exactly those.
            average_price: if average_price != 0.0 {
                average_price
            } else {
                trade.status.average_price
            },
            perm_id,
            why_held: why_held.to_string(),
            // Carried rather than discarded: the venue states them on every
            // report and the session exposes them nowhere else. A bracket's
            // child does not otherwise name its parent, and an order this
            // session did not place does not name who did.
            parent_id,
            last_fill_price,
            client_id,
        };
        Self::tell(&mut self.order_streams, &OrderEvent {
            order_id, status: status.to_string(), filled, remaining, fill: None,
        });
        self.changed();
    }

    fn tick_by_tick_all_last(
        &mut self, req_id: i64, _tick_type: i32, time: i64, price: f64, size: f64,
        attrib: &crate::types::model::TickAttribLast, exchange: &str, conditions: &str,
    ) {
        // What the venue says about the print as well as the print: a trade
        // that goes unreported to the tape is not one to build a series from,
        // and the sale codes are how a caller tells that from an ordinary one.
        let tick = Tick {
            time, price, size,
            exchange: exchange.to_string(),
            past_limit: attrib.past_limit,
            unreported: attrib.unreported,
            conditions: conditions.to_string(),
        };
        // Only to whoever asked for this contract's ticks. Everyone gets
        // everyone else's otherwise, and a caller watching one thing has to
        // filter out the rest. A caller who is behind keeps their subscription:
        // see `tell`.
        self.tick_streams.retain(|(id, to)| {
            *id != req_id
                || !matches!(
                    to.try_send(tick.clone()),
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)),
                )
        });
        // Counted, as every other thing the session is told is. Left out, the
        // one class of change this carries most of did not move `changes()`, so
        // a caller waiting on it for the next thing to happen waited through a
        // busy tape.
        self.changed();
    }

    fn exec_details(&mut self, _req_id: i64, contract: &Contract, execution: &Execution) {
        let fill = Fill {
            contract: contract.clone(),
            execution: execution.clone(),
            commission: self.commissions.get(&execution.exec_id).cloned(),
        };
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
        self.exec_at.insert(
            fill.execution.exec_id.clone(),
            (execution.order_id, self.executions.len()),
        );
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
        if self.live_bars.len() >= STREAM_KEPT {
            self.live_bars.drain(..STREAM_RELEASED);
        }
        self.live_bars.push(bar.clone());
        self.bar_streams
            .retain(|(id, to)| {
                *id != req_id
                    || !matches!(
                        to.try_send(bar.clone()),
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)),
                    )
            });
        self.changed();
    }

    fn commission_and_fees_report(
        &mut self, report: &crate::types::model::CommissionAndFeesReport,
    ) {
        // What a fill cost arrives as its own report, matched to the trade by
        // execution id. Unread, the session names what traded and never what it
        // cost. Either report can arrive first, so this is kept for the fill
        // that has not come yet and written onto the one that has.
        //
        // Found rather than searched for. The order it belongs to and its place
        // in the session's own list are both recorded when the fill arrives, so
        // neither list is walked: only the fills of the one order are looked
        // through, and an order has few.
        if let Some(&(order_id, at)) = self.exec_at.get(&report.exec_id) {
            if let Some(fill) = self
                .trades
                .get_mut(&order_id)
                .and_then(|trade| {
                    trade.fills.iter_mut().find(|f| f.execution.exec_id == report.exec_id)
                })
            {
                fill.commission = Some(report.clone());
            }
            // The session's own list holds the same fills, so the cost is
            // written there too. The trade arrives before its cost, so a fill
            // read through `fills()` otherwise reports no commission whatever
            // the venue states afterwards.
            if let Some(fill) = self.executions.get_mut(at) {
                fill.commission = Some(report.clone());
            }
        }
        self.commissions.insert(report.exec_id.clone(), report.clone());
        self.changed();
    }

    fn tick_news(
        &mut self, req_id: i64, time: i64, provider: &str, article_id: &str,
        headline: &str, extra: &str,
    ) {
        let tick = NewsTick {
            req_id, time, provider: provider.to_string(),
            article_id: article_id.to_string(), headline: headline.to_string(),
            extra: extra.to_string(),
        };
        if self.news.len() >= STREAM_KEPT {
            self.news.drain(..STREAM_RELEASED);
        }
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
        if self.bulletins.len() >= STREAM_KEPT {
            self.bulletins.drain(..STREAM_RELEASED);
        }
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
