//! How the venue names an order's state, and what those names mean.
//!
//! One vocabulary, because everything that reports an order reads it: the
//! shared state deciding what belongs in an open-order snapshot, the engine
//! stringifying a report, and both surfaces handing the string to a caller.

use super::OrderStatus;

/// True when `status` names an IB order state that is still working on the broker.
/// Whitelist (rather than blacklist) so non-canonical or empty strings — and
/// any future terminal states added by IB — are treated as "not open".
///
/// A partly filled order that is still working is `Submitted`, and the filled
/// and remaining quantities beside it are what say how far it has got. There is
/// no separate status for it in this vocabulary, and a program reading one
/// finds it in neither the active set nor the done set.
#[inline]
pub fn is_open_status(status: &str) -> bool {
    matches!(
        status,
        "ApiPending"
            | "PendingSubmit"
            | "PendingCancel"
            | "PendingReplace"
            | "PreSubmitted"
            | "Submitted"
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

/// True when `status`/`completed_status` describe an order that has finished:
/// filled, withdrawn, or refused by the venue.
///
/// A refusal reads as "Inactive" — [`order_status_str`] has no refused string —
/// and only the completed status beside it tells the two apart: the venue
/// states one for a refusal and leaves it empty for an order it merely holds.
/// An "Inactive" row carrying a completed status has therefore finished, while
/// one without can still return to working, and a cancel-all must still reach
/// it. `Unknown` states the opposite of a conclusion and is not finished
/// either.
///
/// Everyone who must not let a late frame reopen a finished order reads the
/// verdict here, so the two cannot answer the question differently.
#[inline]
pub fn is_terminal_status(status: &str, completed_status: &str) -> bool {
    matches!(status, "Filled" | "Cancelled" | "Rejected")
        || (status == "Inactive" && !completed_status.is_empty())
}

/// Convert OrderStatus enum to ibapi-compatible string.
#[inline]
pub fn order_status_str(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::PendingSubmit => "PendingSubmit",
        OrderStatus::PreSubmitted => "PreSubmitted",
        OrderStatus::Submitted => "Submitted",
        OrderStatus::PendingCancel => "PendingCancel",
        // The venue's own word for an order whose change it has not made
        // yet. Named as a pending cancel, a caller watching its order saw a
        // withdrawal under way while a modification was, and its cancel
        // logic fired on a change.
        OrderStatus::PendingReplace => "PendingReplace",
        OrderStatus::Filled => "Filled",
        // The venue reports a partly filled working order as submitted, and
        // the filled and remaining quantities carry the distinction. Named as
        // a status of its own it is in neither the active set nor the done set
        // of a program written against this vocabulary, so a working order
        // read as neither running nor finished.
        OrderStatus::PartiallyFilled => "Submitted",
        OrderStatus::Cancelled => "Cancelled",
        // ibapi has no "Rejected" status string — rejected orders surface as "Inactive"
        // with the rejection reason carried separately on OrderState.completedStatus.
        OrderStatus::Rejected => "Inactive",
        OrderStatus::Inactive => "Inactive",
        OrderStatus::Uncertain => "Unknown",
    }
}
