//! Injective canonical storage codecs for persisted network state (R0 Phase 2, M0).
//!
//! Version tag: [`STATE_CODEC_VERSION`] (`brix.world.state-value@2`).
//!
//! Unlike `Value::to_bytes` (Display-based, so `Int(1)` and `Str("1")` collide), every
//! encoding here is typed and injective: distinct values encode to distinct bytes, and the
//! decoders accept exactly the canonical encoding (minimal integers, strictly sorted field
//! names, no trailing bytes, normalized decimals, no negative-zero / non-finite floats).
//!
//! `DerivationId::digest()` is intentionally untouched: this is a *storage* codec only.

#![deny(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

use brix_canon::{
    CanonDecode, CanonError, CanonReader, CanonWriter, Canonical, Decimal, Digest, FiniteF64,
};
use brix_lower::relation_dag::OperatorId;
use soc_core::calendar::Key;

use super::codec::TupleRecord;
use super::compact_deriv::CompactDeriv;
use super::network::{compute_candidate_calendar_key, CandidateEntry, IntermediateTuple, Value};
use super::types::WorldKey;

/// Codec version tag for persisted state values. `@2`: derivations are persisted as
/// `brix.world.deriv@2` compact derivations (see `compact_deriv`); the validity key of a
/// persisted network state must include this tag.
pub const STATE_CODEC_VERSION: &str = "brix.world.state-value@2";

/// Failure decoding a persisted state value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateCodecError {
    /// Underlying canonical reader failure (EOF, non-minimal integer, bad length, bad UTF-8).
    Canon(CanonError),
    /// Bytes remained after a complete value.
    TrailingBytes,
    /// Unknown variant tag.
    UnknownTag(u64),
    /// Well-formed but not the canonical encoding of its value.
    NonCanonical(String),
}

impl fmt::Display for StateCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canon(e) => write!(f, "state codec: canonical read error: {e:?}"),
            Self::TrailingBytes => write!(f, "state codec: trailing bytes"),
            Self::UnknownTag(t) => write!(f, "state codec: unknown tag {t}"),
            Self::NonCanonical(m) => write!(f, "state codec: non-canonical encoding: {m}"),
        }
    }
}

impl std::error::Error for StateCodecError {}

impl From<CanonError> for StateCodecError {
    fn from(e: CanonError) -> Self {
        Self::Canon(e)
    }
}

type Res<T> = Result<T, StateCodecError>;

pub(super) fn noncanon<T>(msg: &str) -> Res<T> {
    Err(StateCodecError::NonCanonical(msg.to_string()))
}

pub(super) fn read_string(r: &mut CanonReader<'_>) -> Res<String> {
    let b = r.read_bytes()?;
    std::str::from_utf8(b)
        .map(str::to_owned)
        .map_err(|_| StateCodecError::Canon(CanonError::BadLength))
}

pub(super) fn read_digest(r: &mut CanonReader<'_>) -> Res<Digest> {
    let b = r.read_bytes()?;
    let arr: [u8; 32] = b
        .try_into()
        .map_err(|_| StateCodecError::Canon(CanonError::BadLength))?;
    Ok(Digest::from_bytes(arr))
}

fn finish<T>(r: &CanonReader<'_>, v: T) -> Res<T> {
    if r.is_empty() {
        Ok(v)
    } else {
        Err(StateCodecError::TrailingBytes)
    }
}

/// A type with an injective canonical storage encoding.
pub trait StateCodec: Sized {
    fn state_write(&self, w: &mut CanonWriter);
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self>;

    fn state_bytes(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        self.state_write(&mut w);
        w.finish()
    }

    /// Decode a complete value; trailing bytes are an error.
    fn state_from_bytes(bytes: &[u8]) -> Res<Self> {
        let mut r = CanonReader::new(bytes);
        let v = Self::state_read(&mut r)?;
        finish(&r, v)
    }
}

/// Adapter making any [`StateCodec`] type usable as a `TrieMap` key or value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct St<T>(pub T);

impl<T: StateCodec> Canonical for St<T> {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.0.state_write(w);
    }
}

