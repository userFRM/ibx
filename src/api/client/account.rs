//! Account-related methods: positions, PnL, account summary/updates.

use std::sync::atomic::Ordering;
use crate::types::model::PRICE_SCALE_F;
use crate::api::wrapper::Wrapper;
use crate::error_codes::Refusal;
use super::dispatch::NO_REQUEST;
use crate::types::*;

use super::{Contract, EClient};

impl EClient {
    // ── Positions ──

    /// The contract a holding is in, named as fully as this client can.
    ///
    /// Preferred from the definition cache, which carries the exchange,
    /// local symbol and trading class; the feed's own fields answer while the
    /// cache is cold. Shared with the real-time path so a holding is named
    /// the same way whether it is read at the request or as it moves.
    pub(crate) fn position_contract(&self, pi: &crate::types::PositionInfo) -> Contract {
        self.core.get_contract(pi.con_id, &self.shared).unwrap_or_else(|| Contract {
            con_id: pi.con_id,
            symbol: pi.symbol.clone(),
            sec_type: pi.sec_type.clone(),
            currency: pi.currency.clone(),
            multiplier: pi.multiplier.clone(),
            ..Default::default()
        })
    }

    /// Request positions. Matches `reqPositions` in C++.
    ///
    /// Waits for the account data the venue pushes as a session opens, then
    /// delivers what it holds and calls `position_end`. The wait is bounded, and
    /// an account that says nothing within it delivers nothing — which reads the
    /// same as an account holding nothing. Said in the log rather than left to
    /// be inferred, because the two are not the same answer.
    pub fn req_positions(&self, wrapper: &mut impl Wrapper) {
        // Waits for the batch-end signal, not for the first holding: an account
        // with several would otherwise answer with whichever arrived first. An
        // account holding nothing is complete when the batch ends, so this does
        // not wait on rows that are not coming.
        for _ in 0..1000 {
            if self.shared.portfolio.account_download_complete() { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !self.shared.portfolio.account_download_complete() {
            // Reported to the caller as well as the log. A caller reading
            // holdings has no other way to tell a truncated answer from a
            // complete one.
            let why = "the account had not finished stating its holdings within the wait, \
                       so what follows is what this session already held rather than what \
                       the account holds";
            log::warn!("{why}");
            wrapper.error(NO_REQUEST, Refusal::VALIDATION as i64, why, "");
        }
        // A holding arrives as a contract id and a quantity, and its definition
        // is fetched separately. Delivering before that lands names no
        // instrument at all, so give it a moment to arrive rather than handing
        // back a position in a contract the caller cannot identify.
        // The set is read inside the wait and delivered as read. Waiting on
        // one set and delivering another hands back a holding that arrives
        // between the two, which no lookup has named.
        let mut positions = self.shared.portfolio.position_infos();
        for _ in 0..150 {
            let unnamed = positions.iter().any(|pi| {
                pi.position != 0.0
                    && pi.symbol.is_empty()
                    && self.core.get_contract(pi.con_id, &self.shared).is_none()
            });
            if !unnamed { break; }
            std::thread::sleep(std::time::Duration::from_millis(20));
            positions = self.shared.portfolio.position_infos();
        }
        for pi in &positions {
            let c = self.position_contract(pi);
            let avg_cost = pi.avg_cost as f64 / PRICE_SCALE_F;
            wrapper.position(&self.account_id, &c, pi.position, avg_cost);
        }
        // Reported from here, on the next holding to move. What was already
        // recorded is left standing rather than dropped: the record is kept by
        // contract and states what the holding is now, so at worst the caller
        // is told once more what the answer above already said — and dropping
        // it would lose a holding that moved while the answer was assembled.
        self.positions_requested.store(true, Ordering::Release);
        wrapper.position_end();
    }

    // ── PnL ──

    /// Subscribe to account PnL updates. Matches `reqPnL` in C++.
    ///
    /// The venue is asked, under `account` or under the one this session opened
    /// with where none is named. What comes back is what each holding was worth
    /// at midnight and what it has realised since, and the figures reported on
    /// `pnl` are worked out from those against the prices the session is being
    /// told. Without the subscription none of that arrives, and the figures
    /// reduce to the unrealised part with nothing realised on any position.
    ///
    /// `model_code` is taken and not applied: there is no model portfolio to
    /// name here.
    pub fn req_pnl(&self, req_id: i64, account: &str, _model_code: &str) {
        self.core.subscribe_pnl(req_id);
        let account = if account.is_empty() { self.account_id.clone() } else { account.to_string() };
        if let Err(why) = self.send(ControlCommand::SubscribePnl { req_id, account }) {
            self.report_reason(req_id, &why);
        }
    }

    /// Cancel PnL subscription. Matches `cancelPnL` in C++.
    ///
    /// Nothing withdraws one on this wire, and the engine says so: what stops
    /// is the reporting, not the venue.
    pub fn cancel_pnl(&self, req_id: i64) {
        self.core.unsubscribe_pnl(req_id);
        if let Err(why) = self.send(ControlCommand::CancelPnl { req_id }) {
            self.report_reason(req_id, &why);
        }
    }

    /// Subscribe to single-position PnL updates. Matches `reqPnLSingle` in C++.
    ///
    /// `account` and `model_code` are taken and not applied, as on
    /// [`req_pnl`](EClient::req_pnl): the figures are for the account this
    /// session opened under.
    pub fn req_pnl_single(&self, req_id: i64, _account: &str, _model_code: &str, con_id: i64) {
        self.core.subscribe_pnl_single(req_id, con_id);
    }

    /// Cancel single-position PnL subscription. Matches `cancelPnLSingle` in C++.
    pub fn cancel_pnl_single(&self, req_id: i64) {
        self.core.unsubscribe_pnl_single(req_id);
    }

    // ── Account Summary ──

    /// Request account summary. Matches `reqAccountSummary` in C++.
    ///
    /// `group` is taken and not applied. One session holds one account here,
    /// and the venue states its figures for that account without being asked
    /// which, so there is no second account or model portfolio to name.
    pub fn req_account_summary(&self, req_id: i64, _group: &str, tags: &str) {
        self.core.subscribe_account_summary(req_id, tags);
    }

    /// Cancel account summary. Matches `cancelAccountSummary` in C++.
    pub fn cancel_account_summary(&self, req_id: i64) {
        self.core.unsubscribe_account_summary(req_id);
    }

    // ── Account Updates ──

    /// Subscribe to account updates. Matches `reqAccountUpdates` in C++.
    ///
    /// `acct_code` is taken and not applied. One session holds one account
    /// here, and the venue states its figures for that account without being
    /// asked which, so there is no second account or model portfolio to name.
    ///
    /// Subscribing also asks the venue to state the figures now. It restates
    /// them on its own schedule otherwise, which is unhurried: a session that
    /// has just opened waits tens of seconds for its first set, and a caller
    /// that subscribed and then read the account got nothing.
    pub fn req_account_updates(&self, subscribe: bool, _acct_code: &str) {
        self.core.subscribe_account_updates(subscribe);
        if subscribe {
            let account = self.account_id.clone();
            if let Err(why) = self.send(ControlCommand::RefreshAccount { account }) {
                self.report_reason(-1, &why);
            }
        }
    }

    /// Cancel positions subscription. Matches `cancelPositions` in C++.
    ///
    /// Nothing is withdrawn from the venue: it pushes what the account holds
    /// as the session opens and keeps it current whether or not anyone is
    /// listening. What stops is the reporting — a holding that moves after
    /// this is no longer delivered on `position`.
    pub fn cancel_positions(&self) {
        self.positions_requested.store(false, Ordering::Release);
    }

    /// Request managed accounts. Matches `reqManagedAccts` in C++.
    ///
    /// Answered with every account this login holds, comma separated, which is
    /// the shape the reference client answers in. A login with one account is
    /// answered with that one account and no comma.
    pub fn req_managed_accts(&self, wrapper: &mut impl Wrapper) {
        wrapper.managed_accounts(&self.accounts.join(","));
    }

    /// Request account updates for multiple accounts/models. Matches
    /// `reqAccountUpdatesMulti` in C++.
    /// Account values for one account or model, answered on
    /// `account_update_multi`. The reference client answers this request on
    /// its own callbacks, not on the ones `req_account_updates` uses, and a
    /// caller written against it implements those and hears nothing otherwise.
    ///
    /// `ledger_and_nlv` is taken and not applied. The account figures arrive as
    /// the venue states them, and it states the ledger and the net liquidation
    /// among them without being asked.
    ///
    /// The figures are the ones the venue states for the account this session
    /// opened under, and they are labelled with that account. A login holding
    /// several is answered for that one; naming another here does not fetch
    /// the other's figures, and is said in the log rather than answered with
    /// this account's under the other's name.
    pub fn req_account_updates_multi(
        &self, req_id: i64, account: &str, model_code: &str, _ledger_and_nlv: bool,
        wrapper: &mut impl Wrapper,
    ) {
        if !account.is_empty() && account != self.account_id {
            let why = format!(
                "account {account} was named and the figures that follow are {}'s, which \
                 is the account this session opened under",
                self.account_id,
            );
            log::warn!("{why}");
            wrapper.error(req_id, Refusal::VALIDATION as i64, &why, "");
        }
        // As the venue stated them, in the currency it stated them in. Eight
        // of them were worked out here instead, rounded to two decimals and
        // labelled US dollars whatever the account is held in: an account in
        // another currency read as a dollar account, and every figure the
        // venue states beyond those eight was not reported at all.
        for (key, value, currency) in self.shared.portfolio.stated_account_values() {
            wrapper.account_update_multi(
                req_id, &self.account_id, model_code, &key, &value, &currency,
            );
        }
        wrapper.account_update_multi_end(req_id);
    }

    /// Cancel multi-account updates. Matches `cancelAccountUpdatesMulti` in C++.
    ///
    /// `_req_id` reaches nothing, because there is nothing to withdraw: the
    /// request it would name is answered from what this session already holds,
    /// before a caller has this to cancel it with.
    pub fn cancel_account_updates_multi(&self, _req_id: i64) {

    }

    /// Request positions for multiple accounts/models. Matches `reqPositionsMulti` in
    /// C++.
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
                let contract = self.position_contract(&pi);
                (contract, pi.position, pi.avg_cost as f64 / PRICE_SCALE_F)
            })
            .collect();
        if !account.is_empty() && account != self.account_id {
            let why = format!(
                "account {account} was named and the holdings that follow are {}'s, which \
                 is the account this session opened under",
                self.account_id,
            );
            log::warn!("{why}");
            wrapper.error(req_id, Refusal::VALIDATION as i64, &why, "");
        }
        // Labelled with the account they are on, not with the one that was
        // asked about: these are the holdings of the account this session
        // opened under, and echoing the caller's own put another account's
        // name on them.
        for (contract, position, avg_cost) in held {
            wrapper.position_multi(
                req_id, &self.account_id, model_code, &contract, position, avg_cost,
            );
        }
        wrapper.position_multi_end(req_id);
        // Reported from here, on the next holding to move. The same live feed
        // as `position`, asked for under this request id.
        self.positions_multi_requested.lock().unwrap().insert(req_id);
    }

    /// Cancel multi-account positions. Matches `cancelPositionsMulti` in C++.
    /// Stop watching holdings under this request.
    ///
    // nothing to withdraw: the venue keeps the account current whether or not
    // anyone is listening, as for `cancel_positions`. What stops is the
    // reporting — a holding that moves after this is no longer delivered on
    // `position_multi` for this request.
    pub fn cancel_positions_multi(&self, req_id: i64) {
        self.positions_multi_requested.lock().unwrap().remove(&req_id);
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
