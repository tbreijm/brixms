//! Dimensioned quantities and money (Appendix G).
//!
//! Appendix G:
//! - "quantities: normalized to the measure's base unit, value + measure
//!   identifier";
//! - "money: currency code + minor-unit integer".
//!
//! Both carry an exact [`crate::Decimal`]-or-integer value, never a float
//! (Part V §7, §8: "exact dimensional quantities, and exact money values may
//! have canonical identity. Floating values may not be used in keys"). A
//! [`Quantity`] is stored already-normalized to its measure's base unit, so two
//! quantities denoting the same physical amount encode identically regardless of
//! the unit they were written in. [`Money`] is the currency code plus an integer
//! count of minor units (e.g. cents), so `EUR 1.00` and `EUR 100` minor units
//! are the same value with one encoding.

use crate::{CanonWriter, Canonical, Decimal};

/// An exact dimensioned quantity, normalized to its measure's base unit.
///
/// `measure` is the measure (dimension) identifier — e.g. `"Length"`, not
/// `"kilometre"` — because the value is already expressed in the base unit. The
/// canonical encoding is `measure identifier (ident) ++ base-unit value`, in
/// that order, so identity is stable across the surface unit a program happened
/// to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quantity {
    /// Measure/dimension identifier (NFC-folded on encode).
    pub measure: String,
    /// Exact value in the measure's base unit.
    pub value: Decimal,
}

impl Quantity {
    /// A quantity of `value` base units of `measure`.
    pub fn new(measure: impl Into<String>, value: Decimal) -> Self {
        Quantity {
            measure: measure.into(),
            value,
        }
    }
}

impl Canonical for Quantity {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_ident(&self.measure);
        self.value.canon_write(w);
    }
}

/// An exact money value: an ISO-style currency code plus a count of minor units.
///
/// The canonical encoding is `currency code (ident) ++ minor-unit integer`.
/// Storing minor units as an integer (rather than a major-unit decimal) is what
/// App. G means by "currency code + minor-unit integer" and keeps money exact
/// and float-free. `minor_units` is signed so debits/credits both encode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Money {
    /// Currency code, e.g. `"EUR"` (NFC-folded on encode).
    pub currency: String,
    /// Amount in the currency's minor unit (e.g. cents).
    pub minor_units: i128,
}

impl Money {
    /// `minor_units` minor units of `currency`.
    pub fn new(currency: impl Into<String>, minor_units: i128) -> Self {
        Money {
            currency: currency.into(),
            minor_units,
        }
    }
}

impl Canonical for Money {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_ident(&self.currency);
        w.write_int128(self.minor_units);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantity_measure_not_unit() {
        // 1500 metres and 1.5 km both normalize to 1500 base-unit metres, so if
        // both are supplied already-normalized they encode identically.
        let a = Quantity::new("Length", Decimal::new(1500, 0));
        let b = Quantity::new("Length", Decimal::new(15000, 1)); // 1500.0
        assert_eq!(a.canon_bytes(), b.canon_bytes());
    }

    #[test]
    fn money_minor_units_exact() {
        let a = Money::new("EUR", 100); // 100 cents
        let b = Money::new("EUR", 100);
        assert_eq!(a.canon_bytes(), b.canon_bytes());
        let c = Money::new("USD", 100);
        assert_ne!(a.canon_bytes(), c.canon_bytes(), "currency separates money");
    }
}
