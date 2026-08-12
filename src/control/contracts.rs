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
/// The economic-value rule, stated on the definition as its own field.
pub const TAG_EV_RULE: u32 = 6858;
/// What the economic-value evaluation is multiplied by, stated as a number.
pub const TAG_EV_MULTIPLIER: u32 = 6859;
/// The venues SMART routes a contract to, comma separated, in the order whose
/// positions a quote's exchange bitmask refers to.
pub const TAG_SMART_VENUES: u32 = 6177;
/// The contract a derivative is written on: its id, its symbol and its kind.
///
/// Settled from a real option's definition rather than inferred: the id it
/// carried was the id of the share it is written on, and the symbol beside it
/// was that share's symbol.
pub const TAG_UNDERLYING_CON_ID: u32 = 6346;
pub const TAG_UNDERLYING_SYMBOL: u32 = 6855;
pub const TAG_UNDERLYING_SEC_TYPE: u32 = 310;
/// The time of day a contract stops trading, stated beside the date it stops.
pub const TAG_LAST_TRADE_TIME: u32 = 8584;
/// The day a bond was issued. Standard in this protocol, and stated only by
/// contracts that have an issuer — which is why asking about shares never saw
/// it.
pub const TAG_ISSUE_DATE: u32 = 225;
/// The smallest order the venue will take, and the flag that says a contract
/// can be dealt in fractions at all. Stated together; the size without the flag
/// is not in force.
pub const TAG_MIN_SIZE: u32 = 8175;
pub const TAG_FRACTIONABLE: u32 = 8193;
/// How many places the venue states a price and a size to.
pub const TAG_LAST_PRICE_PRECISION: u32 = 8598;
pub const TAG_LAST_SIZE_PRECISION: u32 = 8599;
/// The day a contract really stops trading, where that differs from the month
/// it is named for.
pub const TAG_REAL_EXPIRATION_DATE: u32 = 6614;
/// How a contract settles — by delivery or in cash.
pub const TAG_SETTLEMENT_METHOD: u32 = 6660;
pub const TAG_IB_STOCK_TYPE: u32 = 8077;

