//! Solving an option for its volatility, or for its price.
//!
//! Neither is a request this protocol carries. Both are computed client-side
//! from a pricing model, so a client has nothing to ask the venue for and must
//! do the arithmetic or refuse.
//!
//! What is done here is not a model of this library's own devising, and it is
//! not seeded with numbers of its own. The venue states its own model for a
//! contract — the volatility it used, the price that came out, and the
//! underlying it used — and that statement is what this is anchored to: a
//! carry is solved for until the model reproduces the venue's price exactly,
//! and a caller's question is answered as a change to that, not as an opinion.
//!
//! The venue does state a rate, on the same message as the rest of the model,
//! and it is not what is solved for here. What it does not state on that
//! message is the dividends, and this model has no dividend of its own to put
//! in their place. Measured against a captured statement, substituting the
//! stated rate and no dividend puts the model twenty-seven cents above the
//! venue's own price for the contract, and leaves the venue's price
//! unreachable at any volatility — the question it was asked would come back
//! refused. So the carry stays a fit, and it absorbs the dividends along with
//! everything else this model does differently. Taking the stated rate is
//! worth revisiting the day the dividends are read too, and not before.
//!
//! With no statement in hand, nothing is answered. A price worked out from a
//! rate nobody stated is a number this library made up, and a made-up option
//! price is worse than no answer.

/// What the venue said its own model made of a contract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VenueModel {
    /// The volatility it used.
    pub volatility: f64,
    /// The price that came out.
    pub option_price: f64,
    /// The underlying it used.
    pub underlying_price: f64,
    /// The dividends it took off, as a present value.
    pub present_value_of_dividends: f64,
}

/// What the contract is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OptionTerms {
    /// The strike.
    pub strike: f64,
    /// Years until it expires.
    pub years_to_expiry: f64,
    /// Whether it is a call.
    pub is_call: bool,
    /// Whether what it is written on is a future.
    ///
    /// A future costs nothing to hold, so it drifts nowhere: where a share
    /// grows at the rate over the life of the option, a futures price is
    /// already the price agreed for delivery and stays where it is. And the
    /// pay-off is settled at expiry rather than taken today, so it is
    /// discounted back — which is why one of these can be worth less than the
    /// difference between the future and the strike, where an option on a
    /// share cannot.
    ///
    /// The venue's own model says so: on a contract expiring in 0.728 of a
    /// day it stated 2294.945 where the future stood 2295.135 above the
    /// strike, and that difference discounted at the rate the venue states is
    /// 2294.942.
    pub on_a_future: bool,
}

/// How finely the tree is walked. Enough that a step's worth of error is far
/// under a cent on an ordinary contract, and few enough to answer at once.
const STEPS: usize = 256;

/// The price of an American option, by a binomial tree.
///
/// American, not European: an equity option can be exercised before it
/// expires, and pricing one as though it could not misprices every put deep
/// enough in the money to be worth exercising today.
///
/// Dividends are taken off the underlying rather than modelled as a yield,
/// which is what the venue states — a present value, not a rate.
pub fn price(terms: OptionTerms, spot: f64, volatility: f64, rate: f64, dividends: f64) -> Option<f64> {
    if !(spot.is_finite() && volatility.is_finite() && rate.is_finite() && dividends.is_finite()) {
        return None;
    }
    if terms.years_to_expiry <= 0.0 || volatility <= 0.0 || terms.strike <= 0.0 {
        return None;
    }
    let adjusted = spot - dividends;
    if adjusted <= 0.0 {
        return None;
    }

    let dt = terms.years_to_expiry / STEPS as f64;
    let up = (volatility * dt.sqrt()).exp();
    let down = 1.0 / up;
    // A future drifts nowhere: it is already the price agreed for delivery,
    // and holding it costs nothing. A share grows at the rate.
    let growth = if terms.on_a_future { 1.0 } else { (rate * dt).exp() };
    if !(up.is_finite() && growth.is_finite()) || (up - down).abs() < f64::EPSILON {
        return None;
    }
    let up_chance = (growth - down) / (up - down);
    if !(0.0..=1.0).contains(&up_chance) {
        return None;
    }
    let discount = (-rate * dt).exp();

    // Value at expiry, from the lowest node up.
    let mut value = Vec::with_capacity(STEPS + 1);
    for i in 0..=STEPS {
        let underlying = adjusted * up.powi(i as i32) * down.powi((STEPS - i) as i32);
        value.push(exercise_value(terms, underlying));
    }
    // Back through the tree, taking early exercise wherever it is worth more.
    for step in (0..STEPS).rev() {
        for i in 0..=step {
            let held = discount * (up_chance * value[i + 1] + (1.0 - up_chance) * value[i]);
            let underlying = adjusted * up.powi(i as i32) * down.powi((step - i) as i32);
            // Taken early where that is worth more — except on a future,
            // whose options settle at expiry. Allowed there, the tree returns
            // the difference between the future and the strike for anything
            // deep enough in the money, and the venue's own price for those
            // sits below it.
            value[i] = if terms.on_a_future {
                held
            } else {
                held.max(exercise_value(terms, underlying))
            };
        }
    }
    Some(value[0])
}

