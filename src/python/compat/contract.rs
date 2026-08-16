//! ibapi-compatible Contract, Order, TagValue, and condition classes.


// The classes split by family, published here because that is the path
// everything names them by.
pub use super::{class_conditions::*, class_contracts::*, class_orders::*, class_reports::*};
// Named rather than globbed: the engine's own order type shares the name, and
// a glob loses to it.
pub use super::class_orders::Order;

use pyo3::prelude::*;


// ── Contract ──

impl From<&Contract> for crate::types::ContractRef {
    /// What a request names, taken through the shape the rest of the client
    /// uses so there is one definition of which fields identify a contract.
    fn from(c: &Contract) -> Self {
        Self::from(&c.to_api())
    }
}

/// Set a field on a freshly made object from what a caller named it.
///
/// The reference client names a contract's fields one way and this binding
/// holds them under both, so a program written against that client can set
/// them by the names it already uses. The constructor took only one spelling,
/// which meant the very first line of a ported program — a contract with a
/// `secType` on it — failed before anything reached the venue.
pub(super) fn set_from_keywords(
    object: &Bound<'_, PyAny>,
    keywords: Option<&Bound<'_, pyo3::types::PyDict>>,
) -> PyResult<()> {
    let Some(keywords) = keywords else { return Ok(()) };
    for (name, value) in keywords.iter() {
        let name: String = name.extract()?;
        if object.setattr(name.as_str(), &value).is_err() {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                "no such field: {name}",
            )));
        }
    }
    Ok(())
}

// ── Order ──

// ── Conversions between Python compat types and Rust API types ──

// ── TagValue ──

// ── OrderAllocation (per-account allocation in OrderState) ──

// ── OrderState (for what-if responses) ──

// ── Order Conditions ──

// ── BarData ──

// ── ContractDetails ──

// ── Execution ──

// ── SmartComponent ──

// ── NewsProvider ──

// ── SoftDollarTier ──

// ── CommissionAndFeesReport ──

// ── ContractDescription ──

/// Register all compat contract/order classes on the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Contract>()?;
    m.add_class::<Order>()?;
    m.add_class::<OptionChain>()?;
    m.add_class::<TagValue>()?;
    m.add_class::<OrderState>()?;
    m.add_class::<OrderAllocation>()?;
    m.add_class::<PriceCondition>()?;
    m.add_class::<TimeCondition>()?;
    m.add_class::<MarginCondition>()?;
    m.add_class::<ExecutionCondition>()?;
    m.add_class::<VolumeCondition>()?;
    m.add_class::<PercentChangeCondition>()?;
    m.add_class::<BarData>()?;
    m.add_class::<ContractDetails>()?;
    m.add_class::<ContractDescription>()?;
    m.add_class::<CommissionAndFeesReport>()?;
    m.add_class::<Execution>()?;
    m.add_class::<SmartComponentPy>()?;
    m.add_class::<NewsProviderPy>()?;
    m.add_class::<SoftDollarTierPy>()?;
    m.add_class::<DepthMktDataDescriptionPy>()?;
    m.add_class::<PriceIncrementPy>()?;
    Ok(())
}

