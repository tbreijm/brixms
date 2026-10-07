//! Canonical settlement value and delta encoding. No evaluator state or selection logic.
use super::error::WorldError;
use super::types::{WorldKey, WorldTuple};
use brix_canon::{CanonReader, CanonWriter, Canonical, Digest};
use brix_syntax::ast;
use soc_core::calendar::Key;
use soc_core::store::TrieMap;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A scalar value admitted in relational operators and expressions.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Value {
    /// Internal absence sentinel; never an admissible expression value.
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    F64(brix_canon::FiniteF64),
    Decimal(brix_canon::Decimal),
}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Null => {}
            Self::Bool(v) => v.hash(state),
            Self::Int(v) => v.hash(state),
            Self::Str(v) => v.hash(state),
            Self::F64(v) => v.to_string().hash(state),
            Self::Decimal(v) => {
                v.unscaled().hash(state);
                v.scale().hash(state);
            }
        }
    }
}

impl Value {
    pub fn as_bool(&self) -> Result<bool, WorldError> {
        match self {
            Self::Bool(v) => Ok(*v),
            _ => Err(WorldError::NetworkError(
                "Unknown(EvaluationFault): guard must be Bool".into(),
            )),
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        if let Self::Int(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Self::Str(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_string().into_bytes()
    }
    /// Untyped byte payloads remain strings; schemas alone select numeric decoding.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::Str(String::from_utf8_lossy(bytes).into_owned())
    }
    pub fn from_str_val(s: &str) -> Self {
        Self::Str(s.to_owned())
    }
    pub fn from_typed_bytes(bytes: &[u8], ty: &ast::Ty) -> Result<Self, WorldError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| WorldError::NetworkError(format!("invalid scalar UTF-8: {e}")))?;
        let invalid = || WorldError::NetworkError(format!("invalid {ty:?} scalar {text:?}"));
        match ty {
            ast::Ty::Named(name) => match name.as_str() {
                "Str" => Ok(Self::Str(text.to_owned())),
                "Int" => text.parse().map(Self::Int).map_err(|_| invalid()),
                "Bool" => match text {
                    "true" => Ok(Self::Bool(true)),
                    "false" => Ok(Self::Bool(false)),
                    _ => Err(invalid()),
                },
                "F64" => text.parse().map(Self::F64).map_err(|_| invalid()),
                "Decimal" => brix_canon::decimal_parse(text)
                    .map(Self::Decimal)
                    .map_err(|_| invalid()),
                _ => Err(invalid()),
            },
            _ => Err(invalid()),
        }
    }
    pub fn to_scalar(&self) -> Result<brix_lower::l3_v2::L3ValueV2, WorldError> {
        use brix_lower::l3_v2::L3ValueV2 as V;
        Ok(match self {
            Self::Bool(v) => V::Bool(*v),
            Self::Int(v) => V::Int(*v),
            Self::Str(v) => V::Str(v.clone()),
            Self::F64(v) => V::F64(*v),
            Self::Decimal(v) => V::Decimal(*v),
            Self::Null => {
                return Err(WorldError::NetworkError(
                    "Unknown(EvaluationFault): absent scalar value".into(),
                ))
            }
        })
    }
    pub fn from_scalar(v: brix_lower::l3_v2::L3ValueV2) -> Result<Self, WorldError> {
        use brix_lower::l3_v2::L3ValueV2 as V;
        Ok(match v {
            V::Bool(v) => Self::Bool(v),
            V::Int(v) => Self::Int(v),
            V::Str(v) => Self::Str(v),
            V::F64(v) => Self::F64(v),
            V::Decimal(v) => Self::Decimal(v),
            _ => {
                return Err(WorldError::NetworkError(
                    "expression result is not a scalar".into(),
                ))
            }
        })
    }
}
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "null"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
            Self::F64(v) => write!(f, "{v}"),
            Self::Decimal(v) => write!(f, "{}", brix_canon::decimal_format(*v)),
        }
    }
}

/// A deterministic settled decision chosen by canonical settlement discipline.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SettledDecision {
    pub entity_id: String,
    pub candidate_name: String,
    pub priority: u64,
    pub phase: u64,
    pub value: Value,
    pub calendar_key: Key,
}

impl SettledDecision {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "entity_id": self.entity_id,
            "candidate_name": self.candidate_name,
            "priority": self.priority,
            "phase": self.phase,
            "value": self.value.to_string(),
            "calendar_key": format!("phase={},priority={},tiebreak={}", self.calendar_key.phase, self.calendar_key.priority, self.calendar_key.tiebreak.to_hex()),
        })
    }
}

