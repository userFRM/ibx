//! Market data request/cancel methods.

use pyo3::prelude::*;
use crate::error_codes::Refusal;

use crate::types::*;
use super::{wire_req_id, EClient};
use super::super::contract::Contract;

#[pymethods]
impl EClient {
    /// Set news provider codes for per-contract news ticks (e.g. "BRFG*BRFUPDN").
    #[pyo3(signature = (providers))]
    fn set_news_providers(&self, providers: &str) {
        self.core.set_news_providers(providers);
    }

    /// Request market data for a contract.
    ///
    /// `mkt_data_options` is taken and not applied. This protocol's request
    /// carries no free-form option list, so what a caller puts in one cannot be
    /// sent. The reference client's own list is empty on every ordinary call.
    #[pyo3(signature = (req_id, contract, generic_tick_list="", snapshot=false, regulatory_snapshot=false, mkt_data_options=Vec::new()))]
    fn req_mkt_data(
        &self,
        py: Python<'_>,
        req_id: i64,
        contract: &Contract,
        generic_tick_list: &str,
        snapshot: bool,
        regulatory_snapshot: bool,
        mkt_data_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = mkt_data_options;
        self.req_mkt_data_ex(py, req_id, contract, generic_tick_list, snapshot, regulatory_snapshot, 0)
    }

