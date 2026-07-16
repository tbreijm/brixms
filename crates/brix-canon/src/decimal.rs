//! `Decimal<P,S>` canonical encoding (Appendix G).
//!
//! Appendix G's literal text is "scale byte + unscaled integer encoding;
//! normalized (no trailing zeros beyond declared scale)". A plain
//! `[scale_byte] ++ unscaled_int` layout is not order-preserving across
//! different scales (e.g. `2` at scale 0 vs `1.5` at scale 1 would compare by
//! scale byte first, putting `2` before `1.5`), which breaks the "`Ord` =
//! Appendix G byte order" requirement the numerics section relies on for
//! canonical aggregation order. This module implements the concrete ruling
//! proposed in `spec/errata/0002-decimal-canonical-encoding.md`: a
//! sign/exponent/digit-string layout, analogous to a normalized
//! scientific-notation encoding, that *is* order-preserving. See that erratum
//! for the derivation and proof sketch.
//!
//! Layout: `[sign] ++ magnitude`, where `sign` is one of `NEG`/`ZERO`/`POS`
//! (so all negatives sort before zero sorts before all positives), and for
//! nonzero values `magnitude = exponent_bytes ++ digit_bytes ++ [terminator]`
//! with:
//! - `exponent_bytes`: the order-preserving signed encoding
//!   ([`crate::int_codec`]) of the base-10 exponent of the most significant
//!   digit (`ndigits - 1 - scale`);
//! - `digit_bytes`: each decimal digit `d` (0..=9) as byte `1 + d`, most
//!   significant first;
//! - `terminator`: byte `0x00`, strictly less than any digit byte, which
//!   makes the whole magnitude prefix-free (a shorter digit string can never
//!   be a byte-prefix of an extension of itself) so that comparing two
//!   magnitudes always has a definite first differing byte.
//!
//! For **negative** values the whole magnitude is bitwise-complemented before
//! being written, which correctly reverses "larger magnitude sorts later"
//! into "larger magnitude (more negative) sorts first" *because* the
//! magnitude encoding is prefix-free — bitwise complement only safely reverses
//! order for byte strings that never stand in a prefix relationship with each
//! other (see the erratum).

use crate::{CanonError, CanonWriter, Canonical};
use std::cmp::Ordering;

const SIGN_NEG: u8 = 0x00;
const SIGN_ZERO: u8 = 0x01;
const SIGN_POS: u8 = 0x02;

/// An exact, arbitrary-scale decimal value: `unscaled * 10^-scale`.
///
/// Values are normalized on construction: trailing zero digits are folded
/// into a smaller `scale` (down to a floor of `0`), so that two constructions
/// denoting the same number always compare `==` and canon-encode identically
/// ("normalized" per Appendix G). This mirrors `Decimal<P,S>`'s value space
/// (Part V §2); the static precision/scale type parameters themselves are a
/// brix-ir type-checking concern, not a canon-encoding one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decimal {
    unscaled: i128,
    scale: u8,
}

impl Decimal {
    /// Construct a normalized decimal from an unscaled integer and a scale.
    pub fn new(unscaled: i128, scale: u8) -> Self {
        let mut u = unscaled;
        let mut s = scale;
        while s > 0 && u != 0 && u % 10 == 0 {
            u /= 10;
            s -= 1;
        }
        if u == 0 {
            s = 0;
        }
        Decimal {
            unscaled: u,
            scale: s,
        }
    }

    /// The canonical zero.
    pub const ZERO: Decimal = Decimal {
        unscaled: 0,
        scale: 0,
    };

    /// The normalized unscaled integer.
    pub fn unscaled(&self) -> i128 {
        self.unscaled
    }

    /// The normalized scale.
    pub fn scale(&self) -> u8 {
        self.scale
    }

    fn digits_and_exponent(&self) -> (Vec<u8>, i64) {
        debug_assert_ne!(self.unscaled, 0);
        let mag = self.unscaled.unsigned_abs();
        let mut digits = Vec::new();
        let mut m = mag;
        while m > 0 {
            digits.push((m % 10) as u8);
            m /= 10;
        }
        digits.reverse();
        let ndigits = digits.len() as i64;
        let exponent = (ndigits - 1) - self.scale as i64;
        (digits, exponent)
    }

    fn magnitude_bytes(&self) -> Vec<u8> {
        let (digits, exponent) = self.digits_and_exponent();
        let mut mbuf = CanonWriter::new();
        mbuf.write_int128(exponent as i128);
        let mut tail = Vec::with_capacity(digits.len() + 1);
        for d in &digits {
            tail.push(1 + d);
        }
        tail.push(0x00); // terminator, < any digit byte
        mbuf.write_raw(&tail);
        mbuf.finish()
    }
}

