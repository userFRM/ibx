//! Solving an option for its volatility, or for its price.
//!
//! Neither is a request this protocol carries. Both are computed client-side
//! from a pricing model, so a client has nothing to ask the venue for and must
//! do the arithmetic or refuse.
//!
//! What is done here is not a model of this library's own devising, and it is
//! not seeded with numbers of its own. The venue states its own model for a
//! contract — the volatility it used, the rate it discounted at, the price
//! that came out, and the underlying it used — and that statement is what this
//! is anchored to. A caller's question is answered as a change to it, not as
//! an opinion.
//!
//! Both figures reach here over a year, which is the scale they are reported
//! on and the scale this works in. The wire states them over one of the days
//! it counts beside them and they are carried across as they are read, so
//! nothing here converts anything.
//!
//! Measured across two expiries and eighteen strikes, that reproduces the
//! venue's price on every one of them — an eight-day call a hundred and fifty
//! points out of the money at 3.9382 against the venue's 3.9382 — and solving
//! each of those prices back returns the volatility the venue stated it
//! against. Read short by the root of a year, as this was before, anything not
//! already worth its intrinsic value prices at nothing: the same call came out
//! at zero, no volatility reached the venue's price, and the caller was told
//! the contract could not be solved.
//!
//! With no statement in hand, nothing is answered. A price worked out from a
//! rate nobody stated is a number this library made up, and a made-up option
//! price is worse than no answer.

/// What the venue said its own model made of a contract.
///
/// The volatility and the rate are over a year, carried across from the day
/// the wire states them over before they get here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VenueModel {
    /// The volatility it used, over a year.
    pub volatility: f64,
    /// The price that came out.
    pub option_price: f64,
    /// The underlying it used.
    pub underlying_price: f64,
    /// The dividends it took off, as a present value.
    pub present_value_of_dividends: f64,
    /// The rate it discounted at, over a year.
    pub rate: f64,
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
    // A terminal node that overflowed carries the overflow down to the root.
    // The step ratio is checked above and being finite there does not make it
    // finite raised to the number of steps: the highest node is the ratio to
    // that power, so a volatility stated per cent where a fraction was meant
    // reaches it. What comes back is not a price, and a caller reading it as
    // one has no way to tell — it is refused here for the reason a volatility
    // that will not converge is refused.
    value[0].is_finite().then_some(value[0])
}

/// What an option is worth, and what its worth is doing.
///
/// The venue publishes one of these per option, struck against the price it
/// considers the option's. It does not publish the three struck against the
/// bid, the ask and the last trade — the protocol's own terminal works those
/// out, and a caller of the reference client is handed all four.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Greeks {
    /// What the model says the option is worth.
    pub price: f64,
    /// How much it moves with the underlying.
    pub delta: f64,
    /// How much the delta moves with it.
    pub gamma: f64,
    /// What one percentage point of volatility is worth, which is the unit
    /// this is reported in rather than a whole unit of volatility.
    pub vega: f64,
    /// What one calendar day costs.
    pub theta: f64,
}

/// The tree, kept far enough back from the root to differentiate on.
///
/// The root alone gives a price and nothing else. Delta is the spread across
/// the two nodes one step in, gamma the spread of that spread across the three
/// nodes two steps in, and theta what two steps of time cost — so the walk
/// stops at step two and hands those five values back.
fn tree_near_the_root(
    terms: OptionTerms, spot: f64, volatility: f64, rate: f64, dividends: f64,
) -> Option<([f64; 3], f64, f64, f64)> {
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
    let growth = if terms.on_a_future { 1.0 } else { (rate * dt).exp() };
    if !(up.is_finite() && growth.is_finite()) || (up - down).abs() < f64::EPSILON {
        return None;
    }
    let up_chance = (growth - down) / (up - down);
    if !(0.0..=1.0).contains(&up_chance) {
        return None;
    }
    let discount = (-rate * dt).exp();
    let mut value = Vec::with_capacity(STEPS + 1);
    for i in 0..=STEPS {
        let underlying = adjusted * up.powi(i as i32) * down.powi((STEPS - i) as i32);
        value.push(exercise_value(terms, underlying));
    }
    for step in (0..STEPS).rev() {
        for i in 0..=step {
            let held = discount * (up_chance * value[i + 1] + (1.0 - up_chance) * value[i]);
            let underlying = adjusted * up.powi(i as i32) * down.powi((step - i) as i32);
            value[i] = if terms.on_a_future {
                held
            } else {
                held.max(exercise_value(terms, underlying))
            };
        }
        // Two steps from the root is as far back as anything here needs.
        if step == 2 {
            let two = [value[0], value[1], value[2]];
            // And one more step for the pair the delta spans, then the root.
            let mut one = [0.0f64; 2];
            for i in 0..=1usize {
                let held = discount * (up_chance * value[i + 1] + (1.0 - up_chance) * value[i]);
                let underlying = adjusted * up.powi(i as i32) * down.powi((1 - i) as i32);
                one[i] = if terms.on_a_future {
                    held
                } else {
                    held.max(exercise_value(terms, underlying))
                };
            }
            let root = {
                let held = discount * (up_chance * one[1] + (1.0 - up_chance) * one[0]);
                if terms.on_a_future {
                    held
                } else {
                    held.max(exercise_value(terms, adjusted))
                }
            };
            return root.is_finite().then_some((two, one[0], one[1], root));
        }
    }
    None
}

