//! Market data request/cancel methods and quote accessors.

use crate::types::*;
use crate::error_codes::Refusal;

use super::{wire_req_id, Contract, EClient};

impl EClient {
    // ── Market Data ──

    /// Subscribe to market data. Matches `reqMktData` in C++.
    /// When `snapshot` is true, delivers the first available quote then calls
    /// `tick_snapshot_end` and auto-cancels the subscription.
    ///
    /// `generic_tick_list` is NOT transmitted to the gateway, with one
    /// exception: "292" additionally subscribes per-contract news. Other
    /// generic tick types (RTVolume and friends) are not requested — the venue
    /// asks for those under numbers of its own rather than the ones this list
    /// uses, and this client does not know the mapping. Naming one is warned
    /// about rather than quietly dropped.
    ///
    /// `tick_generic` does fire, for the halt the venue states on its own tick:
    /// tick 49, 0 while a contract is trading and 1 once it has stopped.
    ///
    /// Delayed and frozen data are requested, contrary to what this said: name
    /// the type on [`req_market_data_type`](EClient::req_market_data_type) and
    /// every subscription after it carries the mode, or state it per request
    /// with [`req_mkt_data_ex`](EClient::req_mkt_data_ex). The table there
    /// gives the wire shape of each.
    pub fn req_mkt_data(
        &self, req_id: i64, contract: &Contract,
        generic_tick_list: &str, snapshot: bool, regulatory_snapshot: bool,
    ) -> Result<(), Refusal> {
        // The mode the caller asked for on `req_market_data_type`, which names
        // the type once for every subscription that follows. `req_mkt_data_ex`
        // states it per request instead.
        let mode = self.core.subscription_mode();
        self.req_mkt_data_ex(req_id, contract, generic_tick_list, snapshot, regulatory_snapshot, mode)
    }

    /// Like [`req_mkt_data`](EClient::req_mkt_data), but encodes the market-data mode
    /// per-request via
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
    /// A contract holds one subscription at a time, so this states
    /// the mode for that subscription rather than adding a parallel one — to
    /// compare modes on one contract, cancel between them. To set the mode for
    /// every subscription instead of naming it per request, call
    /// `req_market_data_type`.
    ///
    /// `regulatory_snapshot` is refused rather than dropped. It names a
    /// separate, chargeable one-shot request, and this protocol carries no
    /// field for one: taken and ignored, a caller who asked for a single NBBO
    /// snapshot the venue charges for and stands behind reads an ordinary
    /// subscription instead, which is neither, with nothing to say so. Its
    /// default is false, so an ordinary call is unaffected.
    pub fn req_mkt_data_ex(
        &self, req_id: i64, contract: &Contract,
        generic_tick_list: &str, snapshot: bool, regulatory_snapshot: bool,
        mode_9887: i32,
    ) -> Result<(), Refusal> {
        if regulatory_snapshot {
            return Err(Refusal::validation(
                "regulatory_snapshot is not carried by this protocol: the subscription \
                 states the contract and the kind of feed, with no field asking for the \
                 chargeable one-shot snapshot. Ask for an ordinary snapshot with \
                 snapshot=true",
            ));
        }
        // A contract's news is asked for by the venue's id for the contract,
        // and the caller may have stated a description instead. Resolved only
        // when news is what was asked for: a quote on a description is asked
        // for by description and the venue names it itself.
        // The whole entry, not a number ending in it: 1292 is not 292. Matching on
        // the ending qualifies the contract, which is a request to the venue and a
        // wait on the caller's thread, while the core subscribes to no news.
        let wants_news = generic_tick_list.split(',').any(|t| t.trim() == "292");
        let named;
        let contract = if wants_news && contract.con_id == 0 && !contract.symbol.is_empty() {
            named = self.qualify_contract(contract)?;
            &named
        } else {
            contract
        };

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
    pub fn cancel_mkt_data(&self, req_id: i64) -> Result<(), Refusal> {
        // A stream opened by `watch` holds its id as this client's own for as
        // long as it runs. Withdrawing it is where that ends. An id the caller
        // chose was never held, and releasing one that was not held does
        // nothing.
        self.shared.reference.forget_ours(req_id);
        let (instrument, stop_news) = self.core.unregister_mkt_data(req_id);
        // Asked separately, because the quotes stay up for another caller
        // while the headlines this one asked for stop. Withdrawn only
        // alongside the quotes, they carried on with nobody listening.
        if let Some(instrument) = stop_news {
            let _ = self.send(ControlCommand::UnsubscribeNews { instrument });
        }
        if let Some(instrument) = instrument {
            self.send(ControlCommand::Unsubscribe { instrument })?;
        }
        Ok(())
    }

    /// Subscribe to every trade or every quote change on a contract.
    ///
    /// The feed rides the historical farm, registered there under the name
    /// `TickByTick` beside the five-second bars. No separate service is
    /// involved. A missing entitlement arrives as the venue's refusal
    /// rather than as silence.
    ///
    /// `number_of_ticks` and `ignore_size` are refused rather than dropped.
    /// This protocol's subscription states the contract and the kind of stream
    /// and nothing else: there is no field for a prelude of past ticks, and
    /// none for suppressing size-only changes. A caller that set either and
    /// was answered anyway would be reading a stream it did not ask for,
    /// with nothing to say so. Their defaults — no prelude, sizes included —
    /// are what the venue does, so an ordinary call is unaffected.
    pub fn req_tick_by_tick_data(
        &self, req_id: i64, contract: &Contract, tick_type: &str,
        number_of_ticks: i32, ignore_size: bool,
    ) -> Result<(), Refusal> {
        if number_of_ticks != 0 {
            return Err(Refusal::validation(format!(
                "number_of_ticks={number_of_ticks} is not carried by this protocol: a \
                 tick-by-tick subscription states the contract and the kind of stream, \
                 with no field for a prelude of past ticks. Ask for those with \
                 req_historical_ticks",
            )));
        }
        if ignore_size {
            return Err(Refusal::validation(
                "ignore_size is not carried by this protocol: a tick-by-tick \
                 subscription has no field for suppressing size-only changes, so \
                 every change the venue sends is delivered",
            ));
        }
        let kind = TbtType::named(tick_type)?;

        // A stream is asked for by the venue's id for the contract. Sent
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
            .map(|_| ())?;
        // Which stream the callback names when these trades arrive: 1 = Last,
        // 2 = AllLast. The trade record does not carry it; the subscription does.
        self.tbt_kinds.lock().unwrap().insert(req_id, kind);
        Ok(())
    }


    /// Cancel tick-by-tick data. Matches `cancelTickByTickData` in C++.
    pub fn cancel_tick_by_tick_data(&self, req_id: i64) -> Result<(), Refusal> {
        // Only what this request took out. Removing the contract's quote
        // mapping here took the quotes away from whoever was watching them.
        self.tbt_kinds.lock().unwrap().remove(&req_id);
        if let Some(instrument) = self.core.tbt_to_instrument.lock().unwrap().remove(&req_id) {
            self.send(ControlCommand::UnsubscribeTbt { req_id, instrument })?;
        }
        Ok(())
    }

    // ── Market Depth ──

    /// Subscribe to market depth (L2 order book). Matches `reqMktDepth` in C++.
    ///
    /// A contract that names no venue and no security type is sent as it
    /// stands. The engine reads an unnamed venue as the smart destination and
    /// checks a named security type against the venue's routing table.
    /// Substituting a stock here asks for a future's book as a stock's, which
    /// the venue refuses as a book it does not serve.
    pub fn req_mkt_depth(
        &self, req_id: i64, contract: &Contract,
        num_rows: i32, is_smart_depth: bool,
    ) -> Result<(), Refusal> {
        self.send(ControlCommand::SubscribeDepth {
            contract: ContractRef {
                con_id: contract.con_id,
                symbol: contract.symbol.clone(),
                exchange: contract.exchange.clone(),
                sec_type: contract.sec_type.clone(),
                currency: contract.currency.clone(),
                ..Default::default()
            },
            req_id: wire_req_id(req_id)?,
            filters: contract.lookup_filters(),
            num_rows,
            is_smart_depth,
        })
    }

    /// Cancel market depth. Matches `cancelMktDepth` in C++.
    pub fn cancel_mkt_depth(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::UnsubscribeDepth { req_id: wire_req_id(req_id)? })
    }

