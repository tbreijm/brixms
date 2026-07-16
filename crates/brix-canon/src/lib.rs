//! brix-canon — canonical encoding + identity.
//!
//! The serialization point of the whole toolchain: every hash, identity, log
//! entry, aggregation order, and cross-runtime vector flows through here, so it
//! freezes first and hardest (G0). After the golden vectors in `vectors/` are
//! frozen, that directory is append-only; any change is a spec erratum against
//! Appendix G **plus** a new [`CANON_VERSION`] tag.
//!
//! # Appendix G, implemented
//!
//! This crate implements the byte-level rules of `spec/BrixMS_v9_0.md`
//! Appendix G. The one non-obvious cross-cutting property is that **canonical
//! byte order equals value order** ("`Ord` = Appendix G byte order", spec
//! §"Numerics and ordering"). That forces the integer, decimal, and string
//! encodings to be *order-preserving*, which is stronger than the appendix's
//! sketch text spells out; the two encodings whose order-preserving layout is
//! not literally dictated by the sketch are ruled in `spec/errata/0001` and
//! `spec/errata/0002`. Every other rule is a direct reading of the appendix.
//!
//! Coverage of the Appendix G bullet list:
//! - integers — [`CanonWriter::write_uint`] / [`CanonWriter::write_int`]
//!   (order-preserving minimal-length, [`int_codec`]);
//! - `Decimal<P,S>` — [`Decimal`] (normalized, order-preserving);
//! - strings — [`CanonWriter::write_str`]; identifiers NFC-folded in
//!   [`CanonWriter::write_ident`];
//! - bytes — [`CanonWriter::write_bytes`];
//! - Bool/Unit/Char — [`CanonWriter::write_bool`] / [`CanonWriter::write_unit`]
//!   / [`CanonWriter::write_char`];
//! - enums — [`CanonWriter::write_enum`] (variant ordinal is ABI);
//! - records/rows — [`CanonWriter::write_record`] (fields sorted by field-name
//!   bytes);
//! - Set/Map/List/Bag — [`CanonWriter::write_set`] / [`CanonWriter::write_map`]
//!   / [`CanonWriter::write_list`] / [`CanonWriter::write_bag`];
//! - quantities — [`Quantity`] (base-unit value + measure id);
//! - money — [`Money`] (currency code + minor units);
//! - floats — excluded from key positions; [`total_order_key_f64`] provides the
//!   totalOrder tiebreak bytes used only for aggregation ordering.
//!
//! Normative reference: `spec/BrixMS_v9_0.md` Appendix G (canonical encoding).

mod decimal;
mod float;
mod int_codec;
mod quantity;

pub use decimal::{read_decimal, Decimal};
pub use float::{total_order_key_f32, total_order_key_f64};
pub use quantity::{Money, Quantity};

/// Canon version tag, embedded in every digest domain. A change to any encoding
/// rule requires bumping this AND minting a fresh `vectors/` set — the old
/// vectors stay valid for the old tag forever.
pub const CANON_VERSION: &str = "canon/1";

/// Domain-separation tags. Every [`Digest`] is `blake3(domain_bytes ++ payload)`
/// where `domain_bytes = CANON_VERSION ++ ":" ++ domain.tag()`, so a NodeId and
/// an EdgeId over identical payload bytes can never collide (Ring0 §1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Domain {
    /// `entity compatibility domain` for [`NodeId`].
    Node,
    /// `relation compatibility domain` for [`EdgeId`].
    Edge,
    /// `transaction intent / operation ordinal / source scope` for [`ClaimId`].
    Claim,
    /// Snapshot identity `(namespace, DataRevision, ProgramRevision)`.
    Snapshot,
    /// Generic value digest (canon-encoded values, revision-log entries).
    Value,
}

impl Domain {
    /// Stable byte tag for this domain. These strings are ABI — never reorder or
    /// rename without a `CANON_VERSION` bump.
    pub const fn tag(self) -> &'static str {
        match self {
            Domain::Node => "node",
            Domain::Edge => "edge",
            Domain::Claim => "claim",
            Domain::Snapshot => "snapshot",
            Domain::Value => "value",
        }
    }
}

/// A 256-bit canonical digest. Ordering is byte-lexicographic, which is the
/// canonical order used everywhere iteration order can be observed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    /// Compute `blake3(CANON_VERSION ++ ":" ++ domain.tag() ++ ":" ++ payload)`.
    pub fn of(domain: Domain, payload: &[u8]) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(CANON_VERSION.as_bytes());
        h.update(b":");
        h.update(domain.tag().as_bytes());
        h.update(b":");
        h.update(payload);
        Digest(*h.finalize().as_bytes())
    }

    /// Raw 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, the canonical human/text rendering used in diagnostics and
    /// `brix why` output.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in &self.0 {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
        }
        s
    }
}