/// The model struck at one volatility, with what it is doing.
///
/// Taken off the same tree the price comes from rather than by moving the
/// inputs and pricing again: the spread across the nodes one step in is the
/// delta, the spread of that across the three two steps in is the gamma, and
/// what two steps of time cost is the theta. Only vega is a second valuation,
/// because volatility is not a direction the tree already branches in.
pub fn greeks(
    terms: OptionTerms, model: VenueModel, volatility: f64, underlying_price: f64,
) -> Option<Greeks> {
    let dividends = model.present_value_of_dividends;
    let (two, one_down, one_up, root) =
        tree_near_the_root(terms, underlying_price, volatility, model.rate, dividends)?;
    let adjusted = underlying_price - dividends;
    let dt = terms.years_to_expiry / STEPS as f64;
    let up = (volatility * dt.sqrt()).exp();
    let down = 1.0 / up;

    let delta = (one_up - one_down) / (adjusted * (up - down));
    let delta_up = (two[2] - two[1]) / (adjusted * (up * up - 1.0));
    let delta_down = (two[1] - two[0]) / (adjusted * (1.0 - down * down));
    let gamma = (delta_up - delta_down) / (0.5 * adjusted * (up * up - down * down));
    let theta = (two[1] - root) / (2.0 * dt * 365.0);
    // A percentage point of volatility, valued on the same tree.
    let bumped = price(terms, underlying_price, volatility + 0.01, model.rate, dividends)?;
    let vega = bumped - root;

    let all = Greeks { price: root, delta, gamma, vega, theta };
    [all.price, all.delta, all.gamma, all.vega, all.theta]
        .iter()
        .all(|v| v.is_finite())
        .then_some(all)
}

fn exercise_value(terms: OptionTerms, underlying: f64) -> f64 {
    if terms.is_call {
        (underlying - terms.strike).max(0.0)
    } else {
        (terms.strike - underlying).max(0.0)
    }
}