impl<T: StateCodec> CanonDecode for St<T> {
    fn canon_read(r: &mut CanonReader<'_>) -> Result<Self, CanonError> {
        // Trie node decoding only surfaces `CanonError`; the typed cause is collapsed to
        // `BadLength` there and reported as `CorruptedNode` by the store.
        T::state_read(r).map(St).map_err(|e| match e {
            StateCodecError::Canon(c) => c,
            _ => CanonError::BadLength,
        })
    }
}

// ---------------------------------------------------------------------------
// Value
// ---------------------------------------------------------------------------

const V_NULL: u64 = 0;
const V_BOOL: u64 = 1;
const V_INT: u64 = 2;
const V_STR: u64 = 3;
const V_F64: u64 = 4;
const V_DECIMAL: u64 = 5;

impl StateCodec for Value {
    fn state_write(&self, w: &mut CanonWriter) {
        match self {
            Value::Null => w.write_uint(V_NULL),
            Value::Bool(b) => {
                w.write_uint(V_BOOL);
                w.write_bool(*b);
            }
            Value::Int(i) => {
                w.write_uint(V_INT);
                w.write_int(*i);
            }
            Value::Str(s) => {
                w.write_uint(V_STR);
                w.write_str(s);
            }
            Value::F64(f) => {
                w.write_uint(V_F64);
                w.write_uint(f.bits());
            }
            Value::Decimal(d) => {
                w.write_uint(V_DECIMAL);
                w.write_int128(d.unscaled());
                w.write_uint(u64::from(d.scale()));
            }
        }
    }

    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        match r.read_uint()? {
            V_NULL => Ok(Value::Null),
            V_BOOL => Ok(Value::Bool(r.read_bool()?)),
            V_INT => Ok(Value::Int(r.read_int()?)),
            V_STR => Ok(Value::Str(read_string(r)?)),
            V_F64 => {
                let bits = r.read_uint()?;
                let f = FiniteF64::from_bits(bits)
                    .map_err(|_| StateCodecError::NonCanonical("non-finite f64".into()))?;
                if f.bits() != bits {
                    return noncanon("f64 bits not normalized (negative zero)");
                }
                Ok(Value::F64(f))
            }
            V_DECIMAL => {
                let unscaled = r.read_int128()?;
                let scale = u8::try_from(r.read_uint()?)
                    .map_err(|_| StateCodecError::Canon(CanonError::BadLength))?;
                let d = Decimal::new(unscaled, scale);
                if d.unscaled() != unscaled || d.scale() != scale {
                    return noncanon("decimal not normalized");
                }
                Ok(Value::Decimal(d))
            }
            t => Err(StateCodecError::UnknownTag(t)),
        }
    }
}

/// An ordered list of values (group keys, join keys).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ValueList(pub Vec<Value>);

impl StateCodec for ValueList {
    fn state_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.0.len() as u64);
        for v in &self.0 {
            v.state_write(w);
        }
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        let n = r.read_uint()?;
        let mut out = Vec::new();
        for _ in 0..n {
            out.push(Value::state_read(r)?);
        }
        Ok(ValueList(out))
    }
}

// ---------------------------------------------------------------------------
// Primitive adapters
// ---------------------------------------------------------------------------

impl StateCodec for () {
    fn state_write(&self, _w: &mut CanonWriter) {}
    fn state_read(_r: &mut CanonReader<'_>) -> Res<Self> {
        Ok(())
    }
}

impl StateCodec for u64 {
    fn state_write(&self, w: &mut CanonWriter) {
        w.write_uint(*self);
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        Ok(r.read_uint()?)
    }
}

impl StateCodec for String {
    fn state_write(&self, w: &mut CanonWriter) {
        w.write_str(self);
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        read_string(r)
    }
}

impl StateCodec for WorldKey {
    fn state_write(&self, w: &mut CanonWriter) {
        w.write_bytes(&self.0);
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        Ok(WorldKey(r.read_bytes()?.to_vec()))
    }
}

impl StateCodec for Digest {
    fn state_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.as_bytes());
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        read_digest(r)
    }
}

// ---------------------------------------------------------------------------
// Operator ids / JoinEntry
// ---------------------------------------------------------------------------

pub(super) fn read_op(r: &mut CanonReader<'_>) -> Res<OperatorId> {
    let n = usize::try_from(r.read_uint()?)
        .map_err(|_| StateCodecError::Canon(CanonError::BadLength))?;
    Ok(OperatorId(n))
}

