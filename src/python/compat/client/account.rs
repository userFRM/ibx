//! Account-related methods: positions, PnL, account summary/updates.

use pyo3::prelude::*;

use crate::error_codes::Refusal;
use crate::types::*;
use std::sync::atomic::Ordering;
use super::EClient;
use super::super::contract::Contract;
use super::super::super::types::PRICE_SCALE_F;

impl EClient {
    /// The contract a position is a position in. Prefers the secdef cache,
    /// which carries exchange/localSymbol/tradingClass, and falls back to the
    /// wire-derived `PositionInfo` fields when it is cold.
    pub(crate) fn position_contract(&self, pi: &PositionInfo, shared: &crate::bridge::SharedState) -> Contract {
        self.core.get_contract(pi.con_id, shared)
            .map(|ac| Contract::from_api(&ac))
            .unwrap_or_else(|| Contract {
                con_id: pi.con_id,
                symbol: pi.symbol.clone(),
                sec_type: pi.sec_type.clone(),
                currency: pi.currency.clone(),
                multiplier: pi.multiplier.clone(),
                ..Default::default()
            })
    }
}

#[pymethods]
impl EClient {
    /// Request P&L updates for the account.
    ///
    /// `model_code` is taken and not applied. One session holds one account
    /// here, and the venue states its figures for that account without being
    /// asked which, so there is no second account or model portfolio to name.
    #[pyo3(signature = (req_id, account, model_code=""))]
    fn req_pnl(&self, py: Python<'_>, req_id: i64, account: &str, model_code: &str) -> PyResult<()> {
        self.core.subscribe_pnl(req_id);
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        let acct = if account.is_empty() { self.account() } else { account.to_string() };
        let _ = model_code;
        // Answered on the error callback and returned normally, as a request
        // made before connecting already is. Raising instead is a path a
        // caller written against the reference client does not take, so the
        // two clients answered the same failure differently.
        if let Err(why) =
            Self::send_control(py, &tx, ControlCommand::SubscribePnl { req_id, account: acct })
        {
            return self.report_refusal(py, req_id, Refusal::not_connected(why.to_string()));
        }
        Ok(())
    }

    /// Cancel P&L subscription.
    fn cancel_pnl(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        self.core.unsubscribe_pnl(req_id);
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        // A failed send is reported. Discarded, the subscription stays up and
        // the caller is told the cancel succeeded.
        if let Err(why) = Self::send_control(py, &tx, ControlCommand::CancelPnl { req_id }) {
            return self.report_refusal(py, req_id, Refusal::not_connected(why.to_string()));
        }
        Ok(())
    }

    /// Request P&L for a single position.
    ///
    /// `account` and `model_code` are taken and not applied. One session holds
    /// one account here, and the venue states its figures for that account
    /// without being asked which, so there is no second account or model
    /// portfolio to name.
    #[pyo3(signature = (req_id, account, model_code, con_id))]
    fn req_pnl_single(&self, req_id: i64, account: &str, model_code: &str, con_id: i64) -> PyResult<()> {
        self.core.subscribe_pnl_single(req_id, con_id);
        let _ = (account, model_code);
        Ok(())
    }

    /// Cancel single-position P&L subscription.
    fn cancel_pnl_single(&self, req_id: i64) -> PyResult<()> {
        self.core.unsubscribe_pnl_single(req_id);
        Ok(())
    }

    /// Request account summary.
    ///
    /// `group_name` is taken and not applied. One session holds one account
    /// here, and the venue states its figures for that account without being
    /// asked which, so there is no second account or model portfolio to name.
    #[pyo3(signature = (req_id, group_name, tags))]
    fn req_account_summary(&self, req_id: i64, group_name: &str, tags: &str) -> PyResult<()> {
        self.core.subscribe_account_summary(req_id, tags);
        let _ = group_name;
        Ok(())
    }

    /// Cancel account summary.
    fn cancel_account_summary(&self, req_id: i64) -> PyResult<()> {
        self.core.unsubscribe_account_summary(req_id);
        Ok(())
    }

