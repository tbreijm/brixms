//! Independent naive reference evaluator for the persistent world runtime (ADR-0046, stage P6a).
//!
//! `crates/brix-kb/src/world/network.rs`'s [`crate::world::network::WorldNetwork`] maintains
//! derived relations, candidate frontiers, and settled decisions *incrementally*: $O(\Delta)$
//! propagation, support-count truth maintenance, a persistent operator network. Its own
//! `recompute_from_scratch` "oracle" replays base facts through a **fresh instance of the
//! same engine** — so it only proves the incremental and from-scratch code paths of *one*
//! implementation agree with each other, never that either is correct. That is not
//! independent evidence (see `docs/planning/persistent-world-runtime-plan.md` §6).
//!
//! This module is a second, independently written implementation of the same declarative
//! semantics — `Scan`/`Bind`/`Filter`/`Project`/`EquiJoin`/`Distinct`/`GroupedCount`, and the
//! per-entity decide/propose/settle discipline — computed by plain set semantics from a full
//! base-relation snapshot, with **no incremental state, no derivation/support tracking, no
//! persistent maps, and no `soc_core::calendar::Frontier`**. It is deliberately the slowest,
//! most obviously-correct thing that could work: nested-loop joins, a brute-force
//! group-by, and a full sort to pick the least calendar key. Performance does not matter;
//! only independence and transparent correctness do.
//!
//! ## Independence: what is and is not shared with `network.rs`
//!
//! This module **does not import anything from `crate::world::network`** — not
//! `OperatorState`, not `PMap`/`PSet`, not `DerivationId`, not `Value`, not
//! `IntermediateTuple`, not `compute_candidate_calendar_key`. Every one of those has its own
//! from-scratch counterpart defined below ([`Value`], [`Row`], [`candidate_tiebreak_digest`],
//! `extract_entity_id`, `from_program`, …), written by reading the *behavior* of `network.rs`
//! and re-deriving it, not by calling its code. Two code paths that happen to agree is the
//! whole point.
//!
//! What genuinely **is** shared, and why each is safe to share:
//!
//! 1. **`brix_lower::relation_dag::{RelationDag, OperatorNode, FieldRef, GroupProjection,
//!    lower_relations, resolve_decide_source_relation}`** — the lowered operator DAG and its
//!    node/AST types, plus the pure rule for which relation a `decide` block's `list` names.
//!    This is the *specification* of which operators exist and how they are wired, not an
//!    evaluation strategy. Both engines are handed the same DAG and are free to evaluate it
//!    however they like; sharing the DAG's shape (and the decide/relation resolution rule) is
//!    sharing the problem statement, not the solution.
//! 2. **`brix_syntax::ast`** (`Expr`, `Ty`, `ProposeDecl`, `Callable`, …) and
//!    **`brix_lower::module_graph::LinkedProgram`** — the surface AST and linked-program
//!    container. Same rationale: these are the program *text*, not an engine.
//! 3. **The tuple codec** (`crate::world::codec::TupleRecord`, `crate::world::types::{WorldKey,
//!    WorldTuple}`) — a deterministic, versioned *serialization format*, not relational logic.
//!    A bug here would corrupt both oracles identically, but the codec's correctness is
//!    covered separately by `tests/world_tuple_codec.rs`, and this module's job is to catch
//!    *relational/decision* bugs, not byte-codec bugs. The strict tagged input
//!    decoder and structured-field framing marker are also shared transport
//!    definitions; value conversion and canonical writing remain independent.
//! 4. **The scalar expression evaluator** (`brix_lower::world_expr::CompiledWorldExpr`, via
//!    `brix_lower::l3_v2`) — this lives in `brix-lower`, is already independent of
//!    `WorldNetwork`'s incremental state (it is a pure function of an expression and a
//!    binding environment), and `network.rs` itself calls through exactly this same entry
//!    point. Re-deriving a *third* expression evaluator from scratch would mean re-litigating
//!    arithmetic, comparison, and short-circuit semantics that are out of scope for the
//!    relational/decision bug class this oracle targets, and buys little: a scalar-evaluator
//!    bug would equally corrupt whichever relation or guard touches it in *either* engine,
//!    and is independently covered by `tests/world_scalar_contract.rs` and the `brix-lower`
//!    `l3_v2` test suite.
//! 5. **`soc_core::calendar::Key`** (the plain `(phase, priority, tiebreak)` data type, with
//!    its derived `Ord`) — reused as a *value type* only. Its selection algorithm,
//!    `Frontier::select_least`, is explicitly **not** used; see `settle_entity` below, which
//!    instead does a full linear scan and `Iterator::min_by_key` over `Key`'s derived `Ord`.
//! 6. **The calendar tie-break digest formula** (`candidate_tiebreak_digest` below) —
//!    `network.rs`'s `compute_candidate_calendar_key` is a pure function (tag + decide name +
//!    entity id + candidate name + value, Blake3-digested via `CanonWriter`) with no access
//!    to network state. Its *formula* is a spec-level contract (ADR-0046): two independently
//!    deterministic tie-break conventions would make every genuine priority tie disagree for
//!    no interesting reason. So this module **re-implements the identical byte-for-byte
//!    formula from scratch** (not a call into `network.rs`) rather than inventing an
//!    arbitrary different one. This buys something real: if `network.rs`'s formula ever
//!    silently drifts (wrong field, wrong order, wrong tag), the two independently-written
//!    copies stop agreeing on tie-break digests and the differential test in
//!    `tests/world_reference_differential.rs` catches the drift.
//!
//! ## Entity identity is an explicit, declared field (ADR-0046, decided 2026-10-04)
//!
//! Earlier than this, `network.rs::extract_entity_id` inferred which field of a decide
//! block's bound row named the entity being decided about via an **unspecified heuristic**
//! fallback chain, and this module carried an independently-written copy of that same
//! heuristic to stay behaviorally comparable. Tony's 2026-10-04 decision retired both: a
//! world-profile `decide` block must now write `per <field>` explicitly (enforced at lowering
//! by `brix_lower::relation_dag::lower_relations`), and [`extract_entity_id`] below reads that
//! declared field directly — no fallback chain, no naming-convention guessing. A tuple missing
//! the declared field is a typed [`WorldError`], never a silent default.
//!
//! ## World execution profile admissibility
//!
//! Both engines refuse differing values for the same (decide, entity, proposal).
//! Equal values retain independent supports. Refusal is atomic and does not select
//! a first-arriving support. This restriction belongs to `brix.world.exec@1`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_lower::module_graph::{LinkedProgram, QualifiedName};
use brix_lower::relation_dag::{
    decision_field_schemas, derive_binding_names, derive_decide_binding_names, lower_relations,
    resolve_decide_source_relation, GroupProjection, OperatorId, OperatorNode, RelationDag,
};
use brix_lower::world_expr::{CompiledProgramEnv, CompiledWorldExpr};
use brix_syntax::ast;
use soc_core::calendar::Key;

use super::codec::TupleRecord;
use super::decision_codec::STRUCTURED_PREFIX;
use super::error::WorldError;
pub use super::stable_child_key::CollectOrdering;
use super::types::{WorldKey, WorldTuple};

/// A scalar value, independently defined from `network::Value` (see module docs §"what is
/// and is not shared"). Structurally the same admissible value set, but a separate type: no
/// code from `network.rs` is called to construct, compare, or serialize these.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    F64(brix_canon::FiniteF64),
    Decimal(brix_canon::Decimal),
    Record {
        nominal: String,
        fields: BTreeMap<String, Value>,
    },
    Variant {
        nominal: String,
        variant: String,
        args: Vec<Value>,
    },
    /// Ordered value; duplicates are significant.
    List(Vec<Value>),
}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Null => {}
            Self::List(items) => items.hash(state),
            Self::Bool(v) => v.hash(state),
            Self::Int(v) => v.hash(state),
            Self::Str(v) => v.hash(state),
            Self::F64(v) => v.to_string().hash(state),
            Self::Decimal(v) => {
                v.unscaled().hash(state);
                v.scale().hash(state);
            }
            Self::Record { nominal, fields } => {
                nominal.hash(state);
                fields.hash(state);
            }
            Self::Variant {
                nominal,
                variant,
                args,
            } => {
                nominal.hash(state);
                variant.hash(state);
                args.hash(state);
            }
        }
    }
}

impl Value {
    #[allow(dead_code)]
    fn as_bool(&self) -> Result<bool, WorldError> {
        match self {
            Self::Bool(v) => Ok(*v),
            other => Err(WorldError::NetworkError(format!(
                "reference evaluator: guard must be Bool, got {other:?}"
            ))),
        }
    }

    /// Decode a byte payload under a resolved scalar schema type. Mirrors the admissible
    /// scalar type set of the tuple codec/schema contract (ADR-0046 §3.2), re-derived from
    /// scratch rather than calling `network::Value::from_typed_bytes`.
    #[allow(dead_code)]
    fn from_typed_bytes(bytes: &[u8], ty: &ast::Ty) -> Result<Self, WorldError> {
        Self::from_typed_bytes_with_schemas(bytes, ty, &BTreeMap::new())
    }

    fn from_typed_bytes_with_schemas(
        bytes: &[u8],
        ty: &ast::Ty,
        schemas: &BTreeMap<String, brix_lower::l3_v2::L3Schema>,
    ) -> Result<Self, WorldError> {
        let name = match ty {
            ast::Ty::Named(name) => name.as_str(),
            _ => "structured value",
        };
        let text = || {
            std::str::from_utf8(bytes)
                .map_err(|e| WorldError::NetworkError(format!("invalid scalar UTF-8: {e}")))
        };
        let invalid = || WorldError::NetworkError(format!("invalid {name} scalar"));
        match name {
            "Str" => return Ok(Self::Str(text()?.to_owned())),
            "Int" => return text()?.parse().map(Self::Int).map_err(|_| invalid()),
            "Bool" => {
                return match text()? {
                    "true" => Ok(Self::Bool(true)),
                    "false" => Ok(Self::Bool(false)),
                    _ => Err(invalid()),
                }
            }
            "F64" => return text()?.parse().map(Self::F64).map_err(|_| invalid()),
            "Decimal" => {
                return brix_canon::decimal_parse(text()?)
                    .map(Self::Decimal)
                    .map_err(|_| invalid())
            }
            _ => {}
        }
        let value = if let Some(payload) = bytes.strip_prefix(STRUCTURED_PREFIX) {
            let mut reader = brix_canon::CanonReader::new(payload);
            let value = Self::canon_read(&mut reader)?;
            if !reader.is_empty() {
                return Err(WorldError::NetworkError(
                    "trailing structured value bytes".into(),
                ));
            }
            value.to_scalar()?
        } else {
            brix_lower::input::decode_world_value(bytes, &brix_lower::input::InputLimits::default())
                .map_err(|e| WorldError::NetworkError(format!("invalid structured value: {e}")))?
                .to_l3_value()
        };
        validate_reference_l3_value(&value, ty, schemas, "world input")
            .map_err(|e| WorldError::NetworkError(format!("type validation error: {e}")))?;
        Self::from_scalar(value)
    }

