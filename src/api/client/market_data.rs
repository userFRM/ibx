//! Market data request/cancel methods and quote accessors.

use crate::types::*;

use super::{wire_req_id, Contract, EClient};

impl EClient {
    // ── Market Data ──

    /// Subscribe to market data. Matches `reqMktData` in C++.
    /// When `snapshot` is true, delivers the first available quote then calls
    /// `tick_snapshot_end` and auto-cancels the subscription.
    ///
    /// `generic_tick_list` is NOT transmitted to the gateway, with one
    /// exception: "292" additionally subscribes per-contract news. Other
    /// generic tick types (RTVolume and friends) have no emission path, and
    /// `tick_generic` never fires (ibx#234). Delayed data cannot be
    /// requested either — see `req_market_data_type`.
    pub fn req_mkt_data(
        &self, req_id: i64, contract: &Contract,
        generic_tick_list: &str, snapshot: bool, regulatory_snapshot: bool,
    ) -> Result<(), String> {
        self.req_mkt_data_ex(req_id, contract, generic_tick_list, snapshot, regulatory_snapshot, 0)
    }

    /// Like [`req_mkt_data`], but encodes the market-data mode per-request via
    /// FIX field 9887, allowing parallel realtime + frozen subscriptions for
    /// the same contract:
    ///
    /// | `mode_9887` | mode             | wire shape |
    /// |-------------|------------------|---|
    /// | `0`         | REALTIME         | `264=442` (BID_ASK) + `264=443` (LAST), no 9887 |
    /// | `1`         | DELAYED          | `264=1` (TOP) + `9887=1` |
    /// | `2`         | FROZEN           | `264=1` (TOP) + `9887=2` |
    /// | `3`         | DELAYED_FROZEN   | `264=1` (TOP) + `9887=3` |
    ///
    /// The frozen sub keeps thinly-traded names streaming after-hours when the
    /// realtime feed is silent. Issue 3-4 parallel calls per contract with
    /// different modes and pick whichever feed has data.
    pub fn req_mkt_data_ex(
        &self, req_id: i64, contract: &Contract,
        generic_tick_list: &str, snapshot: bool, _regulatory_snapshot: bool,
        mode_9887: i32,
    ) -> Result<(), String> {
        self.core.register_mkt_data(
            &self.shared, &self.control_tx, req_id,
            contract.con_id, &contract.symbol, &contract.exchange, &contract.sec_type,
            &contract.last_trade_date_or_contract_month, contract.strike, &contract.right, &contract.multiplier,
            snapshot, generic_tick_list, mode_9887,
        )?;
        Ok(())
    }

    /// Cancel market data. Matches `cancelMktData` in C++.
    pub fn cancel_mkt_data(&self, req_id: i64) -> Result<(), String> {
        let (instrument, needs_news_unsub) = self.core.unregister_mkt_data(req_id);
        if let Some(instrument) = instrument {
            self.send(ControlCommand::Unsubscribe { instrument })?;
            if needs_news_unsub {
                let _ = self.send(ControlCommand::UnsubscribeNews { instrument });
            }
        }
        Ok(())
    }

    /// Subscribe to tick-by-tick data. Matches `reqTickByTickData` in C++.
    ///
    /// **Not implemented.** Tick-by-tick is carried by a service of its own,
    /// which this client does not yet speak: the historical service serves
    /// chart, fundamentals, news and scanner requests, and nothing else. A
    /// subscription sent there is accepted and assigned a ticker id, and then
    /// no tick ever follows — verified against a paper account, where three
    /// subscriptions on two contracts were all acknowledged and all silent
    /// (ibx#404).
    ///
    /// Refused here rather than accepted, because a subscription that is taken
    /// and never delivers is worse than one that says so.
    pub fn req_tick_by_tick_data(
        &self, _req_id: i64, _contract: &Contract, _tick_type: &str,
        _number_of_ticks: i32, _ignore_size: bool,
    ) -> Result<(), String> {
        Err("tick-by-tick data is not implemented: it is carried by a service \
             this client does not speak yet, and a subscription sent to the \
             historical service is acknowledged but never delivers (ibx#404)".to_string())
    }


    /// Cancel tick-by-tick data. Matches `cancelTickByTickData` in C++.
    pub fn cancel_tick_by_tick_data(&self, req_id: i64) -> Result<(), String> {
        if let Some(instrument) = self.core.req_to_instrument.lock().unwrap().remove(&req_id) {
            self.core.instrument_to_req.lock().unwrap().remove(&instrument);
            self.core.forget_instrument(instrument);
            self.send(ControlCommand::UnsubscribeTbt { instrument })?;
        }
        Ok(())
    }

