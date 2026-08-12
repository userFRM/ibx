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
        // The mode the caller asked for on `req_market_data_type`, which names
        // the type once for every subscription that follows. `req_mkt_data_ex`
        // states it per request instead.
        let mode = self.core.subscription_mode();
        self.req_mkt_data_ex(req_id, contract, generic_tick_list, snapshot, regulatory_snapshot, mode)
    }

    /// Like [`req_mkt_data`](EClient::req_mkt_data), but encodes the market-data mode per-request via
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
    /// The frozen mode keeps thinly-traded names quoting after-hours, when the
    /// realtime feed is silent.
    ///
    /// A contract holds one subscription at a time (ibx#233), so this states
    /// the mode for that subscription rather than adding a parallel one — to
    /// compare modes on one contract, cancel between them. To set the mode for
    /// every subscription instead of naming it per request, call
    /// `req_market_data_type`.
    pub fn req_mkt_data_ex(
        &self, req_id: i64, contract: &Contract,
        generic_tick_list: &str, snapshot: bool, _regulatory_snapshot: bool,
        mode_9887: i32,
    ) -> Result<(), String> {
        self.core.register_mkt_data(
            &self.shared, &self.control_tx, req_id,
            contract.con_id, &contract.symbol, &contract.exchange, &contract.sec_type,
            &contract.currency,
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

    /// Subscribe to every trade or every quote change on a contract.
    ///
    /// This used to refuse outright, on the reasoning that the feed rode a
    /// service of its own which this client could not reach. That reasoning was
    /// wrong. The feed rides the historical farm this client already reaches —
    /// the counterpart registers it there under the name "TickByTick" beside
    /// the five-second bars that already stream — and no list of services is
    /// involved. The account is entitled; a missing entitlement arrives as the
    /// venue's own refusal, not as silence.
    ///
    /// What was actually wrong was reading what came back. The subscription was
    /// always right, which is why the venue acknowledged it and assigned a
    /// ticker id, and then nothing could be made of the frames that followed.
    pub fn req_tick_by_tick_data(
        &self, req_id: i64, contract: &Contract, tick_type: &str,
        number_of_ticks: i32, ignore_size: bool,
    ) -> Result<(), String> {
        let _ = (number_of_ticks, ignore_size);
        let kind = TbtType::named(tick_type)?;

        // A stream is asked for by the venue's own id for the contract. Sent
        // with none, the venue answers "Unknown contract" against a query this
        // client had not told anyone about, and the caller waited on a stream
        // that was refused before it began.
        let named;
        let contract = if contract.con_id == 0 && !contract.symbol.is_empty() {
            named = self.qualify_contract(contract)?;
            &named
        } else {
            contract
        };

        self.core
            .register_tbt(
                &self.shared,
                &self.control_tx,
                req_id,
                contract.con_id,
                &contract.symbol,
                &contract.sec_type,
                &contract.exchange,
                kind,
            )
            .map(|_| ())
    }


    /// Cancel tick-by-tick data. Matches `cancelTickByTickData` in C++.
    pub fn cancel_tick_by_tick_data(&self, req_id: i64) -> Result<(), String> {
        // Only what this request took out. Removing the contract's quote
        // mapping here took the quotes away from whoever was watching them.
        if let Some(instrument) = self.core.tbt_to_instrument.lock().unwrap().remove(&req_id) {
            self.send(ControlCommand::UnsubscribeTbt { req_id, instrument })?;
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
            symbol: contract.symbol.clone(),
            exchange,
            sec_type,
            currency: contract.currency.clone(),
            filters: contract.lookup_filters(),
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
            filters: contract.lookup_filters(),
            currency: contract.currency.clone(),
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