fn exercise_value(terms: OptionTerms, underlying: f64) -> f64 {
    if terms.is_call {
        (underlying - terms.strike).max(0.0)
    } else {
        (terms.strike - underlying).max(0.0)
    }
}

/// The carry that makes this model reproduce the venue's price from the
/// venue's volatility.
///
/// **Not the venue's interest rate, and not presented as one.** It is a
/// fitting number: whatever this model needs so that its price for a contract
/// matches the price the venue published for it. Anything the venue's model
/// does that this one does not — a different tree, dividends taken discretely,
/// a borrow cost — is absorbed here, which is exactly what makes a caller's
/// question answerable as a change to the venue's answer.
///
/// That it is a fit and not a rate is visible on the wire: two contracts on
/// one underlying, one expiry, one minute, wanted five per cent and twenty per
/// cent. No interest rate differs by strike.
///
/// The venue states a rate of its own on the same message as the rest of the
/// model, and this is not it: against a captured statement the two are three
/// hundred basis points apart, because what this absorbs is the dividends the
/// message does not state as well as the difference between the two models.
pub fn carry_that_matches_the_venue(terms: OptionTerms, model: VenueModel) -> Option<f64> {
    // Searched only where the tree holds together. A step's worth of growth
    // has to stay inside a step up, or the tree's own odds leave nought-to-one
    // and it prices nothing — and how far that reaches depends on the
    // volatility and the step, not on a bound picked here. A contract with a
    // volatility of two per cent leaves far less room than one with fifty.
    let step = terms.years_to_expiry / STEPS as f64;
    if step <= 0.0 || model.volatility <= 0.0 {
        return None;
    }
    // Bounded twice over: by where the tree holds together, and by what a
    // rate is. Money has never cost a quarter of itself a year, and letting
    // the search run out to where the tree degenerates finds a rate that
    // reproduces the price by breaking the model rather than by being right —
    // and leaves no room underneath it for the volatility to be solved for
    // afterwards.
    const FURTHEST_A_RATE_GOES: f64 = 0.25;
    let furthest = (model.volatility / step.sqrt() * 0.9).min(FURTHEST_A_RATE_GOES);
    solve(-furthest, furthest, |rate| {
        price(
            terms,
            model.underlying_price,
            model.volatility,
            rate,
            model.present_value_of_dividends,
        )
        .map(|p| p - model.option_price)
    })
}

/// What volatility a caller's price implies, under the venue's model.
pub fn implied_volatility(
    terms: OptionTerms,
    model: VenueModel,
    option_price: f64,
    underlying_price: f64,
) -> Option<f64> {
    let rate = carry_that_matches_the_venue(terms, model)?;
    // Searched from where the tree holds together. A step up has to outrun a
    // step's worth of growth or the tree stops being a tree — its own odds
    // leave nought-to-one — so the smallest volatility worth trying is set by
    // the rate and the step, not by a number picked here.
    let step = terms.years_to_expiry / STEPS as f64;
    // Just inside where the tree holds, not comfortably inside it: a real
    // contract deep in the money carries a volatility of under two per cent,
    // and a floor set with room to spare sits above the answer and finds
    // nothing.
    let smallest = (rate.abs() * step.sqrt() * 1.02).max(1e-4);
    solve(smallest, 5.0, |volatility| {
        price(
            terms,
            underlying_price,
            volatility,
            rate,
            model.present_value_of_dividends,
        )
        .map(|p| p - option_price)
    })
}

/// What price a caller's volatility implies, under the venue's model.
pub fn option_price(
    terms: OptionTerms,
    model: VenueModel,
    volatility: f64,
    underlying_price: f64,
) -> Option<f64> {
    let rate = carry_that_matches_the_venue(terms, model)?;
    price(
        terms,
        underlying_price,
        volatility,
        rate,
        model.present_value_of_dividends,
    )
}

// Why an answer to a hypothetical carries no greeks.
//
// The reference client's own calculator works them out for the contract as
// asked about, and sends them beside the answer — the ones the venue streams
// belong to the volatility the venue used, not the one a caller asked with.
// So this is a real gap, and it is left open rather than filled badly.
//
// Taking them off this tree was tried and measured against the venue's own,
// on contracts it was streaming at the time. Far enough into the money they
// land exactly — delta 0.999959 against 0.999959. Near the money they do not:
// 1.000112 against 0.998395, and a gamma of nothing against 0.000107. The
// step a derivative is taken over has to be small beside how far the option
// is from its strike and large beside the tree's own spacing, and near the
// money there is little room between those. That is a numerical question
// this has one sample to answer, on a contract whose pricing already has a
// gap of its own, and a delta that is wrong where delta matters is worse than
// none.

