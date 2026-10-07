//! Maintained operator network, truth maintenance, and candidate frontier deliberation (ADR-0046 P4).
//!
//! Provides [`WorldNetwork`], which compiles relational DAGs into an incrementally maintained
//! operator network with:
//! - Exact $O(\Delta)$ propagation across [`OperatorNode::Scan`], [`OperatorNode::Bind`],
//!   [`OperatorNode::Filter`], [`OperatorNode::Project`], [`OperatorNode::EquiJoin`],
//!   [`OperatorNode::Distinct`], and [`OperatorNode::GroupedCount`].
//! - Symmetric indexed joins with derivation support tracking.
//! - Set semantics with derivation support counters ($0 \to 1$ emits $+$, $1 \to 0$ emits $-$, $>1$ tracks multiple supports).
//! - Monotonic group count tracking with $0 \to 1$, count update, and $1 \to 0$ transitions.
//! - Candidate frontier maintenance per entity with multi-support survival.
//! - Canonical settlement discipline selecting least `Key = (phase, priority, tiebreak)` from [`soc_core::calendar`].
//! - Replay comparison ([`WorldNetwork::recompute_from_scratch`]); independent audit is a separate implementation.

#![deny(unsafe_code)]

use super::persistent::{PMap, PSet};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_lower::module_graph::LinkedProgram;
use brix_lower::relation_dag::{
    decision_field_schemas, derive_binding_names, derive_decide_binding_names, lower_relations,
    resolve_decide_source_relation, FieldRef, GroupProjection, OperatorId, OperatorNode,
    RelationDag,
};
use brix_lower::world_expr::{CompiledProgramEnv, CompiledWorldExpr};
use brix_syntax::ast;
use soc_core::calendar::{Frontier, Key};
use soc_core::store::TrieMap;

use super::decision_codec::canon_write_value;
pub use super::decision_codec::{
    compute_decision_root, decision_key, decision_tuple, decode_settled_decision,
    encode_decision_delta, SettledDecision, Value,
};

use super::batch::{WorldBatch, WorldBatchOp};
use super::codec::TupleRecord;
use super::error::WorldError;
use super::types::{WorldKey, WorldTuple};

/// An intermediate tuple carrying evaluated fields in operator pipeline execution.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct IntermediateTuple {
    pub fields: BTreeMap<String, Value>,
}

impl IntermediateTuple {
    pub fn new() -> Self {
        Self {
            fields: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, key: impl Into<String>, val: Value) {
        self.fields.insert(key.into(), val);
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    pub fn get_qualified(&self, binding: &str, field: &str) -> Option<&Value> {
        let qualified = format!("{binding}.{field}");
        self.fields
            .get(&qualified)
            .or_else(|| self.fields.get(field))
    }

    pub fn from_tuple_record(rec: &TupleRecord) -> Self {
        let mut fields = BTreeMap::new();
        for (k, v) in &rec.fields {
            fields.insert(k.clone(), Value::from_bytes(v));
        }
        Self { fields }
    }

    pub fn to_tuple_record(&self) -> TupleRecord {
        let mut rec = TupleRecord::new();
        for (k, v) in &self.fields {
            rec.set(k.clone(), v.to_bytes());
        }
        rec
    }

    pub fn merge(&self, other: &IntermediateTuple) -> IntermediateTuple {
        let mut merged = self.fields.clone();
        for (k, v) in &other.fields {
            merged.insert(k.clone(), v.clone());
        }
        IntermediateTuple { fields: merged }
    }
}

/// A derivation identifier uniquely tracking justifications for TMS truth maintenance.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DerivationId {
    /// Base fact in a relation identified by primary key.
    Base { relation: String, key: WorldKey },
    /// Fact derived through a unary operator node.
    Unary {
        op: OperatorId,
        parent: Box<DerivationId>,
    },
    /// Composite fact derived by joining left and right supports in an EquiJoin.
    Join {
        op: OperatorId,
        left: Box<DerivationId>,
        right: Box<DerivationId>,
    },
    /// Aggregation derivation for a group.
    Group {
        op: OperatorId,
        group_key: Vec<Value>,
    },
    /// Canonical derivation for a distinct tuple produced by set semantics.
    Distinct { op: OperatorId, tuple_key: Vec<u8> },
}

impl DerivationId {
    pub fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Self::Base { relation, key } => {
                w.write_uint(1);
                w.write_str(relation);
                key.canon_write(w);
            }
            Self::Unary { op, parent } => {
                w.write_uint(2);
                w.write_uint(op.0 as u64);
                parent.canon_write(w);
            }
            Self::Join { op, left, right } => {
                w.write_uint(3);
                w.write_uint(op.0 as u64);
                left.canon_write(w);
                right.canon_write(w);
            }
            Self::Group { op, group_key } => {
                w.write_uint(4);
                w.write_uint(op.0 as u64);
                w.write_uint(group_key.len() as u64);
                for v in group_key {
                    w.write_bytes(&v.to_bytes());
                }
            }
            Self::Distinct { op, tuple_key } => {
                w.write_uint(5);
                w.write_uint(op.0 as u64);
                w.write_bytes(tuple_key);
            }
        }
    }

    pub fn digest(&self) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag("brix.derivation@1");
        self.canon_write(&mut w);
        w.digest(Domain::Value)
    }

    /// Recursively collect all contributing base facts (relation, primary key).
    pub fn collect_base_facts(&self, out: &mut BTreeSet<(String, WorldKey)>) {
        match self {
            Self::Base { relation, key } => {
                out.insert((relation.clone(), key.clone()));
            }
            Self::Unary { parent, .. } => {
                parent.collect_base_facts(out);
            }
            Self::Join { left, right, .. } => {
                left.collect_base_facts(out);
                right.collect_base_facts(out);
            }
            Self::Group { .. } | Self::Distinct { .. } => {}
        }
    }
}

/// Incremental delta emitted between operator nodes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TupleDelta {
    Insert {
        tuple: IntermediateTuple,
        derivation: DerivationId,
    },
    Retract {
        tuple: IntermediateTuple,
        derivation: DerivationId,
    },
}

impl TupleDelta {
    pub fn tuple(&self) -> &IntermediateTuple {
        match self {
            Self::Insert { tuple, .. } | Self::Retract { tuple, .. } => tuple,
        }
    }

    pub fn derivation(&self) -> &DerivationId {
        match self {
            Self::Insert { derivation, .. } | Self::Retract { derivation, .. } => derivation,
        }
    }

    pub fn is_insert(&self) -> bool {
        matches!(self, Self::Insert { .. })
    }
}

/// Default scheduling quantum: number of work units / matches an operator processes before yielding.
pub const DEFAULT_SCHEDULING_QUANTUM: usize = 256;

/// An item of work queued for an operator in the network.
#[derive(Clone, Debug)]
pub enum OperatorWorkItem {
    Delta(TupleDelta),
    JoinContinuation {
        delta: TupleDelta,
        is_left: bool,
        join_key: Vec<Value>,
        matches: PMap<DerivationId, IntermediateTuple>,
        next_idx: usize,
    },
}

#[inline]
fn work_item_queued_count(item: &OperatorWorkItem) -> usize {
    match item {
        OperatorWorkItem::Delta(_) => 1,
        OperatorWorkItem::JoinContinuation {
            matches, next_idx, ..
        } => matches.len().saturating_sub(*next_idx).max(1),
    }
}

/// Work and resource limits bounding relational operator execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct NetworkLimits {
    pub max_work: Option<u64>,
    pub max_matches: Option<u64>,
    pub max_expressions: Option<u64>,
    pub max_queued_deltas: Option<usize>,
}

/// Execution, skew, and fanout diagnostics across the operator network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkDiagnostics {
    pub operator_work: BTreeMap<OperatorId, u64>,
    pub relation_deltas: BTreeMap<String, u64>,
    pub join_fanout: BTreeMap<OperatorId, u64>,
    pub hot_join_keys: BTreeMap<String, u64>,
    pub peak_queue_size: usize,
    pub expressions_evaluated: u64,
}

impl NetworkDiagnostics {
    pub fn expensive_relation(&self) -> Option<(&str, u64)> {
        self.relation_deltas
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(rel, &count)| (rel.as_str(), count))
    }

    pub fn expensive_operator(&self) -> Option<(OperatorId, u64)> {
        self.operator_work
            .iter()
            .max_by_key(|(_, &work)| work)
            .map(|(&op, &work)| (op, work))
    }

    pub fn hot_join_key(&self) -> Option<(&str, u64)> {
        self.hot_join_keys
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(key, &count)| (key.as_str(), count))
    }
}