impl Canonical for Decimal {
    fn canon_write(&self, w: &mut CanonWriter) {
        if self.unscaled == 0 {
            w.write_raw(&[SIGN_ZERO]);
            return;
        }
        let negative = self.unscaled < 0;
        let inner = self.magnitude_bytes();
        if negative {
            w.write_raw(&[SIGN_NEG]);
            let complemented: Vec<u8> = inner.iter().map(|b| !b).collect();
            w.write_raw(&complemented);
        } else {
            w.write_raw(&[SIGN_POS]);
            w.write_raw(&inner);
        }
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `Ord` is defined as canonical byte order, by construction (Appendix G:
/// "`Ord` = Appendix G byte order").
impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        self.canon_bytes().cmp(&other.canon_bytes())
    }
}

/// Decode a [`Decimal`] previously written by [`Decimal::canon_write`].
///
/// The negative branch was written as a bitwise complement of the whole
/// magnitude, so decoding complements each raw byte back before interpreting
/// it — reconstructing the original (positive-style) exponent/digit encoding
/// on the fly, one byte at a time, without needing to know its length up front.
pub fn read_decimal(r: &mut crate::CanonReader<'_>) -> Result<Decimal, CanonError> {
    let sign = r.read_u8()?;
    match sign {
        SIGN_ZERO => Ok(Decimal::ZERO),
        SIGN_NEG | SIGN_POS => {
            let negate = sign == SIGN_NEG;
            let xf = |b: u8| if negate { !b } else { b };

            // Reassemble the (un-complemented) int_codec exponent bytes, which
            // are self-delimiting, and decode them with the tested codec.
            let cat = xf(r.read_u8()?);
            let ez = crate::int_codec::INT_ZERO_CATEGORY;
            let ilen = if cat == ez {
                0
            } else if cat > ez {
                (cat - ez) as usize
            } else {
                (ez - cat) as usize
            };
            if ilen > 16 {
                return Err(CanonError::BadLength);
            }
            let mut ibuf = Vec::with_capacity(1 + ilen);
            ibuf.push(cat);
            for _ in 0..ilen {
                ibuf.push(xf(r.read_u8()?));
            }
            let (exp_i128, _) = crate::int_codec::decode_int(&ibuf)?;
            let exponent = exp_i128 as i64;

            let mut digits = Vec::new();
            loop {
                let b = xf(r.read_u8()?);
                if b == 0x00 {
                    break;
                }
                if !(1..=10).contains(&b) {
                    return Err(CanonError::BadLength);
                }
                digits.push(b - 1);
            }
            if digits.is_empty() {
                return Err(CanonError::BadLength);
            }
            let ndigits = digits.len() as i64;
            let scale = ndigits - 1 - exponent;
            if scale < 0 {
                return Err(CanonError::BadLength);
            }
            let mut unscaled: i128 = 0;
            for d in &digits {
                unscaled = unscaled * 10 + *d as i128;
            }
            if negate {
                unscaled = -unscaled;
            }
            Ok(Decimal::new(unscaled, scale as u8))
        }
        _ => Err(CanonError::BadLength),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn normalizes_trailing_zeros() {
        assert_eq!(Decimal::new(150, 1), Decimal::new(15, 0));
        assert_eq!(Decimal::new(1_500_000, 5), Decimal::new(15, 0));
        assert_eq!(Decimal::new(0, 7), Decimal::ZERO);
    }

    #[test]
    fn does_not_strip_significant_zeros() {
        // 150 at scale 0 is really the integer 150, not 15.
        assert_eq!(Decimal::new(150, 0).unscaled(), 150);
        assert_eq!(Decimal::new(150, 0).scale(), 0);
    }

    #[test]
    fn order_matches_value_examples() {
        let a = Decimal::new(15, 0); // 15
        let b = Decimal::new(153, 1); // 15.3
        let c = Decimal::new(2, 0); // 2
        let d = Decimal::new(-2, 0); // -2
        assert!(c < a); // 2 < 15
        assert!(a < b); // 15 < 15.3
        assert!(d < c); // -2 < 2
        assert!(Decimal::new(-153, 1) < Decimal::new(-15, 0)); // -15.3 < -15
    }

    #[test]
    fn roundtrip_examples() {
        for (u, s) in [
            (0i128, 0u8),
            (15, 0),
            (153, 1),
            (-153, 1),
            (-2, 0),
            (999999999999i128, 3),
        ] {
            let d = Decimal::new(u, s);
            let bytes = d.canon_bytes();
            let mut r = crate::CanonReader::new(&bytes);
            let back = read_decimal(&mut r).unwrap();
            assert_eq!(d, back, "roundtrip failed for ({u}, {s})");
            assert!(r.is_empty());
        }
    }

    fn ref_cmp(a: &Decimal, b: &Decimal) -> Ordering {
        // Cross-multiply to compare unscaled_a / 10^scale_a vs unscaled_b / 10^scale_b
        // without floats, using i128 (bounded test inputs keep this in range).
        let (ua, sa) = (a.unscaled(), a.scale() as u32);
        let (ub, sb) = (b.unscaled(), b.scale() as u32);
        let (lhs, rhs) = if sa >= sb {
            (ua, ub * 10i128.pow(sa - sb))
        } else {
            (ua * 10i128.pow(sb - sa), ub)
        };
        lhs.cmp(&rhs)
    }

    proptest! {
        #[test]
        fn ord_matches_numeric_value(
            ua in -1_000_000_000i128..1_000_000_000i128,
            sa in 0u8..10,
            ub in -1_000_000_000i128..1_000_000_000i128,
            sb in 0u8..10,
        ) {
            let a = Decimal::new(ua, sa);
            let b = Decimal::new(ub, sb);
            prop_assert_eq!(a.cmp(&b), ref_cmp(&a, &b));
            // canon byte order must equal Ord, by the spec's own mandate.
            prop_assert_eq!(a.cmp(&b), a.canon_bytes().cmp(&b.canon_bytes()));
        }

        #[test]
        fn roundtrip(u in -1_000_000_000_000i128..1_000_000_000_000i128, s in 0u8..12) {
            let d = Decimal::new(u, s);
            let bytes = d.canon_bytes();
            let mut r = crate::CanonReader::new(&bytes);
            let back = read_decimal(&mut r).unwrap();
            prop_assert_eq!(d, back);
        }
    }
}