// Market rule tags.
pub const TAG_MARKET_RULE_START: u32 = 6019; // value "1" starts a new rule block
pub const TAG_MARKET_RULE_ID: u32 = 6031;    // rule ID integer
pub const TAG_LOW_EDGE: u32 = 6023;          // price increment threshold
pub const TAG_INCREMENT: u32 = 6027;         // tick size at that price level
/// Opens the table of price increments in a rule. What follows, until the size
/// table opens, is a low edge and an increment per price band.
pub const TAG_PRICE_INCREMENT_COUNT: u32 = 6026;
/// Opens the table of SIZE increments in the same rule, under the same low-edge
/// and increment tags as the price table. Treating this as the end of the rule
/// stopped before it, which is why a contract's size increment was never read.
pub const TAG_SIZE_INCREMENT_COUNT: u32 = 6030;

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
    /// What a quoted price must be multiplied by to be a price. A contract
    /// quoted in a hundredth of the currency states a hundred here, and a price
    /// read without it is out by that factor.
    /// What a bond is: its terms, its ratings, and the option on it. A caller
    /// asking about a bond received a contract with none of what makes it one.
    pub coupon: f64,
    pub contract_month: String,
    pub under_sec_type: String,
    /// The rule the venue evaluates a contract's economic value under. Sent on
    /// the definition, not derived: a contract whose value follows something
    /// other than its own price is priced wrongly without it.
    pub ev_rule: String,
    /// What that evaluation is multiplied by. Stated as a number in the tag
    /// beside the rule; a rule without its multiplier values the contract by
    /// the wrong factor, which is not a rounding error.
    pub ev_multiplier: f64,
    pub bond_notes: String,
    pub desc_append: String,
    pub bond_type: String,
    pub coupon_type: String,
    pub next_option_date: String,
    pub next_option_type: String,
    pub ratings: String,
    pub fund_name: String,
    pub fund_family: String,
    pub fund_type: String,
    pub fund_front_load: String,
    pub fund_back_load: String,
    pub fund_back_load_time_interval: String,
    pub fund_management_fee: String,
    pub fund_notify_amount: String,
    pub fund_minimum_initial_purchase: String,
    pub fund_minimum_subsequent_purchase: String,
    pub fund_blue_sky_states: String,
    pub fund_blue_sky_territories: String,
    pub fund_distribution_policy_indicator: String,
    pub fund_asset_type: String,
    pub real_expiration_date: String,
    pub callable: bool,
    pub puttable: bool,
    pub convertible: bool,
    pub next_option_partial: bool,
    pub fund_closed: bool,
    pub fund_closed_for_new_investors: bool,
    pub fund_closed_for_new_money: bool,
    pub agg_group: i32,
    pub price_magnifier: i32,
    /// What the issuer does, from broadest to narrowest. The venue states all
    /// three in one field separated by bars; a caller wants them apart.
    pub industry: String,
    pub category: String,
    pub subcategory: String,
    pub country: String,
    pub market_name: String,
    pub isin: String,
    /// The identifier a contract is known by in the American market. It has no
    /// field of its own on this wire — it is one of the identifiers below,
    /// picked out by its kind.
    pub cusip: String,
    /// Every identifier the contract is known by, as the kind and the value.
    pub sec_id_list: Vec<(String, String)>,
    /// Every field the venue stated that this parser does not name.
    ///
    /// A definition carries more than any one client reads, and what was read
    /// used to be the whole of what survived: the rest was parsed and dropped,
    /// so a fact the venue had stated about the contract could not be reached
    /// by anyone, and there was no way to tell it had been sent.
    ///
    /// Keeping them costs a short list per contract and means naming a field is
    /// an improvement rather than a prerequisite — the value is already here,
    /// under its number, the day the venue starts sending it.
    /// The venues SMART routes this contract to, in the order the venue lists
    /// them.
    ///
    /// The order is the point. A quote states which venues are on the bid, the
    /// ask and the last as a bitmask, and the position of a bit is a position
    /// in this list. A list written by this client can only guess at that, and
    /// the guess bore no resemblance to what the venue actually sends.
    ///
    /// Sent per contract, and only where SMART routing applies, so it is empty
    /// for a contract listed on one venue.
    /// The contract a derivative is written on.
    pub under_con_id: u32,
    pub under_symbol: String,
    /// The time of day trading stops, stated separately from the date.
    ///
    /// A caller holding only the date knows which day a contract stops trading
    /// and not when, and an option that stops at three in the afternoon is not
    /// one that stops at the close.
    pub last_trade_time: String,
    /// The day a bond was issued.
    pub issue_date: String,
    /// The size a contract may be dealt in, from the rule's size table.
    pub size_increment: f64,
    pub suggested_size_increment: f64,
    pub last_price_precision: f64,
    pub last_size_precision: f64,
    pub settlement_method: String,
    pub smart_venues: Vec<String>,
    pub unnamed_fields: Vec<(u32, String)>,
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
            // Not a penny by default. The smallest increment a contract moves
            // in is the venue's to state, and a great many contracts do not
            // move in pennies — most futures do not. Standing a penny in where
            // the venue stated nothing prices those contracts on a grid they
            // are not traded on, and says so with the same confidence as a
            // figure the venue gave.
            min_tick: 0.0,
            multiplier: 1.0,
            valid_exchanges: Vec::new(),
            order_types: Vec::new(),
            market_rule_id: None,
            last_trade_date: String::new(),
            strike: 0.0,
            right: None,
            stock_type: String::new(),
            coupon: 0.0,
            contract_month: String::new(),
            under_sec_type: String::new(),
            ev_rule: String::new(),
            ev_multiplier: 0.0,
            bond_notes: String::new(),
            desc_append: String::new(),
            bond_type: String::new(),
            coupon_type: String::new(),
            next_option_date: String::new(),
            next_option_type: String::new(),
            ratings: String::new(),
            fund_name: String::new(),
            fund_family: String::new(),
            fund_type: String::new(),
            fund_front_load: String::new(),
            fund_back_load: String::new(),
            fund_back_load_time_interval: String::new(),
            fund_management_fee: String::new(),
            fund_notify_amount: String::new(),
            fund_minimum_initial_purchase: String::new(),
            fund_minimum_subsequent_purchase: String::new(),
            fund_blue_sky_states: String::new(),
            fund_blue_sky_territories: String::new(),
            fund_distribution_policy_indicator: String::new(),
            fund_asset_type: String::new(),
            real_expiration_date: String::new(),
            callable: false,
            puttable: false,
            convertible: false,
            next_option_partial: false,
            fund_closed: false,
            fund_closed_for_new_investors: false,
            fund_closed_for_new_money: false,
            agg_group: 0,
            price_magnifier: 0,
            industry: String::new(),
            category: String::new(),
            subcategory: String::new(),
            country: String::new(),
            market_name: String::new(),
            isin: String::new(),
            cusip: String::new(),
            sec_id_list: Vec::new(),
            under_con_id: 0,
            under_symbol: String::new(),
            last_trade_time: String::new(),
            issue_date: String::new(),
            size_increment: 0.0,
            suggested_size_increment: 0.0,
            last_price_precision: 0.0,
            last_size_precision: 0.0,
            settlement_method: String::new(),
            smart_venues: Vec::new(),
            unnamed_fields: Vec::new(),
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
pub fn parse_secdef_responses(
    data: &[u8], island_for_nasdaq: bool,
) -> Vec<ContractDefinition> {
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
        return parse_secdef_response(data, island_for_nasdaq).into_iter().collect();
    }

    let header = &data[..starts[0]];
    let mut out = Vec::with_capacity(starts.len());
    // A contract's fields run from where it is named until the next contract is
    // named. The symbol tag is reused inside the identifier block — `55=BBG`
    // and `55=US` sit there — so an occurrence that states neither what the
    // contract is nor which contract it is does not begin a new one: it belongs
    // to the contract already open, and everything after it does too.
    //
    // Cutting there instead lost every field a contract stated after its own
    // identifier block. In a reply naming fifty bonds, forty-nine came back
    // holding three fields each while the last held twelve hundred, because
    // only the last had nothing following it to be cut by.
    let mut open: Option<Vec<u8>> = None;
    let flush = |open: &mut Option<Vec<u8>>, out: &mut Vec<ContractDefinition>| {
        if let Some(mut record) = open.take() {
            record.push(SOH);
            if let Some(def) = parse_secdef_response(&record, island_for_nasdaq) {
                out.push(def);
            }
        }
    };
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(data.len());
        let body = &data[start..end];
        let names_a_contract = contains_field(body, b"167=") && contains_field(body, b"6008=");
        if names_a_contract {
            flush(&mut open, &mut out);
            let mut record = Vec::with_capacity(header.len() + body.len() + 1);
            record.extend_from_slice(header);
            record.extend_from_slice(body);
            open = Some(record);
        } else if let Some(record) = open.as_mut() {
            record.extend_from_slice(body);
        }
    }
    flush(&mut open, &mut out);
    // Records that all name the same contract are one contract described once.
    // The identifier block repeats the symbol tag and states a type and an id
    // beside it, so a single definition can split in two, and a field the
    // message states once — the long name, the venues it may trade on — then
    // survives in whichever half it followed and is missing from the other.
    // Read whole, it is all there.
    if out.len() > 1 {
        let first = out[0].con_id;
        if out.iter().all(|d| d.con_id == first) {
            return parse_secdef_response(data, island_for_nasdaq).into_iter().collect();
        }
    }
    out
}

/// Whether a tag belongs to the message envelope rather than to the contract.
///
/// These are on every message the venue sends, so counting them among a
/// contract's own fields overstates what is unread — which it did, by ten.
fn is_session_field(tag: u32) -> bool {
    // 6344 is how many contracts the reply carries, which is a fact about the
    // reply and not about any contract in it: it read 1 for a share, 50 for a
    // bond lookup and 21 for a future's expiries.
    matches!(tag, 8 | 9 | 10 | 34 | 35 | 43 | 49 | 52 | 56 | 115 | 146 | 322 | 320 | 6344)
}