/// A proposed candidate entry within a per-entity decide block.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CandidateEntry {
    pub candidate_name: String,
    pub entity_id: String,
    pub priority: u64,
    pub phase: u64,
    pub value: Value,
    pub calendar_key: Key,
    pub supports: PSet<DerivationId>,
}

/// Explanation of an individual candidate in a decide deliberation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CandidateExplanation {
    pub name: String,
    pub priority: u64,
    pub phase: u64,
    pub value: Value,
    pub supports_count: usize,
    pub winning: bool,
}

impl CandidateExplanation {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "priority": self.priority,
            "phase": self.phase,
            "value": self.value.to_string(),
            "supports_count": self.supports_count,
            "winning": self.winning,
        })
    }
}

/// Comprehensive explanation of the candidate deliberation and winning settlement for an entity.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecisionExplanation {
    pub entity_id: String,
    pub decide_name: String,
    pub winning_candidate: Option<String>,
    pub value: Option<Value>,
    pub priority: Option<u64>,
    pub phase: Option<u64>,
    pub calendar_key: Option<Key>,
    pub candidates: Vec<CandidateExplanation>,
    pub contributing_facts: Vec<(String, WorldKey)>,
}

impl DecisionExplanation {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "entity_id": self.entity_id,
            "decide_name": self.decide_name,
            "winning_candidate": self.winning_candidate,
            "value": self.value.as_ref().map(|v| v.to_string()),
            "priority": self.priority,
            "phase": self.phase,
            "calendar_key": self.calendar_key.map(|k| format!("phase={},priority={},tiebreak={}", k.phase, k.priority, k.tiebreak.to_hex())),
            "candidates": self.candidates.iter().map(|c| c.to_json()).collect::<Vec<_>>(),
            "contributing_facts": self.contributing_facts.iter().map(|(rel, key)| {
                let key_repr = std::str::from_utf8(key.as_bytes()).map(|s| s.to_string()).unwrap_or_else(|_| key.to_hex());
                serde_json::json!({
                    "relation": rel,
                    "key": key_repr,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

/// A precompiled proposal entry within a decide block.
#[derive(Clone, Debug)]
pub struct CompiledPropose {
    pub name: String,
    pub priority: u64,
    pub guard: CompiledWorldExpr,
    pub value: CompiledWorldExpr,
}

/// A compiled decide block scoping candidates to individual entities.
#[derive(Clone, Debug)]
pub struct DecideBlock {
    pub name: String,
    pub binder: String,
    pub source_relation: String,
    /// Declared entity-identity field (ADR-0046, decided 2026-10-04): the
    /// field of the bound row (qualified `binder.field` or bare `field`)
    /// whose value names the entity this instance of the block decides about.
    /// `lower_relations` refuses any world-profile `decide` lacking this, so
    /// by the time a `DecideBlock` exists it is always present.
    pub per_field: String,
    pub proposals: Vec<ast::ProposeDecl>,
    pub compiled_proposals: Vec<CompiledPropose>,
}

/// Compute a deterministic canonical Blake3 calendar key for a candidate.
pub fn compute_candidate_calendar_key(
    phase: u64,
    priority: u64,
    decide_name: &str,
    entity_id: &str,
    candidate_name: &str,
    value: &Value,
) -> Key {
    let mut w = CanonWriter::new();
    w.write_tag("brix.candidate.tiebreak@1");
    w.write_str(decide_name);
    w.write_str(entity_id);
    w.write_str(candidate_name);
    canon_write_value(value, &mut w);
    let tiebreak = w.digest(Domain::Value);
    Key::new(phase, priority, tiebreak)
}

#[derive(Clone, Debug)]
pub enum OperatorState {
    Scan {
        relation: String,
        key_fields: Vec<String>,
        schema: ast::Ty,
        records: PMap<WorldKey, (WorldTuple, IntermediateTuple)>,
    },
    Bind {
        input: OperatorId,
        alias: String,
    },
    Filter {
        input: OperatorId,
        predicate: ast::Expr,
        compiled_predicate: Option<CompiledWorldExpr>,
    },
    Project {
        input: OperatorId,
        projections: Vec<(String, ast::Expr)>,
        compiled_projections: Vec<(String, CompiledWorldExpr)>,
    },
    EquiJoin {
        left: OperatorId,
        right: OperatorId,
        left_keys: Vec<FieldRef>,
        right_keys: Vec<FieldRef>,
        left_index: PMap<Vec<Value>, PMap<DerivationId, IntermediateTuple>>,
        right_index: PMap<Vec<Value>, PMap<DerivationId, IntermediateTuple>>,
    },
    Distinct {
        input: OperatorId,
        supports: PMap<IntermediateTuple, PSet<DerivationId>>,
    },
    GroupedCount {
        input: OperatorId,
        group_keys: Vec<ast::Expr>,
        compiled_group_keys: Vec<CompiledWorldExpr>,
        projections: Vec<(String, GroupProjection)>,
        groups: PMap<Vec<Value>, PSet<DerivationId>>,
    },
}

/// Fully observable snapshot of the network state for diffs and verification.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct WorldNetworkState {
    pub base_relations: BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>,
    pub derived_relations: BTreeMap<String, BTreeSet<TupleRecord>>,
    pub candidate_frontier: BTreeMap<String, BTreeMap<String, BTreeMap<String, CandidateEntry>>>,
    pub settlements: BTreeMap<String, BTreeMap<String, SettledDecision>>,
    pub distinct_supports: BTreeMap<OperatorId, BTreeMap<TupleRecord, BTreeSet<DerivationId>>>,
}

/// Outcome report summarizing changes after applying a batch through the network.
#[derive(Clone, Debug, Default)]
pub struct NetworkDeltaReport {
    pub revision: u64,
    pub ops_applied: usize,
    pub intermediate_deltas_count: usize,
    pub derived_tuples_inserted: usize,
    pub derived_tuples_retracted: usize,
    pub candidates_inserted: usize,
    pub candidates_retracted: usize,
    /// Settlements this batch added or changed, keyed by decide block then entity.
    pub settlements: BTreeMap<String, BTreeMap<String, SettledDecision>>,
    /// Entities whose settlement this batch removed (quiescence: no candidate
    /// survives), keyed by decide block (ADR-0046 P6 G3, decided 2026-10-04).
    pub removed_settlements: BTreeMap<String, BTreeSet<String>>,
    /// Operator network execution and fanout diagnostics.
    pub diagnostics: NetworkDiagnostics,
}

/// The maintained operator network running bounded incremental evaluations (ADR-0046 §3.5, §3.7).
#[derive(Clone, Debug)]
pub struct WorldNetwork {
    pub dag: Arc<RelationDag>,
    pub operator_states: PMap<usize, OperatorState>,
    pub relation_subscribers: PMap<String, Vec<OperatorId>>,
    pub downstream: PMap<OperatorId, Vec<OperatorId>>,
    output_relations: PMap<OperatorId, Vec<String>>,
    pub base_relations: PMap<String, PMap<WorldKey, WorldTuple>>,
    pub derived_relations: PMap<String, PSet<TupleRecord>>,
    pub decides: PMap<String, DecideBlock>,
    decision_subscribers: PMap<String, Vec<String>>,
    /// Content-addressed trie of settled decisions, keyed by [`decision_key`].
    /// `pub(crate)` so `session.rs` can persist its nodes to the durable node
    /// store (ADR-0046 P6 G2, decided 2026-10-04) without this module owning
    /// any filesystem concern.
    pub(crate) decision_tree: TrieMap<WorldKey, WorldTuple>,
    pub candidate_frontier: PMap<String, PMap<String, PMap<String, CandidateEntry>>>,
    pub functions: Arc<BTreeMap<String, ast::Callable>>,
    pub program_env: Option<Arc<CompiledProgramEnv>>,
    pub current_revision: u64,
    pub scheduling_quantum: usize,
}

impl WorldNetwork {
    /// Construct a new network from a lowered relational DAG.
    pub fn new(dag: RelationDag) -> Self {
        let mut operator_states = PMap::new();
        let mut relation_subscribers: PMap<String, Vec<OperatorId>> = PMap::new();
        let mut downstream: PMap<OperatorId, Vec<OperatorId>> = PMap::new();
        let mut base_relations = PMap::new();

        for (idx, node) in dag.nodes.iter().enumerate() {
            let op_id = OperatorId(idx);
            match node {
                OperatorNode::Scan {
                    relation,
                    key_fields,
                    schema,
                } => {
                    relation_subscribers
                        .entry(relation.clone())
                        .or_default()
                        .push(op_id);
                    base_relations.insert(relation.clone(), PMap::new());
                    operator_states.insert(
                        idx,
                        OperatorState::Scan {
                            relation: relation.clone(),
                            key_fields: key_fields.clone(),
                            schema: schema.clone(),
                            records: PMap::new(),
                        },
                    );
                }
                OperatorNode::Bind { input, alias } => {
                    downstream.entry(*input).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::Bind {
                            input: *input,
                            alias: alias.clone(),
                        },
                    );
                }
                OperatorNode::Filter { input, predicate } => {
                    downstream.entry(*input).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::Filter {
                            input: *input,
                            predicate: predicate.clone(),
                            compiled_predicate: None,
                        },
                    );
                }
                OperatorNode::Project { input, projections } => {
                    downstream.entry(*input).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::Project {
                            input: *input,
                            projections: projections.clone(),
                            compiled_projections: Vec::new(),
                        },
                    );
                }
                OperatorNode::EquiJoin {
                    left,
                    right,
                    left_keys,
                    right_keys,
                } => {
                    downstream.entry(*left).or_default().push(op_id);
                    downstream.entry(*right).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::EquiJoin {
                            left: *left,
                            right: *right,
                            left_keys: left_keys.clone(),
                            right_keys: right_keys.clone(),
                            left_index: PMap::new(),
                            right_index: PMap::new(),
                        },
                    );
                }
                OperatorNode::Distinct { input } => {
                    downstream.entry(*input).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::Distinct {
                            input: *input,
                            supports: PMap::new(),
                        },
                    );
                }
                OperatorNode::GroupedCount {
                    input,
                    group_keys,
                    projections,
                } => {
                    downstream.entry(*input).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::GroupedCount {
                            input: *input,
                            group_keys: group_keys.clone(),
                            compiled_group_keys: Vec::new(),
                            projections: projections.clone(),
                            groups: PMap::new(),
                        },
                    );
                }
            }
        }

        let mut derived_relations = PMap::new();
        for (name, &op_id) in &dag.relation_outputs {
            if !matches!(dag.nodes[op_id.0], OperatorNode::Scan { .. }) {
                derived_relations.insert(name.clone(), PSet::new());
            }
        }

        let mut output_relations: PMap<OperatorId, Vec<String>> = PMap::new();
        for (name, op) in &dag.relation_outputs {
            output_relations.entry(*op).or_default().push(name.clone());
        }
        Self {
            dag: Arc::new(dag),
            output_relations,
            operator_states,
            relation_subscribers,
            downstream,
            base_relations,
            derived_relations,
            decides: PMap::new(),
            decision_subscribers: PMap::new(),
            decision_tree: TrieMap::new(),
            candidate_frontier: PMap::new(),
            functions: Arc::new(BTreeMap::new()),
            program_env: None,
            current_revision: 0,
            scheduling_quantum: DEFAULT_SCHEDULING_QUANTUM,
        }
    }

    /// Set scheduling quantum (work units per operator slice).
    pub fn with_scheduling_quantum(mut self, quantum: usize) -> Self {
        self.scheduling_quantum = quantum.max(1);
        self
    }

    /// Set scheduling quantum in place.
    pub fn set_scheduling_quantum(&mut self, quantum: usize) {
        self.scheduling_quantum = quantum.max(1);
    }

    /// Attach decide blocks to this network.
    pub fn with_decides(mut self, decides: Vec<DecideBlock>) -> Self {
        for d in decides {
            self.decision_subscribers
                .entry(d.source_relation.clone())
                .or_default()
                .push(d.name.clone());
            self.decides.insert(d.name.clone(), d);
        }
        self
    }

    /// Attach helper functions to this network.
    pub fn with_functions(mut self, functions: BTreeMap<String, ast::Callable>) -> Self {
        self.functions = Arc::new(functions);
        self
    }

    /// Attach compiled program environment to this network.
    pub fn with_program_env(mut self, env: Arc<CompiledProgramEnv>) -> Self {
        self.program_env = Some(env);
        self
    }

    /// Create a clean copy of the network resetting data structures while retaining compiled operators and environment.
    pub fn clear_data(&self) -> Self {
        let mut fresh = self.clone();
        fresh.base_relations = self
            .base_relations
            .iter()
            .map(|(k, _)| (k.clone(), PMap::new()))
            .collect();
        fresh.derived_relations = self
            .derived_relations
            .iter()
            .map(|(k, _)| (k.clone(), PSet::new()))
            .collect();
        fresh.decision_tree = TrieMap::new();
        fresh.candidate_frontier = PMap::new();
        fresh.current_revision = 0;
        let mut op_states = self.operator_states.clone();
        for (idx, state) in &self.operator_states {
            let cleared_state = match state {
                OperatorState::Scan {
                    relation,
                    key_fields,
                    schema,
                    ..
                } => OperatorState::Scan {
                    relation: relation.clone(),
                    key_fields: key_fields.clone(),
                    schema: schema.clone(),
                    records: PMap::new(),
                },
                OperatorState::Bind { .. }
                | OperatorState::Filter { .. }
                | OperatorState::Project { .. } => state.clone(),
                OperatorState::EquiJoin {
                    left,
                    right,
                    left_keys,
                    right_keys,
                    ..
                } => OperatorState::EquiJoin {
                    left: *left,
                    right: *right,
                    left_keys: left_keys.clone(),
                    right_keys: right_keys.clone(),
                    left_index: PMap::new(),
                    right_index: PMap::new(),
                },
                OperatorState::Distinct { input, .. } => OperatorState::Distinct {
                    input: *input,
                    supports: PMap::new(),
                },
                OperatorState::GroupedCount {
                    input,
                    group_keys,
                    compiled_group_keys,
                    projections,
                    ..
                } => OperatorState::GroupedCount {
                    input: *input,
                    group_keys: group_keys.clone(),
                    compiled_group_keys: compiled_group_keys.clone(),
                    projections: projections.clone(),
                    groups: PMap::new(),
                },
            };
            op_states.insert(*idx, cleared_state);
        }
        fresh.operator_states = op_states;
        fresh
    }

    /// Construct a network directly from a linked program.
    pub fn from_program(program: &LinkedProgram) -> Result<Self, WorldError> {
        let mut dag = lower_relations(program)?;
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
                    let qname = brix_lower::module_graph::QualifiedName::new(module, local);
                    let config = program.configs.get(&qname).ok_or_else(|| {
                        WorldError::NetworkError(format!("unknown row schema {qname}"))
                    })?;
                    match &config.body {
                        ast::ConfigBody::Record(fields) => {
                            *schema = ast::Ty::Record(fields.clone())
                        }
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

        let mut network = Self::new(dag);
        for (idx, node) in network.dag.nodes.iter().enumerate() {
            match node {
                OperatorNode::Filter { input, predicate } => {
                    let bindings = derive_binding_names(&op_schemas[input.0]);
                    let compiled = program_env
                        .compile_expr(predicate, &bindings)
                        .map_err(WorldError::NetworkError)?;
                    if let Some(OperatorState::Filter {
                        ref mut compiled_predicate,
                        ..
                    }) = network.operator_states.get_mut(&idx)
                    {
                        *compiled_predicate = Some(compiled);
                    }
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
                    if let Some(OperatorState::Project {
                        compiled_projections: ref mut cp,
                        ..
                    }) = network.operator_states.get_mut(&idx)
                    {
                        *cp = compiled_projections;
                    }
                }
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
                    if let Some(OperatorState::GroupedCount {
                        compiled_group_keys: ref mut cgk,
                        ..
                    }) = network.operator_states.get_mut(&idx)
                    {
                        *cgk = compiled_group_keys;
                    }
                }
                _ => {}
            }
        }

        let mut decides = Vec::new();
        for (qname, decl) in &program.decides {
            let source_relation = resolve_decide_source_relation(
                &decl.list,
                &qname.module,
                &network.dag.relation_outputs,
            );

            if let Some(src) = source_relation {
                let per_field = decl.per.clone().ok_or_else(|| {
                    WorldError::NetworkError(format!(
                        "decide '{qname}' resolved to relation '{src}' with no 'per' field \
                         (expected lower_relations to have refused this)"
                    ))
                })?;
                let op_id = network.dag.relation_outputs.get(&src).ok_or_else(|| {
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
                    compiled_proposals.push(CompiledPropose {
                        name: prop.name.clone(),
                        priority: prop.priority,
                        guard,
                        value,
                    });
                }
                decides.push(DecideBlock {
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

        Ok(network
            .with_decides(decides)
            .with_functions(functions)
            .with_program_env(program_env))
    }

    /// Apply an atomic mutation batch envelope to the operator network.
    pub fn apply_batch(&mut self, batch: &WorldBatch) -> Result<NetworkDeltaReport, WorldError> {
        self.apply_batch_bounded(batch, None)
    }

    /// Apply an atomic mutation batch envelope with explicit execution limits.
    pub fn apply_batch_bounded(
        &mut self,
        batch: &WorldBatch,
        limits: Option<&NetworkLimits>,
    ) -> Result<NetworkDeltaReport, WorldError> {
        if batch.expected_base_revision != self.current_revision {
            return Err(WorldError::StaleBaseRevision {
                expected: batch.expected_base_revision,
                current: self.current_revision,
            });
        }
        let normalized = batch.validate_and_normalize()?;
        self.apply_ops_bounded(&normalized, limits)
    }

    /// Apply normalized batch operations to the operator network.
    pub fn apply_ops(&mut self, ops: &[WorldBatchOp]) -> Result<NetworkDeltaReport, WorldError> {
        self.apply_ops_bounded(ops, None)
    }

    /// Apply normalized batch operations with explicit execution limits.
    pub fn apply_ops_bounded(
        &mut self,
        ops: &[WorldBatchOp],
        limits: Option<&NetworkLimits>,
    ) -> Result<NetworkDeltaReport, WorldError> {
        let mut staged = self.clone();
        let report =
            staged.apply_ops_staged_bounded(ops, limits.unwrap_or(&NetworkLimits::default()))?;
        *self = staged;
        Ok(report)
    }

    /// Apply to a private staging network; callers publish it only on success.
    pub fn apply_ops_staged(
        &mut self,
        ops: &[WorldBatchOp],
    ) -> Result<NetworkDeltaReport, WorldError> {
        self.apply_ops_staged_bounded(ops, &NetworkLimits::default())
    }

    /// Apply to private staging network with explicit resource limits and fair scheduling.
    pub fn apply_ops_staged_bounded(
        &mut self,
        ops: &[WorldBatchOp],
        limits: &NetworkLimits,
    ) -> Result<NetworkDeltaReport, WorldError> {
        let mut report = NetworkDeltaReport {
            revision: self.current_revision + 1,
            ops_applied: ops.len(),
            ..Default::default()
        };

        let mut ready_operators: std::collections::VecDeque<OperatorId> =
            std::collections::VecDeque::new();
        let mut in_ready: BTreeSet<OperatorId> = BTreeSet::new();
        let mut operator_queues: BTreeMap<
            OperatorId,
            std::collections::VecDeque<OperatorWorkItem>,
        > = BTreeMap::new();
        let mut diagnostics = NetworkDiagnostics::default();
        let mut total_work: u64 = 0;
        let mut total_matches: u64 = 0;

        let enqueue =
            |op: OperatorId,
             item: OperatorWorkItem,
             queues: &mut BTreeMap<OperatorId, std::collections::VecDeque<OperatorWorkItem>>,
             ready: &mut std::collections::VecDeque<OperatorId>,
             in_ready_set: &mut BTreeSet<OperatorId>| {
                queues.entry(op).or_default().push_back(item);
                if in_ready_set.insert(op) {
                    ready.push_back(op);
                }
            };

        // 1. Process base ops at Scan nodes
        for op in ops {
            let rel_name = op.relation();
            let base_rel = self
                .base_relations
                .get_mut(rel_name)
                .ok_or_else(|| WorldError::UnknownRelation(rel_name.to_string()))?;

            let scan_ops = self
                .relation_subscribers
                .get(rel_name)
                .cloned()
                .unwrap_or_default();
            for scan_op_id in scan_ops {
                let scan_state = self
                    .operator_states
                    .get_mut(&scan_op_id.0)
                    .expect("scan operator");
                if let OperatorState::Scan {
                    relation,
                    schema,
                    records,
                    ..
                } = scan_state
                {
                    match op {
                        WorldBatchOp::Upsert { key, tuple, .. } => {
                            base_rel.insert(key.clone(), tuple.clone());
                            let derivation = DerivationId::Base {
                                relation: relation.clone(),
                                key: key.clone(),
                            };

                            let rec = TupleRecord::from_tuple(tuple)?;
                            let ast::Ty::Record(fields) = schema else {
                                return Err(WorldError::NetworkError(format!(
                                    "unresolved row schema for {relation}"
                                )));
                            };
                            if rec.fields.len() != fields.len() {
                                return Err(WorldError::NetworkError(format!(
                                    "row fields do not match schema for {relation}"
                                )));
                            }
                            let mut intermediate = IntermediateTuple::new();
                            for field in fields {
                                let bytes = rec.get(&field.name).ok_or_else(|| {
                                    WorldError::NetworkError(format!(
                                        "missing field {relation}.{}",
                                        field.name
                                    ))
                                })?;
                                intermediate.insert(
                                    field.name.clone(),
                                    Value::from_typed_bytes(bytes, &field.ty)?,
                                );
                            }

                            // If old record existed at key, retract it first
                            if let Some((_, old_inter)) = records.get(key) {
                                if old_inter == &intermediate {
                                    // Idempotent value, no change
                                    continue;
                                }
                                let ret_delta = TupleDelta::Retract {
                                    tuple: old_inter.clone(),
                                    derivation: derivation.clone(),
                                };
                                enqueue(
                                    scan_op_id,
                                    OperatorWorkItem::Delta(ret_delta),
                                    &mut operator_queues,
                                    &mut ready_operators,
                                    &mut in_ready,
                                );
                            }

                            let ins_delta = TupleDelta::Insert {
                                tuple: intermediate.clone(),
                                derivation,
                            };
                            records.insert(key.clone(), (tuple.clone(), intermediate));
                            enqueue(
                                scan_op_id,
                                OperatorWorkItem::Delta(ins_delta),
                                &mut operator_queues,
                                &mut ready_operators,
                                &mut in_ready,
                            );
                        }
                        WorldBatchOp::Remove { key, .. } => {
                            base_rel.remove(key);
                            if let Some((_, old_inter)) = records.remove(key) {
                                let derivation = DerivationId::Base {
                                    relation: relation.clone(),
                                    key: key.clone(),
                                };
                                let ret_delta = TupleDelta::Retract {
                                    tuple: old_inter,
                                    derivation,
                                };
                                enqueue(
                                    scan_op_id,
                                    OperatorWorkItem::Delta(ret_delta),
                                    &mut operator_queues,
                                    &mut ready_operators,
                                    &mut in_ready,
                                );
                            }
                        }
                    }
                }
            }
        }

        let init_q_len: usize = operator_queues
            .values()
            .map(|q| q.iter().map(work_item_queued_count).sum::<usize>())
            .sum();
        diagnostics.peak_queue_size = diagnostics.peak_queue_size.max(init_q_len);
        if limits.max_queued_deltas.is_some_and(|max| init_q_len > max) {
            return Err(WorldError::BudgetExhausted);
        }

        // 2. Fair round-robin propagation across all operators bounded by quantum
        let mut relation_deltas: BTreeMap<String, Vec<TupleDelta>> = BTreeMap::new();
        let quantum = self.scheduling_quantum.max(1);

        while let Some(op_id) = ready_operators.pop_front() {
            in_ready.remove(&op_id);
            let op_idx = op_id.0;
            let mut work_in_slice: usize = 0;
            let mut out_deltas = Vec::new();

            while work_in_slice < quantum {
                let Some(item) = operator_queues.get_mut(&op_id).and_then(|q| q.pop_front()) else {
                    break;
                };

                work_in_slice += 1;
                total_work += 1;
                *diagnostics.operator_work.entry(op_id).or_default() += 1;
                if limits.max_work.is_some_and(|max| total_work > max) {
                    return Err(WorldError::BudgetExhausted);
                }

                match self
                    .operator_states
                    .get_mut(&op_idx)
                    .expect("queued operator")
                {
                    OperatorState::Scan { .. } => match item {
                        OperatorWorkItem::Delta(d) => {
                            out_deltas.push(d);
                        }
                        _ => unreachable!(),
                    },
                    OperatorState::Bind { alias, .. } => match item {
                        OperatorWorkItem::Delta(delta) => {
                            let is_ins = delta.is_insert();
                            let (tuple, deriv) = match delta {
                                TupleDelta::Insert { tuple, derivation }
                                | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                            };

                            let mut bound_tuple = IntermediateTuple::new();
                            for (k, v) in &tuple.fields {
                                bound_tuple.insert(format!("{alias}.{k}"), v.clone());
                                bound_tuple.insert(k.clone(), v.clone());
                            }

                            let out_deriv = DerivationId::Unary {
                                op: op_id,
                                parent: Box::new(deriv),
                            };

                            if is_ins {
                                out_deltas.push(TupleDelta::Insert {
                                    tuple: bound_tuple,
                                    derivation: out_deriv,
                                });
                            } else {
                                out_deltas.push(TupleDelta::Retract {
                                    tuple: bound_tuple,
                                    derivation: out_deriv,
                                });
                            }
                        }
                        _ => unreachable!(),
                    },
                    OperatorState::Filter {
                        predicate,
                        compiled_predicate,
                        ..
                    } => match item {
                        OperatorWorkItem::Delta(delta) => {
                            let is_ins = delta.is_insert();
                            let (tuple, deriv) = match delta {
                                TupleDelta::Insert { tuple, derivation }
                                | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                            };

                            diagnostics.expressions_evaluated += 1;
                            if limits
                                .max_expressions
                                .is_some_and(|max| diagnostics.expressions_evaluated > max)
                            {
                                return Err(WorldError::BudgetExhausted);
                            }

                            let passed = if let Some(ref compiled) = compiled_predicate {
                                let bindings = scalar_bindings(&tuple)?;
                                let l3_val =
                                    compiled.eval(&bindings).map_err(WorldError::NetworkError)?;
                                match l3_val {
                                    brix_lower::l3_v2::L3ValueV2::Bool(b) => b,
                                    other => {
                                        return Err(WorldError::NetworkError(format!(
                                            "expected boolean, got {other:?}"
                                        )))
                                    }
                                }
                            } else {
                                eval_expr(predicate, &tuple, &self.functions)?.as_bool()?
                            };
                            if passed {
                                let out_deriv = DerivationId::Unary {
                                    op: op_id,
                                    parent: Box::new(deriv),
                                };
                                if is_ins {
                                    out_deltas.push(TupleDelta::Insert {
                                        tuple,
                                        derivation: out_deriv,
                                    });
                                } else {
                                    out_deltas.push(TupleDelta::Retract {
                                        tuple,
                                        derivation: out_deriv,
                                    });
                                }
                            }
                        }
                        _ => unreachable!(),
                    },
                    OperatorState::Project {
                        projections,
                        compiled_projections,
                        ..
                    } => match item {
                        OperatorWorkItem::Delta(delta) => {
                            let is_ins = delta.is_insert();
                            let (tuple, deriv) = match delta {
                                TupleDelta::Insert { tuple, derivation }
                                | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                            };

                            let mut projected = IntermediateTuple::new();
                            if !compiled_projections.is_empty() {
                                let bindings = scalar_bindings(&tuple)?;
                                for (name, compiled) in compiled_projections.iter() {
                                    diagnostics.expressions_evaluated += 1;
                                    if limits
                                        .max_expressions
                                        .is_some_and(|max| diagnostics.expressions_evaluated > max)
                                    {
                                        return Err(WorldError::BudgetExhausted);
                                    }
                                    let l3_val = compiled
                                        .eval(&bindings)
                                        .map_err(WorldError::NetworkError)?;
                                    let val = Value::from_scalar(l3_val)?;
                                    projected.insert(name.clone(), val);
                                }
                            } else {
                                for (name, expr) in projections.iter() {
                                    diagnostics.expressions_evaluated += 1;
                                    if limits
                                        .max_expressions
                                        .is_some_and(|max| diagnostics.expressions_evaluated > max)
                                    {
                                        return Err(WorldError::BudgetExhausted);
                                    }
                                    let val = eval_expr(expr, &tuple, &self.functions)?;
                                    projected.insert(name.clone(), val);
                                }
                            }

                            let out_deriv = DerivationId::Unary {
                                op: op_id,
                                parent: Box::new(deriv),
                            };

                            if is_ins {
                                out_deltas.push(TupleDelta::Insert {
                                    tuple: projected,
                                    derivation: out_deriv,
                                });
                            } else {
                                out_deltas.push(TupleDelta::Retract {
                                    tuple: projected,
                                    derivation: out_deriv,
                                });
                            }
                        }
                        _ => unreachable!(),
                    },
                    OperatorState::EquiJoin {
                        left,
                        right: _,
                        left_keys,
                        right_keys,
                        left_index,
                        right_index,
                    } => {
                        let left_op = *left;

                        let (delta, is_left, join_key, matches, next_idx) = match item {
                            OperatorWorkItem::JoinContinuation {
                                delta,
                                is_left,
                                join_key,
                                matches,
                                next_idx,
                            } => (delta, is_left, join_key, matches, next_idx),
                            OperatorWorkItem::Delta(delta) => {
                                let (tuple, deriv) = match &delta {
                                    TupleDelta::Insert { tuple, derivation }
                                    | TupleDelta::Retract { tuple, derivation } => {
                                        (tuple, derivation)
                                    }
                                };

                                let is_left = match deriv {
                                    DerivationId::Unary { op, .. }
                                    | DerivationId::Join { op, .. }
                                    | DerivationId::Group { op, .. }
                                    | DerivationId::Distinct { op, .. } => *op == left_op,
                                    DerivationId::Base { relation, .. } => {
                                        match &self.dag.nodes[left_op.0] {
                                            OperatorNode::Scan { relation: r, .. } => r == relation,
                                            _ => false,
                                        }
                                    }
                                };

                                let keys = if is_left { left_keys } else { right_keys };
                                let join_key: Vec<Value> = keys
                                    .iter()
                                    .map(|k| {
                                        tuple
                                            .get_qualified(&k.binding, &k.field)
                                            .cloned()
                                            .ok_or_else(|| {
                                                WorldError::NetworkError(format!(
                                                    "missing join field {}.{}",
                                                    k.binding, k.field
                                                ))
                                            })
                                    })
                                    .collect::<Result<_, _>>()?;

                                let is_ins = delta.is_insert();
                                let matches: PMap<DerivationId, IntermediateTuple> = if is_left {
                                    if is_ins {
                                        let m =
                                            right_index.get(&join_key).cloned().unwrap_or_default();
                                        left_index
                                            .entry(join_key.clone())
                                            .or_default()
                                            .insert(deriv.clone(), tuple.clone());
                                        m
                                    } else {
                                        if let Some(matching_lefts) = left_index.get_mut(&join_key)
                                        {
                                            matching_lefts.remove(deriv);
                                            if matching_lefts.is_empty() {
                                                left_index.remove(&join_key);
                                            }
                                        }
                                        right_index.get(&join_key).cloned().unwrap_or_default()
                                    }
                                } else {
                                    if is_ins {
                                        let m =
                                            left_index.get(&join_key).cloned().unwrap_or_default();
                                        right_index
                                            .entry(join_key.clone())
                                            .or_default()
                                            .insert(deriv.clone(), tuple.clone());
                                        m
                                    } else {
                                        if let Some(matching_rights) =
                                            right_index.get_mut(&join_key)
                                        {
                                            matching_rights.remove(deriv);
                                            if matching_rights.is_empty() {
                                                right_index.remove(&join_key);
                                            }
                                        }
                                        left_index.get(&join_key).cloned().unwrap_or_default()
                                    }
                                };
                                (delta, is_left, join_key, matches, 0)
                            }
                        };

                        let is_ins = delta.is_insert();
                        let (tuple, deriv) = match &delta {
                            TupleDelta::Insert { tuple, derivation }
                            | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                        };

                        let remaining_quantum = quantum.saturating_sub(work_in_slice);
                        let remaining_matches = matches.len().saturating_sub(next_idx);
                        if remaining_matches > 0 {
                            if limits.max_matches.is_some_and(|max| total_matches >= max) {
                                return Err(WorldError::BudgetExhausted);
                            }
                            if limits.max_work.is_some_and(|max| total_work >= max) {
                                return Err(WorldError::BudgetExhausted);
                            }
                        }
                        let take_count = if remaining_matches == 0 {
                            0
                        } else {
                            remaining_quantum.max(1).min(remaining_matches)
                        };

                        let key_str = format!(
                            "{op_id:?}:{}",
                            join_key
                                .iter()
                                .map(|v| v.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        );

                        for (other_deriv, other_tuple) in
                            matches.iter().skip(next_idx).take(take_count)
                        {
                            let (comp_tuple, comp_deriv) = if is_left {
                                let composite = tuple.merge(other_tuple);
                                let comp_deriv = DerivationId::Join {
                                    op: op_id,
                                    left: Box::new(deriv.clone()),
                                    right: Box::new(other_deriv.clone()),
                                };
                                (composite, comp_deriv)
                            } else {
                                let composite = other_tuple.merge(tuple);
                                let comp_deriv = DerivationId::Join {
                                    op: op_id,
                                    left: Box::new(other_deriv.clone()),
                                    right: Box::new(deriv.clone()),
                                };
                                (composite, comp_deriv)
                            };

                            work_in_slice += 1;
                            total_work += 1;
                            total_matches += 1;
                            *diagnostics.operator_work.entry(op_id).or_default() += 1;
                            *diagnostics.join_fanout.entry(op_id).or_default() += 1;
                            *diagnostics
                                .hot_join_keys
                                .entry(key_str.clone())
                                .or_default() += 1;

                            if limits.max_work.is_some_and(|max| total_work > max) {
                                return Err(WorldError::BudgetExhausted);
                            }
                            if limits.max_matches.is_some_and(|max| total_matches > max) {
                                return Err(WorldError::BudgetExhausted);
                            }

                            if is_ins {
                                out_deltas.push(TupleDelta::Insert {
                                    tuple: comp_tuple,
                                    derivation: comp_deriv,
                                });
                            } else {
                                out_deltas.push(TupleDelta::Retract {
                                    tuple: comp_tuple,
                                    derivation: comp_deriv,
                                });
                            }
                        }

                        let new_next_idx = next_idx + take_count;
                        if new_next_idx < matches.len() {
                            operator_queues.entry(op_id).or_default().push_front(
                                OperatorWorkItem::JoinContinuation {
                                    delta,
                                    is_left,
                                    join_key,
                                    matches,
                                    next_idx: new_next_idx,
                                },
                            );
                        }
                    }
                    OperatorState::Distinct { supports, .. } => match item {
                        OperatorWorkItem::Delta(delta) => {
                            let is_ins = delta.is_insert();
                            let (tuple, deriv) = match delta {
                                TupleDelta::Insert { tuple, derivation }
                                | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                            };

                            let tuple_key = tuple.to_tuple_record().to_tuple().0;
                            let out_deriv = DerivationId::Distinct {
                                op: op_id,
                                tuple_key,
                            };

                            if is_ins {
                                let entry = supports.entry(tuple.clone()).or_default();
                                let was_empty = entry.is_empty();
                                entry.insert(deriv);
                                if was_empty {
                                    out_deltas.push(TupleDelta::Insert {
                                        tuple,
                                        derivation: out_deriv,
                                    });
                                }
                            } else if let Some(entry) = supports.get_mut(&tuple) {
                                entry.remove(&deriv);
                                if entry.is_empty() {
                                    supports.remove(&tuple);
                                    out_deltas.push(TupleDelta::Retract {
                                        tuple,
                                        derivation: out_deriv,
                                    });
                                }
                            }
                        }
                        _ => unreachable!(),
                    },
                    OperatorState::GroupedCount {
                        group_keys,
                        compiled_group_keys,
                        projections,
                        groups,
                        ..
                    } => match item {
                        OperatorWorkItem::Delta(delta) => {
                            let is_ins = delta.is_insert();
                            let (tuple, deriv) = match delta {
                                TupleDelta::Insert { tuple, derivation }
                                | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                            };

                            let key_vals: Vec<Value> = if !compiled_group_keys.is_empty() {
                                let bindings = scalar_bindings(&tuple)?;
                                compiled_group_keys
                                    .iter()
                                    .map(|compiled| {
                                        diagnostics.expressions_evaluated += 1;
                                        if limits.max_expressions.is_some_and(|max| {
                                            diagnostics.expressions_evaluated > max
                                        }) {
                                            return Err(WorldError::BudgetExhausted);
                                        }
                                        let l3_val = compiled
                                            .eval(&bindings)
                                            .map_err(WorldError::NetworkError)?;
                                        Value::from_scalar(l3_val)
                                    })
                                    .collect::<Result<_, _>>()?
                            } else {
                                group_keys
                                    .iter()
                                    .map(|expr| {
                                        diagnostics.expressions_evaluated += 1;
                                        if limits.max_expressions.is_some_and(|max| {
                                            diagnostics.expressions_evaluated > max
                                        }) {
                                            return Err(WorldError::BudgetExhausted);
                                        }
                                        eval_expr(expr, &tuple, &self.functions)
                                    })
                                    .collect::<Result<_, _>>()?
                            };

                            if is_ins {
                                let set = groups.entry(key_vals.clone()).or_default();
                                let old_count = set.len();
                                set.insert(deriv);
                                let new_count = set.len();

                                if new_count != old_count {
                                    let out_deriv = DerivationId::Group {
                                        op: op_id,
                                        group_key: key_vals.clone(),
                                    };
                                    if old_count == 0 {
                                        let out_tuple =
                                            build_grouped_tuple(projections, &key_vals, 1);
                                        out_deltas.push(TupleDelta::Insert {
                                            tuple: out_tuple,
                                            derivation: out_deriv,
                                        });
                                    } else {
                                        let old_tuple =
                                            build_grouped_tuple(projections, &key_vals, old_count);
                                        let new_tuple =
                                            build_grouped_tuple(projections, &key_vals, new_count);
                                        out_deltas.push(TupleDelta::Retract {
                                            tuple: old_tuple,
                                            derivation: out_deriv.clone(),
                                        });
                                        out_deltas.push(TupleDelta::Insert {
                                            tuple: new_tuple,
                                            derivation: out_deriv,
                                        });
                                    }
                                }
                            } else if let Some(set) = groups.get_mut(&key_vals) {
                                let old_count = set.len();
                                set.remove(&deriv);
                                let new_count = set.len();

                                if new_count != old_count {
                                    let out_deriv = DerivationId::Group {
                                        op: op_id,
                                        group_key: key_vals.clone(),
                                    };
                                    if new_count == 0 {
                                        let old_tuple =
                                            build_grouped_tuple(projections, &key_vals, 1);
                                        groups.remove(&key_vals);
                                        out_deltas.push(TupleDelta::Retract {
                                            tuple: old_tuple,
                                            derivation: out_deriv,
                                        });
                                    } else {
                                        let old_tuple =
                                            build_grouped_tuple(projections, &key_vals, old_count);
                                        let new_tuple =
                                            build_grouped_tuple(projections, &key_vals, new_count);
                                        out_deltas.push(TupleDelta::Retract {
                                            tuple: old_tuple,
                                            derivation: out_deriv.clone(),
                                        });
                                        out_deltas.push(TupleDelta::Insert {
                                            tuple: new_tuple,
                                            derivation: out_deriv,
                                        });
                                    }
                                }
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            }

            // Propagate out_deltas
            if !out_deltas.is_empty() {
                report.intermediate_deltas_count += out_deltas.len();

                if let Some(outputs) = self.output_relations.get(&op_id) {
                    for rel_name in outputs {
                        *diagnostics
                            .relation_deltas
                            .entry(rel_name.clone())
                            .or_default() += out_deltas.len() as u64;
                        relation_deltas
                            .entry(rel_name.clone())
                            .or_default()
                            .extend(out_deltas.clone());

                        if !self.base_relations.contains_key(rel_name) {
                            for od in &out_deltas {
                                let rec = od.tuple().to_tuple_record();
                                if od.is_insert() {
                                    if self
                                        .derived_relations
                                        .entry(rel_name.clone())
                                        .or_default()
                                        .insert(rec)
                                    {
                                        report.derived_tuples_inserted += 1;
                                    }
                                } else {
                                    if self
                                        .derived_relations
                                        .entry(rel_name.clone())
                                        .or_default()
                                        .remove(&rec)
                                    {
                                        report.derived_tuples_retracted += 1;
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(down) = self.downstream.get(&op_id) {
                    for consumer in down {
                        for od in &out_deltas {
                            enqueue(
                                *consumer,
                                OperatorWorkItem::Delta(od.clone()),
                                &mut operator_queues,
                                &mut ready_operators,
                                &mut in_ready,
                            );
                        }
                    }
                }
            }

            if operator_queues.get(&op_id).is_some_and(|q| !q.is_empty()) && in_ready.insert(op_id)
            {
                ready_operators.push_back(op_id);
            }

            let cur_q_len: usize = operator_queues
                .values()
                .map(|q| q.iter().map(work_item_queued_count).sum::<usize>())
                .sum();
            diagnostics.peak_queue_size = diagnostics.peak_queue_size.max(cur_q_len);
            if limits.max_queued_deltas.is_some_and(|max| cur_q_len > max) {
                return Err(WorldError::BudgetExhausted);
            }
        }

        // 3. Update candidate frontiers for decide blocks from relation deltas
        let mut touched_entities = BTreeSet::new();
        for (relation, rel_deltas) in &relation_deltas {
            for decide_name in self
                .decision_subscribers
                .get(relation)
                .into_iter()
                .flatten()
            {
                let decide = &self.decides[decide_name];
                for delta in rel_deltas
                    .iter()
                    .filter(|d| !d.is_insert())
                    .chain(rel_deltas.iter().filter(|d| d.is_insert()))
                {
                    let is_ins = delta.is_insert();
                    let (tuple, deriv) = match delta {
                        TupleDelta::Insert { tuple, derivation }
                        | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                    };

                    let mut eval_tuple = tuple.clone();
                    for (k, v) in &tuple.fields {
                        eval_tuple.insert(format!("{}.{k}", decide.binder), v.clone());
                    }

                    for (p_idx, propose) in decide.proposals.iter().enumerate() {
                        let entity_id =
                            entity_id_for(&eval_tuple, &decide.binder, &decide.per_field)?;
                        touched_entities.insert((decide.name.clone(), entity_id.clone()));

                        if is_ins {
                            let compiled = decide.compiled_proposals.get(p_idx);
                            let (guard_passed, val) = if let Some(compiled) = compiled {
                                let bindings = scalar_bindings(&eval_tuple)?;
                                diagnostics.expressions_evaluated += 1;
                                if limits
                                    .max_expressions
                                    .is_some_and(|max| diagnostics.expressions_evaluated > max)
                                {
                                    return Err(WorldError::BudgetExhausted);
                                }
                                let guard_l3 = compiled
                                    .guard
                                    .eval(&bindings)
                                    .map_err(WorldError::NetworkError)?;
                                let guard_passed = match guard_l3 {
                                    brix_lower::l3_v2::L3ValueV2::Bool(b) => b,
                                    other => {
                                        return Err(WorldError::NetworkError(format!(
                                            "expected boolean, got {other:?}"
                                        )))
                                    }
                                };
                                if guard_passed {
                                    diagnostics.expressions_evaluated += 1;
                                    if limits
                                        .max_expressions
                                        .is_some_and(|max| diagnostics.expressions_evaluated > max)
                                    {
                                        return Err(WorldError::BudgetExhausted);
                                    }
                                    let val_l3 = compiled
                                        .value
                                        .eval(&bindings)
                                        .map_err(WorldError::NetworkError)?;
                                    let val = Value::from_scalar(val_l3)?;
                                    (true, Some(val))
                                } else {
                                    (false, None)
                                }
                            } else {
                                diagnostics.expressions_evaluated += 1;
                                if limits
                                    .max_expressions
                                    .is_some_and(|max| diagnostics.expressions_evaluated > max)
                                {
                                    return Err(WorldError::BudgetExhausted);
                                }
                                let guard_passed =
                                    eval_expr(&propose.guard, &eval_tuple, &self.functions)?
                                        .as_bool()?;
                                if guard_passed {
                                    diagnostics.expressions_evaluated += 1;
                                    if limits
                                        .max_expressions
                                        .is_some_and(|max| diagnostics.expressions_evaluated > max)
                                    {
                                        return Err(WorldError::BudgetExhausted);
                                    }
                                    let val =
                                        eval_expr(&propose.value, &eval_tuple, &self.functions)?;
                                    (true, Some(val))
                                } else {
                                    (false, None)
                                }
                            };

                            if guard_passed {
                                let val = val.unwrap();
                                let cal_key = compute_candidate_calendar_key(
                                    0,
                                    propose.priority,
                                    &decide.name,
                                    &entity_id,
                                    &propose.name,
                                    &val,
                                );

                                let entity_map = self
                                    .candidate_frontier
                                    .entry(decide.name.clone())
                                    .or_default()
                                    .entry(entity_id.clone())
                                    .or_default();

                                if let Some(existing) = entity_map.get(&propose.name) {
                                    if existing.value != val {
                                        return Err(WorldError::NetworkError(format!(
                                            "conflicting support values for decide '{}' entity \
                                             '{entity_id}' candidate '{}': {:?} vs {val:?}",
                                            decide.name, propose.name, existing.value
                                        )));
                                    }
                                }

                                let entry =
                                    entity_map.entry(propose.name.clone()).or_insert_with(|| {
                                        report.candidates_inserted += 1;
                                        CandidateEntry {
                                            candidate_name: propose.name.clone(),
                                            entity_id: entity_id.clone(),
                                            priority: propose.priority,
                                            phase: 0,
                                            value: val.clone(),
                                            calendar_key: cal_key,
                                            supports: PSet::new(),
                                        }
                                    });

                                entry.supports.insert(deriv.clone());
                            }
                        } else if let Some(decide_map) =
                            self.candidate_frontier.get_mut(&decide.name)
                        {
                            if let Some(entity_map) = decide_map.get_mut(&entity_id) {
                                if let Some(entry) = entity_map.get_mut(&propose.name) {
                                    entry.supports.remove(deriv);
                                    if entry.supports.is_empty() {
                                        entity_map.remove(&propose.name);
                                        report.candidates_retracted += 1;
                                    }
                                }
                                if entity_map.is_empty() {
                                    decide_map.remove(&entity_id);
                                }
                            }
                        }
                    }
                }
            }
        }

        for (decide, entity) in touched_entities {
            if self
                .candidate_frontier
                .get(&decide)
                .is_some_and(|entities| entities.is_empty())
            {
                self.candidate_frontier.remove(&decide);
            }
            let key = decision_key(&decide, &entity);
            if let Some(decision) = self.get_settlement(&decide, &entity) {
                let tuple = decision_tuple(&decision);
                if self.decision_tree.get(&key) == Some(&tuple) {
                    continue;
                }
                self.decision_tree = self.decision_tree.insert(key, tuple);
                report
                    .settlements
                    .entry(decide)
                    .or_default()
                    .insert(entity, decision);
            } else if self.decision_tree.get(&key).is_some() {
                self.decision_tree = self.decision_tree.remove(&key);
                report
                    .removed_settlements
                    .entry(decide)
                    .or_default()
                    .insert(entity);
            }
        }
        report.diagnostics = diagnostics;
        self.current_revision += 1;
        Ok(report)
    }

    /// Canonical decision root, maintained only for affected entities.
    pub fn decision_root(&self) -> Digest {
        self.decision_tree.root_digest()
    }

    /// Retrieve all derived tuples produced for a relation.
    pub fn get_derived_tuples(&self, relation: &str) -> Option<Vec<TupleRecord>> {
        self.derived_relations
            .get(relation)
            .map(|set| set.iter().cloned().collect())
    }

    /// Retrieve candidates for a specific decide block and entity.
    pub fn get_candidates(
        &self,
        decide: &str,
        entity_id: &str,
    ) -> Option<&PMap<String, CandidateEntry>> {
        self.candidate_frontier.get(decide)?.get(entity_id)
    }

    /// Deliberate and select the winning candidate for an entity using canonical settlement discipline.
    pub fn get_settlement(&self, decide: &str, entity_id: &str) -> Option<SettledDecision> {
        let decide_frontier = self.candidate_frontier.get(decide)?;
        let entity_candidates = decide_frontier.get(entity_id)?;

        let mut frontier: Frontier<&CandidateEntry> = Frontier::new();
        for candidate in entity_candidates.values() {
            if !candidate.supports.is_empty() {
                let _ = frontier.insert(candidate.calendar_key, candidate);
            }
        }

        let (key, candidate) = frontier.select_least()?;
        Some(SettledDecision {
            entity_id: candidate.entity_id.clone(),
            candidate_name: candidate.candidate_name.clone(),
            priority: candidate.priority,
            phase: candidate.phase,
            value: candidate.value.clone(),
            calendar_key: key,
        })
    }

    /// Settle decisions across all entities in all decide blocks.
    pub fn all_settlements(&self) -> BTreeMap<String, BTreeMap<String, SettledDecision>> {
        let mut out = BTreeMap::new();
        for (decide_name, entity_map) in &self.candidate_frontier {
            let mut entities = BTreeMap::new();
            for entity_id in entity_map.keys() {
                if let Some(decision) = self.get_settlement(decide_name, entity_id) {
                    entities.insert(entity_id.clone(), decision);
                }
            }
            if !entities.is_empty() {
                out.insert(decide_name.clone(), entities);
            }
        }
        out
    }

    /// Capture current observable network state.
    pub fn current_state(&self) -> WorldNetworkState {
        let mut distinct_supports = BTreeMap::new();
        for (idx, state) in self.operator_states.iter() {
            if let OperatorState::Distinct { supports, .. } = state {
                let mut map = BTreeMap::new();
                for (it, sups) in supports {
                    map.insert(it.to_tuple_record(), sups.iter().cloned().collect());
                }
                distinct_supports.insert(OperatorId(*idx), map);
            }
        }

        WorldNetworkState {
            base_relations: self
                .base_relations
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    )
                })
                .collect(),
            derived_relations: self
                .derived_relations
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            candidate_frontier: self
                .candidate_frontier
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.iter()
                            .map(|(k, v)| {
                                (
                                    k.clone(),
                                    v.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                                )
                            })
                            .collect(),
                    )
                })
                .collect(),
            settlements: self.all_settlements(),
            distinct_supports,
        }
    }

    /// Rebuild from base facts using this engine. This checks update history, not independent semantics.
    pub fn recompute_from_scratch(&self) -> Result<WorldNetworkState, WorldError> {
        let mut fresh = self.clear_data();

        let mut ops = Vec::new();
        for (rel, records) in &self.base_relations {
            for (key, tuple) in records {
                ops.push(WorldBatchOp::Upsert {
                    relation: rel.clone(),
                    key: key.clone(),
                    tuple: tuple.clone(),
                });
            }
        }

        fresh.apply_ops(&ops)?;
        Ok(fresh.current_state())
    }

    /// Verify differential correctness: compare incrementally maintained state with oracle full recompute.
    pub fn verify_differential_correctness(&self) -> Result<(), WorldError> {
        let scratch_state = self.recompute_from_scratch()?;
        let incremental_state = self.current_state();

        if incremental_state.derived_relations != scratch_state.derived_relations {
            return Err(WorldError::NetworkError(format!(
                "differential mismatch in derived relations: incremental={:?}, scratch={:?}",
                incremental_state.derived_relations, scratch_state.derived_relations
            )));
        }

        if incremental_state.settlements != scratch_state.settlements {
            return Err(WorldError::NetworkError(format!(
                "differential mismatch in settlements: incremental={:?}, scratch={:?}",
                incremental_state.settlements, scratch_state.settlements
            )));
        }

        if incremental_state.candidate_frontier != scratch_state.candidate_frontier {
            return Err(WorldError::NetworkError(format!(
                "differential mismatch in candidate frontier: incremental={:?}, scratch={:?}",
                incremental_state.candidate_frontier, scratch_state.candidate_frontier
            )));
        }

        if incremental_state.distinct_supports != scratch_state.distinct_supports {
            return Err(WorldError::NetworkError(format!(
                "differential mismatch in distinct supports: incremental={:?}, scratch={:?}",
                incremental_state.distinct_supports, scratch_state.distinct_supports
            )));
        }

        Ok(())
    }

    /// Recursively resolve contributing base facts for a derivation, expanding through Distinct operators.
    pub fn collect_base_facts_for_derivation(
        &self,
        deriv: &DerivationId,
        out: &mut BTreeSet<(String, WorldKey)>,
    ) {
        match deriv {
            DerivationId::Base { relation, key } => {
                out.insert((relation.clone(), key.clone()));
            }
            DerivationId::Unary { parent, .. } => {
                self.collect_base_facts_for_derivation(parent, out);
            }
            DerivationId::Join { left, right, .. } => {
                self.collect_base_facts_for_derivation(left, out);
                self.collect_base_facts_for_derivation(right, out);
            }
            DerivationId::Distinct { op, tuple_key } => {
                if let Some(OperatorState::Distinct { supports, .. }) =
                    self.operator_states.get(&op.0)
                {
                    for (tuple, parent_derivs) in supports {
                        if tuple.to_tuple_record().to_tuple().0 == *tuple_key {
                            for p in parent_derivs {
                                self.collect_base_facts_for_derivation(p, out);
                            }
                        }
                    }
                }
            }
            DerivationId::Group { .. } => {}
        }
    }

    /// Deliberate and explain the decision for an entity key across any matching decide block.
    pub fn explain_decision(&self, entity_id: &str) -> Option<DecisionExplanation> {
        for decide_name in self.candidate_frontier.keys() {
            if let Some(exp) = self.explain_decision_for(decide_name, entity_id) {
                return Some(exp);
            }
        }
        None
    }

    /// Deliberate and explain the decision for an entity key in a specific decide block.
    pub fn explain_decision_for(
        &self,
        decide_name: &str,
        entity_id: &str,
    ) -> Option<DecisionExplanation> {
        let decide_frontier = self.candidate_frontier.get(decide_name)?;
        let entity_candidates = decide_frontier.get(entity_id)?;

        let settlement = self.get_settlement(decide_name, entity_id);
        let winning_name = settlement.as_ref().map(|s| &s.candidate_name);

        let mut candidates = Vec::new();
        let mut contributing_base = BTreeSet::new();

        for (cand_name, entry) in entity_candidates {
            let winning = winning_name == Some(cand_name);
            for deriv in &entry.supports {
                self.collect_base_facts_for_derivation(deriv, &mut contributing_base);
            }
            candidates.push(CandidateExplanation {
                name: cand_name.clone(),
                priority: entry.priority,
                phase: entry.phase,
                value: entry.value.clone(),
                supports_count: entry.supports.len(),
                winning,
            });
        }

        Some(DecisionExplanation {
            entity_id: entity_id.to_string(),
            decide_name: decide_name.to_string(),
            winning_candidate: settlement.as_ref().map(|s| s.candidate_name.clone()),
            value: settlement.as_ref().map(|s| s.value.clone()),
            priority: settlement.as_ref().map(|s| s.priority),
            phase: settlement.as_ref().map(|s| s.phase),
            calendar_key: settlement.as_ref().map(|s| s.calendar_key),
            candidates,
            contributing_facts: contributing_base.into_iter().collect(),
        })
    }
}

fn build_grouped_tuple(
    projections: &[(String, GroupProjection)],
    key_vals: &[Value],
    count: usize,
) -> IntermediateTuple {
    let mut tuple = IntermediateTuple::new();
    for (name, proj) in projections {
        match proj {
            GroupProjection::Key(idx) => {
                let v = key_vals.get(*idx).cloned().unwrap_or(Value::Null);
                tuple.insert(name, v);
            }
            GroupProjection::Count => {
                tuple.insert(name, Value::Int(count as i64));
            }
        }
    }
    tuple
}

/// Extract a tuple's entity identity using the world profile's declared
/// `per <field>` (ADR-0046 §3.5, decided 2026-10-04). Replaces the retired
/// `extract_entity_id` heuristic fallback chain: an unspecified heuristic
/// cannot be independently reproduced (see `reference.rs`'s former copy of
/// it), so entity identity is now an explicit, lowering-checked part of the
/// `decide` declaration. A tuple missing the declared field is a typed fault,
/// never a silent `"default_entity"` fallback.
fn entity_id_for(
    tuple: &IntermediateTuple,
    binder: &str,
    per_field: &str,
) -> Result<String, WorldError> {
    tuple
        .get_qualified(binder, per_field)
        .map(|v| v.to_string())
        .ok_or_else(|| {
            WorldError::NetworkError(format!(
                "decide entity field '{per_field}' missing from bound row (binder '{binder}')"
            ))
        })
}

/// Evaluate an AST expression over intermediate tuple fields and functions.
/// Build scalar bindings without guessing unqualified aliases. Qualified tuple
/// fields become record fields so the shared evaluator performs exact projection.
pub fn scalar_bindings(
    tuple: &IntermediateTuple,
) -> Result<BTreeMap<String, brix_lower::l3_v2::L3ValueV2>, WorldError> {
    use brix_lower::l3_v2::L3ValueV2;
    let mut bindings = BTreeMap::new();
    let mut records: BTreeMap<String, Vec<(String, L3ValueV2)>> = BTreeMap::new();
    for (name, value) in &tuple.fields {
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

/// Convenience entry point; maintained networks retain `CompiledWorldExpr`.
pub fn eval_expr(
    expr: &ast::Expr,
    tuple: &IntermediateTuple,
    functions: &BTreeMap<String, ast::Callable>,
) -> Result<Value, WorldError> {
    let bindings = scalar_bindings(tuple)?;
    let compiled = brix_lower::world_expr::CompiledWorldExpr::new(
        expr,
        functions,
        &bindings.keys().cloned().collect(),
    )
    .map_err(WorldError::NetworkError)?;
    Value::from_scalar(compiled.eval(&bindings).map_err(WorldError::NetworkError)?)
}
