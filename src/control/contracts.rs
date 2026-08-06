//! Contract/security definition lookups via the auth connection.
//!
//! Key tag mappings: STK→CS (SecurityType), SMART→BEST (Exchange).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::protocol::fix::{self, TAG_MSG_TYPE};

// Tags for security definitions
pub const TAG_SECURITY_REQ_ID: u32 = 320;
pub const TAG_SECURITY_REQ_TYPE: u32 = 321;
pub const TAG_SECURITY_RESPONSE_TYPE: u32 = 323;
pub const TAG_SYMBOL: u32 = 55;
pub const TAG_SECURITY_TYPE: u32 = 167;
pub const TAG_EXCHANGE: u32 = 100;
pub const TAG_CURRENCY: u32 = 15;
pub const TAG_LAST_TRADE_DATE: u32 = 200;
/// MaturityDate. Carries a full expiry date where 200 carries a contract
/// month, so a definition that states one states it here.
pub const TAG_MATURITY_DATE: u32 = 541;
pub const TAG_RIGHT: u32 = 201;
pub const TAG_STRIKE: u32 = 202;
pub const TAG_SECURITY_EXCHANGE: u32 = 207;
pub const TAG_MULTIPLIER: u32 = 231;
pub const TAG_LONG_NAME: u32 = 306;
pub const TAG_SECURITY_ID: u32 = 455;
pub const TAG_SECURITY_ID_SOURCE: u32 = 456;

// IB custom tags
pub const TAG_IB_CON_ID: u32 = 6008;
pub const TAG_IB_LOCAL_SYMBOL: u32 = 6035;
pub const TAG_IB_VALID_EXCHANGES: u32 = 6046;
pub const TAG_IB_TRADING_CLASS: u32 = 6058;
pub const TAG_IB_SOURCE: u32 = 6088;
pub const TAG_IB_PRIMARY_EXCHANGE: u32 = 6470;
pub const TAG_IB_ORDER_TYPES: u32 = 6431;
pub const TAG_IB_MARKET_RULE_ID: u32 = 6031;
pub const TAG_IB_STOCK_TYPE: u32 = 8077;

// Market rule tags.
pub const TAG_MARKET_RULE_START: u32 = 6019; // value "1" starts a new rule block
pub const TAG_MARKET_RULE_ID: u32 = 6031;    // rule ID integer
pub const TAG_LOW_EDGE: u32 = 6023;          // price increment threshold
pub const TAG_INCREMENT: u32 = 6027;         // tick size at that price level
pub const TAG_MARKET_RULE_END: u32 = 6030;   // end marker

/// Security types (IB internal encoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityType {
    Stock,    // CS
    Option,   // OPT
    Future,   // FUT
    Forex,    // CASH
    Index,    // IND
    Bond,     // BOND
    Warrant,  // WAR
    FutureOption, // FOP
    Cfd,      // CFD
    Commodity, // CMDTY
    Fund,     // FUND
    Forward,  // FWD
    Bill,     // BILL
    Combo,    // BAG
    Crypto,    // CRYPTO
    FixedIncome, // FIXED
    SecuritiesLending, // SLB
    News,      // NEWS
    Basket,    // BSK
    IndexOption, // IOPT
    IcuContract, // ICU
    IcsContract, // ICS
    PhysicalSettlement, // PHYSS
    Other,
}

impl SecurityType {
    /// Official API string ("STK", "OPT", ...). THE single mapping for
    /// everything user-visible — the callbacks previously reported a Debug
    /// derive ("Stock"), which no request path accepts, so a returned
    /// Contract could not be fed back into another call (ibx#230).
    /// `Other` maps to "" on purpose: an instrument the engine could not
    /// classify must not masquerade as a stock — the order path is
    /// STK-only and that one wrong guess would not be caught downstream.
    pub fn to_api_str(&self) -> &'static str {
        match self {
            Self::Stock => "STK",
            Self::Option => "OPT",
            Self::Future => "FUT",
            Self::Forex => "CASH",
            Self::Index => "IND",
            Self::Bond => "BOND",
            Self::Warrant => "WAR",
            Self::FutureOption => "FOP",
            Self::Cfd => "CFD",
            Self::Commodity => "CMDTY",
            Self::Fund => "FUND",
            Self::Forward => "FWD",
            Self::Bill => "BILL",
            Self::Combo => "BAG",
            Self::Crypto => "CRYPTO",
            Self::FixedIncome => "FIXED",
            Self::SecuritiesLending => "SLB",
            Self::News => "NEWS",
            Self::Basket => "BSK",
            Self::IndexOption => "IOPT",
            Self::IcuContract => "ICU",
            Self::IcsContract => "ICS",
            Self::PhysicalSettlement => "PHYSS",
            Self::Other => "",
        }
    }

    /// Convert to the wire encoding.
    pub fn to_fix(&self) -> &'static str {
        match self {
            Self::Stock => "CS",
            Self::Option => "OPT",
            Self::Future => "FUT",
            Self::Forex => "CASH",
            Self::Index => "IND",
            Self::Bond => "BOND",
            Self::Warrant => "WAR",
            Self::FutureOption => "FOP",
            Self::Cfd => "CFD",
            Self::Commodity => "CMDTY",
            Self::Fund => "FUND",
            Self::Forward => "FWD",
            Self::Bill => "BILL",
            Self::Combo => "BAG",
            Self::Crypto => "CRYPTO",
            Self::FixedIncome => "FIXED",
            Self::SecuritiesLending => "SLB",
            Self::News => "NEWS",
            Self::Basket => "BSK",
            Self::IndexOption => "IOPT",
            Self::IcuContract => "ICU",
            Self::IcsContract => "ICS",
            Self::PhysicalSettlement => "PHYSS",
            // An unrecognized security type must not be sent as a stock —
            // that misroutes the request silently (ibx#223). Empty draws a
            // visible gateway error instead, matching its own
            // unknown-to-none handling.
            Self::Other => "",
        }
    }

    /// Parse from wire format.
    pub fn from_fix(s: &str) -> Self {
        match s {
            "CS" | "STK" => Self::Stock,
            "OPT" => Self::Option,
            "FUT" => Self::Future,
            "CASH" => Self::Forex,
            "IND" => Self::Index,
            "BOND" => Self::Bond,
            "WAR" => Self::Warrant,
            "FOP" => Self::FutureOption,
            "CFD" => Self::Cfd,
            "CMDTY" => Self::Commodity,
            "FUND" => Self::Fund,
            "FWD" => Self::Forward,
            "BILL" => Self::Bill,
            "BAG" => Self::Combo,
            "CRYPTO" => Self::Crypto,
            "FIXED" => Self::FixedIncome,
            "SLB" => Self::SecuritiesLending,
            "NEWS" => Self::News,
            "BSK" => Self::Basket,
            "IOPT" => Self::IndexOption,
            "ICU" => Self::IcuContract,
            "ICS" => Self::IcsContract,
            "PHYSS" => Self::PhysicalSettlement,

            _ => Self::Other,
        }
    }
}

/// Option right (call/put).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionRight {
    Call,
    Put,
}

/// Full contract definition.
#[derive(Debug, Clone)]
pub struct ContractDefinition {
    pub con_id: u32,
    pub symbol: String,
    pub sec_type: SecurityType,
    pub exchange: String,
    pub primary_exchange: String,
    pub currency: String,
    pub local_symbol: String,
    pub trading_class: String,
    pub long_name: String,
    pub min_tick: f64,
    pub multiplier: f64,
    pub valid_exchanges: Vec<String>,
    pub order_types: Vec<String>,
    pub market_rule_id: Option<u32>,
    // Options/futures specific
    pub last_trade_date: String,
    pub strike: f64,
    pub right: Option<OptionRight>,
    // Extended fields
    pub stock_type: String,
    pub category: String,
    pub country: String,
    pub market_name: String,
    pub isin: String,
    pub min_size: f64,
    /// Trading session string. Populated by merging the paired schedule reply.
    pub trading_hours: Option<String>,
    /// Liquid (regular-session) hours string. Same source as trading_hours.
    pub liquid_hours: Option<String>,
    /// IANA timezone for session times (e.g. "US/Eastern").
    pub time_zone_id: Option<String>,
    /// Exchange-path join key (tag 6256) used to pair secdef ↔ schedule replies.
    /// Internal — not exposed on the public API surface.
    pub join_key: String,
}

impl Default for ContractDefinition {
    fn default() -> Self {
        Self {
            con_id: 0,
            symbol: String::new(),
            sec_type: SecurityType::Stock,
            exchange: String::new(),
            primary_exchange: String::new(),
            currency: String::new(),
            local_symbol: String::new(),
            trading_class: String::new(),
            long_name: String::new(),
            min_tick: 0.01,
            multiplier: 1.0,
            valid_exchanges: Vec::new(),
            order_types: Vec::new(),
            market_rule_id: None,
            last_trade_date: String::new(),
            strike: 0.0,
            right: None,
            stock_type: String::new(),
            category: String::new(),
            country: String::new(),
            market_name: String::new(),
            isin: String::new(),
            min_size: 0.0,
            trading_hours: None,
            liquid_hours: None,
            time_zone_id: None,
            join_key: String::new(),
        }
    }
}

