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

/// Extract a row's entity identity from its declared `per <field>`
/// (ADR-0046, decided 2026-10-04), independently re-derived from `network.rs`'s
/// `entity_id_for` by reading the same declared field, not by calling its
/// code (see module docs). A row missing the declared field is a typed error,
/// never a silent default.
fn extract_entity_id(eval_row: &Row, binder: &str, per_field: &str) -> Result<String, WorldError> {
    get_qualified(eval_row, binder, per_field)
        .map(|v| v.to_string())
        .ok_or_else(|| {
            WorldError::NetworkError(format!(
                "reference evaluator: decide entity field '{per_field}' missing from bound row \
                 (binder '{binder}')"
            ))
        })
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
    /// Declared entity-identity field (ADR-0046, decided 2026-10-04); see
    /// `network::DecideBlock::per_field`.
    pub per_field: String,
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
    Distinct,
    GroupedCount {
        compiled_group_keys: Vec<CompiledWorldExpr>,
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

    let program_env =
        CompiledProgramEnv::from_linked_program(program).map_err(WorldError::NetworkError)?;
    let op_schemas = decision_field_schemas(&dag, program);

    let mut compiled_operators = Vec::with_capacity(dag.nodes.len());
    for node in &dag.nodes {
        match node {
            OperatorNode::Scan { .. } => compiled_operators.push(CompiledOperator::Scan),
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
            OperatorNode::Distinct { .. } => compiled_operators.push(CompiledOperator::Distinct),
            OperatorNode::GroupedCount {
                input, group_keys, ..
            } => {
                let bindings = derive_binding_names(&op_schemas[input.0]);
                let mut compiled_group_keys = Vec::with_capacity(group_keys.len());
                for expr in group_keys {
                    let compiled = program_env
                        .compile_expr(expr, &bindings)
                        .map_err(WorldError::NetworkError)?;
                    compiled_group_keys.push(compiled);
                }
                compiled_operators.push(CompiledOperator::GroupedCount {
                    compiled_group_keys,
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
            let per_field = decl.per.clone().ok_or_else(|| {
                WorldError::NetworkError(format!(
                    "reference evaluator: decide '{qname}' resolved to relation '{src}' with no \
                     'per' field (expected lower_relations to have refused this)"
                ))
            })?;
            let op_id = dag.relation_outputs.get(&src).ok_or_else(|| {
                WorldError::NetworkError(format!(
                    "decide '{qname}' unknown source relation '{src}'"
                ))
            })?;
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
    })
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
    let dag = &program.dag;
    let mut operator_outputs: Vec<Vec<Row>> = Vec::with_capacity(dag.nodes.len());

    for (idx, node) in dag.nodes.iter().enumerate() {
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
                        row.insert(
                            field.name.clone(),
                            Value::from_typed_bytes(bytes, &field.ty)?,
                        );
                    }
                    rows.push(row);
                }
                rows
            }
            OperatorNode::Bind { input, alias } => {
                let mut bound_rows = Vec::with_capacity(operator_outputs[input.0].len());
                for row in &operator_outputs[input.0] {
                    meter.rows_evaluated += 1;
                    charge_work(meter, work_offset, max_work)?;
                    let mut bound = Row::new();
                    for (k, v) in row {
                        bound.insert(format!("{alias}.{k}"), v.clone());
                        bound.insert(k.clone(), v.clone());
                    }
                    bound_rows.push(bound);
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
                    let bindings = scalar_bindings(row)?;
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
                    let bindings = scalar_bindings(row)?;
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

                let mut right_index: BTreeMap<Vec<Value>, Vec<&Row>> = BTreeMap::new();
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
                    right_index.entry(rkey).or_default().push(r);
                }

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
                    if let Some(matches) = right_index.get(&lkey) {
                        for r in matches {
                            meter.join_pairs_evaluated += 1;
                            charge_work(meter, work_offset, max_work)?;
                            out.push(merge_rows(l, r));
                        }
                    }
                }
                out
            }
            OperatorNode::Distinct { input } => {
                let mut set: BTreeSet<Row> = BTreeSet::new();
                for row in &operator_outputs[input.0] {
                    meter.rows_evaluated += 1;
                    charge_work(meter, work_offset, max_work)?;
                    set.insert(row.clone());
                }
                set.into_iter().collect()
            }
            OperatorNode::GroupedCount {
                input,
                group_keys: _,
                projections,
            } => {
                let compiled_keys = match &program.compiled_operators[idx] {
                    CompiledOperator::GroupedCount {
                        compiled_group_keys,
                    } => compiled_group_keys,
                    _ => unreachable!(),
                };
                let mut groups: BTreeMap<Vec<Value>, usize> = BTreeMap::new();
                for row in &operator_outputs[input.0] {
                    meter.rows_evaluated += 1;
                    charge_work(meter, work_offset, max_work)?;
                    let bindings = scalar_bindings(row)?;
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

            for (p_idx, propose) in decide.proposals.iter().enumerate() {
                let compiled_prop = &decide.compiled_proposals[p_idx];
                meter.candidates_evaluated += 1;
                charge_work(meter, work_offset, max_work)?;
                let entity_id = extract_entity_id(&eval_row, &decide.binder, &decide.per_field)?;
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
            extract_entity_id(&eval_row, "f", "order_id").unwrap(),
            "O1",
            "declared field wins regardless of naming-convention precedence"
        );
        assert_eq!(extract_entity_id(&eval_row, "f", "id").unwrap(), "I1");
    }

    /// The declared field may be read either bare or binder-qualified
    /// (`get_qualified`'s existing convention), matching how a bound row
    /// carries both spellings of every field.
    #[test]
    fn entity_id_reads_binder_qualified_field() {
        let eval_row = row(&[("f.entity_id", Value::Str("E1".into()))]);
        assert_eq!(
            extract_entity_id(&eval_row, "f", "entity_id").unwrap(),
            "E1"
        );
    }

    /// A row missing the declared field is a typed error, never a silent
    /// default (contrast the retired heuristic's `"default_entity"` fallback).
    #[test]
    fn entity_id_missing_declared_field_is_a_typed_error() {
        let eval_row = row(&[("other_field", Value::Str("zz".into()))]);
        let err = extract_entity_id(&eval_row, "f", "entity_id").unwrap_err();
        assert!(
            matches!(err, WorldError::NetworkError(_)),
            "missing declared entity field must be a typed WorldError, not a default: {err:?}"
        );
    }
}
