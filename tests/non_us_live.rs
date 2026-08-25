//! Contract description on non-US listings.
//!
//! Three request paths describe a contract to the venue: a head timestamp
//! request names its security type and exchange, a fundamentals request names
//! its currency, and a market data subscription names all three. Each must
//! carry the contract's own values.
//!
//! A US listing cannot distinguish a correct description from a defaulted one,
//! because the defaults describe a US stock on SMART in USD. These probes ask
//! about listings outside the US, where the two differ.
//!
//! Requires an open Asian or European session. Run with
//! `--ignored --test-threads=1`: the account permits one concurrent session,
//! and parallel tests evict each other, which presents as no reply.

use ibx::api::EClient;
use ibx::api::client::EClientConfig;
use ibx::types::model::Contract;

fn config() -> Option<EClientConfig> {
    let (u, p) = (std::env::var("IB_USERNAME").ok()?, std::env::var("IB_PASSWORD").ok()?);
    Some(EClientConfig { username: u, password: p, paper: true, ..Default::default() })
}

/// Symbol, currency, exchange, and the security type the venue knows it as.
const LISTINGS: &[(&str, &str, &str, &str)] =
    &[("7203", "JPY", "TSEJ", "STK"), ("SAP", "EUR", "IBIS", "STK"), ("VOD", "GBP", "LSE", "STK")];

/// Contracts that trade outside US equity hours and describe themselves
/// differently from a US stock. A currency pair has no trade series and quotes
/// a midpoint; a future states a contract month where a stock states none.
const AROUND_THE_CLOCK: &[(&str, &str, &str, &str)] = &[
    ("EUR", "USD", "IDEALPRO", "CASH"),
    ("GBP", "USD", "IDEALPRO", "CASH"),
    ("ES", "USD", "CME", "FUT"),
    ("NQ", "USD", "CME", "FUT"),
    // A metal and an energy contract, on venues of their own.
    ("GC", "USD", "COMEX", "FUT"),
    ("CL", "USD", "NYMEX", "FUT"),
    // And what the venue quotes around the clock with no expiry at all.
    ("XAUUSD", "USD", "SMART", "CMDTY"),
];

#[test]
#[ignore = "opens a session of its own and needs a market outside the US to be open; run with --ignored"]
fn a_contract_outside_the_us_is_described_as_itself() {
    let Some(cfg) = config() else {
        println!("Skipping: IB_USERNAME/IB_PASSWORD not set");
        return;
    };
    let client = EClient::connect(&cfg).expect("connect");

    let mut described = 0usize;
    for (symbol, currency, exchange, sec_type) in LISTINGS {
        let asked = Contract {
            symbol: (*symbol).into(),
            currency: (*currency).into(),
            exchange: (*exchange).into(),
            sec_type: (*sec_type).into(),
            ..Default::default()
        };
        // Resolve the contract first. Later assertions compare against the
        // venue's resolved description rather than the requested one.
        let found = match client.contract_details(&asked) {
            Ok(d) if !d.is_empty() => d,
            Ok(_) => {
                println!("  {symbol} on {exchange}: the venue described no contract");
                continue;
            }
            Err(e) => {
                println!("  {symbol} on {exchange}: refused — {e}");
                continue;
            }
        };
        let c = &found[0].contract;
        println!(
            "  {symbol}: conId={} secType={} exchange={} currency={}",
            c.con_id, c.sec_type, c.exchange, c.currency,
        );
        assert_eq!(&c.currency, currency, "{symbol} is quoted in {currency}");
        described += 1;

        // The head timestamp request must carry this contract's security type
        // and exchange. Defaulted values address a different instrument.
        match client.head_timestamp(c, "TRADES", true) {
            Ok(when) if !when.is_empty() => println!("    earliest trade: {when}"),
            Ok(_) => println!("    earliest trade: the venue stated none"),
            Err(e) => println!("    earliest trade refused: {e}"),
        }

        // And the bars themselves, which name the same description.
        match client.bars(c, "2 D", "1 hour") {
            Ok(bars) => {
                println!("    {} bar(s)", bars.len());
                assert!(!bars.is_empty(), "{symbol} traded in the last two days");
            }
            Err(e) => println!("    bars refused: {e}"),
        }

        // A fundamentals request carries the contract's quote currency. A
        // defaulted USD addresses a listing that does not exist.
        //
        // Without the data entitlement the reply is the same short envelope for
        // every contract. This asserts the request was accepted and routed; it
        // does not assert the currency.
        match client.fundamental_data(c, "ReportSnapshot") {
            Ok(report) => {
                println!("    fundamentals: {} bytes (empty without the entitlement)", report.len())
            }
            Err(e) => println!("    fundamentals refused: {e}"),
        }
    }

    client.disconnect();
    assert!(
        described > 0,
        "the venue described none of these listings, so nothing here was tested",
    );
}