    /// Request all positions.
    ///
    /// Before a session exists this is reported on the error callback and the
    /// call returns, as every other request made before connecting is. A
    /// program written against the reference client has no exception handling
    /// around a request, because that client does not raise there.
    fn req_positions(&self, py: Python<'_>) -> PyResult<()> {
        let Some(_connected) = self.tx_or_report(-1) else { return Ok(()) };
        let shared = self.shared_state()?;
        // Wait for CCP init burst to complete (up to 10s).
        for _ in 0..1000 {
            if shared.portfolio.account_download_complete() { break; }
            py.detach(|| std::thread::sleep(std::time::Duration::from_millis(10)));
        }
        // A position feed names a holding by id and leaves the rest to the
        // definition, which the engine asks for as the feed is read. That
        // answer lands a moment after the account download is called complete,
        // so reading straight through hands back a holding with no symbol,
        // security type or currency on it — one a caller can neither price nor
        // close — and nothing reads it again later.
        //
        // Bounded, and only while something is still unnamed: a definition the
        // venue never sends must cost a caller a wait, not a hang.
        // The set is read inside the wait and delivered as read. Waiting on
        // one set and delivering another hands back a holding that arrives
        // between the two, which no lookup has named.
        let waited_from = std::time::Instant::now();
        let mut positions = shared.portfolio.position_infos();
        loop {
            let unnamed = positions.iter().any(|pi| {
                pi.symbol.is_empty() && self.core.get_contract(pi.con_id, &shared).is_none()
            });
            if !unnamed || waited_from.elapsed() > std::time::Duration::from_secs(2) {
                break;
            }
            py.detach(|| std::thread::sleep(std::time::Duration::from_millis(20)));
            positions = shared.portfolio.position_infos();
        }
        for pi in &positions {
            let c_py = Py::new(py, self.position_contract(pi, &shared))?.into_any();
            let avg_cost = pi.avg_cost as f64 / PRICE_SCALE_F;
            self.callback(py, "position", (self.account().as_str(), &c_py, pi.position, avg_cost))?;
        }
        self.callback(py, "position_end", ())?;
        // Reported from here, on the next holding to move. What was already
        // recorded is left standing rather than dropped: the record is kept by
        // contract and states what the holding is now, so at worst the caller
        // is told once more what the answer above already said — and dropping
        // it would lose a holding that moved while the answer was assembled.
        self.positions_requested.store(true, Ordering::Release);
        Ok(())
    }

    /// Cancel positions.
    // nothing to withdraw: the venue pushes what the account holds when the
    // session opens and keeps it current. Nothing was subscribed, so nothing
    // stops, and reporting an error for withdrawing a subscription that was
    // never made would be wrong.
    fn cancel_positions(&self) -> PyResult<()> {
        self.positions_requested.store(false, Ordering::Release);
        Ok(())
    }

    /// Request account updates.
    ///
    /// `acct_code` is taken and not applied. One session holds one account
    /// here, and the venue states its figures for that account without being
    /// asked which, so there is no second account or model portfolio to name.
    ///
    /// Subscribing also asks the venue to state the figures now. It restates
    /// them on its own schedule otherwise, which is unhurried: a session that
    /// has just opened waits tens of seconds for its first set, and a caller
    /// that subscribed and then read the account got nothing.
    #[pyo3(signature = (subscribe, _acct_code=""))]
    fn req_account_updates(&self, py: Python<'_>, subscribe: bool, _acct_code: &str) -> PyResult<()> {
        self.core.subscribe_account_updates(subscribe);
        if subscribe {
            let Some(tx) = self.tx_or_report(-1) else { return Ok(()) };
            let account = self.account();
            if let Err(why) = Self::send_control(py, &tx, ControlCommand::RefreshAccount { account })
            {
                return self.report_refusal(py, -1, Refusal::not_connected(why.to_string()));
            }
        }
        Ok(())
    }

    /// Request managed accounts list. Answered with every account this login
    /// holds, comma separated, matching the reference client.
    ///
    /// Before a session exists there are no accounts to name, and an empty
    /// list reads as a login holding none rather than as a question asked too
    /// early.
    fn req_managed_accts(&self, py: Python<'_>) -> PyResult<()> {
        let Some(_connected) = self.tx_or_report(-1) else { return Ok(()) };
        self.callback(py, "managed_accounts", (self.accounts_csv().as_str(),))?;
        Ok(())
    }

