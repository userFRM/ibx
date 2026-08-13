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

    /// The request is malformed or contradicts itself.
    pub fn validation(message: impl Into<String>) -> Self {
        Self { code: Self::VALIDATION, message: message.into() }
    }

    /// Nothing the venue holds matches the contract described.
    pub fn no_definition(message: impl Into<String>) -> Self {
        Self { code: Self::NO_DEFINITION, message: message.into() }
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

    #[test]
    fn an_unnamed_contract_is_reported_under_its_own_number() {
        assert_eq!(Refusal::no_definition("nothing matches").code, 200);
    }
}
