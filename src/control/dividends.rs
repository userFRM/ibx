//! What a contract pays out, and when.
//!
//! The venue keeps a schedule per contract — one entry per ex-date, with the
//! amount, the currency it is paid in, and what it is paid out of — and it
//! answers a text query with that schedule and the term rates beside it. That
//! is not the corporate-actions feed: the actions feed states what has
//! happened to a contract, and this states what it is going to pay.
//!
//! It is the input the option model is missing. A tree that folds a whole
//! year's dividends into one continuous yield reproduces the price it was
//! calibrated to and gets the shape wrong, so the figures that depend on the
//! shape — what one percentage point of volatility is worth, what one day
//! costs — come out several per cent off. With the dates, each payment sits at
//! its own step.
//!
//! The query goes out as text and the answer comes back as a small XML
//! document, read here by name for the reason every other answer here is.

/// One payment the venue has on its books for a contract.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Payment {
    /// The day the price goes ex — the one that matters to an option, because
    /// it is the day the underlying drops by the amount.
    pub ex_date: String,
    /// The day it is paid.
    pub pay_date: String,
    /// The day the holder of record is fixed.
    pub record_date: String,
    /// The currency it is paid in, which need not be the contract's.
    pub currency: String,
    /// How much, per share, in that currency.
    pub amount: f64,
    /// What it is paid out of — income, a capital gain, or unstated.
    pub distribution_type: String,
}

/// A contract's whole schedule, as the venue states it.
#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    /// What the venue says a payment is worth after tax, as a ratio.
    ///
    /// One where the venue states none, which leaves every amount as it is.
    /// Whether it is applied is a preference the venue states elsewhere and
    /// this client has not been told, so the amounts here are the stated ones:
    /// applying a ratio nobody asked for would move every figure that follows.
    pub tax_adjustment: f64,
    /// The payments, in the order the venue stated them.
    pub payments: Vec<Payment>,
    /// The rates the venue prices this contract's options at, by term, in the
    /// order stated.
    pub term_rates: Vec<f64>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self { tax_adjustment: 1.0, payments: Vec::new(), term_rates: Vec::new() }
    }
}

/// Ask what a contract pays out.
///
/// The query is text, not a document: the request carries it verbatim and the
/// venue answers the whole schedule under the id it was asked with. Specials
/// are asked for, which is the ordinary case — a special dividend moves the
/// underlying on its ex-date exactly as the regular one does, so a model that
/// left it out would price every option over that date as though nothing
/// happened.
pub fn query_for(con_id: u32) -> String {
    format!("div incSpecial {con_id}")
}

/// The same, for a currency's own rates rather than a contract's.
///
/// The venue takes the same query for both and tells them apart by what
/// follows the word: a number is a contract, letters are a currency.
pub fn query_for_currency(currency: &str) -> String {
    format!("div {currency}")
}

/// Read the answer.
///
/// Absent fields read as absent rather than as nought: a payment whose amount
/// does not state a number is not a payment of nothing, and the venue has no
/// reason to send one, so it is left out rather than carried as a zero that
/// would sit in the tree as a real ex-date.
pub fn parse(xml: &str) -> Schedule {
    let tax_adjustment = crate::control::xml::tag(xml, "taxAdjRatio")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(1.0);
    let payments = crate::control::xml::elements(xml, "div")
        .into_iter()
        .filter_map(one_payment)
        .collect();
    let term_rates = crate::control::xml::elements(xml, "rate")
        .into_iter()
        .filter_map(|r| r.trim().parse::<f64>().ok())
        .filter(|r| r.is_finite())
        .collect();
    Schedule { tax_adjustment, payments, term_rates }
}