/// The reference client spells a field by running its words together, and for a
/// few of them it also chose a different word than this crate did. Code written
/// against that client reads those spellings, so they resolve here too — while a
/// name that names no field is still refused.
pub(super) fn by_reference_name(
    obj: &Bound<'_, PyAny>,
    name: &str,
    aliases: &[(&str, &str)],
) -> PyResult<Py<PyAny>> {
    if let Some((_, ours)) = aliases.iter().find(|(theirs, _)| *theirs == name)
        && let Ok(v) = obj.getattr(*ours)
    {
        return Ok(v.unbind());
    }
    let mut snake = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                snake.push('_');
            }
            snake.extend(c.to_lowercase());
        } else {
            snake.push(c);
        }
    }
    if snake != name
        && let Ok(v) = obj.getattr(snake.as_str())
    {
        return Ok(v.unbind());
    }
    Err(pyo3::exceptions::PyAttributeError::new_err(format!(
        "object has no attribute '{name}'"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::types::PRICE_SCALE_F;
    use crate::types::{ControlCommand, OrderCondition, OrderKind, OrderRequest, Price, Side};
    use crate::client_core::ClientCore;

    /// The values an order carries when the caller states none, as written in
    /// the file that states them.
    fn order_defaults(source: &str) -> Vec<String> {
        // A line rather than a substring, so this cannot match the search
        // string in its own source and go on to compare a test against a
        // struct — which is exactly what it did the day the impl moved.
        let head = "\nimpl Default for Order {\n";
        let at = source.find(head).expect("no Order default in this file");
        let body = &source[at + 1..];
        let end = body.find("\n}").expect("unterminated impl");
        let lines: Vec<String> = body[..end]
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
            .map(str::to_string)
            .collect();
        assert!(
            lines.iter().any(|l| l == "fn default() -> Self {"),
            "read something that is not a Default impl: {:?}",
            &lines[..lines.len().min(3)],
        );
        lines
    }

    /// A hundred and fifty-four defaults, written out on both surfaces because
    /// one of them is a Python class and cannot borrow the other's. Written
    /// twice, they can drift, and an order placed through one surface would
    /// then reach the venue stating something an order placed through the
    /// other does not. So they are compared rather than trusted.
    #[test]
    fn both_surfaces_state_the_same_order_defaults() {
        let rust = order_defaults(include_str!("../../types/model.rs"));
        let python = order_defaults(include_str!("class_orders.rs"));
        assert!(rust.len() > 150, "read {} lines, expected the whole block", rust.len());
        assert_eq!(rust, python);
    }

    /// A TRAIL LIMIT states how far its limit sits from the trigger. Unset is
    /// `f64::MAX` on both sides, so a value dropped in conversion is not merely
    /// lost, it is indistinguishable from absent, and the wire falls back to
    /// `lmt_price` — the order goes out with an offset the caller never chose.
    #[test]
    fn a_python_trail_limit_offset_survives_the_conversion() {
        let o = Order {
            order_type: "TRAIL LIMIT".into(),
            lmt_price_offset: 0.25,
            ..Default::default()
        };

        let api = o.to_api();

        assert_eq!(api.lmt_price_offset, 0.25, "the offset the caller set must reach the wire");
        assert_ne!(
            api.lmt_price_offset, f64::MAX,
            "and must not arrive as the unset sentinel, which reads as never supplied",
        );
    }

    #[test]
    fn contract_default_values() {
        // The defaults ibx actually presents, which are the ones its `#[new]`
        // signature declares — not ibapi's empty strings. This test asserted the
        // latter and had never run to say otherwise.
        let c = Contract::default();
        assert_eq!(c.con_id, 0);
        assert_eq!(c.symbol, "");
        assert_eq!(c.sec_type, "STK");
        assert_eq!(c.exchange, "SMART");
        assert_eq!(c.currency, "USD");
        assert_eq!(c.strike, 0.0);
    }

    #[test]
    fn order_default_values() {
        let o = Order::default();
        assert_eq!(o.order_id, 0);
        assert_eq!(o.action, "");
        assert_eq!(o.total_quantity, 0.0);
        assert_eq!(o.order_type, "");
        assert_eq!(o.tif, "DAY");
        assert!(o.transmit);
        assert!(!o.what_if);
        assert!(!o.outside_rth);
    }

    #[test]
    fn order_side_parsing() {
        let mut o = Order { action: "BUY".into(), ..Default::default() };
        assert_eq!(o.to_api().side().unwrap(), Side::Buy);
        o.action = "SELL".into();
        assert_eq!(o.to_api().side().unwrap(), Side::Sell);
        o.action = "SSHORT".into();
        assert_eq!(o.to_api().side().unwrap(), Side::ShortSell);
    }

    #[test]
    fn order_tif_byte_mapping() {
        let mut o = Order { tif: "DAY".into(), ..Default::default() };
        assert_eq!(o.to_api().tif_byte(), b'0');
        o.tif = "GTC".into();
        assert_eq!(o.to_api().tif_byte(), b'1');
        o.tif = "IOC".into();
        assert_eq!(o.to_api().tif_byte(), b'3');
        o.tif = "FOK".into();
        assert_eq!(o.to_api().tif_byte(), b'4');
    }

    #[test]
    fn order_has_extended_attrs() {
        let o = Order::default();
        assert!(!o.to_api().has_extended_attrs());

        let o2 = Order { hidden: true, ..Default::default() };
        assert!(o2.to_api().has_extended_attrs());
    }

    #[test]
    fn order_attrs_conversion() {
        let o = Order {
            display_size: 50,
            hidden: true,
            discretionary_amt: 0.05,
            ..Default::default()
        };
        let attrs = o.to_api().attrs();
        assert_eq!(attrs.display_size, 50);
        assert!(attrs.hidden);
        assert_eq!(attrs.discretionary_amt, (0.05 * PRICE_SCALE_F) as Price);
    }

    /// A TRAIL LIMIT carries all three fields, and each reaches the wire as a
    /// distinct value, so the assertions cannot pass on a
    /// default: the limit offset is tag 6370 and falls back to `lmt_price`
    /// when unset, which is why the two are set to different numbers here.
    #[test]
    fn to_api_carries_oca_type_trail_stop_and_lmt_price_offset() {
        let o = Order {
            action: "BUY".into(),
            total_quantity: 1.0,
            order_type: "TRAIL LIMIT".into(),
            lmt_price: 10.0,
            lmt_price_offset: 0.5,
            aux_price: 1.0,
            trail_stop_price: 99.0,
            oca_type: 2,
            ..Default::default()
        };

        let cmd = ClientCore::build_order_request(&o.to_api(), 1, 0, None).unwrap();
        let ControlCommand::Order(OrderRequest::SubmitEx {
            kind: OrderKind::TrailingStopLimit { lmt_offset, trail_stop_price, .. },
            attrs,
            ..
        }) = cmd else { panic!("TRAIL LIMIT must build a TrailingStopLimit request") };

        assert_eq!(lmt_offset, (0.5 * PRICE_SCALE_F) as Price);
        assert_eq!(trail_stop_price, (99.0 * PRICE_SCALE_F) as Price);
        assert_eq!(attrs.oca_type, 2);
    }

    #[test]
    fn tag_value_fields() {
        let tv = TagValue { tag: "maxPctVol".into(), value: "0.1".into() };
        assert_eq!(tv.tag, "maxPctVol");
        assert_eq!(tv.value, "0.1");
    }

    #[test]
    fn price_condition_to_internal() {
        let pc = PriceCondition {
            con_id: 265598,
            exchange: "SMART".into(),
            price: 200.0,
            is_more: true,
            trigger_method: 1,
        };
        match pc.to_internal() {
            OrderCondition::Price { con_id, price, is_more, trigger_method, .. } => {
                assert_eq!(con_id, 265598);
                assert_eq!(price, (200.0 * PRICE_SCALE_F) as Price);
                assert!(is_more);
                assert_eq!(trigger_method, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn time_condition_to_internal() {
        let tc = TimeCondition { time: "20260311-09:30:00".into(), is_more: true };
        match tc.to_internal() {
            OrderCondition::Time { time, is_more } => {
                assert_eq!(time, "20260311-09:30:00");
                assert!(is_more);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn volume_condition_to_internal() {
        let vc = VolumeCondition {
            con_id: 265598,
            exchange: "SMART".into(),
            volume: 1_000_000,
            is_more: true,
        };
        match vc.to_internal() {
            OrderCondition::Volume { con_id, volume, is_more, .. } => {
                assert_eq!(con_id, 265598);
                assert_eq!(volume, 1_000_000);
                assert!(is_more);
            }
            _ => panic!("wrong variant"),
        }
    }
}
