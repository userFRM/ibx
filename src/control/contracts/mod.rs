//! Contract/security definition lookups via the auth connection.
//!
//! Key tag mappings: STK→CS (SecurityType), SMART→BEST (Exchange).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::protocol::fix::{self, TAG_MSG_TYPE};

// Tags for security definitions
/// FIX tag 320: the security req id.
pub const TAG_SECURITY_REQ_ID: u32 = 320;
/// FIX tag 321: the security req type.
pub const TAG_SECURITY_REQ_TYPE: u32 = 321;
/// FIX tag 323: the security response type.
pub const TAG_SECURITY_RESPONSE_TYPE: u32 = 323;
/// FIX tag 55: the symbol.
pub const TAG_SYMBOL: u32 = 55;
/// FIX tag 167: the security type.
pub const TAG_SECURITY_TYPE: u32 = 167;
/// FIX tag 100: the exchange.
pub const TAG_EXCHANGE: u32 = 100;
/// FIX tag 15: the currency.
pub const TAG_CURRENCY: u32 = 15;
/// FIX tag 200: the last trade date.
pub const TAG_LAST_TRADE_DATE: u32 = 200;
/// MaturityDate. Carries a full expiry date where 200 carries a contract
/// month, so a definition that states one states it here.
pub const TAG_MATURITY_DATE: u32 = 541;
/// FIX tag 201: the right.
pub const TAG_RIGHT: u32 = 201;
/// FIX tag 202: the strike.
pub const TAG_STRIKE: u32 = 202;
/// FIX tag 207: the security exchange.
pub const TAG_SECURITY_EXCHANGE: u32 = 207;
/// FIX tag 231: the multiplier.
pub const TAG_MULTIPLIER: u32 = 231;
/// FIX tag 306: the long name.
pub const TAG_LONG_NAME: u32 = 306;
/// FIX tag 455: the security id.
pub const TAG_SECURITY_ID: u32 = 455;
/// FIX tag 456: the security id source.
pub const TAG_SECURITY_ID_SOURCE: u32 = 456;

// IB custom tags
/// FIX tag 6008: the con id.
pub const TAG_IB_CON_ID: u32 = 6008;
/// FIX tag 6035: the local symbol.
pub const TAG_IB_LOCAL_SYMBOL: u32 = 6035;
/// FIX tag 6046: the valid exchanges.
pub const TAG_IB_VALID_EXCHANGES: u32 = 6046;
/// FIX tag 6058: the trading class.
pub const TAG_IB_TRADING_CLASS: u32 = 6058;
/// FIX tag 6088: the source.
pub const TAG_IB_SOURCE: u32 = 6088;
/// FIX tag 6470: the primary exchange.
pub const TAG_IB_PRIMARY_EXCHANGE: u32 = 6470;
/// FIX tag 6431: the order types.
pub const TAG_IB_ORDER_TYPES: u32 = 6431;
/// FIX tag 6031: the market rule id.
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
/// FIX tag 6855: the underlying symbol.
pub const TAG_UNDERLYING_SYMBOL: u32 = 6855;
/// FIX tag 310: the underlying sec type.
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
/// FIX tag 8193: the fractionable.
pub const TAG_FRACTIONABLE: u32 = 8193;
/// How many places the venue states a price and a size to.
pub const TAG_LAST_PRICE_PRECISION: u32 = 8598;
/// FIX tag 8599: the last size precision.
pub const TAG_LAST_SIZE_PRECISION: u32 = 8599;
/// The day a contract really stops trading, where that differs from the month
/// it is named for.
pub const TAG_REAL_EXPIRATION_DATE: u32 = 6614;
/// How a contract settles — by delivery or in cash.
pub const TAG_SETTLEMENT_METHOD: u32 = 6660;
/// FIX tag 8077: the stock type.
pub const TAG_IB_STOCK_TYPE: u32 = 8077;

// Market rule tags.
/// value "1" starts a new rule block
pub const TAG_MARKET_RULE_START: u32 = 6019;
/// rule ID integer
pub const TAG_MARKET_RULE_ID: u32 = 6031;
/// price increment threshold
pub const TAG_LOW_EDGE: u32 = 6023;
/// tick size at that price level
pub const TAG_INCREMENT: u32 = 6027;
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
    /// CS
    Stock,
    /// OPT
    Option,
    /// FUT
    Future,
    /// CASH
    Forex,
    /// IND
    Index,
    /// BOND
    Bond,
    /// WAR
    Warrant,
    /// FOP
    FutureOption,
    /// CFD
    Cfd,
    /// CMDTY
    Commodity,
    /// FUND
    Fund,
    /// FWD
    Forward,
    /// BILL
    Bill,
    /// BAG
    Combo,
    /// CRYPTO
    Crypto,
    /// FIXED
    FixedIncome,
    /// SLB
    SecuritiesLending,
    /// NEWS
    News,
    /// BSK
    Basket,
    /// IOPT
    IndexOption,
    /// ICU
    IcuContract,
    /// ICS
    IcsContract,
    /// PHYSS
    PhysicalSettlement,
    /// Anything the venue named that this client does not.
    Other,
}