/// Compute a deterministic canonical Blake3 digest across all settled decisions.
pub fn compute_decision_root(
    settlements: &BTreeMap<String, BTreeMap<String, SettledDecision>>,
) -> Digest {
    let mut tree = TrieMap::new();
    for (decide, entities) in settlements {
        for (entity, decision) in entities {
            tree = tree.insert(decision_key(decide, entity), decision_tuple(decision));
        }
    }
    tree.root_digest()
}

/// Content-addressed key for one `(decide, entity)` settlement in the decision trie.
pub fn decision_key(decide: &str, entity: &str) -> WorldKey {
    let mut w = CanonWriter::new();
    w.write_tag("brix.world.decision-key@1");
    w.write_str(decide);
    w.write_str(entity);
    WorldKey::new(w.finish())
}

/// Canonical encoding of a settled decision's value (everything but its key),
/// as stored in the decision trie. Decoded back by [`decode_settled_decision`].
pub fn decision_tuple(decision: &SettledDecision) -> WorldTuple {
    let mut w = CanonWriter::new();
    w.write_tag("brix.world.decision@1");
    w.write_ident(&decision.candidate_name);
    w.write_uint(decision.priority);
    w.write_uint(decision.phase);
    canon_write_value(&decision.value, &mut w);
    w.write_bytes(decision.calendar_key.tiebreak.as_bytes());
    WorldTuple::new(w.finish())
}

fn canon_decode_err(e: brix_canon::CanonError) -> WorldError {
    WorldError::NetworkError(format!("corrupt decision tuple: {e:?}"))
}

fn canon_read_value(r: &mut CanonReader<'_>) -> Result<Value, WorldError> {
    let tag = r.read_uint().map_err(canon_decode_err)?;
    Ok(match tag {
        0 => Value::Null,
        1 => Value::Int(r.read_int().map_err(canon_decode_err)?),
        2 => {
            let bytes = r.read_bytes().map_err(canon_decode_err)?;
            Value::Str(
                std::str::from_utf8(bytes)
                    .map_err(|e| WorldError::NetworkError(format!("invalid Str UTF-8: {e}")))?
                    .to_string(),
            )
        }
        3 => Value::Bool(match r.read_uint().map_err(canon_decode_err)? {
            0 => false,
            1 => true,
            _ => return Err(WorldError::NetworkError("invalid decision boolean".into())),
        }),
        4 => {
            let raw = r.read_raw(8).map_err(canon_decode_err)?;
            let mut bits = [0u8; 8];
            bits.copy_from_slice(raw);
            Value::F64(
                brix_canon::FiniteF64::from_bits(u64::from_be_bytes(bits)).map_err(|e| {
                    WorldError::NetworkError(format!("corrupt decision tuple: invalid F64: {e:?}"))
                })?,
            )
        }
        5 => Value::Decimal(brix_canon::read_decimal(r).map_err(canon_decode_err)?),
        other => {
            return Err(WorldError::NetworkError(format!(
                "corrupt decision tuple: unknown value tag {other}"
            )))
        }
    })
}

/// Decode a settlement previously encoded by [`decision_tuple`] — the
/// counterpart used to read a *historical* decision back from the node store
/// at a past `decision_root` (ADR-0046 P6 G2, `brix world explain --rev n`,
/// decided 2026-10-04). `entity_id` is supplied by the caller (it is the
/// lookup key, not part of the encoded tuple).
pub fn decode_settled_decision(
    entity_id: &str,
    bytes: &[u8],
) -> Result<SettledDecision, WorldError> {
    let mut r = CanonReader::new(bytes);
    if r.read_bytes().map_err(canon_decode_err)? != b"brix.world.decision@1" {
        return Err(WorldError::NetworkError(
            "invalid decision tuple tag".into(),
        ));
    }
    let candidate_name = std::str::from_utf8(r.read_bytes().map_err(canon_decode_err)?)
        .map_err(|e| WorldError::NetworkError(format!("invalid candidate_name UTF-8: {e}")))?
        .to_string();
    let priority = r.read_uint().map_err(canon_decode_err)?;
    let phase = r.read_uint().map_err(canon_decode_err)?;
    let value = canon_read_value(&mut r)?;
    let tiebreak_bytes = r.read_bytes().map_err(canon_decode_err)?;
    if tiebreak_bytes.len() != 32 {
        return Err(WorldError::NetworkError(
            "corrupt decision tuple: tiebreak must be 32 bytes".into(),
        ));
    }
    let mut tb = [0u8; 32];
    tb.copy_from_slice(tiebreak_bytes);
    if !r.is_empty() {
        return Err(WorldError::NetworkError(
            "corrupt decision tuple: trailing bytes".into(),
        ));
    }
    let decision = SettledDecision {
        entity_id: entity_id.to_string(),
        candidate_name,
        priority,
        phase,
        value,
        calendar_key: Key::new(phase, priority, Digest::from_bytes(tb)),
    };
    if decision_tuple(&decision).as_bytes() != bytes {
        return Err(WorldError::NetworkError(
            "noncanonical decision tuple".into(),
        ));
    }
    Ok(decision)
}