/// Map exchange name.
pub fn exchange_to_fix(exchange: &str) -> &str {
    match exchange {
        "SMART" => "BEST",
        // Legacy spelling for NASDAQ. The depth path already translates it;
        // routing a subscription under the old name reaches nothing.
        "ISLAND" => "NASDAQ",
        other => other,
    }
}

/// Map a security type to its wire format.
pub fn sec_type_to_fix(sec_type: &str) -> &str {
    match sec_type {
        "STK" => "CS",
        other => other,
    }
}

/// Map exchange name back from wire format.
pub fn exchange_from_fix(exchange: &str) -> &str {
    match exchange {
        "BEST" => "SMART",
        other => other,
    }
}

/// Build a SecurityDefinitionRequest by conId.
pub fn build_secdef_request_by_conid(req_id: &str, con_id: u32, seq: u32) -> Vec<u8> {
    let con_id_str = con_id.to_string();
    fix::fix_build(
        &[
            (TAG_MSG_TYPE, "c"),
            (TAG_SECURITY_REQ_ID, req_id),
            (TAG_SECURITY_REQ_TYPE, "2"),
            (TAG_IB_CON_ID, &con_id_str),
            (TAG_IB_SOURCE, "Socket"),
        ],
        seq,
    )
}

/// Build a SecurityDefinitionRequest by symbol.
pub fn build_secdef_request_by_symbol(
    req_id: &str,
    symbol: &str,
    sec_type: SecurityType,
    exchange: &str,
    currency: &str,
    seq: u32,
) -> Vec<u8> {
    fix::fix_build(
        &[
            (TAG_MSG_TYPE, "c"),
            (TAG_SECURITY_REQ_ID, req_id),
            (TAG_SECURITY_REQ_TYPE, "2"),
            (TAG_SYMBOL, symbol),
            (TAG_SECURITY_TYPE, sec_type.to_fix()),
            (TAG_EXCHANGE, exchange_to_fix(exchange)),
            (TAG_CURRENCY, currency),
            (TAG_IB_SOURCE, "Socket"),
        ],
        seq,
    )
}

/// Parse a SecurityDefinition response into a ContractDefinition.
/// Whether a field with this tag appears in a record.
fn contains_field(body: &[u8], field: &[u8]) -> bool {
    use crate::protocol::fix::SOH;
    body.split(|&b| b == SOH).any(|part| part.starts_with(field))
}

/// Every contract a security-definition reply describes.
///
/// One reply can describe several. Asking for a symbol without saying which
/// currency returns each listing that answers to it — the dollar one and the
/// Australian dollar one arrive together — and reading the message as a single
/// contract keeps whichever came last, silently. That is the wrong contract to
/// trade, and it looks exactly like the right one.
///
/// Each contract begins at its symbol. What precedes the first symbol is the
/// message header, and every record needs it, so it rides in front of each.
/// The symbol tag is reused by the identifier block — `55=BBG` and `55=US` sit
/// there — so a record counts only when it also states what the contract is
/// and which contract it is.
pub fn parse_secdef_responses(data: &[u8]) -> Vec<ContractDefinition> {
    use crate::protocol::fix::SOH;
    const SYMBOL_FIELD: &[u8] = b"\x0155=";

    let mut starts = Vec::new();
    let mut at = 0;
    while let Some(found) = data[at..]
        .windows(SYMBOL_FIELD.len())
        .position(|w| w == SYMBOL_FIELD)
    {
        starts.push(at + found);
        at += found + SYMBOL_FIELD.len();
    }
    // Nothing repeats, so the message is its own single record.
    if starts.len() < 2 {
        return parse_secdef_response(data).into_iter().collect();
    }

    let header = &data[..starts[0]];
    let mut out = Vec::with_capacity(starts.len());
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(data.len());
        let body = &data[start..end];
        // The symbol tag is reused inside the identifier block — `55=BBG` and
        // `55=US` sit there — so not every occurrence starts a contract. One
        // that does states what the contract is and which contract it is.
        let names_a_contract = contains_field(body, b"167=") && contains_field(body, b"6008=");
        if !names_a_contract {
            continue;
        }
        let mut record = Vec::with_capacity(header.len() + body.len() + 1);
        record.extend_from_slice(header);
        record.extend_from_slice(body);
        record.push(SOH);
        if let Some(def) = parse_secdef_response(&record) {
            out.push(def);
        }
    }
    out
}

pub fn parse_secdef_response(data: &[u8]) -> Option<ContractDefinition> {
    let tags = fix::fix_parse(data);

    // Verify it's a security definition message
    if tags.get(&TAG_MSG_TYPE).map(|s| s.as_str()) != Some("d") {
        return None;
    }

    let mut def = ContractDefinition::default();

    if let Some(v) = tags.get(&TAG_IB_CON_ID) {
        def.con_id = v.parse().unwrap_or(0);
    }
    if let Some(v) = tags.get(&TAG_SYMBOL) {
        def.symbol = v.clone();
    }
    if let Some(v) = tags.get(&TAG_SECURITY_TYPE) {
        def.sec_type = SecurityType::from_fix(v);
    }
    // Tag 207 (exchange) repeats for each valid exchange — use sequential parse
    // to get the FIRST occurrence (the contract's own exchange, usually BEST/SMART).
    {
        use crate::protocol::fix::SOH;
        let needle = b"207=";
        for part in data.split(|&b| b == SOH) {
            if part.starts_with(needle) {
                let val = std::str::from_utf8(&part[needle.len()..]).unwrap_or("");
                def.exchange = exchange_from_fix(val).to_string();
                break;
            }
        }
    }
    if let Some(v) = tags.get(&TAG_IB_PRIMARY_EXCHANGE) {
        def.primary_exchange = exchange_from_fix(v).to_string();
    }
    if let Some(v) = tags.get(&TAG_CURRENCY) {
        def.currency = v.clone();
    }
    if let Some(v) = tags.get(&TAG_IB_LOCAL_SYMBOL) {
        def.local_symbol = v.clone();
    }
    if let Some(v) = tags.get(&TAG_IB_TRADING_CLASS) {
        def.trading_class = v.clone();
    }
    if let Some(v) = tags.get(&TAG_LONG_NAME) {
        def.long_name = v.clone();
    }
    // Tag 6019 is the market-rule-block start sentinel (value "1"), NOT the
    // literal tick increment — see TAG_MARKET_RULE_START. When the secdef
    // carries an inline price-increment block, fix_parse keeps that sentinel,
    // so reading 6019 as min_tick yields 1.0. Derive min_tick from the parsed
    // increments instead: the smallest increment is the contract's minimum
    // tick (iso ContractDetails.minTick). Absent a rule block, the default
    // (0.01) stands.
    if let Some(min_increment) = parse_market_rules(data)
        .iter()
        .flat_map(|rule| rule.price_increments.iter())
        .map(|inc| inc.increment)
        .filter(|inc| *inc > 0.0)
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    {
        def.min_tick = min_increment;
    }
    if let Some(v) = tags.get(&TAG_MULTIPLIER) {
        def.multiplier = v.parse().unwrap_or(1.0);
    }
    if let Some(v) = tags.get(&TAG_IB_VALID_EXCHANGES) {
        def.valid_exchanges = v.split(',').map(|s| exchange_from_fix(s).to_string()).collect();
    }
    if let Some(v) = tags.get(&TAG_IB_ORDER_TYPES) {
        def.order_types = v.split(',').map(|s| s.to_string()).collect();
    }
    if let Some(v) = tags.get(&TAG_IB_MARKET_RULE_ID) {
        def.market_rule_id = v.parse().ok();
    }
    // Either tag may carry it: a contract month on 200, a full expiry date on
    // 541. Reading only 200 left an option's expiry empty when the definition
    // stated the date, and preferring 200 where both are stated threw the
    // exact expiry away — a weekly option handed back as its month no longer
    // names the contract it came from.
    if let Some(v) = tags.get(&TAG_MATURITY_DATE).or_else(|| tags.get(&TAG_LAST_TRADE_DATE)) {
        def.last_trade_date = v.clone();
    }
    if let Some(v) = tags.get(&TAG_STRIKE) {
        def.strike = v.parse().unwrap_or(0.0);
    }
    if let Some(v) = tags.get(&TAG_RIGHT) {
        // The definition states this numerically — 1 for a call, 0 for a put —
        // which is the same encoding the request carries. Reading only the
        // letter form left every option's right unset, so a call and a put
        // came back indistinguishable outside their local symbol.
        def.right = match v.as_str() {
            "C" | "Call" | "1" => Some(OptionRight::Call),
            "P" | "Put" | "0" => Some(OptionRight::Put),
            _ => None,
        };
    }
    // Extended fields
    if let Some(v) = tags.get(&8077) { // StockType
        def.stock_type = v.clone();
    }
    if let Some(v) = tags.get(&6624) { // Category (pipe-delimited: "Technology|Computers|Computers")
        def.category = v.clone();
    }
    if let Some(v) = tags.get(&6911) { // Country
        def.country = v.clone();
    }
    if let Some(v) = tags.get(&58) { // MarketName
        def.market_name = v.clone();
    }
    if let Some(v) = tags.get(&TAG_SCHEDULE_JOIN_KEY) {
        def.join_key = v.clone();
    }
    // ISIN from SecurityAltID repeating group (tag 455 with source 456=4)
    // fix_parse only keeps last value per tag, so we parse sequentially
    {
        use crate::protocol::fix::SOH;
        let mut last_alt_id = String::new();
        for part in data.split(|&b| b == SOH) {
            let text = String::from_utf8_lossy(part);
            if let Some(val) = text.strip_prefix("455=") {
                last_alt_id = val.to_string();
            } else if let Some(val) = text.strip_prefix("456=")
                && val == "4" { // ISIN
                    def.isin = last_alt_id.clone();
                }
        }
    }
    if let Some(v) = tags.get(&8598) { // MinSizeIncrement
        def.min_size = v.parse().unwrap_or(0.0);
    }

    Some(def)
}