/// Join-index entry: the matched tuple plus its compact derivation (keyed in the index by
/// the derivation's digest).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinEntry {
    pub tuple: IntermediateTuple,
    pub deriv: CompactDeriv,
}

impl StateCodec for JoinEntry {
    fn state_write(&self, w: &mut CanonWriter) {
        self.tuple.state_write(w);
        self.deriv.state_write(w);
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        let tuple = IntermediateTuple::state_read(r)?;
        let deriv = CompactDeriv::state_read(r)?;
        Ok(JoinEntry { tuple, deriv })
    }
}

// ---------------------------------------------------------------------------
// IntermediateTuple / TupleRecord
// ---------------------------------------------------------------------------

impl StateCodec for IntermediateTuple {
    fn state_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.fields.len() as u64);
        for (k, v) in &self.fields {
            w.write_str(k);
            v.state_write(w);
        }
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        let n = r.read_uint()?;
        let mut fields = BTreeMap::new();
        let mut prev: Option<String> = None;
        for _ in 0..n {
            let name = read_string(r)?;
            if prev.as_ref().is_some_and(|p| *p >= name) {
                return noncanon("tuple fields not strictly sorted (unsorted or duplicate)");
            }
            let v = Value::state_read(r)?;
            prev = Some(name.clone());
            fields.insert(name, v);
        }
        Ok(IntermediateTuple { fields })
    }
}

impl StateCodec for TupleRecord {
    fn state_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.fields.len() as u64);
        for (k, v) in &self.fields {
            w.write_str(k);
            w.write_bytes(v);
        }
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        let n = r.read_uint()?;
        let mut fields = BTreeMap::new();
        let mut prev: Option<String> = None;
        for _ in 0..n {
            let name = read_string(r)?;
            if prev.as_ref().is_some_and(|p| *p >= name) {
                return noncanon("record fields not strictly sorted (unsorted or duplicate)");
            }
            let v = r.read_bytes()?.to_vec();
            prev = Some(name.clone());
            fields.insert(name, v);
        }
        Ok(TupleRecord { fields })
    }
}

// ---------------------------------------------------------------------------
// SubRoot
// ---------------------------------------------------------------------------

/// Reference to a nested persisted collection: its trie root digest and entry count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubRoot {
    pub root: Digest,
    pub len: u64,
}

impl SubRoot {
    /// The empty collection.
    pub fn empty() -> Self {
        Self {
            root: Digest::of(brix_canon::Domain::Value, b"brix:trie:empty:v1"),
            len: 0,
        }
    }
}

impl StateCodec for SubRoot {
    fn state_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.root.as_bytes());
        w.write_uint(self.len);
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        let root = read_digest(r)?;
        let len = r.read_uint()?;
        Ok(SubRoot { root, len })
    }
}

/// One support stored in a support index: identified by a digest; the inner set stores
/// `id -> Val` (`Val` is `()` when the id alone is enough).
pub trait SupportItem: StateCodec + Clone + PartialEq + fmt::Debug + 'static + Send + Sync {
    type Val: StateCodec + Clone + 'static + Send + Sync;
    fn id(&self) -> Digest;
    fn val(&self) -> Self::Val;
    fn from_parts(id: Digest, v: Self::Val) -> Self;
}

impl SupportItem for Digest {
    type Val = ();
    fn id(&self) -> Digest {
        *self
    }
    fn val(&self) {}
    fn from_parts(id: Digest, _: ()) -> Self {
        id
    }
}

impl SupportItem for CompactDeriv {
    type Val = CompactDeriv;
    fn id(&self) -> Digest {
        self.digest()
    }
    fn val(&self) -> CompactDeriv {
        self.clone()
    }
    fn from_parts(_: Digest, v: CompactDeriv) -> Self {
        v
    }
}

/// Outer value of a support index: a single support is stored inline, two or more as a
/// nested set. The representation is canonical: `Inline` iff exactly one support, `Sub`
/// (with `len >= 2`) otherwise, so a given support set always has one encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupportRef<I: SupportItem> {
    Inline(I),
    Sub(SubRoot),
}