    fn to_bytes(&self) -> Vec<u8> {
        if matches!(
            self,
            Self::Record { .. } | Self::Variant { .. } | Self::List(_)
        ) {
            let mut writer = CanonWriter::new();
            self.canon_write(&mut writer);
            let mut bytes = STRUCTURED_PREFIX.to_vec();
            bytes.extend(writer.finish());
            bytes
        } else {
            self.to_string().into_bytes()
        }
    }

    fn to_scalar(&self) -> Result<brix_lower::l3_v2::L3ValueV2, WorldError> {
        use brix_lower::l3_v2::L3ValueV2 as V;
        Ok(match self {
            Self::List(items) => V::List(
                items
                    .iter()
                    .map(Self::to_scalar)
                    .collect::<Result<Vec<_>, _>>()?
                    .into(),
            ),
            Self::Bool(v) => V::Bool(*v),
            Self::Int(v) => V::Int(*v),
            Self::Str(v) => V::Str(v.clone()),
            Self::F64(v) => V::F64(*v),
            Self::Decimal(v) => V::Decimal(*v),
            Self::Record { nominal, fields } => {
                let mut f_vec = Vec::new();
                for (k, v) in fields {
                    f_vec.push((k.clone(), v.to_scalar()?));
                }
                V::Record {
                    nominal_config: nominal.clone(),
                    fields: f_vec,
                }
            }
            Self::Variant {
                nominal,
                variant,
                args,
            } => {
                let mut a_vec = Vec::new();
                for a in args {
                    a_vec.push(a.to_scalar()?);
                }
                V::Ctor {
                    nominal_sum: nominal.clone(),
                    variant: variant.clone(),
                    args: a_vec,
                }
            }
            Self::Null => {
                return Err(WorldError::NetworkError(
                    "reference evaluator: absent scalar value".into(),
                ))
            }
        })
    }

    fn from_scalar(v: brix_lower::l3_v2::L3ValueV2) -> Result<Self, WorldError> {
        use brix_lower::l3_v2::L3ValueV2 as V;
        Ok(match v {
            V::Bool(v) => Self::Bool(v),
            V::Int(v) => Self::Int(v),
            V::Str(v) => Self::Str(v),
            V::F64(v) => Self::F64(v),
            V::Decimal(v) => Self::Decimal(v),
            V::Record {
                nominal_config,
                fields,
            } => {
                let mut map = BTreeMap::new();
                for (k, f_val) in fields {
                    map.insert(k, Self::from_scalar(f_val)?);
                }
                Self::Record {
                    nominal: nominal_config,
                    fields: map,
                }
            }
            V::Ctor {
                nominal_sum,
                variant,
                args,
            } => {
                let mut a_vec = Vec::new();
                for a in args {
                    a_vec.push(Self::from_scalar(a)?);
                }
                Self::Variant {
                    nominal: nominal_sum,
                    variant,
                    args: a_vec,
                }
            }
            V::List(items) => Self::List(
                items
                    .iter()
                    .cloned()
                    .map(Self::from_scalar)
                    .collect::<Result<_, _>>()?,
            ),
        })
    }

    fn canon_read(r: &mut brix_canon::CanonReader<'_>) -> Result<Self, WorldError> {
        let mut remaining = brix_lower::input::MAX_WORLD_VALUE_NODES;
        Self::canon_read_bounded(r, 0, &mut remaining)
    }

    fn canon_read_bounded(
        r: &mut brix_canon::CanonReader<'_>,
        depth: usize,
        remaining: &mut usize,
    ) -> Result<Self, WorldError> {
        if depth >= brix_lower::input::MAX_INPUT_VALUE_DEPTH || *remaining == 0 {
            return Err(WorldError::NetworkError(
                "invalid or oversized structured reference value".into(),
            ));
        }
        *remaining -= 1;
        let canon_err = |e: brix_canon::CanonError| {
            WorldError::NetworkError(format!("corrupt canonical value: {e:?}"))
        };
        let tag = r.read_uint().map_err(canon_err)?;
        Ok(match tag {
            0 => Value::Null,
            1 => Value::Int(r.read_int().map_err(canon_err)?),
            2 => {
                let bytes = r.read_bytes().map_err(canon_err)?;
                Value::Str(
                    std::str::from_utf8(bytes)
                        .map_err(|e| WorldError::NetworkError(format!("invalid Str UTF-8: {e}")))?
                        .to_string(),
                )
            }
            3 => Value::Bool(match r.read_uint().map_err(canon_err)? {
                0 => false,
                1 => true,
                _ => return Err(WorldError::NetworkError("invalid decision boolean".into())),
            }),
            4 => {
                let raw = r.read_raw(8).map_err(canon_err)?;
                let mut bits = [0u8; 8];
                bits.copy_from_slice(raw);
                Value::F64(
                    brix_canon::FiniteF64::from_bits(u64::from_be_bytes(bits)).map_err(|e| {
                        WorldError::NetworkError(format!("corrupt value: invalid F64: {e:?}"))
                    })?,
                )
            }
            5 => Value::Decimal(brix_canon::read_decimal(r).map_err(canon_err)?),
            6 => {
                let nominal = std::str::from_utf8(r.read_bytes().map_err(canon_err)?)
                    .map_err(|e| WorldError::NetworkError(format!("invalid nominal UTF-8: {e}")))?
                    .to_string();
                let count = usize::try_from(r.read_uint().map_err(canon_err)?)
                    .map_err(|_| WorldError::NetworkError("invalid record field count".into()))?;
                if count > brix_lower::input::MAX_INPUT_CONTAINER_WIDTH || count > *remaining {
                    return Err(WorldError::NetworkError(
                        "invalid or oversized structured reference value".into(),
                    ));
                }
                let mut fields = BTreeMap::new();
                for _ in 0..count {
                    let name = std::str::from_utf8(r.read_bytes().map_err(canon_err)?)
                        .map_err(|e| {
                            WorldError::NetworkError(format!("invalid field name UTF-8: {e}"))
                        })?
                        .to_string();
                    let val = Self::canon_read_bounded(r, depth + 1, remaining)?;
                    if fields
                        .last_key_value()
                        .is_some_and(|(prev, _)| prev >= &name)
                    {
                        return Err(WorldError::NetworkError(
                            "invalid or oversized structured reference value".into(),
                        ));
                    }
                    fields.insert(name, val);
                }
                Value::Record { nominal, fields }
            }
            7 => {
                let nominal = std::str::from_utf8(r.read_bytes().map_err(canon_err)?)
                    .map_err(|e| WorldError::NetworkError(format!("invalid nominal UTF-8: {e}")))?
                    .to_string();
                let variant = std::str::from_utf8(r.read_bytes().map_err(canon_err)?)
                    .map_err(|e| WorldError::NetworkError(format!("invalid variant UTF-8: {e}")))?
                    .to_string();
                let count = usize::try_from(r.read_uint().map_err(canon_err)?)
                    .map_err(|_| WorldError::NetworkError("invalid variant arg count".into()))?;
                if count > brix_lower::input::MAX_INPUT_CONTAINER_WIDTH || count > *remaining {
                    return Err(WorldError::NetworkError(
                        "invalid or oversized structured reference value".into(),
                    ));
                }
                let mut args = Vec::with_capacity(count);
                for _ in 0..count {
                    args.push(Self::canon_read_bounded(r, depth + 1, remaining)?);
                }
                Value::Variant {
                    nominal,
                    variant,
                    args,
                }
            }
            8 => {
                let count = usize::try_from(r.read_uint().map_err(canon_err)?)
                    .map_err(|_| WorldError::NetworkError("invalid list length".into()))?;
                if count > brix_lower::input::MAX_COLLECTION_ELEMENTS || count > *remaining {
                    return Err(WorldError::NetworkError(
                        "list value exceeds container width or node limit".into(),
                    ));
                }
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(Self::canon_read_bounded(r, depth + 1, remaining)?);
                }
                Value::List(items)
            }
            other => {
                return Err(WorldError::NetworkError(format!(
                    "unknown value tag {other}"
                )))
            }
        })
    }

    /// Injective canonical storage encoding matching `state_codec::Value::state_write`.
    fn state_write(&self, w: &mut CanonWriter) {
        match self {
            Value::List(items) => {
                w.write_uint(8);
                w.write_uint(items.len() as u64);
                for item in items {
                    item.state_write(w);
                }
            }
            Value::Null => w.write_uint(0),
            Value::Bool(b) => {
                w.write_uint(1);
                w.write_bool(*b);
            }
            Value::Int(i) => {
                w.write_uint(2);
                w.write_int(*i);
            }
            Value::Str(s) => {
                w.write_uint(3);
                w.write_str(s);
            }
            Value::F64(f) => {
                w.write_uint(4);
                w.write_uint(f.bits());
            }
            Value::Decimal(d) => {
                w.write_uint(5);
                w.write_int128(d.unscaled());
                w.write_uint(u64::from(d.scale()));
            }
            Value::Record { nominal, fields } => {
                w.write_uint(6);
                w.write_str(nominal);
                w.write_uint(fields.len() as u64);
                for (name, value) in fields {
                    w.write_str(name);
                    value.state_write(w);
                }
            }
            Value::Variant {
                nominal,
                variant,
                args,
            } => {
                w.write_uint(7);
                w.write_str(nominal);
                w.write_str(variant);
                w.write_uint(args.len() as u64);
                for value in args {
                    value.state_write(w);
                }
            }
        }
    }

    /// Canonical write, reproduced from `network.rs`'s `canon_write_value` byte-for-byte
    /// (see module docs point 6: the calendar tie-break formula is a shared spec contract).
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Value::List(items) => {
                w.write_uint(8);
                w.write_uint(items.len() as u64);
                for item in items {
                    item.canon_write(w);
                }
            }
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
            Value::Record { nominal, fields } => {
                w.write_uint(6);
                w.write_str(nominal);
                w.write_uint(fields.len() as u64);
                for (name, val) in fields {
                    w.write_str(name);
                    val.canon_write(w);
                }
            }
            Value::Variant {
                nominal,
                variant,
                args,
            } => {
                w.write_uint(7);
                w.write_str(nominal);
                w.write_str(variant);
                w.write_uint(args.len() as u64);
                for arg in args {
                    arg.canon_write(w);
                }
            }
        }
    }
}