/// Canonical encoding of one revision's decision delta — every `(decide,
/// entity)` this batch added/updated or removed, strictly increasing by
/// `(decide, entity)` — whose digest is `brix.world.revision@2`'s
/// `decision_delta_digest` (ADR-0046 P6 G2/G3, decided 2026-10-04). The body
/// itself is persisted by `session.rs`; this function only defines its bytes.
pub fn encode_decision_delta(
    added: &BTreeMap<String, BTreeMap<String, SettledDecision>>,
    removed: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<u8> {
    let mut merged: BTreeMap<(String, String), Option<&SettledDecision>> = BTreeMap::new();
    for (decide, entities) in added {
        for (entity, decision) in entities {
            merged.insert((decide.clone(), entity.clone()), Some(decision));
        }
    }
    for (decide, entities) in removed {
        for entity in entities {
            merged
                .entry((decide.clone(), entity.clone()))
                .or_insert(None);
        }
    }
    let mut w = CanonWriter::new();
    w.write_tag("brix.world.decision-delta@1");
    w.write_uint(merged.len() as u64);
    for ((decide, entity), settlement) in &merged {
        w.write_str(decide);
        w.write_str(entity);
        match settlement {
            None => w.write_uint(0),
            Some(d) => {
                w.write_uint(1);
                let t = decision_tuple(d);
                w.write_bytes(t.as_bytes());
            }
        }
    }
    w.finish()
}

/// The state maintained inside an operator node.
pub(crate) fn canon_write_value(value: &Value, w: &mut CanonWriter) {
    match value {
        Value::Int(i) => {
            w.write_uint(1);
            w.write_int(*i);
        }
        Value::Str(s) => {
            w.write_uint(2);
            w.write_str(s);
        }
        Value::Bool(b) => {
            w.write_uint(3);
            w.write_uint(if *b { 1 } else { 0 });
        }
        Value::Null => {
            w.write_uint(0);
        }
        Value::F64(v) => {
            w.write_uint(4);
            v.canon_write(w);
        }
        Value::Decimal(v) => {
            w.write_uint(5);
            v.canon_write(w);
        }
    }
}

/// A complete per-revision change map, including settlement deletions.
pub type DecisionDelta = BTreeMap<(String, String), Option<SettledDecision>>;

/// Bounded strict decoding: check count before looping; frame lengths before copies.
pub fn decode_decision_delta(
    bytes: &[u8],
    max_entries: usize,
) -> Result<DecisionDelta, WorldError> {
    let err = || WorldError::NetworkError("invalid decision delta encoding".into());
    let mut r = CanonReader::new(bytes);
    if r.read_bytes().map_err(canon_decode_err)? != b"brix.world.decision-delta@1" {
        return Err(err());
    }
    let count = usize::try_from(r.read_uint().map_err(canon_decode_err)?).map_err(|_| err())?;
    if count > max_entries || count > bytes.len() / 3 {
        return Err(err());
    }
    let mut out = BTreeMap::new();
    for _ in 0..count {
        let decide = std::str::from_utf8(r.read_bytes().map_err(canon_decode_err)?)
            .map_err(|_| err())?
            .to_owned();
        let entity = std::str::from_utf8(r.read_bytes().map_err(canon_decode_err)?)
            .map_err(|_| err())?
            .to_owned();
        let key = (decide, entity);
        if out.last_key_value().is_some_and(|(prev, _)| prev >= &key) {
            return Err(err());
        }
        let value = match r.read_uint().map_err(canon_decode_err)? {
            0 => None,
            1 => Some(decode_settled_decision(
                &key.1,
                r.read_bytes().map_err(canon_decode_err)?,
            )?),
            _ => return Err(err()),
        };
        out.insert(key, value);
    }
    if !r.is_empty() {
        return Err(err());
    }
    Ok(out)
}