impl<I: SupportItem> StateCodec for SupportRef<I> {
    fn state_write(&self, w: &mut CanonWriter) {
        match self {
            SupportRef::Inline(d) => {
                w.write_uint(0);
                d.state_write(w);
            }
            SupportRef::Sub(s) => {
                w.write_uint(1);
                s.state_write(w);
            }
        }
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        match r.read_uint()? {
            0 => Ok(SupportRef::Inline(I::state_read(r)?)),
            1 => {
                let s = SubRoot::state_read(r)?;
                if s.len < 2 {
                    return noncanon("support sub-set with fewer than two members");
                }
                Ok(SupportRef::Sub(s))
            }
            _ => noncanon("unknown support ref tag"),
        }
    }
}

// ---------------------------------------------------------------------------
// CandidateEntry (stored form)
// ---------------------------------------------------------------------------

/// Stored form of a [`CandidateEntry`]: identical fields, but `supports` is a [`SubRoot`]
/// (a persisted `SSet<DerivationId>`) instead of an in-memory set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateRecord {
    pub candidate_name: String,
    pub entity_id: String,
    pub priority: u64,
    pub phase: u64,
    pub value: Value,
    pub calendar_key: Key,
    pub supports: SubRoot,
}

impl CandidateRecord {
    pub fn from_entry(entry: &CandidateEntry, supports: SubRoot) -> Self {
        Self {
            candidate_name: entry.candidate_name.clone(),
            entity_id: entry.entity_id.clone(),
            priority: entry.priority,
            phase: entry.phase,
            value: entry.value.clone(),
            calendar_key: entry.calendar_key,
            supports,
        }
    }

    /// Check the stored calendar key against the recomputed one.
    pub fn verify_calendar_key(&self, decide_name: &str) -> Res<()> {
        let expect = compute_candidate_calendar_key(
            self.phase,
            self.priority,
            decide_name,
            &self.entity_id,
            &self.candidate_name,
            &self.value,
        );
        if expect != self.calendar_key {
            return noncanon("candidate calendar_key does not match recomputed key");
        }
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        self.state_bytes()
    }

    /// Decode a complete record and recompute/check its calendar key for `decide_name`.
    pub fn decode_checked(bytes: &[u8], decide_name: &str) -> Res<Self> {
        let rec = Self::state_from_bytes(bytes)?;
        rec.verify_calendar_key(decide_name)?;
        Ok(rec)
    }
}

/// `StateCodec` decoding is structural only (the decide name is not available); callers
/// must use [`CandidateRecord::decode_checked`] or [`CandidateRecord::verify_calendar_key`].
impl StateCodec for CandidateRecord {
    fn state_write(&self, w: &mut CanonWriter) {
        w.write_str(&self.candidate_name);
        w.write_str(&self.entity_id);
        w.write_uint(self.priority);
        w.write_uint(self.phase);
        self.value.state_write(w);
        w.write_uint(self.calendar_key.phase);
        w.write_uint(self.calendar_key.priority);
        w.write_bytes(self.calendar_key.tiebreak.as_bytes());
        self.supports.state_write(w);
    }
    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        let candidate_name = read_string(r)?;
        let entity_id = read_string(r)?;
        let priority = r.read_uint()?;
        let phase = r.read_uint()?;
        let value = Value::state_read(r)?;
        let kphase = r.read_uint()?;
        let kprio = r.read_uint()?;
        let tiebreak = read_digest(r)?;
        let supports = SubRoot::state_read(r)?;
        Ok(Self {
            candidate_name,
            entity_id,
            priority,
            phase,
            value,
            calendar_key: Key::new(kphase, kprio, tiebreak),
            supports,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rt<T: StateCodec + PartialEq + fmt::Debug>(v: &T) {
        let b = v.state_bytes();
        let back = T::state_from_bytes(&b).expect("decode");
        assert_eq!(&back, v);
        assert_eq!(back.state_bytes(), b, "re-encode must be identical");
        let mut extra = b.clone();
        extra.push(0);
        assert_eq!(
            T::state_from_bytes(&extra).err(),
            Some(StateCodecError::TrailingBytes)
        );
    }

    fn arb_value() -> impl Strategy<Value = super::Value> {
        prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::Int),
            ".{0,12}".prop_map(Value::Str),
            any::<f64>().prop_filter_map("finite", |f| FiniteF64::from_bits(f.to_bits())
                .ok()
                .map(Value::F64)),
            (any::<i128>(), 0u8..40).prop_map(|(u, s)| Value::Decimal(Decimal::new(u, s))),
        ]
    }