/// Independently validate reference values against the syntax-level schema.
/// The production L3 helper contract intentionally only covers scalar and
/// nominal types, while relational `collect(...).take()` introduces anonymous
/// record and nested list shapes. Keep the oracle's structural checks local.
fn validate_reference_l3_value(
    value: &brix_lower::l3_v2::L3ValueV2,
    expected: &ast::Ty,
    schemas: &BTreeMap<String, brix_lower::l3_v2::L3Schema>,
    path: &str,
) -> Result<(), String> {
    use brix_lower::l3_v2::{validate_l3_value, L3SchemaType, L3ValueV2};
    match (value, expected) {
        (L3ValueV2::List(items), ast::Ty::App(name, args)) if name == "List" && args.len() == 1 => {
            if items.len() > brix_lower::input::MAX_COLLECTION_ELEMENTS {
                return Err(format!(
                    "{path}: list has {} element(s), exceeding maximum of {}",
                    items.len(),
                    brix_lower::input::MAX_COLLECTION_ELEMENTS,
                ));
            }
            for (index, item) in items.iter().enumerate() {
                validate_reference_l3_value(item, &args[0], schemas, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        (
            L3ValueV2::Record {
                nominal_config,
                fields,
            },
            ast::Ty::Record(expected_fields),
        ) => {
            if !nominal_config.is_empty() {
                return Err(format!(
                    "{path}: expected anonymous record, found nominal '{nominal_config}'"
                ));
            }
            if fields.len() != expected_fields.len() {
                return Err(format!(
                    "{path}: record expects {} field(s), found {}",
                    expected_fields.len(),
                    fields.len()
                ));
            }
            let mut seen = BTreeSet::new();
            for (name, actual) in fields {
                if !seen.insert(name.as_str()) {
                    return Err(format!("{path}.{name}: duplicate field"));
                }
                let Some(declaration) = expected_fields.iter().find(|field| field.name == *name)
                else {
                    return Err(format!("{path}.{name}: unknown field"));
                };
                validate_reference_l3_value(
                    actual,
                    &declaration.ty,
                    schemas,
                    &format!("{path}.{name}"),
                )?;
            }
            Ok(())
        }
        (_, ast::Ty::Record(_)) => Err(format!("{path}: expected anonymous record")),
        (_, ast::Ty::App(name, args)) if name == "List" && args.len() == 1 => {
            Err(format!("{path}: expected {expected:?}, found non-list"))
        }
        _ => {
            let contract = match expected {
                ast::Ty::Named(name) => match name.as_str() {
                    "Int" => L3SchemaType::Int,
                    "Bool" => L3SchemaType::Bool,
                    "Str" => L3SchemaType::Str,
                    "F64" => L3SchemaType::F64,
                    "Decimal" => L3SchemaType::Decimal,
                    other => L3SchemaType::Named(other.to_owned()),
                },
                ast::Ty::App(name, args) => {
                    let mut parts = Vec::with_capacity(args.len());
                    for arg in args {
                        parts.push(brix_lower::world_expr::contract(arg)?);
                    }
                    L3SchemaType::Named(format!(
                        "{}<{}>",
                        name,
                        parts
                            .iter()
                            .map(L3SchemaType::display_name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
                other => return Err(format!("{path}: unsupported structural type {other:?}")),
            };
            validate_l3_value(value, &contract, schemas, path)
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::List(items) => {
                write!(f, "[")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Self::Null => write!(f, "null"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
            Self::F64(v) => write!(f, "{v}"),
            Self::Decimal(v) => write!(f, "{}", brix_canon::decimal_format(*v)),
            Self::Record { nominal, fields } => {
                write!(f, "{nominal} {{")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
            Self::Variant {
                nominal: _,
                variant,
                args,
            } => {
                write!(f, "{variant}")?;
                if !args.is_empty() {
                    write!(f, "(")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{a}")?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            }
        }
    }
}

/// A row flowing between operators: a plain field-name -> value map.
pub type Row = BTreeMap<String, Value>;

/// Source and derivation provenance carried alongside a relational row's semantic values,
/// kept separate from [`Row`] so relational deduplication (`Distinct`) compares only values.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Provenance {
    pub leaves: Vec<String>,
    pub stable_key: StableRowKey,
}

/// Typed source identity carried independently of production derivation
/// digests. The collect-take oracle orders by declared typed keys and then
/// these structural origins, never by hashed tuple IDs.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum StableRowKey {
    #[default]
    Unknown,
    Source {
        relation: String,
        key: Vec<Value>,
    },
    Join(Box<StableRowKey>, Box<StableRowKey>),
    Derived {
        relation: String,
        key: Vec<(String, Value)>,
    },
}

impl StableRowKey {
    fn compare(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let kind = |key: &Self| match key {
            Self::Unknown => "unknown",
            Self::Source { .. } => "source",
            Self::Join(..) => "join",
            Self::Derived { .. } => "derived",
        };
        match kind(self).cmp(kind(other)) {
            Ordering::Equal => match (self, other) {
                (
                    Self::Source {
                        relation: ar,
                        key: ak,
                    },
                    Self::Source {
                        relation: br,
                        key: bk,
                    },
                ) => compare_value_slices(ak, bk).then_with(|| ar.as_bytes().cmp(br.as_bytes())),
                (Self::Join(al, ar), Self::Join(bl, br)) => {
                    al.compare(bl).then_with(|| ar.compare(br))
                }
                (
                    Self::Derived {
                        relation: ar,
                        key: ak,
                    },
                    Self::Derived {
                        relation: br,
                        key: bk,
                    },
                ) => compare_named_values(ak, bk).then_with(|| ar.as_bytes().cmp(br.as_bytes())),
                (Self::Unknown, Self::Unknown) => Ordering::Equal,
                _ => Ordering::Equal,
            },
            other => other,
        }
    }
}

fn reference_value_rank(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int(_) => 2,
        Value::Decimal(_) => 3,
        Value::F64(_) => 4,
        Value::Str(_) => 5,
        Value::Record { .. } => 6,
        Value::Variant { .. } => 7,
        Value::List(_) => 8,
    }
}

fn compare_reference_values(left: &Value, right: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match reference_value_rank(left).cmp(&reference_value_rank(right)) {
        Ordering::Equal => match (left, right) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::Decimal(a), Value::Decimal(b)) => a.cmp(b),
            (Value::F64(a), Value::F64(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Value::Str(a), Value::Str(b)) => a.as_bytes().cmp(b.as_bytes()),
            (
                Value::Record {
                    nominal: an,
                    fields: af,
                },
                Value::Record {
                    nominal: bn,
                    fields: bf,
                },
            ) => an.as_bytes().cmp(bn.as_bytes()).then_with(|| {
                compare_named_values(
                    &af.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>(),
                    &bf.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>(),
                )
            }),
            (
                Value::Variant {
                    nominal: an,
                    variant: av,
                    args: aa,
                },
                Value::Variant {
                    nominal: bn,
                    variant: bv,
                    args: ba,
                },
            ) => an
                .as_bytes()
                .cmp(bn.as_bytes())
                .then_with(|| av.as_bytes().cmp(bv.as_bytes()))
                .then_with(|| compare_value_slices(aa, ba)),
            (Value::List(a), Value::List(b)) => compare_value_slices(a, b),
            _ => Ordering::Equal,
        },
        other => other,
    }
}

fn compare_value_slices(left: &[Value], right: &[Value]) -> std::cmp::Ordering {
    for (left_value, right_value) in left.iter().zip(right) {
        let order = compare_reference_values(left_value, right_value);
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

fn compare_named_values(left: &[(String, Value)], right: &[(String, Value)]) -> std::cmp::Ordering {
    for ((left_name, left_value), (right_name, right_value)) in left.iter().zip(right) {
        let order = left_name
            .as_bytes()
            .cmp(right_name.as_bytes())
            .then_with(|| compare_reference_values(left_value, right_value));
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

impl Provenance {
    pub fn base(relation: &str, key: &WorldKey, declared_key: Vec<Value>) -> Self {
        Self {
            leaves: vec![key.to_hex()],
            stable_key: StableRowKey::Source {
                relation: relation.to_string(),
                key: declared_key,
            },
        }
    }

    pub fn join(&self, right: &Self) -> Self {
        let mut leaves = Vec::with_capacity(self.leaves.len() + right.leaves.len());
        leaves.extend_from_slice(&self.leaves);
        leaves.extend_from_slice(&right.leaves);
        Self {
            leaves,
            stable_key: StableRowKey::Join(
                Box::new(self.stable_key.clone()),
                Box::new(right.stable_key.clone()),
            ),
        }
    }

    pub fn distinct_row(relation: &str, values: &Row) -> Self {
        let mut w = CanonWriter::new();
        w.write_uint(values.len() as u64);
        for (k, v) in values {
            w.write_str(k);
            v.state_write(&mut w);
        }
        Self {
            leaves: vec![WorldKey::new(w.finish()).to_hex()],
            stable_key: StableRowKey::Derived {
                relation: relation.into(),
                key: values
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            },
        }
    }

    pub fn group_key(relation: &str, key: &[Value]) -> Self {
        let mut w = CanonWriter::new();
        w.write_uint(key.len() as u64);
        for v in key {
            v.state_write(&mut w);
        }
        Self {
            leaves: vec![WorldKey::new(w.finish()).to_hex()],
            stable_key: StableRowKey::Derived {
                relation: relation.into(),
                key: key
                    .iter()
                    .enumerate()
                    .map(|(index, value)| (format!("group_{index}"), value.clone()))
                    .collect(),
            },
        }
    }

    fn union_key(key: &[Value], source: &Self) -> Self {
        let mut legacy = Self::group_key("", key);
        legacy.stable_key = source.stable_key.clone();
        legacy
    }

    pub fn ordering_key(&self) -> Value {
        Value::Str(self.leaves.join(":"))
    }
}

/// An evaluated relational row pairing semantic values with provenance metadata.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct EvaluatedRow {
    pub values: Row,
    pub provenance: Provenance,
}

fn get_qualified<'a>(row: &'a Row, binding: &str, field: &str) -> Option<&'a Value> {
    let mut parts = field.split('.');
    let first = parts.next()?;
    let mut current = row
        .get(&format!("{binding}.{first}"))
        .or_else(|| row.get(first))?;
    for seg in parts {
        match current {
            Value::Record { fields, .. } => {
                current = fields.get(seg)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn output_relation_for_operator(dag: &RelationDag, op_index: usize) -> Result<&str, WorldError> {
    fn unary_ancestor(dag: &RelationDag, current: usize, target: usize) -> bool {
        if current == target {
            return true;
        }
        match dag.nodes.get(current) {
            Some(
                OperatorNode::Bind { input, .. }
                | OperatorNode::Filter { input, .. }
                | OperatorNode::Project { input, .. }
                | OperatorNode::Distinct { input },
            ) => unary_ancestor(dag, input.0, target),
            _ => false,
        }
    }

    if let Some((relation, _)) = dag
        .relation_outputs
        .iter()
        .find(|(_, output)| output.0 == op_index)
    {
        return Ok(relation.as_str());
    }

    // Internal nodes such as GroupedCount can contribute stable identity to
    // a relation after the lowering pass has added set semantics and unary
    // binding/filter/project wrappers. Match the production identity resolver
    // while keeping this oracle's typed comparison implementation independent.
    let mut candidates = dag
        .relation_outputs
        .iter()
        .filter(|(_, output)| unary_ancestor(dag, output.0, op_index))
        .map(|(relation, _)| relation.as_str());
    let relation = candidates.next().ok_or_else(|| {
        WorldError::NetworkError(format!(
            "reference evaluator: operator {op_index} has no qualified relation output"
        ))
    })?;
    if candidates.next().is_some() {
        return Err(WorldError::NetworkError(format!(
            "reference evaluator: operator {op_index} has ambiguous qualified relation outputs"
        )));
    }
    Ok(relation)
}

fn merge_rows(left: &Row, right: &Row) -> Row {
    let mut merged = left.clone();
    for (k, v) in right {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

fn row_to_tuple_record(row: &Row) -> TupleRecord {
    let mut rec = TupleRecord::new();
    for (k, v) in row {
        rec.set(k.clone(), v.to_bytes());
    }
    rec
}

/// Build scalar bindings for the shared expression evaluator, re-derived from scratch from
/// `network.rs`'s `scalar_bindings` behavior: unqualified fields become plain bindings,
/// `binding.field` pairs are regrouped into a `world.row`-nominal record binding so qualified
/// projection (`o.sku`) resolves exactly like an ordinary record field access.
fn scalar_bindings(
    row: &Row,
) -> Result<BTreeMap<String, brix_lower::l3_v2::L3ValueV2>, WorldError> {
    use brix_lower::l3_v2::L3ValueV2;
    let mut bindings = BTreeMap::new();
    let mut records: BTreeMap<String, Vec<(String, L3ValueV2)>> = BTreeMap::new();
    for (name, value) in row {
        if let Some((binding, field)) = name.split_once('.') {
            records
                .entry(binding.to_owned())
                .or_default()
                .push((field.to_owned(), value.to_scalar()?));
        } else {
            bindings.insert(name.clone(), value.to_scalar()?);
        }
    }
    for (binding, fields) in records {
        bindings.insert(
            binding,
            L3ValueV2::Record {
                nominal_config: "world.row".into(),
                fields,
            },
        );
    }
    Ok(bindings)
}

#[allow(dead_code)]
fn eval_expr(
    expr: &ast::Expr,
    row: &Row,
    functions: &BTreeMap<String, ast::Callable>,
) -> Result<Value, WorldError> {
    let bindings = scalar_bindings(row)?;
    let compiled = CompiledWorldExpr::new(expr, functions, &bindings.keys().cloned().collect())
        .map_err(WorldError::NetworkError)?;
    Value::from_scalar(compiled.eval(&bindings).map_err(WorldError::NetworkError)?)
}

type GroupEntry = (Vec<Option<Value>>, Vec<Option<Value>>, EvaluatedRow);

fn build_grouped_row(
    relation: &str,
    projections: &[(String, GroupProjection)],
    key_vals: &[Value],
    entries: &[GroupEntry],
    collect_ordering: CollectOrdering,
) -> Result<EvaluatedRow, WorldError> {
    let mut row = Row::new();
    for (idx, (name, proj)) in projections.iter().enumerate() {
        match proj {
            GroupProjection::Key(idx) => {
                let v = key_vals.get(*idx).cloned().unwrap_or(Value::Null);
                row.insert(name.clone(), v);
            }
            GroupProjection::Count => {
                row.insert(name.clone(), Value::Int(entries.len() as i64));
            }
            GroupProjection::SumInt(_) => {
                let mut sum = 0i128;
                for (agg, _, _) in entries {
                    if let Some(Value::Int(value)) = &agg[idx] {
                        sum = sum.checked_add(i128::from(*value)).ok_or_else(|| {
                            WorldError::NetworkError(
                                "reference grouped Int accumulator overflow".into(),
                            )
                        })?;
                    }
                }
                row.insert(
                    name.clone(),
                    Value::Int(i64::try_from(sum).map_err(|_| {
                        WorldError::NetworkError("reference grouped Int sum overflow".into())
                    })?),
                );
            }
            GroupProjection::SumDecimal(_) => {
                let mut sum = ReferenceWideDecimal::default();
                let mut scale = 0u8;
                for (agg, _, _) in entries {
                    if let Some(Value::Decimal(value)) = &agg[idx] {
                        if value.scale() > scale {
                            sum.mul_pow10(value.scale() - scale);
                            scale = value.scale();
                        }
                        let mut addend = ReferenceWideDecimal::from_i128(value.unscaled());
                        addend.mul_pow10(scale - value.scale());
                        sum.add_assign(&addend);
                    }
                }
                row.insert(
                    name.clone(),
                    Value::Decimal(sum.to_decimal(scale).ok_or_else(|| {
                        WorldError::NetworkError("reference grouped Decimal sum overflow".into())
                    })?),
                );
            }
            GroupProjection::Min(_) | GroupProjection::Max(_) => {
                let selected = if matches!(proj, GroupProjection::Min(_)) {
                    entries
                        .iter()
                        .filter_map(|(agg, _, _)| agg[idx].as_ref())
                        .min()
                } else {
                    entries
                        .iter()
                        .filter_map(|(agg, _, _)| agg[idx].as_ref())
                        .max()
                };
                row.insert(name.clone(), selected.cloned().unwrap_or(Value::Null));
            }
            GroupProjection::Collect { limit, .. } => {
                if entries.len() > *limit {
                    return Err(WorldError::NetworkError(format!(
                        "grouped collect exceeded maximum cardinality limit of {limit}"
                    )));
                }
                let collected_list = ordered_collect_items(entries, idx, collect_ordering);
                row.insert(name.clone(), Value::List(collected_list));
            }
            GroupProjection::CollectTake { limit, .. } => {
                let all_items = ordered_collect_items(entries, idx, collect_ordering);
                let truncated = all_items.len() > *limit;
                let items = all_items.into_iter().take(*limit).collect();
                row.insert(
                    name.clone(),
                    Value::Record {
                        nominal: String::new(),
                        fields: BTreeMap::from([
                            ("items".into(), Value::List(items)),
                            ("truncated".into(), Value::Bool(truncated)),
                        ]),
                    },
                );
            }
        }
    }
    Ok(EvaluatedRow {
        values: row,
        provenance: Provenance::group_key(relation, key_vals),
    })
}

fn ordered_collect_items(
    entries: &[GroupEntry],
    projection_index: usize,
    collect_ordering: CollectOrdering,
) -> Vec<Value> {
    let mut items_with_order: Vec<(Value, StableRowKey, Row, Value, Value)> =
        Vec::with_capacity(entries.len());
    for (aggregates, orders, child_row) in entries {
        let item = aggregates[projection_index].clone().unwrap_or(Value::Null);
        let order = orders
            .get(projection_index)
            .and_then(Clone::clone)
            .unwrap_or_else(|| item.clone());
        items_with_order.push((
            order,
            child_row.provenance.stable_key.clone(),
            child_row.values.clone(),
            child_row.provenance.ordering_key(),
            item,
        ));
    }
    items_with_order.sort_by(|left, right| match collect_ordering {
        CollectOrdering::LegacyV1 => left
            .0
            .partial_cmp(&right.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                left.3
                    .partial_cmp(&right.3)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
        CollectOrdering::TypedKeyV1 => compare_reference_values(&left.0, &right.0)
            .then_with(|| left.1.compare(&right.1))
            .then_with(|| {
                compare_named_values(
                    &left
                        .2
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect::<Vec<_>>(),
                    &right
                        .2
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect::<Vec<_>>(),
                )
            }),
    });
    items_with_order
        .into_iter()
        .map(|(_, _, _, _, item)| item)
        .collect()
}

#[derive(Default)]
struct ReferenceWideDecimal {
    negative: bool,
    digits: Vec<u8>,
}

impl ReferenceWideDecimal {
    fn from_i128(value: i128) -> Self {
        let mut magnitude = value.unsigned_abs();
        let mut digits = Vec::new();
        while magnitude != 0 {
            digits.push((magnitude % 10) as u8);
            magnitude /= 10;
        }
        Self {
            negative: value < 0,
            digits,
        }
    }

    fn mul_pow10(&mut self, power: u8) {
        if !self.digits.is_empty() {
            self.digits
                .splice(0..0, std::iter::repeat_n(0, power as usize));
        }
    }

    fn add_assign(&mut self, other: &Self) {
        if other.digits.is_empty() {
            return;
        }
        if self.digits.is_empty() {
            *self = Self {
                negative: other.negative,
                digits: other.digits.clone(),
            };
            return;
        }
        if self.negative == other.negative {
            let mut carry = 0u8;
            for index in 0..self.digits.len().max(other.digits.len()) {
                if index == self.digits.len() {
                    self.digits.push(0);
                }
                let sum =
                    self.digits[index] + other.digits.get(index).copied().unwrap_or(0) + carry;
                self.digits[index] = sum % 10;
                carry = sum / 10;
            }
            if carry != 0 {
                self.digits.push(carry);
            }
            return;
        }
        let ordering = self
            .digits
            .len()
            .cmp(&other.digits.len())
            .then_with(|| self.digits.iter().rev().cmp(other.digits.iter().rev()));
        match ordering {
            std::cmp::Ordering::Equal => {
                self.digits.clear();
                self.negative = false;
            }
            std::cmp::Ordering::Greater => self.sub_abs(other),
            std::cmp::Ordering::Less => {
                let mut result = Self {
                    negative: other.negative,
                    digits: other.digits.clone(),
                };
                result.sub_abs(self);
                *self = result;
            }
        }
    }

    fn sub_abs(&mut self, other: &Self) {
        let mut borrow = 0i8;
        for index in 0..self.digits.len() {
            let mut digit = self.digits[index] as i8
                - other.digits.get(index).copied().unwrap_or(0) as i8
                - borrow;
            if digit < 0 {
                digit += 10;
                borrow = 1;
            } else {
                borrow = 0;
            }
            self.digits[index] = digit as u8;
        }
        while self.digits.last() == Some(&0) {
            self.digits.pop();
        }
        if self.digits.is_empty() {
            self.negative = false;
        }
    }

    fn to_decimal(&self, scale: u8) -> Option<brix_canon::Decimal> {
        let mut value = Self {
            negative: self.negative,
            digits: self.digits.clone(),
        };
        let mut scale = scale;
        while scale > 0 && value.digits.first() == Some(&0) {
            value.digits.remove(0);
            scale -= 1;
        }
        let magnitude = value.digits.iter().rev().try_fold(0u128, |sum, digit| {
            sum.checked_mul(10)?.checked_add(*digit as u128)
        })?;
        let unscaled = if value.negative {
            if magnitude == i128::MAX as u128 + 1 {
                i128::MIN
            } else {
                -i128::try_from(magnitude).ok()?
            }
        } else {
            i128::try_from(magnitude).ok()?
        };
        Some(brix_canon::Decimal::new(unscaled, scale))
    }
}

/// Extract a row's entity identity from its declared `per` field(s)
/// (ADR-0046, decided 2026-10-04; composite keys added in Alpha.9), independently
/// re-derived from `network.rs`'s `entity_id_for` by reading the same declared
/// field(s), not by calling its code (see module docs). A row missing any declared
/// field is a typed error, never a silent default.
fn extract_entity_id(
    eval_row: &Row,
    binder: &str,
    per_fields: &[String],
) -> Result<String, WorldError> {
    let mut values = Vec::with_capacity(per_fields.len());
    for per_field in per_fields {
        let val = get_qualified(eval_row, binder, per_field).ok_or_else(|| {
            WorldError::NetworkError(format!(
                "reference evaluator: decide entity field '{per_field}' missing from bound row \
                 (binder '{binder}')"
            ))
        })?;
        values.push(val.clone());
    }
    Ok(encode_reference_entity_id(&values))
}

fn encode_reference_entity_id(values: &[Value]) -> String {
    if values.len() == 1 {
        values[0].to_string()
    } else {
        let items: Vec<serde_json::Value> = values
            .iter()
            .map(|val| match val {
                Value::Str(s) => serde_json::Value::String(s.clone()),
                Value::Int(n) => serde_json::Value::Number((*n).into()),
                Value::Bool(b) => serde_json::Value::Bool(*b),
                Value::F64(f) => serde_json::json!({"type": "f64", "value": f.to_string()}),
                Value::Decimal(d) => {
                    serde_json::json!({"type": "decimal", "value": brix_canon::decimal_format(*d)})
                }
                _ => serde_json::Value::Null,
            })
            .collect();
        serde_json::to_string(&items).expect("canonical JSON array of scalar values")
    }
}

fn decode_reference_entity_id(
    entity_id: &str,
    per_types: &[ast::Ty],
) -> Result<Vec<Value>, WorldError> {
    if per_types.len() == 1 {
        return Ok(vec![Value::from_typed_bytes(
            entity_id.as_bytes(),
            &per_types[0],
        )?]);
    }
    let parsed: serde_json::Value = serde_json::from_str(entity_id).map_err(|e| {
        WorldError::NetworkError(format!(
            "reference evaluator: invalid composite entity key '{entity_id}': {e}"
        ))
    })?;
    let Some(items) = parsed.as_array() else {
        return Err(WorldError::NetworkError(format!(
            "reference evaluator: composite entity key '{entity_id}' is not a JSON array"
        )));
    };
    if items.len() != per_types.len() {
        return Err(WorldError::NetworkError(format!(
            "reference evaluator: composite entity key arity mismatch ({} vs {})",
            items.len(),
            per_types.len()
        )));
    }
    let mut out = Vec::with_capacity(per_types.len());
    for (item, ty) in items.iter().zip(per_types.iter()) {
        let expected = brix_lower::world_expr::contract(ty).map_err(WorldError::NetworkError)?;
        let val = match expected.display_name().as_str() {
            "Str" => Value::Str(
                item.as_str()
                    .ok_or_else(|| WorldError::NetworkError("expected Str key component".into()))?
                    .to_owned(),
            ),
            "Int" => Value::Int(
                item.as_i64()
                    .ok_or_else(|| WorldError::NetworkError("expected Int key component".into()))?,
            ),
            "Bool" => {
                Value::Bool(item.as_bool().ok_or_else(|| {
                    WorldError::NetworkError("expected Bool key component".into())
                })?)
            }
            "F64" => {
                let s = item
                    .get("value")
                    .and_then(|v| v.as_str())
                    .or_else(|| item.as_str())
                    .ok_or_else(|| WorldError::NetworkError("expected F64 key component".into()))?;
                Value::F64(
                    s.parse::<brix_canon::FiniteF64>().map_err(|_| {
                        WorldError::NetworkError("invalid F64 key component".into())
                    })?,
                )
            }
            "Decimal" => {
                let s = item
                    .get("value")
                    .and_then(|v| v.as_str())
                    .or_else(|| item.as_str())
                    .ok_or_else(|| {
                        WorldError::NetworkError("expected Decimal key component".into())
                    })?;
                Value::Decimal(brix_canon::decimal_parse(s).map_err(|_| {
                    WorldError::NetworkError("invalid Decimal key component".into())
                })?)
            }
            other => {
                return Err(WorldError::NetworkError(format!(
                    "reference evaluator: unsupported key component type '{other}'"
                )))
            }
        };
        out.push(val);
    }
    Ok(out)
}

/// Calendar tie-break digest, reproduced byte-for-byte from `network.rs`'s
/// `compute_candidate_calendar_key` (see module docs point 6): same tag, same field write
/// order, same canonical value encoding — a deliberate shared spec contract, not a shared
/// implementation.
fn candidate_tiebreak_digest(
    decide_name: &str,
    entity_id: &str,
    candidate_name: &str,
    value: &Value,
) -> Digest {
    let mut w = CanonWriter::new();
    w.write_tag("brix.candidate.tiebreak@1");
    w.write_str(decide_name);
    w.write_str(entity_id);
    w.write_str(candidate_name);
    value.canon_write(&mut w);
    w.digest(Domain::Value)
}

/// A precompiled proposal entry within a reference decide block.
#[derive(Clone, Debug)]
pub struct CompiledReferencePropose {
    pub name: String,
    pub priority: u64,
    pub guard: CompiledWorldExpr,
    pub value: CompiledWorldExpr,
}

/// A compiled decide block, independently defined from `network::DecideBlock` (same four
/// fields, separate type — see module docs).
#[derive(Clone, Debug)]
pub struct DecideSpec {
    pub name: String,
    pub binder: String,
    pub source_relation: String,
    /// Declared entity-identity field(s) (ADR-0046, decided 2026-10-04; composite
    /// keys added in Alpha.9); see `network::DecideBlock::per_fields`.
    pub per_field: String,
    pub per_type: ast::Ty,
    pub per_fields: Vec<String>,
    pub per_types: Vec<ast::Ty>,
    pub proposals: Vec<ast::ProposeDecl>,
    pub compiled_proposals: Vec<CompiledReferencePropose>,
}

/// A candidate surviving in the frontier for one entity, independently defined from
/// `network::CandidateEntry`. Carries `support_count` (how many distinct supporting rows
/// justify this candidate) in place of a derivation-id support *set* — see module docs on
/// why per-derivation support tracking is out of scope for this oracle.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReferenceCandidate {
    pub candidate_name: String,
    pub entity_id: String,
    pub priority: u64,
    pub phase: u64,
    pub value: Value,
    pub calendar_key: Key,
    pub support_count: usize,
}

/// A settled decision, independently defined from `network::SettledDecision`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReferenceSettlement {
    pub entity_id: String,
    pub candidate_name: String,
    pub priority: u64,
    pub phase: u64,
    pub value: Value,
    pub calendar_key: Key,
}

/// The full observable reference state, independently defined from
/// `network::WorldNetworkState`. Deliberately omits `distinct_supports` (an artifact of
/// `network.rs`'s own per-operator truth-maintenance bookkeeping that this oracle has no
/// counterpart for) — differential comparison is over `base_relations`, `derived_relations`,
/// `candidate_frontier`, and `settlements`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ReferenceState {
    pub base_relations: BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>,
    pub derived_relations: BTreeMap<String, BTreeSet<TupleRecord>>,
    pub candidate_frontier:
        BTreeMap<String, BTreeMap<String, BTreeMap<String, ReferenceCandidate>>>,
    pub settlements: BTreeMap<String, BTreeMap<String, ReferenceSettlement>>,
}

/// Precompiled expression sites for a reference operator node.
#[derive(Clone, Debug)]
pub enum CompiledOperator {
    Scan,
    Bind,
    Filter {
        compiled_predicate: CompiledWorldExpr,
    },
    Project {
        compiled_projections: Vec<(String, CompiledWorldExpr)>,
    },
    EquiJoin,
    AntiJoin,
    ParentCount,
    DisjointUnion,
    Distinct,
    GroupedCount {
        compiled_group_keys: Vec<CompiledWorldExpr>,
        compiled_aggregates: Vec<Option<CompiledWorldExpr>>,
        compiled_collect_orders: Vec<Option<CompiledWorldExpr>>,
    },
}

/// A resolved program ready for [`evaluate`]: the lowered DAG, extracted decide specs, and
/// the flat helper-function table. Independently assembled from a [`LinkedProgram`] by
/// [`from_program`] — the only genuinely shared step is [`lower_relations`] itself (see
/// module docs point 1).
#[derive(Clone, Debug)]
pub struct ReferenceProgram {
    pub dag: RelationDag,
    pub decides: Vec<DecideSpec>,
    pub functions: BTreeMap<String, ast::Callable>,
    pub compiled_operators: Vec<CompiledOperator>,
    pub program_env: Option<Arc<CompiledProgramEnv>>,
    pub collect_ordering: CollectOrdering,
}

/// Resolve a [`LinkedProgram`] into a [`ReferenceProgram`]. Independently re-implements the
/// small glue `network::WorldNetwork::from_program` performs around the shared
/// `lower_relations` call: resolving named row schemas to their linked record config, and
/// extracting `decide` blocks' source relation from their `list` expression.
pub fn from_program(program: &LinkedProgram) -> Result<ReferenceProgram, WorldError> {
    from_program_with_collect_ordering(program, CollectOrdering::LegacyV1)
}

/// Resolve a program with the collection-ordering mode selected by the saved
/// world execution profile.
pub fn from_program_with_collect_ordering(
    program: &LinkedProgram,
    collect_ordering: CollectOrdering,
) -> Result<ReferenceProgram, WorldError> {
    let mut dag = lower_relations(program)
        .map_err(|e| WorldError::NetworkError(format!("relational lowering: {e}")))?;

    if collect_ordering == CollectOrdering::LegacyV1 {
        if let Some((index, _)) = dag.nodes.iter().enumerate().find(|(_, node)| {
            matches!(node, OperatorNode::GroupedCount { projections, .. }
                if projections.iter().any(|(_, projection)| matches!(projection, GroupProjection::CollectTake { .. })))
        }) {
            return Err(WorldError::NetworkError(format!(
                "reference legacy collection profile does not support collect(...).take() at operator {index}"
            )));
        }
    }

    if collect_ordering == CollectOrdering::TypedKeyV1 {
        for (index, node) in dag.nodes.iter().enumerate() {
            if let OperatorNode::GroupedCount {
                input, projections, ..
            } = node
            {
                if projections.iter().any(|(_, projection)| {
                    matches!(
                        projection,
                        GroupProjection::Collect { .. } | GroupProjection::CollectTake { .. }
                    )
                }) && !reference_has_stable_identity_path(&dag, *input, &mut BTreeSet::new())
                {
                    return Err(WorldError::NetworkError(format!(
                        "reference collect at operator {index} has no stable typed child identity"
                    )));
                }
            }
        }
    }

    for node in &mut dag.nodes {
        if let OperatorNode::Scan {
            relation, schema, ..
        } = node
        {
            if let ast::Ty::Named(name) = schema {
                let owner = relation
                    .rsplit_once("::")
                    .map(|(m, _)| m)
                    .unwrap_or(&program.root_module);
                let (module, local) = name.rsplit_once("::").unwrap_or((owner, name));
                let qname = QualifiedName::new(module, local);
                let config = program
                    .configs
                    .get(&qname)
                    .or_else(|| program.available_configs.get(&qname))
                    .ok_or_else(|| {
                        WorldError::NetworkError(format!("unknown row schema {qname}"))
                    })?;
                match &config.body {
                    ast::ConfigBody::Record(fields) => *schema = ast::Ty::Record(fields.clone()),
                    _ => {
                        return Err(WorldError::NetworkError(format!(
                            "row schema {qname} must be a record"
                        )))
                    }
                }
            }
        }
    }

    let program_env =
        CompiledProgramEnv::from_linked_program(program).map_err(WorldError::NetworkError)?;
    let op_schemas = decision_field_schemas(&dag, program);

    let mut compiled_operators = Vec::with_capacity(dag.nodes.len());
    for node in &dag.nodes {
        match node {
            OperatorNode::Scan { .. } => compiled_operators.push(CompiledOperator::Scan),
            OperatorNode::DecisionRows { .. } => compiled_operators.push(CompiledOperator::Scan),
            OperatorNode::Bind { .. } => compiled_operators.push(CompiledOperator::Bind),
            OperatorNode::Filter { input, predicate } => {
                let bindings = derive_binding_names(&op_schemas[input.0]);
                let compiled_predicate = program_env
                    .compile_expr(predicate, &bindings)
                    .map_err(WorldError::NetworkError)?;
                compiled_operators.push(CompiledOperator::Filter { compiled_predicate });
            }
            OperatorNode::Project { input, projections } => {
                let bindings = derive_binding_names(&op_schemas[input.0]);
                let mut compiled_projections = Vec::with_capacity(projections.len());
                for (name, expr) in projections {
                    let compiled = program_env
                        .compile_expr(expr, &bindings)
                        .map_err(WorldError::NetworkError)?;
                    compiled_projections.push((name.clone(), compiled));
                }
                compiled_operators.push(CompiledOperator::Project {
                    compiled_projections,
                });
            }
            OperatorNode::EquiJoin { .. } => compiled_operators.push(CompiledOperator::EquiJoin),
            OperatorNode::AntiJoin { .. } => compiled_operators.push(CompiledOperator::AntiJoin),
            OperatorNode::DisjointUnion { .. } => {
                compiled_operators.push(CompiledOperator::DisjointUnion)
            }
            OperatorNode::ParentCount { .. } => {
                compiled_operators.push(CompiledOperator::ParentCount)
            }
            OperatorNode::Distinct { .. } | OperatorNode::UnionDistinct { .. } => {
                compiled_operators.push(CompiledOperator::Distinct)
            }
            OperatorNode::GroupedCount {
                input,
                group_keys,
                projections,
                ..
            } => {
                let bindings = derive_binding_names(&op_schemas[input.0]);
                let mut compiled_group_keys = Vec::with_capacity(group_keys.len());
                for expr in group_keys {
                    let compiled = program_env
                        .compile_expr(expr, &bindings)
                        .map_err(WorldError::NetworkError)?;
                    compiled_group_keys.push(compiled);
                }
                let compiled_aggregates = projections
                    .iter()
                    .map(|(_, projection)| {
                        let expr = match projection {
                            GroupProjection::SumInt(expr)
                            | GroupProjection::SumDecimal(expr)
                            | GroupProjection::Min(expr)
                            | GroupProjection::Max(expr)
                            | GroupProjection::Collect { expr, .. }
                            | GroupProjection::CollectTake { expr, .. } => Some(expr),
                            GroupProjection::Key(_) | GroupProjection::Count => None,
                        };
                        expr.map(|e| program_env.compile_expr(e, &bindings))
                            .transpose()
                            .map_err(WorldError::NetworkError)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let compiled_collect_orders = projections
                    .iter()
                    .map(|(_, projection)| {
                        let order_expr = match projection {
                            GroupProjection::Collect {
                                order_by: Some(order_expr),
                                ..
                            }
                            | GroupProjection::CollectTake {
                                order_by: Some(order_expr),
                                ..
                            } => Some(order_expr),
                            _ => None,
                        };
                        if let Some(order_expr) = order_expr {
                            program_env
                                .compile_expr(order_expr, &bindings)
                                .map(Some)
                                .map_err(WorldError::NetworkError)
                        } else {
                            Ok(None)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                compiled_operators.push(CompiledOperator::GroupedCount {
                    compiled_group_keys,
                    compiled_aggregates,
                    compiled_collect_orders,
                });
            }
        }
    }

    let mut decides = Vec::new();
    for (qname, decl) in &program.decides {
        let source_relation =
            resolve_decide_source_relation(&decl.list, &qname.module, &dag.relation_outputs);

        if let Some(src) = source_relation {
            // `lower_relations` above already refused a relational decide with
            // no declared `per` field; this is defensive, not reachable.
            let per_fields: Vec<String> = if !decl.per_fields.is_empty() {
                decl.per_fields.clone()
            } else if let Some(per) = &decl.per {
                vec![per.clone()]
            } else {
                return Err(WorldError::NetworkError(format!(
                    "reference evaluator: decide '{qname}' resolved to relation '{src}' with no \
                     'per' field (expected lower_relations to have refused this)"
                )));
            };
            let op_id = dag.relation_outputs.get(&src).ok_or_else(|| {
                WorldError::NetworkError(format!(
                    "decide '{qname}' unknown source relation '{src}'"
                ))
            })?;
            let mut per_types = Vec::with_capacity(per_fields.len());
            for pf in &per_fields {
                let pt = op_schemas[op_id.0]
                    .get(&format!("{}.{pf}", decl.binder))
                    .or_else(|| op_schemas[op_id.0].get(pf))
                    .cloned()
                    .ok_or_else(|| {
                        WorldError::NetworkError(format!(
                            "reference decide '{qname}' per field '{pf}' has no resolved type"
                        ))
                    })?;
                per_types.push(pt);
            }
            let per_field = per_fields[0].clone();
            let per_type = per_types[0].clone();
            let bindings = derive_decide_binding_names(&op_schemas[op_id.0], &decl.binder);
            let mut compiled_proposals = Vec::with_capacity(decl.proposals.len());
            for prop in &decl.proposals {
                let guard = program_env
                    .compile_expr(&prop.guard, &bindings)
                    .map_err(WorldError::NetworkError)?;
                let value = program_env
                    .compile_expr(&prop.value, &bindings)
                    .map_err(WorldError::NetworkError)?;
                compiled_proposals.push(CompiledReferencePropose {
                    name: prop.name.clone(),
                    priority: prop.priority,
                    guard,
                    value,
                });
            }
            decides.push(DecideSpec {
                name: qname.to_string(),
                binder: decl.binder.clone(),
                source_relation: src,
                per_field,
                per_type,
                per_fields,
                per_types,
                proposals: decl.proposals.clone(),
                compiled_proposals,
            });
        }
    }

    let mut functions = BTreeMap::new();
    for (qname, callable) in &program.functions {
        functions.insert(qname.to_string(), callable.clone());
        functions.insert(callable.name.clone(), callable.clone());
    }

    Ok(ReferenceProgram {
        dag,
        decides,
        functions,
        compiled_operators,
        program_env: Some(program_env),
        collect_ordering,
    })
}

/// Independent structural check that a collect input has a typed identity
/// path. The oracle does not call the production helper in
/// `stable_child_key.rs`.
fn reference_has_stable_identity_path(
    dag: &RelationDag,
    op: OperatorId,
    seen: &mut BTreeSet<OperatorId>,
) -> bool {
    if !seen.insert(op) {
        return false;
    }
    match dag.nodes.get(op.0) {
        Some(OperatorNode::Scan { .. } | OperatorNode::DecisionRows { .. }) => true,
        Some(
            OperatorNode::Bind { input, .. }
            | OperatorNode::Filter { input, .. }
            | OperatorNode::Project { input, .. },
        ) => reference_has_stable_identity_path(dag, *input, seen),
        Some(OperatorNode::EquiJoin { left, right, .. }) => {
            reference_has_stable_identity_path(dag, *left, seen)
                && reference_has_stable_identity_path(dag, *right, seen)
        }
        Some(OperatorNode::DisjointUnion { left, right, .. }) => {
            reference_has_stable_identity_path(dag, *left, seen)
                && reference_has_stable_identity_path(dag, *right, seen)
        }
        Some(OperatorNode::AntiJoin { left, .. } | OperatorNode::ParentCount { left, .. }) => {
            reference_has_stable_identity_path(dag, *left, seen)
        }
        Some(OperatorNode::Distinct { .. } | OperatorNode::UnionDistinct { .. }) => true,
        Some(OperatorNode::GroupedCount { .. }) => true,
        None => false,
    }
}

/// Evaluate a resolved program against a full base-relation snapshot by plain set semantics,
/// with no incremental state of any kind. See module docs for exactly what is and is not
/// Meter tracking active computational work performed by the reference evaluator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReferenceWorkMeter {
    pub tuples_scanned: u64,
    pub rows_evaluated: u64,
    pub join_pairs_evaluated: u64,
    pub expressions_evaluated: u64,
    pub candidates_evaluated: u64,
    pub settlements_computed: u64,
}

impl ReferenceWorkMeter {
    pub fn total_work(&self) -> u64 {
        self.tuples_scanned
            .saturating_add(self.rows_evaluated)
            .saturating_add(self.join_pairs_evaluated)
            .saturating_add(self.expressions_evaluated)
            .saturating_add(self.candidates_evaluated)
            .saturating_add(self.settlements_computed)
    }
}

#[inline]
fn charge_work(
    meter: &mut ReferenceWorkMeter,
    work_offset: u64,
    max_work: Option<u64>,
) -> Result<(), WorldError> {
    if let Some(max) = max_work {
        if work_offset.saturating_add(meter.total_work()) > max {
            return Err(WorldError::BudgetExhausted);
        }
    }
    Ok(())
}

fn eval_compiled_with_budget(
    compiled: &brix_lower::world_expr::CompiledWorldExpr,
    bindings: &BTreeMap<String, brix_lower::l3_v2::L3ValueV2>,
    meter: &mut ReferenceWorkMeter,
    work_offset: u64,
    max_work: Option<u64>,
) -> Result<brix_lower::l3_v2::L3ValueV2, WorldError> {
    if let Some(max) = max_work {
        if work_offset.saturating_add(meter.total_work()) >= max {
            return Err(WorldError::BudgetExhausted);
        }
    }
    let remaining_steps = max_work.map(|m| {
        let used = work_offset.saturating_add(meter.total_work());
        (m.saturating_sub(used) as usize).max(1)
    });
    let (val, steps) = compiled
        .eval_with_budget(bindings, remaining_steps)
        .map_err(|e| {
            if e.contains("ResourceExhausted") || e.contains("step limit exceeded") {
                WorldError::BudgetExhausted
            } else {
                WorldError::NetworkError(e)
            }
        })?;
    meter.expressions_evaluated += (steps as u64).max(1);
    charge_work(meter, work_offset, max_work)?;
    Ok(val)
}

/// Evaluate a resolved program with active budget and work metering.
pub fn evaluate_with_budget(
    program: &ReferenceProgram,
    base_relations: &BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>,
    meter: &mut ReferenceWorkMeter,
    max_work: Option<u64>,
    work_offset: u64,
) -> Result<ReferenceState, WorldError> {
    let mut staged_relations = base_relations.clone();
    for _ in 0..=program.decides.len() {
        let mut state =
            evaluate_once_with_budget(program, &staged_relations, meter, max_work, work_offset)?;
        let mut next_relations = staged_relations.clone();
        for decide in &program.decides {
            let mut rows = BTreeMap::new();
            if let Some(entities) = state.settlements.get(&decide.name) {
                for (entity_id, winner) in entities {
                    let entity_values = decode_reference_entity_id(entity_id, &decide.per_types)?;
                    let priority = i64::try_from(winner.priority).map_err(|_| {
                        WorldError::NetworkError(format!(
                            "reference winner priority for '{}' exceeds the winner row Int range",
                            decide.name
                        ))
                    })?;
                    let mut row = BTreeMap::from([
                        (
                            "candidate".into(),
                            Value::Str(winner.candidate_name.clone()),
                        ),
                        ("priority".into(), Value::Int(priority)),
                        ("value".into(), winner.value.clone()),
                    ]);
                    for (pf, val) in decide.per_fields.iter().zip(entity_values.iter()) {
                        row.insert(pf.clone(), val.clone());
                    }
                    let row_key = if entity_values.len() == 1 {
                        WorldKey::new(entity_values[0].to_bytes())
                    } else {
                        WorldKey::new(encode_reference_entity_id(&entity_values).into_bytes())
                    };
                    rows.insert(row_key, row_to_tuple_record(&row).to_tuple());
                }
            }
            next_relations.insert(decide.name.clone(), rows);
        }
        if next_relations == staged_relations {
            state.base_relations = base_relations.clone();
            return Ok(state);
        }
        staged_relations = next_relations;
    }
    Err(WorldError::NetworkError(
        "reference winner-row strata did not reach a fixed point despite acyclic lowering".into(),
    ))
}

fn evaluate_once_with_budget(
    program: &ReferenceProgram,
    base_relations: &BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>,
    meter: &mut ReferenceWorkMeter,
    max_work: Option<u64>,
    work_offset: u64,
) -> Result<ReferenceState, WorldError> {
    let dag = &program.dag;
    let mut operator_outputs: Vec<Vec<EvaluatedRow>> = Vec::with_capacity(dag.nodes.len());

    for (idx, node) in dag.nodes.iter().enumerate() {
        let rows = match node {
            OperatorNode::Scan {
                relation, schema, ..
            }
            | OperatorNode::DecisionRows {
                decide: relation,
                schema,
            } => {
                let ast::Ty::Record(fields) = schema else {
                    return Err(WorldError::NetworkError(format!(
                        "unresolved row schema for {relation}"
                    )));
                };
                let empty = BTreeMap::new();
                let records = base_relations.get(relation).unwrap_or(&empty);
                let source_key_fields = match node {
                    OperatorNode::Scan { key_fields, .. } => key_fields.clone(),
                    OperatorNode::DecisionRows { decide, .. } => {
                        let decision = program
                            .decides
                            .iter()
                            .find(|decision| decision.name == *decide)
                            .ok_or_else(|| {
                                WorldError::NetworkError(format!(
                                    "reference evaluator: decision row source '{decide}' has no compiled decide declaration"
                                ))
                            })?;
                        if decision.per_fields.is_empty() {
                            vec![decision.per_field.clone()]
                        } else {
                            decision.per_fields.clone()
                        }
                    }
                    _ => unreachable!(),
                };
                let mut rows = Vec::with_capacity(records.len());
                for (key, tuple) in records {
                    meter.tuples_scanned += 1;
                    charge_work(meter, work_offset, max_work)?;
                    let rec = TupleRecord::from_tuple(tuple)?;
                    if rec.fields.len() != fields.len() {
                        return Err(WorldError::NetworkError(format!(
                            "row fields do not match schema for {relation}"
                        )));
                    }
                    let mut row = Row::new();
                    for field in fields {
                        let bytes = rec.get(&field.name).ok_or_else(|| {
                            WorldError::NetworkError(format!(
                                "missing field {relation}.{}",
                                field.name
                            ))
                        })?;
                        let empty_schemas = BTreeMap::new();
                        let schemas = program
                            .program_env
                            .as_ref()
                            .map(|pe| pe.schemas.as_ref())
                            .unwrap_or(&empty_schemas);
                        row.insert(
                            field.name.clone(),
                            Value::from_typed_bytes_with_schemas(bytes, &field.ty, schemas)
                                .map_err(|e| match e {
                                    WorldError::NetworkError(msg) => WorldError::NetworkError(
                                        format!("{relation}[{key}].{}: {msg}", field.name),
                                    ),
                                    other => other,
                                })?,
                        );
                    }
                    let declared_key = source_key_fields
                        .iter()
                        .map(|name| {
                            row.get(name).cloned().ok_or_else(|| {
                                WorldError::NetworkError(format!(
                                    "missing declared source key {relation}.{name}"
                                ))
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    rows.push(EvaluatedRow {
                        values: row,
                        provenance: Provenance::base(relation, key, declared_key),
                    });
                }
                rows
            }
            OperatorNode::Bind { input, alias } => {
                let mut bound_rows = Vec::with_capacity(operator_outputs[input.0].len());
                for row in &operator_outputs[input.0] {
                    meter.rows_evaluated += 1;
                    charge_work(meter, work_offset, max_work)?;
                    let mut bound = Row::new();
                    for (k, v) in &row.values {
                        bound.insert(format!("{alias}.{k}"), v.clone());
                        bound.insert(k.clone(), v.clone());
                    }
                    bound_rows.push(EvaluatedRow {
                        values: bound,
                        provenance: row.provenance.clone(),
                    });
                }
                bound_rows
            }
            OperatorNode::Filter { input, .. } => {
                let compiled_pred = match &program.compiled_operators[idx] {
                    CompiledOperator::Filter { compiled_predicate } => compiled_predicate,
                    _ => unreachable!(),
                };
                let mut out = Vec::new();
                for row in &operator_outputs[input.0] {
                    meter.rows_evaluated += 1;
                    charge_work(meter, work_offset, max_work)?;
                    let bindings = scalar_bindings(&row.values)?;
                    let l3_val = eval_compiled_with_budget(
                        compiled_pred,
                        &bindings,
                        meter,
                        work_offset,
                        max_work,
                    )?;
                    let passed = match l3_val {
                        brix_lower::l3_v2::L3ValueV2::Bool(b) => b,
                        other => {
                            return Err(WorldError::NetworkError(format!(
                                "reference evaluator: filter predicate must be Bool, got {other:?}"
                            )))
                        }
                    };
                    if passed {
                        out.push(row.clone());
                    }
                }
                out
            }
            OperatorNode::Project { input, .. } => {
                let compiled_projs = match &program.compiled_operators[idx] {
                    CompiledOperator::Project {
                        compiled_projections,
                    } => compiled_projections,
                    _ => unreachable!(),
                };
                let mut out = Vec::with_capacity(operator_outputs[input.0].len());
                for row in &operator_outputs[input.0] {
                    meter.rows_evaluated += 1;
                    let bindings = scalar_bindings(&row.values)?;
                    let mut projected = Row::new();
                    for (name, compiled) in compiled_projs {
                        let l3_val = eval_compiled_with_budget(
                            compiled,
                            &bindings,
                            meter,
                            work_offset,
                            max_work,
                        )?;
                        projected.insert(name.clone(), Value::from_scalar(l3_val)?);
                    }
                    out.push(EvaluatedRow {
                        values: projected,
                        provenance: row.provenance.clone(),
                    });
                }
                out
            }
            OperatorNode::EquiJoin {
                left,
                right,
                left_keys,
                right_keys,
            } => {
                let left_rows = &operator_outputs[left.0];
                let right_rows = &operator_outputs[right.0];

                let mut right_index: BTreeMap<Vec<Value>, Vec<&EvaluatedRow>> = BTreeMap::new();
                for r in right_rows {
                    let rkey: Vec<Value> = right_keys
                        .iter()
                        .map(|k| {
                            get_qualified(&r.values, &k.binding, &k.field)
                                .cloned()
                                .ok_or_else(|| {
                                    WorldError::NetworkError(format!(
                                        "missing join field {}.{}",
                                        k.binding, k.field
                                    ))
                                })
                        })
                        .collect::<Result<_, _>>()?;
                    right_index.entry(rkey).or_default().push(r);
                }

                let mut out = Vec::new();
                for l in left_rows {
                    let lkey: Vec<Value> = left_keys
                        .iter()
                        .map(|k| {
                            get_qualified(&l.values, &k.binding, &k.field)
                                .cloned()
                                .ok_or_else(|| {
                                    WorldError::NetworkError(format!(
                                        "missing join field {}.{}",
                                        k.binding, k.field
                                    ))
                                })
                        })
                        .collect::<Result<_, _>>()?;
                    if let Some(matches) = right_index.get(&lkey) {
                        for r in matches {
                            meter.join_pairs_evaluated += 1;
                            charge_work(meter, work_offset, max_work)?;
                            out.push(EvaluatedRow {
                                values: merge_rows(&l.values, &r.values),
                                provenance: l.provenance.join(&r.provenance),
                            });
                        }
                    }
                }
                out
            }
            OperatorNode::AntiJoin {
                left,
                right,
                left_keys,
                right_keys,
            } => {
                let mut right_index: BTreeMap<Vec<Value>, usize> = BTreeMap::new();
                for row in &operator_outputs[right.0] {
                    let key = right_keys
                        .iter()
                        .map(|k| {
                            get_qualified(&row.values, &k.binding, &k.field)
                                .cloned()
                                .ok_or_else(|| {
                                    WorldError::NetworkError(format!(
                                        "missing anti-join field {}.{}",
                                        k.binding, k.field
                                    ))
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    *right_index.entry(key).or_default() += 1;
                }
                operator_outputs[left.0]
                    .iter()
                    .filter_map(|row| {
                        let key = left_keys
                            .iter()
                            .map(|k| {
                                get_qualified(&row.values, &k.binding, &k.field)
                                    .cloned()
                                    .ok_or_else(|| {
                                        WorldError::NetworkError(format!(
                                            "missing anti-join field {}.{}",
                                            k.binding, k.field
                                        ))
                                    })
                            })
                            .collect::<Result<Vec<_>, _>>();
                        match key {
                            Ok(key) if !right_index.contains_key(&key) => Some(Ok(row.clone())),
                            Ok(_) => None,
                            Err(e) => Some(Err(e)),
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            OperatorNode::DisjointUnion {
                left,
                right,
                key_fields,
            } => {
                let mut keys = BTreeSet::new();
                let mut out = Vec::new();
                for row in operator_outputs[left.0]
                    .iter()
                    .chain(&operator_outputs[right.0])
                {
                    meter.rows_evaluated += 1;
                    charge_work(meter, work_offset, max_work)?;
                    let key: Vec<_> = key_fields
                        .iter()
                        .map(|name| {
                            row.values.get(name).cloned().ok_or_else(|| {
                                WorldError::NetworkError(format!("missing union key field {name}"))
                            })
                        })
                        .collect::<Result<_, _>>()?;
                    if !keys.insert(key.clone()) {
                        return Err(WorldError::NetworkError(format!(
                            "duplicate key in disjoint union: {key:?}"
                        )));
                    }
                    out.push(EvaluatedRow {
                        values: row.values.clone(),
                        provenance: Provenance::union_key(&key, &row.provenance),
                    });
                }
                out
            }
            OperatorNode::ParentCount {
                left,
                right,
                left_key,
                right_key,
                output_key,
                output_count,
            } => {
                let left_rows = &operator_outputs[left.0];
                let right_rows = &operator_outputs[right.0];
                let mut out = Vec::with_capacity(left_rows.len());
                for parent in left_rows {
                    let key = get_qualified(&parent.values, &left_key.binding, &left_key.field)
                        .cloned()
                        .ok_or_else(|| {
                            WorldError::NetworkError(format!(
                                "missing count correlation field {}.{}",
                                left_key.binding, left_key.field
                            ))
                        })?;
                    let mut count = 0i64;
                    for child in right_rows {
                        meter.rows_evaluated += 1;
                        charge_work(meter, work_offset, max_work)?;
                        if get_qualified(&child.values, &right_key.binding, &right_key.field)
                            == Some(&key)
                        {
                            count = count.checked_add(1).ok_or_else(|| {
                                WorldError::NetworkError("count_or_zero overflow".into())
                            })?;
                        }
                    }
                    let mut row = Row::new();
                    row.insert(output_key.clone(), key);
                    row.insert(output_count.clone(), Value::Int(count));
                    out.push(EvaluatedRow {
                        values: row,
                        provenance: parent.provenance.clone(),
                    });
                }
                out
            }
            OperatorNode::Distinct { input } => {
                let relation = output_relation_for_operator(dag, idx)?;
                let mut set: BTreeSet<Row> = BTreeSet::new();
                for row in &operator_outputs[input.0] {
                    meter.rows_evaluated += 1;
                    charge_work(meter, work_offset, max_work)?;
                    set.insert(row.values.clone());
                }
                set.into_iter()
                    .map(|values| {
                        let provenance = Provenance::distinct_row(relation, &values);
                        EvaluatedRow { values, provenance }
                    })
                    .collect()
            }
            OperatorNode::UnionDistinct { sources } => {
                let relation = output_relation_for_operator(dag, idx)?;
                let mut set: BTreeSet<Row> = BTreeSet::new();
                for src in sources {
                    for row in &operator_outputs[src.0] {
                        meter.rows_evaluated += 1;
                        charge_work(meter, work_offset, max_work)?;
                        set.insert(row.values.clone());
                    }
                }
                set.into_iter()
                    .map(|values| {
                        let provenance = Provenance::distinct_row(relation, &values);
                        EvaluatedRow { values, provenance }
                    })
                    .collect()
            }
            OperatorNode::GroupedCount {
                input,
                group_keys: _,
                projections,
            } => {
                let (compiled_keys, compiled_aggregates, compiled_collect_orders) =
                    match &program.compiled_operators[idx] {
                        CompiledOperator::GroupedCount {
                            compiled_group_keys,
                            compiled_aggregates,
                            compiled_collect_orders,
                        } => (
                            compiled_group_keys,
                            compiled_aggregates,
                            compiled_collect_orders,
                        ),
                        _ => unreachable!(),
                    };
                let mut groups: BTreeMap<Vec<Value>, Vec<GroupEntry>> = BTreeMap::new();
                for row in &operator_outputs[input.0] {
                    meter.rows_evaluated += 1;
                    charge_work(meter, work_offset, max_work)?;
                    let bindings = scalar_bindings(&row.values)?;
                    let mut key = Vec::with_capacity(compiled_keys.len());
                    for compiled in compiled_keys {
                        let l3_val = eval_compiled_with_budget(
                            compiled,
                            &bindings,
                            meter,
                            work_offset,
                            max_work,
                        )?;
                        key.push(Value::from_scalar(l3_val)?);
                    }
                    let mut aggregates = Vec::with_capacity(compiled_aggregates.len());
                    for compiled in compiled_aggregates {
                        let value = if let Some(compiled) = compiled {
                            let l3_val = eval_compiled_with_budget(
                                compiled,
                                &bindings,
                                meter,
                                work_offset,
                                max_work,
                            )?;
                            Some(Value::from_scalar(l3_val)?)
                        } else {
                            None
                        };
                        aggregates.push(value);
                    }
                    let mut orders = Vec::with_capacity(compiled_collect_orders.len());
                    for compiled in compiled_collect_orders {
                        let value = if let Some(compiled) = compiled {
                            let l3_val = eval_compiled_with_budget(
                                compiled,
                                &bindings,
                                meter,
                                work_offset,
                                max_work,
                            )?;
                            Some(Value::from_scalar(l3_val)?)
                        } else {
                            None
                        };
                        orders.push(value);
                    }
                    groups
                        .entry(key)
                        .or_default()
                        .push((aggregates, orders, row.clone()));
                }
                groups
                    .into_iter()
                    .map(|(key, entries)| {
                        build_grouped_row(
                            output_relation_for_operator(dag, idx)?,
                            projections,
                            &key,
                            &entries,
                            program.collect_ordering,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        operator_outputs.push(rows);
    }

    let mut derived_relations = BTreeMap::new();
    for (name, op_id) in &dag.relation_outputs {
        if matches!(dag.nodes[op_id.0], OperatorNode::Scan { .. }) {
            continue;
        }
        let set: BTreeSet<TupleRecord> = operator_outputs[op_id.0]
            .iter()
            .map(|r| row_to_tuple_record(&r.values))
            .collect();
        derived_relations.insert(name.clone(), set);
    }

    let mut candidate_frontier: BTreeMap<
        String,
        BTreeMap<String, BTreeMap<String, ReferenceCandidate>>,
    > = BTreeMap::new();
    let mut settlements: BTreeMap<String, BTreeMap<String, ReferenceSettlement>> = BTreeMap::new();

    for decide in &program.decides {
        let source_op: OperatorId = *dag
            .relation_outputs
            .get(&decide.source_relation)
            .ok_or_else(|| {
                WorldError::NetworkError(format!(
                    "decide {} source relation {} not found in DAG",
                    decide.name, decide.source_relation
                ))
            })?;
        let source_rows = &operator_outputs[source_op.0];

        let mut entities: BTreeMap<String, BTreeMap<String, ReferenceCandidate>> = BTreeMap::new();

        for row in source_rows {
            let mut eval_row = row.values.clone();
            for (k, v) in &row.values {
                eval_row.insert(format!("{}.{k}", decide.binder), v.clone());
            }

            for (p_idx, propose) in decide.proposals.iter().enumerate() {
                let compiled_prop = &decide.compiled_proposals[p_idx];
                meter.candidates_evaluated += 1;
                charge_work(meter, work_offset, max_work)?;
                let entity_id = extract_entity_id(&eval_row, &decide.binder, &decide.per_fields)?;
                let bindings = scalar_bindings(&eval_row)?;
                let guard_l3 = eval_compiled_with_budget(
                    &compiled_prop.guard,
                    &bindings,
                    meter,
                    work_offset,
                    max_work,
                )?;
                let guard_passed = match guard_l3 {
                    brix_lower::l3_v2::L3ValueV2::Bool(b) => b,
                    other => {
                        return Err(WorldError::NetworkError(format!(
                            "reference evaluator: guard must be Bool, got {other:?}"
                        )))
                    }
                };
                if !guard_passed {
                    continue;
                }
                let val_l3 = eval_compiled_with_budget(
                    &compiled_prop.value,
                    &bindings,
                    meter,
                    work_offset,
                    max_work,
                )?;
                let val = Value::from_scalar(val_l3)?;

                let entity_map = entities.entry(entity_id.clone()).or_default();
                match entity_map.get_mut(&propose.name) {
                    Some(existing) => {
                        if existing.value != val {
                            return Err(WorldError::NetworkError(format!(
                                "reference evaluator: ambiguous candidate value for decide \
                                 '{}' entity '{entity_id}' candidate '{}': {:?} vs {val:?} \
                                 (propose.value must be a pure function of entity identity; \
                                 see reference.rs module docs)",
                                decide.name, propose.name, existing.value
                            )));
                        }
                        existing.support_count += 1;
                    }
                    None => {
                        let phase = 0u64;
                        let calendar_key = Key::new(
                            phase,
                            propose.priority,
                            candidate_tiebreak_digest(
                                &decide.name,
                                &entity_id,
                                &propose.name,
                                &val,
                            ),
                        );
                        entity_map.insert(
                            propose.name.clone(),
                            ReferenceCandidate {
                                candidate_name: propose.name.clone(),
                                entity_id: entity_id.clone(),
                                priority: propose.priority,
                                phase,
                                value: val,
                                calendar_key,
                                support_count: 1,
                            },
                        );
                    }
                }
            }
        }

        let mut decide_settlements = BTreeMap::new();
        for (entity_id, candidates) in &entities {
            if let Some(winner) = candidates
                .values()
                .filter(|c| c.support_count > 0)
                .min_by_key(|c| c.calendar_key)
            {
                meter.settlements_computed += 1;
                charge_work(meter, work_offset, max_work)?;
                decide_settlements.insert(
                    entity_id.clone(),
                    ReferenceSettlement {
                        entity_id: winner.entity_id.clone(),
                        candidate_name: winner.candidate_name.clone(),
                        priority: winner.priority,
                        phase: winner.phase,
                        value: winner.value.clone(),
                        calendar_key: winner.calendar_key,
                    },
                );
            }
        }

        if !entities.is_empty() {
            candidate_frontier.insert(decide.name.clone(), entities);
        }
        if !decide_settlements.is_empty() {
            settlements.insert(decide.name.clone(), decide_settlements);
        }
    }

    Ok(ReferenceState {
        base_relations: base_relations.clone(),
        derived_relations,
        candidate_frontier,
        settlements,
    })
}

/// Evaluate a resolved program against a full base-relation snapshot by plain set semantics,
/// with no incremental state of any kind. Convenience wrapper over [`evaluate_with_budget`].
pub fn evaluate(
    program: &ReferenceProgram,
    base_relations: &BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>,
) -> Result<ReferenceState, WorldError> {
    let mut meter = ReferenceWorkMeter::default();
    evaluate_with_budget(program, base_relations, &mut meter, None, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(fields: &[(&str, Value)]) -> Row {
        fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// `extract_entity_id` reads exactly the declared `per` field — bare or
    /// binder-qualified — and ignores every other id-shaped field on the row.
    #[test]
    fn entity_id_reads_declared_field_only() {
        let eval_row = row(&[
            ("entity_id", Value::Str("E1".into())),
            ("order_id", Value::Str("O1".into())),
            ("id", Value::Str("I1".into())),
        ]);
        assert_eq!(
            extract_entity_id(&eval_row, "f", &["order_id".to_string()]).unwrap(),
            "O1",
            "declared field wins regardless of naming-convention precedence"
        );
        assert_eq!(
            extract_entity_id(&eval_row, "f", &["id".to_string()]).unwrap(),
            "I1"
        );
    }

    /// The declared field may be read either bare or binder-qualified
    /// (`get_qualified`'s existing convention), matching how a bound row
    /// carries both spellings of every field.
    #[test]
    fn entity_id_reads_binder_qualified_field() {
        let eval_row = row(&[("f.entity_id", Value::Str("E1".into()))]);
        assert_eq!(
            extract_entity_id(&eval_row, "f", &["entity_id".to_string()]).unwrap(),
            "E1"
        );
    }

    /// A row missing the declared field is a typed error, never a silent
    /// default (contrast the retired heuristic's `"default_entity"` fallback).
    #[test]
    fn entity_id_missing_declared_field_is_a_typed_error() {
        let eval_row = row(&[("other_field", Value::Str("zz".into()))]);
        let err = extract_entity_id(&eval_row, "f", &["entity_id".to_string()]).unwrap_err();
        assert!(
            matches!(err, WorldError::NetworkError(_)),
            "missing declared entity field must be a typed WorldError, not a default: {err:?}"
        );
    }
}