impl core::fmt::Debug for Digest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Digest({})", self.to_hex())
    }
}

/// Typed identity wrappers (spec Part III §3). Distinct types over the same
/// digest representation so a NodeId cannot be passed where an EdgeId is wanted.
macro_rules! typed_id {
    ($(#[$m:meta])* $name:ident, $domain:expr) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub Digest);
        impl $name {
            /// Hash canon-encoded `payload` in this id's domain.
            pub fn from_canon(payload: &[u8]) -> Self {
                $name(Digest::of($domain, payload))
            }
            /// Underlying digest.
            pub fn digest(&self) -> Digest {
                self.0
            }
        }
    };
}

typed_id!(
    /// `NodeId = Hash(entity compatibility domain, canonical key encoding)`.
    NodeId, Domain::Node);
typed_id!(
    /// `EdgeId = Hash(relation compatibility domain, canonical role tuple)`.
    EdgeId, Domain::Edge);
typed_id!(
    /// `ClaimId = Hash(transaction intent, operation ordinal, source scope)`.
    ClaimId, Domain::Claim);
typed_id!(
    /// `SnapshotId = Hash(namespace, DataRevision, ProgramRevision)`.
    SnapshotId, Domain::Snapshot);

/// Canonical byte sink. Writers produce Appendix-G byte strings; the same bytes
/// are what gets hashed and what the append-only revision log stores (the log
/// needs no second serializer — Ring0 §1.1).
#[derive(Default, Clone)]
pub struct CanonWriter {
    buf: Vec<u8>,
}

impl CanonWriter {
    /// A fresh, empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume the writer, returning the canonical byte string.
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    /// Borrow the bytes written so far.
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Digest the bytes written so far in `domain`.
    pub fn digest(&self, domain: Domain) -> Digest {
        Digest::of(domain, &self.buf)
    }

    /// Append raw bytes with no framing. Internal helper for encodings that
    /// manage their own structure (decimals, nested fields).
    pub(crate) fn write_raw(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    /// Unsigned integer (`Nat`, `U8..U128`) in canonical minimal-length,
    /// **order-preserving** form (App. G "minimal-length ints", [`int_codec`]):
    /// each value has exactly one encoding and byte-lexicographic order equals
    /// numeric order.
    pub fn write_uint(&mut self, v: u64) {
        int_codec::encode_uint(v as u128, &mut self.buf);
    }

    /// 128-bit unsigned integer (`U128`).
    pub fn write_uint128(&mut self, v: u128) {
        int_codec::encode_uint(v, &mut self.buf);
    }

    /// Signed integer (`Int`, `I8..I128`) in canonical minimal-length,
    /// order-preserving form: negatives sort before positives, magnitude order
    /// preserved within each sign.
    pub fn write_int(&mut self, v: i64) {
        int_codec::encode_int(v as i128, &mut self.buf);
    }

    /// 128-bit signed integer (`I128`).
    pub fn write_int128(&mut self, v: i128) {
        int_codec::encode_int(v, &mut self.buf);
    }

    /// Length-prefixed raw byte string.
    pub fn write_bytes(&mut self, b: &[u8]) {
        self.write_uint(b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    /// A canonical identifier: NFC-normalized (App. G "strings: NFC for
    /// identifiers") then encoded as a length-prefixed UTF-8 byte string. NFC
    /// folding means identifiers that differ only in Unicode composition hash
    /// and sort identically.
    pub fn write_ident(&mut self, s: &str) {
        let nfc = nfc_cow(s);
        self.write_bytes(nfc.as_bytes());
    }

    /// A canonical UTF-8 string *value* — raw Unicode scalar sequence,
    /// length-prefixed, **no** NFC folding (App. G: "values as raw Unicode
    /// scalar sequences"). Values preserve their exact code points.
    pub fn write_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    /// A tag/discriminant byte string written length-prefixed. Enum *ordinals*
    /// are the ABI (App. G); prefer [`CanonWriter::write_enum`] where an ordinal
    /// exists. This form is for named tags whose identity is the name itself.
    pub fn write_tag(&mut self, tag: &str) {
        self.write_bytes(tag.as_bytes());
    }

    /// `Bool` as a single tagged byte (`0` / `1`), App. G "Bool/Unit/Char:
    /// single tagged bytes".
    pub fn write_bool(&mut self, b: bool) {
        self.buf.push(b as u8);
    }

    /// `Unit` as a single fixed byte.
    pub fn write_unit(&mut self) {
        self.buf.push(0);
    }

    /// `Char` as its Unicode scalar value, order-preservingly (so char order
    /// equals scalar order).
    pub fn write_char(&mut self, c: char) {
        self.write_uint(c as u64);
    }

    /// An enum value: variant `ordinal` (declaration order is ABI — reordering
    /// is a compatibility-domain change, App. G) followed by `payload`'s
    /// canonical bytes.
    pub fn write_enum(&mut self, ordinal: u64, payload: impl FnOnce(&mut CanonWriter)) {
        self.write_uint(ordinal);
        payload(self);
    }

    /// A record/row: fields **sorted by canonical field-name bytes**, count-
    /// prefixed, each entry `name (ident) ++ value bytes` (App. G "records/rows:
    /// fields sorted by canonical field-name bytes, each name-prefixed"). The
    /// caller supplies each field's already-canonicalized value bytes.
    pub fn write_record<I, K>(&mut self, fields: I)
    where
        I: IntoIterator<Item = (K, Vec<u8>)>,
        K: AsRef<str>,
    {
        // Field names are identifiers → fold to NFC, then sort by those bytes.
        let mut entries: Vec<(Vec<u8>, Vec<u8>)> = fields
            .into_iter()
            .map(|(name, value)| (nfc_cow(name.as_ref()).into_owned().into_bytes(), value))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        self.write_uint(entries.len() as u64);
        for (name_bytes, value) in entries {
            self.write_bytes(&name_bytes);
            self.write_raw(&value);
        }
    }

    /// A `List`/`Vector`: elements in **sequence order** (App. G), count-
    /// prefixed. `elements` are already-canonicalized value byte strings.
    pub fn write_list<I>(&mut self, elements: I)
    where
        I: IntoIterator<Item = Vec<u8>>,
    {
        let items: Vec<Vec<u8>> = elements.into_iter().collect();
        self.write_uint(items.len() as u64);
        for e in items {
            self.write_bytes(&e);
        }
    }

    /// A `Set`: entries **sorted by canonical element bytes**, deduplicated,
    /// count-prefixed (App. G).
    pub fn write_set<I>(&mut self, elements: I)
    where
        I: IntoIterator<Item = Vec<u8>>,
    {
        let mut items: Vec<Vec<u8>> = elements.into_iter().collect();
        items.sort();
        items.dedup();
        self.write_uint(items.len() as u64);
        for e in items {
            self.write_bytes(&e);
        }
    }

    /// A `Map`: entries **sorted by canonical key bytes**, count-prefixed, each
    /// entry `key ++ value` (App. G). On duplicate keys the last value wins,
    /// matching map-construction semantics.
    pub fn write_map<I>(&mut self, entries: I)
    where
        I: IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    {
        let mut items: Vec<(Vec<u8>, Vec<u8>)> = entries.into_iter().collect();
        // Stable sort keeps the last duplicate reachable; dedup keeping the last.
        items.sort_by(|a, b| a.0.cmp(&b.0));
        // Keep the *last* value for a duplicate key: reverse, dedup-by-key, reverse.
        items.reverse();
        items.dedup_by(|a, b| a.0 == b.0);
        items.reverse();
        self.write_uint(items.len() as u64);
        for (k, v) in items {
            self.write_bytes(&k);
            self.write_bytes(&v);
        }
    }

    /// A `Bag`/multiset: **sorted (element, multiplicity) pairs** (App. G),
    /// count-prefixed. `elements` may contain repeats; multiplicities are
    /// accumulated.
    pub fn write_bag<I>(&mut self, elements: I)
    where
        I: IntoIterator<Item = Vec<u8>>,
    {
        use std::collections::BTreeMap;
        let mut counts: BTreeMap<Vec<u8>, u64> = BTreeMap::new();
        for e in elements {
            *counts.entry(e).or_insert(0) += 1;
        }
        self.write_uint(counts.len() as u64);
        for (e, m) in counts {
            self.write_bytes(&e);
            self.write_uint(m);
        }
    }
}

/// NFC-fold `s`, borrowing unchanged when already NFC (the common case).
fn nfc_cow(s: &str) -> std::borrow::Cow<'_, str> {
    use unicode_normalization::UnicodeNormalization;
    if s.is_ascii() {
        // ASCII is always already in NFC.
        std::borrow::Cow::Borrowed(s)
    } else {
        std::borrow::Cow::Owned(s.nfc().collect())
    }
}

/// Types with a canonical byte encoding. Implementing this is the *only* way a
/// type participates in identity, hashing, logging, or aggregation order.
///
/// Note: floating-point types deliberately do **not** implement `Canonical`,
/// because App. G excludes floats from key positions. Use
/// [`total_order_key_f64`] to obtain a totalOrder tiebreak key for aggregation
/// ordering only.
pub trait Canonical {
    /// Append this value's canonical bytes to `w`.
    fn canon_write(&self, w: &mut CanonWriter);

    /// Canonical byte string for this value alone.
    fn canon_bytes(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        self.canon_write(&mut w);
        w.finish()
    }

    /// Digest of this value in `domain`.
    fn canon_digest(&self, domain: Domain) -> Digest {
        Digest::of(domain, &self.canon_bytes())
    }
}

impl Canonical for u64 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(*self);
    }
}
impl Canonical for u128 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint128(*self);
    }
}
impl Canonical for i64 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_int(*self);
    }
}
impl Canonical for i128 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_int128(*self);
    }
}
impl Canonical for bool {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bool(*self);
    }
}
impl Canonical for char {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_char(*self);
    }
}
impl Canonical for str {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_str(self);
    }
}
impl Canonical for String {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_str(self);
    }
}
impl<T: Canonical> Canonical for Vec<T> {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_list(self.iter().map(|e| e.canon_bytes()));
    }
}
impl<T: Canonical> Canonical for Option<T> {
    /// `Option` is the two-variant enum `None` (ordinal 0) / `Some` (ordinal 1),
    /// so it round-trips through the enum ABI like any other.
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            None => w.write_uint(0),
            Some(v) => {
                w.write_uint(1);
                v.canon_write(w);
            }
        }
    }
}
impl<T: Canonical> Canonical for std::collections::BTreeSet<T> {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_set(self.iter().map(|e| e.canon_bytes()));
    }
}
impl<K: Canonical, V: Canonical> Canonical for std::collections::BTreeMap<K, V> {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_map(self.iter().map(|(k, v)| (k.canon_bytes(), v.canon_bytes())));
    }
}