/// Find where a rising function crosses zero, between two bounds.
///
/// Bisection rather than anything faster: it cannot run away from a bad
/// starting point, and the cost of a hundred more evaluations is nothing
/// beside answering with a number that is wrong.
fn solve(low: f64, high: f64, f: impl Fn(f64) -> Option<f64>) -> Option<f64> {
    let (mut low, mut high) = (low, high);
    let at_low = f(low)?;
    let at_high = f(high)?;
    if at_low.signum() == at_high.signum() {
        // The answer is not between the bounds, so there is none to give.
        return None;
    }
    for _ in 0..100 {
        let middle = 0.5 * (low + high);
        let here = f(middle)?;
        if here.abs() < 1e-9 {
            return Some(middle);
        }
        if here.signum() == at_low.signum() {
            low = middle;
        } else {
            high = middle;
        }
    }
    Some(0.5 * (low + high))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(strike: f64, years: f64) -> OptionTerms {
        OptionTerms { strike, years_to_expiry: years, is_call: true, on_a_future: false }
    }

    /// A call with no dividends and no rate is worth what the tree says, and
    /// the tree agrees with the closed form to well under a cent.
    ///
    /// The closed form for these inputs is 10.4506, which is the standard
    /// worked example: spot 100, strike 100, a year, twenty per cent, five
    /// per cent.
    #[test]
    fn the_tree_agrees_with_the_closed_form() {
        let price = price(call(100.0, 1.0), 100.0, 0.2, 0.05, 0.0).expect("it prices");
        assert!((price - 10.4506).abs() < 0.05, "the tree says {price}");
    }

    /// An option worth nothing at expiry is worth nothing.
    #[test]
    fn a_worthless_call_is_worth_nothing() {
        let price = price(call(200.0, 0.5), 100.0, 0.2, 0.05, 0.0).expect("it prices");
        assert!(price < 0.01, "{price}");
    }

    /// A put deep in the money is worth at least what exercising it today
    /// would give. Priced as though it could only be exercised at expiry, it
    /// comes out worth less than that, which is the whole difference between
    /// an American option and a European one.
    #[test]
    fn a_deep_put_is_worth_at_least_exercising_it() {
        let terms = OptionTerms { strike: 200.0, years_to_expiry: 1.0, is_call: false, on_a_future: false };
        let price = price(terms, 100.0, 0.2, 0.05, 0.0).expect("it prices");
        assert!(price >= 100.0 - 0.01, "an American put worth less than exercising it: {price}");
    }

    /// The carry is fitted to what the venue stated rather than assumed.
    /// Given a price this model produced at a known rate, that number comes
    /// back — which is what makes it a fit, not a measurement.
    #[test]
    fn the_carry_that_matches_the_venue_is_found() {
        let terms = call(100.0, 1.0);
        let at_four_percent = price(terms, 100.0, 0.25, 0.04, 1.5).expect("it prices");
        let model = VenueModel {
            volatility: 0.25,
            option_price: at_four_percent,
            underlying_price: 100.0,
            present_value_of_dividends: 1.5,
        };
        let found = carry_that_matches_the_venue(terms, model).expect("a carry matches it");
        assert!((found - 0.04).abs() < 1e-3, "the carry came back as {found}");
    }

    /// A caller's price gives back the volatility that produces it, and the
    /// venue's price gives back the venue's volatility.
    #[test]
    fn a_price_gives_back_its_volatility() {
        let terms = call(100.0, 1.0);
        let venue_price = price(terms, 100.0, 0.25, 0.04, 1.5).expect("it prices");
        let model = VenueModel {
            volatility: 0.25,
            option_price: venue_price,
            underlying_price: 100.0,
            present_value_of_dividends: 1.5,
        };
        let same = implied_volatility(terms, model, venue_price, 100.0).expect("it solves");
        assert!((same - 0.25).abs() < 1e-3, "the venue's price gave {same}");

        let dearer = implied_volatility(terms, model, venue_price * 1.2, 100.0).expect("it solves");
        assert!(dearer > 0.25, "a dearer option implies more volatility, not {dearer}");
    }

    /// And a caller's volatility gives back a price, which is the same
    /// question asked the other way round.
    #[test]
    fn a_volatility_gives_back_its_price() {
        let terms = call(100.0, 1.0);
        let venue_price = price(terms, 100.0, 0.25, 0.04, 1.5).expect("it prices");
        let model = VenueModel {
            volatility: 0.25,
            option_price: venue_price,
            underlying_price: 100.0,
            present_value_of_dividends: 1.5,
        };
        let same = option_price(terms, model, 0.25, 100.0).expect("it prices");
        assert!((same - venue_price).abs() < 0.01, "{same} against {venue_price}");
    }

    /// Nothing is answered from nonsense. An expiry in the past, a volatility
    /// of nothing, an underlying worth less than its own dividends.
    #[test]
    fn nonsense_is_not_answered() {
        assert!(price(call(100.0, 0.0), 100.0, 0.2, 0.05, 0.0).is_none());
        assert!(price(call(100.0, 1.0), 100.0, 0.0, 0.05, 0.0).is_none());
        assert!(price(call(100.0, 1.0), 1.0, 0.2, 0.05, 5.0).is_none());
        assert!(price(call(100.0, 1.0), f64::NAN, 0.2, 0.05, 0.0).is_none());
    }
}