/// Extract the SecurityReqID from a response to match with the original request.
pub fn secdef_response_req_id(data: &[u8]) -> Option<String> {
    let tags = fix::fix_parse(data);
    tags.get(&TAG_SECURITY_REQ_ID).cloned()
}

/// Check if a response is the last one (response type 5 or 6).
pub fn secdef_response_is_last(data: &[u8]) -> bool {
    let tags = fix::fix_parse(data);
    matches!(
        tags.get(&TAG_SECURITY_RESPONSE_TYPE).map(|s| s.as_str()),
        Some("5") | Some("6")
    )
}

// ─── Market rules ───

/// A price increment rule defining tick sizes at different price levels.
#[derive(Debug, Clone)]
pub struct PriceIncrement {
    pub low_edge: f64,
    pub increment: f64,
}

/// A market rule containing a rule ID and its price increment table.
#[derive(Debug, Clone)]
pub struct MarketRule {
    pub rule_id: i32,
    pub price_increments: Vec<PriceIncrement>,
}

/// Parse market rules from a raw message.
///
/// Uses sequential tag parsing since rules are a repeating group.
pub fn parse_market_rules(data: &[u8]) -> Vec<MarketRule> {
    use crate::protocol::fix::SOH;

    let mut tags: Vec<(u32, String)> = Vec::new();
    for part in data.split(|&b| b == SOH) {
        if part.is_empty() { continue; }
        let text = String::from_utf8_lossy(part);
        if let Some((tag_str, val)) = text.split_once('=')
            && let Ok(tag) = tag_str.parse::<u32>() {
                tags.push((tag, val.to_string()));
            }
    }

    let mut rules: Vec<MarketRule> = Vec::new();
    let mut current: Option<MarketRule> = None;
    let mut pending_low_edge: Option<f64> = None;

    for (tag, val) in &tags {
        match *tag {
            TAG_MARKET_RULE_START if val == "1" => {
                // Flush previous rule if any
                if let Some(rule) = current.take() {
                    rules.push(rule);
                }
                current = Some(MarketRule {
                    rule_id: 0,
                    price_increments: Vec::new(),
                });
                pending_low_edge = None;
            }
            TAG_MARKET_RULE_ID => {
                if let Some(ref mut rule) = current {
                    rule.rule_id = val.parse().unwrap_or(0);
                }
            }
            TAG_LOW_EDGE => {
                if current.is_some() {
                    pending_low_edge = val.parse().ok();
                }
            }
            TAG_INCREMENT => {
                if let Some(ref mut rule) = current
                    && let Some(low_edge) = pending_low_edge.take()
                        && let Ok(increment) = val.parse::<f64>() {
                            rule.price_increments.push(PriceIncrement { low_edge, increment });
                        }
            }
            TAG_MARKET_RULE_END => {
                if let Some(rule) = current.take() {
                    rules.push(rule);
                }
                pending_low_edge = None;
            }
            _ => {}
        }
    }
    // Flush last rule if no 6030 end marker was present
    if let Some(rule) = current.take() {
        rules.push(rule);
    }

    rules
}

/// Cache of contract definitions by conId.
#[derive(Debug, Default)]
pub struct ContractStore {
    by_con_id: HashMap<u32, ContractDefinition>,
    by_symbol: HashMap<String, u32>,
}

impl ContractStore {
    pub fn insert(&mut self, def: ContractDefinition) {
        let key = format!("{}:{}:{}", def.symbol, def.sec_type.to_fix(), def.currency);
        self.by_symbol.insert(key, def.con_id);
        self.by_con_id.insert(def.con_id, def);
    }

    pub fn get(&self, con_id: u32) -> Option<&ContractDefinition> {
        self.by_con_id.get(&con_id)
    }

    pub fn find(&self, symbol: &str, sec_type: SecurityType, currency: &str) -> Option<&ContractDefinition> {
        let key = format!("{}:{}:{}", symbol, sec_type.to_fix(), currency);
        self.by_symbol.get(&key).and_then(|id| self.by_con_id.get(id))
    }

    pub fn len(&self) -> usize {
        self.by_con_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_con_id.is_empty()
    }
}

// ─── Schedule subscription ───

/// Tags for schedule subscription responses.
pub const TAG_SUB_PROTOCOL: u32 = 6040;
pub const TAG_SCHEDULE_TIMEZONE: u32 = 6734;
pub const TAG_SESSION_COUNT: u32 = 6840;
pub const TAG_SESSION_START: u32 = 6841;
pub const TAG_SESSION_END: u32 = 6842;
pub const TAG_TRADE_DATE: u32 = 75;
pub const TAG_IS_TRADING_HOURS: u32 = 6843;
pub const TAG_IS_LIQUID_HOURS: u32 = 6844;
/// Exchange-path key shared by paired secdef and schedule replies.
pub const TAG_SCHEDULE_JOIN_KEY: u32 = 6256;
/// Subscribe protocol value for schedule subscription.
pub const SUB_PROTOCOL_SCHEDULE_SUBSCRIBE: &str = "106";
/// Subscribe protocol value for schedule reply.
pub const SUB_PROTOCOL_SCHEDULE_REPLY: &str = "107";

/// A single trading/liquid hours session.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleSession {
    pub start: String,
    pub end: String,
    pub trade_date: String,
}

/// Parsed schedule response.
#[derive(Debug, Clone)]
pub struct ContractSchedule {
    pub timezone: String,
    pub trading_hours: Vec<ScheduleSession>,
    pub liquid_hours: Vec<ScheduleSession>,
}

/// Tag/value pairs in the order the message states them. Repeating groups are
/// told apart by where a tag sits, which a keyed parse cannot express.
fn tag_sequence(data: &[u8]) -> Vec<(u32, String)> {
    let mut tags: Vec<(u32, String)> = Vec::new();
    for part in data.split(|&b| b == fix::SOH) {
        if part.is_empty() { continue; }
        let text = String::from_utf8_lossy(part);
        if let Some((tag_str, val)) = text.split_once('=')
            && let Ok(tag) = tag_str.parse::<u32>() {
                tags.push((tag, val.to_string()));
            }
    }
    tags
}

/// Parse a schedule response into trading/liquid hours.
///
/// Uses sequential tag parsing since sessions are a repeating group.
pub fn parse_schedule_response(data: &[u8]) -> Option<ContractSchedule> {
    let tags = tag_sequence(data);

    // Verify this is a schedule response
    let msg_type = tags.iter().find(|(t, _)| *t == fix::TAG_MSG_TYPE)?.1.as_str();
    if msg_type != "U" { return None; }
    let sub_protocol = tags.iter().find(|(t, _)| *t == TAG_SUB_PROTOCOL)?.1.as_str();
    if sub_protocol != "107" { return None; }

    let timezone = tags.iter()
        .find(|(t, _)| *t == TAG_SCHEDULE_TIMEZONE)
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    // Parse repeating session groups.
    // Each session starts with tag 6841 (start) and includes 6842 (end), 75 (date),
    // and either 6843 (trading) or 6844 (liquid).
    let mut trading_hours = Vec::new();
    let mut liquid_hours = Vec::new();

    let mut start = String::new();
    let mut end = String::new();
    let mut trade_date = String::new();
    let mut is_trading = false;
    let mut is_liquid = false;
    let mut in_session = false;

    // 24h venues (e.g. FOREX) emit sessions with both 6843=1 AND 6844=1 — append
    // to both lists independently.
    for (tag, val) in &tags {
        match *tag {
            TAG_SESSION_START => {
                if in_session {
                    flush_session(&mut trading_hours, &mut liquid_hours,
                        start.clone(), end.clone(), trade_date.clone(), is_trading, is_liquid);
                }
                start = val.clone();
                end.clear();
                trade_date.clear();
                is_trading = false;
                is_liquid = false;
                in_session = true;
            }
            TAG_SESSION_END => end = val.clone(),
            TAG_TRADE_DATE => trade_date = val.clone(),
            TAG_IS_TRADING_HOURS => is_trading = val == "1",
            TAG_IS_LIQUID_HOURS => is_liquid = val == "1",
            _ => {}
        }
    }
    if in_session {
        flush_session(&mut trading_hours, &mut liquid_hours,
            start, end, trade_date, is_trading, is_liquid);
    }

    Some(ContractSchedule { timezone, trading_hours, liquid_hours })
}