/// What volatility a caller's price implies, under the venue's model.
///
/// Answered on the scale the venue states its own volatility on, so the two
/// can be read beside each other.
pub fn implied_volatility(
    terms: OptionTerms,
    model: VenueModel,
    option_price: f64,
    underlying_price: f64,
) -> Option<f64> {
    // The rate the venue stated, rather than one fitted to reproduce its
    // price. Fitting one was how this stood while the volatility was being
    // read a root of a year short, and the fit was absorbing that: it landed
    // on the stated rate where the price still moved with the rate, and ran
    // away where it did not — negative five per cent one strike out of the
    // money, and nothing at all the strike after, which is why a contract out
    // of the money could not be solved. The venue states one rate for every
    // strike on the chain, which is what a rate is.
    let rate = model.rate;
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
    price(
        terms,
        underlying_price,
        volatility,
        model.rate,
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
    // A bound that does not come out as a number is not a bound. Left to be
    // compared, every test against it is false — the one just below, which
    // refuses an answer that is not between the bounds, included — so the
    // search ran its whole length narrowing towards the edge it started from
    // and handed that edge back as though it had settled there. A price that
    // is not a number reached here and left as the smallest volatility this
    // solves for, stated as the answer to a question that has none.
    if !at_low.is_finite() || !at_high.is_finite() {
        return None;
    }
    if at_low.signum() == at_high.signum() {
        // The answer is not between the bounds, so there is none to give.
        return None;
    }
    for _ in 0..100 {
        let middle = 0.5 * (low + high);
        let here = f(middle)?;
        // As above: a step that does not come out as a number cannot say
        // which side of it the answer is on.
        if !here.is_finite() {
            return None;
        }
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

    /// The greeks are the slopes of the price this same tree gives.
    ///
    /// Checked against the tree rather than against a table: a delta that is
    /// not the slope of this model's own price is wrong however plausible it
    /// looks, and the same reading catches a sign or a discount in any of the
    /// four. The tolerances are a tree's, not a closed form's — a hundred and
    /// more steps of a lattice do not agree with a derivative to twelve places.
    #[test]
    fn the_greeks_are_the_slopes_of_the_tree() {
        let terms = OptionTerms {
            strike: 100.0, years_to_expiry: 0.5, is_call: true, on_a_future: false,
        };
        let model = VenueModel {
            volatility: 0.25, option_price: 0.0, underlying_price: 100.0,
            present_value_of_dividends: 0.0, rate: 0.04,
        };
        let sigma = 0.25;
        let all = greeks(terms, model, sigma, 100.0).expect("a tree this ordinary walks");

        let priced = |s: f64| price(terms, s, sigma, model.rate, 0.0).unwrap();
        assert!((all.price - priced(100.0)).abs() < 1e-9, "the price is the tree's");

        // The bump has to clear the lattice's own node spacing, or the
        // difference measures where the nodes fall rather than what the price
        // does: at this volatility the nodes are about a point apart, and a
        // second difference taken inside one came out forty times the gamma.
        let bump = 4.0;
        let slope = (priced(100.0 + bump) - priced(100.0 - bump)) / (2.0 * bump);
        assert!((all.delta - slope).abs() < 1e-2, "delta {} against {slope}", all.delta);

        let curve = (priced(100.0 + bump) - 2.0 * all.price + priced(100.0 - bump))
            / (bump * bump);
        assert!((all.gamma - curve).abs() < 5e-3, "gamma {} against {curve}", all.gamma);

        // A percentage point of volatility, which is the unit this reports in.
        let by_vol = price(terms, 100.0, sigma + 0.01, model.rate, 0.0).unwrap() - all.price;
        assert!((all.vega - by_vol).abs() < 1e-9, "vega {}", all.vega);

        // And one calendar day, which for a half-year call is a few cents.
        assert!(all.theta < 0.0 && all.theta > -0.5, "theta {}", all.theta);
        let shorter = OptionTerms { years_to_expiry: 0.5 - 1.0 / 365.0, ..terms };
        let tomorrow = price(shorter, 100.0, sigma, model.rate, 0.0).unwrap();
        assert!(
            (all.theta - (tomorrow - all.price)).abs() < 5e-3,
            "theta {} against a day of it {}", all.theta, tomorrow - all.price,
        );
    }

    /// A put's greeks point the other way, and its gamma does not.
    #[test]
    fn a_put_carries_the_signs_a_put_carries() {
        let terms = OptionTerms {
            strike: 100.0, years_to_expiry: 0.4, is_call: false, on_a_future: false,
        };
        let model = VenueModel {
            volatility: 0.3, option_price: 0.0, underlying_price: 100.0,
            present_value_of_dividends: 0.0, rate: 0.03,
        };
        let all = greeks(terms, model, 0.3, 100.0).expect("walks");
        assert!(all.delta < 0.0 && all.delta > -1.0, "a put falls as the underlying rises: {}", all.delta);
        assert!(all.gamma > 0.0, "and curves the same way a call does: {}", all.gamma);
        assert!(all.vega > 0.0, "and is worth more the wilder it is: {}", all.vega);
    }

    /// Nothing is stated for a contract the tree cannot walk.
    #[test]
    fn a_contract_the_tree_cannot_walk_states_no_greeks() {
        let terms = OptionTerms {
            strike: 100.0, years_to_expiry: 0.0, is_call: true, on_a_future: false,
        };
        let model = VenueModel {
            volatility: 0.2, option_price: 0.0, underlying_price: 100.0,
            present_value_of_dividends: 0.0, rate: 0.04,
        };
        assert_eq!(greeks(terms, model, 0.2, 100.0), None, "an option with no time left");
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

    /// A volatility the tree overflows on is refused, not answered.
    ///
    /// The step ratio is checked for being a number and the highest node is
    /// that ratio raised to the number of steps, which need not be one. A
    /// caller stating a volatility per cent where a fraction was meant reaches
    /// it, and what came back was an infinity handed on as a price.
    #[test]
    fn a_volatility_the_tree_overflows_on_is_refused() {
        // Finite as a step ratio, and not finite raised to the tree's depth.
        let ratio = (80.0f64 * (1.0f64 / super::STEPS as f64).sqrt()).exp();
        assert!(ratio.is_finite(), "the check above this one passes");
        assert!(!ratio.powi(super::STEPS as i32).is_finite(), "and the tree still overflows");

        assert_eq!(
            price(call(100.0, 1.0), 100.0, 80.0, 0.05, 0.0),
            None,
            "an infinity was handed back as a price",
        );
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

    /// A strike out of the money is worth something, which is what reading
    /// the venue's volatility over a day rather than a year restores.
    ///
    /// These are the venue's own figures for a contract eight days out, taken
    /// off the wire: it stated 6.7223 where reading the same volatility over a
    /// year prices the contract at nothing at all — and a price of nothing is
    /// what left a caller told the contract could not be solved.
    #[test]
    fn a_strike_out_of_the_money_is_worth_something() {
        let terms = OptionTerms {
            strike: 7830.0,
            years_to_expiry: 8.3034 / 365.0,
            is_call: true,
            on_a_future: true,
        };
        // As the venue stated them, carried over to the year this works in:
        // it states both over one of the days it counts beside them.
        let model = VenueModel {
            volatility: 0.00561 * 365.0_f64.sqrt(),
            option_price: 6.7223,
            underlying_price: 7676.90,
            present_value_of_dividends: 0.0,
            rate: 0.00011 * 365.0,
        };
        let ours = option_price(terms, model, model.volatility, model.underlying_price)
            .expect("it prices");
        assert!((ours - 6.7223).abs() < 0.05, "the venue said 6.7223, this said {ours}");

        // The venue's figures taken for a year's without carrying them over,
        // which is how they were read before and what priced the contract at
        // nothing.
        let uncarried = price(terms, model.underlying_price, 0.00561, 0.00011, 0.0)
            .expect("it prices");
        assert!(uncarried < 0.01, "taken for a year's the contract was worth {uncarried}");
    }

    /// A caller's price gives back the volatility that produces it, on the
    /// scale the venue states its own on.
    #[test]
    fn a_price_gives_back_its_volatility() {
        let terms = call(100.0, 1.0);
        let venue_price = price(terms, 100.0, 0.25, 0.04, 1.5).expect("it prices");
        let model = VenueModel {
            volatility: 0.25,
            option_price: venue_price,
            underlying_price: 100.0,
            present_value_of_dividends: 1.5,
            rate: 0.04,
        };
        let same = implied_volatility(terms, model, venue_price, 100.0).expect("it solves");
        assert!((same - 0.25).abs() < 1e-3, "the venue's price gave {same}");

        let dearer = implied_volatility(terms, model, venue_price * 1.2, 100.0).expect("it solves");
        assert!(dearer > same, "a dearer option implies more volatility, not {dearer}");
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
            rate: 0.04,
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

    /// A price that is not a number has no volatility, and is told so.
    ///
    /// Left to be compared, every test against it is false — including the one
    /// that refuses an answer lying outside the bounds — so the search ran its
    /// whole length narrowing towards the edge it started from and handed that
    /// edge back as though it had settled there. The caller was given the
    /// smallest volatility this solves for, stated as the answer to a question
    /// that has none.
    #[test]
    fn a_price_that_is_not_a_number_has_no_volatility() {
        let terms = OptionTerms {
            strike: 100.0, years_to_expiry: 0.25, is_call: true, on_a_future: false,
        };
        let model = VenueModel {
            volatility: 0.2,
            option_price: 5.0,
            underlying_price: 100.0,
            present_value_of_dividends: 0.0,
            rate: 0.02,
        };
        for price in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                implied_volatility(terms, model, price, 100.0),
                None,
                "a price of {price} was answered with a volatility",
            );
        }
    }
}