/// Every tag this parser reads from a definition.
///
/// Written out rather than derived, and checked against the parser by a test,
/// so that asking "what arrived that we did not read" has an answer that cannot
/// quietly drift as fields are added.
pub fn tags_read_from_a_definition() -> Vec<u32> {
    let source = include_str!("contracts.rs");
    let mut seen: Vec<u32> = Vec::new();
    // The parser reads a definition by looking tags up in one map, so every tag
    // it reads appears as a lookup on that map.
    for cap in source.split("tags.get(&").skip(1) {
        let token: String = cap.chars().take_while(|c| *c != ')').collect();
        let tag = token
            .trim()
            .parse::<u32>()
            .ok()
            .or_else(|| named_tag(token.trim()));
        if let Some(tag) = tag
            && !seen.contains(&tag)
        {
            seen.push(tag);
        }
    }
    seen.sort_unstable();
    seen
}

/// The value of a tag the parser refers to by name rather than by number.
fn named_tag(name: &str) -> Option<u32> {
    let source = include_str!("contracts.rs");
    let needle = format!("pub const {name}: u32 = ");
    let at = source.find(&needle)? + needle.len();
    let digits: String = source[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The tags a definition carried that nothing here reads.
///
/// The point of asking a venue for a contract is to be told about it, and a
/// field that arrives and is dropped is a fact about the contract nobody can
/// see. This names them so the gap is measurable rather than suspected.
pub fn unread_definition_tags(data: &[u8]) -> Vec<u32> {
    let read = tags_read_from_a_definition();
    let mut unread: Vec<u32> = fix::fix_parse(data)
        .keys()
        .copied()
        .filter(|t| !read.contains(t))
        .collect();
    unread.sort_unstable();
    unread.dedup();
    unread
}

pub fn parse_secdef_response(
    data: &[u8], island_for_nasdaq: bool,
) -> Option<ContractDefinition> {
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
    // The name a US stock on Nasdaq is handed back under. Set here, where the
    // definition is read, so every path that hands one to a caller carries it
    // rather than each remembering to. What goes back out routes under the
    // venue's own name regardless: `exchange_to_fix` translates it.
    let sec_type = def.sec_type.to_api_str();
    def.exchange = delivered_exchange(&def.exchange, sec_type, &def.currency, island_for_nasdaq);
    def.primary_exchange =
        delivered_exchange(&def.primary_exchange, sec_type, &def.currency, island_for_nasdaq);
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
    // tick. Absent a rule block it stays unset, because the venue stated none
    // and a penny is a guess that most futures fail.
    if let Some(min_increment) = parse_market_rules(data)
        .iter()
        .flat_map(|rule| rule.price_increments.iter())
        .map(|inc| inc.increment)
        .filter(|inc| *inc > 0.0)
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    {
        def.min_tick = min_increment;
    }

    // The smallest size the rule states, from its size table. Taken from the
    // rule the venue sent rather than assumed: a contract dealt in whole units
    // and one dealt in fractions state different tables.
    if let Some(size) = parse_market_rules(data)
        .iter()
        .flat_map(|r| r.size_increments.iter())
        .map(|b| b.increment)
        .find(|v| *v > 0.0)
    {
        def.size_increment = size;
        // The reference client publishes a suggested size beside the required
        // one. The venue states one table; where it states no separate
        // suggestion, the suggestion is the increment itself.
        def.suggested_size_increment = size;
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
    if let Some(v) = tags.get(&200) { def.contract_month = v.clone(); }
    if let Some(v) = tags.get(&6577) { def.under_sec_type = v.clone(); }
    if let Some(v) = tags.get(&TAG_EV_RULE) { def.ev_rule = v.clone(); }
    if let Some(v) = tags.get(&TAG_EV_MULTIPLIER) && let Ok(x) = v.trim().parse() {
        def.ev_multiplier = x;
    }
    if let Some(v) = tags.get(&TAG_UNDERLYING_CON_ID) {
        def.under_con_id = v.parse().unwrap_or(0);
    }
    if let Some(v) = tags.get(&TAG_UNDERLYING_SYMBOL) {
        def.under_symbol = v.clone();
    }
    if let Some(v) = tags.get(&TAG_UNDERLYING_SEC_TYPE)
        && def.under_sec_type.is_empty()
    {
        def.under_sec_type = v.clone();
    }
    if let Some(v) = tags.get(&TAG_LAST_TRADE_TIME) {
        def.last_trade_time = v.clone();
    }
    if let Some(v) = tags.get(&TAG_ISSUE_DATE) {
        def.issue_date = v.clone();
    }
    if let Some(v) = tags.get(&TAG_SMART_VENUES) {
        def.smart_venues = v
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| exchange_from_fix(s).to_string())
            .collect();
    }

    // Whatever the venue stated that nothing above names. Kept rather than
    // dropped: the fields a client has not got round to naming are still facts
    // about the contract, and a caller can read them under their number today.
    // Read from the bytes rather than from the parsed map. A definition
    // repeats tags — a market rule states an increment per price band, an
    // identifier group states one per identifier — and a map keeps only the
    // last of each. Keeping only the last is the same loss this field exists to
    // prevent, one layer down.
    let named = tags_read_from_a_definition();
    def.unnamed_fields = Vec::new();
    for part in data.split(|&b| b == fix::SOH) {
        if part.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(part);
        let Some((tag_str, value)) = text.split_once('=') else { continue };
        let Ok(tag) = tag_str.parse::<u32>() else { continue };
        if named.contains(&tag) || is_session_field(tag) {
            continue;
        }
        def.unnamed_fields.push((tag, value.to_string()));
    }
    def.unnamed_fields.sort_by_key(|(tag, _)| *tag);
    if let Some(v) = tags.get(&6493) { def.bond_notes = v.clone(); }
    if let Some(v) = tags.get(&6494) { def.desc_append = v.clone(); }
    if let Some(v) = tags.get(&6495) { def.bond_type = v.clone(); }
    if let Some(v) = tags.get(&6496) { def.coupon_type = v.clone(); }
    if let Some(v) = tags.get(&6501) { def.next_option_date = v.clone(); }
    if let Some(v) = tags.get(&6502) { def.next_option_type = v.clone(); }
    if let Some(v) = tags.get(&6720) { def.ratings = v.clone(); }
    if let Some(v) = tags.get(&6481) { def.fund_name = v.clone(); }
    if let Some(v) = tags.get(&6472) { def.fund_family = v.clone(); }
    if let Some(v) = tags.get(&6473) { def.fund_type = v.clone(); }
    if let Some(v) = tags.get(&6474) { def.fund_front_load = v.clone(); }
    if let Some(v) = tags.get(&6475) { def.fund_back_load = v.clone(); }
    if let Some(v) = tags.get(&6482) { def.fund_back_load_time_interval = v.clone(); }
    if let Some(v) = tags.get(&6476) { def.fund_management_fee = v.clone(); }
    if let Some(v) = tags.get(&6478) { def.fund_notify_amount = v.clone(); }
    if let Some(v) = tags.get(&6479) { def.fund_minimum_initial_purchase = v.clone(); }
    if let Some(v) = tags.get(&8505) { def.fund_minimum_subsequent_purchase = v.clone(); }
    if let Some(v) = tags.get(&8150) { def.fund_blue_sky_states = v.clone(); }
    if let Some(v) = tags.get(&8151) { def.fund_blue_sky_territories = v.clone(); }
    if let Some(v) = tags.get(&8502) { def.fund_distribution_policy_indicator = v.clone(); }
    if let Some(v) = tags.get(&8503) { def.fund_asset_type = v.clone(); }
    if let Some(v) = tags.get(&8383) { def.real_expiration_date = v.clone(); }
    // The smallest order the venue will take, stated only where a contract can
    // be dealt in fractions and gated by the flag that says so.
    //
    // This used to read 8598, which is the precision a price is stated to, not
    // a size at all: a share came back claiming its smallest order was a
    // millionth of a share.
    if tags.get(&TAG_FRACTIONABLE).map(|v| v.as_str()) == Some("1")
        && let Some(v) = tags.get(&TAG_MIN_SIZE)
    {
        def.min_size = v.parse().unwrap_or(0.0);
    }
    // How many places the venue states a price and a size to. Published by the
    // reference client and recorded here as computed rather than sent, which
    // was wrong — they are sent.
    if let Some(v) = tags.get(&TAG_LAST_PRICE_PRECISION) {
        def.last_price_precision = v.parse().unwrap_or(0.0);
    }
    if let Some(v) = tags.get(&TAG_LAST_SIZE_PRECISION) {
        def.last_size_precision = v.parse().unwrap_or(0.0);
    }
    // Only where the field this parser already reads states nothing: that one
    // was established earlier and is not displaced on the strength of a second
    // candidate carrying the same value.
    if def.real_expiration_date.is_empty()
        && let Some(v) = tags.get(&TAG_REAL_EXPIRATION_DATE)
    {
        def.real_expiration_date = v.clone();
    }
    if let Some(v) = tags.get(&TAG_SETTLEMENT_METHOD) {
        def.settlement_method = v.clone();
    }

    // Stated under its own field, or under the shorter one where that is all
    // the venue gives.
    if def.desc_append.is_empty()
        && let Some(v) = tags.get(&6853) { def.desc_append = v.clone(); }
    if let Some(v) = tags.get(&6497) { def.callable = v == "1" || v.eq_ignore_ascii_case("true"); }
    if let Some(v) = tags.get(&6498) { def.puttable = v == "1" || v.eq_ignore_ascii_case("true"); }
    if let Some(v) = tags.get(&6499) { def.convertible = v == "1" || v.eq_ignore_ascii_case("true"); }
    if let Some(v) = tags.get(&6500) { def.next_option_partial = v == "1" || v.eq_ignore_ascii_case("true"); }
    if let Some(v) = tags.get(&6477) { def.fund_closed = v == "1" || v.eq_ignore_ascii_case("true"); }
    if let Some(v) = tags.get(&6511) { def.fund_closed_for_new_investors = v == "1" || v.eq_ignore_ascii_case("true"); }
    if let Some(v) = tags.get(&6512) { def.fund_closed_for_new_money = v == "1" || v.eq_ignore_ascii_case("true"); }
    if let Some(v) = tags.get(&223) && let Ok(x) = v.trim().parse() { def.coupon = x; }
    if let Some(v) = tags.get(&6178) && let Ok(x) = v.trim().parse() { def.agg_group = x; }
    if let Some(v) = tags.get(&6021)
        && let Ok(n) = v.trim().parse::<i32>()
    {
        def.price_magnifier = n;
    }
    if let Some(v) = tags.get(&6624) {
        // Stated as one field with bars between: broadest first, then narrower.
        // Kept whole it read as a category nobody has, spelled with bars.
        let mut parts = v.split('|');
        def.industry = parts.next().unwrap_or_default().trim().to_string();
        def.category = parts.next().unwrap_or_default().trim().to_string();
        def.subcategory = parts.next().unwrap_or_default().trim().to_string();
        // A value stating only one thing is the category, which is what a
        // caller asking for one means by it.
        if def.category.is_empty() && def.subcategory.is_empty() {
            def.category = std::mem::take(&mut def.industry);
        }
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
    // The identifiers a contract is known by elsewhere, each stated as a value
    // followed by what kind of identifier it is. Read sequentially because a
    // keyed parse keeps only the last of a repeated tag, and this group repeats.
    //
    // A contract's CUSIP has no field of its own anywhere on this wire: it is
    // one of these, picked out by its kind. Only the ISIN was being picked out,
    // so a caller asking for a CUSIP got nothing while the CUSIP sat in a list
    // that was thrown away.
    {
        use crate::protocol::fix::SOH;
        let mut last_alt_id = String::new();
        for part in data.split(|&b| b == SOH) {
            let text = String::from_utf8_lossy(part);
            if let Some(val) = text.strip_prefix("455=") {
                last_alt_id = val.to_string();
            } else if let Some(kind) = text.strip_prefix("456=") {
                def.sec_id_list.push((kind.to_string(), last_alt_id.clone()));
                match kind {
                    "1" => def.cusip = std::mem::take(&mut last_alt_id),
                    "4" => def.isin = last_alt_id.clone(),
                    _ => {}
                }
            }
        }
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
    /// The size a contract may be dealt in, per size band.
    ///
    /// Stated in the same rule under the same tags as the price bands, after a
    /// count that opens a second table. Reading stopped at that count, so this
    /// was empty for every contract.
    pub size_increments: Vec<PriceIncrement>,
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

    /// Which of a rule's two tables the bands now arriving belong to. Both are
    /// stated under the same tags, and only the count that opened them says
    /// which is which.
    enum Table { Price, Size }

    let mut rules: Vec<MarketRule> = Vec::new();
    let mut current: Option<MarketRule> = None;
    let mut pending_low_edge: Option<f64> = None;
    let mut filling = Table::Price;

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
                    size_increments: Vec::new(),
                });
                pending_low_edge = None;
                filling = Table::Price;
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
                            let band = PriceIncrement { low_edge, increment };
                            match filling {
                                Table::Price => rule.price_increments.push(band),
                                Table::Size => rule.size_increments.push(band),
                            }
                        }
            }
            TAG_PRICE_INCREMENT_COUNT => {
                filling = Table::Price;
                pending_low_edge = None;
            }
            // Opens the size table. This used to end the rule, so everything
            // stated after it — the sizes a contract may be dealt in — was
            // never read.
            TAG_SIZE_INCREMENT_COUNT => {
                filling = Table::Size;
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
                (TAG_SIZE_INCREMENT_COUNT, "1"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg, true).unwrap();
        assert_eq!(def.con_id, 265598);
        assert_eq!(def.symbol, "AAPL");
        assert_eq!(def.sec_type, SecurityType::Stock);
        // Handed back under the name the counterpart hands it back under. What
        // goes out still routes under the venue's own name.
        assert_eq!(def.exchange, "ISLAND");
        assert_eq!(exchange_to_fix(&def.exchange), "NASDAQ");
        assert_eq!(def.currency, "USD");
        assert_eq!(def.long_name, "APPLE INC");
        assert_eq!(def.min_tick, 0.01);
        assert_eq!(def.valid_exchanges, vec!["SMART", "NYSE", "ARCA"]);
        assert_eq!(def.primary_exchange, "ISLAND");
    }

    #[test]
    fn parse_rejects_non_secdef() {
        let msg = fix::fix_build(&[(TAG_MSG_TYPE, "A")], 1);
        assert!(super::parse_secdef_response(&msg, true).is_none());
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
                (TAG_SIZE_INCREMENT_COUNT, "1"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg, true).unwrap();
        // Smallest increment across bands, not the "1" rule sentinel.
        assert_eq!(def.min_tick, 0.0001);
    }

    /// A definition stating no rule block states no smallest increment, and
    /// none is invented for it. A penny used to stand in, which prices most
    /// futures on a grid they are not traded on and says so as confidently as
    /// a figure the venue gave.
    #[test]
    fn a_definition_stating_no_rule_states_no_increment() {
        let msg = fix::fix_build(
            &[
                (TAG_MSG_TYPE, "d"),
                (TAG_IB_CON_ID, "265598"),
                (TAG_SYMBOL, "AAPL"),
                (TAG_SECURITY_TYPE, "CS"),
            ],
            1,
        );
        let def = super::parse_secdef_response(&msg, true).unwrap();
        assert_eq!(def.min_tick, 0.0, "unstated, not a penny");
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
        let def = super::parse_secdef_response(&msg, true).unwrap();
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
        let def = super::parse_secdef_response(&msg, true).unwrap();
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
        let def = super::parse_secdef_response(&msg, true).unwrap();
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
        let def = super::parse_secdef_response(&msg, true).unwrap();
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
        let def = super::parse_secdef_response(&msg, true).unwrap();
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
                (TAG_SIZE_INCREMENT_COUNT, "1"),
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
                (TAG_SIZE_INCREMENT_COUNT, "1"),
                // Rule 2: nickel increments above $1
                (TAG_MARKET_RULE_START, "1"),
                (TAG_MARKET_RULE_ID, "42"),
                (TAG_LOW_EDGE, "0"),
                (TAG_INCREMENT, "0.01"),
                (TAG_LOW_EDGE, "1"),
                (TAG_INCREMENT, "0.05"),
                (TAG_SIZE_INCREMENT_COUNT, "1"),
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

        let defs = parse_secdef_responses(&msg, true);
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
        let defs = parse_secdef_responses(&with_ids, true);
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
        let defs = parse_secdef_responses(&single, true);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].con_id, 265598);
    }
}

