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
    /// A settled winner row supplied by a prior decision stratum.
    Winner {
        decide: String,
        entity_id: String,
        candidate: String,
        revision: u64,
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
            Self::Winner {
                decide,
                entity_id,
                candidate,
                revision,
            } => {
                w.write_uint(6);
                w.write_str(decide);
                w.write_str(entity_id);
                w.write_str(candidate);
                w.write_uint(*revision);
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
            Self::Group { .. } | Self::Distinct { .. } | Self::Winner { .. } => {}
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
            "value": self.value.to_json(),
            "supports_count": self.supports_count,
            "winning": self.winning,
            "disposition": if self.winning { "selected" } else if self.supports_count == 0 { "ineligible" } else { "superseded" },
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
    pub upstream_settlements: Vec<UpstreamSettlementLink>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct UpstreamSettlementLink {
    pub decide: String,
    pub entity_id: String,
    pub candidate: String,
    pub revision: u64,
}

impl DecisionExplanation {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "entity_id": self.entity_id,
            "decide_name": self.decide_name,
            "winning_candidate": self.winning_candidate,
            "value": self.value.as_ref().map(|v| v.to_json()),
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
            "upstream_settlements": self.upstream_settlements.iter().map(|link| serde_json::json!({
                "decide": link.decide,
                "entity_id": link.entity_id,
                "candidate": link.candidate,
                "revision": link.revision,
            })).collect::<Vec<_>>(),
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
    pub per_type: ast::Ty,
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
        compiled_aggregates: Vec<Option<CompiledWorldExpr>>,
        groups: PMap<Vec<Value>, GroupAccumulator>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct GroupAccumulator {
    supports: PSet<DerivationId>,
    int_sums: Vec<i128>,
    decimal_sums: Vec<WideSigned>,
    decimal_scales: Vec<u8>,
    ordered: Vec<BTreeMap<Value, usize>>,
}

/// A signed base-10 integer with no fixed precision ceiling. Decimal group
/// totals use this while a batch is being accumulated, then narrow once the
/// whole group stratum has settled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WideSigned {
    negative: bool,
    digits: Vec<u8>,
}

impl WideSigned {
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
            *self = other.clone();
            return;
        }
        if self.negative == other.negative {
            let mut carry = 0u8;
            for idx in 0..self.digits.len().max(other.digits.len()) {
                if idx == self.digits.len() {
                    self.digits.push(0);
                }
                let sum = self.digits[idx] + other.digits.get(idx).copied().unwrap_or(0) + carry;
                self.digits[idx] = sum % 10;
                carry = sum / 10;
            }
            if carry != 0 {
                self.digits.push(carry);
            }
            return;
        }
        match self.abs_cmp(other) {
            std::cmp::Ordering::Equal => {
                self.digits.clear();
                self.negative = false;
            }
            std::cmp::Ordering::Greater => self.sub_abs(other),
            std::cmp::Ordering::Less => {
                let mut result = other.clone();
                result.sub_abs(self);
                *self = result;
            }
        }
    }

    fn abs_cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.digits
            .len()
            .cmp(&other.digits.len())
            .then_with(|| self.digits.iter().rev().cmp(other.digits.iter().rev()))
    }

    fn sub_abs(&mut self, other: &Self) {
        let mut borrow = 0i8;
        for idx in 0..self.digits.len() {
            let mut digit =
                self.digits[idx] as i8 - other.digits.get(idx).copied().unwrap_or(0) as i8 - borrow;
            if digit < 0 {
                digit += 10;
                borrow = 1;
            } else {
                borrow = 0;
            }
            self.digits[idx] = digit as u8;
        }
        while self.digits.last() == Some(&0) {
            self.digits.pop();
        }
        if self.digits.is_empty() {
            self.negative = false;
        }
    }

    fn to_decimal(&self, scale: u8) -> Option<brix_canon::Decimal> {
        let mut wide = self.clone();
        let mut scale = scale;
        while scale > 0 && wide.digits.first() == Some(&0) {
            wide.digits.remove(0);
            scale -= 1;
        }
        let magnitude = wide.digits.iter().rev().try_fold(0u128, |acc, digit| {
            acc.checked_mul(10)?.checked_add(*digit as u128)
        })?;
        let signed = if wide.negative {
            if magnitude == (i128::MAX as u128) + 1 {
                i128::MIN
            } else {
                -i128::try_from(magnitude).ok()?
            }
        } else {
            i128::try_from(magnitude).ok()?
        };
        Some(brix_canon::Decimal::new(signed, scale))
    }
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
    virtual_relations: BTreeSet<String>,
    winner_versions: PMap<(String, WorldKey), u64>,
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
        let mut virtual_relations = BTreeSet::new();

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
                OperatorNode::DecisionRows { decide, schema } => {
                    virtual_relations.insert(decide.clone());
                    let key_field = match schema {
                        ast::Ty::Record(fields) => {
                            fields.first().map(|f| f.name.clone()).unwrap_or_default()
                        }
                        _ => String::new(),
                    };
                    relation_subscribers
                        .entry(decide.clone())
                        .or_default()
                        .push(op_id);
                    // The virtual relation has the same maintained scan shape
                    // as an input relation, but is populated only by the
                    // settlement stage (never by source batch operations).
                    operator_states.insert(
                        idx,
                        OperatorState::Scan {
                            relation: decide.clone(),
                            key_fields: vec![key_field],
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
                            compiled_aggregates: vec![None; projections.len()],
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
            virtual_relations,
            winner_versions: PMap::new(),
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
        fresh.winner_versions = PMap::new();
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
                    compiled_aggregates,
                    ..
                } => OperatorState::GroupedCount {
                    input: *input,
                    group_keys: group_keys.clone(),
                    compiled_group_keys: compiled_group_keys.clone(),
                    projections: projections.clone(),
                    compiled_aggregates: compiled_aggregates.clone(),
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
                    input,
                    group_keys,
                    projections,
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
                                | GroupProjection::Max(expr) => Some(expr),
                                GroupProjection::Key(_) | GroupProjection::Count => None,
                            };
                            expr.map(|e| program_env.compile_expr(e, &bindings))
                                .transpose()
                                .map_err(WorldError::NetworkError)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if let Some(OperatorState::GroupedCount {
                        compiled_group_keys: ref mut cgk,
                        compiled_aggregates: ref mut ca,
                        ..
                    }) = network.operator_states.get_mut(&idx)
                    {
                        *cgk = compiled_group_keys;
                        *ca = compiled_aggregates;
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
                let per_type = op_schemas[op_id.0]
                    .get(&format!("{}.{per_field}", decl.binder))
                    .or_else(|| op_schemas[op_id.0].get(&per_field))
                    .cloned()
                    .ok_or_else(|| {
                        WorldError::NetworkError(format!(
                            "decide '{qname}' per field '{per_field}' has no resolved type"
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
                    per_type,
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
        self.apply_ops_staged_inner(ops, limits, false)
    }

    fn apply_ops_staged_inner(
        &mut self,
        ops: &[WorldBatchOp],
        limits: &NetworkLimits,
        allow_winner_rows: bool,
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
            let is_winner_relation = self.virtual_relations.contains(rel_name);
            if is_winner_relation && !allow_winner_rows {
                return Err(WorldError::UnknownRelation(rel_name.to_string()));
            }
            let mut base_rel = self.base_relations.get_mut(rel_name);
            if base_rel.is_none() && !is_winner_relation {
                return Err(WorldError::UnknownRelation(rel_name.to_string()));
            }

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
                    key_fields,
                    schema,
                    records,
                    ..
                } = scan_state
                {
                    match op {
                        WorldBatchOp::Upsert { key, tuple, .. } => {
                            if let Some(base_rel) = base_rel.as_mut() {
                                base_rel.insert(key.clone(), tuple.clone());
                            }
                            let derivation = if is_winner_relation {
                                let entity_id = Value::from_typed_bytes(
                                    key.as_bytes(),
                                    &schema_key_type(schema, &key_fields),
                                )?
                                .to_string();
                                let candidate = TupleRecord::from_tuple(tuple)?
                                    .get_str("candidate")
                                    .ok_or_else(|| {
                                        WorldError::NetworkError(
                                            "winner relation row missing candidate".into(),
                                        )
                                    })?
                                    .to_string();
                                DerivationId::Winner {
                                    decide: relation.clone(),
                                    entity_id,
                                    candidate,
                                    revision: self.current_revision + 1,
                                }
                            } else {
                                DerivationId::Base {
                                    relation: relation.clone(),
                                    key: key.clone(),
                                }
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
                                let empty_schemas = BTreeMap::new();
                                let schemas = self
                                    .program_env
                                    .as_ref()
                                    .map(|pe| pe.schemas.as_ref())
                                    .unwrap_or(&empty_schemas);
                                intermediate.insert(
                                    field.name.clone(),
                                    Value::from_typed_bytes_with_schemas(
                                        bytes, &field.ty, schemas,
                                    )?,
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
                                    derivation: if is_winner_relation {
                                        winner_derivation_from_row(
                                            relation,
                                            old_inter,
                                            &key_fields,
                                            self.winner_versions
                                                .get(&(relation.clone(), key.clone()))
                                                .copied()
                                                .unwrap_or(self.current_revision),
                                        )?
                                    } else {
                                        derivation.clone()
                                    },
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
                            if is_winner_relation {
                                self.winner_versions.insert(
                                    (relation.clone(), key.clone()),
                                    self.current_revision + 1,
                                );
                            }
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
                            if let Some(base_rel) = base_rel.as_mut() {
                                base_rel.remove(key);
                            }
                            if let Some((_, old_inter)) = records.remove(key) {
                                let derivation = if is_winner_relation {
                                    let entity_id = old_inter
                                        .get(&key_fields[0])
                                        .map(ToString::to_string)
                                        .unwrap_or_default();
                                    let candidate = old_inter
                                        .get("candidate")
                                        .and_then(Value::as_str)
                                        .ok_or_else(|| {
                                            WorldError::NetworkError(
                                                "winner relation row missing candidate".into(),
                                            )
                                        })?
                                        .to_string();
                                    DerivationId::Winner {
                                        decide: relation.clone(),
                                        entity_id,
                                        candidate,
                                        revision: self
                                            .winner_versions
                                            .get(&(relation.clone(), key.clone()))
                                            .copied()
                                            .unwrap_or(self.current_revision),
                                    }
                                } else {
                                    DerivationId::Base {
                                        relation: relation.clone(),
                                        key: key.clone(),
                                    }
                                };
                                let ret_delta = TupleDelta::Retract {
                                    tuple: old_inter,
                                    derivation,
                                };
                                if is_winner_relation {
                                    self.winner_versions
                                        .remove(&(relation.clone(), key.clone()));
                                }
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
        let mut deferred_groups = BTreeSet::new();

        while let Some(op_id) = ready_operators.pop_front() {
            in_ready.remove(&op_id);
            let op_idx = op_id.0;
            let is_group_operator =
                matches!(self.dag.nodes[op_idx], OperatorNode::GroupedCount { .. });
            if is_group_operator && has_pending_ancestor(op_id, &self.dag, &operator_queues) {
                deferred_groups.insert(op_id);
                continue;
            }
            let mut work_in_slice: usize = 0;
            let mut out_deltas = Vec::new();
            let mut grouped_before: BTreeMap<Vec<Value>, Option<IntermediateTuple>> =
                BTreeMap::new();
            let slice_quantum = if is_group_operator {
                usize::MAX
            } else {
                quantum
            };

            while work_in_slice < slice_quantum {
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
                                    DerivationId::Base { relation, .. }
                                    | DerivationId::Winner {
                                        decide: relation, ..
                                    } => match &self.dag.nodes[left_op.0] {
                                        OperatorNode::Scan { relation: r, .. } => r == relation,
                                        OperatorNode::DecisionRows { decide, .. } => {
                                            decide == relation
                                        }
                                        _ => false,
                                    },
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
                        compiled_aggregates,
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

                            let bindings = scalar_bindings(&tuple)?;
                            let mut aggregate_values =
                                Vec::with_capacity(compiled_aggregates.len());
                            for compiled in compiled_aggregates {
                                if let Some(compiled) = compiled {
                                    diagnostics.expressions_evaluated += 1;
                                    if limits
                                        .max_expressions
                                        .is_some_and(|max| diagnostics.expressions_evaluated > max)
                                    {
                                        return Err(WorldError::BudgetExhausted);
                                    }
                                    aggregate_values.push(Some(Value::from_scalar(
                                        compiled
                                            .eval(&bindings)
                                            .map_err(WorldError::NetworkError)?,
                                    )?));
                                } else {
                                    aggregate_values.push(None);
                                }
                            }
                            let existing = groups.get(&key_vals).cloned();
                            if !is_ins && existing.is_none() {
                                continue;
                            }
                            grouped_before.entry(key_vals.clone()).or_insert_with(|| {
                                existing.as_ref().map(|state| {
                                    build_grouped_tuple(projections, &key_vals, state)
                                        .expect("previously emitted group is representable")
                                })
                            });
                            let mut state = existing
                                .clone()
                                .unwrap_or_else(|| empty_group_accumulator(projections));
                            if state.supports.contains(&deriv) == is_ins {
                                continue;
                            }
                            if is_ins {
                                state.supports.insert(deriv);
                            } else {
                                state.supports.remove(&deriv);
                            }
                            for (idx, ((_, projection), value)) in
                                projections.iter().zip(aggregate_values).enumerate()
                            {
                                let Some(value) = value else { continue };
                                match projection {
                                    GroupProjection::SumInt(_) => {
                                        let Value::Int(value) = value else {
                                            return Err(WorldError::NetworkError(
                                                "grouped Int sum received non-Int".into(),
                                            ));
                                        };
                                        let amount = i128::from(value);
                                        state.int_sums[idx] = if is_ins {
                                            state.int_sums[idx].checked_add(amount)
                                        } else {
                                            state.int_sums[idx].checked_sub(amount)
                                        }
                                        .ok_or_else(|| {
                                            WorldError::NetworkError(
                                                "grouped Int sum accumulator overflow".into(),
                                            )
                                        })?;
                                    }
                                    GroupProjection::SumDecimal(_) => {
                                        let Value::Decimal(value) = value else {
                                            return Err(WorldError::NetworkError(
                                                "grouped Decimal sum received non-Decimal".into(),
                                            ));
                                        };
                                        let value_scale = value.scale();
                                        if value_scale > state.decimal_scales[idx] {
                                            state.decimal_sums[idx]
                                                .mul_pow10(value_scale - state.decimal_scales[idx]);
                                            state.decimal_scales[idx] = value_scale;
                                        }
                                        let mut amount = WideSigned::from_i128(value.unscaled());
                                        amount.mul_pow10(state.decimal_scales[idx] - value_scale);
                                        if !is_ins {
                                            amount.negative = !amount.negative;
                                        }
                                        state.decimal_sums[idx].add_assign(&amount);
                                    }
                                    GroupProjection::Min(_) | GroupProjection::Max(_) => {
                                        let multiset = &mut state.ordered[idx];
                                        if is_ins {
                                            *multiset.entry(value).or_default() += 1;
                                        } else if let Some(count) = multiset.get_mut(&value) {
                                            *count -= 1;
                                            if *count == 0 {
                                                multiset.remove(&value);
                                            }
                                        }
                                    }
                                    GroupProjection::Key(_) | GroupProjection::Count => {}
                                }
                            }
                            if state.supports.is_empty() {
                                groups.remove(&key_vals);
                            } else {
                                groups.insert(key_vals, state);
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            }

            if is_group_operator {
                if let Some(OperatorState::GroupedCount {
                    projections,
                    groups,
                    ..
                }) = self.operator_states.get_mut(&op_idx)
                {
                    for (group_key, old_tuple) in grouped_before {
                        let new_tuple = groups
                            .get(&group_key)
                            .map(|state| build_grouped_tuple(projections, &group_key, state))
                            .transpose()?;
                        if old_tuple == new_tuple {
                            continue;
                        }
                        let derivation = DerivationId::Group {
                            op: op_id,
                            group_key,
                        };
                        if let Some(tuple) = old_tuple {
                            out_deltas.push(TupleDelta::Retract {
                                tuple,
                                derivation: derivation.clone(),
                            });
                        }
                        if let Some(tuple) = new_tuple {
                            out_deltas.push(TupleDelta::Insert { tuple, derivation });
                        }
                    }
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

            let newly_ready: Vec<_> = deferred_groups
                .iter()
                .copied()
                .filter(|group| !has_pending_ancestor(*group, &self.dag, &operator_queues))
                .collect();
            for group in newly_ready {
                deferred_groups.remove(&group);
                if operator_queues.get(&group).is_some_and(|q| !q.is_empty())
                    && in_ready.insert(group)
                {
                    ready_operators.push_back(group);
                }
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
        let winner_ops = self.winner_row_ops(&report)?;
        if !winner_ops.is_empty() {
            let prior_revision = self.current_revision;
            let used_work = report
                .diagnostics
                .operator_work
                .values()
                .copied()
                .sum::<u64>();
            let used_matches = report
                .diagnostics
                .join_fanout
                .values()
                .copied()
                .sum::<u64>();
            let used_expressions = report.diagnostics.expressions_evaluated;
            let child_limits = NetworkLimits {
                max_work: limits.max_work.map(|max| max.saturating_sub(used_work)),
                max_matches: limits
                    .max_matches
                    .map(|max| max.saturating_sub(used_matches)),
                max_expressions: limits
                    .max_expressions
                    .map(|max| max.saturating_sub(used_expressions)),
                max_queued_deltas: limits.max_queued_deltas,
            };
            let nested = self.apply_ops_staged_inner(&winner_ops, &child_limits, true)?;
            self.current_revision = prior_revision;
            merge_stage_report(&mut report, nested);
        }
        self.current_revision += 1;
        Ok(report)
    }

    fn winner_row_ops(&self, report: &NetworkDeltaReport) -> Result<Vec<WorldBatchOp>, WorldError> {
        let mut changed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (decide, entities) in &report.settlements {
            changed
                .entry(decide.clone())
                .or_default()
                .extend(entities.keys().cloned());
        }
        for (decide, entities) in &report.removed_settlements {
            changed
                .entry(decide.clone())
                .or_default()
                .extend(entities.iter().cloned());
        }
        let mut ops = Vec::new();
        for (name, entities) in changed {
            let Some(decide) = self.decides.get(&name) else {
                continue;
            };
            for entity_id in entities {
                let entity_value = Value::from_typed_bytes(entity_id.as_bytes(), &decide.per_type)?;
                let key = WorldKey::new(entity_value.to_bytes());
                if let Some(winner) = self.get_settlement(&name, &entity_id) {
                    let priority = i64::try_from(winner.priority).map_err(|_| {
                        WorldError::NetworkError(format!(
                            "winner priority for '{name}' exceeds the winner row Int range"
                        ))
                    })?;
                    let mut row = TupleRecord::new();
                    row.set(&decide.per_field, entity_value.to_bytes());
                    row.set("candidate", Value::Str(winner.candidate_name).to_bytes());
                    row.set("priority", Value::Int(priority).to_bytes());
                    row.set("value", winner.value.to_bytes());
                    ops.push(WorldBatchOp::Upsert {
                        relation: name.clone(),
                        key,
                        tuple: row.to_tuple(),
                    });
                } else {
                    ops.push(WorldBatchOp::Remove {
                        relation: name.clone(),
                        key,
                    });
                }
            }
        }
        Ok(ops)
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

        // A scratch rebuild represents the current base state in one synthetic
        // revision, while the incremental network retains each winner's actual
        // settlement revision. Compare support topology and winner identity,
        // while leaving the stored provenance links untouched.
        let normalize_supports =
            |supports: &BTreeMap<OperatorId, BTreeMap<TupleRecord, BTreeSet<DerivationId>>>| {
                supports
                    .iter()
                    .map(|(op, tuples)| {
                        (
                            *op,
                            tuples
                                .iter()
                                .map(|(tuple, derivations)| {
                                    (
                                        tuple.clone(),
                                        derivations
                                            .iter()
                                            .map(normalize_derivation_revision)
                                            .collect::<BTreeSet<_>>(),
                                    )
                                })
                                .collect::<BTreeMap<_, _>>(),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            };
        if normalize_supports(&incremental_state.distinct_supports)
            != normalize_supports(&scratch_state.distinct_supports)
        {
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
            DerivationId::Winner {
                decide,
                entity_id,
                candidate,
                revision,
            } => {
                if let Some(entry) = self
                    .candidate_frontier
                    .get(decide)
                    .and_then(|entities| entities.get(entity_id))
                    .and_then(|candidates| candidates.get(candidate))
                {
                    for parent in &entry.supports {
                        self.collect_base_facts_for_derivation(parent, out);
                    }
                }
                let _ = revision;
            }
        }
    }

    fn collect_upstream_settlements_for_derivation(
        &self,
        deriv: &DerivationId,
        out: &mut BTreeSet<UpstreamSettlementLink>,
    ) {
        match deriv {
            DerivationId::Base { .. } => {}
            DerivationId::Unary { parent, .. } => {
                self.collect_upstream_settlements_for_derivation(parent, out);
            }
            DerivationId::Join { left, right, .. } => {
                self.collect_upstream_settlements_for_derivation(left, out);
                self.collect_upstream_settlements_for_derivation(right, out);
            }
            DerivationId::Distinct { op, tuple_key } => {
                if let Some(OperatorState::Distinct { supports, .. }) =
                    self.operator_states.get(&op.0)
                {
                    for (tuple, parents) in supports {
                        if tuple.to_tuple_record().to_tuple().0 == *tuple_key {
                            for parent in parents {
                                self.collect_upstream_settlements_for_derivation(parent, out);
                            }
                        }
                    }
                }
            }
            DerivationId::Group { op, group_key } => {
                if let Some(OperatorState::GroupedCount { groups, .. }) =
                    self.operator_states.get(&op.0)
                {
                    if let Some(parents) = groups.get(group_key) {
                        for parent in &parents.supports {
                            self.collect_upstream_settlements_for_derivation(parent, out);
                        }
                    }
                }
            }
            DerivationId::Winner {
                decide,
                entity_id,
                candidate,
                revision,
            } => {
                out.insert(UpstreamSettlementLink {
                    decide: decide.clone(),
                    entity_id: entity_id.clone(),
                    candidate: candidate.clone(),
                    revision: *revision,
                });
                if let Some(entry) = self
                    .candidate_frontier
                    .get(decide)
                    .and_then(|entities| entities.get(entity_id))
                    .and_then(|candidates| candidates.get(candidate))
                {
                    for parent in &entry.supports {
                        self.collect_upstream_settlements_for_derivation(parent, out);
                    }
                }
            }
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
        let decide = self.decides.get(decide_name)?;
        let decide_frontier = self.candidate_frontier.get(decide_name);
        let entity_candidates = decide_frontier.and_then(|df| df.get(entity_id));

        let settlement = self.get_settlement(decide_name, entity_id);
        if entity_candidates.is_none() && settlement.is_none() {
            return None;
        }

        let winning_name = settlement.as_ref().map(|s| &s.candidate_name);

        let mut candidates = Vec::new();
        let mut contributing_base = BTreeSet::new();
        let mut upstream_settlements = BTreeSet::new();

        if let Some(entity_candidates) = entity_candidates {
            for (cand_name, entry) in entity_candidates {
                let winning = winning_name == Some(cand_name);
                for deriv in &entry.supports {
                    self.collect_base_facts_for_derivation(deriv, &mut contributing_base);
                    self.collect_upstream_settlements_for_derivation(
                        deriv,
                        &mut upstream_settlements,
                    );
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
        }

        // Also explain proposals that were declared in the decide block but were NOT admitted
        // (e.g. guard evaluated to false or no matching rows).
        for prop in &decide.proposals {
            if !candidates.iter().any(|c| c.name == prop.name) {
                candidates.push(CandidateExplanation {
                    name: prop.name.clone(),
                    priority: prop.priority,
                    phase: 0,
                    value: Value::Null,
                    supports_count: 0,
                    winning: false,
                });
            }
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
            upstream_settlements: upstream_settlements.into_iter().collect(),
        })
    }
}

fn normalize_derivation_revision(derivation: &DerivationId) -> DerivationId {
    match derivation {
        DerivationId::Winner {
            decide,
            entity_id,
            candidate,
            ..
        } => DerivationId::Winner {
            decide: decide.clone(),
            entity_id: entity_id.clone(),
            candidate: candidate.clone(),
            revision: 0,
        },
        DerivationId::Unary { op, parent } => DerivationId::Unary {
            op: *op,
            parent: Box::new(normalize_derivation_revision(parent)),
        },
        DerivationId::Join { op, left, right } => DerivationId::Join {
            op: *op,
            left: Box::new(normalize_derivation_revision(left)),
            right: Box::new(normalize_derivation_revision(right)),
        },
        other => other.clone(),
    }
}

fn build_grouped_tuple(
    projections: &[(String, GroupProjection)],
    key_vals: &[Value],
    state: &GroupAccumulator,
) -> Result<IntermediateTuple, WorldError> {
    let mut tuple = IntermediateTuple::new();
    for (idx, (name, proj)) in projections.iter().enumerate() {
        match proj {
            GroupProjection::Key(idx) => {
                let v = key_vals.get(*idx).cloned().unwrap_or(Value::Null);
                tuple.insert(name, v);
            }
            GroupProjection::Count => {
                tuple.insert(name, Value::Int(state.supports.len() as i64));
            }
            GroupProjection::SumInt(_) => {
                tuple.insert(
                    name,
                    Value::Int(i64::try_from(state.int_sums[idx]).map_err(|_| {
                        WorldError::NetworkError("grouped Int sum overflow".into())
                    })?),
                );
            }
            GroupProjection::SumDecimal(_) => {
                let value = state.decimal_sums[idx]
                    .to_decimal(state.decimal_scales[idx])
                    .ok_or_else(|| {
                        WorldError::NetworkError("grouped Decimal sum overflow".into())
                    })?;
                tuple.insert(name, Value::Decimal(value));
            }
            GroupProjection::Min(_) => {
                tuple.insert(
                    name,
                    state.ordered[idx]
                        .first_key_value()
                        .map(|(value, _)| value.clone())
                        .unwrap_or(Value::Null),
                );
            }
            GroupProjection::Max(_) => {
                tuple.insert(
                    name,
                    state.ordered[idx]
                        .last_key_value()
                        .map(|(value, _)| value.clone())
                        .unwrap_or(Value::Null),
                );
            }
        }
    }
    Ok(tuple)
}

fn has_pending_ancestor(
    operator: OperatorId,
    dag: &RelationDag,
    queues: &BTreeMap<OperatorId, std::collections::VecDeque<OperatorWorkItem>>,
) -> bool {
    let mut pending = Vec::new();
    match &dag.nodes[operator.0] {
        OperatorNode::Bind { input, .. }
        | OperatorNode::Filter { input, .. }
        | OperatorNode::Project { input, .. }
        | OperatorNode::Distinct { input }
        | OperatorNode::GroupedCount { input, .. } => pending.push(*input),
        OperatorNode::EquiJoin { left, right, .. } => {
            pending.push(*left);
            pending.push(*right);
        }
        OperatorNode::Scan { .. } | OperatorNode::DecisionRows { .. } => return false,
    }
    let mut visited = BTreeSet::new();
    while let Some(ancestor) = pending.pop() {
        if !visited.insert(ancestor) {
            continue;
        }
        if queues.get(&ancestor).is_some_and(|queue| !queue.is_empty()) {
            return true;
        }
        match &dag.nodes[ancestor.0] {
            OperatorNode::Bind { input, .. }
            | OperatorNode::Filter { input, .. }
            | OperatorNode::Project { input, .. }
            | OperatorNode::Distinct { input }
            | OperatorNode::GroupedCount { input, .. } => pending.push(*input),
            OperatorNode::EquiJoin { left, right, .. } => {
                pending.push(*left);
                pending.push(*right);
            }
            OperatorNode::Scan { .. } | OperatorNode::DecisionRows { .. } => {}
        }
    }
    false
}

fn empty_group_accumulator(projections: &[(String, GroupProjection)]) -> GroupAccumulator {
    let mut int_sums = Vec::with_capacity(projections.len());
    let mut decimal_sums = Vec::with_capacity(projections.len());
    let mut decimal_scales = Vec::with_capacity(projections.len());
    let mut ordered = Vec::with_capacity(projections.len());
    for _ in projections {
        int_sums.push(0);
        decimal_sums.push(WideSigned::default());
        decimal_scales.push(0);
        ordered.push(BTreeMap::new());
    }
    GroupAccumulator {
        supports: PSet::new(),
        int_sums,
        decimal_sums,
        decimal_scales,
        ordered,
    }
}

fn schema_key_type(schema: &ast::Ty, key_fields: &[String]) -> ast::Ty {
    let Some(key) = key_fields.first() else {
        return ast::Ty::Named("Str".into());
    };
    match schema {
        ast::Ty::Record(fields) => fields
            .iter()
            .find(|field| &field.name == key)
            .map(|field| field.ty.clone())
            .unwrap_or_else(|| ast::Ty::Named("Str".into())),
        _ => ast::Ty::Named("Str".into()),
    }
}

fn winner_derivation_from_row(
    decide: &str,
    row: &IntermediateTuple,
    key_fields: &[String],
    revision: u64,
) -> Result<DerivationId, WorldError> {
    let key_field = key_fields.first().ok_or_else(|| {
        WorldError::NetworkError(format!(
            "winner relation '{decide}' has no entity key field"
        ))
    })?;
    let entity_id = row.get(key_field).map(ToString::to_string).ok_or_else(|| {
        WorldError::NetworkError(format!(
            "winner relation '{decide}' has no '{key_field}' value"
        ))
    })?;
    let candidate = row
        .get("candidate")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            WorldError::NetworkError(format!("winner relation '{decide}' has no candidate value"))
        })?
        .to_string();
    Ok(DerivationId::Winner {
        decide: decide.to_string(),
        entity_id,
        candidate,
        revision,
    })
}

fn merge_stage_report(into: &mut NetworkDeltaReport, next: NetworkDeltaReport) {
    into.intermediate_deltas_count += next.intermediate_deltas_count;
    into.derived_tuples_inserted += next.derived_tuples_inserted;
    into.derived_tuples_retracted += next.derived_tuples_retracted;
    into.candidates_inserted += next.candidates_inserted;
    into.candidates_retracted += next.candidates_retracted;
    for (decide, mut entities) in next.settlements {
        into.settlements
            .entry(decide)
            .or_default()
            .append(&mut entities);
    }
    for (decide, entities) in next.removed_settlements {
        into.removed_settlements
            .entry(decide)
            .or_default()
            .extend(entities);
    }
    for (op, count) in next.diagnostics.operator_work {
        *into.diagnostics.operator_work.entry(op).or_default() += count;
    }
    for (relation, count) in next.diagnostics.relation_deltas {
        *into
            .diagnostics
            .relation_deltas
            .entry(relation)
            .or_default() += count;
    }
    for (op, count) in next.diagnostics.join_fanout {
        *into.diagnostics.join_fanout.entry(op).or_default() += count;
    }
    for (key, count) in next.diagnostics.hot_join_keys {
        *into.diagnostics.hot_join_keys.entry(key).or_default() += count;
    }
    into.diagnostics.peak_queue_size = into
        .diagnostics
        .peak_queue_size
        .max(next.diagnostics.peak_queue_size);
    into.diagnostics.expressions_evaluated += next.diagnostics.expressions_evaluated;
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
