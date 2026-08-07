//! Account-related methods: positions, PnL, account summary/updates.

use crate::api::types::PRICE_SCALE_F;
use crate::api::wrapper::Wrapper;
use crate::types::*;

use super::{Contract, EClient};

impl EClient {
    // ── Positions ──

    /// Request positions. Matches `reqPositions` in C++.
    /// Waits for server-pushed account data before delivering, then calls position_end.
    pub fn req_positions(&self, wrapper: &mut impl Wrapper) {
        // Wait for init burst to deliver account/position data (up to 5s).
        for _ in 0..500 {
            if self.shared.portfolio.account_data_received() { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Position updates (UP msgs) may still be in-flight after account data arrives.
        // If account shows positions exist but none received yet, wait up to 2s.
        if self.shared.portfolio.account().gross_position_value > 0
            && self.shared.portfolio.position_infos().is_empty()
        {
            for _ in 0..200 {
                std::thread::sleep(std::time::Duration::from_millis(10));
                if !self.shared.portfolio.position_infos().is_empty() { break; }
            }
        }
        // A holding arrives as a contract id and a quantity, and its definition
        // is fetched separately. Delivering before that lands names no
        // instrument at all, so give it a moment to arrive rather than handing
        // back a position in a contract the caller cannot identify.
        for _ in 0..150 {
            let unnamed = self.shared.portfolio.position_infos().into_iter().any(|pi| {
                pi.position != 0.0
                    && pi.symbol.is_empty()
                    && self.core.get_contract(pi.con_id, &self.shared).is_none()
            });
            if !unnamed { break; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let positions = self.shared.portfolio.position_infos();
        for pi in &positions {
            // Prefer the secdef cache (carries exchange/localSymbol/tradingClass),
            // but fall back to the wire-derived PositionInfo fields when cold.
            let c = self.core.get_contract(pi.con_id, &self.shared)
                .unwrap_or_else(|| Contract {
                    con_id: pi.con_id,
                    symbol: pi.symbol.clone(),
                    sec_type: pi.sec_type.clone(),
                    currency: pi.currency.clone(),
                    multiplier: pi.multiplier.clone(),
                    ..Default::default()
                });
            let avg_cost = pi.avg_cost as f64 / PRICE_SCALE_F;
            wrapper.position(&self.account_id, &c, pi.position, avg_cost);
        }
        wrapper.position_end();
    }

    // ── PnL ──

    /// Subscribe to account PnL updates. Matches `reqPnL` in C++.
    pub fn req_pnl(&self, req_id: i64, _account: &str, _model_code: &str) {
        self.core.subscribe_pnl(req_id);
    }

    /// Cancel PnL subscription. Matches `cancelPnL` in C++.
    pub fn cancel_pnl(&self, req_id: i64) {
        self.core.unsubscribe_pnl(req_id);
    }

    /// Subscribe to single-position PnL updates. Matches `reqPnLSingle` in C++.
    pub fn req_pnl_single(&self, req_id: i64, _account: &str, _model_code: &str, con_id: i64) {
        self.core.subscribe_pnl_single(req_id, con_id);
    }

    /// Cancel single-position PnL subscription. Matches `cancelPnLSingle` in C++.
    pub fn cancel_pnl_single(&self, req_id: i64) {
        self.core.unsubscribe_pnl_single(req_id);
    }

    // ── Account Summary ──

    /// Request account summary. Matches `reqAccountSummary` in C++.
    pub fn req_account_summary(&self, req_id: i64, _group: &str, tags: &str) {
        self.core.subscribe_account_summary(req_id, tags);
    }

    /// Cancel account summary. Matches `cancelAccountSummary` in C++.
    pub fn cancel_account_summary(&self, req_id: i64) {
        self.core.unsubscribe_account_summary(req_id);
    }

    // ── Account Updates ──

    /// Subscribe to account updates. Matches `reqAccountUpdates` in C++.
    pub fn req_account_updates(&self, subscribe: bool, _acct_code: &str) {
        self.core.subscribe_account_updates(subscribe);
    }

    /// Cancel positions subscription. Matches `cancelPositions` in C++.
    pub fn cancel_positions(&self) {
        // No-op: positions are delivered immediately by req_positions.
    }

    /// Request managed accounts. Matches `reqManagedAccts` in C++.
    ///
    /// Answered with every account this login holds, comma separated, which is
    /// the shape the reference client answers in. A login with one account is
    /// answered with that one account and no comma.
    pub fn req_managed_accts(&self, wrapper: &mut impl Wrapper) {
        wrapper.managed_accounts(&self.accounts.join(","));
    }

    /// Request account updates for multiple accounts/models. Matches `reqAccountUpdatesMulti` in C++.
    /// Account values for one account or model, answered on
    /// `account_update_multi`. The reference client answers this request on
    /// its own callbacks, not on the ones `req_account_updates` uses, and a
    /// caller written against it implements those and hears nothing otherwise.
    pub fn req_account_updates_multi(
        &self, req_id: i64, account: &str, model_code: &str, _ledger_and_nlv: bool,
        wrapper: &mut impl Wrapper,
    ) {
        let acct = self.shared.portfolio.account();
        let fields: &[(&str, f64)] = &[
            ("NetLiquidation", acct.net_liquidation as f64 / PRICE_SCALE_F),
            ("TotalCashValue", acct.total_cash_value as f64 / PRICE_SCALE_F),
            ("BuyingPower", acct.buying_power as f64 / PRICE_SCALE_F),
            ("GrossPositionValue", acct.gross_position_value as f64 / PRICE_SCALE_F),
            ("UnrealizedPnL", acct.unrealized_pnl as f64 / PRICE_SCALE_F),
            ("RealizedPnL", acct.realized_pnl as f64 / PRICE_SCALE_F),
            ("InitMarginReq", acct.init_margin_req as f64 / PRICE_SCALE_F),
            ("MaintMarginReq", acct.maint_margin_req as f64 / PRICE_SCALE_F),
        ];
        let account = if account.is_empty() { self.account_id.as_str() } else { account };
        for (key, val) in fields {
            let val_str = format!("{val:.2}");
            wrapper.account_update_multi(req_id, account, model_code, key, &val_str, "USD");
        }
        wrapper.account_update_multi_end(req_id);
    }

    /// Cancel multi-account updates. Matches `cancelAccountUpdatesMulti` in C++.
    pub fn cancel_account_updates_multi(&self, _req_id: i64) {
        // No-op: delivered immediately.
    }

    /// Request positions for multiple accounts/models. Matches `reqPositionsMulti` in C++.
    /// Holdings for one account or model, answered on `position_multi`.
    ///
    /// Answered from the holdings this session already has, rather than by
    /// pumping for them: pumping here would drain every queued event into a
    /// collector that reports holdings and discards the rest, so a caller
    /// running its own loop would lose whatever had arrived since it last
    /// pumped.
    pub fn req_positions_multi(
        &self, req_id: i64, account: &str, model_code: &str,
        wrapper: &mut impl Wrapper,
    ) {
        let held: Vec<_> = self.shared.portfolio.position_infos()
            .into_iter()
            .filter(|pi| pi.position != 0.0)
            .map(|pi| {
                let contract = self.core.get_contract(pi.con_id, &self.shared)
                    .unwrap_or_else(|| Contract {
                        con_id: pi.con_id,
                        symbol: pi.symbol.clone(),
                        sec_type: pi.sec_type.clone(),
                        currency: pi.currency.clone(),
                        multiplier: pi.multiplier.clone(),
                        ..Default::default()
                    });
                (contract, pi.position, pi.avg_cost as f64 / PRICE_SCALE_F)
            })
            .collect();
        let account = if account.is_empty() { self.account_id.as_str() } else { account };
        for (contract, position, avg_cost) in held {
            wrapper.position_multi(req_id, account, model_code, &contract, position, avg_cost);
        }
        wrapper.position_multi_end(req_id);
    }

    /// Cancel multi-account positions. Matches `cancelPositionsMulti` in C++.
    pub fn cancel_positions_multi(&self, _req_id: i64) {
        // No-op: delivered immediately.
    }

    /// Holdings the venue reports that this broker does not hold itself:
    /// positions held away at another broker, and rows it marks as shown but
    /// not held.
    ///
    /// Kept apart from `positions`, which answers what the account itself
    /// holds. The reference client has no call for these — its own front end
    /// shows them in a separate table — so this is the only way to reach them.
    pub fn positions_elsewhere(&self) -> Vec<crate::types::PositionElsewhere> {
        self.shared.portfolio.positions_elsewhere()
    }

    /// The account figures describing one of the sets of holdings the account
    /// does not hold itself, as name and value.
    ///
    /// The venue states these the same way it states the account's own, and
    /// mixing them in would overstate what the account is worth, so they are
    /// kept where the holdings they describe are kept.
    pub fn values_elsewhere(&self, held: crate::types::HeldElsewhere) -> Vec<(String, String)> {
        self.shared.portfolio.values_elsewhere(held)
    }

    /// Read account state snapshot.
    pub fn account(&self) -> AccountState {
        self.shared.portfolio.account()
    }
}
