//! How the venue names an order's state, and what those names mean.
//!
//! One vocabulary, because everything that reports an order reads it: the
//! shared state deciding what belongs in an open-order snapshot, the engine
//! stringifying a report, and both surfaces handing the string to a caller.

use super::OrderStatus;

/// True when `status` names an IB order state that is still working on the broker.
/// Whitelist (rather than blacklist) so non-canonical or empty strings — and
/// any future terminal states added by IB — are treated as "not open".
#[inline]
pub fn is_open_status(status: &str) -> bool {
    matches!(
        status,
        "ApiPending"
            | "PendingSubmit"
            | "PendingCancel"
            | "PreSubmitted"
            | "Submitted"
            | "PartiallyFilled"
    )
}

/// True when `status`/`completed_status` describe an order that belongs in
/// the open-order snapshot: either genuinely open per [`is_open_status`], or
/// a genuinely-Inactive order (FIX 39=I) that can still reactivate.
///
/// `order_status_str` collapses both Rejected (39=8) and Inactive (39=I) to
/// the single ibapi string "Inactive" (ibapi has no Rejected string), so
/// widening `is_open_status` to admit "Inactive" would also readmit rejected
/// orders into the open book — the trap this function avoids by checking
/// `completed_status` too. It is populated only for terminal statuses
/// (Filled/Cancelled/Rejected) and stays empty for a genuine Inactive, so an
/// empty `completed_status` on an "Inactive" row means the order is parked,
/// not dead.
#[inline]
pub fn is_open_or_reactivatable(status: &str, completed_status: &str) -> bool {
    is_open_status(status) || (status == "Inactive" && completed_status.is_empty())
}

/// Convert OrderStatus enum to ibapi-compatible string.
#[inline]
pub fn order_status_str(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::PendingSubmit => "PendingSubmit",
        OrderStatus::PreSubmitted => "PreSubmitted",
        OrderStatus::Submitted => "Submitted",
        OrderStatus::PendingCancel => "PendingCancel",
        OrderStatus::PendingReplace => "PendingCancel", // IB API has no PendingReplace string
        OrderStatus::Filled => "Filled",
        OrderStatus::PartiallyFilled => "PartiallyFilled",
        OrderStatus::Cancelled => "Cancelled",
        // ibapi has no "Rejected" status string — rejected orders surface as "Inactive"
        // with the rejection reason carried separately on OrderState.completedStatus.
        OrderStatus::Rejected => "Inactive",
        OrderStatus::Inactive => "Inactive",
        OrderStatus::Uncertain => "Unknown",
    }
}
