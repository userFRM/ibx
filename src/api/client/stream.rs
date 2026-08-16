//! Reading what the session pushes, without writing the match.
//!
//! A session pushes what it was not asked for: a trade printed, an order
//! filled, a transport lost and regained. [`connect_with_events`] hands back a
//! channel carrying all of it, which is the right shape for a program that
//! handles everything and a poor one for a program that wants trades.
//!
//! [`Events`] is that channel with the match already written. Each method below
//! yields one kind and steps over the rest, so what a reader sees is the loop
//! and not the sorting.
//!
//! ```no_run
//! # use ibx::{Client, EClientConfig};
//! # use ibx::types::model::Contract;
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let (ib, events) = Client::connect_with_events(&EClientConfig {
//!     username: "user".into(), password: "pass".into(),
//!     paper: true, ..Default::default()
//! }, 1024)?;
//!
//! let spy = ib.qualify(Contract::stock("SPY"))?;
//! ib.req_tick_by_tick_data(1, &spy, "Last", 0, false)?;
//!
//! for trade in events.trades().take(10) {
//!     println!("{} at {}", trade.size, trade.price);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! There is one of these per session, and taking a stream from it takes the
//! session's events with it: a kind stepped over is a kind delivered nowhere.
//! A program that wants two kinds reads [`all`](Events::all) and matches, which
//! is what these are written over.
//!
//! [`connect_with_events`]: super::EClient::connect_with_events

use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::bridge::Event;
use crate::types::{Fill, InstrumentId, OrderUpdate, TbtQuote, TbtTrade, TickNews};

/// Everything a session pushes, in the order it happened.
///
/// The engine writes to this channel as messages arrive, so nothing has to be
/// pumped for these to fill: a program may sit on one of the streams below and
/// nothing else. The channel holds what was asked for at
/// [`connect_with_events`](super::EClient::connect_with_events); an event that
/// arrives at a full one is discarded rather than made to wait, so a reader
/// that falls behind loses what happened while it was behind and the session
/// carries on at its own speed. Ask for a capacity that covers the longest a
/// reader might be away.
pub struct Events {
    rx: Receiver<Event>,
}

impl Events {
    pub(crate) fn new(rx: Receiver<Event>) -> Self {
        Self { rx }
    }

    /// Everything, in order, until the session ends.
    pub fn all(self) -> impl Iterator<Item = Event> {
        self.rx.into_iter()
    }

    /// The next event, or `None` if nothing arrives within `timeout`.
    ///
    /// For a program with something else to do between events. A `None` here
    /// means nothing arrived, not that the session ended.
    pub fn next_within(&self, timeout: Duration) -> Option<Event> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Every trade printed on a contract under a tick-by-tick subscription.
    pub fn trades(self) -> impl Iterator<Item = TbtTrade> {
        self.only(|e| match e {
            Event::TbtTrade(t) => Some(t),
            _ => None,
        })
    }

    /// Every change to the best bid or offer under a tick-by-tick subscription.
    pub fn quotes(self) -> impl Iterator<Item = TbtQuote> {
        self.only(|e| match e {
            Event::TbtQuote(q) => Some(q),
            _ => None,
        })
    }

    /// Every fill against this session's orders, partial or whole.
    pub fn fills(self) -> impl Iterator<Item = Fill> {
        self.only(|e| match e {
            Event::Fill(f) => Some(f),
            _ => None,
        })
    }

    /// Every change of state an order goes through.
    pub fn order_updates(self) -> impl Iterator<Item = OrderUpdate> {
        self.only(|e| match e {
            Event::OrderUpdate(u) => Some(u),
            _ => None,
        })
    }

    /// Every headline the session is subscribed to.
    pub fn news(self) -> impl Iterator<Item = TickNews> {
        self.only(|e| match e {
            Event::News(n) => Some(n),
            _ => None,
        })
    }

    /// Which contract's quote just changed.
    ///
    /// The tick itself is not carried: a quote is held as state and read with
    /// [`quote_of`](super::EClient::quote_of), so a reader that falls behind
    /// reads the current price rather than working through stale ones.
    pub fn tick_of(self) -> impl Iterator<Item = InstrumentId> {
        self.only(|e| match e {
            Event::Tick(i) => Some(i),
            _ => None,
        })
    }

    /// Every loss of the transport, and every recovery of it.
    ///
    /// `true` for a session that is carrying traffic again, `false` for one
    /// that has just lost it. A program that stands down on a loss needs both,
    /// or an overnight outage leaves it stood down for good.
    pub fn connectivity(self) -> impl Iterator<Item = bool> {
        self.only(|e| match e {
            Event::Reconnected => Some(true),
            Event::Disconnected => Some(false),
            _ => None,
        })
    }

    fn only<T>(self, pick: impl Fn(Event) -> Option<T>) -> impl Iterator<Item = T> {
        self.rx.into_iter().filter_map(pick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    /// A stream yields its own kind and steps over the others. Yielding one it
    /// was not asked for would hand a program a value of a shape it does not
    /// handle; stopping at one would end the stream on the first unrelated
    /// thing the session pushed, which on a live session is immediate.
    #[test]
    fn a_stream_yields_its_own_kind_and_steps_over_the_rest() {
        let (tx, rx) = sync_channel(16);
        tx.send(Event::Reconnected).unwrap();
        tx.send(Event::Tick(7)).unwrap();
        tx.send(Event::Disconnected).unwrap();
        tx.send(Event::Tick(9)).unwrap();
        drop(tx);
        assert_eq!(
            Events::new(rx).tick_of().collect::<Vec<_>>(),
            vec![7, 9],
            "both ticks, neither notice, in the order they happened",
        );

        let (tx, rx) = sync_channel(16);
        tx.send(Event::Tick(1)).unwrap();
        tx.send(Event::Disconnected).unwrap();
        tx.send(Event::Reconnected).unwrap();
        drop(tx);
        assert_eq!(
            Events::new(rx).connectivity().collect::<Vec<_>>(),
            vec![false, true],
            "a loss and the recovery after it, in that order",
        );
    }

    /// Nothing arriving is not the session ending. Reported as the end, a
    /// program that waits on a quiet market stands down on the quiet.
    #[test]
    fn a_wait_that_finds_nothing_says_so_without_ending_the_stream() {
        let (tx, rx) = sync_channel::<Event>(4);
        let events = Events::new(rx);
        assert!(events.next_within(Duration::from_millis(5)).is_none());
        tx.send(Event::Reconnected).unwrap();
        assert!(events.next_within(Duration::from_millis(50)).is_some());
    }
}