/// Append a parsed session to the hour lists. A session with NEITHER flag
/// set is a closed day; it used to be dropped, leaving "market closed"
/// indistinguishable from "data missing" (ibx#223). It is kept as a
/// zero-length session in both lists, which renders as `<date>:CLOSED`.
fn flush_session(
    trading_hours: &mut Vec<ScheduleSession>,
    liquid_hours: &mut Vec<ScheduleSession>,
    start: String,
    end: String,
    trade_date: String,
    is_trading: bool,
    is_liquid: bool,
) {
    let closed = !is_trading && !is_liquid;
    let session = ScheduleSession {
        end: if closed { start.clone() } else { end },
        start,
        trade_date,
    };
    if closed {
        trading_hours.push(session.clone());
        liquid_hours.push(session);
    } else {
        if is_trading { trading_hours.push(session.clone()); }
        if is_liquid { liquid_hours.push(session); }
    }
}

/// Format a list of sessions into a semicolon-delimited string.
///
/// Output: `"YYYYMMDD:HHMM-YYYYMMDD:HHMM;YYYYMMDD:CLOSED;..."`.
/// Times are in UTC as received from the upstream wire — consumers should
/// convert to local time using the paired timezone identifier when displaying.
/// A zero-length session is a closed day and renders as `<date>:CLOSED`,
/// the official-API convention (ibx#223).
/// Returns an empty string if `sessions` is empty.
pub fn format_sessions_string(sessions: &[ScheduleSession]) -> String {
    let mut out = String::with_capacity(sessions.len() * 32);
    for (i, s) in sessions.iter().enumerate() {
        if i > 0 { out.push(';'); }
        if s.start == s.end {
            // `get` rather than a length check and a slice: these are gateway
            // strings decoded with `from_utf8_lossy`, so an invalid byte
            // becomes a three-byte replacement character and a cut that was
            // ASCII in the intended payload lands mid-character. Slicing a
            // `&str` off a character boundary is a panic, and this runs on the
            // hot loop (ibx#258).
            //
            // What replaces the panic is a degraded field, not a correct one:
            // a reply this malformed has no recoverable date in it. The point
            // is that it does not take the loop down with it.
            let date = s.trade_date.get(..8)
                .or_else(|| s.start.get(..8))
                .unwrap_or(s.start.as_str());
            out.push_str(date);
            out.push_str(":CLOSED");
        } else {
            out.push_str(&trim_session_endpoint(&s.start));
            out.push('-');
            out.push_str(&trim_session_endpoint(&s.end));
        }
    }
    out
}

/// Convert wire `YYYYMMDD-HH:MM:SS` to compact `YYYYMMDD:HHMM`.
/// Returns the input unchanged if the format does not match.
fn trim_session_endpoint(s: &str) -> String {
    let bytes = s.as_bytes();
    // The format is ASCII by definition, and requiring that outright is what
    // makes the byte positions below sound rather than incidentally correct:
    // the guard establishes what is at bytes 8 and 11 and says nothing about
    // 12 to 14, where a multi-byte character would panic the slice (ibx#258).
    if s.is_ascii() && bytes.len() >= 14 && bytes[8] == b'-' && bytes[11] == b':' {
        let mut out = String::with_capacity(13);
        out.push_str(&s[..8]);
        out.push(':');
        out.push_str(&s[9..11]);
        out.push_str(&s[12..14]);
        out
    } else {
        s.to_string()
    }
}
#[cfg(test)]
mod hot_loop_panic_tests {
    use super::*;

    /// ibx#258: frame bodies are decoded with from_utf8_lossy, so one invalid
    /// byte becomes a three-byte U+FFFD that can straddle a byte-indexed slice
    /// boundary. These must return a value, not abort the hot loop.
    #[test]
    fn session_endpoint_survives_a_lossily_decoded_field() {
        // U+FFFD at bytes 12..15 — the old &s[12..14] cut inside it.
        let lossy = format!("20260728-09:{}0", '\u{FFFD}');
        assert!(!lossy.is_ascii());
        let _ = trim_session_endpoint(&lossy);

        let lossy2 = format!("2026072{}-09:30", '\u{FFFD}');
        let _ = trim_session_endpoint(&lossy2);

        // The ASCII form still trims exactly as before.
        assert_eq!(trim_session_endpoint("20260728-09:30"), "20260728:0930");
        assert_eq!(trim_session_endpoint("short"), "short");
    }

    /// The closed-session branch is the one that slices trade_date/start, and
    /// it is only taken when start == end. The previous version of this test
    /// set them differently, so it never reached the slice at all and passed
    /// against the unfixed code too.
    #[test]
    fn sessions_string_survives_a_non_ascii_trade_date() {
        let closed = format!("2026072{}", '\u{FFFD}');
        let s = ScheduleSession {
            trade_date: closed.clone(),
            start: closed.clone(),
            end: closed,
        };
        let out = format_sessions_string(&[s]);
        assert!(!out.is_empty(), "a closed session must still render");

        // A short trade_date falls back to start, and a short start to the
        // whole string, without slicing past a boundary.
        let short = ScheduleSession {
            trade_date: "2026".to_string(),
            start: "2026".to_string(),
            end: "2026".to_string(),
        };
        let _ = format_sessions_string(&[short]);
    }
}


// ─── Matching symbols search ───

/// Tags for matching symbols.
pub const TAG_MATCH_PATTERN: u32 = 58;
pub const TAG_MATCH_COUNT: u32 = 146;
pub const TAG_MATCH_PRIMARY_EXCHANGE: u32 = 6453;
pub const TAG_MATCH_DESCRIPTION: u32 = 306;
pub const TAG_MATCH_DERIVATIVE_TYPES: u32 = 6070;

/// A single matching symbol result.
#[derive(Debug, Clone)]
pub struct SymbolMatch {
    pub con_id: u32,
    pub symbol: String,
    pub sec_type: SecurityType,
    pub currency: String,
    pub primary_exchange: String,
    pub description: String,
    pub derivative_types: Vec<String>,
}

/// Build a matching symbols request.
pub fn build_matching_symbols_request(pattern: &str, req_id: &str, seq: u32) -> Vec<u8> {
    fix::fix_build(
        &[
            (fix::TAG_MSG_TYPE, "U"),
            (TAG_SUB_PROTOCOL, "185"),
            (TAG_SECURITY_REQ_ID, req_id),
            (TAG_MATCH_PATTERN, pattern),
        ],
        seq,
    )
}

/// Parse a matching symbols response.
///
/// Uses sequential tag parsing since matches are a repeating group.
pub fn parse_matching_symbols_response(data: &[u8]) -> Option<Vec<SymbolMatch>> {
    let tags = tag_sequence(data);

    // Verify this is a matching symbols response
    let msg_type = tags.iter().find(|(t, _)| *t == fix::TAG_MSG_TYPE)?.1.as_str();
    if msg_type != "U" { return None; }
    let sub_protocol = tags.iter().find(|(t, _)| *t == TAG_SUB_PROTOCOL)?.1.as_str();
    if sub_protocol != "186" { return None; }

    // Parse repeating groups: each match starts with tag 55 (symbol)
    let mut matches = Vec::new();
    let mut current: Option<SymbolMatch> = None;

    for (tag, val) in &tags {
        match *tag {
            TAG_SYMBOL => {
                if let Some(m) = current.take()
                    && m.con_id > 0 { matches.push(m); }
                current = Some(SymbolMatch {
                    con_id: 0,
                    symbol: val.clone(),
                    sec_type: SecurityType::Stock,
                    currency: String::new(),
                    primary_exchange: String::new(),
                    description: String::new(),
                    derivative_types: Vec::new(),
                });
            }
            TAG_SECURITY_TYPE => {
                if let Some(ref mut m) = current {
                    m.sec_type = SecurityType::from_fix(val);
                }
            }
            TAG_CURRENCY => {
                if let Some(ref mut m) = current {
                    m.currency = val.clone();
                }
            }
            TAG_IB_CON_ID => {
                if let Some(ref mut m) = current {
                    m.con_id = val.parse().unwrap_or(0);
                }
            }
            TAG_MATCH_PRIMARY_EXCHANGE => {
                if let Some(ref mut m) = current {
                    m.primary_exchange = val.clone();
                }
            }
            TAG_MATCH_DESCRIPTION => {
                if let Some(ref mut m) = current {
                    m.description = val.clone();
                }
            }
            TAG_MATCH_DERIVATIVE_TYPES => {
                if let Some(ref mut m) = current {
                    m.derivative_types = val.split(',').map(|s| s.to_string()).collect();
                }
            }
            _ => {}
        }
    }
    // Flush last match
    if let Some(m) = current
        && m.con_id > 0 { matches.push(m); }

    Some(matches)
}

// ─── Option chain parameters ───

/// The strikes a scope lists, as one delimited value.
const TAG_CHAIN_STRIKES: u32 = 6997;
/// The tags a record states its expirations on. Regular and non-regular
/// expirations are both expirations of the chain that was asked for, so all
/// four are read into the one list.
const TAG_CHAIN_EXPIRATIONS: [u32; 4] = [6775, 6777, 6778, 6971];
/// Opens a keyed bucket inside an expiration value. What comes before the
/// first one is the chain itself.
const EXPIRATION_BUCKET_MARKER: &str = "/EXP";

/// Reported once per process rather than once per value: a shape this does not
/// recognise repeats on every record of the reply.
static EXPIRATION_FORM_REPORTED: AtomicBool = AtomicBool::new(false);