    /// Request account updates for multiple accounts/models.
    ///
    /// `ledger_and_nlv` is taken and not applied. The account figures arrive as
    /// the venue states them, and it states the ledger and the net liquidation
    /// among them without being asked.
    #[pyo3(signature = (req_id, account, model_code, ledger_and_nlv=false))]
    fn req_account_updates_multi(
        &self, py: Python<'_>, req_id: i64, account: &str, model_code: &str, ledger_and_nlv: bool,
    ) -> PyResult<()> {
        // Reported and returned, as `req_positions` above and every other
        // request before connecting. Raising made this one request out of the
        // set the caller had to guard.
        let Some(_connected) = self.tx_or_report(req_id) else { return Ok(()) };
        let shared = self.shared_state()?;
        let _ = ledger_and_nlv;
        for _ in 0..500 {
            if shared.portfolio.account_data_received() { break; }
            py.detach(|| std::thread::sleep(std::time::Duration::from_millis(10)));
        }
        let acct_default = self.account();
        let acct_name = if !account.is_empty() { account } else { acct_default.as_str() };
        // Every figure the venue stated, in the currency it stated it in.
        // Rebuilding from this client's typed copy reports an account held in
        // any other currency as dollars, and drops every figure outside the
        // handful that copy carries.
        for (key, value, currency) in shared.portfolio.stated_account_values() {
            self.callback(py, "account_update_multi",
                (req_id, acct_name, model_code, key.as_str(), value.as_str(), currency.as_str()))?;
        }
        self.callback(py, "account_update_multi_end", (req_id,))?;
        Ok(())
    }

    /// Cancel multi-account updates.
    // nothing to withdraw: account values arrive with the session rather than
    // by subscription.
    fn cancel_account_updates_multi(&self, req_id: i64) -> PyResult<()> {
        let _ = req_id;
        Ok(())
    }

    /// Request positions across multiple accounts/models.
    #[pyo3(signature = (req_id, account, model_code))]
    fn req_positions_multi(&self, py: Python<'_>, req_id: i64, account: &str, model_code: &str) -> PyResult<()> {
        // As above.
        let Some(_connected) = self.tx_or_report(req_id) else { return Ok(()) };
        let shared = self.shared_state()?;
        for _ in 0..500 {
            if shared.portfolio.account_data_received() { break; }
            py.detach(|| std::thread::sleep(std::time::Duration::from_millis(10)));
        }
        let positions = shared.portfolio.position_infos();
        for pi in &positions {
            let c_py = Py::new(py, self.position_contract(pi, &shared))?.into_any();
            let avg_cost = pi.avg_cost as f64 / PRICE_SCALE_F;
            self.callback(py, "position_multi",
                (req_id, account, model_code, &c_py, pi.position, avg_cost))?;
        }
        self.callback(py, "position_multi_end", (req_id,))?;
        // Reported from here, on the next holding to move. The same live feed
        // as `position`, asked for under this request id.
        self.positions_multi_requested.lock().unwrap().insert(req_id);
        Ok(())
    }

    /// Cancel multi-account positions.
    ///
    // nothing to withdraw: the venue pushes what the account holds and keeps
    // it current whether or not anyone is listening, as for
    // `cancel_positions`. What stops is the reporting — a holding that moves
    // after this is no longer delivered on `position_multi` for this request.
    fn cancel_positions_multi(&self, req_id: i64) -> PyResult<()> {
        self.positions_multi_requested.lock().unwrap().remove(&req_id);
        Ok(())
    }

    /// Read account state snapshot. Returns a dict with all account values.
    fn account_snapshot(&self) -> PyResult<Option<Py<PyAny>>> {
        let shared = match self.shared.lock().unwrap().clone() {
            Some(s) => s,
            None => return Ok(None),
        };
        let acct = shared.portfolio.account();
        Python::attach(|py| {
            let ps = PRICE_SCALE_F;
            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("net_liquidation", acct.net_liquidation as f64 / ps)?;
            dict.set_item("buying_power", acct.buying_power as f64 / ps)?;
            dict.set_item("total_cash_value", acct.total_cash_value as f64 / ps)?;
            dict.set_item("gross_position_value", acct.gross_position_value as f64 / ps)?;
            dict.set_item("unrealized_pnl", acct.unrealized_pnl as f64 / ps)?;
            dict.set_item("realized_pnl", acct.realized_pnl as f64 / ps)?;
            dict.set_item("daily_pnl", acct.daily_pnl as f64 / ps)?;
            dict.set_item("init_margin_req", acct.init_margin_req as f64 / ps)?;
            dict.set_item("maint_margin_req", acct.maint_margin_req as f64 / ps)?;
            dict.set_item("available_funds", acct.available_funds as f64 / ps)?;
            dict.set_item("excess_liquidity", acct.excess_liquidity as f64 / ps)?;
            dict.set_item("settled_cash", acct.settled_cash as f64 / ps)?;
            dict.set_item("equity_with_loan", acct.equity_with_loan as f64 / ps)?;
            dict.set_item("cushion", acct.cushion as f64 / ps)?;
            dict.set_item("leverage", acct.leverage as f64 / ps)?;
            dict.set_item("sma", acct.sma as f64 / ps)?;
            dict.set_item("day_trades_remaining", acct.day_trades_remaining)?;
            Ok(Some(dict.into_any().unbind()))
        })
    }
}