#[cfg(test)]
mod industry_tests {
    use super::*;

    fn secdef(extra: &str) -> Vec<u8> {
        format!("35=d\u{1}320=R1\u{1}6008=756733\u{1}55=SPY\u{1}167=CS\u{1}\
                 207=SMART\u{1}15=USD\u{1}{extra}")
            .into_bytes()
    }

    /// A contract whose economic value follows something other than its own
    /// price says so on its definition. The field was arriving and going
    /// nowhere, so a caller pricing such a contract had nothing to price it by.
    #[test]
    fn the_economic_value_rule_is_read_from_the_definition() {
        let def = parse_secdef_response(&secdef("6858=IND-FUT-CASH\u{1}6859=0.25\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.ev_rule, "IND-FUT-CASH");
        assert_eq!(def.ev_multiplier, 0.25, "a rule without its multiplier values the contract wrongly");
    }

    /// The venue states what the issuer does as one field with bars between,
    /// broadest first. Kept whole, a caller asking for the category was handed
    /// all three of them with bars in the middle.
    #[test]
    fn what_the_issuer_does_arrives_as_three_things() {
        let def = parse_secdef_response(&secdef("6624=Technology|Computers|Computers\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.industry, "Technology");
        assert_eq!(def.category, "Computers");
        assert_eq!(def.subcategory, "Computers");
    }

    /// A contract's CUSIP has no field of its own on this wire: it is one of
    /// the identifiers the contract is known by, picked out by its kind. Only
    /// the ISIN was picked out, so a caller asking for a CUSIP got nothing
    /// while the CUSIP sat in a list that was thrown away.
    #[test]
    fn the_identifiers_are_kept_and_the_cusip_picked_out_of_them() {
        let def = parse_secdef_response(&secdef(
            "455=US0378331005\u{1}456=4\u{1}455=037833100\u{1}456=1\u{1}",
        ), true).expect("the definition parses");
        assert_eq!(def.isin, "US0378331005");
        assert_eq!(def.cusip, "037833100", "the CUSIP is in the list, by its kind");
        assert_eq!(def.sec_id_list.len(), 2, "and every identifier is kept");
    }

    /// A bond is its terms: what it pays, when it can be called, what it is
    /// rated. Read from nothing, a caller asking about a bond received a
    /// contract with none of what makes it one.
    #[test]
    fn a_bond_carries_its_terms() {
        let def = parse_secdef_response(&secdef(
            "223=4.25\u{1}6495=CORP\u{1}6496=FIXED\u{1}6497=1\u{1}6498=0\u{1}\
             6499=0\u{1}6501=20270115\u{1}6502=CALL\u{1}6720=A+\u{1}6493=notes\u{1}",
        ), true).expect("the definition parses");
        assert_eq!(def.coupon, 4.25);
        assert_eq!(def.bond_type, "CORP");
        assert_eq!(def.coupon_type, "FIXED");
        assert!(def.callable, "this one can be called");
        assert!(!def.puttable, "and cannot be put");
        assert!(!def.convertible);
        assert_eq!(def.next_option_date, "20270115");
        assert_eq!(def.next_option_type, "CALL");
        assert_eq!(def.ratings, "A+");
        assert_eq!(def.bond_notes, "notes");
    }

    /// A fund is what it charges and what it is closed to. Without those it is
    /// a symbol.
    #[test]
    fn a_fund_carries_what_it_charges_and_what_it_is_closed_to() {
        let def = parse_secdef_response(&secdef(
            "6481=Some Fund\u{1}6472=Some Family\u{1}6473=Bond\u{1}6474=1.5\u{1}\
             6475=0.5\u{1}6476=0.75\u{1}6477=0\u{1}6511=1\u{1}6512=1\u{1}\
             8150=NY,CA\u{1}8503=Fixed Income\u{1}",
        ), true).expect("the definition parses");
        assert_eq!(def.fund_name, "Some Fund");
        assert_eq!(def.fund_family, "Some Family");
        assert_eq!(def.fund_type, "Bond");
        assert_eq!(def.fund_front_load, "1.5");
        assert_eq!(def.fund_management_fee, "0.75");
        assert!(!def.fund_closed, "open to existing holders");
        assert!(def.fund_closed_for_new_investors, "and closed to new ones");
        assert!(def.fund_closed_for_new_money);
        assert_eq!(def.fund_blue_sky_states, "NY,CA");
        assert_eq!(def.fund_asset_type, "Fixed Income");
    }

    /// The venue states a short description under its own field where it has
    /// one, and under a shorter field where that is all it gives.
    #[test]
    fn a_description_falls_back_to_the_shorter_field() {
        let def = parse_secdef_response(&secdef("6853=short form\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.desc_append, "short form");
    }

    /// A price quoted in a fraction of the currency is out by that fraction
    /// unless the multiplier comes with it.
    #[test]
    fn a_price_carries_what_it_must_be_multiplied_by() {
        let def = parse_secdef_response(&secdef("6021=100\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.price_magnifier, 100);
    }

    /// A value stating one thing is the category, which is what a caller asking
    /// for a category means by it.
    #[test]
    fn one_thing_stated_is_the_category() {
        let def = parse_secdef_response(&secdef("6624=Financial\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.category, "Financial");
        assert!(def.industry.is_empty() && def.subcategory.is_empty());
    }
}

#[cfg(test)]
mod unread_tag_tests {
    use super::*;

    /// The list of tags the parser reads is derived from the parser itself, so
    /// it cannot fall behind as fields are added.
    #[test]
    fn the_tags_read_include_the_ones_known_to_be_read() {
        let read = tags_read_from_a_definition();
        for known in [TAG_IB_CON_ID, TAG_EV_RULE, 6577, 6624] {
            assert!(read.contains(&known), "{known} is read but not reported as read");
        }
        assert!(read.len() > 40, "only {} tags reported as read", read.len());
    }

    /// A tag the venue sends that nothing reads is named, so the gap is
    /// measurable rather than suspected.
    #[test]
    fn a_tag_that_arrives_and_is_dropped_is_named() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x019999=something\x01";
        let unread = unread_definition_tags(frame);
        assert!(unread.contains(&9999), "an unread tag was not reported");
        assert!(!unread.contains(&6008), "a tag that is read was reported unread");
    }
}

#[cfg(test)]
mod unnamed_field_tests {
    use super::*;

    fn frame(extra: &str) -> Vec<u8> {
        format!("35=d\u{1}320=R1\u{1}6008=756733\u{1}55=SPY\u{1}167=CS\u{1}\
                 207=SMART\u{1}15=USD\u{1}{extra}").into_bytes()
    }

    /// A field the venue stated and this client does not name is still a fact
    /// about the contract. Dropping it put it beyond reach with nothing to say
    /// it had ever arrived.
    #[test]
    fn a_field_this_client_does_not_name_is_kept_under_its_number() {
        // Numbers this parser names none of, so the test does not go stale the
        // day one of them is given a name.
        let def = parse_secdef_response(&frame("9998=something\u{1}9997=42\u{1}"), true)
            .expect("the definition parses");
        let kept: Vec<u32> = def.unnamed_fields.iter().map(|(t, _)| *t).collect();
        assert!(kept.contains(&9998));
        assert!(kept.contains(&9997));
        let value = def.unnamed_fields.iter().find(|(t, _)| *t == 9998).unwrap();
        assert_eq!(value.1, "something");
    }

    /// The message envelope is not part of the contract. Counting those among
    /// its fields overstated what was unread, by ten.
    #[test]
    fn the_messages_own_fields_are_not_counted_as_the_contracts() {
        let def = parse_secdef_response(&frame(""), true).expect("the definition parses");
        let kept: Vec<u32> = def.unnamed_fields.iter().map(|(t, _)| *t).collect();
        for envelope in [8, 9, 10, 34, 35, 52, 320] {
            assert!(!kept.contains(&envelope), "{envelope} is the message's, not the contract's");
        }
    }

    /// A field that is named is read into its own place, not left as a number.
    #[test]
    fn a_field_this_client_names_does_not_also_appear_unnamed() {
        let def = parse_secdef_response(&frame("6858=IND-FUT-CASH\u{1}6859=0.25\u{1}"), true)
            .expect("the definition parses");
        assert_eq!(def.ev_rule, "IND-FUT-CASH");
        assert_eq!(def.ev_multiplier, 0.25, "a rule without its multiplier values the contract wrongly");
        for named in [TAG_EV_RULE, TAG_EV_MULTIPLIER] {
            assert!(!def.unnamed_fields.iter().any(|(t, _)| *t == named), "tag {named} is named");
        }
    }
}

#[cfg(test)]
mod smart_venue_tests {
    use super::*;

    /// The venue states which venues SMART routes a contract to, and the order
    /// it states them in is the order a quote's exchange bitmask refers to.
    /// This client had its own list in its own order, which bore no
    /// resemblance, so every quote's bid, ask and last named the wrong venue.
    #[test]
    fn the_venues_own_routing_list_is_read_in_the_order_it_states() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01\
                      6177=AMEX,NYSE,CHX,ARCA,NASDAQ,DRCTEDGE,BEX,BATS,EDGE\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(
            def.smart_venues,
            vec!["AMEX", "NYSE", "CHX", "ARCA", "NASDAQ", "DRCTEDGE", "BEX", "BATS", "EDGE"],
        );
    }

    /// A contract listed on one venue is routed nowhere, and the venue sends
    /// no list for it. That is empty, not missing.
    #[test]
    fn a_contract_that_is_not_smart_routed_lists_no_venues() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=VOD\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert!(def.smart_venues.is_empty());
    }

    /// A venue's letter is the server's to state, and nothing is stated for
    /// one it has not named.
    ///
    /// This client used to answer from a table of eight venues written into
    /// its own source. That table named nothing for every other venue — most
    /// of the United States, and all of everywhere else — and nothing checked
    /// it against what the server assigns. The counterpart carries no such
    /// table either: it reads the map off the wire.
    #[test]
    fn a_venues_letter_is_not_this_clients_to_invent() {
        use crate::types::exchange_letter;
        assert_eq!(exchange_letter("NASDAQ"), "");
        assert_eq!(exchange_letter("SOMEWHERE"), "");
    }
}

#[cfg(test)]
mod underlying_tests {
    use super::*;

    /// An option is written on something, and its definition says what. These
    /// were settled from a real reply rather than inferred: the id the venue
    /// sent was the id of the share the option is written on, and the symbol
    /// beside it was that share's symbol.
    #[test]
    fn a_derivative_names_the_contract_it_is_written_on() {
        let frame = b"35=d\x01320=R1\x016008=36233584\x0155=SPY\x01167=OPT\x01\
                      6346=756733\x016855=SPY\x01310=STK\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.under_con_id, 756733);
        assert_eq!(def.under_symbol, "SPY");
        assert_eq!(def.under_sec_type, "STK");
        // And it is not the contract's own id.
        assert_ne!(def.under_con_id, def.con_id);
    }

    /// Trading stops at a time of day, not only on a date. A caller holding
    /// the date alone knows which day a contract stops and not when, and an
    /// option that stops at three in the afternoon is not one that stops at
    /// the close.
    #[test]
    fn the_time_trading_stops_is_read_as_well_as_the_date() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=SPY\x018583=20260918\x018584=150000\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.last_trade_time, "150000");
    }

    /// A share is written on nothing, and says so by sending none of them.
    #[test]
    fn a_contract_written_on_nothing_names_nothing() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01167=CS\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.under_con_id, 0);
        assert!(def.under_symbol.is_empty());
    }
}

#[cfg(test)]
mod issue_date_tests {
    use super::*;

    /// A bond was issued on a day, and its definition states which. Only a
    /// contract with an issuer states it, which is why asking about shares
    /// never saw the field and it looked absent from the wire.
    #[test]
    fn a_bond_states_the_day_it_was_issued() {
        let frame = b"35=d\x01320=R1\x016008=851160433\x0155=IBM\x01167=CORP\x01225=20260203\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.issue_date, "20260203");
    }

    /// A share has no issuer in this sense and states none, which is empty
    /// rather than missing.
    #[test]
    fn a_share_states_no_issue_date() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01167=CS\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert!(def.issue_date.is_empty());
    }
}

#[cfg(test)]
mod repeated_field_tests {
    use super::*;

    /// A definition repeats tags: a market rule states an increment per price
    /// band. Keeping only the last of each turns a schedule of tick sizes into
    /// a single number with no way to tell it was ever a schedule.
    #[test]
    fn a_tag_stated_more_than_once_is_kept_more_than_once() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=SPY\x01\
                      9001=alpha\x019001=beta\x019001=gamma\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        let values: Vec<&str> = def
            .unnamed_fields
            .iter()
            .filter(|(t, _)| *t == 9001)
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(values, vec!["alpha", "beta", "gamma"]);
    }
}

#[cfg(test)]
mod record_boundary_tests {
    use super::*;

