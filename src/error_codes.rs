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
    /// Binding orders entered elsewhere was asked for by a client that is not
    /// the one they are bound to.
    ///
    /// The number the venue answers this with. It answers the request
    /// itself rather than sending it on, so this is the whole of what happens.
    pub const AUTO_BIND_NOT_THIS_CLIENT: i32 = 327;

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

/// The code a request number that is already watching something is refused
/// under.
pub const DUPLICATE_TICKER_ID: i32 = 102;

/// The code a combination naming no legs is refused under.
pub const COMBINATION_NEEDS_LEGS: i32 = 314;

/// The code a combination leg this client cannot state is refused under.
pub const COMBINATION_LEG_INVALID: i32 = 313;

/// The code an order on a security type the account may not trade is refused
/// under.
pub const SECURITY_NOT_PERMITTED: i32 = 203;

/// The code an order missing the price that triggers it is refused under.
pub const TRIGGER_PRICE_MISSING: i32 = 403;

/// The code an order stating a trigger method the venue does not carry is
/// refused under.
pub const TRIGGER_METHOD_INVALID: i32 = 146;

/// The code a condition on an order that does not describe its contract is
/// refused under.
pub const CONDITION_CONTRACT_INCOMPLETE: i32 = 147;

/// The code an unreadable good-till date is refused under.
pub const GOOD_TILL_DATE_INVALID: i32 = 334;

/// The code a change that would move an order to a type the change cannot
/// restate is refused under.
pub const CHANGE_CANNOT_CHANGE_TYPE: i32 = 329;

/// The code a log level outside the range the client carries is refused under.
pub const LOG_LEVEL_INVALID: i32 = 319;

/// The code a placement under a number the venue has already worked an order
/// under is refused under.
///
/// The venue refuses a repeated number only while it is still working one, so
/// after a fill it takes the placement as a new order -- which is how a caller
/// retrying what it thought had failed ends up holding two.
pub const DUPLICATE_ORDER_ID: i32 = 103;

/// The code a request number already running a scan is refused under.
///
/// Its own number rather than the one a quote subscription is refused under,
/// for the same reason a historical query has its own: a caller branches on
/// which request it made.
pub const DUPLICATE_SCANNER_SUBSCRIPTION: i32 = 385;

/// The code a withdrawal naming a scan this client is not running is answered
/// under.
pub const NO_SUCH_SCANNER_SUBSCRIPTION: i32 = 365;

/// The code a withdrawal naming a book this client does not hold is answered
/// under.
///
/// Its own number rather than the one a quote subscription is withdrawn under:
/// the catalogue names depth separately, and a caller branches on which of the
/// two it asked for.
pub const NO_SUCH_BOOK: i32 = 310;

/// The code the venue's restart of a book is reported under.
///
/// Not a refusal: it tells a caller holding a book to empty it before applying
/// what follows, which is the only way a book that shrank can shrink on the
/// caller's side.
pub const DEPTH_BOOK_RESET: i32 = 317;

/// The code a withdrawal naming a subscription this client does not hold is
/// answered under.
///
/// A caller branches on this to learn that its own record and this client's
/// disagree -- that what it believes it is withdrawing is not something this
/// client holds. Silence is indistinguishable from a withdrawal that worked.
pub const NO_SUCH_SUBSCRIPTION: i32 = 300;

/// The code a request number already answering a historical query is refused
/// under.
///
/// A separate number from the one a live quote subscription is refused under:
/// the two are different requests and a caller branches on which it made.
pub const DUPLICATE_HISTORICAL_QUERY: i32 = 386;

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