/// One (exchange, trading class, multiplier) scope of an option chain: the
/// strikes listed under it, and the expirations of the record it belongs to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OptionChainScope {
    pub symbol: String,
    pub exchange: String,
    pub trading_class: String,
    pub multiplier: String,
    pub expirations: Vec<String>,
    pub strikes: Vec<f64>,
}

/// Parse an option chain reply.
///
/// The venue answers a chain request with one record per underlying, delimited
/// by the symbol tag, and each record names every scope it lists strikes under.
/// The expirations belong to the record rather than to one of its scopes, so
/// they are stamped onto all of them once the record is complete.
pub fn parse_option_chain_response(data: &[u8]) -> Option<Vec<OptionChainScope>> {
    let tags = tag_sequence(data);

    let msg_type = tags.iter().find(|(t, _)| *t == TAG_MSG_TYPE)?.1.as_str();
    if msg_type != "U" { return None; }
    let sub_protocol = tags.iter().find(|(t, _)| *t == TAG_SUB_PROTOCOL)?.1.as_str();
    if sub_protocol != "139" { return None; }

    let starts: Vec<usize> = tags.iter().enumerate()
        .filter(|(_, (tag, _))| *tag == TAG_SYMBOL)
        .map(|(i, _)| i)
        .collect();
    let mut scopes = Vec::new();
    for (n, &start) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(tags.len());
        parse_chain_record(&tags[start].1, &tags[start..end], &mut scopes);
    }
    Some(scopes)
}

/// One record of a chain reply. A scope states only what it changes, so the
/// keys carry forward until the record names them again, and a strike list
/// closes the scope it was stated under.
fn parse_chain_record(symbol: &str, tags: &[(u32, String)], out: &mut Vec<OptionChainScope>) {
    let first = out.len();
    let mut scope = OptionChainScope { symbol: symbol.to_string(), ..Default::default() };
    let mut expirations = Vec::new();

    for (tag, val) in tags {
        match *tag {
            TAG_EXCHANGE => scope.exchange = val.clone(),
            TAG_IB_TRADING_CLASS => scope.trading_class = val.clone(),
            TAG_MULTIPLIER => scope.multiplier = val.clone(),
            TAG_CHAIN_STRIKES => {
                let strikes = val.split(';').filter_map(|s| s.trim().parse().ok()).collect();
                out.push(OptionChainScope { strikes, ..scope.clone() });
            }
            tag if TAG_CHAIN_EXPIRATIONS.contains(&tag) => collect_expirations(val, &mut expirations),
            _ => {}
        }
    }

    // A record that listed no strikes still states expirations, and those are
    // half of what was asked for. They are reported under the keys the record
    // did name rather than dropped.
    if out.len() == first && !expirations.is_empty() {
        out.push(scope);
    }
    for scope in &mut out[first..] {
        scope.expirations = expirations.clone();
    }
}

/// Expirations ride as a compound value: the chain itself, then buckets under
/// keys of their own. Everything before the first bucket is the chain, and its
/// dates are separated by the character the bucket marker opens with. A value
/// holding no date in that shape is read as a plain list instead, so an
/// encoding this has not met is reported rather than dropped.
fn collect_expirations(value: &str, out: &mut Vec<String>) {
    let chain = value.split(EXPIRATION_BUCKET_MARKER).next().unwrap_or_default();
    let mut found = false;
    for entry in chain.split('/').filter(|e| is_expiration(e)) {
        push_unique(out, entry);
        found = true;
    }
    if found || value.is_empty() {
        return;
    }
    if !EXPIRATION_FORM_REPORTED.swap(true, Ordering::Relaxed) {
        log::warn!("Option chain expirations in an unrecognised form ('{value}') — read as a plain list");
    }
    for entry in value.split(['/', ',', ';', ' ']).filter(|e| is_expiration(e)) {
        push_unique(out, entry);
    }
}

/// A date the venue states as YYYYMM or YYYYMMDD. Anything else in a compound
/// value is structure rather than an expiration.
fn is_expiration(entry: &str) -> bool {
    matches!(entry.len(), 6 | 8) && entry.bytes().all(|b| b.is_ascii_digit())
}