    proptest! {
        #[test]
        fn value_roundtrip(v in arb_value()) { rt(&v); }

        #[test]
        fn value_encoding_injective(a in arb_value(), b in arb_value()) {
            prop_assert_eq!(a == b, a.state_bytes() == b.state_bytes());
        }

        #[test]
        fn tuple_roundtrip(fields in proptest::collection::btree_map("[a-z.]{1,6}", arb_value(), 0..5)) {
            rt(&IntermediateTuple { fields });
        }

        #[test]
        fn record_roundtrip(fields in proptest::collection::btree_map("[a-z]{1,6}", proptest::collection::vec(any::<u8>(), 0..8), 0..5)) {
            rt(&TupleRecord { fields });
        }
    }

    #[test]
    fn int_and_str_one_are_distinct() {
        assert_ne!(
            Value::Int(1).state_bytes(),
            Value::Str("1".into()).state_bytes()
        );
        assert_eq!(Value::Int(1).to_bytes(), Value::Str("1".into()).to_bytes());
    }

    #[test]
    fn negative_zero_and_nonfinite_rejected() {
        let mut w = CanonWriter::new();
        w.write_uint(V_F64);
        w.write_uint((-0.0f64).to_bits());
        assert!(matches!(
            Value::state_from_bytes(&w.finish()),
            Err(StateCodecError::NonCanonical(_))
        ));
        let mut w = CanonWriter::new();
        w.write_uint(V_F64);
        w.write_uint(f64::NAN.to_bits());
        assert!(Value::state_from_bytes(&w.finish()).is_err());
    }

    #[test]
    fn unnormalized_decimal_rejected() {
        let mut w = CanonWriter::new();
        w.write_uint(V_DECIMAL);
        w.write_int128(10);
        w.write_uint(1); // 1.0 -> normalizes to 1, scale 0
        assert!(matches!(
            Value::state_from_bytes(&w.finish()),
            Err(StateCodecError::NonCanonical(_))
        ));
        rt(&Value::Decimal(Decimal::new(12345, 3)));
    }

    #[test]
    fn unsorted_or_duplicate_fields_rejected() {
        for names in [["b", "a"], ["a", "a"]] {
            let mut w = CanonWriter::new();
            w.write_uint(2);
            for n in names {
                w.write_str(n);
                Value::Int(1).state_write(&mut w);
            }
            assert!(matches!(
                IntermediateTuple::state_from_bytes(&w.finish()),
                Err(StateCodecError::NonCanonical(_))
            ));
            let mut w = CanonWriter::new();
            w.write_uint(2);
            for n in names {
                w.write_str(n);
                w.write_bytes(b"x");
            }
            assert!(matches!(
                TupleRecord::state_from_bytes(&w.finish()),
                Err(StateCodecError::NonCanonical(_))
            ));
        }
    }

    #[test]
    fn non_minimal_and_unknown_tag_rejected() {
        let mut w = CanonWriter::new();
        w.write_uint(99);
        assert_eq!(
            Value::state_from_bytes(&w.finish()),
            Err(StateCodecError::UnknownTag(99))
        );
        assert!(Value::state_from_bytes(&[]).is_err());
        assert!(Value::state_from_bytes(&[0xff]).is_err());
    }

    #[test]
    fn candidate_record_roundtrip_and_key_check() {
        let value = Value::Int(5);
        let key = compute_candidate_calendar_key(2, 1, "dec", "e1", "c1", &value);
        let rec = CandidateRecord {
            candidate_name: "c1".into(),
            entity_id: "e1".into(),
            priority: 1,
            phase: 2,
            value,
            calendar_key: key,
            supports: SubRoot::empty(),
        };
        rt(&rec);
        let b = rec.encode();
        assert_eq!(CandidateRecord::decode_checked(&b, "dec").unwrap(), rec);
        assert!(CandidateRecord::decode_checked(&b, "other").is_err());
        let mut bad = rec.clone();
        bad.calendar_key = Key::new(2, 1, Digest::from_bytes([9; 32]));
        assert!(CandidateRecord::decode_checked(&bad.encode(), "dec").is_err());
    }
}