    // ── Real-Time Bars ──

    /// Subscribe to real-time 5-second bars. Matches `reqRealTimeBars` in C++.
    ///
    /// `bar_size` is taken and not applied. The venue's real-time bar is five
    /// seconds and there is no field asking for another; the reference client
    /// takes the number and sends none either.
    pub fn req_real_time_bars(
        &self, req_id: i64, contract: &Contract,
        _bar_size: i32, what_to_show: &str, use_rth: bool,
    ) -> Result<(), Refusal> {
        // Refused here rather than turned into trades on the way out: a
        // misspelled "BID" answered with trade bars looks like data.
        crate::control::historical::BarDataType::from_api_str(what_to_show)?;
        self.send(ControlCommand::SubscribeRealTimeBar {
            contract: contract.into(),
            req_id: wire_req_id(req_id)?,
            filters: contract.lookup_filters(),
            what_to_show: what_to_show.into(),
            use_rth,
        })
    }

    /// Cancel real-time bars. Matches `cancelRealTimeBars` in C++.
    pub fn cancel_real_time_bars(&self, req_id: i64) -> Result<(), Refusal> {
        self.send(ControlCommand::CancelRealTimeBar { req_id: wire_req_id(req_id)? })
    }

    /// Request an auth-connection round-trip time sample: sends a
    /// lightweight liveness probe with no side effects on subscriptions,
    /// contract caches, or pacing budgets. The result lands asynchronously —
    /// poll `last_rtt()` after a moment. No-op while a probe is already in
    /// flight or the connection is down.
    pub fn req_ping(&self) -> Result<(), Refusal> {
        self.send(ControlCommand::Ping)
    }

    /// Last measured auth-connection round-trip time, if any.
    /// A gauge, not a benchmark: the sample is the interval from a probe to
    /// the first inbound traffic that followed it, which on an active feed
    /// can undercount by racing data already in flight. Also sampled
    /// automatically whenever liveness sends its own probe.
    pub fn last_rtt(&self) -> Option<std::time::Duration> {
        self.shared.last_ccp_rtt()
    }

    /// Which feed the subscriptions after this one ask for: 1 live, 2 frozen,
    /// 3 delayed, 4 delayed and frozen.
    ///
    /// Sent with each subscription, in the field this protocol carries it in,
    /// and the `market_data_type` callback reports the type the subscription
    /// was made under. To state it for one request rather than for the ones
    /// that follow, [`req_mkt_data_ex`](EClient::req_mkt_data_ex) takes it.
    /// A number naming no type leaves subscriptions realtime, and says so.
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
    /// Returns `None` for an id outside the instrument table.
    #[inline]
    pub fn quote_by_instrument(&self, instrument: InstrumentId) -> Option<Quote> {
        self.shared.market.try_quote(instrument)
    }
}