    /// Like `req_mkt_data`, but encodes the market-data mode per request
    /// (0=realtime, 1=delayed, 2=frozen, 3=delayed-frozen), so several
    /// subscriptions on the same contract can run in parallel and the caller
    /// picks whichever feed has data. The frozen one keeps thinly-traded names
    /// streaming after hours when the realtime feed is silent.
    ///
    /// `regulatory_snapshot` is taken and not applied. A regulatory snapshot is
    /// a separate, chargeable request this protocol does not carry, so asking
    /// for one here would be answered with an ordinary subscription and a
    /// charge nobody agreed to.
    #[pyo3(signature = (req_id, contract, generic_tick_list="", snapshot=false, regulatory_snapshot=false, mode_9887=0))]
    fn req_mkt_data_ex(
        &self,
        py: Python<'_>,
        req_id: i64,
        contract: &Contract,
        generic_tick_list: &str,
        snapshot: bool,
        regulatory_snapshot: bool,
        mode_9887: i32,
    ) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };

        // A contract's news is asked for by the venue's id for the contract,
        // and the caller may have stated a description instead. Resolved only
        // when news is what was asked for: a quote on a description is asked
        // for by description and the venue names it itself.
        let wants_news = generic_tick_list.split(',').any(|t| t.trim().ends_with("292"));
        let named;
        let contract = if wants_news && contract.con_id == 0 && !contract.symbol.is_empty() {
            match self.qualify_contract_stated(py, contract) {
                Ok(found) => { named = found; &named }
                // Under the code that caused it. Called a missing definition
                // whatever went wrong, a session that ended mid-lookup reads
                // as a contract that does not exist, and a caller that
                // branches on the code retries the description for ever.
                Err(why) => return self.report_refusal(py, req_id, why),
            }
        } else {
            contract
        };

        let shared = self.shared_state()?;

        // The engine can take up to REGISTRATION_TIMEOUT to reply; release
        // the GIL for the round trip so a slow reply stalls this call, not
        // every Python thread. Own the contract fields first —
        // `contract` itself must not cross the detach boundary.
        let con_id = contract.con_id;
        let symbol = contract.symbol.clone();
        let exchange = contract.exchange.clone();
        let sec_type = contract.sec_type.clone();
        let currency = contract.currency.clone();
        let last_trade_date = contract.last_trade_date_or_contract_month.clone();
        let strike = contract.strike;
        let right = contract.right.clone();
        let multiplier = contract.multiplier.clone();
        let generic_tick_list = generic_tick_list.to_string();
        if let Err(why) = py.detach(|| self.core.register_mkt_data(
            &shared, &tx, req_id,
            con_id, &symbol, &exchange, &sec_type, &currency,
            &last_trade_date, strike, &right, &multiplier,
            snapshot, &generic_tick_list, mode_9887,
        )) {
            return self.report_refusal(py, req_id, why);
        }
        self.core.cache_contract(contract.con_id, crate::api::types::Contract {
            con_id: contract.con_id,
            symbol: contract.symbol.clone(),
            sec_type: contract.sec_type.clone(),
            exchange: contract.exchange.clone(),
            currency: contract.currency.clone(),
            last_trade_date_or_contract_month: contract.last_trade_date_or_contract_month.clone(),
            strike: contract.strike,
            right: contract.right.clone(),
            multiplier: contract.multiplier.clone(),
            ..Default::default()
        });

        let _ = regulatory_snapshot;

        Ok(())
    }

    /// Cancel market data.
    pub fn cancel_mkt_data(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let (instrument, needs_news_unsub) = self.core.unregister_mkt_data(req_id);
        if let Some(instrument) = instrument {
            let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
            Self::send_control(py, &tx, ControlCommand::Unsubscribe { instrument })?;
            if needs_news_unsub {
                let _ = Self::send_control(py, &tx, ControlCommand::UnsubscribeNews { instrument });
            }
        }
        Ok(())
    }

    /// Request tick-by-tick data.
    ///
    /// `number_of_ticks` and `ignore_size` are taken and not applied. The
    /// subscription states the contract and the kind of stream and nothing
    /// else: there is no field for a prelude of past ticks, and none for
    /// suppressing size-only changes. The Rust surface refuses them rather than
    /// dropping them; here they are answered with the stream the venue gives,
    /// which is what their defaults describe.
    #[pyo3(signature = (req_id, contract, tick_type, number_of_ticks=0, ignore_size=false))]
    fn req_tick_by_tick_data(
        &self,
        py: Python<'_>,
        req_id: i64,
        contract: &Contract,
        tick_type: &str,
        number_of_ticks: i32,
        ignore_size: bool,
    ) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };

        let tbt_type = match TbtType::named(tick_type) {
            Ok(named) => named,
            // A tick type this client does not carry is a request it will not
            // send, which is what validation means.
            Err(why) => return self.report_refusal(py, req_id, Refusal::validation(why)),
        };

        // A stream is asked for by the venue's own id for the contract. Sent
        // with none, the venue answers "Unknown contract" against a query this
        // client had not told anyone about, and the caller waited on a stream
        // that was refused before it began.
        let named;
        let contract = if contract.con_id == 0 && !contract.symbol.is_empty() {
            match self.qualify_contract_stated(py, contract) {
                Ok(found) => { named = found; &named }
                // Under the code that caused it. Called a missing definition
                // whatever went wrong, a session that ended mid-lookup reads
                // as a contract that does not exist, and a caller that
                // branches on the code retries the description for ever.
                Err(why) => return self.report_refusal(py, req_id, why),
            }
        } else {
            contract
        };

        let shared = self.shared_state()?;
        Self::send_control(py, &tx, ControlCommand::RegisterInstrument {
            contract: ContractRef { con_id: contract.con_id, symbol: contract.symbol.clone(), sec_type: contract.sec_type.clone(), exchange: contract.exchange.clone(), ..Default::default() },
            identity: String::new(),
            reply_tx: None,
        })?;
        // Same registration-wait hazard as req_mkt_data: release the GIL for
        // the reply round trip.
        let con_id = contract.con_id;
        let symbol = contract.symbol.clone();
        let (sec_type, exchange) = (contract.sec_type.clone(), contract.exchange.clone());
        if let Err(why) = py.detach(|| self.core.register_tbt(
            &shared, &tx, req_id, con_id, &symbol, &sec_type, &exchange, tbt_type,
        )) {
            return self.report_refusal(py, req_id, why);
        }

        let _ = (number_of_ticks, ignore_size);
        Ok(())
    }

    /// Cancel tick-by-tick data.
    fn cancel_tick_by_tick_data(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        // Only what this request took out. Removing the contract's quote
        // mapping here took the quotes away from whoever was watching them.
        // Removed before the send, not across it: the send is bounded and runs
        // detached from Python, so a guard spanning it blocks another thread
        // cancelling a different subscription.
        let instrument = self.core.tbt_to_instrument.lock().unwrap().remove(&req_id);
        if let Some(instrument) = instrument {
            let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
            Self::send_control(py, &tx, ControlCommand::UnsubscribeTbt { req_id, instrument })?;
        }
        Ok(())
    }

    /// Request an auth-connection round-trip time sample: sends a
    /// lightweight liveness probe with no side effects on subscriptions,
    /// contract caches, or pacing budgets. Poll `last_rtt_ms()` after a
    /// moment for the result.
    fn req_ping(&self, py: Python<'_>) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(-1) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::Ping)?;
        Ok(())
    }

    /// Last measured auth-connection round-trip time in milliseconds, or
    /// None if never measured. A gauge, not a benchmark
    /// `req_ping`. Also sampled automatically by the engine's own liveness
    /// probes.
    fn last_rtt_ms(&self) -> PyResult<Option<f64>> {
        let shared = match self.shared.lock().unwrap().clone() {
            Some(s) => s,
            None => return Ok(None),
        };
        Ok(shared.last_ccp_rtt().map(|d| d.as_secs_f64() * 1_000.0))
    }

    /// NOT supported end to end: the requested type (1=live,
    /// 2=frozen, 3=delayed, 4=delayed-frozen) is stored locally but never
    /// sent to the gateway, so subscriptions always deliver realtime data
    /// and delayed tick variants never arrive. Requesting a non-realtime
    /// type logs a warning, and the `market_data_type` callback reports the
    /// DELIVERED type (realtime) rather than echoing the request.
    fn req_market_data_type(&self, market_data_type: i32) -> PyResult<()> {
        self.core.set_market_data_type(market_data_type);
        Ok(())
    }

    /// Request market depth (L2 order book).
    ///
    /// `mkt_depth_options` is taken and not applied. This protocol's request
    /// carries no free-form option list, so what a caller puts in one cannot be
    /// sent. The reference client's own list is empty on every ordinary call.
    #[pyo3(signature = (req_id, contract, num_rows=5, is_smart_depth=false, mkt_depth_options=Vec::new()))]
    fn req_mkt_depth(
        &self,
        py: Python<'_>,
        req_id: i64,
        contract: &Contract,
        num_rows: i32,
        is_smart_depth: bool,
        mkt_depth_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let _ = mkt_depth_options;
        let exchange = if contract.exchange.is_empty() { "SMART".to_string() } else { contract.exchange.clone() };
        let sec_type = if contract.sec_type.is_empty() { "STK".to_string() } else { contract.sec_type.clone() };
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::SubscribeDepth {
            contract: ContractRef { con_id: contract.con_id, symbol: contract.symbol.clone(), exchange, sec_type, currency: contract.currency.clone(), ..Default::default() },
            req_id: wire_req_id(req_id)?,
            num_rows,
            is_smart_depth,
            filters: contract.lookup_filters(),
        })?;
        Ok(())
    }

    /// Cancel market depth.
    ///
    /// `is_smart_depth` is taken and not applied. A book is withdrawn by the
    /// request that asked for it, and this client remembers which kind that
    /// was, so the caller restating it changes nothing.
    #[pyo3(signature = (req_id, is_smart_depth=false))]
    fn cancel_mkt_depth(&self, py: Python<'_>, req_id: i64, is_smart_depth: bool) -> PyResult<()> {
        let _ = is_smart_depth;
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::UnsubscribeDepth { req_id: wire_req_id(req_id)? })?;
        Ok(())
    }

    /// Request real-time 5-second bars.
    ///
    /// `bar_size` and `real_time_bars_options` are taken and not applied. The
    /// venue's real-time bar is five seconds and there is no field asking for
    /// another, and this protocol's request carries no free-form option list.
    #[pyo3(signature = (req_id, contract, bar_size=5, what_to_show="TRADES", use_rth=0, real_time_bars_options=Vec::new()))]
    fn req_real_time_bars(
        &self,
        py: Python<'_>,
        req_id: i64,
        contract: &Contract,
        bar_size: i32,
        what_to_show: &str,
        use_rth: i32,
        real_time_bars_options: Vec<Py<PyAny>>,
    ) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        let _ = (bar_size, real_time_bars_options);
        Self::send_control(py, &tx, ControlCommand::SubscribeRealTimeBar {
            contract: contract.into(),
            req_id: wire_req_id(req_id)?,
            what_to_show: what_to_show.to_string(),
            use_rth: use_rth != 0,
            filters: contract.lookup_filters(),
        })?;
        Ok(())
    }

    /// Cancel real-time bars.
    fn cancel_real_time_bars(&self, py: Python<'_>, req_id: i64) -> PyResult<()> {
        let Some(tx) = self.tx_or_report(req_id) else { return Ok(()) };
        Self::send_control(py, &tx, ControlCommand::CancelRealTimeBar { req_id: wire_req_id(req_id)? })?;
        Ok(())
    }

    // ── Quote Access ──

    /// Zero-copy SeqLock quote read by req_id.
    /// Returns a dict with bid, ask, last, bid_size, ask_size, last_size, volume,
    /// high, low, open, close, or None if the req_id is not mapped.
    fn quote(&self, req_id: i64) -> PyResult<Option<Py<PyAny>>> {
        let shared = self.shared_state()?;
        let map = self.core.req_to_instrument.lock().unwrap();
        let iid = match map.get(&req_id) {
            Some(&iid) => iid,
            None => return Ok(None),
        };
        drop(map);
        let q = shared.market.quote(iid);
        Python::attach(|py| {
            let ps = super::super::super::types::PRICE_SCALE_F;
            let qs = crate::types::QTY_SCALE as f64;
            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("bid", q.bid as f64 / ps)?;
            dict.set_item("ask", q.ask as f64 / ps)?;
            dict.set_item("last", q.last as f64 / ps)?;
            dict.set_item("bid_size", q.bid_size as f64 / qs)?;
            dict.set_item("ask_size", q.ask_size as f64 / qs)?;
            dict.set_item("last_size", q.last_size as f64 / qs)?;
            dict.set_item("volume", q.volume as f64 / qs)?;
            dict.set_item("high", q.high as f64 / ps)?;
            dict.set_item("low", q.low as f64 / ps)?;
            dict.set_item("open", q.open as f64 / ps)?;
            dict.set_item("close", q.close as f64 / ps)?;
            Ok(Some(dict.into_any().unbind()))
        })
    }

    /// Zero-copy SeqLock quote read by InstrumentId.
    /// Returns a dict with bid, ask, last, bid_size, ask_size, last_size, volume,
    /// high, low, open, close, or None if not connected.
    fn quote_by_instrument(&self, instrument: u32) -> PyResult<Option<Py<PyAny>>> {
        let shared = match self.shared.lock().unwrap().clone() {
            Some(s) => s,
            None => return Ok(None),
        };
        // Out-of-range id: None, not a cross-language panic.
        let Some(q) = shared.market.try_quote(instrument) else {
            return Ok(None);
        };
        Python::attach(|py| {
            let ps = super::super::super::types::PRICE_SCALE_F;
            let qs = crate::types::QTY_SCALE as f64;
            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("bid", q.bid as f64 / ps)?;
            dict.set_item("ask", q.ask as f64 / ps)?;
            dict.set_item("last", q.last as f64 / ps)?;
            dict.set_item("bid_size", q.bid_size as f64 / qs)?;
            dict.set_item("ask_size", q.ask_size as f64 / qs)?;
            dict.set_item("last_size", q.last_size as f64 / qs)?;
            dict.set_item("volume", q.volume as f64 / qs)?;
            dict.set_item("high", q.high as f64 / ps)?;
            dict.set_item("low", q.low as f64 / ps)?;
            dict.set_item("open", q.open as f64 / ps)?;
            dict.set_item("close", q.close as f64 / ps)?;
            Ok(Some(dict.into_any().unbind()))
        })
    }
}
