//! Checked exact-decimal decision arithmetic, without changing the frozen
//! arbitrary-scale [`Decimal`] encoding. Decision values admit scale 0..=18.

use crate::Decimal;
use std::fmt;

pub const MAX_DECIMAL_SCALE: u8 = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericError {
    InvalidNumber,
    NonFinite,
    Overflow,
    ScaleOutOfRange,
    DivisionByZero,
    InexactDivision,
    InvalidRoundingMode,
}

impl fmt::Display for NumericError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidNumber => "invalid numeric text",
            Self::NonFinite => "F64 requires a finite result",
            Self::Overflow => "decimal arithmetic overflow",
            Self::ScaleOutOfRange => "decimal scale exceeds 18",
            Self::DivisionByZero => "division by zero",
            Self::InexactDivision => {
                "division is not exact at scale <= 18; use decimal_div with explicit rounding"
            }
            Self::InvalidRoundingMode => "rounding must be floor, ceil, half_even, or trunc",
        })
    }
}

impl std::error::Error for NumericError {}

fn admitted(value: Decimal) -> Result<Decimal, NumericError> {
    if value.scale() > MAX_DECIMAL_SCALE {
        Err(NumericError::ScaleOutOfRange)
    } else {
        Ok(value)
    }
}

fn signed(magnitude: u128, negative: bool) -> Result<i128, NumericError> {
    if negative && magnitude == (1u128 << 127) {
        return Ok(i128::MIN);
    }
    let value = i128::try_from(magnitude).map_err(|_| NumericError::Overflow)?;
    Ok(if negative { -value } else { value })
}

/// Parse `[+-]digits[.digits]`, normalizing trailing fractional zeros before
/// enforcing the scale bound. Exponents, whitespace, and separators are rejected.
pub fn decimal_parse(text: &str) -> Result<Decimal, NumericError> {
    let (negative, unsigned) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (integer, fractional) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if integer.is_empty()
        || !integer.bytes().all(|b| b.is_ascii_digit())
        || !fractional.bytes().all(|b| b.is_ascii_digit())
        || (unsigned.contains('.') && fractional.is_empty())
    {
        return Err(NumericError::InvalidNumber);
    }
    let fractional = fractional.trim_end_matches('0');
    if fractional.len() > MAX_DECIMAL_SCALE as usize {
        return Err(NumericError::ScaleOutOfRange);
    }
    let mut magnitude = 0u128;
    for digit in integer.bytes().chain(fractional.bytes()) {
        magnitude = magnitude
            .checked_mul(10)
            .and_then(|v| v.checked_add((digit - b'0') as u128))
            .ok_or(NumericError::Overflow)?;
    }
    Ok(Decimal::new(
        signed(magnitude, negative)?,
        fractional.len() as u8,
    ))
}

pub fn decimal_format(value: Decimal) -> String {
    let mut digits = value.unscaled().unsigned_abs().to_string();
    let scale = value.scale() as usize;
    if scale > 0 {
        if digits.len() <= scale {
            digits = format!("{}{}", "0".repeat(scale + 1 - digits.len()), digits);
        }
        digits.insert(digits.len() - scale, '.');
    }
    if value.unscaled() < 0 {
        digits.insert(0, '-');
    }
    digits
}

pub fn decimal_from_i64(value: i64) -> Decimal {
    Decimal::new(value as i128, 0)
}

fn align(a: Decimal, b: Decimal) -> Result<(i128, i128, u8), NumericError> {
    admitted(a)?;
    admitted(b)?;
    let scale = a.scale().max(b.scale());
    let rescale = |v: Decimal| {
        v.unscaled()
            .checked_mul(10i128.pow((scale - v.scale()) as u32))
            .ok_or(NumericError::Overflow)
    };
    Ok((rescale(a)?, rescale(b)?, scale))
}

pub fn decimal_add(a: Decimal, b: Decimal) -> Result<Decimal, NumericError> {
    let (a, b, scale) = align(a, b)?;
    Ok(Decimal::new(
        a.checked_add(b).ok_or(NumericError::Overflow)?,
        scale,
    ))
}

pub fn decimal_sub(a: Decimal, b: Decimal) -> Result<Decimal, NumericError> {
    let (a, b, scale) = align(a, b)?;
    Ok(Decimal::new(
        a.checked_sub(b).ok_or(NumericError::Overflow)?,
        scale,
    ))
}

