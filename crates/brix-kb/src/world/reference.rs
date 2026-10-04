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
//!    lower_relations}`** — the lowered operator DAG and its node/AST types. This is the
//!    *specification* of which operators exist and how they are wired, not an evaluation
//!    strategy. Both engines are handed the same DAG and are free to evaluate it however they
//!    like; sharing the DAG's shape is sharing the problem statement, not the solution.
//! 2. **`brix_syntax::ast`** (`Expr`, `Ty`, `ProposeDecl`, `Callable`, …) and
//!    **`brix_lower::module_graph::LinkedProgram`** — the surface AST and linked-program
//!    container. Same rationale: these are the program *text*, not an engine.
//! 3. **The tuple codec** (`crate::world::codec::TupleRecord`, `crate::world::types::{WorldKey,
//!    WorldTuple}`) — a deterministic, versioned *serialization format*, not relational logic.
//!    A bug here would corrupt both oracles identically, but the codec's correctness is
//!    covered separately by `tests/world_tuple_codec.rs`, and this module's job is to catch
//!    *relational/decision* bugs, not byte-codec bugs.
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
//! ## The entity-id extraction rule is an unspecified heuristic, treated as a frozen spec
//!
//! `network.rs::extract_entity_id` (the function that decides which field of a decide
//! block's bound row names the entity being decided about) is **not specified by any ADR
//! text** — it is a heuristic fallback chain: explicit `propose(deps...)` dependency, then
//! `entity_id`/`{binder}.entity_id`, then `order_id`/`{binder}.order_id`, then `id`/`{binder}.id`,
//! then `key`/`{binder}.key`, then the first field whose name ends in `.id` or `_id`
//! (alphabetically first, since the row is a `BTreeMap`), then the alphabetically-first field
//! of any name, then the literal string `"default_entity"`. Because it is unspecified rather
//! than documented, this module cannot derive it independently from an ADR; instead
//! [`extract_entity_id`] below is a from-scratch re-implementation of that exact fallback
//! chain, written by reading `network.rs`'s current behavior and copying the *rule*, not the
//! code. It lives in one small, clearly-labeled, easily-swappable function precisely because
//! the rule is expected to be frozen/formalized in a later P6 PR — when that happens, only
//! this one function needs to change. `tests/world_reference_differential.rs` includes a
//! dedicated case (`entity_id_fallback_precedence`) exercising a row carrying both `entity_id`
//! and `order_id` with different values, to pin down today's precedence order.
//!
//! ## One behavioral assumption: propose values are a pure function of entity identity
//!
//! `network.rs` records a candidate's `value` only on the **first** support it sees for a
//! given `(decide, entity, candidate_name)`; a later distinct supporting row with a
//! *different* value is silently ignored (the existing `CandidateEntry.value` is never
//! revisited). Because that "first" is whichever support arrives first in **incremental
//! batch-application order** — and this module instead evaluates every distinct supporting
//! row of a full snapshot with no notion of arrival order — there is no order-independent way
//! to replicate that specific tie-break. Rather than silently picking an arbitrary winner
//! (which could paper over a real bug), [`evaluate`] treats "every distinct supporting row
//! agrees on the proposed value" as an invariant and returns a [`WorldError::NetworkError`]
//! naming the conflicting values if it is ever violated. This has not been observed to fire in
//! the differential suite's generators (which deliberately keep a propose's `value` expression
//! a function of the bound entity, not of incidental per-row fields), and is flagged here as a
//! known order-dependence risk in `network.rs` rather than something this oracle papers over.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_lower::module_graph::{LinkedProgram, QualifiedName};
use brix_lower::relation_dag::{
    lower_relations, GroupProjection, OperatorId, OperatorNode, RelationDag,
};
use brix_lower::world_expr::CompiledWorldExpr;
use brix_syntax::ast::{self, Expr};
use soc_core::calendar::Key;

use super::codec::TupleRecord;
use super::error::WorldError;
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
}

