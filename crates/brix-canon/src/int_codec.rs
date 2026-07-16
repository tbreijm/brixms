//! Order-preserving minimal-length integer encoding.
//!
//! Appendix G says: "integers: minimal-length big-endian two's complement with
//! sign byte; Nat unsigned." Read completely literally that text underspecifies
//! a byte layout: a plain length-free big-endian two's-complement encoding does
//! **not** make raw byte-lexicographic order equal numeric order across
//! different magnitudes (compare the 1-byte encoding of `2` against the 2-byte
//! encoding of `256`: `0x02` sorts after `0x01,0x00` even though `2 < 256`).
//! Appendix G's own numerics section requires `Ord` to equal canonical byte
//! order (spec `BrixMS_v9_0.md` line ~4722), and Part V §8 leans on canonical
//! row order for aggregation — so the encoding must be order-preserving.
//!
//! This module implements the concrete ruling proposed in
//! `spec/errata/0001-integer-canonical-encoding.md`: a length/category byte
//! precedes the minimal big-endian magnitude, so that byte-lexicographic order
//! equals numeric order. See that erratum for the full derivation.
//!
//! **Unsigned (`Nat`/uint):** `[len] ++ magnitude_be`, `len` = number of
//! magnitude bytes (`0` for the value `0`), `magnitude_be` minimal (no leading
//! zero byte). Larger `len` always denotes a larger value, so the length byte
//! dominates comparison correctly; equal-length magnitudes compare correctly
//! as plain big-endian bytes.
//!
//! **Signed (`Int`):** a single category byte `0x80 + sign*len` (`0x80` alone
//! for zero), followed by the magnitude bytes — plain for positive, bitwise
//! complemented for negative (so a larger negative magnitude, which is a
//! *smaller* value, sorts first). This makes the whole encoding prefix-free
//! (self-delimiting), a property [`crate::decimal`] reuses for its exponent
//! field.

use crate::CanonError;

/// The byte for a signed value of zero; also the fixed point of the
/// sign/length category scheme (negative categories below it, positive above).
pub(crate) const INT_ZERO_CATEGORY: u8 = 0x80;

/// Maximum magnitude byte length we support (128-bit values: up to 16 bytes).
const MAX_MAG_LEN: usize = 16;

/// Encode an unsigned magnitude as `[len] ++ magnitude_be`, appending to `out`.
pub(crate) fn encode_uint(v: u128, out: &mut Vec<u8>) {
    if v == 0 {
        out.push(0);
        return;
    }
    let be = v.to_be_bytes();
    let first = be.iter().position(|&b| b != 0).expect("v != 0");
    let mag = &be[first..];
    out.push(mag.len() as u8);
    out.extend_from_slice(mag);
}

/// Decode an unsigned magnitude. Returns `(value, bytes_consumed)`.
pub(crate) fn decode_uint(buf: &[u8]) -> Result<(u128, usize), CanonError> {
    let len = *buf.first().ok_or(CanonError::UnexpectedEof)? as usize;
    if len == 0 {
        return Ok((0, 1));
    }
    if len > MAX_MAG_LEN {
        return Err(CanonError::BadLength);
    }
    let mag = buf.get(1..1 + len).ok_or(CanonError::UnexpectedEof)?;
    if mag[0] == 0 {
        // A leading zero byte means the length wasn't minimal.
        return Err(CanonError::NonMinimalInt);
    }
    let mut v: u128 = 0;
    for &b in mag {
        v = (v << 8) | b as u128;
    }
    Ok((v, 1 + len))
}

/// Encode a signed value with the sign/length category scheme, appending to `out`.
pub(crate) fn encode_int(v: i128, out: &mut Vec<u8>) {
    if v == 0 {
        out.push(INT_ZERO_CATEGORY);
        return;
    }
    let negative = v < 0;
    let mag: u128 = v.unsigned_abs();
    let be = mag.to_be_bytes();
    let first = be.iter().position(|&b| b != 0).expect("mag != 0");
    let mag_bytes = &be[first..];
    let len = mag_bytes.len() as u8;
    if negative {
        out.push(INT_ZERO_CATEGORY - len);
        out.extend(mag_bytes.iter().map(|b| !b));
    } else {
        out.push(INT_ZERO_CATEGORY + len);
        out.extend_from_slice(mag_bytes);
    }
}

