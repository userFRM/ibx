//! A stream a caller can iterate.
//!
//! Some of what a venue sends is one answer to one question, and some of it
//! keeps arriving until it is stopped: bars every five seconds, every change to
//! a book, every trade as it prints. The callback shape delivers those by
//! calling a handler; this shape hands back something a caller can loop over.
//!
//! Two things it does that a bare loop over a queue would not:
//!
//! Withdrawing. A subscription that is dropped stops asking. Left to itself the
//! venue keeps sending to a session nobody is reading, which costs the account a
//! line it is not using and eventually the ones it is.
//!
//! Ending. The venue's refusal of the request ends the stream rather than
//! leaving a caller blocked on data that is never coming, and the refusal is
//! kept where the caller can read it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::bridge::SharedState;

/// How long a caller waits for the next item before the stream ends.
///
/// A stream that blocks forever cannot be got out of; one that returns
/// immediately is a busy loop. This is the compromise, and a caller wanting a
/// different one says so with [`Subscription::with_timeout`].
pub const DEFAULT_IDLE: Duration = Duration::from_secs(30);

/// How long to sleep between looks. Short enough not to add noticeable delay to
/// a five-second bar, long enough not to spin a core.
const POLL: Duration = Duration::from_millis(5);

/// Reads whatever has arrived for one request.
type Take<T> = Box<dyn Fn(&SharedState, i64) -> Vec<T> + Send>;

/// Withdraws a request.
type Cancel = Box<dyn Fn(i64) + Send>;

/// A stream of one kind of thing, for one request.
pub struct Subscription<T> {
    req_id: i64,
    shared: Arc<SharedState>,
    take: Take<T>,
    cancel: Option<Cancel>,
    buffered: VecDeque<T>,
    idle: Duration,
    /// The venue's own words, if it refused the request.
    refusal: Option<(i64, String)>,
    done: bool,
}

impl<T> Subscription<T> {
    /// Build a stream over a request already sent.
    ///
    /// `take` reads whatever has arrived for this request; `cancel` withdraws
    /// it. A stream with no way to withdraw is one the venue keeps feeding
    /// after the caller has gone, so `cancel` is asked for rather than optional
    /// by default.
    pub fn new(
        req_id: i64,
        shared: Arc<SharedState>,
        take: impl Fn(&SharedState, i64) -> Vec<T> + Send + 'static,
        cancel: impl Fn(i64) + Send + 'static,
    ) -> Self {
        Self {
            req_id,
            shared,
            take: Box::new(take),
            cancel: Some(Box::new(cancel)),
            buffered: VecDeque::new(),
            idle: DEFAULT_IDLE,
            refusal: None,
            done: false,
        }
    }

    /// A stream that nothing needs to withdraw — one the venue closes itself.
    pub fn without_cancel(
        req_id: i64,
        shared: Arc<SharedState>,
        take: impl Fn(&SharedState, i64) -> Vec<T> + Send + 'static,
    ) -> Self {
        Self {
            req_id,
            shared,
            take: Box::new(take),
            cancel: None,
            buffered: VecDeque::new(),
            idle: DEFAULT_IDLE,
            refusal: None,
            done: false,
        }
    }

    /// How long to wait for the next item before the stream ends.
    pub fn with_timeout(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }

    /// The id the venue answers this stream under.
    pub fn req_id(&self) -> i64 {
        self.req_id
    }

    /// The venue's refusal, if the stream ended because of one.
    ///
    /// A stream that ended because the venue said no and one that ended because
    /// nothing came look the same from the outside. This tells them apart.
    pub fn refusal(&self) -> Option<&(i64, String)> {
        self.refusal.as_ref()
    }