impl Value {
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
    fn from_typed_bytes(bytes: &[u8], ty: &ast::Ty) -> Result<Self, WorldError> {
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

    fn to_bytes(&self) -> Vec<u8> {
        self.to_string().into_bytes()
    }

    fn to_scalar(&self) -> Result<brix_lower::l3_v2::L3ValueV2, WorldError> {
        use brix_lower::l3_v2::L3ValueV2 as V;
        Ok(match self {
            Self::Bool(v) => V::Bool(*v),
            Self::Int(v) => V::Int(*v),
            Self::Str(v) => V::Str(v.clone()),
            Self::F64(v) => V::F64(*v),
            Self::Decimal(v) => V::Decimal(*v),
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
            _ => {
                return Err(WorldError::NetworkError(
                    "reference evaluator: expression result is not a scalar".into(),
                ))
            }
        })
    }

    /// Canonical write, reproduced from `network.rs`'s `canon_write_value` byte-for-byte
    /// (see module docs point 6: the calendar tie-break formula is a shared spec contract).
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
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
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

/// A row flowing between operators: a plain field-name -> value map, with no derivation or
/// provenance tracking of any kind (contrast `network::IntermediateTuple`, which exists only
/// as a vehicle for `network.rs`'s derivation ids).
pub type Row = BTreeMap<String, Value>;

fn get_qualified<'a>(row: &'a Row, binding: &str, field: &str) -> Option<&'a Value> {
    row.get(&format!("{binding}.{field}"))
        .or_else(|| row.get(field))
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

fn build_grouped_row(
    projections: &[(String, GroupProjection)],
    key_vals: &[Value],
    count: usize,
) -> Row {
    let mut row = Row::new();
    for (name, proj) in projections {
        match proj {
            GroupProjection::Key(idx) => {
                let v = key_vals.get(*idx).cloned().unwrap_or(Value::Null);
                row.insert(name.clone(), v);
            }
            GroupProjection::Count => {
                row.insert(name.clone(), Value::Int(count as i64));
            }
        }
    }
    row
}

/// The unspecified `network.rs::extract_entity_id` heuristic, re-implemented from scratch
/// (see module docs). Kept isolated in this one function so it is trivial to swap when the
/// rule is formalized in a later P6 PR.
fn extract_entity_id(eval_row: &Row, binder: &str, propose: &ast::ProposeDecl) -> String {
    if !propose.deps.is_empty() {
        let dep = &propose.deps[0];
        if let Some(v) = eval_row.get(dep).or_else(|| {
            dep.split_once('.')
                .and_then(|(_, field)| eval_row.get(field))
        }) {
            return v.to_string();
        }
    }

    let candidates = [
        "entity_id".to_string(),
        format!("{binder}.entity_id"),
        "order_id".to_string(),
        format!("{binder}.order_id"),
        "id".to_string(),
        format!("{binder}.id"),
        "key".to_string(),
        format!("{binder}.key"),
    ];
    for c in &candidates {
        if let Some(v) = eval_row.get(c) {
            return v.to_string();
        }
    }

    if let Some((_, v)) = eval_row
        .iter()
        .find(|(k, _)| k.ends_with(".id") || k.ends_with("_id"))
    {
        return v.to_string();
    }

    if let Some((_, v)) = eval_row.iter().next() {
        return v.to_string();
    }

    "default_entity".to_string()
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

/// A compiled decide block, independently defined from `network::DecideBlock` (same four
/// fields, separate type — see module docs).
#[derive(Clone, Debug)]
pub struct DecideSpec {
    pub name: String,
    pub binder: String,
    pub source_relation: String,
    pub proposals: Vec<ast::ProposeDecl>,
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

/// A resolved program ready for [`evaluate`]: the lowered DAG, extracted decide specs, and
/// the flat helper-function table. Independently assembled from a [`LinkedProgram`] by
/// [`from_program`] — the only genuinely shared step is [`lower_relations`] itself (see
/// module docs point 1).
#[derive(Clone, Debug)]
pub struct ReferenceProgram {
    pub dag: RelationDag,
    pub decides: Vec<DecideSpec>,
    pub functions: BTreeMap<String, ast::Callable>,
}

/// Resolve a [`LinkedProgram`] into a [`ReferenceProgram`]. Independently re-implements the
/// small glue `network::WorldNetwork::from_program` performs around the shared
/// `lower_relations` call: resolving named row schemas to their linked record config, and
/// extracting `decide` blocks' source relation from their `list` expression.
pub fn from_program(program: &LinkedProgram) -> Result<ReferenceProgram, WorldError> {
    let mut dag = lower_relations(program)
        .map_err(|e| WorldError::NetworkError(format!("relational lowering: {e}")))?;

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
                let config = program.configs.get(&qname).ok_or_else(|| {
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

    let mut decides = Vec::new();
    for (qname, decl) in &program.decides {
        let mut source_relation = None;
        if let Expr::Var(v) = &decl.list {
            if dag.relation_outputs.contains_key(v) {
                source_relation = Some(v.clone());
            } else {
                let qualified = format!("{}::{v}", program.root_module);
                if dag.relation_outputs.contains_key(&qualified) {
                    source_relation = Some(qualified);
                } else if let Some(found) = dag
                    .relation_outputs
                    .keys()
                    .find(|k| k.ends_with(&format!("::{v}")))
                {
                    source_relation = Some(found.clone());
                }
            }
        }

        if let Some(src) = source_relation {
            decides.push(DecideSpec {
                name: qname.to_string(),
                binder: decl.binder.clone(),
                source_relation: src,
                proposals: decl.proposals.clone(),
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
    })
}

/// Evaluate a resolved program against a full base-relation snapshot by plain set semantics,
/// with no incremental state of any kind. See module docs for exactly what is and is not
/// independent of `network.rs`.
pub fn evaluate(
    program: &ReferenceProgram,
    base_relations: &BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>,
) -> Result<ReferenceState, WorldError> {
    let dag = &program.dag;
    let mut operator_outputs: Vec<Vec<Row>> = Vec::with_capacity(dag.nodes.len());

    for node in &dag.nodes {
        let rows = match node {
            OperatorNode::Scan {
                relation, schema, ..
            } => {
                let ast::Ty::Record(fields) = schema else {
                    return Err(WorldError::NetworkError(format!(
                        "unresolved row schema for {relation}"
                    )));
                };
                let empty = BTreeMap::new();
                let records = base_relations.get(relation).unwrap_or(&empty);
                let mut rows = Vec::with_capacity(records.len());
                for tuple in records.values() {
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
                        row.insert(
                            field.name.clone(),
                            Value::from_typed_bytes(bytes, &field.ty)?,
                        );
                    }
                    rows.push(row);
                }
                rows
            }
            OperatorNode::Bind { input, alias } => operator_outputs[input.0]
                .iter()
                .map(|row| {
                    let mut bound = Row::new();
                    for (k, v) in row {
                        bound.insert(format!("{alias}.{k}"), v.clone());
                        bound.insert(k.clone(), v.clone());
                    }
                    bound
                })
                .collect(),
            OperatorNode::Filter { input, predicate } => {
                let mut out = Vec::new();
                for row in &operator_outputs[input.0] {
                    if eval_expr(predicate, row, &program.functions)?.as_bool()? {
                        out.push(row.clone());
                    }
                }
                out
            }
            OperatorNode::Project { input, projections } => {
                let mut out = Vec::with_capacity(operator_outputs[input.0].len());
                for row in &operator_outputs[input.0] {
                    let mut projected = Row::new();
                    for (name, expr) in projections {
                        projected.insert(name.clone(), eval_expr(expr, row, &program.functions)?);
                    }
                    out.push(projected);
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
                let mut out = Vec::new();
                for l in left_rows {
                    let lkey: Vec<Value> = left_keys
                        .iter()
                        .map(|k| {
                            get_qualified(l, &k.binding, &k.field)
                                .cloned()
                                .ok_or_else(|| {
                                    WorldError::NetworkError(format!(
                                        "missing join field {}.{}",
                                        k.binding, k.field
                                    ))
                                })
                        })
                        .collect::<Result<_, _>>()?;
                    for r in right_rows {
                        let rkey: Vec<Value> = right_keys
                            .iter()
                            .map(|k| {
                                get_qualified(r, &k.binding, &k.field)
                                    .cloned()
                                    .ok_or_else(|| {
                                        WorldError::NetworkError(format!(
                                            "missing join field {}.{}",
                                            k.binding, k.field
                                        ))
                                    })
                            })
                            .collect::<Result<_, _>>()?;
                        if lkey == rkey {
                            out.push(merge_rows(l, r));
                        }
                    }
                }
                out
            }
            OperatorNode::Distinct { input } => {
                let set: BTreeSet<Row> = operator_outputs[input.0].iter().cloned().collect();
                set.into_iter().collect()
            }
            OperatorNode::GroupedCount {
                input,
                group_keys,
                projections,
            } => {
                let mut groups: BTreeMap<Vec<Value>, usize> = BTreeMap::new();
                for row in &operator_outputs[input.0] {
                    let key: Vec<Value> = group_keys
                        .iter()
                        .map(|e| eval_expr(e, row, &program.functions))
                        .collect::<Result<_, _>>()?;
                    *groups.entry(key).or_insert(0) += 1;
                }
                groups
                    .into_iter()
                    .map(|(key, count)| build_grouped_row(projections, &key, count))
                    .collect()
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
            .map(row_to_tuple_record)
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
            let mut eval_row = row.clone();
            for (k, v) in row {
                eval_row.insert(format!("{}.{k}", decide.binder), v.clone());
            }

            for propose in &decide.proposals {
                let entity_id = extract_entity_id(&eval_row, &decide.binder, propose);
                let guard_passed =
                    eval_expr(&propose.guard, &eval_row, &program.functions)?.as_bool()?;
                if !guard_passed {
                    continue;
                }
                let val = eval_expr(&propose.value, &eval_row, &program.functions)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(fields: &[(&str, Value)]) -> Row {
        fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn dummy_propose(name: &str) -> ast::ProposeDecl {
        ast::ProposeDecl {
            name: name.to_string(),
            deps: Vec::new(),
            priority: 10,
            guard: ast::Expr::Bool(true),
            value: ast::Expr::Bool(true),
            deps_declared: false,
            otherwise: false,
        }
    }

    /// `extract_entity_id` must prefer `entity_id` over `order_id`/`id`/`key` and over the
    /// alphabetically-first `*_id`/`.id`-suffixed field, when more than one is present on the
    /// same row with *different* values — pinning down today's (unspecified-by-ADR) fallback
    /// precedence per the coordinator note on this task.
    #[test]
    fn entity_id_fallback_precedence() {
        let eval_row = row(&[
            ("entity_id", Value::Str("E1".into())),
            ("order_id", Value::Str("O1".into())),
            ("id", Value::Str("I1".into())),
            ("other_field", Value::Str("zz".into())),
        ]);
        let propose = dummy_propose("p");
        assert_eq!(extract_entity_id(&eval_row, "f", &propose), "E1");
    }

    /// With no `entity_id`, `order_id` must win over `id`/`key`/other `*_id` fields.
    #[test]
    fn entity_id_fallback_order_id_before_id() {
        let eval_row = row(&[
            ("order_id", Value::Str("O1".into())),
            ("id", Value::Str("I1".into())),
            ("sku_id", Value::Str("S1".into())),
        ]);
        let propose = dummy_propose("p");
        assert_eq!(extract_entity_id(&eval_row, "f", &propose), "O1");
    }

    /// With none of the named candidates present, the alphabetically-first `*_id`/`.id`
    /// field wins (BTreeMap iteration order).
    #[test]
    fn entity_id_fallback_any_id_suffixed_field() {
        let eval_row = row(&[
            ("zz_id", Value::Str("Z1".into())),
            ("aa_id", Value::Str("A1".into())),
            ("other", Value::Str("not it".into())),
        ]);
        let propose = dummy_propose("p");
        assert_eq!(extract_entity_id(&eval_row, "f", &propose), "A1");
    }

    /// With nothing id-shaped at all, the alphabetically-first field of any name wins.
    #[test]
    fn entity_id_fallback_first_field() {
        let eval_row = row(&[
            ("zebra", Value::Str("Z".into())),
            ("apple", Value::Str("A".into())),
        ]);
        let propose = dummy_propose("p");
        assert_eq!(extract_entity_id(&eval_row, "f", &propose), "A");
    }

    /// An explicit `propose(dep)` dependency always wins over every heuristic fallback.
    #[test]
    fn entity_id_explicit_dep_wins() {
        let eval_row = row(&[
            ("entity_id", Value::Str("E1".into())),
            ("custom_key", Value::Str("C1".into())),
        ]);
        let mut propose = dummy_propose("p");
        propose.deps = vec!["custom_key".to_string()];
        propose.deps_declared = true;
        assert_eq!(extract_entity_id(&eval_row, "f", &propose), "C1");
    }
}