    // ── Market Depth ──

    /// Subscribe to market depth (L2 order book). Matches `reqMktDepth` in C++.
    pub fn req_mkt_depth(
        &self, req_id: i64, contract: &Contract,
        num_rows: i32, is_smart_depth: bool,
    ) -> Result<(), String> {
        let exchange = if contract.exchange.is_empty() { "SMART".to_string() } else { contract.exchange.clone() };
        let sec_type = if contract.sec_type.is_empty() { "STK".to_string() } else { contract.sec_type.clone() };
        self.send(ControlCommand::SubscribeDepth {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id,
            exchange,
            sec_type,
            num_rows,
            is_smart_depth,
        })
    }

    /// Cancel market depth. Matches `cancelMktDepth` in C++.
    pub fn cancel_mkt_depth(&self, req_id: i64) -> Result<(), String> {
        self.send(ControlCommand::UnsubscribeDepth { req_id: wire_req_id(req_id)? })
    }

    // ── Real-Time Bars ──

    /// Subscribe to real-time 5-second bars. Matches `reqRealTimeBars` in C++.
    pub fn req_real_time_bars(
        &self, req_id: i64, contract: &Contract,
        _bar_size: i32, what_to_show: &str, use_rth: bool,
    ) -> Result<(), String> {
        self.send(ControlCommand::SubscribeRealTimeBar {
            req_id: wire_req_id(req_id)?,
            con_id: contract.con_id,
            symbol: contract.symbol.clone(),
            sec_type: contract.sec_type.clone(),
            exchange: contract.exchange.clone(),
            what_to_show: what_to_show.into(),
            use_rth,
        })
    }

    /// Cancel real-time bars. Matches `cancelRealTimeBars` in C++.
    pub fn cancel_real_time_bars(&self, req_id: i64) -> Result<(), String> {
        self.send(ControlCommand::CancelRealTimeBar { req_id: wire_req_id(req_id)? })
    }

    /// Set market data type preference (1=live, 2=frozen, 3=delayed, 4=delayed-frozen).
    /// Request an auth-connection round-trip time sample (ibx#158): sends a
    /// lightweight liveness probe with no side effects on subscriptions,
    /// contract caches, or pacing budgets. The result lands asynchronously —
    /// poll `last_rtt()` after a moment. No-op while a probe is already in
    /// flight or the connection is down.
    pub fn req_ping(&self) -> Result<(), String> {
        self.send(ControlCommand::Ping)
    }

    /// Last measured auth-connection round-trip time, if any (ibx#158).
    /// A gauge, not a benchmark: the sample is the interval from a probe to
    /// the first inbound traffic that followed it, which on an active feed
    /// can undercount by racing data already in flight. Also sampled
    /// automatically whenever liveness sends its own probe.
    pub fn last_rtt(&self) -> Option<std::time::Duration> {
        self.shared.last_ccp_rtt()
    }

    /// NOT supported end to end (ibx#234): the requested type is stored
    /// locally but never sent to the gateway, so subscriptions always
    /// deliver realtime data and delayed tick variants never arrive.
    /// Requesting a non-realtime type logs a warning, and the
    /// `market_data_type` callback reports the DELIVERED type (realtime)
    /// rather than echoing the request.
    pub fn req_market_data_type(&self, market_data_type: i32) {
        self.core.set_market_data_type(market_data_type);
    }

    /// Set news provider codes for per-contract news ticks.
    pub fn set_news_providers(&self, providers: &str) {
        self.core.set_news_providers(providers);
    }

    // ── Escape Hatch ──

    /// Zero-copy SeqLock quote read. Maps reqId → InstrumentId → SeqLock.
    /// Returns `None` if the reqId is not mapped to a subscription.
    #[inline]
    pub fn quote(&self, req_id: i64) -> Option<Quote> {
        let map = self.core.req_to_instrument.lock().unwrap();
        map.get(&req_id).map(|&iid| self.shared.market.quote(iid))
    }

    /// Direct SeqLock read by InstrumentId (for callers who track IDs themselves).
    /// Returns None for an out-of-range id — this used to panic (ibx#234).
    #[inline]
    pub fn quote_by_instrument(&self, instrument: InstrumentId) -> Option<Quote> {
        self.shared.market.try_quote(instrument)
    }
}