/// One entry, or nothing where it states no amount or no ex-date.
///
/// Both are what a payment is for the purpose it is read for here. An entry
/// with neither would be an ex-date the tree cannot place or a step the
/// underlying drops by nothing over, and a schedule is better one payment
/// short than carrying a payment that is not one.
fn one_payment(entry: &str) -> Option<Payment> {
    let stated = |name: &str| {
        crate::control::xml::tag(entry, name).map(str::trim).filter(|v| !v.is_empty())
    };
    let amount: f64 = stated("amt")?.parse().ok()?;
    if !amount.is_finite() {
        return None;
    }
    Some(Payment {
        ex_date: stated("date")?.to_string(),
        pay_date: stated("payDate").unwrap_or_default().to_string(),
        record_date: stated("recDate").unwrap_or_default().to_string(),
        currency: stated("curr").unwrap_or_default().to_string(),
        amount,
        distribution_type: stated("dt").unwrap_or_default().to_string(),
    })
}

/// The payments an option's life covers, as the tree wants them: how far off
/// each ex-date is, in years, and what the underlying drops by.
///
/// In the contract's own currency only. The venue states the currency of each
/// payment and it need not be the one the contract is quoted in — and where
/// the contract's currency is not known, a payment that names one cannot be
/// shown to be in it, so it is left out.
///
/// Measured from the day the caller names, as a count of days since the epoch
/// — the same count this library reads a date the venue stated into.
///
/// The venue's own count is what places them. A payment on the valuation day
/// itself has already happened as far as the underlying's price is concerned,
/// and one after expiry is somebody else's; both are left out, which is the
/// rule the reference model applies.
///
/// Counted in whole days, which is how the reference model counts to an
/// ex-date: it rounds the fraction of a day up, and it decides what is
/// eligible by comparing calendar dates — so a payment going ex tomorrow is a
/// day away whatever hour it is now, and one going ex today has gone.
pub fn over_the_life(
    schedule: &Schedule, from: i64, years_to_expiry: f64, currency: &str,
) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for payment in &schedule.payments {
        // In the money the contract is quoted in, or not at all. The tree
        // takes the amount off the underlying's price and off the strike, so
        // a payment in another currency taken at face value is a number of
        // euros subtracted from a price in dollars — a finite answer in no
        // units at all. Converting it needs a rate nothing here states.
        //
        // A payment that names a currency is matched against one that is
        // named. Where the contract's own is not known, no payment naming one
        // can be shown to be in it — and passing them all through on an empty
        // string made the guard a guard against nothing at all.
        if !payment.currency.is_empty() && !payment.currency.eq_ignore_ascii_case(currency) {
            continue;
        }
        let Some(ex) = crate::protocol::datetime::day_number(&payment.ex_date) else { continue };
        let days = ex - from;
        if days <= 0 {
            continue;
        }
        // Against the same whole days the ex-date is counted in. The venue
        // states how long the contract has left as a fraction — ninety-eight
        // and a twenty-fifth of a day — and a payment going ex on the day it
        // expires is a whole day count one larger than that fraction. Compared
        // as they stand, such a payment read as falling after expiry and was
        // dropped, on exactly the contracts where it matters most.
        if days as f64 > (years_to_expiry * 365.0).ceil() {
            continue;
        }
        out.push((days as f64 / 365.0, payment.amount));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answer as the venue writes it, read back whole.
    ///
    /// The amounts are what the tree drops the underlying by and the ex-dates
    /// are where it drops them, so a field read short here is a price wrong by
    /// a dividend.
    #[test]
    fn the_schedule_reads_back_as_the_venue_states_it() {
        let answer = "\
            <dividends>\
            <div><date>20260320</date><payDate>20260331</payDate>\
            <recDate>20260323</recDate><curr>USD</curr><amt>1.81</amt><dt>I</dt></div>\
            <div><date>20260619</date><payDate>20260630</payDate>\
            <recDate>20260622</recDate><curr>USD</curr><amt>1.77</amt><dt>I</dt></div>\
            </dividends>\
            <taxAdjRatio>0.85</taxAdjRatio>\
            <termrates><rate>0.0433</rate><rate>0.0441</rate></termrates>";
        let read = parse(answer);
        assert_eq!(read.tax_adjustment, 0.85);
        assert_eq!(read.term_rates, [0.0433, 0.0441]);
        assert_eq!(read.payments.len(), 2, "{:?}", read.payments);
        assert_eq!(read.payments[0].ex_date, "20260320");
        assert_eq!(read.payments[0].pay_date, "20260331");
        assert_eq!(read.payments[0].record_date, "20260323");
        assert_eq!(read.payments[0].currency, "USD");
        assert_eq!(read.payments[0].amount, 1.81);
        assert_eq!(read.payments[0].distribution_type, "I");
        assert_eq!(read.payments[1].amount, 1.77);
    }

    /// A contract that pays nothing says so, and that is an answer.
    #[test]
    fn a_contract_that_pays_nothing_reads_as_no_payments() {
        let read = parse("<dividends></dividends><termrates></termrates>");
        assert!(read.payments.is_empty());
        assert_eq!(read.tax_adjustment, 1.0, "no ratio stated leaves every amount as it is");
    }

    /// An entry the venue did not finish is left out rather than carried as a
    /// payment of nothing on a date the tree cannot place.
    #[test]
    fn an_entry_without_an_amount_or_a_date_is_not_a_payment() {
        let read = parse(
            "<dividends>\
             <div><date>20260320</date><curr>USD</curr></div>\
             <div><curr>USD</curr><amt>1.81</amt></div>\
             <div><date>20260619</date><curr>USD</curr><amt>1.77</amt></div>\
             </dividends>",
        );
        assert_eq!(read.payments.len(), 1, "{:?}", read.payments);
        assert_eq!(read.payments[0].ex_date, "20260619");
    }

    /// A ratio that is not a number, or is nought, leaves the amounts alone
    /// rather than scaling every one of them to nothing.
    #[test]
    fn a_ratio_that_is_not_one_to_use_reads_as_one() {
        for stated in ["", "nonsense", "0", "-1"] {
            let read = parse(&format!("<taxAdjRatio>{stated}</taxAdjRatio>"));
            assert_eq!(read.tax_adjustment, 1.0, "a ratio of {stated:?} was used");
        }
    }

    /// The query names the contract and asks for specials.
    #[test]
    fn the_query_names_what_it_is_about() {
        assert_eq!(query_for(756_733), "div incSpecial 756733");
        assert_eq!(query_for_currency("USD"), "div USD");
    }

    /// What the tree is handed: the payments inside the option's life, counted
    /// in years from today, and nothing else.
    ///
    /// A payment already gone is in the underlying's price; one after expiry
    /// belongs to whoever holds the shares then. Carried either way, the tree
    /// drops the underlying at a step that is not there.
    #[test]
    fn only_the_payments_the_options_life_covers_reach_the_tree() {
        let schedule = Schedule {
            tax_adjustment: 1.0,
            term_rates: Vec::new(),
            payments: vec![
                Payment { ex_date: "20260101".into(), amount: 1.0, ..Default::default() },
                Payment { ex_date: "20260320".into(), amount: 1.81, ..Default::default() },
                Payment { ex_date: "20260619".into(), amount: 1.77, ..Default::default() },
                Payment { ex_date: "20270101".into(), amount: 1.9, ..Default::default() },
            ],
        };
        // From the first of March, with half a year to run: the January
        // payment has gone, the two in between are in, and the next January is
        // past expiry.
        let from = crate::protocol::datetime::day_number("20260301").expect("a real day");
        let over = over_the_life(&schedule, from, 0.5, "USD");
        assert_eq!(over.len(), 2, "{over:?}");
        assert_eq!(over[0].1, 1.81);
        assert_eq!(over[1].1, 1.77);
        // Nineteen days to the first, carried in years on the venue's basis.
        assert!((over[0].0 - 19.0 / 365.0).abs() < 1e-12, "{}", over[0].0);

        // A payment going ex on the day the contract expires is inside its
        // life. The venue states how long is left as a fraction, and a whole
        // day count is one larger than that fraction — compared as they
        // stand, the payment read as falling after expiry and was dropped.
        let expiring = Schedule {
            tax_adjustment: 1.0,
            term_rates: Vec::new(),
            payments: vec![
                Payment { ex_date: "20260311".into(), amount: 1.0, ..Default::default() },
            ],
        };
        let nine_and_a_half = 9.5 / 365.0;
        let over = over_the_life(&expiring, from, nine_and_a_half, "");
        assert_eq!(over.len(), 1, "the payment on the day it expires: {over:?}");
    }
}
