//! What to do about a connection that went away.
//!
//! A dropped connection is not one event. The terminal keeps a reason for every
//! way a session can end and decides what to do from it — a validation failure
//! waits thirty seconds where an ordinary broken socket waits one — and the
//! difference matters most at the two ends: a login the server refused is not
//! going to be accepted on the next attempt, and a session another login took
//! is not ours to take back.
//!
//! So a failure is classified before it is retried, and the classification
//! decides whether to retry at all.

use std::io;
use std::time::Duration;

/// Why a connection attempt or a session ended.
///
/// Named for what the client can tell from where it sits. The terminal
/// distinguishes more cases than this, but most of them are indistinguishable
/// from outside its own state machine, and a reason nobody can observe is not
/// worth reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    /// The socket broke, the peer went away, or a read timed out. Ordinary, and
    /// the common case.
    Transport,
    /// Nothing came back within the liveness deadline.
    NoResponse,
    /// The server refused the credentials. Trying again with the same ones
    /// changes nothing.
    AuthorizationFailed,
    /// Another login took the session. It belongs to whoever connected last,
    /// and racing them for it helps nobody.
    TakenOver,
    /// The server is up but not serving yet.
    NotReady,
    /// The client asked to stop.
    ByDesign,
}

/// What a reason says about trying again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Try again on the ladder.
    Retry,
    /// Try again, but slowly: the far side needs time more than it needs
    /// another attempt.
    RetrySlowly,
    /// Do not try again. Nothing about repeating this makes it work, and the
    /// caller has to act.
    Stop,
}

impl DisconnectReason {
    /// Classify a failed attempt from what the transport reported.
    pub fn from_error(e: &io::Error) -> Self {
        match e.kind() {
            // The logon was answered with a rejection. The credentials, the
            // account or the entitlement is wrong, and the next attempt carries
            // exactly the same ones.
            io::ErrorKind::PermissionDenied => Self::AuthorizationFailed,
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Self::NoResponse,
            _ => {
                // The reason often survives only in the message the server
                // sent, which is the one place a competing login is named.
                let text = e.to_string().to_ascii_lowercase();
                if text.contains("competing") || text.contains("another user")
                    || text.contains("logged in from")
                {
                    Self::TakenOver
                } else if text.contains("not ready") || text.contains("try again later") {
                    Self::NotReady
                } else {
                    Self::Transport
                }
            }
        }
    }

    pub fn recovery(self) -> Recovery {
        match self {
            Self::Transport | Self::NoResponse => Recovery::Retry,
            // Slowly, not never: a session is taken by whoever connected last,
            // and reconnecting straight into it takes it back off them. Two
            // clients doing that to each other never stop. Waiting long enough
            // that a person notices is the useful behaviour.
            Self::TakenOver => Recovery::RetrySlowly,
            Self::NotReady => Recovery::RetrySlowly,
            Self::AuthorizationFailed | Self::ByDesign => Recovery::Stop,
        }
    }

    /// Whether the caller has to do something before this can work.
    pub fn is_terminal(self) -> bool {
        self.recovery() == Recovery::Stop
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transport => "the connection broke",
            Self::NoResponse => "the server stopped answering",
            Self::AuthorizationFailed => "the server refused the credentials",
            Self::TakenOver => "another login took the session",
            Self::NotReady => "the server is not ready",
            Self::ByDesign => "the client asked to stop",
        }
    }
}

/// How long to wait before the next attempt, for a reason that warrants one.
///
/// The ordinary ladder climbs; a reason that wants slowing down starts past the
/// top of it, so a client that has lost its session to someone else is not
/// back inside a second taking it off them again.
pub fn delay_for(reason: DisconnectReason, ladder: Duration) -> Duration {
    match reason.recovery() {
        Recovery::Retry => ladder,
        Recovery::RetrySlowly => ladder.max(SLOW_FLOOR),
        Recovery::Stop => Duration::ZERO,
    }
}

/// Floor for a reason that needs the far side, or a person, to change something.
const SLOW_FLOOR: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_login_is_not_retried() {
        let e = io::Error::new(io::ErrorKind::PermissionDenied, "Farm logon rejected: bad user");
        let reason = DisconnectReason::from_error(&e);
        assert_eq!(reason, DisconnectReason::AuthorizationFailed);
        assert!(reason.is_terminal(), "the same credentials fail the same way next time");
    }

    /// Two clients on one session, each reconnecting the instant it is dropped,
    /// take it off each other forever. Neither ever keeps it.
    #[test]
    fn a_session_taken_by_another_login_is_not_snatched_back() {
        let e = io::Error::other("competing live session detected");
        let reason = DisconnectReason::from_error(&e);
        assert_eq!(reason, DisconnectReason::TakenOver);
        assert!(!reason.is_terminal(), "it may come back, so keep trying");
        assert!(
            delay_for(reason, Duration::from_secs(2)) >= Duration::from_secs(30),
            "but not immediately, or the two clients fight over it",
        );
    }

    #[test]
    fn an_ordinary_drop_keeps_the_ladder_it_was_given() {
        let e = io::Error::other("connection reset by peer");
        let reason = DisconnectReason::from_error(&e);
        assert_eq!(reason, DisconnectReason::Transport);
        assert_eq!(delay_for(reason, Duration::from_secs(7)), Duration::from_secs(7));
    }

    #[test]
    fn a_stalled_read_is_the_server_not_answering() {
        let e = io::Error::new(io::ErrorKind::TimedOut, "no data start after auth");
        assert_eq!(DisconnectReason::from_error(&e), DisconnectReason::NoResponse);
        assert!(!DisconnectReason::from_error(&e).is_terminal());
    }
}