    /// A contract's fields run until the next contract is named. The symbol tag
    /// is reused inside the identifier block, and treating that as the start of
    /// a new contract cut every contract short: in a reply naming fifty bonds,
    /// forty-nine came back holding almost nothing while the last held
    /// everything, because only the last had nothing after it to be cut by.
    #[test]
    fn a_contract_keeps_what_it_states_after_its_own_identifier_block() {
        let data = b"35=d\x01320=R1\x01\
                     55=AAA\x01167=CORP\x016008=111\x01\
                     55=BBG\x01456=A\x01\
                     225=20260101\x01\
                     55=ZZZ\x01167=CORP\x016008=222\x01\
                     225=20270202\x01";
        let defs = parse_secdef_responses(data, true);
        assert_eq!(defs.len(), 2, "two contracts are named");
        assert_eq!(defs[0].con_id, 111);
        assert_eq!(defs[1].con_id, 222);
        // The field stated after the first contract's identifier block belongs
        // to that contract, and used to be dropped.
        assert_eq!(defs[0].issue_date, "20260101");
        assert_eq!(defs[1].issue_date, "20270202");
    }

    /// A field stated after the last contract still belongs to it.
    #[test]
    fn the_last_contract_keeps_what_follows_it() {
        let data = b"35=d\x01320=R1\x01\
                     55=AAA\x01167=CORP\x016008=111\x01\
                     55=BBG\x01456=A\x01225=20260101\x01";
        let defs = parse_secdef_responses(data, true);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].issue_date, "20260101");
    }
}

