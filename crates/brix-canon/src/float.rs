//! Float exclusion + totalOrder tiebreak bytes (Appendix G).
//!
//! Appendix G is explicit: "floats: NOT encodable in `canon/1` key positions;
//! canonical row order for aggregation uses the non-float remainder of the row,
//! with full-row totalOrder tiebreak defined over canonicalized bit patterns
//! for the float components." Part V §8 adds: "NaNs canonicalize to one bit
//! pattern" and `totalOrder` is the deterministic sort order.
//!
//! So floats never implement [`crate::Canonical`] (that trait is what admits a
//! type into key/identity positions). Instead this module produces an 8- or
//! 4-byte **totalOrder key**: an order-preserving unsigned image of the IEEE-754
//! bit pattern, after NaN canonicalization, used **only** as the last-resort
//! tiebreak when ordering aggregation rows whose non-float remainder ties.
//!
//! The key is the standard IEEE-754 totalOrder transform:
//! - canonicalize any NaN to a single quiet-NaN bit pattern;
//! - if the sign bit is set (negative, including -0.0 and -inf), flip **all**
//!   bits; otherwise flip **only** the sign bit.
//!
//! This yields a `uN` whose unsigned order equals IEEE totalOrder: -inf < …
//! < -0 < +0 < … < +inf < NaN, deterministically and reproducibly.

/// Canonical quiet-NaN bit pattern for f64 (sign 0, all exponent bits 1,
/// top mantissa bit 1, rest 0). NaN canonicalizes here before ordering.
const CANON_NAN_F64: u64 = 0x7ff8_0000_0000_0000;
/// Canonical quiet-NaN bit pattern for f32.
const CANON_NAN_F32: u32 = 0x7fc0_0000;

/// TotalOrder tiebreak key for an `f64`. **Not** a key/identity encoding —
/// aggregation ordering only (App. G float rule).
pub fn total_order_key_f64(x: f64) -> [u8; 8] {
    let bits = if x.is_nan() {
        CANON_NAN_F64
    } else {
        x.to_bits()
    };
    let ordered = if bits >> 63 == 1 {
        !bits
    } else {
        bits ^ (1u64 << 63)
    };
    ordered.to_be_bytes()
}

/// TotalOrder tiebreak key for an `f32`. See [`total_order_key_f64`].
pub fn total_order_key_f32(x: f32) -> [u8; 4] {
    let bits = if x.is_nan() {
        CANON_NAN_F32
    } else {
        x.to_bits()
    };
    let ordered = if bits >> 31 == 1 {
        !bits
    } else {
        bits ^ (1u32 << 31)
    };
    ordered.to_be_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn known_ordering() {
        let order = [
            f64::NEG_INFINITY,
            -1.5,
            -0.0,
            0.0,
            1.5,
            f64::INFINITY,
            f64::NAN,
        ];
        for w in order.windows(2) {
            assert!(
                total_order_key_f64(w[0]) <= total_order_key_f64(w[1]),
                "totalOrder violated at {:?} vs {:?}",
                w[0],
                w[1]
            );
        }
        // -0.0 strictly precedes +0.0 under totalOrder.
        assert!(total_order_key_f64(-0.0) < total_order_key_f64(0.0));
    }

    #[test]
    fn all_nans_canonicalize() {
        let a = f64::from_bits(0x7ff8_0000_0000_0001);
        let b = f64::from_bits(0xffff_0000_0000_0000);
        assert_eq!(total_order_key_f64(a), total_order_key_f64(b));
        assert_eq!(total_order_key_f64(a), total_order_key_f64(f64::NAN));
    }

    proptest! {
        #[test]
        fn matches_ieee_total_order(a: f64, b: f64) {
            // Compare against IEEE totalOrder via total_cmp (NaN-inclusive),
            // canonicalizing NaNs first so all NaNs are equal here.
            let ca = if a.is_nan() { f64::from_bits(CANON_NAN_F64) } else { a };
            let cb = if b.is_nan() { f64::from_bits(CANON_NAN_F64) } else { b };
            let key_ord = total_order_key_f64(a).cmp(&total_order_key_f64(b));
            prop_assert_eq!(key_ord, ca.total_cmp(&cb));
        }

        #[test]
        fn f32_matches_ieee_total_order(a: f32, b: f32) {
            let ca = if a.is_nan() { f32::from_bits(CANON_NAN_F32) } else { a };
            let cb = if b.is_nan() { f32::from_bits(CANON_NAN_F32) } else { b };
            let key_ord = total_order_key_f32(a).cmp(&total_order_key_f32(b));
            prop_assert_eq!(key_ord, ca.total_cmp(&cb));
        }
    }
}