    /// Stop asking, now, rather than when this is dropped.
    pub fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel(self.req_id);
        }
        self.done = true;
    }

    /// The next item, or nothing once the stream has ended.
    ///
    /// Blocks while it waits. Ends on the venue's refusal, on `cancel`, or when
    /// nothing has arrived for the idle period.
    pub fn next_item(&mut self) -> Option<T> {
        if let Some(item) = self.buffered.pop_front() {
            return Some(item);
        }
        if self.done {
            return None;
        }

        let deadline = Instant::now() + self.idle;
        loop {
            for item in (self.take)(&self.shared, self.req_id) {
                self.buffered.push_back(item);
            }
            if let Some(item) = self.buffered.pop_front() {
                return Some(item);
            }
            if let Some((code, message)) = self.shared.reference.take_error_for(self.req_id as u32) {
                self.refusal = Some((code as i64, message));
                self.done = true;
                return None;
            }
            if Instant::now() >= deadline {
                self.done = true;
                return None;
            }
            std::thread::sleep(POLL);
        }
    }
}

impl<T> Iterator for Subscription<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.next_item()
    }
}

impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel(self.req_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    fn state() -> Arc<SharedState> {
        Arc::new(SharedState::new())
    }

    /// What has arrived is handed over in the order it arrived.
    #[test]
    fn a_stream_yields_what_has_arrived() {
        let shared = state();
        let served = Arc::new(AtomicI64::new(0));
        let s = Arc::clone(&served);
        let mut sub: Subscription<i64> = Subscription::without_cancel(
            1,
            Arc::clone(&shared),
            move |_, _| {
                let n = s.fetch_add(1, Ordering::Relaxed);
                if n < 3 { vec![n] } else { vec![] }
            },
        )
        .with_timeout(Duration::from_millis(50));

        assert_eq!(sub.next_item(), Some(0));
        assert_eq!(sub.next_item(), Some(1));
        assert_eq!(sub.next_item(), Some(2));
        // Nothing more arrives, so the stream ends rather than blocking.
        assert_eq!(sub.next_item(), None);
    }

    /// Dropping stops asking. Left running, the venue keeps sending to a
    /// session nobody reads, which costs the account a line it is not using.
    #[test]
    fn dropping_a_stream_withdraws_it() {
        let shared = state();
        let cancelled = Arc::new(AtomicI64::new(0));
        let c = Arc::clone(&cancelled);
        {
            let _sub: Subscription<i64> = Subscription::new(
                42,
                Arc::clone(&shared),
                |_, _| vec![],
                move |req_id| { c.store(req_id, Ordering::Relaxed); },
            );
        }
        assert_eq!(cancelled.load(Ordering::Relaxed), 42);
    }

    /// Withdrawing twice would ask the venue to stop something already stopped.
    #[test]
    fn a_stream_withdrawn_by_hand_is_not_withdrawn_again_when_dropped() {
        let shared = state();
        let count = Arc::new(AtomicI64::new(0));
        let c = Arc::clone(&count);
        {
            let mut sub: Subscription<i64> = Subscription::new(
                7,
                Arc::clone(&shared),
                |_, _| vec![],
                move |_| { c.fetch_add(1, Ordering::Relaxed); },
            );
            sub.cancel();
        }
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    /// A refusal ends the stream and is kept, so a caller can tell "the venue
    /// said no" from "nothing came".
    #[test]
    fn the_venues_refusal_ends_the_stream_and_is_readable() {
        let shared = state();
        shared.reference.push_historical_error(9, 354, "Requested market data is not subscribed".into());
        let mut sub: Subscription<i64> = Subscription::without_cancel(
            9,
            Arc::clone(&shared),
            |_, _| vec![],
        )
        .with_timeout(Duration::from_secs(30));

        assert_eq!(sub.next_item(), None);
        let (code, message) = sub.refusal().expect("the refusal is kept");
        assert_eq!(*code, 354);
        assert!(message.contains("not subscribed"));
    }

    /// A stream that ended because nothing came holds no refusal, which is how
    /// the two are told apart.
    #[test]
    fn a_stream_that_simply_went_quiet_holds_no_refusal() {
        let shared = state();
        let mut sub: Subscription<i64> = Subscription::without_cancel(
            1,
            Arc::clone(&shared),
            |_, _| vec![],
        )
        .with_timeout(Duration::from_millis(20));
        assert_eq!(sub.next_item(), None);
        assert!(sub.refusal().is_none());
    }
}