/// Decode a signed value. Returns `(value, bytes_consumed)`.
pub(crate) fn decode_int(buf: &[u8]) -> Result<(i128, usize), CanonError> {
    let cat = *buf.first().ok_or(CanonError::UnexpectedEof)?;
    if cat == INT_ZERO_CATEGORY {
        return Ok((0, 1));
    }
    if cat > INT_ZERO_CATEGORY {
        let len = (cat - INT_ZERO_CATEGORY) as usize;
        if len == 0 || len > MAX_MAG_LEN {
            return Err(CanonError::BadLength);
        }
        let mag = buf.get(1..1 + len).ok_or(CanonError::UnexpectedEof)?;
        if mag[0] == 0 {
            return Err(CanonError::NonMinimalInt);
        }
        let mut m: u128 = 0;
        for &b in mag {
            m = (m << 8) | b as u128;
        }
        let v = i128::try_from(m).map_err(|_| CanonError::BadLength)?;
        Ok((v, 1 + len))
    } else {
        let len = (INT_ZERO_CATEGORY - cat) as usize;
        if len == 0 || len > MAX_MAG_LEN {
            return Err(CanonError::BadLength);
        }
        let comp = buf.get(1..1 + len).ok_or(CanonError::UnexpectedEof)?;
        if comp[0] == 0xFF {
            // Complemented leading byte 0xFF means the true magnitude's
            // leading byte was 0 - non-minimal.
            return Err(CanonError::NonMinimalInt);
        }
        let mut mag: u128 = 0;
        for &b in comp {
            mag = (mag << 8) | (!b) as u128;
        }
        // mag is in 1..=2^127, which covers i128::MIN's magnitude (2^127).
        let v = if mag == 1u128 << 127 {
            i128::MIN
        } else {
            -(i128::try_from(mag).map_err(|_| CanonError::BadLength)?)
        };
        Ok((v, 1 + len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn enc_u(v: u128) -> Vec<u8> {
        let mut o = Vec::new();
        encode_uint(v, &mut o);
        o
    }
    fn enc_i(v: i128) -> Vec<u8> {
        let mut o = Vec::new();
        encode_int(v, &mut o);
        o
    }

    #[test]
    fn zero_is_single_byte() {
        assert_eq!(enc_u(0), vec![0]);
        assert_eq!(enc_i(0), vec![INT_ZERO_CATEGORY]);
    }

    #[test]
    fn int_min_roundtrips() {
        let bytes = enc_i(i128::MIN);
        let (v, n) = decode_int(&bytes).unwrap();
        assert_eq!(v, i128::MIN);
        assert_eq!(n, bytes.len());
    }

    proptest! {
        #[test]
        fn uint_roundtrip(v: u64) {
            let bytes = enc_u(v as u128);
            let (got, n) = decode_uint(&bytes).unwrap();
            prop_assert_eq!(got, v as u128);
            prop_assert_eq!(n, bytes.len());
        }

        #[test]
        fn int_roundtrip(v: i64) {
            let bytes = enc_i(v as i128);
            let (got, n) = decode_int(&bytes).unwrap();
            prop_assert_eq!(got, v as i128);
            prop_assert_eq!(n, bytes.len());
        }

        #[test]
        fn uint_order_preserving(a: u64, b: u64) {
            let ea = enc_u(a as u128);
            let eb = enc_u(b as u128);
            prop_assert_eq!(a.cmp(&b), ea.cmp(&eb));
        }

        #[test]
        fn int_order_preserving(a: i64, b: i64) {
            let ea = enc_i(a as i128);
            let eb = enc_i(b as i128);
            prop_assert_eq!(a.cmp(&b), ea.cmp(&eb));
        }

        #[test]
        fn uint_encoding_unique(a: u64, b: u64) {
            prop_assert_eq!(a == b, enc_u(a as u128) == enc_u(b as u128));
        }
    }
}