/// Canonical reader over a byte string produced by [`CanonWriter`]. Mirrors the
/// writer's forms so the revision log round-trips through one serializer.
pub struct CanonReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

/// Errors from canonical decoding. Non-minimal encodings are rejected so the
/// "exactly one encoding per value" law is enforced on read as well as write.
#[derive(Debug, PartialEq, Eq)]
pub enum CanonError {
    /// Ran out of bytes mid-value.
    UnexpectedEof,
    /// A magnitude carried a non-minimal leading zero byte, so the value had
    /// more than one encoding.
    NonMinimalInt,
    /// A length prefix pointed past the end of the buffer, or a value fell
    /// outside its type's representable range.
    BadLength,
}

impl<'a> CanonReader<'a> {
    /// Wrap a byte string.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Whether all bytes have been consumed.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }

    /// Read a single raw byte. Internal helper for self-framing encodings.
    pub(crate) fn read_u8(&mut self) -> Result<u8, CanonError> {
        let b = *self.buf.get(self.pos).ok_or(CanonError::UnexpectedEof)?;
        self.pos += 1;
        Ok(b)
    }

    /// Read a minimal-length, order-preserving unsigned integer.
    pub fn read_uint(&mut self) -> Result<u64, CanonError> {
        let (v, n) = int_codec::decode_uint(&self.buf[self.pos..])?;
        self.pos += n;
        u64::try_from(v).map_err(|_| CanonError::BadLength)
    }