#[cfg(test)]
mod size_and_precision_tests {
    use super::*;

    /// The smallest order a venue will take is stated only where a contract can
    /// be dealt in fractions, and gated by the flag that says so. This used to
    /// be read from the field stating how many places a price is quoted to, so
    /// a share came back claiming its smallest order was a millionth of a share.
    #[test]
    fn the_smallest_order_is_read_from_the_size_field_not_a_price_precision() {
        let frame = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01\
                      8193=1\x018175=0.0001\x018598=0.01\x018599=0.000001\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.min_size, 0.0001);
        assert_eq!(def.last_price_precision, 0.01);
        assert_eq!(def.last_size_precision, 0.000001);
    }

    /// A contract that cannot be dealt in fractions states no smallest order,
    /// and the size is not in force without the flag that admits it.
    #[test]
    fn a_size_stated_without_the_flag_is_not_in_force() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=SPY\x018175=0.0001\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.min_size, 0.0);
    }

    /// How a contract settles, and the day it really stops trading.
    #[test]
    fn how_a_contract_settles_is_read() {
        let frame = b"35=d\x01320=R1\x016008=1\x0155=ES\x016660=C\x016614=20260918\x01";
        let def = parse_secdef_response(frame, true).expect("the definition parses");
        assert_eq!(def.settlement_method, "C");
        assert_eq!(def.real_expiration_date, "20260918");
    }
}

