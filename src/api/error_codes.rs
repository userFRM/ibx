//! What a refusal is, and the number it carries.
//!
//! The reference client reports a refused request through `error(id, code,
//! message)` under a number the caller can branch on, rather than as a failure
//! of the call itself. A refusal raised here carries the same number, so a
//! program written against that client reads the same value whichever surface
//! it came through.

use std::fmt;

/// A request the client will not send, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The number the reference client reports this class of refusal under.
    pub code: i32,
    /// What was wrong, in words a caller can read.
    pub message: String,
}

impl Refusal {
    /// The request is malformed or contradicts itself.
    pub const VALIDATION: i32 = 321;
    /// Nothing the venue holds matches the contract described.
    pub const NO_DEFINITION: i32 = 200;
    /// The session is not up, so nothing can be sent on it.
    ///
    /// The number the reference client reports a request made before
    /// connecting, or after the connection went, under.
    pub const NOT_CONNECTED: i32 = 504;
    /// The venue said nothing at all before the wait ran out.
    ///
    /// This client's own number rather than the venue's: the reference client
    /// has none, because it does not wait — it hands a request over and leaves
    /// the caller to decide how long to care. A caller that wants to tell
    /// silence apart from a refusal branches on this; the venue never sends it.
    pub const NO_ANSWER: i32 = -1;

    /// The request is malformed or contradicts itself.
    pub fn validation(message: impl Into<String>) -> Self {
        Self { code: Self::VALIDATION, message: message.into() }
    }

    /// Nothing the venue holds matches the contract described.
    pub fn no_definition(message: impl Into<String>) -> Self {
        Self { code: Self::NO_DEFINITION, message: message.into() }
    }

    /// Nothing can be sent, because there is no session to send it on.
    pub fn not_connected(message: impl Into<String>) -> Self {
        Self { code: Self::NOT_CONNECTED, message: message.into() }
    }

    /// The venue said nothing before the wait ran out.
    pub fn no_answer(message: impl Into<String>) -> Self {
        Self { code: Self::NO_ANSWER, message: message.into() }
    }

    /// A refusal exactly as the venue stated it, under its own number.
    ///
    /// The number is the point: a caller branches on it the way it would
    /// against the reference client, which reports the same one on its error
    /// callback. Flattened into text it can only be matched on prose.
    pub fn stated(code: i32, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refusal {}

/// The validators shared with the Rust client state a reason and no number;
/// a request refused for a reason of that kind failed validation.
impl From<String> for Refusal {
    fn from(message: String) -> Self {
        Self::validation(message)
    }
}

impl From<&str> for Refusal {
    fn from(message: &str) -> Self {
        Self::validation(message)
    }
}

impl From<Refusal> for String {
    fn from(refusal: Refusal) -> String {
        refusal.message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reason_alone_is_a_validation_refusal() {
        let refusal: Refusal = "quantity must be positive".to_string().into();
        assert_eq!(refusal.code, Refusal::VALIDATION);
        assert_eq!(refusal.to_string(), "quantity must be positive");
    }

    /// A refusal the venue stated keeps the venue's number.
    ///
    /// Flattened into text — which is what happened to every answer the
    /// waiting calls returned — the number is still readable by a person and
    /// no longer branchable by a program. A caller against the reference
    /// client gets it on the error callback and switches on it.
    #[test]
    fn a_refusal_the_venue_stated_keeps_its_number() {
        let refused = Refusal::stated(10197, "no market data during competing session");
        assert_eq!(refused.code, 10197);
        assert_eq!(refused.to_string(), "no market data during competing session");

        // A request that never left has its own number, and it is the one the
        // reference client reports for a request made with no session. Left as
        // an untyped message it became a validation failure, which says the
        // venue refused something it never saw.
        let gone = Refusal::not_connected("Engine stopped: sending on a closed channel");
        assert_eq!(gone.code, 504);
        assert_ne!(gone.code, Refusal::VALIDATION);

        // Silence is this client's own answer, not the venue's, and says so
        // with a number the venue never sends.
        let quiet = Refusal::no_answer("no answer within 15s to head timestamp");
        assert_eq!(quiet.code, Refusal::NO_ANSWER);
        assert!(quiet.code < 0, "not a number the venue can state");
        assert_ne!(quiet.code, Refusal::VALIDATION, "silence is not a bad request");

        // And it still reads as prose wherever a caller wants prose.
        assert_eq!(String::from(refused), "no market data during competing session");
    }

    #[test]
    fn an_unnamed_contract_is_reported_under_its_own_number() {
        assert_eq!(Refusal::no_definition("nothing matches").code, 200);
    }
}
