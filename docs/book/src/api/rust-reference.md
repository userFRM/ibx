# Rust API Reference (v0.7.1)

*Auto-generated from source — do not edit.*

## Table of Contents

- [EClient: Connection](#connection)
- [EClient: Calls That Answer](#calls-that-answer)
- [EClient: Account & Portfolio](#account--portfolio)
- [EClient: Orders](#orders)
- [EClient: Market Data](#market-data)
- [EClient: Reference Data](#reference-data)
- [EClient: Gateway-Local & Stubs](#gateway-local--stubs)
- [Wrapper Callbacks](#wrapper-callbacks)

## Connection

#### `connect`

Connect to IB and start the engine.

```rust
pub fn connect(config: &EClientConfig) -> Result<Self, Box<dyn std::error::Error>>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `config` | `&EClientConfig` | Connection configuration (username, password, host, paper, core_id). |

**Returns:** `Result<Self, Box<dyn std::error::Error>>`

---

#### `connect_with_events`

Connect to IB and start the engine with an event channel attached. A second, optional delivery path for a program that would rather own a queue than be called back. It is bounded, and an event arriving at a full one is discarded rather than made to wait — a session that stalled on a slow reader would stop carrying market data. Read `events_lost` to learn whether that happened. One reader, and it is told what it drains and nothing else. For a program that wants none of it dropped, drive `process_msgs` with a `Wrapper` instead: its callbacks are called with the message rather than sent a copy of it, so there is no queue to fill and nothing to fall out of one. This is a second, optional delivery path that runs alongside `process_msgs` — it does not replace it, and nothing is removed from the wrapper callbacks when it is in use. The channel is bounded by `capacity`; the engine never blocks on it, so a consumer that falls behind loses events rather than slowing the hot loop. Drain it from a thread that is not the one calling `process_msgs()`, or keep `capacity` generous. Attaching a channel makes the engine build events it would otherwise skip, which for bar batches and contract definitions means one deep copy each. Use `connect()` when you only need the wrapper callbacks.

```rust
pub fn connect_with_events( config: &EClientConfig, capacity: usize, ) -> Result<(Self, Receiver<Event>), Box<dyn std::error::Error>>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `config` | `&EClientConfig` | Connection configuration (username, password, host, paper, core_id). |
| `capacity` | `usize` |  |

**Returns:** `Result<(Self, Receiver<Event>), Box<dyn std::error::Error>>`

---

#### `from_parts`

Construct from pre-built components (for testing or custom setups).

```rust
pub fn from_parts( shared: Arc<SharedState>, control_tx: SyncSender<ControlCommand>, handle: thread::JoinHandle<()>, account_id: String, ) -> Self
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `shared` | `Arc<SharedState>` | Shared state handle. |
| `control_tx` | `SyncSender<ControlCommand>` | Control channel sender. |
| `handle` | `thread::JoinHandle<(` | Background thread handle. |

**Returns:** `Self`

---

#### `map_req_instrument`

Map a reqId to an InstrumentId (for testing without a live engine).

```rust
pub fn map_req_instrument(&self, req_id: i64, instrument: InstrumentId)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `instrument` | `InstrumentId` | Instrument type for scanner (e.g. `"STK"`, `"FUT"`). |

---

#### `track_order_for_test`

Pre-populate the order tracker (for testing the dispatcher path without going through the engine's place-order flow).

```rust
pub fn track_order_for_test( &self, order_id: u64, contract: ApiContract, order: ApiOrder, instrument: InstrumentId, )
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `order_id` | `u64` | Order identifier. Must be unique per session. |
| `contract` | `ApiContract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `order` | `ApiOrder` | Order parameters (action, quantity, type, price, TIF, etc.). |
| `instrument` | `InstrumentId` | Instrument type for scanner (e.g. `"STK"`, `"FUT"`). |

---

#### `seed_instrument`

Pre-seed a con_id → InstrumentId mapping (for testing without a live engine).

```rust
pub fn seed_instrument(&self, con_id: i64, instrument: InstrumentId)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `con_id` | `i64` | Contract ID. Unique per instrument. |
| `instrument` | `InstrumentId` | Instrument type for scanner (e.g. `"STK"`, `"FUT"`). |

---

#### `events_lost`

How many events the channel from `connect_with_events` discarded. The engine never waits on a reader — a session that stalled on one would stop carrying market data — so an event arriving at a full channel is dropped. A program that acted on every fill it saw needs to know the difference between that and every fill there was. Zero for a session with no channel attached, and for one whose reader kept up.

```rust
pub fn events_lost(&self) -> u64
```

**Returns:** `u64`

---

#### `is_connected`

False after `disconnect()`, and after a `process_msgs()` call that observed the engine stopping.

```rust
pub fn is_connected(&self) -> bool
```

**Returns:** `bool`

---

#### `disconnect`

Disconnect from IB.  Sends `Shutdown` to the hot loop, waits for the background thread to exit, and marks the client as disconnected.

```rust
pub fn disconnect(&self)
```

---

#### `instrument_of`

Which slot a contract holds on this session, if it holds one.

```rust
pub fn instrument_of(&self, con_id: i64) -> Option<crate::types::InstrumentId>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `con_id` | `i64` | Contract ID. Unique per instrument. |

**Returns:** `Option<crate::types::InstrumentId>`

---

#### `shared_state`

The session's own state, for reading what has arrived.

```rust
pub fn shared_state(&self) -> &Arc<SharedState>
```

**Returns:** `&Arc<SharedState>`

---

#### `adjustments`

What the venue last stated for a contract, and the contract as it named it. Not every action of the contract's life: each question replaces what the one before it left, so this states the answer to the last range asked about. The venue serves no adjusted series of its own: asked for one by name it answers that it has no such data, and the trades it does serve are raw. A series that crosses a split steps by the split's ratio with nothing in it saying so, which is a wrong number rather than a missing one. This is what a caller adjusts with. `scale_before` turns a date and these actions into the factor a price from that date carries, so a caller can put a series on one scale. Splits are what it applies: the ratio is the value the action states, established against a contract that split ten for one where the closes either side were 1208.88 and 121.79. Dividends are stated here and not applied by it, because how much of one comes off a historical price is a convention this client has not established against anything it can check. Empty until the venue has stated them for the contract, which it does once per contract on a historical request.

```rust
pub fn adjustments(&self, con_id: &str) -> Option<(AdjustedContract, Vec<Adjustment>)>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `con_id` | `&str` | Contract ID. Unique per instrument. |

**Returns:** `Option<(AdjustedContract, Vec<Adjustment>)>`

---

#### `unread_wire`

Frames this session kept exactly as the venue sent them, by connection. Empty unless `IBX_CAPTURE_WIRE` is set. A reading checked only against frames this client made up says nothing about the ones that arrive.

```rust
pub fn unread_wire(&self) -> Vec<(&'static str, String)>
```

**Returns:** `Vec<(&'static str, String)>`

---

#### `ccp_session_id`

Session ID surfaced to webapp REST clients as `x-ccp-session-id`.

```rust
pub fn ccp_session_id(&self) -> String
```

**Returns:** `String`

---

#### `misc_url`

Logical-name → host URL lookup from the gateway logon MiscUrls push (e.g. `region_dam`). Returns `None` when the gateway did not push this key.

```rust
pub fn misc_url(&self, key: &str) -> Option<String>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `key` | `&str` | Account value key (e.g. `"NetLiquidation"`, `"BuyingPower"`). |

**Returns:** `Option<String>`

---

#### `session_token_bytes`

Canonical big-endian session-token bytes (leading zeros stripped) captured at connect. Round-trips through `BigUint::from_bytes_be` to the SRP shared secret K and is the second SHA-1 input for SSO `Authenticate-TWS` bodies.

```rust
pub fn session_token_bytes(&self) -> &[u8]
```

**Returns:** `&[u8]`

---

#### `session`

The session this connection established, for a caller that wants to resume from it later. Hand it back through [`EClientConfig::resume`] on a subsequent connect. Keep it wherever the process keeps secrets — it is a credential, and where it lives is the caller's decision, which is why nothing here writes it anywhere by default.

```rust
pub fn session(&self) -> &crate::auth::resume::ResumableSession
```

**Returns:** `&crate::auth::resume::ResumableSession`

---

#### `token_type`

`stoken_type` discriminator captured at connect (`"st"`, `"tst"`, `"zenith"`, or empty for the SRP-only path). Sent verbatim in SSO authenticator bodies.

```rust
pub fn token_type(&self) -> &str
```

**Returns:** `&str`

---

## Calls That Answer

#### `historical_data`

Bars for a contract, as `req_historical_data` asks for them. `ADJUSTED_LAST` is served here and refused by `req_historical_data`, and the difference is not arbitrary. The venue has no adjusted series to pass through: what it serves is raw, and adjusting it needs the contract's actions in hand before a bar can be handed to anyone. A call that waits can hold both; one that answers on a callback cannot, and would have to hand over raw bars under an adjusted name.

```rust
pub fn historical_data( &self, contract: &Contract, end_date_time: &str, duration: &str, bar_size: &str, what_to_show: &str, use_rth: bool, ) -> Result<Vec<BarData>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `end_date_time` | `&str` | End date/time in `"YYYYMMDD HH:MM:SS"` format, or empty for now. |
| `duration` | `&str` | Duration string, e.g. `"1 D"`, `"1 W"`, `"1 M"`, `"1 Y"`. |
| `bar_size` | `&str` | Bar size: `"1 min"`, `"5 mins"`, `"1 hour"`, `"1 day"`, etc. |
| `what_to_show` | `&str` | Data type: `"TRADES"`, `"MIDPOINT"`, `"BID"`, `"ASK"`, `"BID_ASK"`, etc. |
| `use_rth` | `bool` | If `true`, only return data from Regular Trading Hours. |

**Returns:** `Result<Vec<BarData>, Refusal>`

---

#### `corporate_actions`

A contract's corporate actions, asked for and waited on. The venue answers these per contract rather than per request, which is enough to file an answer and not enough to know whose question it answers: two questions about one contract over different ranges are answered by two replies naming the same contract. The id the request went out under is carried through, and this takes only the answer to its own. A contract the venue states nothing for answers empty, which is an answer: it is how a contract that has never split says so. `contract` must carry the venue's id for it, which `qualify_contract` supplies. Days are `YYYYMMDD`.

```rust
pub fn corporate_actions( &self, contract: &Contract, start_date: &str, end_date: &str, ) -> Result<Vec<crate::control::adjustments::Adjustment>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `start_date` | `&str` |  |
| `end_date` | `&str` |  |

**Returns:** `Result<Vec<crate::control::adjustments::Adjustment>, Refusal>`

---

#### `option_chain`

Every expiration and strike each venue lists for an underlying. `underlying` must carry the id of the contract the options are on — the stock, not the option — which `qualify_contract` supplies.

```rust
pub fn option_chain( &self, underlying: &Contract, ) -> Result<Vec<OptionChain>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `underlying` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |

**Returns:** `Result<Vec<OptionChain>, Refusal>`

---

#### `head_timestamp`

The earliest moment the venue holds data for a contract. The same question `req_head_time_stamp` asks.

```rust
pub fn head_timestamp( &self, contract: &Contract, what_to_show: &str, use_rth: bool, ) -> Result<String, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `what_to_show` | `&str` | Data type: `"TRADES"`, `"MIDPOINT"`, `"BID"`, `"ASK"`, `"BID_ASK"`, etc. |
| `use_rth` | `bool` | If `true`, only return data from Regular Trading Hours. |

**Returns:** `Result<String, Refusal>`

---

#### `matching_symbols`

Contracts whose name or symbol matches a pattern.

```rust
pub fn matching_symbols( &self, pattern: &str, ) -> Result<Vec<crate::types::model::ContractDescription>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `pattern` | `&str` | Symbol search pattern. |

**Returns:** `Result<Vec<crate::types::model::ContractDescription>, Refusal>`

---

#### `news_headlines`

The headlines the venue holds for a contract, up to the number asked for. Each is the time, the provider's code, the article's id and the headline itself. Reading an article needs the first two. The venue states whether it holds more than it sent, and that is reported through the log rather than in the returned rows: what comes back is a page, and a full one is not evidence there is no next one. Ask for more, or narrow the window, to see the rest.

```rust
pub fn news_headlines( &self, con_id: i64, provider_codes: &str, start_date_time: &str, end_date_time: &str, total_results: i32, ) -> Result<Vec<Headline>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `con_id` | `i64` | Contract ID. Unique per instrument. |
| `provider_codes` | `&str` | Pipe-separated news provider codes. |
| `start_date_time` | `&str` | Start date/time for tick query. |
| `end_date_time` | `&str` | End date/time in `"YYYYMMDD HH:MM:SS"` format, or empty for now. |
| `total_results` | `i32` | Maximum number of news results. |

**Returns:** `Result<Vec<Headline>, Refusal>`

---

#### `histogram_data`

How a contract's trades were spread across prices.

```rust
pub fn histogram_data( &self, contract: &Contract, use_rth: bool, period: &str, ) -> Result<Vec<(f64, i64)>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `use_rth` | `bool` | If `true`, only return data from Regular Trading Hours. |
| `period` | `&str` | Histogram period, e.g. `"1week"`, `"1month"`. |

**Returns:** `Result<Vec<(f64, i64)>, Refusal>`

---

#### `fundamental_data`

A fundamental document about a contract, as the venue writes it.

```rust
pub fn fundamental_data( &self, contract: &Contract, report_type: &str, ) -> Result<String, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `report_type` | `&str` | Report type: `"ReportSnapshot"`, `"ReportsFinSummary"`, `"RESC"`, etc. |

**Returns:** `Result<String, Refusal>`

---

#### `what_if_order`

What the venue says an order would cost, without placing it. The order is marked as a question rather than an instruction, so nothing reaches the market. The preview states the order's own type, so a security that refuses a type refuses the preview of it rather than answering about an order that was not asked about. What it cannot state is the instruction that separates types sharing one: trailing, relative and the pegged pair are all sent as "P" and separated by their ExecInst, which a preview does not carry, so the venue reads any of them as the same peg. The margin is the same either way — it follows the position the order would leave, not the instruction that reaches it. A type this client states no value for is previewed as a limit at the same price, which is the only thing left to ask.

```rust
pub fn what_if_order( &self, contract: &Contract, order: &crate::types::model::Order, ) -> Result<crate::types::model::OrderState, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `order` | `&crate::types::model::Order` | Order parameters (action, quantity, type, price, TIF, etc.). |

**Returns:** `Result<crate::types::model::OrderState, Refusal>`

---

#### `positions`

Every holding in the account.

```rust
pub fn positions(&self) -> Result<Vec<PositionRow>, Refusal>
```

**Returns:** `Result<Vec<PositionRow>, Refusal>`

---

#### `account_summary`

The account values named by `tags`, as `req_account_summary` asks for them. `tags` is a comma-separated list, or `All`.

```rust
pub fn account_summary(&self, tags: &str) -> Result<Vec<AccountValue>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `tags` | `&str` | Comma-separated account tags: `"NetLiquidation,BuyingPower,..."`. |

**Returns:** `Result<Vec<AccountValue>, Refusal>`

---

#### `await_order`

Wait for an order to reach a state the venue will not move it from. Placing an order says only that it was sent. What happened to it arrives later, on a callback, spread across a status and possibly a refusal. This waits for the venue to finish with it and reports where it landed — including the refusal, which is the part a caller most needs and the part most easily missed. A wait that runs out is not a failure of the order: it says only that the venue had not finished, and the order is still working.

```rust
pub fn await_order( &self, order_id: i64, timeout: Duration, ) -> Result<OrderReport, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `order_id` | `i64` | Order identifier. Must be unique per session. |
| `timeout` | `Duration` |  |

**Returns:** `Result<OrderReport, Refusal>`

---

#### `contract_details`

Every contract matching the one described. The same question `req_contract_details` asks, answered here instead of on a callback.

```rust
pub fn contract_details(&self, contract: &Contract) -> Result<Vec<ContractDetails>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |

**Returns:** `Result<Vec<ContractDetails>, Refusal>`

---

#### `qualify_contract`

Fill in what the venue knows about a contract, above all its id. Most of what this client sends carries a contract, and a contract with an id is worth more than one without: market data is answered only for a contract named by id, and an order that carries one needs to state nothing else. Ask this first and pass the result around. A description matching more than one contract is refused rather than resolved to whichever came back first — the same symbol on the same venue exists in more than one currency, and picking one silently is how an order ends up on the wrong one.

```rust
pub fn qualify_contract(&self, contract: &Contract) -> Result<Contract, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |

**Returns:** `Result<Contract, Refusal>`

---

#### `qualify_contracts`

Fill in a batch of contracts, keeping the caller's order. Stops at the first that cannot be named, because a caller building a basket wants to know which one is wrong, not to trade the rest.

```rust
pub fn qualify_contracts(&self, contracts: &[Contract]) -> Result<Vec<Contract>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contracts` | `&[Contract]` | Contract specification (symbol, secType, exchange, currency, etc.). |

**Returns:** `Result<Vec<Contract>, Refusal>`

---

#### `scan`

Run a scan and hand back what it found. The subscription is withdrawn before this returns: a scan asked for once is a question, and left running it keeps answering into a session nobody is reading.

```rust
pub fn scan( &self, instrument: &str, location: &str, scan_code: &str, most: u32, ) -> Result<Vec<ScanRow>, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `instrument` | `&str` | Instrument type for scanner (e.g. `"STK"`, `"FUT"`). |
| `location` | `&str` |  |
| `scan_code` | `&str` | Scanner code (e.g. `"TOP_PERC_GAIN"`, `"HIGH_OPT_IMP_VOLAT"`). |
| `most` | `u32` |  |

**Returns:** `Result<Vec<ScanRow>, Refusal>`

---

#### `schedule`

When a contract trades, over a window ending now.

```rust
pub fn schedule(&self, contract: &Contract, duration: &str) -> Result<Schedule, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `duration` | `&str` | Duration string, e.g. `"1 D"`, `"1 W"`, `"1 M"`, `"1 Y"`. |

**Returns:** `Result<Schedule, Refusal>`

---

#### `calendar_schema`

What the corporate-events calendar says it carries. As the venue's JSON: it states a schema of its own that changes without notice, and a shape imposed here would be a shape to keep in step with it.

```rust
pub fn calendar_schema(&self) -> Result<String, Refusal>
```

**Returns:** `Result<String, Refusal>`

---

#### `calendar_events`

The calendar's events for one contract, as the venue's JSON.

```rust
pub fn calendar_events(&self, con_id: i64) -> Result<String, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `con_id` | `i64` | Contract ID. Unique per instrument. |

**Returns:** `Result<String, Refusal>`

---

## Account & Portfolio

#### `req_positions`

Request positions. Waits for the account data the venue pushes as a session opens, then delivers what it holds and calls `position_end`. The wait is bounded, and an account that says nothing within it delivers nothing — which reads the same as an account holding nothing. Said in the log rather than left to be inferred, because the two are not the same answer.

```rust
pub fn req_positions(&self, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_pnl`

Subscribe to account PnL updates. The venue is asked, under `account` or under the one this session opened with where none is named. What comes back is what each holding was worth at midnight and what it has realised since, and the figures reported on `pnl` are worked out from those against the prices the session is being told. Without the subscription none of that arrives, and the figures reduce to the unrealised part with nothing realised on any position. `model_code` is taken and not applied: there is no model portfolio to name here.

```rust
pub fn req_pnl(&self, req_id: i64, account: &str, _model_code: &str)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `account` | `&str` | Account ID. |
| `model_code` | `&str` | Model portfolio code (empty for default). |

---

#### `cancel_pnl`

Cancel PnL subscription. Nothing withdraws one on this wire, and the engine says so: what stops is the reporting, not the venue.

```rust
pub fn cancel_pnl(&self, req_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `req_pnl_single`

Subscribe to single-position PnL updates. `account` and `model_code` are taken and not applied, as on `req_pnl`: the figures are for the account this session opened under.

```rust
pub fn req_pnl_single(&self, req_id: i64, _account: &str, _model_code: &str, con_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `account` | `&str` | Account ID. |
| `model_code` | `&str` | Model portfolio code (empty for default). |
| `con_id` | `i64` | Contract ID. Unique per instrument. |

---

#### `cancel_pnl_single`

Cancel single-position PnL subscription.

```rust
pub fn cancel_pnl_single(&self, req_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `req_account_summary`

Request account summary. `group` is taken and not applied. One session holds one account here, and the venue states its figures for that account without being asked which, so there is no second account or model portfolio to name.

```rust
pub fn req_account_summary(&self, req_id: i64, _group: &str, tags: &str)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `group` | `&str` | Account group name (e.g. `"All"`). |
| `tags` | `&str` | Comma-separated account tags: `"NetLiquidation,BuyingPower,..."`. |

---

#### `cancel_account_summary`

Cancel account summary.

```rust
pub fn cancel_account_summary(&self, req_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `req_account_updates`

Subscribe to account updates. `acct_code` is taken and not applied. One session holds one account here, and the venue states its figures for that account without being asked which, so there is no second account or model portfolio to name. Subscribing also asks the venue to state the figures now. It restates them on its own schedule otherwise, which is unhurried: a session that has just opened waits tens of seconds for its first set, and a caller that subscribed and then read the account got nothing.

```rust
pub fn req_account_updates(&self, subscribe: bool, _acct_code: &str)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `subscribe` | `bool` | `true` to start updates, `false` to stop. |
| `acct_code` | `&str` | Account code (e.g. `"DU1234567"`). |

---

#### `cancel_positions`

Cancel positions subscription. Nothing is withdrawn from the venue: it pushes what the account holds as the session opens and keeps it current whether or not anyone is listening. What stops is the reporting — a holding that moves after this is no longer delivered on `position`.

```rust
pub fn cancel_positions(&self)
```

---

#### `req_managed_accts`

Request managed accounts. Answered with every account this login holds, comma separated, which is the shape the reference client answers in. A login with one account is answered with that one account and no comma.

```rust
pub fn req_managed_accts(&self, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_account_updates_multi`

Request account updates for multiple accounts/models. Account values for one account or model, answered on `account_update_multi`. The reference client answers this request on its own callbacks, not on the ones `req_account_updates` uses, and a caller written against it implements those and hears nothing otherwise. `ledger_and_nlv` is taken and not applied. The account figures arrive as the venue states them, and it states the ledger and the net liquidation among them without being asked. The figures are the ones the venue states for the account this session opened under, and they are labelled with that account. A login holding several is answered for that one; naming another here does not fetch the other's figures, and is said in the log rather than answered with this account's under the other's name.

```rust
pub fn req_account_updates_multi( &self, req_id: i64, account: &str, model_code: &str, _ledger_and_nlv: bool, wrapper: &mut impl Wrapper, )
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `account` | `&str` | Account ID. |
| `model_code` | `&str` | Model portfolio code (empty for default). |
| `ledger_and_nlv` | `bool` | If `true`, include ledger and NLV data. |
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `cancel_account_updates_multi`

Cancel multi-account updates. `_req_id` reaches nothing, because there is nothing to withdraw: the request it would name is answered from what this session already holds, before a caller has this to cancel it with.

```rust
pub fn cancel_account_updates_multi(&self, _req_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `req_positions_multi`

Request positions for multiple accounts/models. Holdings for one account or model, answered on `position_multi`. Answered from the holdings this session already has, rather than by pumping for them: pumping here would drain every queued event into a collector that reports holdings and discards the rest, so a caller running its own loop would lose whatever had arrived since it last pumped.

```rust
pub fn req_positions_multi( &self, req_id: i64, account: &str, model_code: &str, wrapper: &mut impl Wrapper, )
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `account` | `&str` | Account ID. |
| `model_code` | `&str` | Model portfolio code (empty for default). |
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `cancel_positions_multi`

```rust
pub fn cancel_positions_multi(&self, req_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `positions_elsewhere`

Holdings the venue reports that this broker does not hold itself: positions held away at another broker, and rows it marks as shown but not held. Kept apart from `positions`, which answers what the account itself holds. The reference client has no call for these — its own front end shows them in a separate table — so this is the only way to reach them.

```rust
pub fn positions_elsewhere(&self) -> Vec<crate::types::PositionElsewhere>
```

**Returns:** `Vec<crate::types::PositionElsewhere>`

---

#### `values_elsewhere`

The account figures describing one of the sets of holdings the account does not hold itself, as name and value. The venue states these the same way it states the account's own, and mixing them in would overstate what the account is worth, so they are kept where the holdings they describe are kept.

```rust
pub fn values_elsewhere(&self, held: crate::types::HeldElsewhere) -> Vec<(String, String)>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `held` | `crate::types::HeldElsewhere` |  |

**Returns:** `Vec<(String, String)>`

---

#### `account`

Read account state snapshot.

```rust
pub fn account(&self) -> AccountState
```

**Returns:** `AccountState`

---

## Orders

#### `order_permissions`

Security type → the order types the venue permits for it, as stated at logon. Empty until the session is up.

```rust
pub fn order_permissions(&self) -> std::collections::HashMap<String, Vec<String>>
```

**Returns:** `std::collections::HashMap<String, Vec<String>>`

---

#### `permitted_order_types`

The order types permitted for one security type, or `None` when the type is not permitted at all. A combination is named `COMB`.

```rust
pub fn permitted_order_types(&self, sec_type: &str) -> Option<Vec<String>>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `sec_type` | `&str` |  |

**Returns:** `Option<Vec<String>>`

---

#### `enabled_features`

Feature tokens the venue enables for this account: those stated at logon, and those the account configuration adds afterwards.

```rust
pub fn enabled_features(&self) -> Vec<String>
```

**Returns:** `Vec<String>`

---

#### `algorithms`

Which algorithms the venue offers, keyed `PROVIDER/SECTYPE`. Stated on the session rather than per contract. An algorithm absent here is one this account may not use, and an order naming it is refused by the venue.

```rust
pub fn algorithms(&self) -> std::collections::HashMap<String, Vec<String>>
```

**Returns:** `std::collections::HashMap<String, Vec<String>>`

---

#### `algorithms_for`

The algorithms offered for one security type, across every provider.

```rust
pub fn algorithms_for(&self, sec_type: &str) -> Vec<String>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `sec_type` | `&str` |  |

**Returns:** `Vec<String>`

---

#### `place_order`

Place an order. An order names its contract by the venue's id. A caller who states a description instead of an id — which every example written against the reference client does — has it resolved here, once the order itself is known to be one the venue would take: an order that names no contract is one the venue has nothing to match, and answers with nothing at all. Resolving it costs a request and an answer the first time, so this call does not return until the venue has named the contract — up to the answer timeout. Once per description: the answer is kept, and later orders on the same contract are sent without asking again. The reference client never waits here, because a gateway resolved the contract before the order reached it; this client is the gateway, so the work happens somewhere, and today it happens on the caller's thread. A caller placing orders from inside a callback stalls its own dispatch loop for that time. Pass a contract carrying `con_id` — from `qualify_contract`, or from any contract-details answer — and nothing is resolved and nothing waits.

```rust
pub fn place_order(&self, order_id: i64, contract: &Contract, order: &Order) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `order_id` | `i64` | Order identifier. Must be unique per session. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `order` | `&Order` | Order parameters (action, quantity, type, price, TIF, etc.). |

**Returns:** `Result<(), Refusal>`

---

#### `exercise_options`

Exercise or lapse a long option position. `exercise_action` is 1 to exercise and 2 to lapse; anything else is refused. `override_` is taken and not sent, because there is no tag for it: it names a check made before the order is built, not one the venue makes. The check it names is a real one — it is what stops an exercise of an option that is out of the money and a lapse of one that is in it — and this client does not make it, because what it rests on is the venue's word on where the option stands, which this client does not ask for. So an instruction is sent as given, and `override_ = false` buys no protection here. Passing `true` is the honest description of what happens either way; passing `false` says so in the log.

```rust
pub fn exercise_options( &self, req_id: i64, contract: &Contract, exercise_action: i32, exercise_quantity: i32, account: &str, override_: bool, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `exercise_action` | `i32` | 1=exercise, 2=lapse. |
| `exercise_quantity` | `i32` | Number of contracts to exercise. |
| `account` | `&str` | Account ID. |
| `override_` | `bool` |  |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_order`

Cancel an order. `manual_order_cancel_time` is taken and not applied. A cancel names five fields on this wire and no time among them, as the protocol's cancel does.

```rust
pub fn cancel_order(&self, order_id: i64, _manual_order_cancel_time: &str) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `order_id` | `i64` | Order identifier. Must be unique per session. |
| `manual_order_cancel_time` | `&str` | Manual cancel time (empty for immediate). |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_order_by_perm_id`

Cancel an order identified by `permId` — stable across sessions. `permId` is the broker-assigned identifier returned in `order_status` callbacks and surfaced in account tools. Useful for cancelling an order placed in a prior session, where the local `order_id` is not retained. the CCP cancel frame is orderId-only, so ibx looks up the local `order_id` from `permId` in the open-order cache (populated by `place_order` callbacks or by the CCP session-recovery push hydrated in `handle_exec_report`). Fails if `perm_id` is not currently tracked.

```rust
pub fn cancel_order_by_perm_id(&self, perm_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `perm_id` | `i64` | Permanent order ID assigned by the server. |

**Returns:** `Result<(), Refusal>`

---

#### `req_global_cancel`

Cancel all orders.

```rust
pub fn req_global_cancel(&self) -> Result<(), Refusal>
```

**Returns:** `Result<(), Refusal>`

---

#### `req_ids`

Request next valid order ID.

```rust
pub fn req_ids(&self, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `next_order_id`

Get the next order ID (local counter).

```rust
pub fn next_order_id(&self) -> i64
```

**Returns:** `i64`

---

#### `req_open_orders`

Request open orders for this client. Answers with every order working on the account, as `req_all_open_orders` does. The protocol carries no client number on an order, so this session cannot tell which orders it placed; reporting fewer would omit working orders.

```rust
pub fn req_open_orders(&self, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_all_open_orders`

Request all open orders.

```rust
pub fn req_all_open_orders(&self, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_completed_orders`

Request completed orders. Immediately delivers every completed order this session archived, then calls `completed_orders_end`. `api_only` is taken and not applied. It asks for orders entered through an API rather than by hand, and nothing this client holds says which an order was: the completed orders are the ones this session saw, and the venue states no origin on them. Passing `true` is answered with all of them rather than with a guess at which were typed.

```rust
pub fn req_completed_orders(&self, api_only: bool, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `api_only` | `bool` |  |
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_auto_open_orders`

Automatically bind future orders to this client. Bind orders entered elsewhere to this client. Nothing goes to the venue; this is answered locally, setting a property of its own and refusing it for any client but the one those orders bind to. What that property gates does not arise here — this session is told about every order on the account, whether it placed them or not — and this surface names no client, so there is nothing to refuse and nothing left to do. `b_auto_bind` is taken and not applied. Whether it asks to bind or to stop binding, the answer is the same: this session hears about every order on the account either way.

```rust
pub fn req_auto_open_orders(&self, _b_auto_bind: bool)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `b_auto_bind` | `bool` | If `true`, auto-bind future orders to this client. |

---

#### `req_executions`

Request execution reports. Replays stored executions (optionally filtered), firing `exec_details` + `commission_and_fees_report` for each, then `exec_details_end`.

```rust
pub fn req_executions(&self, req_id: i64, filter: &ExecutionFilter, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `filter` | `&ExecutionFilter` | Execution filter (client_id, acct_code, time, symbol, sec_type, exchange, side). |
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `parse_algo_params`

Parse algo strategy and TagValue params into internal AlgoParams. A key the caller never set defaults the way IB's own algos do (0.0, false, or the documented default enum value). A key the caller *did* set — even to an empty string — is refused if it does not parse, rather than taking that same default: `riskAversion="Aggresive"` would otherwise submit a Neutral algo with no error, and `maxPctVol=""` would submit 0.0. A strategy modelled here is re-encoded from the fields it names rather than forwarded as the caller wrote it, so a key it does not name would go no further. Said rather than dropped: the caller had set it for a reason and the order that reached the venue would not have carried it.

```rust
pub fn parse_algo_params(strategy: &str, params: &[TagValue]) -> Result<AlgoParams, Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `strategy` | `&str` | Algo strategy name (e.g. `"Vwap"`, `"Twap"`). |
| `params` | `&[TagValue]` | Algo parameter list. |

**Returns:** `Result<AlgoParams, Refusal>`

---

## Market Data

#### `req_mkt_data`

Subscribe to market data. When `snapshot` is true, delivers the first available quote then calls `tick_snapshot_end` and auto-cancels the subscription. That is a subscription this client ends, not a request of its own: the venue's own one-shot snapshot is the chargeable one, asked for with `regulatory_snapshot` on `req_mkt_data_ex`. `generic_tick_list` is NOT transmitted to the gateway, with one exception: "292" additionally subscribes per-contract news. Other generic tick types (RTVolume and friends) are not requested — the venue asks for those under numbers of its own rather than the ones this list uses, and this client does not know the mapping. Naming one is warned about rather than quietly dropped. `tick_generic` does fire, for the halt the venue states on its own tick: tick 49, 0 while a contract is trading and 1 once it has stopped. Delayed and frozen data are requested, contrary to what this said: name the type on `req_market_data_type` and every subscription after it carries the mode, or state it per request with `req_mkt_data_ex`. The table there gives the wire shape of each.

```rust
pub fn req_mkt_data( &self, req_id: i64, contract: &Contract, generic_tick_list: &str, snapshot: bool, regulatory_snapshot: bool, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `generic_tick_list` | `&str` | Comma-separated generic tick IDs (e.g. `"233"` for RT volume). |
| `snapshot` | `bool` | If `true`, delivers one quote then auto-cancels. |
| `regulatory_snapshot` | `bool` | If `true`, request a regulatory snapshot (additional fees may apply). |

**Returns:** `Result<(), Refusal>`

---

#### `req_mkt_data_ex`

Like `req_mkt_data`, but encodes the market-data mode per-request via FIX field 9887, allowing parallel realtime + frozen subscriptions for the same contract: | `mode_9887` | mode             | wire shape | |-------------|------------------|---| | `0`         | REALTIME         | `264=442` (BID_ASK) + `264=443` (LAST), no 9887 | | `1`         | DELAYED          | `264=1` (TOP) + `9887=1` | | `2`         | FROZEN           | `264=1` (TOP) + `9887=2` | | `3`         | DELAYED_FROZEN   | `264=1` (TOP) + `9887=3` | The frozen mode keeps thinly-traded names quoting after-hours, when the realtime feed is silent. A contract holds one subscription at a time, so this states the mode for that subscription rather than adding a parallel one — to compare modes on one contract, cancel between them. To set the mode for every subscription instead of naming it per request, call `req_market_data_type`. `regulatory_snapshot` asks for the venue's own chargeable one-shot snapshot: a request type of its own rather than a mode on an ordinary quote, asked for under the snapshot action and with no feed named beside it. It is billed per snapshot and needs the entitlement — an account without it is refused by the venue, which names the request type back. It ends the way an ordinary snapshot does, so a caller hears `tick_snapshot_end` either way. Its default is false.

```rust
pub fn req_mkt_data_ex( &self, req_id: i64, contract: &Contract, generic_tick_list: &str, snapshot: bool, regulatory_snapshot: bool, mode_9887: i32, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `generic_tick_list` | `&str` | Comma-separated generic tick IDs (e.g. `"233"` for RT volume). |
| `snapshot` | `bool` | If `true`, delivers one quote then auto-cancels. |
| `regulatory_snapshot` | `bool` | If `true`, request a regulatory snapshot (additional fees may apply). |
| `mode_9887` | `i32` |  |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_mkt_data`

Cancel market data.

```rust
pub fn cancel_mkt_data(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_tick_by_tick_data`

Subscribe to every trade or every quote change on a contract. The feed rides the historical farm, registered there under the name `TickByTick` beside the five-second bars. No separate service is involved. A missing entitlement arrives as the venue's refusal rather than as silence. `number_of_ticks` and `ignore_size` are refused rather than dropped. This protocol's subscription states the contract and the kind of stream and nothing else: there is no field for a prelude of past ticks, and none for suppressing size-only changes. A caller that set either and was answered anyway would be reading a stream it did not ask for, with nothing to say so. Their defaults — no prelude, sizes included — are what the venue does, so an ordinary call is unaffected.

```rust
pub fn req_tick_by_tick_data( &self, req_id: i64, contract: &Contract, tick_type: &str, number_of_ticks: i32, ignore_size: bool, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `tick_type` | `&str` | Tick type ID or tick-by-tick type string. |
| `number_of_ticks` | `i32` | Maximum number of ticks to return. |
| `ignore_size` | `bool` | If `true`, ignore size in tick-by-tick data. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_tick_by_tick_data`

Cancel tick-by-tick data.

```rust
pub fn cancel_tick_by_tick_data(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_mkt_depth`

Subscribe to market depth (L2 order book). A contract that names no venue and no security type is sent as it stands. The engine reads an unnamed venue as the smart destination and checks a named security type against the venue's routing table. Substituting a stock here asks for a future's book as a stock's, which the venue refuses as a book it does not serve.

```rust
pub fn req_mkt_depth( &self, req_id: i64, contract: &Contract, num_rows: i32, is_smart_depth: bool, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `num_rows` | `i32` | Number of order book rows to subscribe to. |
| `is_smart_depth` | `bool` | If `true`, aggregate depth from multiple exchanges via SMART. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_mkt_depth`

Cancel market depth.

```rust
pub fn cancel_mkt_depth(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_real_time_bars`

Subscribe to real-time 5-second bars. `bar_size` is taken and not applied. The venue's real-time bar is five seconds and there is no field asking for another; the reference client takes the number and sends none either.

```rust
pub fn req_real_time_bars( &self, req_id: i64, contract: &Contract, _bar_size: i32, what_to_show: &str, use_rth: bool, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `bar_size` | `i32` | Bar size: `"1 min"`, `"5 mins"`, `"1 hour"`, `"1 day"`, etc. |
| `what_to_show` | `&str` | Data type: `"TRADES"`, `"MIDPOINT"`, `"BID"`, `"ASK"`, `"BID_ASK"`, etc. |
| `use_rth` | `bool` | If `true`, only return data from Regular Trading Hours. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_real_time_bars`

Cancel real-time bars.

```rust
pub fn cancel_real_time_bars(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_ping`

Request an auth-connection round-trip time sample: sends a lightweight liveness probe with no side effects on subscriptions, contract caches, or pacing budgets. The result lands asynchronously — poll `last_rtt()` after a moment. No-op while a probe is already in flight or the connection is down.

```rust
pub fn req_ping(&self) -> Result<(), Refusal>
```

**Returns:** `Result<(), Refusal>`

---

#### `last_rtt`

Last measured auth-connection round-trip time, if any. A gauge, not a benchmark: the sample is the interval from a probe to the first inbound traffic that followed it, which on an active feed can undercount by racing data already in flight. Also sampled automatically whenever liveness sends its own probe.

```rust
pub fn last_rtt(&self) -> Option<std::time::Duration>
```

**Returns:** `Option<std::time::Duration>`

---

#### `req_market_data_type`

Which feed the subscriptions after this one ask for: 1 live, 2 frozen, 3 delayed, 4 delayed and frozen. Sent with each subscription, in the field this protocol carries it in, and the `market_data_type` callback reports the type the subscription was made under. To state it for one request rather than for the ones that follow, `req_mkt_data_ex` takes it. A number naming no type leaves subscriptions realtime, and says so.

```rust
pub fn req_market_data_type(&self, market_data_type: i32)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `market_data_type` | `i32` | 1=live, 2=frozen, 3=delayed, 4=delayed-frozen. |

---

#### `set_news_providers`

Set news provider codes for per-contract news ticks.

```rust
pub fn set_news_providers(&self, providers: &str)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `providers` | `&str` | News provider list. |

---

#### `quote`

Zero-copy SeqLock quote read. Maps reqId → InstrumentId → SeqLock. Returns `None` if the reqId is not mapped to a subscription.

```rust
pub fn quote(&self, req_id: i64) -> Option<Quote>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Option<Quote>`

---

#### `quote_by_instrument`

Direct SeqLock read by InstrumentId (for callers who track IDs themselves). Returns `None` for an id outside the instrument table.

```rust
pub fn quote_by_instrument(&self, instrument: InstrumentId) -> Option<Quote>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `instrument` | `InstrumentId` | Instrument type for scanner (e.g. `"STK"`, `"FUT"`). |

**Returns:** `Option<Quote>`

---

## Reference Data

#### `req_historical_data`

Request historical data. With `keep_up_to_date`, the bar still forming is folded here from the stream the venue sends, and it opens on a whole multiple of its own length counted from the epoch. For every size up to an hour that is the clock boundary a caller expects. For `1 day` it is midnight UTC, which is the trading day of an instrument that trades around the clock and is the middle of the evening for one that does not — a US listing's forming daily bar opens in its after-hours session and spans two of them. Bars already closed are the venue's own and are not folded here.

```rust
pub fn req_historical_data( &self, req_id: i64, contract: &Contract, end_date_time: &str, duration: &str, bar_size: &str, what_to_show: &str, use_rth: bool, format_date: i32, keep_up_to_date: bool, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `end_date_time` | `&str` | End date/time in `"YYYYMMDD HH:MM:SS"` format, or empty for now. |
| `duration` | `&str` | Duration string, e.g. `"1 D"`, `"1 W"`, `"1 M"`, `"1 Y"`. |
| `bar_size` | `&str` | Bar size: `"1 min"`, `"5 mins"`, `"1 hour"`, `"1 day"`, etc. |
| `what_to_show` | `&str` | Data type: `"TRADES"`, `"MIDPOINT"`, `"BID"`, `"ASK"`, `"BID_ASK"`, etc. |
| `use_rth` | `bool` | If `true`, only return data from Regular Trading Hours. |
| `format_date` | `i32` | Date format: 1=`"YYYYMMDD HH:MM:SS"`, 2=Unix seconds. |
| `keep_up_to_date` | `bool` | If `true`, continue receiving updates after initial history. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_historical_data`

Cancel historical data.

```rust
pub fn cancel_historical_data(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_head_time_stamp`

Request head timestamp.

```rust
pub fn req_head_time_stamp( &self, req_id: i64, contract: &Contract, what_to_show: &str, use_rth: bool, format_date: i32, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `what_to_show` | `&str` | Data type: `"TRADES"`, `"MIDPOINT"`, `"BID"`, `"ASK"`, `"BID_ASK"`, etc. |
| `use_rth` | `bool` | If `true`, only return data from Regular Trading Hours. |
| `format_date` | `i32` | Date format: 1=`"YYYYMMDD HH:MM:SS"`, 2=Unix seconds. |

**Returns:** `Result<(), Refusal>`

---

#### `req_contract_details`

Request contract details.

```rust
pub fn req_contract_details(&self, req_id: i64, contract: &Contract) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |

**Returns:** `Result<(), Refusal>`

---

#### `req_mkt_depth_exchanges`

Request available exchanges for market depth.

```rust
pub fn req_mkt_depth_exchanges(&self) -> Result<(), Refusal>
```

**Returns:** `Result<(), Refusal>`

---

#### `req_matching_symbols`

Request matching symbols.

```rust
pub fn req_matching_symbols(&self, req_id: i64, pattern: &str) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `pattern` | `&str` | Symbol search pattern. |

**Returns:** `Result<(), Refusal>`

---

#### `req_wsh_meta_data`

Ask what event types the corporate-events calendar carries. Must be asked before events can be requested: an event request cannot be built until the event types are known.

```rust
pub fn req_wsh_meta_data(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_wsh_meta_data`

Stop waiting on the event types. The query is one message and one answer, so there is nothing at the venue to withdraw: what is withdrawn is the answer, which would otherwise reach a caller who has said they are done with it. A cancel naming no waiting request says so rather than returning as though it acted.

```rust
pub fn cancel_wsh_meta_data(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_wsh_event_data`

Stop waiting on the calendar's events. As above.

```rust
pub fn cancel_wsh_event_data(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_wsh_event_data`

Ask the corporate-events calendar for events. A caller either names a contract or writes its own filter. The filter goes to the venue as written: the venue validates it, and rewriting it here would change what was asked.

```rust
pub fn req_wsh_event_data( &self, req_id: i64, query: crate::types::CalendarQuery, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `query` | `crate::types::CalendarQuery` |  |

**Returns:** `Result<(), Refusal>`

---

#### `req_sec_def_opt_params`

Request option chain parameters. `fut_fop_exchange` names the venue for a futures option chain and is empty for an equity or index one.

```rust
pub fn req_sec_def_opt_params( &self, req_id: i64, underlying_symbol: &str, fut_fop_exchange: &str, underlying_sec_type: &str, underlying_con_id: i64, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `underlying_symbol` | `&str` | Underlying symbol (e.g. `"AAPL"`). |
| `fut_fop_exchange` | `&str` | Exchange for futures/FOP options. |
| `underlying_sec_type` | `&str` | Underlying security type (e.g. `"STK"`). |
| `underlying_con_id` | `i64` | Underlying contract ID. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_head_time_stamp`

Cancel head timestamp request.

```rust
pub fn cancel_head_time_stamp(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_market_rule`

The price increments a market rule states. A rule is not asked for on its own: the venue sends the rules a contract uses along with that contract's details. So this answers from what those have already brought in, and says so when the rule is not among them rather than returning in silence.

```rust
pub fn req_market_rule(&self, market_rule_id: i32, wrapper: &mut impl crate::api::wrapper::Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `market_rule_id` | `i32` | Market rule ID. |
| `wrapper` | `&mut impl crate::api::wrapper::Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_news_bulletins`

Subscribe to news bulletins. `all_msgs` asks for the day's bulletins as well as the ones still to come. The subscription carries no field asking the venue for them, but the venue has been broadcasting them at this session since it opened and they are still queued, so a caller asking for every message of the day is answered from those. Asking only for what follows drops them, which is what stopped a subscription from opening with bulletins published before anyone asked for any.

```rust
pub fn req_news_bulletins(&self, all_msgs: bool)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `all_msgs` | `bool` | If `true`, receive all existing bulletins on subscribe. |

---

#### `cancel_news_bulletins`

Cancel news bulletin subscription.

```rust
pub fn cancel_news_bulletins(&self)
```

---

#### `req_scanner_parameters`

Request scanner parameters XML.

```rust
pub fn req_scanner_parameters(&self) -> Result<(), Refusal>
```

**Returns:** `Result<(), Refusal>`

---

#### `req_scanner_subscription`

Subscribe to a market scanner. `filters` are the scanner filter tags named by `req_scanner_parameters`, e.g. `priceAbove` = `"10"` or `stkTypes` = `"inc:ETF"`.

```rust
pub fn req_scanner_subscription( &self, req_id: i64, instrument: &str, location_code: &str, scan_code: &str, max_items: u32, filters: &[TagValue], ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `instrument` | `&str` | Instrument type for scanner (e.g. `"STK"`, `"FUT"`). |
| `location_code` | `&str` | Scanner location (e.g. `"STK.US.MAJOR"`). |
| `scan_code` | `&str` | Scanner code (e.g. `"TOP_PERC_GAIN"`, `"HIGH_OPT_IMP_VOLAT"`). |
| `max_items` | `u32` | Maximum number of scanner results. |
| `filters` | `&[TagValue]` | Scanner filter tags from `req_scanner_parameters`, e.g. `priceAbove` = `"10"`. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_scanner_subscription`

Cancel a scanner subscription.

```rust
pub fn cancel_scanner_subscription(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_historical_news`

Request historical news headlines. `start_time` and `end_time` are refused rather than taken and dropped: the query this client sends carries no time bounds, and `max_results` is what limits the answer. No more than three hundred are asked for however many are wanted. The reference client caps it there before the request goes out, so a bigger number is one the venue is never asked.

```rust
pub fn req_historical_news( &self, req_id: i64, con_id: i64, provider_codes: &str, start_time: &str, end_time: &str, max_results: u32, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `con_id` | `i64` | Contract ID. Unique per instrument. |
| `provider_codes` | `&str` | Pipe-separated news provider codes. |
| `start_time` | `&str` | Start date/time for news query. |
| `end_time` | `&str` | End date/time for news query. |
| `max_results` | `u32` | Maximum number of results. |

**Returns:** `Result<(), Refusal>`

---

#### `req_news_article`

Request a news article by provider and article ID.

```rust
pub fn req_news_article(&self, req_id: i64, provider_code: &str, article_id: &str) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `provider_code` | `&str` | News provider code (e.g. `"BRFG"`). |
| `article_id` | `&str` | News article identifier. |

**Returns:** `Result<(), Refusal>`

---

#### `req_adjustments`

Ask for a contract's corporate actions over a range of days. The answer is filed against the contract it names rather than handed to a callback under this id, because the venue answers per contract: `EClient::adjustments` reads it once it has arrived, and `corporate_actions` asks and waits in one call. `start_date` and `end_date` are days, as `YYYYMMDD`.

```rust
pub fn req_adjustments( &self, req_id: i64, con_id: i64, sec_type: &str, exchange: &str, start_date: &str, end_date: &str, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `con_id` | `i64` | Contract ID. Unique per instrument. |
| `sec_type` | `&str` |  |
| `exchange` | `&str` | Exchange name. |
| `start_date` | `&str` |  |
| `end_date` | `&str` |  |

**Returns:** `Result<(), Refusal>`

---

#### `req_fundamental_data`

Request fundamental data. Three reports, which are the three the venue states: `ReportSnapshot`, `RESC` for what analysts expect, and `CalendarReport` for what the issuer has coming. The contract is named by its venue id and nothing else of it is carried, so pass one that has an id: from `qualify_contract`, or from any contract-details answer. A description is refused rather than sent as a request about contract zero.

```rust
pub fn req_fundamental_data(&self, req_id: i64, contract: &Contract, report_type: &str) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `report_type` | `&str` | Report type: `"ReportSnapshot"`, `"ReportsFinSummary"`, `"RESC"`, etc. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_fundamental_data`

Cancel fundamental data.

```rust
pub fn cancel_fundamental_data(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_histogram_data`

Request price histogram data. Named by its venue id, as `req_fundamental_data` is.

```rust
pub fn req_histogram_data(&self, req_id: i64, contract: &Contract, use_rth: bool, period: &str) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `use_rth` | `bool` | If `true`, only return data from Regular Trading Hours. |
| `period` | `&str` | Histogram period, e.g. `"1week"`, `"1month"`. |

**Returns:** `Result<(), Refusal>`

---

#### `cancel_histogram_data`

Cancel histogram data.

```rust
pub fn cancel_histogram_data(&self, req_id: i64) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

**Returns:** `Result<(), Refusal>`

---

#### `req_historical_ticks`

Request historical tick data. Named from one end and counted from there: give `start_date_time` for the ticks after a moment or `end_date_time` for the ones before it, and `number_of_ticks` says how far it reaches. Naming both, or neither, is what the venue refuses.

```rust
pub fn req_historical_ticks( &self, req_id: i64, contract: &Contract, start_date_time: &str, end_date_time: &str, number_of_ticks: i32, what_to_show: &str, use_rth: bool, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `start_date_time` | `&str` | Start date/time for tick query. |
| `end_date_time` | `&str` | End date/time in `"YYYYMMDD HH:MM:SS"` format, or empty for now. |
| `number_of_ticks` | `i32` | Maximum number of ticks to return. |
| `what_to_show` | `&str` | Data type: `"TRADES"`, `"MIDPOINT"`, `"BID"`, `"ASK"`, `"BID_ASK"`, etc. |
| `use_rth` | `bool` | If `true`, only return data from Regular Trading Hours. |

**Returns:** `Result<(), Refusal>`

---

#### `req_historical_schedule`

Request historical trading schedule.

```rust
pub fn req_historical_schedule( &self, req_id: i64, contract: &Contract, end_date_time: &str, duration: &str, use_rth: bool, ) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `end_date_time` | `&str` | End date/time in `"YYYYMMDD HH:MM:SS"` format, or empty for now. |
| `duration` | `&str` | Duration string, e.g. `"1 D"`, `"1 W"`, `"1 M"`, `"1 Y"`. |
| `use_rth` | `bool` | If `true`, only return data from Regular Trading Hours. |

**Returns:** `Result<(), Refusal>`

---

## Gateway-Local & Stubs

#### `req_smart_components`

Request smart routing components for a BBO exchange. Gateway-local — returns component exchanges from init data. `bbo_exchange` is taken and not applied. The venue states one table of routing components at logon, for this session rather than per exchange, and that whole table is what comes back.

```rust
pub fn req_smart_components(&self, req_id: i64, _bbo_exchange: &str, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `bbo_exchange` | `&str` | BBO exchange for smart component lookup (e.g. `"SMART"`). |
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_news_providers`

Request available news providers. Gateway-local — returns provider list from init data.

```rust
pub fn req_news_providers(&self, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_current_time`

The venue's clock, as `reqCurrentTime` reports it. Every message the venue sends is stamped with the time it sent it, and the last one is held. A caller asking for the server's time is asking how far apart the two clocks are, which this machine's own clock cannot answer. Where no message has been stamped yet — before the session is up — there is nothing to report but the local clock.

```rust
pub fn req_current_time(&self, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_current_time_in_millis`

The venue's clock in milliseconds, as `reqCurrentTimeInMillis` reports it. The same clock `req_current_time` reports and read the same way — the venue's own last stamp, falling back to this machine only before the session has been stamped at all. What differs is the precision kept: the venue sometimes stamps a fraction of a second, and asking in seconds throws it away. A stamp with no fraction lands on a whole second. That is the precision the venue stated, not a rounding of something finer — and it is what a session measured against this venue actually gets, because the stamps seen here carried no fraction at all. The call reads one where the venue states one; it does not manufacture precision where it does not.

```rust
pub fn req_current_time_in_millis(&self, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `request_fa`

Ask the venue for a partition of the advisor's own configuration. The reference client names the partition by a number — its aliases, its groups, its allocation profiles — and the venue names it by a word, so the number is turned into the word it stands for. A number that stands for nothing is refused rather than sent as an empty partition. The request reaches the venue; its answer is not read back yet, so [`Wrapper::receive_fa`] does not fire. What the venue replies with lands among the messages this client records as unread. Reading it needs an advisor account to state the reply's shape, and inventing one would be a guess about a frame nobody here has seen.

```rust
pub fn request_fa(&self, fa_data_type: i32) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `fa_data_type` | `i32` | FA data type (1=Groups, 2=Profiles, 3=Aliases). |

**Returns:** `Result<(), Refusal>`

---

#### `replace_fa`

Replace a partition of the advisor's configuration with the one given. As with `request_fa`, the replacement reaches the venue and its answer is not read back, so [`Wrapper::replace_fa_end`] does not fire.

```rust
pub fn replace_fa(&self, fa_data_type: i32, cxml: &str) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `fa_data_type` | `i32` | FA data type (1=Groups, 2=Profiles, 3=Aliases). |
| `cxml` | `&str` | FA XML configuration data. |

**Returns:** `Result<(), Refusal>`

---

#### `calculate_implied_volatility`

What volatility a price implies, under the venue's model. This protocol carries no request for it, so the value is computed here, anchored to the venue's last stated model output for this contract. Where the venue has stated no model, nothing is answered rather than a number derived from an unstated rate. Answered over a year, which is the scale `tick_option_computation` reports the venue's own volatility on, so the two read against each other.

```rust
pub fn calculate_implied_volatility( &self, req_id: i64, contract: &super::Contract, option_price: f64, under_price: f64, )
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&super::Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `option_price` | `f64` | Option market price. |
| `under_price` | `f64` | Underlying asset price. |

---

#### `calculate_option_price`

What price a volatility implies, under that same model.

```rust
pub fn calculate_option_price( &self, req_id: i64, contract: &super::Contract, volatility: f64, under_price: f64, )
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&super::Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `volatility` | `f64` | Implied volatility. |
| `under_price` | `f64` | Underlying asset price. |

---

#### `cancel_calculate_implied_volatility`

Withdraw a question that was waiting on the venue to state a model. A question answered from a model already stated started nothing and stops nothing. One that opened a watch to get an answer withdraws it here, so a caller that changes its mind is not left watching a contract it no longer asks about.

```rust
pub fn cancel_calculate_implied_volatility(&self, req_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `cancel_calculate_option_price`

As for `cancel_calculate_implied_volatility`.

```rust
pub fn cancel_calculate_option_price(&self, req_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `query_display_groups`

Query display groups. Not yet implemented. The display groups on offer. Answered on `display_group_list`. A display group is a way for several callers on one session to agree on a contract. The venue knows nothing about them, and never did: the vendor's own client keeps them in its own state and serves them to its callers from there, which is exactly what this does.

```rust
pub fn query_display_groups(&self, req_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `subscribe_to_group_events`

Follow a display group. Answered on `display_group_updated`, at once with what the group holds and again whenever it changes.

```rust
pub fn subscribe_to_group_events(&self, req_id: i64, group_id: i32)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `group_id` | `i32` | Display group ID. |

---

#### `unsubscribe_from_group_events`

Stop following a display group.

```rust
pub fn unsubscribe_from_group_events(&self, req_id: i64)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `update_display_group`

Put a contract in the group this request follows, stated as `conId@exchange`, or `none` to empty it. Every follower of that group is told, including this one.

```rust
pub fn update_display_group(&self, req_id: i64, contract_info: &str) -> Result<(), Refusal>
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract_info` | `&str` | Display group contract info string. |

**Returns:** `Result<(), Refusal>`

---

#### `req_soft_dollar_tiers`

Request soft dollar tiers. Gateway-local — returns tiers parsed from CCP logon tag 6560.

```rust
pub fn req_soft_dollar_tiers(&self, req_id: i64, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `req_family_codes`

Request family codes. Gateway-local — returns codes parsed from CCP logon tag 6823.

```rust
pub fn req_family_codes(&self, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

#### `set_server_log_level`

Set server log level. Taken and not applied. The session holds no log level of its own and this protocol carries no message asking the venue to change one, so what a caller states here is written to this client's log and nothing else. This client's own logging is set where the process sets it, through `IBX_LOG_LEVEL` or `RUST_LOG`.

```rust
pub fn set_server_log_level(&self, log_level: i32)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `log_level` | `i32` | Log level: 1=error, 2=warn, 3=info, 4=debug, 5=trace. |

---

#### `req_user_info`

Request user info. Gateway-local — returns whiteBrandingId from CCP logon.

```rust
pub fn req_user_info(&self, req_id: i64, wrapper: &mut impl Wrapper)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `wrapper` | `&mut impl Wrapper` | Wrapper callback receiver for synchronous delivery. |

---

## Wrapper Callbacks

#### `connect_ack`

The session is open. Nothing has been asked for yet.

---

#### `connection_closed`

The session is over, because this client ended it. A session that went away instead is reported on `error` under 1100.

---

#### `next_valid_id`

The first order id this session may use. Each order needs one higher than the last.

| Parameter | Type | Description |
|-----------|------|-------------|
| `order_id` | `i64` | Order identifier. Must be unique per session. |

---

#### `managed_accounts`

Every account this login may act for, separated by commas. One for most logins; an advisor has several.

| Parameter | Type | Description |
|-----------|------|-------------|
| `accounts_list` | `&str` | Comma-separated account IDs. |

---

#### `error`

What the venue said about a request, under the number it says it with. Codes from 2100 to 2200 are notices about a connection rather than failures. `req_id` is -1 for anything that answers no particular request.  A request this client will not send is reported here too, under the same numbers the reference client uses: 321 for a request that fails validation, 200 for a contract description that matches nothing, 504 for a call made with no session.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `error_code` | `i64` | Error code. |
| `error_string` | `&str` | Error message. |
| `advanced_order_reject_json` | `&str` | JSON with advanced rejection details. |

---

#### `current_time`

The venue's clock, in seconds since the epoch.

| Parameter | Type | Description |
|-----------|------|-------------|
| `time` | `i64` | Tick timestamp (Unix seconds). |

---

#### `current_time_in_millis`

The venue's clock, in milliseconds since the epoch.  The same clock [`current_time`](Self::current_time) reports, at the precision the venue stated it in. The stamp can carry a fraction of a second and this reads it where it does — but on the sessions measured here the venue stated none, so the answer lands on a whole second and is the other call's thousandfold. Read the precision off the number rather than assuming this one has more of it.

| Parameter | Type | Description |
|-----------|------|-------------|
| `time_in_millis` | `i64` |  |

---

#### `tick_price`

One price of a quote, and which price it is. `tick_type` names it — 1 bid, 2 ask, 4 last, 9 close — and `attrib` says whether it can be traded against and whether it is past its limit. A size arrives on `tick_size` under the type that belongs to it.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `tick_type` | `i32` | Tick type ID or tick-by-tick type string. |
| `price` | `f64` | Tick price. |
| `attrib` | `&TickAttrib` | Tick attributes. |

---

#### `tick_size`

One size of a quote, and which size it is: 0 bid, 3 ask, 5 last, 8 the day's volume.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `tick_type` | `i32` | Tick type ID or tick-by-tick type string. |
| `size` | `f64` | Tick size. |

---

#### `tick_string`

A quote's value that is not a number — a timestamp, an exchange map, a set of ids.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `tick_type` | `i32` | Tick type ID or tick-by-tick type string. |
| `value` | `&str` | Account value. |

---

#### `tick_generic`

A quote's value that is a number and is not a price or a size — an implied volatility, an index future's premium, a halt.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `tick_type` | `i32` | Tick type ID or tick-by-tick type string. |
| `value` | `f64` | Account value. |

---

#### `tick_snapshot_end`

A snapshot has stated everything it is going to. Only for a subscription asked for as a snapshot; a streaming one never ends.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `market_data_type`

Which feed a subscription is being served from: 1 live, 2 frozen, 3 delayed, 4 delayed and frozen.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `market_data_type` | `i32` | 1=live, 2=frozen, 3=delayed, 4=delayed-frozen. |

---

#### `order_status`

Where an order stands now. Fires on every change, and again on each fill. `filled` and `remaining` are shares, `avg_fill_price` the average of what has filled so far.

| Parameter | Type | Description |
|-----------|------|-------------|
| `order_id` | `i64` | Order identifier. Must be unique per session. |
| `status` | `&str` | Order status string (`"Submitted"`, `"Filled"`, `"Cancelled"`, etc.). |
| `filled` | `f64` | Cumulative filled quantity. |
| `remaining` | `f64` | Remaining quantity. |
| `avg_fill_price` | `f64` | Average fill price. |
| `perm_id` | `i64` | Permanent order ID assigned by the server. |
| `parent_id` | `i64` | Parent order ID (0 if no parent). |
| `last_fill_price` | `f64` | Price of the last fill. |
| `client_id` | `i64` | Client ID (unused — single-client engine). |
| `why_held` | `&str` | Reason the order is held (e.g. `"locate"`). |
| `mkt_cap_price` | `f64` | Market cap price for the order. |

---

#### `open_order`

An order as the venue holds it, and the state it is in. Fires beside every `order_status`, when open orders are asked for, and once for a preview — where the state carries what the order would cost and no status follows, because a preview is not an order.

| Parameter | Type | Description |
|-----------|------|-------------|
| `order_id` | `i64` | Order identifier. Must be unique per session. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `order` | `&Order` | Order parameters (action, quantity, type, price, TIF, etc.). |
| `order_state` | `&OrderState` | Order state (status, margin, commission info). |

---

#### `open_order_end`

Every open order has been stated.

---

#### `exec_details`

One fill, against the order and contract it filled. What it cost arrives separately, on `commission_and_fees_report`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `execution` | `&Execution` | Execution details (exec_id, time, price, shares, etc.). |

---

#### `exec_details_end`

Every execution answering this request has been stated.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `commission_and_fees_report`

What a fill cost, matched to it by execution id.

| Parameter | Type | Description |
|-----------|------|-------------|
| `report` | `&CommissionAndFeesReport` | Commission report (exec_id, commission, currency, realized P&L). |

---

#### `update_account_value`

One figure the venue states about an account, in the currency it states it in. An account is stated in several currencies at once, so the same key arrives more than once.

| Parameter | Type | Description |
|-----------|------|-------------|
| `key` | `&str` | Account value key (e.g. `"NetLiquidation"`, `"BuyingPower"`). |
| `value` | `&str` | Account value. |
| `currency` | `&str` | Currency code (e.g. `"USD"`). |
| `account_name` | `&str` | Account identifier. |

---

#### `update_portfolio`

One position, as the venue values it now.

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `position` | `f64` | Book position (row index) or position size. |
| `market_price` | `f64` | Current market price. |
| `market_value` | `f64` | Current market value of position. |
| `average_cost` | `f64` | Average cost basis. |
| `unrealized_pnl` | `f64` | Unrealized profit/loss. |
| `realized_pnl` | `f64` | Realized profit/loss. |
| `account_name` | `&str` | Account identifier. |

---

#### `update_account_time`

When the account figures above were last stated.

| Parameter | Type | Description |
|-----------|------|-------------|
| `timestamp` | `&str` | Timestamp string. |

---

#### `account_download_end`

The account has been fully stated. Fires once the venue has stopped adding to it, not on the first figure.

| Parameter | Type | Description |
|-----------|------|-------------|
| `account` | `&str` | Account ID. |

---

#### `account_summary`

One figure answering `req_account_summary`, in the currency the venue states it in.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `account` | `&str` | Account ID. |
| `tag` | `&str` | Account tag name (e.g. `"NetLiquidation"`). |
| `value` | `&str` | Account value. |
| `currency` | `&str` | Currency code (e.g. `"USD"`). |

---

#### `account_summary_end`

Every figure answering this request has been stated.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `position`

One position held, on any account this login may act for.

| Parameter | Type | Description |
|-----------|------|-------------|
| `account` | `&str` | Account ID. |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `pos` | `f64` | Position size (decimal shares). |
| `avg_cost` | `f64` | Average cost per share. |

---

#### `position_end`

Every position has been stated.

---

#### `position_multi`

A holding, answering `req_positions_multi`. Separate from `position`: a caller asks per account or model and is answered per request.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `account` | `&str` | Account ID. |
| `model_code` | `&str` | Model portfolio code (empty for default). |
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `pos` | `f64` | Position size (decimal shares). |
| `avg_cost` | `f64` | Average cost per share. |

---

#### `position_multi_end`

Every position answering this request has been stated.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `account_update_multi`

An account value, answering `req_account_updates_multi`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `account` | `&str` | Account ID. |
| `model_code` | `&str` | Model portfolio code (empty for default). |
| `key` | `&str` | Account value key (e.g. `"NetLiquidation"`, `"BuyingPower"`). |
| `value` | `&str` | Account value. |
| `currency` | `&str` | Currency code (e.g. `"USD"`). |

---

#### `account_update_multi_end`

Every figure answering this request has been stated.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `pnl`

An account's running profit: today's, what is unrealised, and what has been realised.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `daily_pnl` | `f64` | Daily profit/loss. |
| `unrealized_pnl` | `f64` | Unrealized profit/loss. |
| `realized_pnl` | `f64` | Realized profit/loss. |

---

#### `pnl_single`

The same for one position, with the size held.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `pos` | `f64` | Position size (decimal shares). |
| `daily_pnl` | `f64` | Daily profit/loss. |
| `unrealized_pnl` | `f64` | Unrealized profit/loss. |
| `realized_pnl` | `f64` | Realized profit/loss. |
| `value` | `f64` | Account value. |

---

#### `historical_data`

One bar answering a historical request. `bar.date` is a day for a daily bar and a moment for anything shorter, in the zone the bar carries.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `bar` | `&BarData` | Bar data (date, open, high, low, close, volume, wap, bar_count). |

---

#### `historical_data_end`

Every bar answering this request has been stated, and the window they cover.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `start` | `&str` | Period start date/time. |
| `end` | `&str` | Period end date/time. |

---

#### `historical_data_update`

A bar that continues a `keep_up_to_date` request, after its first batch completed. The bar still forming is restated as it changes.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `bar` | `&BarData` | Bar data (date, open, high, low, close, volume, wap, bar_count). |

---

#### `head_timestamp`

The earliest moment the venue holds data for a contract.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `head_timestamp` | `&str` | Earliest available data timestamp string. |

---

#### `contract_details`

One contract matching a description, with everything the venue states about it. A description can match more than one.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `details` | `&ContractDetails` | Contract details object. |

---

#### `contract_details_end`

Every contract matching this request has been stated.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `symbol_samples`

Contracts whose symbol or name matches a pattern, across venues.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `descriptions` | `&[ContractDescription]` | Array of matching contract descriptions. |

---

#### `tick_by_tick_all_last`

One trade, as it happens. `tick_attrib_last` says whether it was past a limit and whether it goes unreported to the tape.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `tick_type` | `i32` | Tick type ID or tick-by-tick type string. |
| `time` | `i64` | Tick timestamp (Unix seconds). |
| `price` | `f64` | Tick price. |
| `size` | `f64` | Tick size. |
| `attrib` | `&TickAttribLast` | Tick attributes. |
| `exchange` | `&str` | Exchange name. |
| `special_conditions` | `&str` | Special trade conditions. |

---

#### `tick_by_tick_bid_ask`

One change to the top of the book, as it happens.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `time` | `i64` | Tick timestamp (Unix seconds). |
| `bid_price` | `f64` | Bid price. |
| `ask_price` | `f64` | Ask price. |
| `bid_size` | `f64` | Bid size. |
| `ask_size` | `f64` | Ask size. |
| `attrib` | `&TickAttribBidAsk` | Tick attributes. |

---

#### `tick_by_tick_mid_point`

One change to the midpoint, as it happens.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `time` | `i64` | Tick timestamp (Unix seconds). |
| `mid_point` | `f64` | Midpoint price. |

---

#### `scanner_data`

One row of a scan, in rank order.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `rank` | `i32` | Scanner result rank (0-based). |
| `details` | `&ContractDetails` | Contract details object. |
| `distance` | `&str` | Scanner distance metric. |
| `benchmark` | `&str` | Scanner benchmark. |
| `projection` | `&str` | Scanner projection. |
| `legs_str` | `&str` | Combo legs description. |

---

#### `scanner_data_end`

Every row of this scan has been stated.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `scanner_parameters`

Every scan the venue offers and what each can be filtered by, as the XML the venue publishes.

| Parameter | Type | Description |
|-----------|------|-------------|
| `xml` | `&str` | XML string. |

---

#### `update_news_bulletin`

A notice the venue broadcasts to everyone — an exchange unavailable, a system message.

| Parameter | Type | Description |
|-----------|------|-------------|
| `msg_id` | `i64` | Bulletin message ID. |
| `msg_type` | `i32` | Bulletin message type (1=regular, 2=exchange). |
| `message` | `&str` | Bulletin message text. |
| `orig_exchange` | `&str` | Originating exchange. |

---

#### `tick_news`

A headline about a contract being watched, as it is published.

| Parameter | Type | Description |
|-----------|------|-------------|
| `ticker_id` | `i64` | Ticker/request ID. |
| `timestamp` | `i64` | Timestamp string. |
| `provider_code` | `&str` | News provider code (e.g. `"BRFG"`). |
| `article_id` | `&str` | News article identifier. |
| `headline` | `&str` | News headline text. |
| `extra_data` | `&str` | Additional tick data. |

---

#### `historical_news`

One headline from the archive.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `time` | `&str` | Tick timestamp (Unix seconds). |
| `provider_code` | `&str` | News provider code (e.g. `"BRFG"`). |
| `article_id` | `&str` | News article identifier. |
| `headline` | `&str` | News headline text. |

---

#### `historical_news_end`

Every headline answering this request has been stated, and whether the archive holds more.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `has_more` | `bool` | If `true`, more results available. |

---

#### `news_article`

The body of one article. `article_type` is 0 for text and 1 for a binary document.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `article_type` | `i32` | Article type: 0=plain text, 1=HTML. |
| `article_text` | `&str` | Full article body. |

---

#### `real_time_bar`

One five-second bar of a live stream.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `date` | `i64` | Bar date string. |
| `open` | `f64` | Open price. |
| `high` | `f64` | High price. |
| `low` | `f64` | Low price. |
| `close` | `f64` | Close price. |
| `volume` | `f64` | Volume. |
| `wap` | `f64` | Volume-weighted average price. |
| `count` | `i32` | Trade count. |

---

#### `historical_ticks`

Historical midpoints, in batches, until `done`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `ticks` | `&HistoricalTickData` | Historical tick data. |
| `done` | `bool` | If `true`, all ticks have been delivered. |

---

#### `historical_ticks_bid_ask`

Historical quotes, in batches, until `done`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `ticks` | `&HistoricalTickData` | Historical tick data. |
| `done` | `bool` | If `true`, all ticks have been delivered. |

---

#### `historical_ticks_last`

Historical trades, in batches, until `done`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `ticks` | `&HistoricalTickData` | Historical tick data. |
| `done` | `bool` | If `true`, all ticks have been delivered. |

---

#### `tick_option_computation`

The venue's model for an option: the volatility its price implies, the greeks, and what the model says the option and its underlying are worth.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `tick_type` | `i32` | Tick type ID or tick-by-tick type string. |
| `tick_attrib` | `i32` |  |
| `implied_vol` | `f64` | Implied volatility. |
| `delta` | `f64` | Option delta. |
| `opt_price` | `f64` | Option theoretical price. |
| `pv_dividend` | `f64` | Present value of dividends. |
| `gamma` | `f64` | Option gamma. |
| `vega` | `f64` | Option vega. |
| `theta` | `f64` | Option theta. |
| `und_price` | `f64` | Underlying price. |

---

#### `display_group_list`

The display groups this client offers, `|`-separated.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `groups` | `&str` | FA group definitions. |

---

#### `display_group_updated`

The contract a display group now holds, as `conId@exchange`, or `none`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `contract_info` | `&str` | Display group contract info string. |

---

#### `bond_contract_details`

A bond's contract details, answering `req_contract_details` for a bond. The venue answers bonds on the same callback as everything else here, so this exists for callers written against a client that separates them.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `details` | `&ContractDetails` | Contract details object. |

---

#### `order_bound`

The permanent id an order was given, paired with the id this client used.

| Parameter | Type | Description |
|-----------|------|-------------|
| `perm_id` | `i64` | Permanent order ID assigned by the server. |
| `client_id` | `i64` | Client ID (unused — single-client engine). |
| `order_id` | `i64` | Order identifier. Must be unique per session. |

---

#### `receive_fa`

An advisor's allocation groups, profiles or aliases, as XML.

| Parameter | Type | Description |
|-----------|------|-------------|
| `fa_data_type` | `i32` | FA data type (1=Groups, 2=Profiles, 3=Aliases). |
| `cxml` | `&str` | FA XML configuration data. |

---

#### `replace_fa_end`

The end of a `replace_fa` exchange.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `text` | `&str` | Informational text. |

---

#### `wsh_meta_data`

What the event calendar can answer about, as JSON.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `data_json` | `&str` |  |

---

#### `wsh_event_data`

Calendar events, as JSON.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `data_json` | `&str` |  |

---

#### `security_definition_option_parameter`

One venue's option chain for an underlying: the expiries and strikes it lists.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `exchange` | `&str` | Exchange name. |
| `underlying_con_id` | `i64` | Underlying contract ID. |
| `trading_class` | `&str` | Trading class. |
| `multiplier` | `&str` | Contract multiplier. |
| `expirations` | `&[String]` | Available expiration dates. |
| `strikes` | `&[f64]` | Available strike prices. |

---

#### `security_definition_option_parameter_end`

Every venue's chain has been stated.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |

---

#### `delta_neutral_validation`

The contract the venue paired with a delta-neutral order.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `con_id` | `i64` | Contract ID. Unique per instrument. |
| `delta` | `f64` | Option delta. |
| `price` | `f64` | Tick price. |

---

#### `histogram_data`

How much traded at each price over a window.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `items` | `&[(f64, i64` | Histogram entries `[(price, count)]`. |

---

#### `market_rule`

The price ladder a contract trades on: each step, and what the price moves in above it.

| Parameter | Type | Description |
|-----------|------|-------------|
| `market_rule_id` | `i64` | Market rule ID. |
| `price_increments` | `&[PriceIncrement]` | Price increment rules `[{low_edge, increment}]`. |

---

#### `completed_order`

An order that is done — filled, cancelled or expired — as the venue holds it.

| Parameter | Type | Description |
|-----------|------|-------------|
| `contract` | `&Contract` | Contract specification (symbol, secType, exchange, currency, etc.). |
| `order` | `&Order` | Order parameters (action, quantity, type, price, TIF, etc.). |
| `order_state` | `&OrderState` | Order state (status, margin, commission info). |

---

#### `completed_orders_end`

Every completed order has been stated.

---

#### `historical_schedule`

When a contract's venue was open over a window, session by session, in the zone the venue keeps.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `start_date_time` | `&str` | Start date/time for tick query. |
| `end_date_time` | `&str` | End date/time in `"YYYYMMDD HH:MM:SS"` format, or empty for now. |
| `time_zone` | `&str` | Timezone string (e.g. `"US/Eastern"`). |
| `sessions` | `&[(String, String, String` | Trading sessions `[(ref_date, open, close)]`. |

---

#### `fundamental_data`

A fundamental report, as the XML the venue publishes.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `data` | `&str` | Raw data string (XML/JSON). |

---

#### `update_mkt_depth`

One level of a book that names no venue. `operation` is 0 to insert, 1 to update, 2 to delete; `side` is 0 ask, 1 bid.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `position` | `i32` | Book position (row index) or position size. |
| `operation` | `i32` | Book operation: 0=insert, 1=update, 2=delete. |
| `side` | `i32` | Book side: 0=ask, 1=bid. Or order side `"BOT"`/`"SLD"`. |
| `price` | `f64` | Tick price. |
| `size` | `f64` | Tick size. |

---

#### `update_mkt_depth_l2`

One level of a book that names the venue it stands on. Every level from this client names one.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `position` | `i32` | Book position (row index) or position size. |
| `market_maker` | `&str` | Market maker ID. |
| `operation` | `i32` | Book operation: 0=insert, 1=update, 2=delete. |
| `side` | `i32` | Book side: 0=ask, 1=bid. Or order side `"BOT"`/`"SLD"`. |
| `price` | `f64` | Tick price. |
| `size` | `f64` | Tick size. |
| `is_smart_depth` | `bool` | If `true`, aggregate depth from multiple exchanges via SMART. |

---

#### `mkt_depth_exchanges`

Every exchange the venue names, in the two sections it names them in: shares and derivatives.

| Parameter | Type | Description |
|-----------|------|-------------|
| `descriptions` | `&[crate::types::DepthMktDataDescription]` | Array of matching contract descriptions. |

---

#### `tick_req_params`

What a subscription was given: the increment its prices move in, which venues it is served from, and which feed answered.

| Parameter | Type | Description |
|-----------|------|-------------|
| `ticker_id` | `i64` | Ticker/request ID. |
| `min_tick` | `f64` | Minimum tick size. |
| `bbo_exchange` | `&str` | BBO exchange for smart component lookup (e.g. `"SMART"`). |
| `snapshot_permissions` | `i64` | Snapshot permissions bitmask. |

---

#### `smart_components`

Which venue each bit of a quote's exchange mask refers to, and the letter that venue is named by.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `components` | `&[crate::types::SmartComponent]` | Smart routing component exchanges. |

---

#### `news_providers`

Every news provider this account may read.

| Parameter | Type | Description |
|-----------|------|-------------|
| `providers` | `&[crate::types::NewsProvider]` | News provider list. |

---

#### `soft_dollar_tiers`

The soft dollar tiers this account may direct commission to.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `tiers` | `&[crate::types::SoftDollarTier]` | Soft dollar tier list. |

---

#### `family_codes`

The account families this login belongs to.

| Parameter | Type | Description |
|-----------|------|-------------|
| `codes` | `&[crate::types::FamilyCode]` | Family code list. |

---

#### `user_info`

What the login is entitled to, as the venue states it.

| Parameter | Type | Description |
|-----------|------|-------------|
| `req_id` | `i64` | Request identifier. Used to match responses to requests. |
| `white_branding_id` | `&str` | White branding ID (empty for standard accounts). |

---