#[cfg(test)]
mod size_table_tests {
    use super::*;

    /// A rule states two tables under the same tags: price bands first, then
    /// size bands after a second count. That count used to end the rule, so
    /// everything after it — the sizes a contract may be dealt in — was never
    /// read, and the last price band was silently the last thing seen.
    #[test]
    fn a_rules_size_table_is_read_as_well_as_its_price_table() {
        let data = b"35=d\x01320=R1\x016008=756733\x0155=SPY\x01\
                     6019=1\x016031=26\x01\
                     6026=1\x016023=0\x016027=0.01\x01\
                     6030=1\x016023=0\x016027=40\x01";
        let rules = parse_market_rules(data);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].price_increments.len(), 1, "the price band is read");
        assert_eq!(rules[0].price_increments[0].increment, 0.01);
        assert_eq!(rules[0].size_increments.len(), 1, "the size band is read");
        assert_eq!(rules[0].size_increments[0].increment, 40.0);

        let def = parse_secdef_response(data, true).expect("the definition parses");
        assert_eq!(def.min_tick, 0.01, "the price table gives the tick");
        assert_eq!(def.size_increment, 40.0, "the size table gives the size");
    }

    /// A rule stating only price bands leaves the size unset rather than
    /// borrowing the price increment for it.
    #[test]
    fn a_rule_with_no_size_table_states_no_size() {
        let data = b"35=d\x01320=R1\x016008=1\x0155=SPY\x01\
                     6019=1\x016031=26\x016026=1\x016023=0\x016027=0.05\x01";
        let def = parse_secdef_response(data, true).expect("the definition parses");
        assert_eq!(def.min_tick, 0.05);
        assert_eq!(def.size_increment, 0.0);
    }
}