/// The same expiration is stated on more than one tag of a record, and a caller
/// asked for the dates a class trades on, not for how often each was named.
fn push_unique(out: &mut Vec<String>, entry: &str) {
    if !out.iter().any(|e| e == entry) {
        out.push(entry.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_type_roundtrip() {
        for st in [
            SecurityType::Stock,
            SecurityType::Option,
            SecurityType::Future,
            SecurityType::Forex,
        ] {
            assert_eq!(SecurityType::from_fix(st.to_fix()), st);
        }
    }

    #[test]
    fn exchange_mapping() {
        assert_eq!(exchange_to_fix("SMART"), "BEST");
        assert_eq!(exchange_to_fix("NYSE"), "NYSE");
        assert_eq!(exchange_to_fix("ISLAND"), "NASDAQ", "legacy spelling must route");
        assert_eq!(exchange_from_fix("BEST"), "SMART");
        assert_eq!(exchange_from_fix("ARCA"), "ARCA");
    }

    #[test]
    fn sec_type_wire_mapping() {
        // Only STK is renamed on the wire; everything else passes through.
        assert_eq!(sec_type_to_fix("STK"), "CS");
        assert_eq!(sec_type_to_fix("FUT"), "FUT");
        assert_eq!(sec_type_to_fix("OPT"), "OPT");
        assert_eq!(sec_type_to_fix("CASH"), "CASH");
    }

    #[test]
    fn build_secdef_by_conid() {
        let msg = build_secdef_request_by_conid("R1", 265598, 1);
        let tags = fix::fix_parse(&msg);
        assert_eq!(tags[&TAG_MSG_TYPE], "c");
        assert_eq!(tags[&TAG_SECURITY_REQ_ID], "R1");
        assert_eq!(tags[&TAG_SECURITY_REQ_TYPE], "2");
        assert_eq!(tags[&TAG_IB_CON_ID], "265598");
        assert_eq!(tags[&TAG_IB_SOURCE], "Socket");
    }

    #[test]
    fn build_secdef_by_symbol() {
        let msg = build_secdef_request_by_symbol("R2", "AAPL", SecurityType::Stock, "SMART", "USD", 2);
        let tags = fix::fix_parse(&msg);
        assert_eq!(tags[&TAG_MSG_TYPE], "c");
        assert_eq!(tags[&TAG_SYMBOL], "AAPL");
        assert_eq!(tags[&TAG_SECURITY_TYPE], "CS");
        assert_eq!(tags[&TAG_EXCHANGE], "BEST"); // SMART→BEST
        assert_eq!(tags[&TAG_CURRENCY], "USD");
    }

    #[test]
    fn parse_secdef_response() {
        // Build a fake security definition response
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_SECURITY_REQ_ID, "R1"),
                (TAG_SECURITY_RESPONSE_TYPE, "4"),
                (TAG_IB_CON_ID, "265598"),
                (TAG_SYMBOL, "AAPL"),
                (TAG_SECURITY_TYPE, "CS"),
                (TAG_SECURITY_EXCHANGE, "NASDAQ"),
                (TAG_CURRENCY, "USD"),
                (TAG_LONG_NAME, "APPLE INC"),
                (TAG_IB_VALID_EXCHANGES, "BEST,NYSE,ARCA"),
                (TAG_IB_PRIMARY_EXCHANGE, "NASDAQ"),
                // Inline price-increment block: min_tick is derived from the
                // smallest increment, not the 6019 rule-start sentinel.
                (TAG_MARKET_RULE_START, "1"),
                (TAG_MARKET_RULE_ID, "26"),
                (TAG_LOW_EDGE, "0"),
                (TAG_INCREMENT, "0.01"),
                (TAG_MARKET_RULE_END, "1"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg).unwrap();
        assert_eq!(def.con_id, 265598);
        assert_eq!(def.symbol, "AAPL");
        assert_eq!(def.sec_type, SecurityType::Stock);
        assert_eq!(def.exchange, "NASDAQ");
        assert_eq!(def.currency, "USD");
        assert_eq!(def.long_name, "APPLE INC");
        assert_eq!(def.min_tick, 0.01);
        assert_eq!(def.valid_exchanges, vec!["SMART", "NYSE", "ARCA"]);
        assert_eq!(def.primary_exchange, "NASDAQ");
    }

    #[test]
    fn parse_rejects_non_secdef() {
        let msg = fix::fix_build(&[(TAG_MSG_TYPE, "A")], 1);
        assert!(super::parse_secdef_response(&msg).is_none());
    }

    // Regression for ibx#197: a US equity secdef carries an inline price-
    // increment block whose start sentinel is `6019=1`. Tag 6019 must NOT be
    // read as min_tick (it would yield 1.0) — min_tick is the smallest parsed
    // increment.
    #[test]
    fn secdef_min_tick_from_price_increments_not_rule_sentinel() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "4726868"),
                (TAG_SYMBOL, "AXTI"),
                (TAG_SECURITY_TYPE, "CS"),
                (TAG_CURRENCY, "USD"),
                // Inline market-rule block (6019="1" is the start sentinel).
                (TAG_MARKET_RULE_START, "1"),
                (TAG_MARKET_RULE_ID, "26"),
                (TAG_LOW_EDGE, "0"),
                (TAG_INCREMENT, "0.0001"),
                (TAG_LOW_EDGE, "1"),
                (TAG_INCREMENT, "0.01"),
                (TAG_MARKET_RULE_END, "1"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg).unwrap();
        // Smallest increment across bands, not the "1" rule sentinel.
        assert_eq!(def.min_tick, 0.0001);
    }

    // Absent any inline rule block, min_tick keeps its default.
    #[test]
    fn secdef_min_tick_defaults_without_rule_block() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "265598"),
                (TAG_SYMBOL, "AAPL"),
                (TAG_SECURITY_TYPE, "CS"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg).unwrap();
        assert_eq!(def.min_tick, 0.01);
    }

    #[test]
    fn secdef_response_last_check() {
        let msg5 = fix::fix_build(
            &[(TAG_MSG_TYPE, "d"), (TAG_SECURITY_RESPONSE_TYPE, "5")],
            1,
        );
        let msg4 = fix::fix_build(
            &[(TAG_MSG_TYPE, "d"), (TAG_SECURITY_RESPONSE_TYPE, "4")],
            2,
        );
        assert!(secdef_response_is_last(&msg5));
        assert!(!secdef_response_is_last(&msg4));
    }

    #[test]
    fn contract_store_insert_and_lookup() {
        let mut store = ContractStore::default();
        let def = ContractDefinition {
            con_id: 265598,
            symbol: "AAPL".to_string(),
            sec_type: SecurityType::Stock,
            currency: "USD".to_string(),
            exchange: "NASDAQ".to_string(),
            ..Default::default()
        };
        store.insert(def);

        assert_eq!(store.len(), 1);
        let found = store.get(265598).unwrap();
        assert_eq!(found.symbol, "AAPL");

        let by_sym = store.find("AAPL", SecurityType::Stock, "USD").unwrap();
        assert_eq!(by_sym.con_id, 265598);

        assert!(store.find("MSFT", SecurityType::Stock, "USD").is_none());
    }

    #[test]
    fn contract_store_update_replaces() {
        let mut store = ContractStore::default();
        store.insert(ContractDefinition {
            con_id: 265598,
            symbol: "AAPL".to_string(),
            long_name: "OLD".to_string(),
            ..Default::default()
        });
        store.insert(ContractDefinition {
            con_id: 265598,
            symbol: "AAPL".to_string(),
            long_name: "APPLE INC".to_string(),
            ..Default::default()
        });
        assert_eq!(store.len(), 1);
        assert_eq!(store.get(265598).unwrap().long_name, "APPLE INC");
    }

    /// A definition that states both keeps the one that names the contract.
    /// Handing back the month instead turned a weekly option into something
    /// that no longer identifies what it came from.
    #[test]
    fn the_exact_expiry_wins_over_the_month_it_falls_in() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "12345"),
                (TAG_SYMBOL, "SPY"),
                (TAG_SECURITY_TYPE, "OPT"),
                (TAG_LAST_TRADE_DATE, "202609"),
                (TAG_MATURITY_DATE, "20260918"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg).unwrap();
        assert_eq!(def.last_trade_date, "20260918");
    }

    /// An expiry is a date and a contract month is not, and the two ride
    /// different tags. Reading only MaturityMonthYear left the expiry empty on
    /// any definition that stated the date.
    #[test]
    fn an_expiry_stated_as_a_date_is_read_from_its_own_tag() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "12345"),
                (TAG_SYMBOL, "AAPL"),
                (TAG_SECURITY_TYPE, "OPT"),
                (TAG_MATURITY_DATE, "20260918"),
                (TAG_STRIKE, "200.0"),
                (TAG_RIGHT, "C"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg).unwrap();
        assert_eq!(def.last_trade_date, "20260918");
    }

    #[test]
    fn option_contract_fields() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "12345"),
                (TAG_SYMBOL, "AAPL"),
                (TAG_SECURITY_TYPE, "OPT"),
                (TAG_LAST_TRADE_DATE, "20260321"),
                (TAG_STRIKE, "200.0"),
                (TAG_RIGHT, "C"),
                (TAG_MULTIPLIER, "100"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg).unwrap();
        assert_eq!(def.sec_type, SecurityType::Option);
        assert_eq!(def.last_trade_date, "20260321");
        assert_eq!(def.strike, 200.0);
        assert_eq!(def.right, Some(OptionRight::Call));
        assert_eq!(def.multiplier, 100.0);
    }

    #[test]
    fn parse_schedule_response_basic() {
        // Build a fake schedule response with 2 trading + 2 liquid sessions
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "U"),
                (TAG_SUB_PROTOCOL, "107"),
                (TAG_SCHEDULE_TIMEZONE, "US/Eastern"),
                (TAG_SESSION_COUNT, "4"),
                // Trading session 1
                (TAG_SESSION_START, "20260311-08:00:00"),
                (TAG_SESSION_END, "20260312-00:00:00"),
                (TAG_TRADE_DATE, "20260311"),
                (TAG_IS_TRADING_HOURS, "1"),
                // Liquid session 1
                (TAG_SESSION_START, "20260311-13:30:00"),
                (TAG_SESSION_END, "20260311-20:00:00"),
                (TAG_TRADE_DATE, "20260311"),
                (TAG_IS_LIQUID_HOURS, "1"),
                // Trading session 2
                (TAG_SESSION_START, "20260312-08:00:00"),
                (TAG_SESSION_END, "20260313-00:00:00"),
                (TAG_TRADE_DATE, "20260312"),
                (TAG_IS_TRADING_HOURS, "1"),
                // Liquid session 2
                (TAG_SESSION_START, "20260312-13:30:00"),
                (TAG_SESSION_END, "20260312-20:00:00"),
                (TAG_TRADE_DATE, "20260312"),
                (TAG_IS_LIQUID_HOURS, "1"),
            ],
            1,
        );
        let sched = parse_schedule_response(&msg).unwrap();
        assert_eq!(sched.timezone, "US/Eastern");
        assert_eq!(sched.trading_hours.len(), 2);
        assert_eq!(sched.liquid_hours.len(), 2);

        assert_eq!(sched.trading_hours[0].start, "20260311-08:00:00");
        assert_eq!(sched.trading_hours[0].end, "20260312-00:00:00");
        assert_eq!(sched.trading_hours[0].trade_date, "20260311");

        assert_eq!(sched.liquid_hours[0].start, "20260311-13:30:00");
        assert_eq!(sched.liquid_hours[0].end, "20260311-20:00:00");
    }

    #[test]
    fn parse_schedule_dual_flag_appends_to_both() {
        // 24h venues (FOREX) emit sessions with both 6843=1 AND 6844=1.
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "U"),
                (TAG_SUB_PROTOCOL, "107"),
                (TAG_SCHEDULE_TIMEZONE, "US/Eastern"),
                (TAG_SESSION_COUNT, "1"),
                (TAG_SESSION_START, "20260427-22:15:00"),
                (TAG_SESSION_END, "20260428-22:00:00"),
                (TAG_TRADE_DATE, "20260427"),
                (TAG_IS_TRADING_HOURS, "1"),
                (TAG_IS_LIQUID_HOURS, "1"),
            ],
            1,
        );
        let sched = parse_schedule_response(&msg).unwrap();
        assert_eq!(sched.trading_hours.len(), 1);
        assert_eq!(sched.liquid_hours.len(), 1);
        assert_eq!(sched.trading_hours[0].start, sched.liquid_hours[0].start);
    }

    #[test]
    fn format_sessions_string_basic() {
        let sessions = vec![
            ScheduleSession {
                start: "20260427-13:30:00".into(),
                end: "20260427-20:00:00".into(),
                trade_date: "20260427".into(),
            },
            ScheduleSession {
                start: "20260428-13:30:00".into(),
                end: "20260428-20:00:00".into(),
                trade_date: "20260428".into(),
            },
        ];
        let s = format_sessions_string(&sessions);
        assert_eq!(s, "20260427:1330-20260427:2000;20260428:1330-20260428:2000");
    }

    #[test]
    fn format_sessions_string_empty() {
        assert_eq!(format_sessions_string(&[]), "");
    }

    #[test]
    fn parse_schedule_rejects_non_schedule() {
        let msg = fix::fix_build(&[(TAG_MSG_TYPE, "d")], 1);
        assert!(parse_schedule_response(&msg).is_none());

        // Wrong sub-protocol
        let msg = fix::fix_build(
            &[(TAG_MSG_TYPE, "U"), (TAG_SUB_PROTOCOL, "100")],
            1,
        );
        assert!(parse_schedule_response(&msg).is_none());
    }

    #[test]
    fn market_rule_id_parsed() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "756733"),
                (TAG_SYMBOL, "SPY"),
                (TAG_SECURITY_TYPE, "CS"),
                (TAG_IB_MARKET_RULE_ID, "4563"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg).unwrap();
        assert_eq!(def.market_rule_id, Some(4563));
    }

    #[test]
    fn market_rule_id_absent() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "756733"),
                (TAG_SYMBOL, "SPY"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg).unwrap();
        assert_eq!(def.market_rule_id, None);
    }

    #[test]
    fn build_matching_symbols_request_structure() {
        let msg = build_matching_symbols_request("APP", "R1", 1);
        let tags = fix::fix_parse(&msg);
        assert_eq!(tags[&fix::TAG_MSG_TYPE], "U");
        assert_eq!(tags[&TAG_SUB_PROTOCOL], "185");
        assert_eq!(tags[&TAG_SECURITY_REQ_ID], "R1");
        assert_eq!(tags[&TAG_MATCH_PATTERN], "APP");
    }

    #[test]
    fn parse_matching_symbols_response_basic() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "U"),
                (TAG_SUB_PROTOCOL, "186"),
                (TAG_SECURITY_REQ_ID, "R1"),
                (TAG_MATCH_COUNT, "2"),
                // Match 1
                (TAG_SYMBOL, "AAPL"),
                (TAG_SECURITY_TYPE, "CS"),
                (TAG_CURRENCY, "USD"),
                (TAG_IB_CON_ID, "265598"),
                (TAG_MATCH_PRIMARY_EXCHANGE, "NASDAQ"),
                (TAG_MATCH_DESCRIPTION, "APPLE INC"),
                (TAG_MATCH_DERIVATIVE_TYPES, "OPT,WAR"),
                // Match 2
                (TAG_SYMBOL, "APP"),
                (TAG_SECURITY_TYPE, "CS"),
                (TAG_CURRENCY, "USD"),
                (TAG_IB_CON_ID, "481863646"),
                (TAG_MATCH_PRIMARY_EXCHANGE, "NASDAQ"),
                (TAG_MATCH_DESCRIPTION, "APPLOVIN CORP"),
                (TAG_MATCH_DERIVATIVE_TYPES, "OPT"),
            ],
            1,
        );
        let matches = parse_matching_symbols_response(&msg).unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].symbol, "AAPL");
        assert_eq!(matches[0].con_id, 265598);
        assert_eq!(matches[0].primary_exchange, "NASDAQ");
        assert_eq!(matches[0].description, "APPLE INC");
        assert_eq!(matches[0].derivative_types, vec!["OPT", "WAR"]);
        assert_eq!(matches[1].symbol, "APP");
        assert_eq!(matches[1].con_id, 481863646);
    }

    /// One reply, two underlyings, and two classes under the first of them.
    /// A scope states only what it changes from the one before it, and the
    /// expirations belong to the record rather than to either class.
    #[test]
    fn parse_option_chain_response_reads_every_scope_of_every_record() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "U"),
                (TAG_SUB_PROTOCOL, "139"),
                (8371, "opaque"),
                (6455, "2"),
                // First underlying, listed on two venues.
                (TAG_SYMBOL, "AAPL"),
                (6775, "20260116/20260220/20260320/EXPW=20260109"),
                (6777, "20260320"),
                (6971, "20260109"),
                (6346, "265598"),
                (TAG_EXCHANGE, "SMART"),
                (TAG_IB_TRADING_CLASS, "AAPL"),
                (TAG_MULTIPLIER, "100"),
                (6997, "140.0;145.0;150.0"),
                (TAG_EXCHANGE, "CBOE"),
                (TAG_IB_TRADING_CLASS, "AAPL1"),
                (6997, "145.0;150.0"),
                // Second underlying.
                (TAG_SYMBOL, "SPX"),
                (6778, "20260220"),
                (TAG_EXCHANGE, "CBOE"),
                (TAG_IB_TRADING_CLASS, "SPXW"),
                (TAG_MULTIPLIER, "100"),
                (6997, "5000.0;5100.0"),
            ],
            1,
        );

        let scopes = parse_option_chain_response(&msg).unwrap();

        assert_eq!(scopes.len(), 3, "two classes on AAPL and one on SPX");
        assert_eq!(scopes[0].symbol, "AAPL");
        assert_eq!(scopes[0].exchange, "SMART");
        assert_eq!(scopes[0].trading_class, "AAPL");
        assert_eq!(scopes[0].multiplier, "100");
        assert_eq!(scopes[0].strikes, vec![140.0, 145.0, 150.0]);
        assert_eq!(
            scopes[0].expirations,
            vec!["20260116", "20260220", "20260320", "20260109"],
            "the chain itself, without the keyed bucket, and each date once",
        );
        assert_eq!(scopes[1].exchange, "CBOE");
        assert_eq!(scopes[1].trading_class, "AAPL1");
        assert_eq!(scopes[1].multiplier, "100", "carried over from the class before it");
        assert_eq!(scopes[1].strikes, vec![145.0, 150.0]);
        assert_eq!(scopes[1].expirations, scopes[0].expirations, "both classes of one record");
        assert_eq!(scopes[2].symbol, "SPX");
        assert_eq!(scopes[2].trading_class, "SPXW");
        assert_eq!(scopes[2].strikes, vec![5000.0, 5100.0]);
        assert_eq!(scopes[2].expirations, vec!["20260220"], "and nothing of the record before it");
    }

    /// A value that does not hold a date in the compound shape is still read
    /// for the dates it does hold, rather than answering with no expirations.
    #[test]
    fn parse_option_chain_expirations_fall_back_to_a_plain_list() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "U"),
                (TAG_SUB_PROTOCOL, "139"),
                (TAG_SYMBOL, "AAPL"),
                (6775, "20260116,20260220"),
                (TAG_EXCHANGE, "SMART"),
                (TAG_IB_TRADING_CLASS, "AAPL"),
                (6997, "140.0"),
            ],
            1,
        );

        let scopes = parse_option_chain_response(&msg).unwrap();

        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].expirations, vec!["20260116", "20260220"]);
    }

    #[test]
    fn parse_option_chain_rejects_non_chain() {
        let msg = fix::fix_build(&[(TAG_MSG_TYPE, "U"), (TAG_SUB_PROTOCOL, "186")], 1);
        assert!(parse_option_chain_response(&msg).is_none());
    }

    // ibx#223: a closed day (neither hours flag set) must be represented,
    // not dropped — "market closed" and "data missing" were previously
    // indistinguishable.
    #[test]
    fn schedule_closed_day_is_kept_and_renders_closed() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "U"),
                (TAG_SUB_PROTOCOL, "107"),
                (TAG_SCHEDULE_TIMEZONE, "US/Eastern"),
                (TAG_SESSION_COUNT, "2"),
                // Saturday: closed — no 6843/6844 flags.
                (TAG_SESSION_START, "20260718-00:00:00"),
                (TAG_SESSION_END, "20260718-00:00:00"),
                (TAG_TRADE_DATE, "20260718"),
                // Monday: normal trading session.
                (TAG_SESSION_START, "20260720-13:30:00"),
                (TAG_SESSION_END, "20260720-20:00:00"),
                (TAG_TRADE_DATE, "20260720"),
                (TAG_IS_TRADING_HOURS, "1"),
                (TAG_IS_LIQUID_HOURS, "1"),
            ],
            1,
        );
        let sched = parse_schedule_response(&msg).unwrap();
        assert_eq!(sched.trading_hours.len(), 2, "closed day must appear");
        assert_eq!(sched.liquid_hours.len(), 2);
        let rendered = format_sessions_string(&sched.trading_hours);
        assert_eq!(rendered, "20260718:CLOSED;20260720:1330-20260720:2000");
    }

    // ibx#223: an unrecognized security type must not be encoded as a stock.
    #[test]
    fn to_fix_other_is_not_stock() {
        assert_eq!(SecurityType::Other.to_fix(), "");
        assert_eq!(SecurityType::from_fix(""), SecurityType::Other);
    }

    // ibx#230: user-visible sec_type must be the official API string, and
    // an unclassifiable instrument must not masquerade as a stock.
    #[test]
    fn sec_type_to_api_str_round_trips_and_other_is_empty() {
        assert_eq!(SecurityType::Stock.to_api_str(), "STK");
        assert_eq!(SecurityType::Forex.to_api_str(), "CASH");
        assert_eq!(SecurityType::Warrant.to_api_str(), "WAR");

        // Every type the terminal names, and each one back again. A type this
        // does not know is sent as an empty security type on purpose, so one
        // that is merely missing from here does not quietly route as a stock —
        // which means a gap shows up as a refused request rather than an order
        // on the wrong contract, and is worth keeping honest.
        for (ty, wire) in [
            (SecurityType::Stock, "STK"), (SecurityType::Option, "OPT"),
            (SecurityType::Future, "FUT"), (SecurityType::Forex, "CASH"),
            (SecurityType::Index, "IND"), (SecurityType::Bond, "BOND"),
            (SecurityType::Warrant, "WAR"), (SecurityType::FutureOption, "FOP"),
            (SecurityType::Cfd, "CFD"), (SecurityType::Commodity, "CMDTY"),
            (SecurityType::Fund, "FUND"), (SecurityType::Forward, "FWD"),
            (SecurityType::Bill, "BILL"), (SecurityType::Combo, "BAG"),
            (SecurityType::Crypto, "CRYPTO"),
            (SecurityType::FixedIncome, "FIXED"),
            (SecurityType::SecuritiesLending, "SLB"),
            (SecurityType::News, "NEWS"),
            (SecurityType::Basket, "BSK"),
            (SecurityType::IndexOption, "IOPT"),
            (SecurityType::IcuContract, "ICU"),
            (SecurityType::IcsContract, "ICS"),
            (SecurityType::PhysicalSettlement, "PHYSS"),
        ] {
            assert_eq!(ty.to_api_str(), wire, "{ty:?} states itself as {wire}");
            assert_eq!(SecurityType::from_fix(wire), ty, "and is read back from {wire}");
        }
        assert_eq!(SecurityType::from_fix("NOPE"), SecurityType::Other);
        assert_eq!(SecurityType::Other.to_fix(), "", "unknown stays unroutable, not a stock");
        assert_eq!(SecurityType::Other.to_api_str(), "");
        // Every non-Other variant survives the round trip back through the
        // inbound parser (which accepts API strings too), so a reported
        // Contract can be fed into another request.
        for st in [SecurityType::Stock, SecurityType::Option, SecurityType::Future,
                   SecurityType::Forex, SecurityType::Index, SecurityType::Bond,
                   SecurityType::Warrant] {
            assert_eq!(SecurityType::from_fix(st.to_api_str()), st, "{st:?}");
        }
    }

    #[test]
    fn parse_matching_symbols_rejects_non_match() {
        let msg = fix::fix_build(&[(TAG_MSG_TYPE, "d")], 1);
        assert!(parse_matching_symbols_response(&msg).is_none());

        let msg = fix::fix_build(
            &[(TAG_MSG_TYPE, "U"), (TAG_SUB_PROTOCOL, "107")],
            1,
        );
        assert!(parse_matching_symbols_response(&msg).is_none());
    }

    #[test]
    fn parse_market_rules_single_rule() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "265598"),
                (TAG_SYMBOL, "AAPL"),
                // Market rule block
                (TAG_MARKET_RULE_START, "1"),
                (TAG_MARKET_RULE_ID, "26"),
                (TAG_LOW_EDGE, "0"),
                (TAG_INCREMENT, "0.01"),
                (TAG_LOW_EDGE, "1"),
                (TAG_INCREMENT, "0.01"),
                (TAG_MARKET_RULE_END, "1"),
            ],
            1,
        );
        let rules = parse_market_rules(&msg);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].rule_id, 26);
        assert_eq!(rules[0].price_increments.len(), 2);
        assert_eq!(rules[0].price_increments[0].low_edge, 0.0);
        assert_eq!(rules[0].price_increments[0].increment, 0.01);
        assert_eq!(rules[0].price_increments[1].low_edge, 1.0);
        assert_eq!(rules[0].price_increments[1].increment, 0.01);
    }

    #[test]
    fn parse_market_rules_multiple_rules() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                // Rule 1: penny increments
                (TAG_MARKET_RULE_START, "1"),
                (TAG_MARKET_RULE_ID, "26"),
                (TAG_LOW_EDGE, "0"),
                (TAG_INCREMENT, "0.01"),
                (TAG_MARKET_RULE_END, "1"),
                // Rule 2: nickel increments above $1
                (TAG_MARKET_RULE_START, "1"),
                (TAG_MARKET_RULE_ID, "42"),
                (TAG_LOW_EDGE, "0"),
                (TAG_INCREMENT, "0.01"),
                (TAG_LOW_EDGE, "1"),
                (TAG_INCREMENT, "0.05"),
                (TAG_MARKET_RULE_END, "1"),
            ],
            1,
        );
        let rules = parse_market_rules(&msg);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].rule_id, 26);
        assert_eq!(rules[0].price_increments.len(), 1);
        assert_eq!(rules[1].rule_id, 42);
        assert_eq!(rules[1].price_increments.len(), 2);
        assert_eq!(rules[1].price_increments[1].low_edge, 1.0);
        assert_eq!(rules[1].price_increments[1].increment, 0.05);
    }

    #[test]
    fn parse_market_rules_empty_when_none() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "265598"),
                (TAG_SYMBOL, "AAPL"),
            ],
            1,
        );
        let rules = parse_market_rules(&msg);
        assert!(rules.is_empty());
    }

    #[test]
    fn parse_market_rules_no_end_marker() {
        // Rule without explicit 6030 end marker -- should still be collected
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_MARKET_RULE_START, "1"),
                (TAG_MARKET_RULE_ID, "10"),
                (TAG_LOW_EDGE, "0"),
                (TAG_INCREMENT, "0.005"),
            ],
            1,
        );
        let rules = parse_market_rules(&msg);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].rule_id, 10);
        assert_eq!(rules[0].price_increments.len(), 1);
        assert_eq!(rules[0].price_increments[0].increment, 0.005);
    }

    /// The character `from_utf8_lossy` substitutes for an invalid byte.
    const REPLACEMENT: &str = "\u{FFFD}";

    /// ibx#258: schedule strings come off the wire and are decoded with
    /// `from_utf8_lossy`, so one invalid byte becomes a three-byte replacement
    /// character and every byte position after it shifts. The parser validated
    /// byte positions and then sliced the `&str`, which panics off a character
    /// boundary — and this runs on the hot loop, so it is an engine-down on
    /// malformed input rather than a dropped message.
    #[test]
    fn a_schedule_that_is_not_ascii_does_not_panic() {
        // The shapes a lossy decode produces: a replacement character sitting
        // where the parser expects an ASCII digit, at each boundary it cuts on.
        let hostile = [
            "2026010@-09:30:00",
            "20260101-0@:30:00",
            "20260101-09:3@:00",
            "20260101-09:30:0@",
            "@",
            "",
            "short",
            "@@@@@",
        ];

        for h in hostile {
            let h = h.replace('@', REPLACEMENT);
            let sessions = [ScheduleSession {
                start: h.clone(),
                end: "20260101-16:00:00".to_string(),
                trade_date: h.clone(),
            }];
            let _ = format_sessions_string(&sessions);

            // And the closed-session branch, which takes the other two slices.
            let closed = [ScheduleSession {
                start: h.clone(),
                end: h.clone(),
                trade_date: h,
            }];
            let _ = format_sessions_string(&closed);
        }
    }

    /// The positive control: a well-formed schedule still gets trimmed to the
    /// compact form, so the guard above is not simply refusing everything.
    #[test]
    fn a_well_formed_schedule_is_still_trimmed() {
        let sessions = [
            ScheduleSession {
                start: "20260101-09:30:00".to_string(),
                end: "20260101-16:00:00".to_string(),
                trade_date: "20260101".to_string(),
            },
            ScheduleSession {
                start: "20260102-00:00:00".to_string(),
                end: "20260102-00:00:00".to_string(),
                trade_date: "20260102".to_string(),
            },
        ];
        assert_eq!(
            format_sessions_string(&sessions),
            "20260101:0930-20260101:1600;20260102:CLOSED",
        );
    }

    /// A reply naming two listings of one symbol is two contracts, not the
    /// last one. Read as a single contract it resolves to whichever came last,
    /// which is how asking for SPY yields the Australian dollar listing.
    #[test]
    fn a_reply_naming_two_listings_is_two_contracts() {
        let msg = crate::protocol::fix::fix_build(&[
            (crate::protocol::fix::TAG_MSG_TYPE, "d"),
            (TAG_SECURITY_REQ_ID, "7"),
            (TAG_SECURITY_RESPONSE_TYPE, "4"),
            (TAG_SYMBOL, "SPY"),
            (TAG_SECURITY_TYPE, "CS"),
            (207, "BEST"),
            (TAG_IB_CON_ID, "756733"),
            (TAG_CURRENCY, "USD"),
            (TAG_SYMBOL, "SPY"),
            (TAG_SECURITY_TYPE, "CS"),
            (207, "ASX"),
            (TAG_IB_CON_ID, "237937002"),
            (TAG_CURRENCY, "AUD"),
        ], 1);

        let defs = parse_secdef_responses(&msg);
        assert_eq!(defs.len(), 2, "both listings: {defs:?}");
        assert_eq!(defs[0].con_id, 756733);
        assert_eq!(defs[0].currency, "USD");
        assert_eq!(defs[1].con_id, 237937002);
        assert_eq!(defs[1].currency, "AUD", "and neither borrowed the other's fields");

        // The identifier block reuses the symbol tag without naming a
        // contract, so it starts no record.
        let with_ids = crate::protocol::fix::fix_build(&[
            (crate::protocol::fix::TAG_MSG_TYPE, "d"),
            (TAG_SECURITY_REQ_ID, "9"),
            (TAG_SYMBOL, "SPY"),
            (TAG_SECURITY_TYPE, "CS"),
            (TAG_IB_CON_ID, "756733"),
            (TAG_CURRENCY, "USD"),
            (454, "2"),
            (TAG_SYMBOL, "BBG"),
            (455, "BBG000BDTBL9"),
            (TAG_SYMBOL, "US"),
            (455, "US78462F1030"),
        ], 1);
        let defs = parse_secdef_responses(&with_ids);
        assert_eq!(defs.len(), 1, "one contract and two identifiers: {defs:?}");
        assert_eq!(defs[0].con_id, 756733);

        // One contract still reads as one.
        let single = crate::protocol::fix::fix_build(&[
            (crate::protocol::fix::TAG_MSG_TYPE, "d"),
            (TAG_SECURITY_REQ_ID, "8"),
            (TAG_SYMBOL, "AAPL"),
            (TAG_IB_CON_ID, "265598"),
            (TAG_CURRENCY, "USD"),
        ], 1);
        let defs = parse_secdef_responses(&single);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].con_id, 265598);
    }
}