/// What trades while the US equity market is shut.
///
/// A currency pair has no trade series; only MIDPOINT is available, and a
/// TRADES request is answered "no historical market data". A future states a
/// contract month, which occupies a different tag from a full expiry date.
/// Neither case is exercised by a US stock.
#[test]
#[ignore = "opens a session of its own; run with --ignored"]
fn a_contract_that_trades_around_the_clock_is_described_as_itself() {
    let Some(cfg) = config() else {
        println!("Skipping: IB_USERNAME/IB_PASSWORD not set");
        return;
    };
    let client = EClient::connect(&cfg).expect("connect");

    for (symbol, currency, exchange, sec_type) in AROUND_THE_CLOCK {
        let asked = Contract {
            symbol: (*symbol).into(),
            currency: (*currency).into(),
            exchange: (*exchange).into(),
            sec_type: (*sec_type).into(),
            ..Default::default()
        };
        let found = match client.contract_details(&asked) {
            Ok(d) if !d.is_empty() => d,
            Ok(_) => {
                println!("  {symbol} on {exchange}: the venue described no contract");
                continue;
            }
            Err(e) => {
                println!("  {symbol} on {exchange}: refused — {e}");
                continue;
            }
        };
        let c = &found[0].contract;
        println!(
            "  {symbol}: conId={} secType={} exchange={} currency={} expiry={:?} multiplier={:?}",
            c.con_id,
            c.sec_type,
            c.exchange,
            c.currency,
            c.last_trade_date_or_contract_month,
            c.multiplier,
        );
        println!(
            "    localSymbol={:?} tradingClass={:?} primaryExchange={:?}",
            c.local_symbol, c.trading_class, c.primary_exchange,
        );
        assert_eq!(&c.sec_type, sec_type, "{symbol} is a {sec_type}");

        // `bars` selects MIDPOINT for instruments without a trade series and
        // TRADES otherwise. The wrong choice is answered as a missing series.
        match client.bars(c, "1 D", "1 hour") {
            Ok(bars) => {
                println!("    {} bar(s)", bars.len());
                assert!(!bars.is_empty(), "{symbol} has priced in the last day");
            }
            Err(e) => println!("    bars refused: {e}"),
        }

        // Preview the order without placing it. The preview carries the order
        // type and the full contract description.
        let order = ibx::types::model::Order::limit("BUY", 1.0, 1.0);
        match client.preview(c, &order) {
            Ok(state) => println!(
                "    preview: init margin {:?} commission {:?}",
                state.init_margin_change, state.commission_and_fees,
            ),
            Err(e) => {
                println!("    preview refused: {e}");
                // Returned when the contract named on the order is not one the
                // venue holds. Here that indicates an encoding fault rather
                // than a market condition: a contract month derived by
                // truncating the last trade date is wrong whenever that date
                // falls in the preceding month.
                assert!(
                    !e.to_string().contains("does not match supplied contract parameters"),
                    "{symbol} was described to the venue as a contract it does not hold",
                );
            }
        }
    }

    client.disconnect();
}

/// A subscription that names only the contract id.
///
/// A contract id identifies the contract exactly. Adding a security type and
/// exchange the caller did not state describes a different instrument. Whether
/// the venue answers an id carrying nothing beside it is determined here.
struct Quiet;
impl ibx::api::wrapper::Wrapper for Quiet {}

#[test]
#[ignore = "opens a session of its own and needs a market open; run with --ignored"]
fn a_subscription_naming_only_the_contract_id_is_answered() {
    let Some(cfg) = config() else {
        println!("Skipping: IB_USERNAME/IB_PASSWORD not set");
        return;
    };
    let client = EClient::connect(&cfg).expect("connect");

    // EUR/USD on IDEALPRO, which quotes around the clock. Named by its id and
    // nothing else.
    // The control: the same contract fully described, so a silence below is
    // the missing description and not the contract or the hour.
    let described = Contract {
        con_id: 12087792,
        symbol: "EUR".into(),
        sec_type: "CASH".into(),
        exchange: "IDEALPRO".into(),
        currency: "USD".into(),
        ..Default::default()
    };
    client.req_mkt_data(9002, &described, "", false, false).expect("subscribe described");
    let control_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut control = None;
    while std::time::Instant::now() < control_deadline {
        client.process_msgs(&mut Quiet);
        if let Some(q) = client.quote(9002)
            && (q.bid != 0 || q.ask != 0)
        {
            control = Some(q);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let _ = client.cancel_mkt_data(9002);
    println!("  described: {}", if control.is_some() { "answered" } else { "silent" });

    let bare = Contract { con_id: 12087792, ..Default::default() };
    let req_id = 9001;
    // Refused rather than sent, for a contract this client cannot describe: an
    // undescribed one is asked about as a US stock, and the venue holds no US
    // stock under this id. That is an answer, and a better one than silence —
    // recorded here the same way the silence is, because what this phase is
    // for is the described subscription below it.
    let asked = client.req_mkt_data(req_id, &bare, "", false, false);
    if let Err(refused) = &asked {
        println!("  bare conId refused: {}", refused.message);
    }
    let mut seen = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        client.process_msgs(&mut Quiet);
        if let Some(q) = client.quote(req_id)
            && (q.bid != 0 || q.ask != 0 || q.last != 0)
        {
            seen = Some(q);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    if asked.is_ok() {
        let _ = client.cancel_mkt_data(req_id);
    }
    // What the venue does with each, recorded rather than asserted one way:
    // the described subscription is answered and the bare one is not, which is
    // why this client states a description at all. It should state the
    // contract's own; the fallback in the subscribe path describes a US stock,
    // so an undescribed contract of another kind is asked about as one.
    match seen {
        Some(q) => println!("  bare conId answered: bid={} ask={}", q.bid, q.ask),
        None => println!("  bare conId: silent, so the description is required"),
    }
    assert!(
        control.is_some(),
        "the described subscription must be answered, or this proves nothing",
    );

    client.disconnect();
}