pub fn decimal_neg(value: Decimal) -> Result<Decimal, NumericError> {
    admitted(value)?;
    Ok(Decimal::new(
        value
            .unscaled()
            .checked_neg()
            .ok_or(NumericError::Overflow)?,
        value.scale(),
    ))
}

pub fn decimal_mul(a: Decimal, b: Decimal) -> Result<Decimal, NumericError> {
    admitted(a)?;
    admitted(b)?;
    admitted(Decimal::new(
        a.unscaled()
            .checked_mul(b.unscaled())
            .ok_or(NumericError::Overflow)?,
        a.scale() + b.scale(),
    ))
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

// Cancel common factors before scaling; unsigned magnitudes admit i128::MIN.
// Even these intermediate magnitudes use checked arithmetic and never round.
fn ratio(a: Decimal, b: Decimal, output_scale: u8) -> Result<(u128, u128, bool), NumericError> {
    admitted(a)?;
    admitted(b)?;
    if b.unscaled() == 0 {
        return Err(NumericError::DivisionByZero);
    }
    let (mut numerator, mut denominator) =
        (a.unscaled().unsigned_abs(), b.unscaled().unsigned_abs());
    let common = gcd(numerator, denominator);
    numerator /= common;
    denominator /= common;
    let exponent = b.scale() as i16 + output_scale as i16 - a.scale() as i16;
    let factor = 10u128.pow(exponent.unsigned_abs() as u32);
    if exponent >= 0 {
        let common = gcd(factor, denominator);
        denominator /= common;
        numerator = numerator
            .checked_mul(factor / common)
            .ok_or(NumericError::Overflow)?;
    } else {
        let common = gcd(factor, numerator);
        numerator /= common;
        denominator = denominator
            .checked_mul(factor / common)
            .ok_or(NumericError::Overflow)?;
    }
    Ok((
        numerator,
        denominator,
        (a.unscaled() < 0) != (b.unscaled() < 0),
    ))
}

pub fn decimal_div_exact(a: Decimal, b: Decimal) -> Result<Decimal, NumericError> {
    let (numerator, denominator, negative) = ratio(a, b, 0)?;
    let mut coefficient = numerator / denominator;
    let mut remainder = numerator % denominator;
    let mut scale = 0;
    while remainder != 0 {
        if scale == MAX_DECIMAL_SCALE {
            return Err(NumericError::InexactDivision);
        }
        remainder = remainder.checked_mul(10).ok_or(NumericError::Overflow)?;
        coefficient = coefficient
            .checked_mul(10)
            .and_then(|v| v.checked_add(remainder / denominator))
            .ok_or(NumericError::Overflow)?;
        remainder %= denominator;
        scale += 1;
    }
    Ok(Decimal::new(signed(coefficient, negative)?, scale))
}

/// Divide at the requested scale using an explicit rounding rule. The returned
/// value is normalized: scale specifies rounding precision, not display padding.
pub fn decimal_div_round(
    a: Decimal,
    b: Decimal,
    scale: u8,
    mode: &str,
) -> Result<Decimal, NumericError> {
    if scale > MAX_DECIMAL_SCALE {
        return Err(NumericError::ScaleOutOfRange);
    }
    if !matches!(mode, "floor" | "ceil" | "half_even" | "trunc") {
        return Err(NumericError::InvalidRoundingMode);
    }
    let (numerator, denominator, negative) = ratio(a, b, scale)?;
    let mut coefficient = numerator / denominator;
    let remainder = numerator % denominator;
    let increment = remainder != 0
        && match mode {
            "floor" => negative,
            "ceil" => !negative,
            "half_even" => {
                remainder > denominator - remainder
                    || (remainder == denominator - remainder && coefficient % 2 == 1)
            }
            _ => false,
        };
    if increment {
        coefficient = coefficient.checked_add(1).ok_or(NumericError::Overflow)?;
    }
    Ok(Decimal::new(signed(coefficient, negative)?, scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn d(text: &str) -> Decimal {
        decimal_parse(text).unwrap()
    }

    #[test]
    fn parse_format_and_boundaries() {
        for text in [
            "0",
            "-0.01",
            "170141183460469231731687303715884105727",
            "-170141183460469231731687303715884105728",
            "0.000000000000000001",
        ] {
            assert_eq!(decimal_format(d(text)), text);
        }
        assert_eq!(d("-0.0000"), Decimal::ZERO);
        assert_eq!(d("001.200000000000000000000"), d("1.2"));
        for text in ["", " 1", "1e2", ".1", "1.", "1.2.3", "NaN"] {
            assert_eq!(
                decimal_parse(text),
                Err(NumericError::InvalidNumber),
                "{text}"
            );
        }
        assert_eq!(
            decimal_parse("0.0000000000000000001"),
            Err(NumericError::ScaleOutOfRange)
        );
        assert_eq!(
            decimal_parse("170141183460469231731687303715884105728"),
            Err(NumericError::Overflow)
        );
    }

    #[test]
    fn exact_arithmetic_and_faults() {
        assert_eq!(decimal_add(d("0.1"), d("0.2")).unwrap(), d("0.3"));
        assert_eq!(decimal_sub(d("1"), d("0.02")).unwrap(), d("0.98"));
        assert_eq!(decimal_mul(d("1.25"), d("0.8")).unwrap(), d("1"));
        assert_eq!(decimal_div_exact(d("1"), d("8")).unwrap(), d("0.125"));
        assert_eq!(decimal_div_exact(d("0.1"), d("0.02")).unwrap(), d("5"));
        assert_eq!(
            decimal_div_exact(d("1"), d("3")),
            Err(NumericError::InexactDivision)
        );
        assert_eq!(
            decimal_div_exact(d("1"), d("0")),
            Err(NumericError::DivisionByZero)
        );
        assert_eq!(
            decimal_mul(d("0.0000000001"), d("0.0000000001")),
            Err(NumericError::ScaleOutOfRange)
        );
        let min = Decimal::new(i128::MIN, 0);
        assert_eq!(decimal_neg(min), Err(NumericError::Overflow));
        assert_eq!(decimal_div_exact(min, d("1")).unwrap(), min);
        assert_eq!(decimal_div_exact(min, d("-1")), Err(NumericError::Overflow));
        assert_eq!(
            decimal_add(Decimal::new(i128::MAX, 0), d("1")),
            Err(NumericError::Overflow)
        );
    }

    #[test]
    fn explicit_rounding_including_negative_ties() {
        for (numerator, mode, expected) in [
            ("1", "floor", "0.12"),
            ("1", "ceil", "0.13"),
            ("-1", "floor", "-0.13"),
            ("-1", "ceil", "-0.12"),
            ("-1", "trunc", "-0.12"),
            ("1", "half_even", "0.12"),
            ("3", "half_even", "0.38"),
            ("-3", "half_even", "-0.38"),
        ] {
            assert_eq!(
                decimal_div_round(d(numerator), d("8"), 2, mode).unwrap(),
                d(expected)
            );
        }
        assert_eq!(
            decimal_div_round(d("1"), d("3"), 0, "half_even").unwrap(),
            d("0")
        );
        assert_eq!(
            decimal_div_round(d("1"), d("3"), 2, "unknown"),
            Err(NumericError::InvalidRoundingMode)
        );
        assert_eq!(
            decimal_div_round(d("1"), d("3"), 19, "floor"),
            Err(NumericError::ScaleOutOfRange)
        );
    }

    proptest! {
        // Verify each rounding rule against rational inequalities, rather
        // than duplicating the implementation's cancellation/remainder code.
        #[test]
        fn rounded_division_satisfies_its_defining_bounds(
            a in -1_000_000i64..=1_000_000,
            b in -1_000_000i64..=1_000_000,
            a_scale in 0u8..=6,
            b_scale in 0u8..=6,
            output_scale in 0u8..=6,
        ) {
            prop_assume!(b != 0);
            let x = Decimal::new(a as i128, a_scale);
            let y = Decimal::new(b as i128, b_scale);
            let mut numerator = (a as i128) * 10i128.pow((b_scale + output_scale) as u32);
            let mut denominator = (b as i128) * 10i128.pow(a_scale as u32);
            if denominator < 0 { numerator = -numerator; denominator = -denominator; }
            for mode in ["floor", "ceil", "trunc", "half_even"] {
                let result = decimal_div_round(x, y, output_scale, mode).unwrap();
                let q = result.unscaled() * 10i128.pow((output_scale - result.scale()) as u32);
                let error = q * denominator - numerator;
                match mode {
                    "floor" => prop_assert!(-denominator < error && error <= 0),
                    "ceil" => prop_assert!(0 <= error && error < denominator),
                    "trunc" => {
                        prop_assert!(error.abs() < denominator);
                        let toward_zero = if numerator < 0 { error >= 0 } else { error <= 0 };
                        prop_assert!(toward_zero);
                    }
                    "half_even" => {
                        prop_assert!(2 * error.abs() <= denominator);
                        if 2 * error.abs() == denominator { prop_assert_eq!(q % 2, 0); }
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}