impl SecurityType {
    /// The official API string: `STK`, `OPT`, and so on.
    ///
    /// The one mapping everything user-visible reads, so a contract a callback
    /// hands back can be fed straight into another call. A name no request
    /// path accepts would make the returned contract unusable.
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
            // that misroutes the request silently. Empty draws a
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
    /// A call.
    Call,
    /// A put.
    Put,
}

/// Full contract definition.
#[derive(Debug, Clone)]
pub struct ContractDefinition {
    /// The venue's own id for this contract.
    pub con_id: u32,
    /// Its ticker.
    pub symbol: String,
    /// What kind of contract it is.
    pub sec_type: SecurityType,
    /// Where it is routed.
    pub exchange: String,
    /// Where it is listed.
    pub primary_exchange: String,
    /// What it is priced in.
    pub currency: String,
    /// The venue's own name for it.
    pub local_symbol: String,
    /// Which class within its chain.
    pub trading_class: String,
    /// The issuer's full name.
    pub long_name: String,
    /// The smallest its price can move.
    pub min_tick: f64,
    /// How many units one contract is worth.
    pub multiplier: f64,
    /// Every venue it can be routed to.
    pub valid_exchanges: Vec<String>,
    /// Which order types the venue takes for it.
    pub order_types: Vec<String>,
    /// Which price ladder it trades on.
    pub market_rule_id: Option<u32>,
    // Options/futures specific
    /// The last day it trades.
    pub last_trade_date: String,
    /// An option's strike.
    pub strike: f64,
    /// `C` or `P`.
    pub right: Option<OptionRight>,
    // Extended fields
    /// What kind of share it is.
    pub stock_type: String,
    /// What a quoted price must be multiplied by to be a price. A contract
    /// quoted in a hundredth of the currency states a hundred here, and a price
    /// read without it is out by that factor.
    /// What a bond is: its terms, its ratings, and the option on it. A caller
    /// asking about a bond received a contract with none of what makes it one.
    pub coupon: f64,
    /// A future's delivery month.
    pub contract_month: String,
    /// What kind of contract the underlying is.
    pub under_sec_type: String,
    /// The rule the venue evaluates a contract's economic value under. Sent on
    /// the definition, not derived: a contract whose value follows something
    /// other than its own price is priced wrongly without it.
    pub ev_rule: String,
    /// What that evaluation is multiplied by. Stated as a number in the tag
    /// beside the rule; a rule without its multiplier values the contract by
    /// the wrong factor, which is not a rounding error.
    pub ev_multiplier: f64,
    /// What the venue notes about a bond.
    pub bond_notes: String,
    /// What it appends to the description.
    pub desc_append: String,
    /// What kind of bond it is.
    pub bond_type: String,
    /// How its coupon is set.
    pub coupon_type: String,
    /// When the next call or put may be exercised.
    pub next_option_date: String,
    /// Which of the two it is.
    pub next_option_type: String,
    /// What the agencies rate it.
    pub ratings: String,
    /// A fund's name.
    pub fund_name: String,
    /// The family it belongs to.
    pub fund_family: String,
    /// What kind of fund it is.
    pub fund_type: String,
    /// What it charges on the way in.
    pub fund_front_load: String,
    /// What it charges on the way out.
    pub fund_back_load: String,
    /// Over what period that exit charge falls away.
    pub fund_back_load_time_interval: String,
    /// What it charges to run.
    pub fund_management_fee: String,
    /// The amount above which it asks to be told in advance.
    pub fund_notify_amount: String,
    /// The least that may be bought to open.
    pub fund_minimum_initial_purchase: String,
    /// The least that may be added.
    pub fund_minimum_subsequent_purchase: String,
    /// Which US states it may be sold in.
    pub fund_blue_sky_states: String,
    /// And which territories.
    pub fund_blue_sky_territories: String,
    /// Whether it distributes income or accumulates it.
    pub fund_distribution_policy_indicator: String,
    /// What it holds.
    pub fund_asset_type: String,
    /// When it actually expires, where that differs from its last trading day.
    pub real_expiration_date: String,
    /// Whether the issuer may redeem it early.
    pub callable: bool,
    /// Whether the holder may demand redemption.
    pub puttable: bool,
    /// Whether it converts to equity.
    pub convertible: bool,
    /// Whether that call or put redeems part of the principal rather than
    /// all of it.
    pub next_option_partial: bool,
    /// Whether it is closed.
    pub fund_closed: bool,
    /// Whether it is closed to new investors.
    pub fund_closed_for_new_investors: bool,
    /// Whether it is closed to new money from existing ones.
    pub fund_closed_for_new_money: bool,
    /// Which group of venues its book aggregates into.
    pub agg_group: i32,
    /// What a quoted price is multiplied by to reach money. Not one for
    /// every contract, and a price read without it is wrong by that factor.
    pub price_magnifier: i32,
    /// What the issuer does, from broadest to narrowest. The venue states all
    /// three in one field separated by bars; a caller wants them apart.
    pub industry: String,
    /// What sector the issuer is in.
    pub category: String,
    /// More narrowly.
    pub subcategory: String,
    /// Where the issuer is.
    pub country: String,
    /// The venue's own name for its market.
    pub market_name: String,
    /// Its ISIN.
    pub isin: String,
    /// The identifier a contract is known by in the American market. It has no
    /// field of its own on this wire — it is one of the identifiers below,
    /// picked out by its kind.
    pub cusip: String,
    /// Every identifier the contract is known by, as the kind and the value.
    pub sec_id_list: Vec<(String, String)>,
    /// The contract a derivative is written on.
    pub under_con_id: u32,
    /// The underlying's ticker.
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
    /// What the venue suggests trading in.
    pub suggested_size_increment: f64,
    /// How many decimal places its prices carry.
    pub last_price_precision: f64,
    /// How many its sizes carry.
    pub last_size_precision: f64,
    /// How it settles: physically, or in cash.
    pub settlement_method: String,
    /// The venues SMART routes it to, in the order a quote's exchange mask
    /// refers to them.
    ///
    /// The order is the point. A quote states which venues are on the bid, the
    /// ask and the last as a bitmask, and the position of a bit is a position
    /// in this list. A list written by this client can only guess at that, and
    /// the guess bore no resemblance to what the venue actually sends.
    ///
    /// Sent per contract, and only where SMART routing applies, so it is empty
    /// for a contract listed on one venue.
    pub smart_venues: Vec<String>,
    /// Every field the venue stated that this client does not name, kept
    /// under its tag number so nothing the venue said is discarded.
    pub unnamed_fields: Vec<(u32, String)>,
    /// The smallest amount of it that can be traded.
    pub min_size: f64,
    /// Trading session string. Populated by merging the paired schedule reply.
    pub trading_hours: Option<String>,
    /// Liquid (regular-session) hours string. Same source as trading_hours.
    pub liquid_hours: Option<String>,
    /// IANA timezone for session times (e.g. "US/Eastern").
    pub time_zone_id: Option<String>,
    /// Exchange-path join key (tag 6256) pairing secdef and schedule replies.
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
/// so that asking what arrived and went unread has an answer that cannot
/// quietly drift as fields are added.
pub fn tags_read_from_a_definition() -> Vec<u32> {
    let source = include_str!("mod.rs");
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
    let source = include_str!("mod.rs");
    let needle = format!("pub const {name}: u32 = ");
    let at = source.find(&needle)? + needle.len();
    let digits: String = source[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The tags a definition carried that nothing here reads.
///
/// The point of asking a venue for a contract is to be told about it, and a
/// field that arrives and is dropped is a fact about the contract nobody can
///  This names them so the gap is measurable rather than suspected.
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

/// Read one contract definition out of a security-definition
/// message.
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
    // Not 8598, which states the precision of a price rather than a size.
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
    /// Where this step starts.
    pub low_edge: f64,
    /// What the price moves in above it.
    pub increment: f64,
}

/// A market rule containing a rule ID and its price increment table.
#[derive(Debug, Clone)]
pub struct MarketRule {
    /// Which ladder this is.
    pub rule_id: i32,
    /// Each step of it.
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
            // Opens the size table rather than ending the rule: the sizes a
            // contract may be dealt in are stated after this count.
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
    /// Remember a definition, replacing any held under the same id.
    pub fn insert(&mut self, def: ContractDefinition) {
        let key = format!("{}:{}:{}", def.symbol, def.sec_type.to_fix(), def.currency);
        self.by_symbol.insert(key, def.con_id);
        self.by_con_id.insert(def.con_id, def);
    }

    /// The definition held under an id, if there is one.
    pub fn get(&self, con_id: u32) -> Option<&ContractDefinition> {
        self.by_con_id.get(&con_id)
    }

    /// The definition matching a symbol, kind and currency, if there is one.
    pub fn find(&self, symbol: &str, sec_type: SecurityType, currency: &str) -> Option<&ContractDefinition> {
        let key = format!("{}:{}:{}", symbol, sec_type.to_fix(), currency);
        self.by_symbol.get(&key).and_then(|id| self.by_con_id.get(id))
    }

    /// How many definitions are held.
    pub fn len(&self) -> usize {
        self.by_con_id.len()
    }

    /// Whether none are.
    pub fn is_empty(&self) -> bool {
        self.by_con_id.is_empty()
    }
}

// ─── Schedule subscription ───

/// Tags for schedule subscription responses.
pub const TAG_SUB_PROTOCOL: u32 = 6040;
/// FIX tag 6734: the schedule timezone.
pub const TAG_SCHEDULE_TIMEZONE: u32 = 6734;
/// FIX tag 6840: the session count.
pub const TAG_SESSION_COUNT: u32 = 6840;
/// FIX tag 6841: the session start.
pub const TAG_SESSION_START: u32 = 6841;
/// FIX tag 6842: the session end.
pub const TAG_SESSION_END: u32 = 6842;
/// FIX tag 75: the trade date.
pub const TAG_TRADE_DATE: u32 = 75;
/// FIX tag 6843: the is trading hours.
pub const TAG_IS_TRADING_HOURS: u32 = 6843;
/// FIX tag 6844: the is liquid hours.
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
    /// When the session opened.
    pub start: String,
    /// When it closed.
    pub end: String,
    /// The day it belongs to.
    pub trade_date: String,
}

/// Parsed schedule response.
#[derive(Debug, Clone)]
pub struct ContractSchedule {
    /// The zone these times are stated in.
    pub timezone: String,
    /// When the venue is open for the contract.
    pub trading_hours: Vec<ScheduleSession>,
    /// When it is liquid, which is narrower.
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
/// set is a closed day. It is kept as a zero-length session in both lists,
/// rendering as `<date>:CLOSED`, so a closed market is distinguishable from
/// absent data.
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
/// the official-API convention.
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
            // hot loop.
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
    // 12 to 14, where a multi-byte character would panic the slice.
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


// ─── Matching symbols search ───

/// Tags for matching symbols.
pub const TAG_MATCH_PATTERN: u32 = 58;
/// FIX tag 146: the match count.
pub const TAG_MATCH_COUNT: u32 = 146;
/// FIX tag 6453: the match primary exchange.
pub const TAG_MATCH_PRIMARY_EXCHANGE: u32 = 6453;
/// FIX tag 306: the match description.
pub const TAG_MATCH_DESCRIPTION: u32 = 306;
/// FIX tag 6070: the match derivative types.
pub const TAG_MATCH_DERIVATIVE_TYPES: u32 = 6070;

/// A single matching symbol result.
#[derive(Debug, Clone)]
pub struct SymbolMatch {
    /// The venue's own id for this contract.
    pub con_id: u32,
    /// Its ticker.
    pub symbol: String,
    /// What kind of contract it is.
    pub sec_type: SecurityType,
    /// What it is priced in.
    pub currency: String,
    /// Where it is listed.
    pub primary_exchange: String,
    /// The venue's own description.
    pub description: String,
    /// Which kinds of derivative it lists on it.
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
    /// Its ticker.
    pub symbol: String,
    /// Where it is routed.
    pub exchange: String,
    /// Which class within its chain.
    pub trading_class: String,
    /// How many units one contract is worth.
    pub multiplier: String,
    /// Every expiry this venue lists.
    pub expirations: Vec<String>,
    /// Every strike it lists.
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

// ── What a caller reads these as ─────────────────────────────────────────────
//
// The model is the shared vocabulary both surfaces present; a definition and a
// search result are what this module parses off the wire. The conversion
// between them is written here because a definition knows what it holds and
// the model does not know a definition exists.

impl crate::types::model::ContractDetails {
    /// Everything the venue stated about a contract, as a caller reads it.
    pub fn from_definition(def: &ContractDefinition) -> Self {
        let c = crate::types::model::Contract {
            con_id: def.con_id as i64,
            symbol: def.symbol.clone(),
            sec_type: def.sec_type.to_api_str().to_string(),
            exchange: def.exchange.clone(),
            primary_exchange: def.primary_exchange.clone(),
            currency: def.currency.clone(),
            local_symbol: def.local_symbol.clone(),
            trading_class: def.trading_class.clone(),
            last_trade_date_or_contract_month: def.last_trade_date.clone(),
            strike: def.strike,
            // Never carried across, so every option came back with its right
            // unset and a call was indistinguishable from a put outside the
            // local symbol.
            right: match def.right {
                Some(OptionRight::Call) => "C".to_string(),
                Some(OptionRight::Put) => "P".to_string(),
                None => String::new(),
            },
            multiplier: if def.multiplier != 1.0 { format!("{}", def.multiplier) } else { String::new() },
            ..Default::default()
        };
        Self {
            contract: c,
            // Parsed from the reply all along but thrown away.
            market_name: def.market_name.clone(),
            min_tick: def.min_tick,
            order_types: def.order_types.join(","),
            valid_exchanges: def.valid_exchanges.join(","),
            long_name: def.long_name.clone(),
            last_trade_date: def.last_trade_date.clone(),
            multiplier: if def.multiplier != 1.0 { format!("{}", def.multiplier) } else { String::new() },
            trading_hours: def.trading_hours.clone(),
            liquid_hours: def.liquid_hours.clone(),
            time_zone_id: def.time_zone_id.clone(),
            market_rule_ids: def.market_rule_id.map(|r| r.to_string()).unwrap_or_default(),
            stock_type: def.stock_type.clone(),
            ev_rule: def.ev_rule.clone(),
            ev_multiplier: def.ev_multiplier,
            coupon: def.coupon,
            contract_month: def.contract_month.clone(),
            under_sec_type: def.under_sec_type.clone(),
            under_con_id: def.under_con_id,
            under_symbol: def.under_symbol.clone(),
            last_trade_time: def.last_trade_time.clone(),
            issue_date: def.issue_date.clone(),
            size_increment: def.size_increment,
            suggested_size_increment: def.suggested_size_increment,
            last_price_precision: def.last_price_precision,
            last_size_precision: def.last_size_precision,
            settlement_method: def.settlement_method.clone(),
            unnamed_fields: def.unnamed_fields.clone(),
            bond_notes: def.bond_notes.clone(),
            desc_append: def.desc_append.clone(),
            bond_type: def.bond_type.clone(),
            coupon_type: def.coupon_type.clone(),
            next_option_date: def.next_option_date.clone(),
            next_option_type: def.next_option_type.clone(),
            ratings: def.ratings.clone(),
            fund_name: def.fund_name.clone(),
            fund_family: def.fund_family.clone(),
            fund_type: def.fund_type.clone(),
            fund_front_load: def.fund_front_load.clone(),
            fund_back_load: def.fund_back_load.clone(),
            fund_back_load_time_interval: def.fund_back_load_time_interval.clone(),
            fund_management_fee: def.fund_management_fee.clone(),
            fund_notify_amount: def.fund_notify_amount.clone(),
            fund_minimum_initial_purchase: def.fund_minimum_initial_purchase.clone(),
            fund_minimum_subsequent_purchase: def.fund_minimum_subsequent_purchase.clone(),
            fund_blue_sky_states: def.fund_blue_sky_states.clone(),
            fund_blue_sky_territories: def.fund_blue_sky_territories.clone(),
            fund_distribution_policy_indicator: def.fund_distribution_policy_indicator.clone(),
            fund_asset_type: def.fund_asset_type.clone(),
            real_expiration_date: def.real_expiration_date.clone(),
            callable: def.callable,
            puttable: def.puttable,
            convertible: def.convertible,
            next_option_partial: def.next_option_partial,
            fund_closed: def.fund_closed,
            fund_closed_for_new_investors: def.fund_closed_for_new_investors,
            fund_closed_for_new_money: def.fund_closed_for_new_money,
            agg_group: def.agg_group,
            price_magnifier: def.price_magnifier,
            industry: def.industry.clone(),
            category: def.category.clone(),
            subcategory: def.subcategory.clone(),
            country: def.country.clone(),
            isin: def.isin.clone(),
            cusip: def.cusip.clone(),
            sec_id_list: def.sec_id_list.clone(),
            min_size: def.min_size,
        }
    }
}

impl From<&SymbolMatch> for crate::types::model::ContractDescription {
    /// A symbol search result, as a caller reads it.
    fn from(m: &SymbolMatch) -> Self {
        Self {
            con_id: m.con_id as i64,
            symbol: m.symbol.clone(),
            sec_type: m.sec_type.to_fix().to_string(),
            currency: m.currency.clone(),
            primary_exchange: m.primary_exchange.clone(),
            derivative_sec_types: m.derivative_types.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