/// The name a US stock trading on Nasdaq is handed back under.
///
/// The counterpart hands it back under the older spelling, and a program
/// written against it compares against that spelling. It does so when the
/// venue has turned the translation on for the session — which it does — and
/// when its own setting says to, which is its default.
///
/// Only US stocks: the older name means nothing for a future, and nothing
/// outside the United States trades under it.
pub fn delivered_exchange(
    exchange: &str, sec_type: &str, currency: &str, island_for_nasdaq: bool,
) -> String {
    let stock = sec_type.eq_ignore_ascii_case("STK");
    let american = currency.eq_ignore_ascii_case("USD");
    if stock && american && exchange.eq_ignore_ascii_case("NASDAQ") && island_for_nasdaq {
        return "ISLAND".to_string();
    }
    exchange.to_string()
}




#[cfg(test)]
mod delivered_name_tests {
    use super::delivered_exchange;

    /// A US stock on Nasdaq is handed back under the older spelling, the way
    /// the counterpart hands it back — so a program written against that one
    /// compares the same here.
    #[test]
    fn a_us_stock_on_nasdaq_is_handed_back_as_island() {
        assert_eq!(delivered_exchange("NASDAQ", "STK", "USD", true), "ISLAND");
    }

    /// Nothing else is. The older name means nothing for a future, and
    /// nothing outside the United States trades under it.
    #[test]
    fn nothing_else_is_renamed() {
        assert_eq!(delivered_exchange("NASDAQ", "FUT", "USD", true), "NASDAQ");
        assert_eq!(delivered_exchange("NASDAQ", "STK", "CAD", true), "NASDAQ");
        assert_eq!(delivered_exchange("ARCA", "STK", "USD", true), "ARCA");
        assert_eq!(delivered_exchange("", "STK", "USD", true), "");
    }

    /// A session that wants the venue's own name says so, and says it for
    /// itself: this used to be read from the process, so one session saying it
    /// said it for every other session running beside it.
    #[test]
    fn the_venues_own_name_can_be_asked_for() {
        assert_eq!(delivered_exchange("NASDAQ", "STK", "USD", false), "NASDAQ");
        assert_eq!(delivered_exchange("NASDAQ", "STK", "USD", true), "ISLAND");
    }

    /// And what goes out still routes under the venue's own name, whatever a
    /// caller was handed: the older spelling reaches nothing.
    #[test]
    fn what_goes_out_still_routes_as_nasdaq() {
        assert_eq!(super::exchange_to_fix("ISLAND"), "NASDAQ");
    }
}
