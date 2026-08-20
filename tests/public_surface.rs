//! Every public path this crate has published, named the way a caller names it.
//!
//! A module that moves takes its paths with it unless something says otherwise,
//! and the compiler will not say so: a `pub use` deleted during a refactor
//! breaks a downstream program and nothing here. Twenty paths were lost that
//! way in one afternoon, and every one of them still had a working replacement
//! — the loss was the old name, not the code behind it.
//!
//! So the names live here. Adding one is a decision; removing one is a
//! breaking change, and this file is where that gets noticed.

#![allow(unused_imports, dead_code)]

// ── The surface a program is written against ────────────────────────────────
use ibx::api::error_codes::Refusal;
use ibx::api::reliability::{ReconnectConfig, RecoveryBudget};
use ibx::api::settings::{GatewaySettings, SessionSettings};
use ibx::api::types::{
    BarData, CommissionAndFeesReport, Contract, ContractDescription, ContractDetails,
    Execution, Order, OrderState, TagValue,
};
use ibx::api::{Client, EClient, EClientConfig, Subscription, Wrapper};
use ibx::{EClient as RootEClient, Refusal as RootRefusal};

// ── Reachable because a program already reaches it ──────────────────────────
//
// These moved during the reorganisation. The path each was published under is
// kept, so what follows is the whole of what "nothing a caller names has moved"
// means.
use ibx::client_core::{is_open_or_reactivatable, is_open_status, order_status_str};
use ibx::config::{
    IbExpiry, TimestampBuf, chrono_free_timestamp, days_to_ymd, ib_datetime_to_unix,
    midnight_days_ago, parse_ib_expiry, unix_to_ib_datetime, unix_to_ib_utc_dash,
};
use ibx::control::calendar::CalendarQuery;
use ibx::gateway::{build_mktdata_subscribe, build_mktdata_unsubscribe};
use ibx::protocol::fix::{fix_build, fix_parse, fix_read_deadline};

/// Items a `use` cannot name on its own: an associated function, and a method.
#[test]
fn every_published_name_still_resolves() {
    let _ = ibx::gateway::chrono_free_timestamp();
    let _ = ibx::gateway::days_to_ymd(0);
    let _ = ibx::client_core::ClientCore::contract_identity("", 0.0, "", "", "");
    let _ = ibx::client_core::parse_algo_params("", &[]);
    let _ = ibx::types::model::contract_identity("", 0.0, "", "", "");

    // Handing the open connections to the loop is named on the engine, where
    // what it builds lives. Named on the session module instead, that module
    // named the engine while the engine was already naming the session.
    let _built_by_the_engine = ibx::engine::hot_loop::HotLoop::for_session;
}

/// The three a caller configures, reachable from the crate root as well as
/// through `api`, because that is where a caller looks first.
#[test]
fn what_a_caller_configures_is_reachable_from_the_root() {
    let _: ibx::settings::GatewaySettings = Default::default();
    let _: ibx::reliability::ReconnectConfig = Default::default();
    let _ = ibx::error_codes::Refusal::NOT_CONNECTED;
}
