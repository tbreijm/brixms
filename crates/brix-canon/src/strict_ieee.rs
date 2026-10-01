//! Finite binary64 arithmetic for decision values, isolated from key encodings.
//!
//! Supported Rust targets must implement IEEE-754 binary64 basic arithmetic
//! with roundTiesToEven and gradual underflow (the default on supported x86_64
//! and aarch64 targets). Hosts must not change rounding or flush-to-zero modes.
//! No fast-math, fused operations, or platform-dependent transcendental calls
//! are used. Every operation rounds separately; nonfinite results fail closed.
//! See Rust's [IEEE arithmetic contract](https://doc.rust-lang.org/std/primitive.f32.html)
//! and [binary64 parsing contract](https://doc.rust-lang.org/std/primitive.f64.html#impl-FromStr-for-f64).

use crate::{total_order_key_f64, CanonWriter, Canonical, NumericError};
use std::{cmp::Ordering, fmt, str::FromStr};

/// A finite binary64 decision value. Negative zero is normalized to positive
/// zero. Its fixed-width value encoding is not a canon/1 entity-key encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FiniteF64(u64);

impl FiniteF64 {
    pub fn from_bits(bits: u64) -> Result<Self, NumericError> {
        Self::new(f64::from_bits(bits))
    }

    fn new(value: f64) -> Result<Self, NumericError> {
        if !value.is_finite() {
            return Err(NumericError::NonFinite);
        }
        Ok(Self(if value == 0.0 { 0 } else { value.to_bits() }))
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Explicit integer conversion, rounded to nearest with ties to even.
    pub fn from_i64(value: i64) -> Self {
        Self((value as f64).to_bits())
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, NumericError> {
        Self::new(f64::from_bits(self.0) + f64::from_bits(rhs.0))
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self, NumericError> {
        Self::new(f64::from_bits(self.0) - f64::from_bits(rhs.0))
    }

    pub fn checked_mul(self, rhs: Self) -> Result<Self, NumericError> {
        Self::new(f64::from_bits(self.0) * f64::from_bits(rhs.0))
    }

    pub fn checked_div(self, rhs: Self) -> Result<Self, NumericError> {
        if rhs.0 == 0 {
            return Err(NumericError::DivisionByZero);
        }
        Self::new(f64::from_bits(self.0) / f64::from_bits(rhs.0))
    }

    pub fn checked_neg(self) -> Result<Self, NumericError> {
        Self::new(-f64::from_bits(self.0))
    }
}

impl FromStr for FiniteF64 {
    type Err = NumericError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::new(
            text.parse::<f64>()
                .map_err(|_| NumericError::InvalidNumber)?,
        )
    }
}

impl fmt::Display for FiniteF64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f64::from_bits(self.0).fmt(f)
    }
}

impl Ord for FiniteF64 {
    fn cmp(&self, rhs: &Self) -> Ordering {
        total_order_key_f64(f64::from_bits(self.0)).cmp(&total_order_key_f64(f64::from_bits(rhs.0)))
    }
}

impl PartialOrd for FiniteF64 {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        Some(self.cmp(rhs))
    }
}

impl Canonical for FiniteF64 {
    fn canon_write(&self, writer: &mut CanonWriter) {
        writer.write_raw(&self.0.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn n(text: &str) -> FiniteF64 {
        text.parse().unwrap()
    }

    #[test]
    fn rejects_nonfinite_and_normalizes_zero() {
        for text in ["NaN", "inf", "-inf", "1e309", "", " 1"] {
            assert!(text.parse::<FiniteF64>().is_err(), "{text}");
        }
        assert!(FiniteF64::from_bits(0x7ff0_0000_0000_0000).is_err());
        assert_eq!(n("-0"), n("0"));
        assert_eq!(n("-0").canon_bytes(), [0; 8]);
        assert_eq!(n("1").canon_bytes(), 0x3ff0_0000_0000_0000u64.to_be_bytes());
        assert_eq!(
            n("1").checked_div(n("-0")),
            Err(NumericError::DivisionByZero)
        );
        assert_eq!(n("1e308").checked_mul(n("2")), Err(NumericError::NonFinite));
    }

    #[test]
    fn ieee_rounding_and_subnormals() {
        assert_eq!(
            n("0.1").checked_add(n("0.2")).unwrap().to_string(),
            "0.30000000000000004"
        );
        assert_eq!(
            n("9007199254740992").checked_add(n("1")).unwrap(),
            n("9007199254740992")
        );
        assert_eq!(FiniteF64::from_i64(9007199254740993), n("9007199254740992"));
        let smallest = FiniteF64::from_bits(1).unwrap();
        assert_eq!(smallest.checked_div(n("2")).unwrap(), n("0"));
        assert_eq!(smallest.checked_mul(n("2")).unwrap().bits(), 2);
        assert_eq!(
            n("1").checked_div(n("3")).unwrap().bits(),
            0x3fd5_5555_5555_5555
        );
        assert_eq!(
            n("1.0000000000000002").checked_sub(n("1")).unwrap().bits(),
            0x3cb0_0000_0000_0000
        );
        assert_eq!(
            n("1")
                .checked_add(n("1.1102230246251565e-16"))
                .unwrap()
                .bits(),
            0x3ff0_0000_0000_0000
        );
        assert!(n("-2") < n("-1"));
        assert!(n("-1") < n("0"));
    }

    proptest! {
        #[test]
        fn text_roundtrips(bits: u64) {
            if let Ok(value) = FiniteF64::from_bits(bits) {
                prop_assert_eq!(value.to_string().parse::<FiniteF64>().unwrap(), value);
            }
        }
    }
}