    /// Read a 128-bit unsigned integer.
    pub fn read_uint128(&mut self) -> Result<u128, CanonError> {
        let (v, n) = int_codec::decode_uint(&self.buf[self.pos..])?;
        self.pos += n;
        Ok(v)
    }

    /// Read a minimal-length, order-preserving signed integer.
    pub fn read_int(&mut self) -> Result<i64, CanonError> {
        let (v, n) = int_codec::decode_int(&self.buf[self.pos..])?;
        self.pos += n;
        i64::try_from(v).map_err(|_| CanonError::BadLength)
    }

    /// Read a 128-bit signed integer.
    pub fn read_int128(&mut self) -> Result<i128, CanonError> {
        let (v, n) = int_codec::decode_int(&self.buf[self.pos..])?;
        self.pos += n;
        Ok(v)
    }

    /// Read a `bool` written by [`CanonWriter::write_bool`].
    pub fn read_bool(&mut self) -> Result<bool, CanonError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(CanonError::BadLength),
        }
    }

    /// Read a length-prefixed byte string.
    pub fn read_bytes(&mut self) -> Result<&'a [u8], CanonError> {
        let len = self.read_uint()? as usize;
        let end = self.pos.checked_add(len).ok_or(CanonError::BadLength)?;
        let slice = self.buf.get(self.pos..end).ok_or(CanonError::BadLength)?;
        self.pos = end;
        Ok(slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn domains_separate_identical_payloads() {
        let n = NodeId::from_canon(b"x");
        let e = EdgeId::from_canon(b"x");
        assert_ne!(n.digest(), e.digest(), "domain tags must separate ids");
    }

    #[test]
    fn version_tag_is_stable() {
        assert_eq!(CANON_VERSION, "canon/1");
    }

    #[test]
    fn nfc_identifiers_fold() {
        // U+00E9 (é, precomposed) vs U+0065 U+0301 (e + combining acute).
        let precomposed = "café";
        let decomposed = "cafe\u{0301}";
        assert_ne!(precomposed, decomposed, "inputs differ as raw code points");
        let mut a = CanonWriter::new();
        a.write_ident(precomposed);
        let mut b = CanonWriter::new();
        b.write_ident(decomposed);
        assert_eq!(a.finish(), b.finish(), "NFC must fold them together");
    }

    #[test]
    fn string_values_do_not_fold() {
        let mut a = CanonWriter::new();
        a.write_str("café");
        let mut b = CanonWriter::new();
        b.write_str("cafe\u{0301}");
        assert_ne!(a.finish(), b.finish(), "values keep raw code points");
    }

    #[test]
    fn record_field_order_is_canonical() {
        let mut a = CanonWriter::new();
        a.write_record(vec![
            ("beta", 1u64.canon_bytes()),
            ("alpha", 2u64.canon_bytes()),
        ]);
        let mut b = CanonWriter::new();
        b.write_record(vec![
            ("alpha", 2u64.canon_bytes()),
            ("beta", 1u64.canon_bytes()),
        ]);
        assert_eq!(
            a.finish(),
            b.finish(),
            "field declaration order must not matter"
        );
    }

    #[test]
    fn set_order_independent() {
        let mut a = CanonWriter::new();
        a.write_set([3u64, 1, 2, 1].iter().map(|v| v.canon_bytes()));
        let mut b = CanonWriter::new();
        b.write_set([1u64, 2, 3].iter().map(|v| v.canon_bytes()));
        assert_eq!(
            a.finish(),
            b.finish(),
            "set insertion order + dups must not matter"
        );
    }

    proptest! {
        #[test]
        fn uint_roundtrip(v: u64) {
            let mut w = CanonWriter::new();
            w.write_uint(v);
            let bytes = w.finish();
            let mut r = CanonReader::new(&bytes);
            prop_assert_eq!(r.read_uint().unwrap(), v);
            prop_assert!(r.is_empty());
        }

        #[test]
        fn int_roundtrip(v: i64) {
            let mut w = CanonWriter::new();
            w.write_int(v);
            let bytes = w.finish();
            let mut r = CanonReader::new(&bytes);
            prop_assert_eq!(r.read_int().unwrap(), v);
        }

        #[test]
        fn uint_order_preserving(a: u64, b: u64) {
            let ea = { let mut w = CanonWriter::new(); w.write_uint(a); w.finish() };
            let eb = { let mut w = CanonWriter::new(); w.write_uint(b); w.finish() };
            prop_assert_eq!(a.cmp(&b), ea.cmp(&eb));
        }

        #[test]
        fn int_order_preserving(a: i64, b: i64) {
            let ea = { let mut w = CanonWriter::new(); w.write_int(a); w.finish() };
            let eb = { let mut w = CanonWriter::new(); w.write_int(b); w.finish() };
            prop_assert_eq!(a.cmp(&b), ea.cmp(&eb));
        }

        #[test]
        fn uint_encoding_is_unique(a: u64, b: u64) {
            let ea = { let mut w = CanonWriter::new(); w.write_uint(a); w.finish() };
            let eb = { let mut w = CanonWriter::new(); w.write_uint(b); w.finish() };
            prop_assert_eq!(a == b, ea == eb);
        }

        #[test]
        fn bytes_roundtrip(data: Vec<u8>) {
            let mut w = CanonWriter::new();
            w.write_bytes(&data);
            let bytes = w.finish();
            let mut r = CanonReader::new(&bytes);
            prop_assert_eq!(r.read_bytes().unwrap(), &data[..]);
        }

        #[test]
        fn string_value_equal_iff_bytes_equal(a: String, b: String) {
            let ea = { let mut w = CanonWriter::new(); w.write_str(&a); w.finish() };
            let eb = { let mut w = CanonWriter::new(); w.write_str(&b); w.finish() };
            prop_assert_eq!(a == b, ea == eb);
        }
    }
}
