//! brix-canon — canonical encoding + identity.
//!
//! The serialization point of the whole toolchain: every hash, identity, log
//! entry, aggregation order, and cross-runtime vector flows through here, so it
//! freezes first and hardest (G0). After the golden vectors in `vectors/` are
//! frozen, that directory is append-only; any change is a spec erratum against
//! Appendix G **plus** a new [`CANON_VERSION`] tag.
//!
//! # Status: pre-G0 API surface
//!
//! This is the stable *public API* every downstream lane compiles against —
//! the trait, the writer/reader, [`Digest`], and the typed id wrappers. The
//! byte-exact Appendix G rules are being implemented and cross-checked against
//! an independent implementation before freeze; sites still owing full App. G
//! fidelity are marked `APP-G:` in the source. Downstream crates should depend
//! only on the items re-exported here and file toolchain bugs (never reinvent a
//! second encoder — see spec/Ring0_Build_Plan.md §1.7).
//!
//! Normative reference: `spec/BrixMS_v9_0.md` Appendix G (canonical encoding).

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

    /// Unsigned integer in canonical minimal-length base-128 varint form
    /// (App. G "minimal-length ints"): no trailing zero-continuation groups, so
    /// each value has exactly one encoding.
    pub fn write_uint(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                break;
            }
            self.buf.push(byte | 0x80);
        }
    }

    /// Signed integer via zigzag over [`CanonWriter::write_uint`], so
    /// small-magnitude values of either sign stay short and the encoding is
    /// unique.
    pub fn write_int(&mut self, v: i64) {
        self.write_uint(((v << 1) ^ (v >> 63)) as u64);
    }

    /// Length-prefixed raw byte string.
    pub fn write_bytes(&mut self, b: &[u8]) {
        self.write_uint(b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    /// A canonical identifier. APP-G: identifiers MUST be NFC-normalized before
    /// encoding; that normalization is not yet applied here (it needs a unicode
    /// dependency whitelisted in DEPS.md). Until then this encodes raw UTF-8 and
    /// callers must pass already-NFC text. Tracked as a canon-lane freeze
    /// blocker.
    pub fn write_ident(&mut self, s: &str) {
        // APP-G TODO: s = nfc(s) once unicode-normalization is justified in DEPS.md.
        self.write_bytes(s.as_bytes());
    }

    /// A canonical UTF-8 string value (not an identifier — no NFC folding).
    pub fn write_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    /// A tag/discriminant byte string (field names, enum names) written
    /// length-prefixed. Enum *ordinals* are the ABI (App. G); prefer
    /// [`CanonWriter::write_uint`] on the ordinal where one exists.
    pub fn write_tag(&mut self, tag: &str) {
        self.write_bytes(tag.as_bytes());
    }
}

/// Types with a canonical byte encoding. Implementing this is the *only* way a
/// type participates in identity, hashing, logging, or aggregation order.
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
impl Canonical for i64 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_int(*self);
    }
}
impl Canonical for bool {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(*self as u64);
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
    /// A varint used more continuation bytes than its value requires.
    NonMinimalInt,
    /// A length prefix pointed past the end of the buffer.
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

    /// Read a minimal-length unsigned varint, rejecting non-minimal encodings.
    pub fn read_uint(&mut self) -> Result<u64, CanonError> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = *self.buf.get(self.pos).ok_or(CanonError::UnexpectedEof)?;
            self.pos += 1;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                // Minimality: the final byte of a multi-byte value must carry bits.
                if byte == 0 && shift != 0 {
                    return Err(CanonError::NonMinimalInt);
                }
                return Ok(result);
            }
            shift += 7;
            if shift >= 64 {
                return Err(CanonError::NonMinimalInt);
            }
        }
    }

    /// Read a zigzag signed varint.
    pub fn read_int(&mut self) -> Result<i64, CanonError> {
        let u = self.read_uint()?;
        Ok(((u >> 1) as i64) ^ -((u & 1) as i64))
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
        fn uint_encoding_is_unique(a: u64, b: u64) {
            // Ordering-law scaffolding: equal values encode identically,
            // distinct values encode distinctly.
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
    }
}
