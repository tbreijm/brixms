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

use super::compact_deriv::{leaf_counts, CompactDeriv, DRoot, Derivation, Leaf};
use super::network_state::{NetworkStateHeader, OperatorRoots, ValidityKey};
use super::persistent::{PMap, PSet};
use super::state_codec::{
    CandKey, CandidateHead, CandidateRec, FrontierBucket, FrontierKey, JoinEntry, StateCodec,
    StateCodecError, SubRoot, SupportItem, SupportRef, ValueList,
};
use super::state_store::{PageCursor, SMap, StateError, StateStore, DEFAULT_NODE_CACHE_BYTES};
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

/// The typed, injective key identifying a distinct tuple inside
/// [`DerivationId::Distinct`]; decodable back to the tuple with
/// [`StateCodec::state_from_bytes`].
pub fn distinct_key(tuple: &IntermediateTuple) -> Vec<u8> {
    tuple.state_bytes()
}

fn derivation_provenance_key(leaves: &[Leaf]) -> Value {
    let mut parts = Vec::with_capacity(leaves.len());
    for leaf in leaves {
        match leaf {
            Leaf::Base(k) => parts.push(k.to_hex()),
            Leaf::Distinct(k) => parts.push(WorldKey::new(k.clone()).to_hex()),
            Leaf::Group(vs) => {
                let mut w = CanonWriter::new();
                w.write_uint(vs.len() as u64);
                for v in vs {
                    v.state_write(&mut w);
                }
                parts.push(WorldKey::new(w.finish()).to_hex());
            }
        }
    }
    Value::Str(parts.join(":"))
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
    ///
    /// `tuple_key` is [`distinct_key`] of the tuple: the *typed* injective state encoding.
    /// (Before R0 M2 part 2 it was the Display-based `Value::to_bytes` encoding, under which
    /// `Int(1)` and `Str("1")` -- likewise `Bool(true)`/`Str("true")`, `Null`/`Str("null")` --
    /// produced the same key. The two now stay distinct; that is an intentional semantic
    /// change, only observable when a column holds mixed dynamic types.)
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
        derivation: Derivation,
    },
    Retract {
        tuple: IntermediateTuple,
        derivation: Derivation,
    },
}

impl TupleDelta {
    pub fn tuple(&self) -> &IntermediateTuple {
        match self {
            Self::Insert { tuple, .. } | Self::Retract { tuple, .. } => tuple,
        }
    }

    pub fn derivation(&self) -> &Derivation {
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
    SideDelta {
        delta: TupleDelta,
        is_left: bool,
    },
    JoinContinuation {
        delta: TupleDelta,
        is_left: bool,
        join_key: Vec<Value>,
        /// Snapshot of the opposite side's inner map for `join_key`.
        matches: SMap<Digest, JoinEntry>,
        cursor: Option<PageCursor<Digest>>,
        remaining: usize,
    },
}

#[inline]
fn work_item_queued_count(item: &OperatorWorkItem) -> usize {
    match item {
        OperatorWorkItem::Delta(_) | OperatorWorkItem::SideDelta { .. } => 1,
        OperatorWorkItem::JoinContinuation { remaining, .. } => (*remaining).max(1),
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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct UpstreamSettlementLink {
    pub decide: String,
    pub entity_id: String,
    pub candidate: String,
    pub revision: u64,
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
            "upstream_settlements": self.upstream_settlements.iter().map(|s| serde_json::json!({
                "decide": s.decide,
                "entity_id": s.entity_id,
                "candidate": s.candidate,
                "revision": s.revision,
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

/// One side of a join index: join key -> inner map of the tuples (with compact
/// derivations) currently supporting that key.
///
/// `outer` is the persisted index (key -> inner [`SubRoot`]); `hot` holds the inner maps
/// modified (and so not yet reachable from the store) since the last dehydration.
#[derive(Clone, Debug, Default)]
pub struct JoinSide {
    pub outer: SMap<ValueList, SubRoot>,
    pub hot: PMap<ValueList, SMap<Digest, JoinEntry>>,
}

impl JoinSide {
    /// The inner map for `key`, from `hot` if modified, else from the persisted index.
    fn read(
        &self,
        key: &ValueList,
        store: &StateStore,
    ) -> Result<Option<SMap<Digest, JoinEntry>>, WorldError> {
        if let Some(m) = self.hot.get(key) {
            return Ok(Some(m.clone()));
        }
        Ok(self.outer.get(key, store)?.map(SMap::from_sub_root))
    }

    /// Record the new inner map for `key`; an empty map removes the key everywhere.
    fn write(
        &mut self,
        key: ValueList,
        inner: SMap<Digest, JoinEntry>,
        store: &StateStore,
    ) -> Result<(), WorldError> {
        if inner.is_empty() {
            self.hot.remove(&key);
            self.outer = self.outer.remove(&key, store)?;
        } else {
            self.outer = self.outer.upsert(key.clone(), inner.sub_root(), store)?;
            self.hot.insert(key, inner);
        }
        Ok(())
    }

    fn persist(&self, store: &StateStore) -> usize {
        let mut n = 0;
        for inner in self.hot.values() {
            n += inner.persist(store);
        }
        n + self.outer.persist(store)
    }

    fn dehydrated(&self, store: &StateStore) -> Result<Self, WorldError> {
        Ok(Self {
            outer: self.outer.dehydrate(store)?,
            hot: PMap::new(),
        })
    }
}

fn indexed_side_apply(
    side: &mut JoinSide,
    key: ValueList,
    delta: &TupleDelta,
    store: &StateStore,
) -> Result<(usize, usize), WorldError> {
    let (tuple, derivation) = match delta {
        TupleDelta::Insert { tuple, derivation } | TupleDelta::Retract { tuple, derivation } => {
            (tuple, derivation)
        }
    };
    let id = derivation.c.digest();
    let existing = side.read(&key, store)?;
    let old = existing.as_ref().map_or(0, SMap::len);
    let inner = if delta.is_insert() {
        existing.unwrap_or_default().upsert(
            id,
            JoinEntry {
                tuple: tuple.clone(),
                deriv: derivation.c.clone(),
            },
            store,
        )?
    } else if let Some(existing) = existing {
        existing.remove(&id, store)?
    } else {
        SMap::new()
    };
    let new = inner.len();
    side.write(key, inner, store)?;
    Ok((old, new))
}

fn indexed_side_rows(
    side: &JoinSide,
    key: &ValueList,
    store: &StateStore,
    remaining_rows: Option<u64>,
) -> Result<Vec<JoinEntry>, WorldError> {
    let Some(inner) = side.read(key, store)? else {
        return Ok(Vec::new());
    };
    // Refuse an over-budget fanout before allocating or decoding its bucket.
    if remaining_rows.is_some_and(|max| inner.len() as u64 > max) {
        return Err(WorldError::BudgetExhausted);
    }
    let mut rows = Vec::with_capacity(inner.len());
    let mut cursor = None;
    loop {
        let (page, next) = inner.page(cursor.as_ref(), 256, store)?;
        rows.extend(page.into_iter().map(|(_, entry)| entry));
        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(rows)
}

fn tuple_index_key(
    tuple: &IntermediateTuple,
    fields: &[FieldRef],
) -> Result<ValueList, WorldError> {
    Ok(ValueList(
        fields
            .iter()
            .map(|field| {
                tuple
                    .get_qualified(&field.binding, &field.field)
                    .cloned()
                    .ok_or_else(|| {
                        WorldError::NetworkError(format!(
                            "missing indexed field {}.{}",
                            field.binding, field.field
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

// Check counts before materializing a row; even a corrupt/high-fanout bucket
// cannot turn duplicate-key validation into an unbounded scan.
fn disjoint_union_tuple(
    left: &mut JoinSide,
    right: &mut JoinSide,
    key: &ValueList,
    store: &StateStore,
) -> Result<Option<IntermediateTuple>, WorldError> {
    let left_count = left.read(key, store)?.map_or(0, |m| m.len());
    let right_count = right.read(key, store)?.map_or(0, |m| m.len());
    if left_count > 1 || right_count > 1 || (left_count == 1 && right_count == 1) {
        return Err(WorldError::NetworkError(format!(
            "duplicate key in disjoint union: {:?}",
            key.0
        )));
    }
    let side = if left_count == 1 {
        left
    } else if right_count == 1 {
        right
    } else {
        return Ok(None);
    };
    Ok(indexed_side_rows(side, key, store, Some(1))?
        .into_iter()
        .next()
        .map(|entry| entry.tuple))
}

fn parent_count_tuple(
    key: &Value,
    output_key: &str,
    output_count: &str,
    count: usize,
) -> IntermediateTuple {
    let mut tuple = IntermediateTuple::new();
    tuple.insert(output_key, key.clone());
    tuple.insert(output_count, Value::Int(count as i64));
    tuple
}

/// The supports of one key, as read from a [`SupportIndex`].
pub(crate) enum Supports<I: SupportItem> {
    None,
    One(I),
    Many(SMap<Digest, I::Val>),
}

impl<I: SupportItem> Supports<I> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Supports::None => 0,
            Supports::One(_) => 1,
            Supports::Many(set) => set.len(),
        }
    }

    pub(crate) fn members(&self, store: &StateStore) -> Result<Vec<I>, WorldError> {
        match self {
            Supports::None => Ok(Vec::new()),
            Supports::One(d) => Ok(vec![d.clone()]),
            Supports::Many(set) => {
                let mut out = Vec::with_capacity(set.len());
                let mut cursor = None;
                loop {
                    let (rows, next) = set.page(cursor.as_ref(), 256, store)?;
                    out.extend(rows.into_iter().map(|(k, v)| I::from_parts(k, v)));
                    match next {
                        Some(c) => cursor = Some(c),
                        None => break,
                    }
                }
                Ok(out)
            }
        }
    }
}

/// A persisted "key -> set of supporting derivations" index (Distinct, GroupedCount).
///
/// `outer` maps each key to its [`SupportRef`]: a single support inline, two or more as the
/// [`SubRoot`] of an inner `SMap<id, Val>` (canonical: one representation per support
/// set). `hot` holds the inner sets of `Sub` keys modified (and so not yet reachable from
/// the store) since the last dehydration. `I` the stored support item (the compact derivation for Distinct, its digest for
/// GroupedCount, which never needs to expand its supports).
#[derive(Clone, Debug)]
pub struct SupportIndex<K, I>
where
    K: StateCodec + Ord + Clone + 'static + Send + Sync,
    I: SupportItem,
{
    pub outer: SMap<K, SupportRef<I>>,
    pub hot: PMap<K, SMap<Digest, I::Val>>,
}

impl<K, I> Default for SupportIndex<K, I>
where
    K: StateCodec + Ord + Clone + 'static + Send + Sync,
    I: SupportItem,
{
    fn default() -> Self {
        Self {
            outer: SMap::new(),
            hot: PMap::new(),
        }
    }
}

impl<K, I> SupportIndex<K, I>
where
    K: StateCodec + Ord + Clone + 'static + Send + Sync,
    I: SupportItem,
{
    /// The supports for `key`, from `hot` if modified, else from the persisted index.
    pub(crate) fn read(&self, key: &K, store: &StateStore) -> Result<Supports<I>, WorldError> {
        if let Some(m) = self.hot.get(key) {
            return Ok(Supports::Many(m.clone()));
        }
        Ok(match self.outer.get(key, store)? {
            None => Supports::None,
            Some(SupportRef::Inline(d)) => Supports::One(d),
            Some(SupportRef::Sub(s)) => Supports::Many(SMap::from_sub_root(s)),
        })
    }

    /// Store a set of two or more supports.
    fn write_many(
        &mut self,
        key: K,
        inner: SMap<Digest, I::Val>,
        store: &StateStore,
    ) -> Result<(), WorldError> {
        debug_assert!(inner.len() >= 2);
        self.outer = self
            .outer
            .upsert(key.clone(), SupportRef::Sub(inner.sub_root()), store)?;
        self.hot.insert(key, inner);
        Ok(())
    }

    /// Add support `item` for `key`; returns the support count before and after.
    pub(crate) fn add(
        &mut self,
        key: K,
        item: I,
        store: &StateStore,
    ) -> Result<(usize, usize), WorldError> {
        match self.read(&key, store)? {
            Supports::None => {
                self.outer = self.outer.upsert(key, SupportRef::Inline(item), store)?;
                Ok((0, 1))
            }
            Supports::One(d) => {
                if d.id() == item.id() {
                    return Ok((1, 1));
                }
                let set = SMap::new().upsert(d.id(), d.val(), store)?.upsert(
                    item.id(),
                    item.val(),
                    store,
                )?;
                self.write_many(key, set, store)?;
                Ok((1, 2))
            }
            Supports::Many(set) => {
                let old = set.len();
                let set = set.upsert(item.id(), item.val(), store)?;
                let new = set.len();
                if new != old {
                    self.write_many(key, set, store)?;
                }
                Ok((old, new))
            }
        }
    }

    /// Remove support `id` for `key`; returns the support count before and after
    /// (`(0, 0)` if the key is absent).
    pub(crate) fn remove(
        &mut self,
        key: &K,
        id: &Digest,
        store: &StateStore,
    ) -> Result<(usize, usize), WorldError> {
        match self.read(key, store)? {
            Supports::None => Ok((0, 0)),
            Supports::One(d) => {
                if d.id() != *id {
                    return Ok((1, 1));
                }
                self.hot.remove(key);
                self.outer = self.outer.remove(key, store)?;
                Ok((1, 0))
            }
            Supports::Many(set) => {
                let old = set.len();
                let set = set.remove(id, store)?;
                let new = set.len();
                if new == old {
                    return Ok((old, new));
                }
                if new == 1 {
                    let (rows, _) = set.page(None, 1, store)?;
                    let (k, v) = rows.into_iter().next().ok_or_else(|| {
                        WorldError::NetworkError("support set lost its last member".to_string())
                    })?;
                    self.hot.remove(key);
                    self.outer = self.outer.upsert(
                        key.clone(),
                        SupportRef::Inline(I::from_parts(k, v)),
                        store,
                    )?;
                } else {
                    self.write_many(key.clone(), set, store)?;
                }
                Ok((old, new))
            }
        }
    }

    /// Number of keys with at least one support.
    pub fn len(&self) -> usize {
        self.outer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.outer.is_empty()
    }

    /// Every key (outer trie order); reads only the outer index.
    pub(crate) fn keys(&self, store: &StateStore) -> Result<Vec<K>, WorldError> {
        let mut out = Vec::with_capacity(self.outer.len());
        let mut cursor = None;
        loop {
            let (rows, next) = self.outer.page(cursor.as_ref(), 1024, store)?;
            out.extend(rows.into_iter().map(|(k, _)| k));
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(out)
    }

    /// Every key with its supports (hot-aware), in outer trie order.
    pub(crate) fn entries(&self, store: &StateStore) -> Result<Vec<(K, Vec<I>)>, WorldError> {
        let mut out = Vec::with_capacity(self.outer.len());
        let mut cursor = None;
        loop {
            let (rows, next) = self.outer.page(cursor.as_ref(), 256, store)?;
            for (k, sref) in rows {
                let sup = match (self.hot.get(&k), sref) {
                    (Some(m), _) => Supports::Many(m.clone()),
                    (None, SupportRef::Inline(d)) => Supports::One(d),
                    (None, SupportRef::Sub(s)) => Supports::Many(SMap::from_sub_root(s)),
                };
                out.push((k, sup.members(store)?));
            }
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(out)
    }

    pub(crate) fn persist(&self, store: &StateStore) -> usize {
        let mut n = 0;
        for inner in self.hot.values() {
            n += inner.persist(store);
        }
        n + self.outer.persist(store)
    }

    pub(crate) fn dehydrated(&self, store: &StateStore) -> Result<Self, WorldError> {
        Ok(Self {
            outer: self.outer.dehydrate(store)?,
            hot: PMap::new(),
        })
    }
}

/// Distinct support index, keyed by the typed tuple (`Int(1)` and `Str("1")` stay distinct).
pub type DistinctSupports = SupportIndex<IntermediateTuple, CompactDeriv>;
/// Grouped-count index, keyed by the group key values.
pub type GroupSupports = SupportIndex<ValueList, Digest>;

#[derive(Clone, Debug, Default)]
pub struct AggregateIndex {
    pub rows: SMap<ValueList, ValueList>,
}

impl AggregateIndex {
    fn accumulator_key(key: &ValueList) -> ValueList {
        let mut values = vec![Value::Str("world::aggregate_state".into())];
        values.extend(key.0.iter().cloned());
        ValueList(values)
    }
    fn extremum_node_key(group: &ValueList, projection: usize, value: &Value) -> ValueList {
        let mut values = vec![
            Value::Str("world::aggregate_extremum".into()),
            Value::Int(projection as i64),
        ];
        values.extend(group.0.iter().cloned());
        values.push(value.clone());
        ValueList(values)
    }
    pub fn get(
        &self,
        key: &ValueList,
        store: &StateStore,
    ) -> Result<Option<ValueList>, WorldError> {
        Ok(self.rows.get(&Self::accumulator_key(key), store)?)
    }
    pub fn set(
        &mut self,
        key: ValueList,
        value: ValueList,
        store: &StateStore,
    ) -> Result<(), WorldError> {
        self.rows = self
            .rows
            .upsert(Self::accumulator_key(&key), value, store)?;
        Ok(())
    }
    pub fn remove(&mut self, key: &ValueList, store: &StateStore) -> Result<(), WorldError> {
        self.rows = self.rows.remove(&Self::accumulator_key(key), store)?;
        Ok(())
    }
    fn insert_extremum(
        &mut self,
        group: &ValueList,
        projection: usize,
        root: Option<&Value>,
        value: &Value,
        store: &StateStore,
        work: &mut u64,
    ) -> Result<Value, WorldError> {
        validate_extremum_value(value)?;
        let new_root = treap_insert(
            self,
            group,
            projection,
            root.cloned(),
            value.clone(),
            store,
            0,
            work,
        )?;
        Ok(new_root)
    }
    fn remove_extremum(
        &mut self,
        group: &ValueList,
        projection: usize,
        root: Option<&Value>,
        value: &Value,
        store: &StateStore,
        work: &mut u64,
    ) -> Result<Option<Value>, WorldError> {
        treap_remove(
            self,
            group,
            projection,
            root.cloned(),
            value,
            store,
            0,
            work,
        )
    }
    fn extremum(
        &self,
        group: &ValueList,
        projection: usize,
        root: Option<&Value>,
        max: bool,
        store: &StateStore,
        work: &mut u64,
    ) -> Result<Option<Value>, WorldError> {
        let Some(mut current) = root.cloned() else {
            return Ok(None);
        };
        let mut visited = BTreeSet::new();
        loop {
            if visited.len() >= MAX_EXTREMUM_DEPTH || !visited.insert(current.clone()) {
                return Err(WorldError::NetworkError(
                    "malformed ordered aggregate tree depth or cycle".into(),
                ));
            }
            *work = work.saturating_add(1);
            let node = self.extremum_node(group, projection, &current, store)?;
            validate_ext_node(group, projection, &current, &node)?;
            let next = if max { node.right } else { node.left };
            match next {
                Some(value) => current = value,
                None => return Ok(Some(current)),
            }
        }
    }
    fn extremum_node(
        &self,
        group: &ValueList,
        projection: usize,
        value: &Value,
        store: &StateStore,
    ) -> Result<ExtNode, WorldError> {
        let key = Self::extremum_node_key(group, projection, value);
        let stored = self
            .rows
            .get(&key, store)?
            .ok_or_else(|| WorldError::NetworkError("missing ordered aggregate node".into()))?;
        ExtNode::decode(&stored)
    }
    fn set_extremum_node(
        &mut self,
        group: &ValueList,
        projection: usize,
        value: &Value,
        node: ExtNode,
        store: &StateStore,
    ) -> Result<(), WorldError> {
        self.rows = self.rows.upsert(
            Self::extremum_node_key(group, projection, value),
            node.encode(),
            store,
        )?;
        Ok(())
    }
    fn delete_extremum_node(
        &mut self,
        group: &ValueList,
        projection: usize,
        value: &Value,
        store: &StateStore,
    ) -> Result<(), WorldError> {
        self.rows = self
            .rows
            .remove(&Self::extremum_node_key(group, projection, value), store)?;
        Ok(())
    }
    pub fn persist(&self, store: &StateStore) -> usize {
        self.rows.persist(store)
    }
    pub fn dehydrated(&self, store: &StateStore) -> Result<Self, WorldError> {
        Ok(Self {
            rows: self.rows.dehydrate(store)?,
        })
    }
}

#[derive(Clone, Debug)]
struct ExtNode {
    count: usize,
    left: Option<Value>,
    right: Option<Value>,
}

impl ExtNode {
    fn encode(&self) -> ValueList {
        ValueList(vec![Value::Record {
            nominal: "world::AggregateExtremumNode".into(),
            fields: BTreeMap::from([
                ("count".into(), Value::Int(self.count as i64)),
                ("left".into(), encode_extremum_link(self.left.as_ref())),
                ("right".into(), encode_extremum_link(self.right.as_ref())),
            ]),
        }])
    }
    fn decode(list: &ValueList) -> Result<Self, WorldError> {
        let [Value::Record { nominal, fields }] = list.0.as_slice() else {
            return Err(WorldError::NetworkError(
                "malformed ordered aggregate node".into(),
            ));
        };
        if nominal != "world::AggregateExtremumNode" || fields.len() != 3 {
            return Err(WorldError::NetworkError(
                "malformed ordered aggregate node".into(),
            ));
        }
        let Some(Value::Int(count)) = fields.get("count") else {
            return Err(WorldError::NetworkError(
                "malformed ordered aggregate node count".into(),
            ));
        };
        let count = usize::try_from(*count)
            .ok()
            .filter(|count| *count > 0)
            .ok_or_else(|| {
                WorldError::NetworkError("invalid ordered aggregate multiplicity".into())
            })?;
        let child = |name: &str| match fields.get(name) {
            Some(value) => decode_extremum_link(value),
            None => Err(WorldError::NetworkError(
                "malformed ordered aggregate node child".into(),
            )),
        };
        Ok(Self {
            count,
            left: child("left")?,
            right: child("right")?,
        })
    }
}

fn encode_extremum_link(value: Option<&Value>) -> Value {
    match value {
        Some(value) => Value::Variant {
            nominal: "world::AggregateExtremumLink".into(),
            variant: "Some".into(),
            args: vec![value.clone()],
        },
        None => Value::Variant {
            nominal: "world::AggregateExtremumLink".into(),
            variant: "None".into(),
            args: vec![],
        },
    }
}

fn decode_extremum_link(value: &Value) -> Result<Option<Value>, WorldError> {
    match value {
        Value::Variant {
            nominal,
            variant,
            args,
        } if nominal == "world::AggregateExtremumLink" && variant == "None" && args.is_empty() => {
            Ok(None)
        }
        Value::Variant {
            nominal,
            variant,
            args,
        } if nominal == "world::AggregateExtremumLink" && variant == "Some" && args.len() == 1 => {
            validate_extremum_value(&args[0])?;
            Ok(Some(args[0].clone()))
        }
        _ => Err(WorldError::NetworkError(
            "malformed ordered aggregate child link".into(),
        )),
    }
}

fn encode_extremum_root(value: Option<&Value>) -> Value {
    match value {
        Some(value) => Value::Variant {
            nominal: "world::AggregateExtremumRoot".into(),
            variant: "Some".into(),
            args: vec![value.clone()],
        },
        None => Value::Null,
    }
}

fn decode_extremum_root(value: &Value) -> Result<Option<Value>, WorldError> {
    match value {
        Value::Null => Ok(None),
        Value::Variant {
            nominal,
            variant,
            args,
        } if nominal == "world::AggregateExtremumRoot" && variant == "Some" && args.len() == 1 => {
            validate_extremum_value(&args[0])?;
            Ok(Some(args[0].clone()))
        }
        _ => Err(WorldError::NetworkError(
            "malformed ordered aggregate root".into(),
        )),
    }
}

fn extremum_priority(group: &ValueList, projection: usize, value: &Value) -> Digest {
    let mut parts = vec![Value::Int(projection as i64)];
    parts.extend(group.0.iter().cloned());
    parts.push(value.clone());
    Digest::of(Domain::Value, &ValueList(parts).state_bytes())
}

const MAX_EXTREMUM_DEPTH: usize = 128;

fn validate_extremum_value(value: &Value) -> Result<(), WorldError> {
    if matches!(
        value,
        Value::Null
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Str(_)
            | Value::F64(_)
            | Value::Decimal(_)
    ) {
        Ok(())
    } else {
        Err(WorldError::NetworkError(
            "ordered aggregate requires a scalar value".into(),
        ))
    }
}

fn compare_extremum_priority(
    group: &ValueList,
    projection: usize,
    left: &Value,
    right: &Value,
) -> std::cmp::Ordering {
    extremum_priority(group, projection, left)
        .cmp(&extremum_priority(group, projection, right))
        .then_with(|| left.cmp(right))
}

fn validate_ext_node(
    group: &ValueList,
    projection: usize,
    value: &Value,
    node: &ExtNode,
) -> Result<(), WorldError> {
    validate_extremum_value(value)?;
    for (child, is_left) in [(node.left.as_ref(), true), (node.right.as_ref(), false)] {
        if let Some(child) = child {
            validate_extremum_value(child)?;
            let ordered = if is_left {
                child < value
            } else {
                child > value
            };
            if !ordered
                || compare_extremum_priority(group, projection, value, child)
                    == std::cmp::Ordering::Greater
            {
                return Err(WorldError::NetworkError(
                    "malformed ordered aggregate child ordering".into(),
                ));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn treap_insert(
    index: &mut AggregateIndex,
    group: &ValueList,
    projection: usize,
    root: Option<Value>,
    value: Value,
    store: &StateStore,
    depth: usize,
    work: &mut u64,
) -> Result<Value, WorldError> {
    if depth >= MAX_EXTREMUM_DEPTH {
        return Err(WorldError::NetworkError(
            "ordered aggregate tree depth limit exceeded".into(),
        ));
    }
    *work = work.saturating_add(1);
    let Some(root_value) = root else {
        index.set_extremum_node(
            group,
            projection,
            &value,
            ExtNode {
                count: 1,
                left: None,
                right: None,
            },
            store,
        )?;
        return Ok(value);
    };
    let mut node = index.extremum_node(group, projection, &root_value, store)?;
    validate_ext_node(group, projection, &root_value, &node)?;
    if value == root_value {
        node.count = node.count.checked_add(1).ok_or_else(|| {
            WorldError::NetworkError("ordered aggregate multiplicity overflow".into())
        })?;
        index.set_extremum_node(group, projection, &root_value, node, store)?;
        return Ok(root_value);
    }
    if value < root_value {
        node.left = Some(treap_insert(
            index,
            group,
            projection,
            node.left.clone(),
            value,
            store,
            depth + 1,
            work,
        )?);
        index.set_extremum_node(group, projection, &root_value, node.clone(), store)?;
        let left_value = node.left.clone().expect("inserted left child");
        if compare_extremum_priority(group, projection, &left_value, &root_value)
            == std::cmp::Ordering::Less
        {
            let mut left = index.extremum_node(group, projection, &left_value, store)?;
            node.left = left.right.take();
            index.set_extremum_node(group, projection, &root_value, node, store)?;
            left.right = Some(root_value.clone());
            index.set_extremum_node(group, projection, &left_value, left, store)?;
            return Ok(left_value);
        }
    } else {
        node.right = Some(treap_insert(
            index,
            group,
            projection,
            node.right.clone(),
            value,
            store,
            depth + 1,
            work,
        )?);
        index.set_extremum_node(group, projection, &root_value, node.clone(), store)?;
        let right_value = node.right.clone().expect("inserted right child");
        if compare_extremum_priority(group, projection, &right_value, &root_value)
            == std::cmp::Ordering::Less
        {
            let mut right = index.extremum_node(group, projection, &right_value, store)?;
            node.right = right.left.take();
            index.set_extremum_node(group, projection, &root_value, node, store)?;
            right.left = Some(root_value.clone());
            index.set_extremum_node(group, projection, &right_value, right, store)?;
            return Ok(right_value);
        }
    }
    Ok(root_value)
}

#[allow(clippy::too_many_arguments)]
fn treap_remove(
    index: &mut AggregateIndex,
    group: &ValueList,
    projection: usize,
    root: Option<Value>,
    value: &Value,
    store: &StateStore,
    depth: usize,
    work: &mut u64,
) -> Result<Option<Value>, WorldError> {
    if depth >= MAX_EXTREMUM_DEPTH {
        return Err(WorldError::NetworkError(
            "ordered aggregate tree depth limit exceeded".into(),
        ));
    }
    *work = work.saturating_add(1);
    let Some(root_value) = root else {
        return Err(WorldError::NetworkError(
            "retracting absent aggregate extremum".into(),
        ));
    };
    let mut node = index.extremum_node(group, projection, &root_value, store)?;
    validate_ext_node(group, projection, &root_value, &node)?;
    if value < &root_value {
        node.left = treap_remove(
            index,
            group,
            projection,
            node.left.clone(),
            value,
            store,
            depth + 1,
            work,
        )?;
        index.set_extremum_node(group, projection, &root_value, node, store)?;
        return Ok(Some(root_value));
    }
    if value > &root_value {
        node.right = treap_remove(
            index,
            group,
            projection,
            node.right.clone(),
            value,
            store,
            depth + 1,
            work,
        )?;
        index.set_extremum_node(group, projection, &root_value, node, store)?;
        return Ok(Some(root_value));
    }
    if node.count > 1 {
        node.count -= 1;
        index.set_extremum_node(group, projection, &root_value, node, store)?;
        return Ok(Some(root_value));
    }
    index.delete_extremum_node(group, projection, &root_value, store)?;
    treap_merge(
        index, group, projection, node.left, node.right, store, depth, work,
    )
}

#[allow(clippy::too_many_arguments)]
fn treap_merge(
    index: &mut AggregateIndex,
    group: &ValueList,
    projection: usize,
    left: Option<Value>,
    right: Option<Value>,
    store: &StateStore,
    depth: usize,
    work: &mut u64,
) -> Result<Option<Value>, WorldError> {
    if depth >= MAX_EXTREMUM_DEPTH {
        return Err(WorldError::NetworkError(
            "ordered aggregate tree depth limit exceeded".into(),
        ));
    }
    match (left, right) {
        (None, right) => Ok(right),
        (left, None) => Ok(left),
        (Some(left_value), Some(right_value)) => {
            *work = work.saturating_add(1);
            if compare_extremum_priority(group, projection, &left_value, &right_value)
                == std::cmp::Ordering::Less
            {
                let mut node = index.extremum_node(group, projection, &left_value, store)?;
                validate_ext_node(group, projection, &left_value, &node)?;
                node.right = treap_merge(
                    index,
                    group,
                    projection,
                    node.right.clone(),
                    Some(right_value),
                    store,
                    depth + 1,
                    work,
                )?;
                index.set_extremum_node(group, projection, &left_value, node, store)?;
                Ok(Some(left_value))
            } else {
                let mut node = index.extremum_node(group, projection, &right_value, store)?;
                validate_ext_node(group, projection, &right_value, &node)?;
                node.left = treap_merge(
                    index,
                    group,
                    projection,
                    Some(left_value),
                    node.left.clone(),
                    store,
                    depth + 1,
                    work,
                )?;
                index.set_extremum_node(group, projection, &right_value, node, store)?;
                Ok(Some(right_value))
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GroupAccumulator {
    pub int_sums: Vec<i128>,
    pub decimal_sums: Vec<WideSigned>,
    pub decimal_scales: Vec<u8>,
    pub extrema_roots: Vec<Option<Value>>,
    pub collected: Vec<Vec<(Value, Value, Digest, Value)>>,
}

impl GroupAccumulator {
    pub fn empty(projections: &[(String, GroupProjection)]) -> Self {
        let n = projections.len();
        Self {
            int_sums: vec![0; n],
            decimal_sums: vec![WideSigned::default(); n],
            decimal_scales: vec![0; n],
            extrema_roots: vec![None; n],
            collected: vec![Vec::new(); n],
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_delta(
        &mut self,
        is_ins: bool,
        projections: &[(String, GroupProjection)],
        aggregate_values: &[Option<Value>],
        order_values: &[Option<Value>],
        child_key: Value,
        child_id: Digest,
        group: &ValueList,
        index: &mut AggregateIndex,
        store: &StateStore,
    ) -> Result<u64, WorldError> {
        let mut tree_work = 0u64;
        for (idx, ((_, projection), value)) in projections.iter().zip(aggregate_values).enumerate()
        {
            if let GroupProjection::Collect { limit, .. } = projection {
                if is_ins {
                    let item_val = value.clone().unwrap_or(Value::Null);
                    let order_val = order_values
                        .get(idx)
                        .and_then(|v| v.clone())
                        .unwrap_or_else(|| item_val.clone());
                    let key_val = child_key.clone();
                    self.collected[idx].retain(|(_, _, id, _)| id != &child_id);
                    if self.collected[idx].len() >= *limit {
                        return Err(WorldError::NetworkError(format!(
                            "grouped collect exceeded maximum cardinality limit of {limit}"
                        )));
                    }
                    self.collected[idx].push((order_val, key_val, child_id, item_val));
                    self.collected[idx].sort_by(|a, b| {
                        a.0.partial_cmp(&b.0)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| a.1.cmp(&b.1))
                            .then_with(|| a.2.cmp(&b.2))
                    });
                } else {
                    self.collected[idx].retain(|(_, _, id, _)| id != &child_id);
                }
                continue;
            }
            let Some(value) = value else { continue };
            match projection {
                GroupProjection::SumInt(_) => {
                    let Value::Int(value) = value else {
                        return Err(WorldError::NetworkError(
                            "grouped Int sum received non-Int".into(),
                        ));
                    };
                    let amount = i128::from(*value);
                    self.int_sums[idx] = if is_ins {
                        self.int_sums[idx].checked_add(amount)
                    } else {
                        self.int_sums[idx].checked_sub(amount)
                    }
                    .ok_or_else(|| {
                        WorldError::NetworkError("grouped Int sum accumulator overflow".into())
                    })?;
                }
                GroupProjection::SumDecimal(_) => {
                    let Value::Decimal(value) = value else {
                        return Err(WorldError::NetworkError(
                            "grouped Decimal sum received non-Decimal".into(),
                        ));
                    };
                    let value_scale = value.scale();
                    if value_scale > self.decimal_scales[idx] {
                        self.decimal_sums[idx].mul_pow10(value_scale - self.decimal_scales[idx]);
                        self.decimal_scales[idx] = value_scale;
                    }
                    let mut amount = WideSigned::from_i128(value.unscaled());
                    amount.mul_pow10(self.decimal_scales[idx] - value_scale);
                    if !is_ins {
                        amount.negative = !amount.negative;
                    }
                    self.decimal_sums[idx].add_assign(&amount);
                }
                GroupProjection::Min(_) | GroupProjection::Max(_) => {
                    self.extrema_roots[idx] = if is_ins {
                        Some(index.insert_extremum(
                            group,
                            idx,
                            self.extrema_roots[idx].as_ref(),
                            value,
                            store,
                            &mut tree_work,
                        )?)
                    } else {
                        index.remove_extremum(
                            group,
                            idx,
                            self.extrema_roots[idx].as_ref(),
                            value,
                            store,
                            &mut tree_work,
                        )?
                    };
                }
                GroupProjection::Key(_)
                | GroupProjection::Count
                | GroupProjection::Collect { .. } => {}
            }
        }
        Ok(tree_work)
    }

    pub fn to_value_list(&self, projections: &[(String, GroupProjection)]) -> ValueList {
        let mut values = Vec::with_capacity(projections.len());
        for (idx, (_, proj)) in projections.iter().enumerate() {
            match proj {
                GroupProjection::Key(_) | GroupProjection::Count => {
                    values.push(Value::Null);
                }
                GroupProjection::SumInt(_) => {
                    values.push(WideSigned::from_i128(self.int_sums[idx]).stored());
                }
                GroupProjection::SumDecimal(_) => {
                    values.push(Value::Record {
                        nominal: "world::DecimalAccumulator".into(),
                        fields: BTreeMap::from([
                            ("scale".into(), Value::Int(self.decimal_scales[idx] as i64)),
                            ("sum".into(), self.decimal_sums[idx].stored()),
                        ]),
                    });
                }
                GroupProjection::Min(_) | GroupProjection::Max(_) => {
                    values.push(encode_extremum_root(self.extrema_roots[idx].as_ref()))
                }
                GroupProjection::Collect { .. } => {
                    let items = self.collected[idx]
                        .iter()
                        .map(|(order, key_val, id, item)| Value::Record {
                            nominal: "world::CollectedItem".into(),
                            fields: BTreeMap::from([
                                ("order".into(), order.clone()),
                                ("key".into(), key_val.clone()),
                                ("id".into(), Value::Str(id.to_hex())),
                                ("item".into(), item.clone()),
                            ]),
                        })
                        .collect();
                    values.push(Value::List(items));
                }
            }
        }
        ValueList(values)
    }

    pub fn from_value_list(
        list: &ValueList,
        projections: &[(String, GroupProjection)],
    ) -> Result<Self, WorldError> {
        if list.0.len() != projections.len() {
            return Err(WorldError::NetworkError(
                "malformed grouped aggregate accumulator length".into(),
            ));
        }
        let mut accum = Self::empty(projections);
        for (idx, (_, proj)) in projections.iter().enumerate() {
            let val = &list.0[idx];
            match proj {
                GroupProjection::Key(_) | GroupProjection::Count => {
                    if !matches!(val, Value::Null) {
                        return Err(WorldError::NetworkError(
                            "malformed grouped aggregate passthrough slot".into(),
                        ));
                    }
                }
                GroupProjection::SumInt(_) => {
                    let w = WideSigned::restored(val)?;
                    accum.int_sums[idx] = w.to_i128().ok_or_else(|| {
                        WorldError::NetworkError("stored Int aggregate out of range".into())
                    })?;
                }
                GroupProjection::SumDecimal(_) => {
                    let Value::Record { nominal, fields } = val else {
                        return Err(WorldError::NetworkError(
                            "malformed Decimal aggregate accumulator".into(),
                        ));
                    };
                    if nominal != "world::DecimalAccumulator" || fields.len() != 2 {
                        return Err(WorldError::NetworkError(
                            "malformed Decimal aggregate accumulator".into(),
                        ));
                    }
                    let Some(Value::Int(scale)) = fields.get("scale") else {
                        return Err(WorldError::NetworkError(
                            "malformed Decimal aggregate scale".into(),
                        ));
                    };
                    accum.decimal_scales[idx] = u8::try_from(*scale)
                        .ok()
                        .filter(|scale| *scale <= brix_canon::MAX_DECIMAL_SCALE)
                        .ok_or_else(|| {
                            WorldError::NetworkError("malformed Decimal aggregate scale".into())
                        })?;
                    let sum = fields.get("sum").ok_or_else(|| {
                        WorldError::NetworkError("missing Decimal aggregate sum".into())
                    })?;
                    accum.decimal_sums[idx] = WideSigned::restored(sum)?;
                }
                GroupProjection::Min(_) | GroupProjection::Max(_) => {
                    accum.extrema_roots[idx] = decode_extremum_root(val)?;
                }
                GroupProjection::Collect { .. } => {
                    let Value::List(items) = val else {
                        return Err(WorldError::NetworkError(
                            "expected List for collected accumulator".into(),
                        ));
                    };
                    let mut entries = Vec::with_capacity(items.len());
                    for item in items {
                        let Value::Record { fields, .. } = item else {
                            return Err(WorldError::NetworkError(
                                "expected Record for collected item".into(),
                            ));
                        };
                        let order = fields.get("order").cloned().unwrap_or(Value::Null);
                        let key_val = fields
                            .get("key")
                            .cloned()
                            .or_else(|| fields.get("tuple").cloned())
                            .unwrap_or(Value::Null);
                        let id_str = match fields.get("id") {
                            Some(Value::Str(s)) => s,
                            _ => {
                                return Err(WorldError::NetworkError(
                                    "expected string id for collected item".into(),
                                ))
                            }
                        };
                        let key = WorldKey::from_hex(id_str).map_err(|e| {
                            WorldError::NetworkError(format!("malformed collected item id: {e}"))
                        })?;
                        if key.0.len() != 32 {
                            return Err(WorldError::NetworkError(
                                "collected item id must be 32 bytes".into(),
                            ));
                        }
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&key.0);
                        let id = Digest::from_bytes(arr);
                        let item = fields.get("item").cloned().unwrap_or(Value::Null);
                        entries.push((order, key_val, id, item));
                    }
                    accum.collected[idx] = entries;
                }
            }
        }
        Ok(accum)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WideSigned {
    negative: bool,
    digits: Vec<u8>,
}

impl WideSigned {
    pub fn to_i128(&self) -> Option<i128> {
        let magnitude = self.digits.iter().rev().try_fold(0u128, |acc, digit| {
            acc.checked_mul(10)?.checked_add(*digit as u128)
        })?;
        if self.negative {
            if magnitude == i128::MAX as u128 + 1 {
                Some(i128::MIN)
            } else {
                i128::try_from(magnitude).ok().map(|x| -x)
            }
        } else {
            i128::try_from(magnitude).ok()
        }
    }
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
        } else {
            match self
                .digits
                .len()
                .cmp(&other.digits.len())
                .then_with(|| self.digits.iter().rev().cmp(other.digits.iter().rev()))
            {
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
            if magnitude == i128::MAX as u128 + 1 {
                i128::MIN
            } else {
                -i128::try_from(magnitude).ok()?
            }
        } else {
            i128::try_from(magnitude).ok()?
        };
        Some(brix_canon::Decimal::new(signed, scale))
    }
    fn stored(&self) -> Value {
        Value::Record {
            nominal: "world::WideSigned".into(),
            fields: BTreeMap::from([
                ("negative".into(), Value::Bool(self.negative)),
                (
                    "digits".into(),
                    Value::Str(self.digits.iter().map(|d| char::from(b'0' + *d)).collect()),
                ),
            ]),
        }
    }
    fn restored(value: &Value) -> Result<Self, WorldError> {
        let Value::Record { nominal, fields } = value else {
            return Err(WorldError::NetworkError(
                "bad stored decimal accumulator".into(),
            ));
        };
        if nominal != "world::WideSigned" || fields.len() != 2 {
            return Err(WorldError::NetworkError(
                "bad stored decimal accumulator type".into(),
            ));
        }
        let negative = match fields.get("negative") {
            Some(Value::Bool(b)) => *b,
            _ => {
                return Err(WorldError::NetworkError(
                    "bad stored decimal accumulator sign".into(),
                ))
            }
        };
        let digits = match fields.get("digits") {
            Some(Value::Str(s)) => s
                .bytes()
                .map(|b| {
                    b.checked_sub(b'0').filter(|d| *d <= 9).ok_or_else(|| {
                        WorldError::NetworkError("bad stored decimal accumulator digit".into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => {
                return Err(WorldError::NetworkError(
                    "bad stored decimal accumulator digits".into(),
                ))
            }
        };
        if digits.last() == Some(&0) || (digits.is_empty() && negative) {
            return Err(WorldError::NetworkError(
                "noncanonical decimal accumulator".into(),
            ));
        }
        Ok(Self { negative, digits })
    }
}
/// Contents of a derived relation that is not a `Distinct` output: each distinct typed tuple
/// with the digests of the derivations currently supporting it. A tuple is in the relation
/// iff it has at least one support, so collapsing derivations (several rows projecting to
/// one tuple) is order-independent, and `Int(1)` / `Str("1")` stay distinct (the pre-M3 set
/// was keyed by the Display bytes of a `TupleRecord`, which collapsed them).
pub type DerivedSupports = SupportIndex<IntermediateTuple, Digest>;

/// How the contents of one derived relation are stored.
#[derive(Clone, Debug)]
pub enum DerivedRel {
    /// The relation is the output of `Distinct` operator `op` (lowering ends every `select`
    /// with one). Its tuples are exactly that operator's support-index keys, typed and with
    /// one canonical derivation each, so they are not stored a second time.
    DistinctOutput(OperatorId),
    /// Any other output operator: a support-counted typed tuple index.
    Counted(DerivedSupports),
}

/// The candidate frontier, store-backed: per `(decide, entity)` a [`FrontierBucket`] holding
/// the live candidates, each with its head and its supports (a single support inline, two or
/// more as the [`SubRoot`] of a nested set, canonical like every support index). A candidate
/// is live iff it has a support; an entity without candidates has no bucket.
///
/// `hot` holds the nested support sets modified (and so not yet reachable from the store)
/// since the last dehydration, keyed by candidate.
#[derive(Clone, Debug, Default)]
pub struct CandidateFrontier {
    pub buckets: SMap<FrontierKey, FrontierBucket>,
    pub hot: PMap<CandKey, SMap<Digest, CompactDeriv>>,
}

impl CandidateFrontier {
    /// The live candidates of one entity, with every head's calendar key re-checked against
    /// its recomputation (a mismatch is corrupt state).
    pub fn bucket(
        &self,
        key: &FrontierKey,
        store: &StateStore,
    ) -> Result<Option<FrontierBucket>, WorldError> {
        let Some(bucket) = self.buckets.get(key, store)? else {
            return Ok(None);
        };
        verify_bucket(key, &bucket)?;
        Ok(Some(bucket))
    }

    /// Write an entity's bucket (an empty or absent bucket removes the entity).
    fn put_bucket(
        &mut self,
        key: FrontierKey,
        bucket: Option<FrontierBucket>,
        store: &StateStore,
    ) -> Result<(), WorldError> {
        self.buckets = match bucket {
            Some(b) if !b.0.is_empty() => self.buckets.upsert(key, b, store)?,
            _ => self.buckets.remove(&key, store)?,
        };
        Ok(())
    }

    /// Visit every entity bucket in outer trie order, one page at a time, so a caller that
    /// reduces each bucket (settlement, counting) never holds more than a page of decoded
    /// buckets. Every bucket is verified before it is visited.
    pub(crate) fn for_each_bucket(
        &self,
        store: &StateStore,
        mut visit: impl FnMut(FrontierKey, FrontierBucket) -> Result<(), WorldError>,
    ) -> Result<(), WorldError> {
        let mut cursor = None;
        loop {
            let (rows, next) = self.buckets.page(cursor.as_ref(), 256, store)?;
            for (k, b) in rows {
                verify_bucket(&k, &b)?;
                visit(k, b)?;
            }
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(())
    }

    /// Every entity bucket, in outer trie order.
    pub(crate) fn all_buckets(
        &self,
        store: &StateStore,
    ) -> Result<Vec<(FrontierKey, FrontierBucket)>, WorldError> {
        let mut out = Vec::with_capacity(self.buckets.len());
        self.for_each_bucket(store, |k, b| {
            out.push((k, b));
            Ok(())
        })?;
        Ok(out)
    }

    /// The supports of candidate `ck`, whose stored record is `rec`.
    pub(crate) fn supports_of(&self, ck: &CandKey, rec: &CandidateRec) -> Supports<CompactDeriv> {
        match &rec.supports {
            SupportRef::Inline(d) => Supports::One(d.clone()),
            SupportRef::Sub(s) => Supports::Many(
                self.hot
                    .get(ck)
                    .cloned()
                    .unwrap_or_else(|| SMap::from_sub_root(*s)),
            ),
        }
    }

    /// Add support `item` to candidate `ck` (record `rec`, to be written back by the
    /// caller). Idempotent for a support already present.
    pub(crate) fn add_support(
        &mut self,
        ck: &CandKey,
        rec: &mut CandidateRec,
        item: CompactDeriv,
        store: &StateStore,
    ) -> Result<(), WorldError> {
        let set = match &rec.supports {
            SupportRef::Inline(d) => {
                if d.id() == item.id() {
                    return Ok(());
                }
                SMap::new().upsert(d.id(), d.clone(), store)?
            }
            SupportRef::Sub(s) => self
                .hot
                .get(ck)
                .cloned()
                .unwrap_or_else(|| SMap::from_sub_root(*s)),
        };
        let old = set.len();
        let set = set.upsert(item.id(), item, store)?;
        if set.len() != old {
            rec.supports = SupportRef::Sub(set.sub_root());
            self.hot.insert(ck.clone(), set);
        }
        Ok(())
    }

    /// Remove the support with id `id` from candidate `ck`; returns how many remain (the
    /// caller drops the candidate at 0).
    pub(crate) fn remove_support(
        &mut self,
        ck: &CandKey,
        rec: &mut CandidateRec,
        id: &Digest,
        store: &StateStore,
    ) -> Result<usize, WorldError> {
        match &rec.supports {
            SupportRef::Inline(d) => {
                if d.id() == *id {
                    self.hot.remove(ck);
                    Ok(0)
                } else {
                    Ok(1)
                }
            }
            SupportRef::Sub(s) => {
                let set = self
                    .hot
                    .get(ck)
                    .cloned()
                    .unwrap_or_else(|| SMap::from_sub_root(*s));
                let old = set.len();
                let set = set.remove(id, store)?;
                let new = set.len();
                if new == old {
                    return Ok(old);
                }
                if new == 1 {
                    let (rows, _) = set.page(None, 1, store)?;
                    let (_, only) = rows.into_iter().next().ok_or_else(|| {
                        WorldError::NetworkError("support set lost its last member".to_string())
                    })?;
                    rec.supports = SupportRef::Inline(only);
                    self.hot.remove(ck);
                } else {
                    rec.supports = SupportRef::Sub(set.sub_root());
                    self.hot.insert(ck.clone(), set);
                }
                Ok(new)
            }
        }
    }

    fn persist(&self, store: &StateStore) -> usize {
        let mut n = 0;
        for inner in self.hot.values() {
            n += inner.persist(store);
        }
        n + self.buckets.persist(store)
    }

    fn dehydrated(&self, store: &StateStore) -> Result<Self, WorldError> {
        Ok(Self {
            buckets: self.buckets.dehydrate(store)?,
            hot: PMap::new(),
        })
    }
}

/// The net effect of one batch's deltas on a relation, retractions first.
///
/// A `(tuple, derivation)` pair inserted and retracted within the batch (a join result that
/// exists only transiently) cancels out. Applying all retractions before all insertions
/// without cancelling first would retract the not-yet-present pair and then insert it,
/// leaving a candidate (and a settlement) for a tuple that is no longer derived.
fn net_effect(deltas: &[TupleDelta]) -> Vec<&TupleDelta> {
    if deltas.iter().all(TupleDelta::is_insert) || deltas.iter().all(|d| !d.is_insert()) {
        return deltas.iter().collect();
    }
    let key = |d: &TupleDelta| (d.derivation().c.clone(), d.tuple().clone());
    let mut balance: BTreeMap<(CompactDeriv, IntermediateTuple), i64> = BTreeMap::new();
    for d in deltas {
        *balance.entry(key(d)).or_default() += if d.is_insert() { 1 } else { -1 };
    }
    let mut emitted = BTreeSet::new();
    let mut retractions = Vec::new();
    let mut insertions = Vec::new();
    for d in deltas {
        let k = key(d);
        let b = balance[&k];
        if (b > 0 && d.is_insert() || b < 0 && !d.is_insert()) && emitted.insert((k, d.is_insert()))
        {
            if d.is_insert() {
                insertions.push(d);
            } else {
                retractions.push(d);
            }
        }
    }
    retractions.extend(insertions);
    retractions
}

fn verify_bucket(key: &FrontierKey, bucket: &FrontierBucket) -> Result<(), WorldError> {
    for (name, rec) in &bucket.0 {
        rec.head
            .verify(&key.decide, &key.entity, name)
            .map_err(|e| WorldError::from(StateError::from(e)))?;
    }
    Ok(())
}

/// The settled decision of one entity: the live candidate with the least calendar key.
fn settle(entity: &str, bucket: &FrontierBucket) -> Option<SettledDecision> {
    let mut frontier: Frontier<(&String, &CandidateHead)> = Frontier::new();
    for (name, rec) in &bucket.0 {
        let _ = frontier.insert(rec.head.calendar_key, (name, &rec.head));
    }
    let (key, (name, head)) = frontier.select_least()?;
    Some(SettledDecision {
        entity_id: entity.to_string(),
        candidate_name: name.clone(),
        priority: head.priority,
        phase: head.phase,
        value: head.value.clone(),
        calendar_key: key,
    })
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
        left_side: JoinSide,
        right_side: JoinSide,
    },
    AntiJoin {
        left: OperatorId,
        right: OperatorId,
        left_keys: Vec<FieldRef>,
        right_keys: Vec<FieldRef>,
        left_side: JoinSide,
        right_side: JoinSide,
    },
    DisjointUnion {
        left: OperatorId,
        right: OperatorId,
        key_fields: Vec<FieldRef>,
        left_side: JoinSide,
        right_side: JoinSide,
    },
    ParentCount {
        left: OperatorId,
        right: OperatorId,
        left_key: FieldRef,
        right_key: FieldRef,
        output_key: String,
        output_count: String,
        left_side: JoinSide,
        right_side: JoinSide,
    },
    Distinct {
        input: OperatorId,
        supports: DistinctSupports,
    },
    GroupedCount {
        input: OperatorId,
        group_keys: Vec<ast::Expr>,
        compiled_group_keys: Vec<CompiledWorldExpr>,
        projections: Vec<(String, GroupProjection)>,
        compiled_aggregates: Vec<Option<CompiledWorldExpr>>,
        compiled_collect_orders: Vec<Option<CompiledWorldExpr>>,
        groups: GroupSupports,
        aggregates: AggregateIndex,
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

/// First-seen (pre-batch) and latest decision of one entity within a batch.
#[derive(Clone, Debug)]
struct DecisionTrack {
    prior: Option<WorldTuple>,
    last: Option<SettledDecision>,
}

/// In-flight state of a batch applied as chunks ([`WorldNetwork::begin_batch`]).
#[derive(Clone, Debug, Default)]
pub struct BatchCtx {
    report: NetworkDeltaReport,
    diagnostics: NetworkDiagnostics,
    total_work: u64,
    total_matches: u64,
    total_expressions: u64,
    decisions: BTreeMap<(String, String), DecisionTrack>,
    chunks: usize,
}

impl BatchCtx {
    /// Chunks applied so far.
    pub fn chunks(&self) -> usize {
        self.chunks
    }
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
    pub derived_relations: PMap<String, DerivedRel>,
    pub decides: PMap<String, DecideBlock>,
    decision_subscribers: PMap<String, Vec<String>>,
    pub virtual_relations: BTreeSet<String>,
    /// Content-addressed trie of settled decisions, keyed by [`decision_key`]
    /// (ADR-0046 P6 G2). Store-backed: only fallible, store-aware operations are
    /// reachable, so an unhydrated node can never read as an empty subtree. Its node
    /// encoding is identical to the pre-M3 `TrieMap<WorldKey, WorldTuple>`, so decision
    /// roots recorded in revisions are unchanged.
    pub(crate) decision_tree: SMap<WorldKey, WorldTuple>,
    pub candidate_frontier: CandidateFrontier,
    pub functions: Arc<BTreeMap<String, ast::Callable>>,
    pub program_env: Option<Arc<CompiledProgramEnv>>,
    pub current_revision: u64,
    pub scheduling_quantum: usize,
    /// Node store backing persisted operator state (join and support indexes).
    state: StateStore,
    /// Leaves each operator's derivations carry (index = operator id).
    leaf_counts: Arc<Vec<usize>>,
    /// Whether this network holds the base facts itself (`base_relations`, `Scan.records`).
    /// A network restored from persisted state (R0 M4) does not: the old tuple of a changed
    /// key is looked up through the [`BaseLookup`] the session passes to
    /// [`Self::apply_ops_staged_view`]. The legacy oracle shadows are only meaningful for a
    /// network that has seen every fact since empty, so they follow this flag too.
    base_resident: bool,
}

/// Read access to the pre-batch base facts, supplied by whoever owns them (the session's
/// relation tries) to a network that does not keep its own copy.
pub type BaseLookup<'a> = dyn Fn(&str, &WorldKey) -> Result<Option<WorldTuple>, WorldError> + 'a;

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
                            left_side: JoinSide::default(),
                            right_side: JoinSide::default(),
                        },
                    );
                }
                OperatorNode::AntiJoin {
                    left,
                    right,
                    left_keys,
                    right_keys,
                } => {
                    downstream.entry(*left).or_default().push(op_id);
                    downstream.entry(*right).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::AntiJoin {
                            left: *left,
                            right: *right,
                            left_keys: left_keys.clone(),
                            right_keys: right_keys.clone(),
                            left_side: JoinSide::default(),
                            right_side: JoinSide::default(),
                        },
                    );
                }
                OperatorNode::DisjointUnion {
                    left,
                    right,
                    key_fields,
                } => {
                    downstream.entry(*left).or_default().push(op_id);
                    downstream.entry(*right).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::DisjointUnion {
                            left: *left,
                            right: *right,
                            key_fields: key_fields
                                .iter()
                                .map(|field| FieldRef {
                                    binding: String::new(),
                                    field: field.clone(),
                                })
                                .collect(),
                            left_side: JoinSide::default(),
                            right_side: JoinSide::default(),
                        },
                    );
                }
                OperatorNode::ParentCount {
                    left,
                    right,
                    left_key,
                    right_key,
                    output_key,
                    output_count,
                } => {
                    downstream.entry(*left).or_default().push(op_id);
                    downstream.entry(*right).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::ParentCount {
                            left: *left,
                            right: *right,
                            left_key: left_key.clone(),
                            right_key: right_key.clone(),
                            output_key: output_key.clone(),
                            output_count: output_count.clone(),
                            left_side: JoinSide::default(),
                            right_side: JoinSide::default(),
                        },
                    );
                }
                OperatorNode::Distinct { input } => {
                    downstream.entry(*input).or_default().push(op_id);
                    operator_states.insert(
                        idx,
                        OperatorState::Distinct {
                            input: *input,
                            supports: DistinctSupports::default(),
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
                            compiled_collect_orders: vec![None; projections.len()],
                            groups: GroupSupports::default(),
                            aggregates: AggregateIndex::default(),
                        },
                    );
                }
            }
        }

        let mut derived_relations = PMap::new();
        for (name, &op_id) in &dag.relation_outputs {
            match dag.nodes[op_id.0] {
                OperatorNode::Scan { .. } => {}
                OperatorNode::Distinct { .. } => {
                    derived_relations.insert(name.clone(), DerivedRel::DistinctOutput(op_id));
                }
                _ => {
                    derived_relations.insert(
                        name.clone(),
                        DerivedRel::Counted(DerivedSupports::default()),
                    );
                }
            }
        }

        let mut output_relations: PMap<OperatorId, Vec<String>> = PMap::new();
        for (name, op) in &dag.relation_outputs {
            output_relations.entry(*op).or_default().push(name.clone());
        }
        let counts = leaf_counts(&dag);
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
            decision_tree: SMap::new(),
            candidate_frontier: CandidateFrontier::default(),
            functions: Arc::new(BTreeMap::new()),
            program_env: None,
            current_revision: 0,
            scheduling_quantum: DEFAULT_SCHEDULING_QUANTUM,
            state: StateStore::memory(DEFAULT_NODE_CACHE_BYTES),
            leaf_counts: Arc::new(counts),
            base_resident: true,
        }
    }

    /// Back operator state with `store` (call before any data is applied).
    pub fn attach_state_store(&mut self, store: StateStore) {
        self.state = store;
    }

    /// The store backing persisted operator state.
    pub fn state_store(&self) -> &StateStore {
        &self.state
    }

    /// Write all not-yet-persisted join state (inner maps, then outer indexes, then
    /// support indexes). Returns the number of objects written. The caller
    /// flushes the store.
    pub fn persist_state(&self) -> Result<usize, WorldError> {
        let mut n = 0;
        for rel in self.derived_relations.values() {
            if let DerivedRel::Counted(supports) = rel {
                n += supports.persist(&self.state);
            }
        }
        n += self.candidate_frontier.persist(&self.state);
        n += self.decision_tree.persist(&self.state);
        for (_, state) in self.operator_states.iter() {
            match state {
                OperatorState::EquiJoin {
                    left_side,
                    right_side,
                    ..
                } => {
                    n += left_side.persist(&self.state);
                    n += right_side.persist(&self.state);
                }
                OperatorState::AntiJoin {
                    left_side,
                    right_side,
                    ..
                }
                | OperatorState::DisjointUnion {
                    left_side,
                    right_side,
                    ..
                }
                | OperatorState::ParentCount {
                    left_side,
                    right_side,
                    ..
                } => {
                    n += left_side.persist(&self.state);
                    n += right_side.persist(&self.state);
                }
                OperatorState::Distinct { supports, .. } => n += supports.persist(&self.state),
                OperatorState::GroupedCount {
                    groups, aggregates, ..
                } => {
                    n += groups.persist(&self.state);
                    n += aggregates.persist(&self.state);
                }
                _ => {}
            }
        }
        Ok(n)
    }

    /// Write all not-yet-persisted state ([`Self::persist_state`]) and replace every
    /// persisted trie by its `(root, len)` store reference, dropping the hydrated nodes
    /// and the support caches. Roots are unchanged. Returns the objects written. Only
    /// valid at a quiescent point (between chunks of a batch, never inside
    /// [`Self::apply_chunk`]); the caller decides when to flush the store.
    pub fn persist_and_dehydrate_in_place(&mut self) -> Result<usize, WorldError> {
        let n = self.persist_state()?;
        let d = self.dehydrated()?;
        *self = d;
        Ok(n)
    }

    /// A copy holding only store references for persisted state. Requires
    /// [`Self::persist_state`] (and a flush) first; fails if a root is not persisted.
    pub fn dehydrated(&self) -> Result<Self, WorldError> {
        let mut out = self.clone();
        for (name, rel) in self.derived_relations.iter() {
            if let DerivedRel::Counted(supports) = rel {
                out.derived_relations.insert(
                    name.clone(),
                    DerivedRel::Counted(supports.dehydrated(&self.state)?),
                );
            }
        }
        out.candidate_frontier = self.candidate_frontier.dehydrated(&self.state)?;
        out.decision_tree = self.decision_tree.dehydrate(&self.state)?;
        for (idx, state) in self.operator_states.iter() {
            match state {
                OperatorState::EquiJoin {
                    left,
                    right,
                    left_keys,
                    right_keys,
                    left_side,
                    right_side,
                } => {
                    out.operator_states.insert(
                        *idx,
                        OperatorState::EquiJoin {
                            left: *left,
                            right: *right,
                            left_keys: left_keys.clone(),
                            right_keys: right_keys.clone(),
                            left_side: left_side.dehydrated(&self.state)?,
                            right_side: right_side.dehydrated(&self.state)?,
                        },
                    );
                }
                OperatorState::AntiJoin {
                    left,
                    right,
                    left_keys,
                    right_keys,
                    left_side,
                    right_side,
                } => {
                    out.operator_states.insert(
                        *idx,
                        OperatorState::AntiJoin {
                            left: *left,
                            right: *right,
                            left_keys: left_keys.clone(),
                            right_keys: right_keys.clone(),
                            left_side: left_side.dehydrated(&self.state)?,
                            right_side: right_side.dehydrated(&self.state)?,
                        },
                    );
                }
                OperatorState::DisjointUnion {
                    left,
                    right,
                    key_fields,
                    left_side,
                    right_side,
                } => {
                    out.operator_states.insert(
                        *idx,
                        OperatorState::DisjointUnion {
                            left: *left,
                            right: *right,
                            key_fields: key_fields.clone(),
                            left_side: left_side.dehydrated(&self.state)?,
                            right_side: right_side.dehydrated(&self.state)?,
                        },
                    );
                }
                OperatorState::ParentCount {
                    left,
                    right,
                    left_key,
                    right_key,
                    output_key,
                    output_count,
                    left_side,
                    right_side,
                } => {
                    out.operator_states.insert(
                        *idx,
                        OperatorState::ParentCount {
                            left: *left,
                            right: *right,
                            left_key: left_key.clone(),
                            right_key: right_key.clone(),
                            output_key: output_key.clone(),
                            output_count: output_count.clone(),
                            left_side: left_side.dehydrated(&self.state)?,
                            right_side: right_side.dehydrated(&self.state)?,
                        },
                    );
                }
                OperatorState::Distinct { input, supports } => {
                    out.operator_states.insert(
                        *idx,
                        OperatorState::Distinct {
                            input: *input,
                            supports: supports.dehydrated(&self.state)?,
                        },
                    );
                }
                OperatorState::GroupedCount {
                    input,
                    group_keys,
                    compiled_group_keys,
                    projections,
                    compiled_aggregates,
                    compiled_collect_orders,
                    groups,
                    aggregates,
                } => {
                    out.operator_states.insert(
                        *idx,
                        OperatorState::GroupedCount {
                            input: *input,
                            group_keys: group_keys.clone(),
                            compiled_group_keys: compiled_group_keys.clone(),
                            projections: projections.clone(),
                            compiled_aggregates: compiled_aggregates.clone(),
                            compiled_collect_orders: compiled_collect_orders.clone(),
                            groups: groups.dehydrated(&self.state)?,
                            aggregates: aggregates.dehydrated(&self.state)?,
                        },
                    );
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// Per join operator, the `(left, right)` outer index roots.
    pub fn join_index_roots(&self) -> BTreeMap<OperatorId, (SubRoot, SubRoot)> {
        let mut out = BTreeMap::new();
        for (idx, state) in self.operator_states.iter() {
            if let OperatorState::EquiJoin {
                left_side,
                right_side,
                ..
            } = state
            {
                out.insert(
                    OperatorId(*idx),
                    (left_side.outer.sub_root(), right_side.outer.sub_root()),
                );
            }
        }
        out
    }

    /// Per Distinct operator, the outer support index root.
    pub fn distinct_roots(&self) -> BTreeMap<OperatorId, SubRoot> {
        let mut out = BTreeMap::new();
        for (idx, state) in self.operator_states.iter() {
            if let OperatorState::Distinct { supports, .. } = state {
                out.insert(OperatorId(*idx), supports.outer.sub_root());
            }
        }
        out
    }

    /// Per GroupedCount operator, the outer group index root.
    pub fn group_roots(&self) -> BTreeMap<OperatorId, SubRoot> {
        let mut out = BTreeMap::new();
        for (idx, state) in self.operator_states.iter() {
            if let OperatorState::GroupedCount { groups, .. } = state {
                out.insert(OperatorId(*idx), groups.outer.sub_root());
            }
        }
        out
    }

    /// Per derived relation, the root of the index holding its tuples: the Distinct
    /// operator's support index for a `Distinct` output (see [`DerivedRel`]), the relation's
    /// own index otherwise.
    pub fn derived_roots(&self) -> BTreeMap<String, SubRoot> {
        let distinct = self.distinct_roots();
        self.derived_relations
            .iter()
            .map(|(name, rel)| {
                let root = match rel {
                    DerivedRel::DistinctOutput(op) => {
                        distinct.get(op).copied().unwrap_or_else(SubRoot::empty)
                    }
                    DerivedRel::Counted(s) => s.outer.sub_root(),
                };
                (name.clone(), root)
            })
            .collect()
    }

    /// The candidate frontier root (buckets, with the supports nested in them).
    pub fn frontier_root(&self) -> SubRoot {
        self.candidate_frontier.buckets.sub_root()
    }

    /// Root and size of the settled-decision trie.
    pub fn decision_sub_root(&self) -> SubRoot {
        self.decision_tree.sub_root()
    }

    /// Structural fingerprint of this network: the lowered operator DAG (node kinds, ids,
    /// keys, expressions, projections), the relation outputs, the decide blocks and the
    /// functions. Part of the validity key of persisted state, so state computed by a
    /// different program can never be installed into this one.
    pub fn fingerprint(&self) -> Digest {
        use std::fmt::Write;
        let mut s = String::from("brix.world.net-fingerprint@1\n");
        let _ = writeln!(s, "{:?}", self.dag.nodes);
        let _ = writeln!(s, "{:?}", self.dag.relation_outputs);
        let decides: BTreeMap<_, _> = self.decides.iter().collect();
        for (name, d) in decides {
            let _ = writeln!(
                s,
                "{name}|{}|{}|{}|{:?}",
                d.binder, d.source_relation, d.per_field, d.proposals
            );
        }
        let _ = writeln!(s, "{:?}", self.functions);
        Digest::of(Domain::State, s.as_bytes())
    }

    /// Whether this network keeps its own copy of the base facts (see [`BaseLookup`]).
    pub fn base_resident(&self) -> bool {
        self.base_resident
    }

    /// The header naming every persisted root of this network, under `validity`. Call on a
    /// network whose state has been persisted ([`Self::persist_state`]); the roots do not
    /// depend on whether it was.
    pub fn state_header(&self, validity: ValidityKey) -> NetworkStateHeader {
        let mut derived = BTreeMap::new();
        // Distinct-backed derived relations are views over their Distinct operator's
        // index, recorded once under `operators`; listing them here would count them twice.
        for (name, rel) in self.derived_relations.iter() {
            if let DerivedRel::Counted(s) = rel {
                derived.insert(name.clone(), s.outer.sub_root());
            }
        }
        let mut operators = BTreeMap::new();
        for (idx, state) in self.operator_states.iter() {
            let roots = match state {
                OperatorState::DisjointUnion {
                    left_side,
                    right_side,
                    ..
                }
                | OperatorState::EquiJoin {
                    left_side,
                    right_side,
                    ..
                } => OperatorRoots::Join {
                    left: left_side.outer.sub_root(),
                    right: right_side.outer.sub_root(),
                },
                OperatorState::AntiJoin {
                    left_side,
                    right_side,
                    ..
                } => OperatorRoots::Anti {
                    left: left_side.outer.sub_root(),
                    right: right_side.outer.sub_root(),
                },
                OperatorState::ParentCount {
                    left_side,
                    right_side,
                    ..
                } => OperatorRoots::ParentCount {
                    left: left_side.outer.sub_root(),
                    right: right_side.outer.sub_root(),
                },
                OperatorState::Distinct { supports, .. } => {
                    OperatorRoots::Distinct(supports.outer.sub_root())
                }
                OperatorState::GroupedCount {
                    groups, aggregates, ..
                } => OperatorRoots::Group {
                    supports: groups.outer.sub_root(),
                    aggregates: aggregates.rows.sub_root(),
                },
                _ => continue,
            };
            operators.insert(OperatorId(*idx), roots);
        }
        NetworkStateHeader {
            validity,
            decision: self.decision_sub_root(),
            frontier: self.frontier_root(),
            derived,
            operators,
        }
    }

    /// Install the persisted state `header` names into this (freshly built, empty) network,
    /// lazily: nothing but the roots is read, apart from a probe of each non-empty root that
    /// fails closed on a missing or corrupt root node. The header's operator set must match
    /// this program exactly. The network then holds no base copy (see [`BaseLookup`]).
    pub fn restore_state(&mut self, header: &NetworkStateHeader) -> Result<(), WorldError> {
        let mismatch = |m: String| WorldError::ValidityMismatch(m);

        let mut expect: BTreeMap<OperatorId, &'static str> = BTreeMap::new();
        for (idx, state) in self.operator_states.iter() {
            match state {
                OperatorState::DisjointUnion { .. } | OperatorState::EquiJoin { .. } => {
                    expect.insert(OperatorId(*idx), "join");
                }
                OperatorState::AntiJoin { .. } => {
                    expect.insert(OperatorId(*idx), "anti-join");
                }
                OperatorState::ParentCount { .. } => {
                    expect.insert(OperatorId(*idx), "parent-count");
                }
                OperatorState::Distinct { .. } => {
                    expect.insert(OperatorId(*idx), "distinct");
                }
                OperatorState::GroupedCount { .. } => {
                    expect.insert(OperatorId(*idx), "group");
                }
                _ => {}
            }
        }
        let got: BTreeMap<OperatorId, &'static str> = header
            .operators
            .iter()
            .map(|(op, r)| (*op, r.kind_name()))
            .collect();
        if got != expect {
            return Err(mismatch(format!(
                "persisted operators {got:?} do not match the program's {expect:?}"
            )));
        }
        let counted: BTreeSet<&String> = self
            .derived_relations
            .iter()
            .filter(|(_, r)| matches!(r, DerivedRel::Counted(_)))
            .map(|(n, _)| n)
            .collect();
        let listed: BTreeSet<&String> = header.derived.keys().collect();
        if counted != listed {
            return Err(mismatch(format!(
                "persisted derived relations {listed:?} do not match the program's {counted:?}"
            )));
        }

        for (name, root) in &header.derived {
            if let Some(DerivedRel::Counted(supports)) = self.derived_relations.get_mut(name) {
                supports.outer = SMap::from_sub_root(*root);
                supports.hot = PMap::new();
            }
        }
        for (op, roots) in &header.operators {
            match (self.operator_states.get_mut(&op.0), roots) {
                (
                    Some(
                        OperatorState::DisjointUnion {
                            left_side,
                            right_side,
                            ..
                        }
                        | OperatorState::EquiJoin {
                            left_side,
                            right_side,
                            ..
                        },
                    ),
                    OperatorRoots::Join { left, right },
                ) => {
                    left_side.outer = SMap::from_sub_root(*left);
                    left_side.hot = PMap::new();
                    right_side.outer = SMap::from_sub_root(*right);
                    right_side.hot = PMap::new();
                }
                (
                    Some(OperatorState::AntiJoin {
                        left_side,
                        right_side,
                        ..
                    }),
                    OperatorRoots::Anti { left, right },
                )
                | (
                    Some(OperatorState::ParentCount {
                        left_side,
                        right_side,
                        ..
                    }),
                    OperatorRoots::ParentCount { left, right },
                ) => {
                    left_side.outer = SMap::from_sub_root(*left);
                    left_side.hot = PMap::new();
                    right_side.outer = SMap::from_sub_root(*right);
                    right_side.hot = PMap::new();
                }
                (Some(OperatorState::Distinct { supports, .. }), OperatorRoots::Distinct(r)) => {
                    supports.outer = SMap::from_sub_root(*r);
                    supports.hot = PMap::new();
                }
                (
                    Some(OperatorState::GroupedCount {
                        groups, aggregates, ..
                    }),
                    OperatorRoots::Group {
                        supports,
                        aggregates: aggregate_root,
                    },
                ) => {
                    groups.outer = SMap::from_sub_root(*supports);
                    groups.hot = PMap::new();
                    aggregates.rows = SMap::from_sub_root(*aggregate_root);
                }
                _ => return Err(mismatch(format!("operator {} has the wrong kind", op.0))),
            }
        }
        self.candidate_frontier.buckets = SMap::from_sub_root(header.frontier);
        self.candidate_frontier.hot = PMap::new();
        self.decision_tree = SMap::from_sub_root(header.decision);
        self.base_resident = false;

        self.probe_roots()
    }

    /// Touch the first path of every non-empty persisted root, so a missing or corrupt
    /// root node is reported when the network is restored rather than at first use.
    fn probe_roots(&self) -> Result<(), WorldError> {
        let store = &self.state;
        for (_, state) in self.operator_states.iter() {
            match state {
                OperatorState::EquiJoin {
                    left_side,
                    right_side,
                    ..
                } => {
                    left_side.outer.page(None, 1, store)?;
                    right_side.outer.page(None, 1, store)?;
                }
                OperatorState::AntiJoin {
                    left_side,
                    right_side,
                    ..
                }
                | OperatorState::DisjointUnion {
                    left_side,
                    right_side,
                    ..
                }
                | OperatorState::ParentCount {
                    left_side,
                    right_side,
                    ..
                } => {
                    left_side.outer.page(None, 1, store)?;
                    right_side.outer.page(None, 1, store)?;
                }
                OperatorState::Distinct { supports, .. } => {
                    supports.outer.page(None, 1, store)?;
                }
                OperatorState::GroupedCount {
                    groups, aggregates, ..
                } => {
                    groups.outer.page(None, 1, store)?;
                    aggregates.rows.page(None, 1, store)?;
                }
                _ => {}
            }
        }
        for (_, rel) in self.derived_relations.iter() {
            if let DerivedRel::Counted(s) = rel {
                s.outer.page(None, 1, store)?;
            }
        }
        self.candidate_frontier.buckets.page(None, 1, store)?;
        self.decision_tree.page(None, 1, store)?;
        Ok(())
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
            .map(|(k, rel)| {
                let cleared = match rel {
                    DerivedRel::DistinctOutput(op) => DerivedRel::DistinctOutput(*op),
                    DerivedRel::Counted(_) => DerivedRel::Counted(DerivedSupports::default()),
                };
                (k.clone(), cleared)
            })
            .collect();
        fresh.decision_tree = SMap::new();
        fresh.candidate_frontier = CandidateFrontier::default();
        fresh.current_revision = 0;
        fresh.base_resident = true;
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
                    left_side: JoinSide::default(),
                    right_side: JoinSide::default(),
                },
                OperatorState::AntiJoin {
                    left,
                    right,
                    left_keys,
                    right_keys,
                    ..
                } => OperatorState::AntiJoin {
                    left: *left,
                    right: *right,
                    left_keys: left_keys.clone(),
                    right_keys: right_keys.clone(),
                    left_side: JoinSide::default(),
                    right_side: JoinSide::default(),
                },
                OperatorState::DisjointUnion {
                    left,
                    right,
                    key_fields,
                    ..
                } => OperatorState::DisjointUnion {
                    left: *left,
                    right: *right,
                    key_fields: key_fields.clone(),
                    left_side: JoinSide::default(),
                    right_side: JoinSide::default(),
                },
                OperatorState::ParentCount {
                    left,
                    right,
                    left_key,
                    right_key,
                    output_key,
                    output_count,
                    ..
                } => OperatorState::ParentCount {
                    left: *left,
                    right: *right,
                    left_key: left_key.clone(),
                    right_key: right_key.clone(),
                    output_key: output_key.clone(),
                    output_count: output_count.clone(),
                    left_side: JoinSide::default(),
                    right_side: JoinSide::default(),
                },
                OperatorState::Distinct { input, .. } => OperatorState::Distinct {
                    input: *input,
                    supports: DistinctSupports::default(),
                },
                OperatorState::GroupedCount {
                    input,
                    group_keys,
                    compiled_group_keys,
                    projections,
                    compiled_aggregates,
                    compiled_collect_orders,
                    ..
                } => OperatorState::GroupedCount {
                    input: *input,
                    group_keys: group_keys.clone(),
                    compiled_group_keys: compiled_group_keys.clone(),
                    projections: projections.clone(),
                    compiled_aggregates: compiled_aggregates.clone(),
                    compiled_collect_orders: compiled_collect_orders.clone(),
                    groups: GroupSupports::default(),
                    aggregates: AggregateIndex::default(),
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
                                | GroupProjection::Max(expr)
                                | GroupProjection::Collect { expr, .. } => expr,
                                GroupProjection::Key(_) | GroupProjection::Count => {
                                    return Ok(None);
                                }
                            };
                            program_env
                                .compile_expr(expr, &bindings)
                                .map(Some)
                                .map_err(WorldError::NetworkError)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let compiled_collect_orders = projections
                        .iter()
                        .map(|(_, projection)| {
                            if let GroupProjection::Collect {
                                order_by: Some(order_expr),
                                ..
                            } = projection
                            {
                                program_env
                                    .compile_expr(order_expr, &bindings)
                                    .map(Some)
                                    .map_err(WorldError::NetworkError)
                            } else {
                                Ok(None)
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if let Some(OperatorState::GroupedCount {
                        compiled_group_keys: ref mut cgk,
                        compiled_aggregates: ref mut ca,
                        compiled_collect_orders: ref mut cco,
                        ..
                    }) = network.operator_states.get_mut(&idx)
                    {
                        *cgk = compiled_group_keys;
                        *ca = compiled_aggregates;
                        *cco = compiled_collect_orders;
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
        self.apply_ops_staged_view(ops, limits, None)
    }

    /// As [`Self::apply_ops_staged_bounded`], reading pre-batch base facts through `base`.
    /// Required for a network restored from persisted state, which holds no base copy; a
    /// resident network ignores it. `base` must reflect the facts *before* `ops`.
    ///
    /// Exactly one chunk: [`Self::begin_batch`], [`Self::apply_chunk`], [`Self::finish_batch`].
    pub fn apply_ops_staged_view(
        &mut self,
        ops: &[WorldBatchOp],
        limits: &NetworkLimits,
        base: Option<&BaseLookup<'_>>,
    ) -> Result<NetworkDeltaReport, WorldError> {
        let mut ctx = self.begin_batch();
        self.apply_chunk(&mut ctx, ops, limits, base)?;
        self.finish_batch_cascade(ctx, limits, base)
    }

    /// Finish a chunked batch and run the decision-chain cascade: winner rows of the
    /// settlements this batch changed are published to the virtual decision relations,
    /// and any further settlements they cause are applied until quiescence. Every route
    /// that ends a batch (single chunk, multi-chunk, replay) must use this so the
    /// outcome never depends on where chunk boundaries fell.
    pub fn finish_batch_cascade(
        &mut self,
        ctx: BatchCtx,
        limits: &NetworkLimits,
        base: Option<&BaseLookup<'_>>,
    ) -> Result<NetworkDeltaReport, WorldError> {
        let mut total_work = ctx.total_work;
        let mut total_matches = ctx.total_matches;
        let mut total_expressions = ctx.total_expressions;
        self.seed_winner_scan_records(&ctx)?;
        let mut final_report = self.finish_batch(ctx);
        let mut stage_report = final_report.clone();
        loop {
            let winner_ops = self.winner_row_ops(&stage_report)?;
            if winner_ops.is_empty() {
                break;
            }
            let prior_revision = self.current_revision;
            let mut next_ctx = self.begin_batch();
            next_ctx.total_work = total_work;
            next_ctx.total_matches = total_matches;
            next_ctx.total_expressions = total_expressions;
            self.apply_chunk(&mut next_ctx, &winner_ops, limits, base)?;
            total_work = next_ctx.total_work;
            total_matches = next_ctx.total_matches;
            total_expressions = next_ctx.total_expressions;
            self.seed_winner_scan_records(&next_ctx)?;
            let next_report = self.finish_batch(next_ctx);
            self.current_revision = prior_revision;
            stage_report = next_report.clone();
            merge_stage_report(&mut final_report, next_report);
        }
        Ok(final_report)
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
            if !self.virtual_relations.contains(&name) {
                continue;
            }
            let Some(decide) = self.decides.get(&name) else {
                continue;
            };
            for entity_id in entities {
                let entity_value = Value::from_typed_bytes(entity_id.as_bytes(), &decide.per_type)?;
                let key = WorldKey::new(entity_value.to_bytes());
                if let Some(winner) = self.get_settlement(&name, &entity_id)? {
                    ops.push(WorldBatchOp::Upsert {
                        relation: name.clone(),
                        key,
                        tuple: winner_row_tuple(&name, decide, &entity_value, &winner)?,
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

    /// A restored network's winner `Scan`s start with empty `records`, so the previously
    /// published winner row of a changed entity could not be retracted and stale rows
    /// would accumulate downstream. Before the cascade publishes new winner rows, put the
    /// row each touched entity held *before* this batch (decoded from its prior
    /// settlement) back into the `Scan`. Lazy: only entities this batch touched, so cold
    /// open stays independent of the number of decisions.
    fn seed_winner_scan_records(&mut self, ctx: &BatchCtx) -> Result<(), WorldError> {
        if self.virtual_relations.is_empty() {
            return Ok(());
        }
        let schemas = self.program_env.as_ref().map(|pe| pe.schemas.clone());
        let empty_schemas = BTreeMap::new();
        let schemas = schemas.as_deref().unwrap_or(&empty_schemas);
        for ((decide, entity), track) in &ctx.decisions {
            if !self.virtual_relations.contains(decide) {
                continue;
            }
            let Some(prior) = &track.prior else { continue };
            let Some(block) = self.decides.get(decide) else {
                continue;
            };
            let old = decode_settled_decision(entity, prior.as_bytes())?;
            let entity_value = Value::from_typed_bytes(entity.as_bytes(), &block.per_type)?;
            let key = WorldKey::new(entity_value.to_bytes());
            let tuple = winner_row_tuple(decide, block, &entity_value, &old)?;
            let scans = self
                .relation_subscribers
                .get(decide)
                .cloned()
                .unwrap_or_default();
            for scan_op_id in scans {
                if let Some(OperatorState::Scan {
                    relation,
                    schema,
                    records,
                    ..
                }) = self.operator_states.get_mut(&scan_op_id.0)
                {
                    if records.get(&key).is_none() {
                        let inter =
                            scan_intermediate(relation, Some(&key), schema, &tuple, schemas)?;
                        records.insert(key.clone(), (tuple.clone(), inter));
                    }
                }
            }
        }
        Ok(())
    }

    /// Start a batch that is applied as one or more contiguous chunks of its normalized
    /// operations (CPD). The chunks together must be exactly the batch's operations, in
    /// order; the result is independent of where the chunk boundaries fall.
    pub fn begin_batch(&self) -> BatchCtx {
        BatchCtx {
            report: NetworkDeltaReport {
                revision: self.current_revision + 1,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Apply one contiguous slice of a batch (operator network, frontier and decision
    /// tree); see [`Self::begin_batch`]. Does not advance the revision and does not
    /// report settlements: [`Self::finish_batch`] does, from the net change over all chunks.
    /// On `Err` the network is left in an unspecified state: discard it.
    pub fn apply_chunk(
        &mut self,
        ctx: &mut BatchCtx,
        ops: &[WorldBatchOp],
        limits: &NetworkLimits,
        base: Option<&BaseLookup<'_>>,
    ) -> Result<(), WorldError> {
        let _scope = self.state.trace_scope("apply");
        ctx.chunks += 1;
        let mut report = std::mem::take(&mut ctx.report);
        report.ops_applied += ops.len();
        let mut diagnostics = std::mem::take(&mut ctx.diagnostics);
        let mut total_work: u64 = ctx.total_work;
        let mut total_matches: u64 = ctx.total_matches;
        let mut total_expressions: u64 = ctx.total_expressions;

        let mut ready_operators: std::collections::VecDeque<OperatorId> =
            std::collections::VecDeque::new();
        let mut in_ready: BTreeSet<OperatorId> = BTreeSet::new();
        let mut operator_queues: BTreeMap<
            OperatorId,
            std::collections::VecDeque<OperatorWorkItem>,
        > = BTreeMap::new();

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
        let resident = self.base_resident;
        if !resident
            && base.is_none()
            && ops
                .iter()
                .any(|op| !self.virtual_relations.contains(op.relation()))
        {
            return Err(WorldError::NetworkError(
                "network was restored without its base facts; apply through a BaseLookup".into(),
            ));
        }
        let empty_schemas = BTreeMap::new();
        let schemas = self
            .program_env
            .as_ref()
            .map(|pe| pe.schemas.as_ref())
            .unwrap_or(&empty_schemas);
        for op in ops {
            let rel_name = op.relation();
            let is_winner_relation = self.virtual_relations.contains(rel_name);
            let mut base_rel = self.base_relations.get_mut(rel_name);
            if base_rel.is_none() && !is_winner_relation {
                return Err(WorldError::UnknownRelation(rel_name.to_string()));
            }

            let scan_ops = self
                .relation_subscribers
                .get(rel_name)
                .cloned()
                .unwrap_or_default();
            // The pre-batch tuple of a changed key, once per op for a restored network
            // (every Scan of the relation sees the same one).
            let mut looked_up: Option<Option<WorldTuple>> = None;
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
                    let key = op.key();
                    // The tuple this key currently holds in the Scan, as an intermediate.
                    let old_inter: Option<IntermediateTuple> = if is_winner_relation || resident {
                        records.get(key).map(|(_, i)| i.clone())
                    } else {
                        if looked_up.is_none() {
                            let lookup = base.expect("checked above");
                            looked_up = Some(lookup(rel_name, key)?);
                        }
                        match looked_up.as_ref().and_then(|t| t.as_ref()) {
                            Some(t) => {
                                Some(scan_intermediate(relation, Some(key), schema, t, schemas)?)
                            }
                            None => None,
                        }
                    };
                    match op {
                        WorldBatchOp::Upsert { key, tuple, .. } => {
                            if let Some(base_rel) = base_rel.as_mut() {
                                if resident {
                                    base_rel.insert(key.clone(), tuple.clone());
                                }
                            }
                            let derivation = Derivation::base(relation, key);
                            let intermediate =
                                scan_intermediate(relation, Some(key), schema, tuple, schemas)?;

                            // If old record existed at key, retract it first
                            if let Some(old_inter) = old_inter {
                                if old_inter == intermediate {
                                    // Idempotent value, no change
                                    continue;
                                }
                                let ret_delta = TupleDelta::Retract {
                                    tuple: old_inter,
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
                            if resident || is_winner_relation {
                                records.insert(key.clone(), (tuple.clone(), intermediate));
                            }
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
                                if resident {
                                    base_rel.remove(key);
                                }
                            }
                            if resident || is_winner_relation {
                                records.remove(key);
                            }
                            if let Some(old_inter) = old_inter {
                                let derivation = Derivation::base(relation, key);
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
        let mut deferred_groups = BTreeSet::new();

        while let Some(op_id) = ready_operators.pop_front() {
            in_ready.remove(&op_id);
            let op_idx = op_id.0;
            let is_group_operator = matches!(
                self.dag.nodes[op_idx],
                OperatorNode::GroupedCount { .. }
                    | OperatorNode::ParentCount { .. }
                    | OperatorNode::DisjointUnion { .. }
            );
            if is_group_operator && has_pending_ancestor(op_id, &self.dag, &operator_queues) {
                deferred_groups.insert(op_id);
                continue;
            }
            let mut work_in_slice: usize = 0;
            let mut out_deltas = Vec::new();
            let mut union_before: BTreeMap<ValueList, Option<IntermediateTuple>> = BTreeMap::new();
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

                            let out_deriv = deriv.unary(op_id);

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
                            total_expressions += 1;
                            if limits
                                .max_expressions
                                .is_some_and(|max| total_expressions > max)
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
                                let out_deriv = deriv.unary(op_id);
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
                                    total_expressions += 1;
                                    if limits
                                        .max_expressions
                                        .is_some_and(|max| total_expressions > max)
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
                                    total_expressions += 1;
                                    if limits
                                        .max_expressions
                                        .is_some_and(|max| total_expressions > max)
                                    {
                                        return Err(WorldError::BudgetExhausted);
                                    }
                                    let val = eval_expr(expr, &tuple, &self.functions)?;
                                    projected.insert(name.clone(), val);
                                }
                            }

                            let out_deriv = deriv.unary(op_id);

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
                        left_side,
                        right_side,
                    } => {
                        let left_op = *left;
                        let store = &self.state;
                        let counts = &self.leaf_counts;

                        let (delta, is_left, join_key, matches, cursor, remaining) = match item {
                            OperatorWorkItem::JoinContinuation {
                                delta,
                                is_left,
                                join_key,
                                matches,
                                cursor,
                                remaining,
                            } => (delta, is_left, join_key, matches, cursor, remaining),
                            OperatorWorkItem::SideDelta { .. } => {
                                unreachable!("ordinary join uses untagged deltas")
                            }
                            OperatorWorkItem::Delta(delta) => {
                                let (tuple, deriv) = match &delta {
                                    TupleDelta::Insert { tuple, derivation }
                                    | TupleDelta::Retract { tuple, derivation } => {
                                        (tuple, derivation)
                                    }
                                };

                                // Same rule as before: a derivation rooted at an operator is
                                // left iff that is the left input; a scan output is left iff the
                                // left input is a scan of the same relation (self-join quirk).
                                let is_left = match &deriv.c.root {
                                    DRoot::Op(op) => *op == left_op,
                                    DRoot::Base(relation) => match &self.dag.nodes[left_op.0] {
                                        OperatorNode::Scan { relation: r, .. } => {
                                            r.as_str() == &**relation
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
                                let (own, other) = if is_left {
                                    (left_side, right_side)
                                } else {
                                    (right_side, left_side)
                                };

                                if !deriv.c.leaf_count_ok(counts) {
                                    return Err(corrupt_derivation());
                                }
                                let vk = ValueList(join_key.clone());
                                let own_id = deriv.c.digest();
                                let existing = own.read(&vk, store)?;
                                if is_ins {
                                    let inner = existing.unwrap_or_default().upsert(
                                        own_id,
                                        JoinEntry {
                                            tuple: tuple.clone(),
                                            deriv: deriv.c.clone(),
                                        },
                                        store,
                                    )?;
                                    own.write(vk.clone(), inner, store)?;
                                } else if let Some(inner) = existing {
                                    let inner = inner.remove(&own_id, store)?;
                                    own.write(vk.clone(), inner, store)?;
                                }
                                let matches = other.read(&vk, store)?.unwrap_or_default();

                                let remaining = matches.len();
                                (delta, is_left, join_key, matches, None, remaining)
                            }
                        };

                        let is_ins = delta.is_insert();
                        let (tuple, deriv) = match &delta {
                            TupleDelta::Insert { tuple, derivation }
                            | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                        };

                        let remaining_quantum = quantum.saturating_sub(work_in_slice);
                        let remaining_matches = remaining;
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

                        let (page, next_cursor) = if take_count == 0 {
                            (Vec::new(), None)
                        } else {
                            matches.page(cursor.as_ref(), take_count, store)?
                        };
                        let own_deriv = deriv;
                        // Diagnostics are accumulated per page and flushed once (also on the
                        // budget-exhausted exits), instead of one string clone and three map
                        // updates per match.
                        let mut page_matches = 0u64;
                        let flush_diag = |diagnostics: &mut NetworkDiagnostics, n: u64| {
                            if n == 0 {
                                return;
                            }
                            *diagnostics.operator_work.entry(op_id).or_default() += n;
                            *diagnostics.join_fanout.entry(op_id).or_default() += n;
                            let key_str = format!(
                                "{op_id:?}:{}",
                                join_key
                                    .iter()
                                    .map(|v| v.to_string())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            );
                            *diagnostics.hot_join_keys.entry(key_str).or_default() += n;
                        };

                        for (_, entry) in page.iter() {
                            let other_tuple = &entry.tuple;
                            if !entry.deriv.leaf_count_ok(counts) {
                                return Err(corrupt_derivation());
                            }
                            // Full leaf-kind check of a stored derivation; a mismatch is
                            // corrupt state (also in release builds).
                            entry
                                .deriv
                                .check_shape(&self.dag)
                                .map_err(|e| WorldError::from(StateError::from(e)))?;
                            let other_deriv =
                                Derivation::from_stored(entry.deriv.clone(), &self.dag)
                                    .map_err(|e| WorldError::from(StateError::from(e)))?;
                            let (comp_tuple, comp_deriv) = if is_left {
                                (
                                    tuple.merge(other_tuple),
                                    Derivation::join(op_id, own_deriv, &other_deriv),
                                )
                            } else {
                                (
                                    other_tuple.merge(tuple),
                                    Derivation::join(op_id, &other_deriv, own_deriv),
                                )
                            };

                            work_in_slice += 1;
                            total_work += 1;
                            total_matches += 1;
                            page_matches += 1;

                            if limits.max_work.is_some_and(|max| total_work > max)
                                || limits.max_matches.is_some_and(|max| total_matches > max)
                            {
                                flush_diag(&mut diagnostics, page_matches);
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

                        flush_diag(&mut diagnostics, page_matches);
                        let new_remaining = remaining.saturating_sub(page.len());
                        if new_remaining > 0 && next_cursor.is_some() {
                            operator_queues.entry(op_id).or_default().push_front(
                                OperatorWorkItem::JoinContinuation {
                                    delta,
                                    is_left,
                                    join_key,
                                    matches,
                                    cursor: next_cursor,
                                    remaining: new_remaining,
                                },
                            );
                        }
                    }
                    OperatorState::AntiJoin {
                        left_keys,
                        right_keys,
                        left_side,
                        right_side,
                        ..
                    } => {
                        let OperatorWorkItem::SideDelta { delta, is_left } = item else {
                            unreachable!("anti-join receives side-tagged deltas")
                        };
                        let (tuple, derivation) = match &delta {
                            TupleDelta::Insert { tuple, derivation }
                            | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                        };
                        let key =
                            tuple_index_key(tuple, if is_left { left_keys } else { right_keys })?;
                        let store = &self.state;
                        if is_left {
                            let right_count = right_side.read(&key, store)?.map_or(0, |m| m.len());
                            let (old_count, new_count) =
                                indexed_side_apply(left_side, key, &delta, store)?;
                            if old_count != new_count && right_count == 0 {
                                let out_deriv =
                                    Derivation::from_stored(derivation.c.clone(), &self.dag)
                                        .map_err(|e| WorldError::from(StateError::from(e)))?
                                        .unary(op_id);
                                let out = if delta.is_insert() {
                                    TupleDelta::Insert {
                                        tuple: tuple.clone(),
                                        derivation: out_deriv,
                                    }
                                } else {
                                    TupleDelta::Retract {
                                        tuple: tuple.clone(),
                                        derivation: out_deriv,
                                    }
                                };
                                out_deltas.push(out);
                            }
                        } else {
                            let (old_count, new_count) =
                                indexed_side_apply(right_side, key.clone(), &delta, store)?;
                            if (old_count == 0) != (new_count == 0) {
                                let remaining_rows = limits
                                    .max_matches
                                    .map(|max| max.saturating_sub(total_matches))
                                    .into_iter()
                                    .chain(
                                        limits.max_work.map(|max| max.saturating_sub(total_work)),
                                    )
                                    .min();
                                for entry in
                                    indexed_side_rows(left_side, &key, store, remaining_rows)?
                                {
                                    total_matches += 1;
                                    total_work += 1;
                                    *diagnostics.operator_work.entry(op_id).or_default() += 1;
                                    let left_deriv =
                                        Derivation::from_stored(entry.deriv, &self.dag)
                                            .map_err(|e| WorldError::from(StateError::from(e)))?
                                            .unary(op_id);
                                    out_deltas.push(if old_count == 0 {
                                        TupleDelta::Retract {
                                            tuple: entry.tuple,
                                            derivation: left_deriv,
                                        }
                                    } else {
                                        TupleDelta::Insert {
                                            tuple: entry.tuple,
                                            derivation: left_deriv,
                                        }
                                    });
                                }
                            }
                        }
                    }
                    OperatorState::DisjointUnion {
                        key_fields,
                        left_side,
                        right_side,
                        ..
                    } => {
                        let OperatorWorkItem::SideDelta { delta, is_left } = item else {
                            unreachable!("union receives side-tagged deltas")
                        };
                        let key = tuple_index_key(delta.tuple(), key_fields)?;
                        if !union_before.contains_key(&key) {
                            union_before.insert(
                                key.clone(),
                                disjoint_union_tuple(left_side, right_side, &key, &self.state)?,
                            );
                        }
                        let side = if is_left { left_side } else { right_side };
                        indexed_side_apply(side, key, &delta, &self.state)?;
                    }
                    OperatorState::ParentCount {
                        left_key,
                        right_key,
                        output_key,
                        output_count,
                        left_side,
                        right_side,
                        ..
                    } => {
                        let OperatorWorkItem::SideDelta { delta, is_left } = item else {
                            unreachable!("parent-count receives side-tagged deltas")
                        };
                        let (tuple, derivation) = match &delta {
                            TupleDelta::Insert { tuple, derivation }
                            | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                        };
                        let field = if is_left { &*left_key } else { &*right_key };
                        let key = tuple_index_key(tuple, std::slice::from_ref(field))?;
                        let value = key.0.first().cloned().unwrap_or(Value::Null);
                        let store = &self.state;
                        if is_left {
                            let count = right_side.read(&key, store)?.map_or(0, |m| m.len());
                            let (old_supports, new_supports) =
                                indexed_side_apply(left_side, key, &delta, store)?;
                            if old_supports != new_supports {
                                let left_deriv =
                                    Derivation::from_stored(derivation.c.clone(), &self.dag)
                                        .map_err(|e| WorldError::from(StateError::from(e)))?
                                        .unary(op_id);
                                let out_tuple =
                                    parent_count_tuple(&value, output_key, output_count, count);
                                out_deltas.push(if delta.is_insert() {
                                    TupleDelta::Insert {
                                        tuple: out_tuple,
                                        derivation: left_deriv,
                                    }
                                } else {
                                    TupleDelta::Retract {
                                        tuple: out_tuple,
                                        derivation: left_deriv,
                                    }
                                });
                            }
                        } else {
                            let (old_count, new_count) =
                                indexed_side_apply(right_side, key.clone(), &delta, store)?;
                            if old_count != new_count {
                                let remaining_rows = limits
                                    .max_matches
                                    .map(|max| max.saturating_sub(total_matches))
                                    .into_iter()
                                    .chain(
                                        limits.max_work.map(|max| max.saturating_sub(total_work)),
                                    )
                                    .min();
                                for entry in
                                    indexed_side_rows(left_side, &key, store, remaining_rows)?
                                {
                                    total_matches += 1;
                                    total_work += 1;
                                    *diagnostics.operator_work.entry(op_id).or_default() += 1;
                                    let left_value = tuple_index_key(
                                        &entry.tuple,
                                        std::slice::from_ref(left_key),
                                    )?
                                    .0
                                    .into_iter()
                                    .next()
                                    .unwrap_or(Value::Null);
                                    let left_deriv =
                                        Derivation::from_stored(entry.deriv, &self.dag)
                                            .map_err(|e| WorldError::from(StateError::from(e)))?
                                            .unary(op_id);
                                    out_deltas.push(TupleDelta::Retract {
                                        tuple: parent_count_tuple(
                                            &left_value,
                                            output_key,
                                            output_count,
                                            old_count,
                                        ),
                                        derivation: left_deriv.clone(),
                                    });
                                    out_deltas.push(TupleDelta::Insert {
                                        tuple: parent_count_tuple(
                                            &left_value,
                                            output_key,
                                            output_count,
                                            new_count,
                                        ),
                                        derivation: left_deriv,
                                    });
                                }
                            }
                        }
                    }
                    OperatorState::Distinct { supports, .. } => match item {
                        OperatorWorkItem::Delta(delta) => {
                            let is_ins = delta.is_insert();
                            let (tuple, deriv) = match delta {
                                TupleDelta::Insert { tuple, derivation }
                                | TupleDelta::Retract { tuple, derivation } => (tuple, derivation),
                            };
                            let store = &self.state;

                            let tuple_key = distinct_key(&tuple);
                            let out_deriv = Derivation::distinct(op_id, tuple_key);

                            let (old_count, new_count) = if is_ins {
                                supports.add(tuple.clone(), deriv.c.clone(), store)?
                            } else {
                                supports.remove(&tuple, &deriv.c.digest(), store)?
                            };

                            if is_ins {
                                if old_count == 0 {
                                    out_deltas.push(TupleDelta::Insert {
                                        tuple,
                                        derivation: out_deriv,
                                    });
                                }
                            } else if old_count > 0 && new_count == 0 {
                                out_deltas.push(TupleDelta::Retract {
                                    tuple,
                                    derivation: out_deriv,
                                });
                            }
                        }
                        _ => unreachable!(),
                    },
                    OperatorState::GroupedCount {
                        group_keys,
                        compiled_group_keys,
                        projections,
                        compiled_aggregates,
                        compiled_collect_orders,
                        groups,
                        aggregates,
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
                                        total_expressions += 1;
                                        if limits
                                            .max_expressions
                                            .is_some_and(|max| total_expressions > max)
                                        {
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
                                        total_expressions += 1;
                                        if limits
                                            .max_expressions
                                            .is_some_and(|max| total_expressions > max)
                                        {
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
                                    total_expressions += 1;
                                    if limits
                                        .max_expressions
                                        .is_some_and(|max| total_expressions > max)
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

                            let mut order_values =
                                Vec::with_capacity(compiled_collect_orders.len());
                            for compiled in compiled_collect_orders {
                                if let Some(compiled) = compiled {
                                    diagnostics.expressions_evaluated += 1;
                                    total_expressions += 1;
                                    if limits
                                        .max_expressions
                                        .is_some_and(|max| total_expressions > max)
                                    {
                                        return Err(WorldError::BudgetExhausted);
                                    }
                                    order_values.push(Some(Value::from_scalar(
                                        compiled
                                            .eval(&bindings)
                                            .map_err(WorldError::NetworkError)?,
                                    )?));
                                } else {
                                    order_values.push(None);
                                }
                            }

                            let store = &self.state;
                            let gk = ValueList(key_vals.clone());

                            if !grouped_before.contains_key(&key_vals) {
                                let existing_count = groups.read(&gk, store)?.len();
                                let existing_tuple = if existing_count > 0 {
                                    let existing_accum = match aggregates.get(&gk, store)? {
                                        Some(stored_list) => GroupAccumulator::from_value_list(
                                            &stored_list,
                                            projections,
                                        )?,
                                        None => {
                                            return Err(WorldError::NetworkError(
                                                "missing persisted aggregate accumulator".into(),
                                            ))
                                        }
                                    };
                                    let mut tuple_tree_work = 0;
                                    let tuple = build_grouped_tuple(
                                        projections,
                                        &key_vals,
                                        &existing_accum,
                                        existing_count,
                                        &gk,
                                        aggregates,
                                        store,
                                        &mut tuple_tree_work,
                                    )?;
                                    charge_aggregate_tree_work(
                                        op_id,
                                        tuple_tree_work,
                                        &mut total_work,
                                        &mut diagnostics,
                                        limits,
                                    )?;
                                    Some(tuple)
                                } else {
                                    None
                                };
                                grouped_before.insert(key_vals.clone(), existing_tuple);
                            }

                            let mut accum = match aggregates.get(&gk, store)? {
                                Some(stored_list) => {
                                    GroupAccumulator::from_value_list(&stored_list, projections)?
                                }
                                None if groups.read(&gk, store)?.len() == 0 => {
                                    GroupAccumulator::empty(projections)
                                }
                                None => {
                                    return Err(WorldError::NetworkError(
                                        "missing persisted aggregate accumulator".into(),
                                    ))
                                }
                            };

                            let (old_count, new_count) = if is_ins {
                                groups.add(gk.clone(), deriv.c.digest(), store)?
                            } else {
                                groups.remove(&gk, &deriv.c.digest(), store)?
                            };

                            if new_count != old_count {
                                let child_key = derivation_provenance_key(&deriv.c.leaves);
                                let tree_work = accum.apply_delta(
                                    is_ins,
                                    projections,
                                    &aggregate_values,
                                    &order_values,
                                    child_key,
                                    deriv.c.digest(),
                                    &gk,
                                    aggregates,
                                    store,
                                )?;
                                charge_aggregate_tree_work(
                                    op_id,
                                    tree_work,
                                    &mut total_work,
                                    &mut diagnostics,
                                    limits,
                                )?;

                                if new_count == 0 {
                                    aggregates.remove(&gk, store)?;
                                } else {
                                    aggregates.set(
                                        gk.clone(),
                                        accum.to_value_list(projections),
                                        store,
                                    )?;
                                }
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
                    aggregates,
                    ..
                }) = self.operator_states.get_mut(&op_idx)
                {
                    let store = &self.state;
                    for (group_key, old_tuple) in grouped_before {
                        let gk = ValueList(group_key.clone());
                        let current_count = groups.read(&gk, store)?.len();
                        let new_tuple = if current_count > 0 {
                            let accum = match aggregates.get(&gk, store)? {
                                Some(stored_list) => {
                                    GroupAccumulator::from_value_list(&stored_list, projections)?
                                }
                                None => {
                                    return Err(WorldError::NetworkError(
                                        "missing persisted aggregate accumulator".into(),
                                    ))
                                }
                            };
                            let mut tuple_tree_work = 0;
                            let tuple = build_grouped_tuple(
                                projections,
                                &group_key,
                                &accum,
                                current_count,
                                &gk,
                                aggregates,
                                store,
                                &mut tuple_tree_work,
                            )?;
                            charge_aggregate_tree_work(
                                op_id,
                                tuple_tree_work,
                                &mut total_work,
                                &mut diagnostics,
                                limits,
                            )?;
                            Some(tuple)
                        } else {
                            None
                        };

                        if old_tuple == new_tuple {
                            continue;
                        }

                        let derivation = Derivation::group(op_id, group_key);
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

            if let Some(OperatorState::DisjointUnion {
                left_side,
                right_side,
                ..
            }) = self.operator_states.get_mut(&op_idx)
            {
                for (key, old) in union_before {
                    let new = disjoint_union_tuple(left_side, right_side, &key, &self.state)?;
                    if old == new {
                        continue;
                    }
                    let derivation = Derivation::group(op_id, key.0);
                    if let Some(tuple) = old {
                        out_deltas.push(TupleDelta::Retract {
                            tuple,
                            derivation: derivation.clone(),
                        });
                    }
                    if let Some(tuple) = new {
                        out_deltas.push(TupleDelta::Insert { tuple, derivation });
                    }
                }
            }

            // Parent counts are batch observations too. Let all ancestor work
            // settle, then cancel transient counts before the next cascade stage
            // joins them. Otherwise each intermediate count multiplies downstream
            // work even though only the final count can survive the transaction.
            if matches!(self.dag.nodes[op_idx], OperatorNode::ParentCount { .. }) {
                out_deltas = net_effect(&out_deltas).into_iter().cloned().collect();
            }

            // Propagate out_deltas. Each consumer of the batch gets a copy except the last,
            // which takes ownership (avoids one full clone of every batch).
            if !out_deltas.is_empty() {
                let n_out = out_deltas.len();
                report.intermediate_deltas_count += n_out;
                let outputs = self.output_relations.get(&op_id);
                let down = self.downstream.get(&op_id);
                let mut uses_left = outputs.map_or(0, |o| o.len()) + down.map_or(0, |d| d.len());

                if let Some(outputs) = outputs {
                    for rel_name in outputs {
                        *diagnostics
                            .relation_deltas
                            .entry(rel_name.clone())
                            .or_default() += n_out as u64;

                        if !self.base_relations.contains_key(rel_name) {
                            let store = &self.state;
                            let rel =
                                self.derived_relations.get_mut(rel_name).ok_or_else(|| {
                                    WorldError::NetworkError(format!(
                                        "output relation {rel_name} has no derived index"
                                    ))
                                })?;
                            for od in &out_deltas {
                                let is_ins = od.is_insert();

                                let changed = match rel {
                                    // A Distinct output emits only on 0 -> 1 / 1 -> 0 support
                                    // transitions of its own index, which is the relation.
                                    DerivedRel::DistinctOutput(_) => true,
                                    DerivedRel::Counted(supports) => {
                                        let id = od.derivation().digest();
                                        let (old, new) = if is_ins {
                                            supports.add(od.tuple().clone(), id, store)?
                                        } else {
                                            supports.remove(od.tuple(), &id, store)?
                                        };
                                        if is_ins {
                                            old == 0 && new > 0
                                        } else {
                                            old > 0 && new == 0
                                        }
                                    }
                                };

                                if changed {
                                    if is_ins {
                                        report.derived_tuples_inserted += 1;
                                    } else {
                                        report.derived_tuples_retracted += 1;
                                    }
                                }
                            }
                        }

                        uses_left -= 1;
                        let batch = if uses_left == 0 {
                            std::mem::take(&mut out_deltas)
                        } else {
                            out_deltas.clone()
                        };
                        relation_deltas
                            .entry(rel_name.clone())
                            .or_default()
                            .extend(batch);
                    }
                }

                if let Some(down) = down {
                    for consumer in down {
                        uses_left -= 1;
                        let batch = if uses_left == 0 {
                            std::mem::take(&mut out_deltas)
                        } else {
                            out_deltas.clone()
                        };
                        for od in batch {
                            let item = match self.operator_states.get(&consumer.0) {
                                Some(OperatorState::AntiJoin { left, right, .. })
                                | Some(OperatorState::DisjointUnion { left, right, .. })
                                | Some(OperatorState::ParentCount { left, right, .. }) => {
                                    OperatorWorkItem::SideDelta {
                                        delta: od,
                                        is_left: *left == op_id && *right != op_id,
                                    }
                                }
                                _ => OperatorWorkItem::Delta(od),
                            };
                            enqueue(
                                *consumer,
                                item,
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
        // Buckets are read once, edited in memory across all deltas of the batch, and
        // written back once per entity (`None`: the entity has no live candidate).
        let mut staged_buckets: BTreeMap<FrontierKey, Option<FrontierBucket>> = BTreeMap::new();
        // Globally retract-first: every relation's netted retractions, then every
        // insertion (stable within each class). A candidate's value can then never meet
        // a stale support from another relation, so the outcome does not depend on the
        // order relations are visited in or on where chunk boundaries fall (ADR-0046
        // CPD, H4). Batches that succeed produce the same state as before.
        let mut frontier_work: Vec<(&DecideBlock, &TupleDelta)> = Vec::new();
        for (relation, rel_deltas) in &relation_deltas {
            for decide_name in self
                .decision_subscribers
                .get(relation)
                .into_iter()
                .flatten()
            {
                let decide = &self.decides[decide_name];
                for delta in net_effect(rel_deltas) {
                    frontier_work.push((decide, delta));
                }
            }
        }
        frontier_work.sort_by_key(|(_, d)| d.is_insert());
        for (decide, delta) in frontier_work {
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
                let entity_id = entity_id_for(&eval_tuple, &decide.binder, &decide.per_field)?;
                touched_entities.insert((decide.name.clone(), entity_id.clone()));

                if is_ins {
                    let compiled = decide.compiled_proposals.get(p_idx);
                    let (guard_passed, val) = if let Some(compiled) = compiled {
                        let bindings = scalar_bindings(&eval_tuple)?;
                        diagnostics.expressions_evaluated += 1;
                        total_expressions += 1;
                        if limits
                            .max_expressions
                            .is_some_and(|max| total_expressions > max)
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
                            total_expressions += 1;
                            if limits
                                .max_expressions
                                .is_some_and(|max| total_expressions > max)
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
                        total_expressions += 1;
                        if limits
                            .max_expressions
                            .is_some_and(|max| total_expressions > max)
                        {
                            return Err(WorldError::BudgetExhausted);
                        }
                        let guard_passed =
                            eval_expr(&propose.guard, &eval_tuple, &self.functions)?.as_bool()?;
                        if guard_passed {
                            diagnostics.expressions_evaluated += 1;
                            total_expressions += 1;
                            if limits
                                .max_expressions
                                .is_some_and(|max| total_expressions > max)
                            {
                                return Err(WorldError::BudgetExhausted);
                            }
                            let val = eval_expr(&propose.value, &eval_tuple, &self.functions)?;
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

                        let store = &self.state;
                        let fk = FrontierKey::new(&decide.name, &entity_id);
                        if !staged_buckets.contains_key(&fk) {
                            let loaded = self.candidate_frontier.bucket(&fk, store)?;
                            staged_buckets.insert(fk.clone(), loaded);
                        }
                        let bucket = staged_buckets
                            .get_mut(&fk)
                            .expect("bucket staged above")
                            .get_or_insert_with(Default::default);
                        let ck = CandKey::new(&decide.name, &entity_id, &propose.name);
                        match bucket.0.get_mut(&propose.name) {
                            Some(rec) => {
                                if rec.head.value != val {
                                    return Err(WorldError::NetworkError(format!(
                                        "conflicting support values for decide '{}' \
                                                 entity '{entity_id}' candidate '{}': {:?} vs \
                                                 {val:?}",
                                        decide.name, propose.name, rec.head.value
                                    )));
                                }
                                self.candidate_frontier.add_support(
                                    &ck,
                                    rec,
                                    deriv.c.clone(),
                                    store,
                                )?;
                            }
                            None => {
                                report.candidates_inserted += 1;
                                bucket.0.insert(
                                    propose.name.clone(),
                                    CandidateRec {
                                        head: CandidateHead {
                                            priority: propose.priority,
                                            phase: 0,
                                            value: val,
                                            calendar_key: cal_key,
                                        },
                                        supports: SupportRef::Inline(deriv.c.clone()),
                                    },
                                );
                            }
                        }
                    }
                } else {
                    let store = &self.state;
                    let fk = FrontierKey::new(&decide.name, &entity_id);
                    if !staged_buckets.contains_key(&fk) {
                        let loaded = self.candidate_frontier.bucket(&fk, store)?;
                        staged_buckets.insert(fk.clone(), loaded);
                    }
                    let slot = staged_buckets.get_mut(&fk).expect("bucket staged above");
                    if let Some(bucket) = slot.as_mut() {
                        if let Some(rec) = bucket.0.get_mut(&propose.name) {
                            let ck = CandKey::new(&decide.name, &entity_id, &propose.name);
                            let remaining = self.candidate_frontier.remove_support(
                                &ck,
                                rec,
                                &deriv.c.digest(),
                                store,
                            )?;
                            if remaining == 0 {
                                bucket.0.remove(&propose.name);
                                report.candidates_retracted += 1;
                                if bucket.0.is_empty() {
                                    *slot = None;
                                }
                            }
                        }
                    }
                }
            }
        }

        for (decide, entity) in touched_entities {
            let key = decision_key(&decide, &entity);
            let settlement = match staged_buckets.get(&FrontierKey::new(&decide, &entity)) {
                Some(Some(bucket)) => settle(&entity, bucket),
                Some(None) => None,
                None => self.settlement_of(&decide, &entity)?,
            };

            // The pre-batch value is the tree's value the first time any chunk touches
            // the entity; the batch's settlement delta is last-vs-prior (H1), not a merge
            // of per-chunk reports.
            let current = self.decision_tree.get(&key, &self.state)?;
            let track = ctx
                .decisions
                .entry((decide.clone(), entity.clone()))
                .or_insert_with(|| DecisionTrack {
                    prior: current.clone(),
                    last: None,
                });
            if let Some(decision) = settlement {
                let tuple = decision_tuple(&decision);
                if current.as_ref() != Some(&tuple) {
                    self.decision_tree = self.decision_tree.upsert(key, tuple, &self.state)?;
                }
                track.last = Some(decision);
            } else {
                if current.is_some() {
                    self.decision_tree = self.decision_tree.remove(&key, &self.state)?;
                }
                track.last = None;
            }
        }

        for (fk, bucket) in staged_buckets {
            self.candidate_frontier
                .put_bucket(fk, bucket, &self.state)?;
        }

        ctx.report = report;
        ctx.diagnostics = diagnostics;
        ctx.total_work = total_work;
        ctx.total_matches = total_matches;
        ctx.total_expressions = total_expressions;
        Ok(())
    }

    /// End a batch: report the net settlement change over all chunks and advance the
    /// revision. For a single chunk this is exactly what the unchunked path reported.
    pub fn finish_batch(&mut self, ctx: BatchCtx) -> NetworkDeltaReport {
        let BatchCtx {
            mut report,
            diagnostics,
            decisions,
            ..
        } = ctx;
        for ((decide, entity), track) in decisions {
            match track.last {
                Some(decision) => {
                    if track.prior.as_ref() != Some(&decision_tuple(&decision)) {
                        report
                            .settlements
                            .entry(decide)
                            .or_default()
                            .insert(entity, decision);
                    }
                }
                None => {
                    if track.prior.is_some() {
                        report
                            .removed_settlements
                            .entry(decide)
                            .or_default()
                            .insert(entity);
                    }
                }
            }
        }
        report.diagnostics = diagnostics;
        self.current_revision += 1;
        report
    }

    /// Number of settlements in the persisted decision trie.
    pub fn decision_count(&self) -> usize {
        self.decision_tree.len()
    }

    /// Whether the decision trie holds exactly `decision` for `(decide, entity)`.
    pub fn decision_matches(
        &self,
        decide: &str,
        entity: &str,
        decision: &SettledDecision,
    ) -> Result<bool, WorldError> {
        Ok(self
            .decision_tree
            .get(&decision_key(decide, entity), &self.state)?
            .as_ref()
            == Some(&decision_tuple(decision)))
    }

    /// Whether the decision trie holds nothing for `(decide, entity)`.
    pub fn decision_absent(&self, decide: &str, entity: &str) -> Result<bool, WorldError> {
        Ok(self
            .decision_tree
            .get(&decision_key(decide, entity), &self.state)?
            .is_none())
    }

    /// Canonical decision root, maintained only for affected entities.
    pub fn decision_root(&self) -> Digest {
        self.decision_tree.root()
    }

    /// Retrieve all derived tuples produced for a relation, in `TupleRecord` order.
    ///
    /// `None` if `relation` is not a derived relation. Tuples are stored typed, so two
    /// distinct typed tuples whose display bytes coincide (`Int(1)` vs `Str("1")`) are two
    /// entries internally and collapse to one record only here, in the observable view.
    pub fn get_derived_tuples(
        &self,
        relation: &str,
    ) -> Result<Option<Vec<TupleRecord>>, WorldError> {
        let _scope = self.state.trace_scope("query");
        let Some(rel) = self.derived_relations.get(relation) else {
            return Ok(None);
        };
        let records: BTreeSet<TupleRecord> = self
            .derived_tuples(rel)?
            .iter()
            .map(IntermediateTuple::to_tuple_record)
            .collect();
        Ok(Some(records.into_iter().collect()))
    }

    /// The typed tuples of one derived relation.
    fn derived_tuples(&self, rel: &DerivedRel) -> Result<Vec<IntermediateTuple>, WorldError> {
        match rel {
            DerivedRel::Counted(supports) => supports.keys(&self.state),
            DerivedRel::DistinctOutput(op) => match self.operator_states.get(&op.0) {
                Some(OperatorState::Distinct { supports, .. }) => supports.keys(&self.state),
                _ => Err(WorldError::NetworkError(format!(
                    "derived relation refers to operator {} which is not a Distinct",
                    op.0
                ))),
            },
        }
    }

    /// Candidates (with their expanded supports) for a specific decide block and entity.
    pub fn get_candidates(
        &self,
        decide: &str,
        entity_id: &str,
    ) -> Result<Option<BTreeMap<String, CandidateEntry>>, WorldError> {
        let _scope = self.state.trace_scope("query");
        let Some(bucket) = self
            .candidate_frontier
            .bucket(&FrontierKey::new(decide, entity_id), &self.state)?
        else {
            return Ok(None);
        };
        Ok(Some(self.candidate_entries(decide, entity_id, &bucket)?))
    }

    fn candidate_entries(
        &self,
        decide: &str,
        entity_id: &str,
        bucket: &FrontierBucket,
    ) -> Result<BTreeMap<String, CandidateEntry>, WorldError> {
        let mut out = BTreeMap::new();
        for (name, rec) in &bucket.0 {
            let items = self
                .candidate_frontier
                .supports_of(&CandKey::new(decide, entity_id, name), rec)
                .members(&self.state)?;
            let mut supports = PSet::new();
            for id in self.expand_supports(&items)? {
                supports.insert(id);
            }
            out.insert(
                name.clone(),
                CandidateEntry {
                    candidate_name: name.clone(),
                    entity_id: entity_id.to_string(),
                    priority: rec.head.priority,
                    phase: rec.head.phase,
                    value: rec.head.value.clone(),
                    calendar_key: rec.head.calendar_key,
                    supports,
                },
            );
        }
        Ok(out)
    }

    /// Deliberate and select the winning candidate for an entity using canonical settlement discipline.
    pub fn get_settlement(
        &self,
        decide: &str,
        entity_id: &str,
    ) -> Result<Option<SettledDecision>, WorldError> {
        let _scope = self.state.trace_scope("query");
        self.settlement_of(decide, entity_id)
    }

    /// [`Self::get_settlement`] without opening a trace scope (used inside an apply).
    fn settlement_of(
        &self,
        decide: &str,
        entity_id: &str,
    ) -> Result<Option<SettledDecision>, WorldError> {
        Ok(self
            .candidate_frontier
            .bucket(&FrontierKey::new(decide, entity_id), &self.state)?
            .and_then(|bucket| settle(entity_id, &bucket)))
    }

    /// Settle decisions across all entities in all decide blocks.
    pub fn all_settlements(
        &self,
    ) -> Result<BTreeMap<String, BTreeMap<String, SettledDecision>>, WorldError> {
        let _scope = self.state.trace_scope("query");
        let mut out: BTreeMap<String, BTreeMap<String, SettledDecision>> = BTreeMap::new();
        // Streamed: each bucket is reduced to its settlement and dropped, so peak memory is
        // the settlements themselves rather than every decoded bucket besides.
        self.candidate_frontier
            .for_each_bucket(&self.state, |key, bucket| {
                if let Some(decision) = settle(&key.entity, &bucket) {
                    out.entry(key.decide)
                        .or_default()
                        .insert(key.entity, decision);
                }
                Ok(())
            })?;
        Ok(out)
    }

    /// One page of settlements from the candidate frontier, at most `limit` buckets (so at
    /// most `limit` settlements) after `cursor`, in deterministic trie order (not sorted by
    /// name), without materialising the rest. Every bucket is verified.
    #[allow(clippy::type_complexity)]
    pub fn settlements_page(
        &self,
        cursor: Option<&PageCursor<FrontierKey>>,
        limit: usize,
    ) -> Result<
        (
            Vec<(String, String, SettledDecision)>,
            Option<PageCursor<FrontierKey>>,
        ),
        WorldError,
    > {
        let _scope = self.state.trace_scope("query");
        let (rows, next) = self
            .candidate_frontier
            .buckets
            .page(cursor, limit, &self.state)?;
        let mut out = Vec::with_capacity(rows.len());
        for (key, bucket) in rows {
            verify_bucket(&key, &bucket)?;
            if let Some(decision) = settle(&key.entity, &bucket) {
                out.push((key.decide, key.entity, decision));
            }
        }
        Ok((out, next))
    }

    /// Capture current observable network state.
    pub fn current_state(&self) -> Result<WorldNetworkState, WorldError> {
        let _scope = self.state.trace_scope("query");
        let mut distinct_supports = BTreeMap::new();
        for (idx, state) in self.operator_states.iter() {
            if let OperatorState::Distinct { supports, .. } = state {
                let mut map = BTreeMap::new();
                for (it, items) in supports.entries(&self.state)? {
                    map.insert(it.to_tuple_record(), self.expand_supports(&items)?);
                }
                distinct_supports.insert(OperatorId(*idx), map);
            }
        }

        let mut derived_relations = BTreeMap::new();
        for (name, rel) in self.derived_relations.iter() {
            derived_relations.insert(
                name.clone(),
                self.derived_tuples(rel)?
                    .iter()
                    .map(IntermediateTuple::to_tuple_record)
                    .collect::<BTreeSet<_>>(),
            );
        }

        let mut candidate_frontier: BTreeMap<
            String,
            BTreeMap<String, BTreeMap<String, CandidateEntry>>,
        > = BTreeMap::new();
        let mut settlements: BTreeMap<String, BTreeMap<String, SettledDecision>> = BTreeMap::new();
        for (key, bucket) in self.candidate_frontier.all_buckets(&self.state)? {
            if let Some(decision) = settle(&key.entity, &bucket) {
                settlements
                    .entry(key.decide.clone())
                    .or_default()
                    .insert(key.entity.clone(), decision);
            }
            let entries = self.candidate_entries(&key.decide, &key.entity, &bucket)?;
            candidate_frontier
                .entry(key.decide)
                .or_default()
                .insert(key.entity, entries);
        }

        Ok(WorldNetworkState {
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
            derived_relations,
            candidate_frontier,
            settlements,
            distinct_supports,
        })
    }

    /// Expand stored compact supports to derivation ids.
    fn expand_supports(
        &self,
        items: &[CompactDeriv],
    ) -> Result<BTreeSet<DerivationId>, WorldError> {
        let mut out = BTreeSet::new();
        for c in items {
            out.insert(
                c.expand(&self.dag)
                    .map_err(|e| WorldError::from(StateError::from(e)))?,
            );
        }
        Ok(out)
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
        fresh.current_state()
    }

    /// Verify differential correctness: compare incrementally maintained state with oracle full recompute.
    pub fn verify_differential_correctness(&self) -> Result<(), WorldError> {
        let scratch_state = self.recompute_from_scratch()?;
        let incremental_state = self.current_state()?;

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
    ) -> Result<(), WorldError> {
        match deriv {
            DerivationId::Base { relation, key } => {
                if let Some(decide) = self.decides.get(relation) {
                    let entity_id = Value::from_typed_bytes(key.as_bytes(), &decide.per_type)
                        .map(|v| v.to_string())
                        .unwrap_or_else(|_| {
                            std::str::from_utf8(key.as_bytes())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|_| key.to_hex())
                        });
                    if let Some(settlement) = self.get_settlement(relation, &entity_id)? {
                        if let Some(bucket) = self
                            .candidate_frontier
                            .bucket(&FrontierKey::new(relation, &entity_id), &self.state)?
                        {
                            if let Some(rec) = bucket.0.get(&settlement.candidate_name) {
                                let supports = self.candidate_frontier.supports_of(
                                    &CandKey::new(relation, &entity_id, &settlement.candidate_name),
                                    rec,
                                );
                                let items = supports.members(&self.state)?;
                                for parent in self.expand_supports(&items)? {
                                    self.collect_base_facts_for_derivation(&parent, out)?;
                                }
                            }
                        }
                    }
                } else {
                    out.insert((relation.clone(), key.clone()));
                }
            }
            DerivationId::Unary { parent, .. } => {
                self.collect_base_facts_for_derivation(parent, out)?;
            }
            DerivationId::Join { left, right, .. } => {
                self.collect_base_facts_for_derivation(left, out)?;
                self.collect_base_facts_for_derivation(right, out)?;
            }
            DerivationId::Distinct { op, tuple_key } => {
                if let Some(OperatorState::Distinct { supports, .. }) =
                    self.operator_states.get(&op.0)
                {
                    let tuple = IntermediateTuple::state_from_bytes(tuple_key)
                        .map_err(|e| WorldError::from(StateError::from(e)))?;
                    let items = supports.read(&tuple, &self.state)?.members(&self.state)?;
                    for p in self.expand_supports(&items)? {
                        self.collect_base_facts_for_derivation(&p, out)?;
                    }
                }
            }
            DerivationId::Group { .. } => {}
        }
        Ok(())
    }

    pub fn collect_upstream_settlements_for_derivation(
        &self,
        deriv: &DerivationId,
        out: &mut BTreeSet<UpstreamSettlementLink>,
    ) -> Result<(), WorldError> {
        match deriv {
            DerivationId::Base { relation, key } => {
                if let Some(decide) = self.decides.get(relation) {
                    let entity_id = Value::from_typed_bytes(key.as_bytes(), &decide.per_type)
                        .map(|v| v.to_string())
                        .unwrap_or_else(|_| {
                            std::str::from_utf8(key.as_bytes())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|_| key.to_hex())
                        });
                    if let Some(settlement) = self.get_settlement(relation, &entity_id)? {
                        out.insert(UpstreamSettlementLink {
                            decide: relation.clone(),
                            entity_id: entity_id.clone(),
                            candidate: settlement.candidate_name.clone(),
                            revision: self.current_revision,
                        });
                        if let Some(bucket) = self
                            .candidate_frontier
                            .bucket(&FrontierKey::new(relation, &entity_id), &self.state)?
                        {
                            if let Some(rec) = bucket.0.get(&settlement.candidate_name) {
                                let supports = self.candidate_frontier.supports_of(
                                    &CandKey::new(relation, &entity_id, &settlement.candidate_name),
                                    rec,
                                );
                                let items = supports.members(&self.state)?;
                                for parent in self.expand_supports(&items)? {
                                    self.collect_upstream_settlements_for_derivation(&parent, out)?;
                                }
                            }
                        }
                    }
                }
            }
            DerivationId::Unary { parent, .. } => {
                self.collect_upstream_settlements_for_derivation(parent, out)?;
            }
            DerivationId::Join { left, right, .. } => {
                self.collect_upstream_settlements_for_derivation(left, out)?;
                self.collect_upstream_settlements_for_derivation(right, out)?;
            }
            DerivationId::Distinct { op, tuple_key } => {
                if let Some(OperatorState::Distinct { supports, .. }) =
                    self.operator_states.get(&op.0)
                {
                    let tuple = IntermediateTuple::state_from_bytes(tuple_key)
                        .map_err(|e| WorldError::from(StateError::from(e)))?;
                    let items = supports.read(&tuple, &self.state)?.members(&self.state)?;
                    for p in self.expand_supports(&items)? {
                        self.collect_upstream_settlements_for_derivation(&p, out)?;
                    }
                }
            }
            DerivationId::Group { .. } => {}
        }
        Ok(())
    }

    /// Deliberate and explain the decision for an entity key across any matching decide block.
    pub fn explain_decision(
        &self,
        entity_id: &str,
    ) -> Result<Option<DecisionExplanation>, WorldError> {
        for decide_name in self.decides.keys() {
            if let Some(exp) = self.explain_decision_for(decide_name, entity_id)? {
                return Ok(Some(exp));
            }
        }
        Ok(None)
    }

    /// Deliberate and explain the decision for an entity key in a specific decide block.
    pub fn explain_decision_for(
        &self,
        decide_name: &str,
        entity_id: &str,
    ) -> Result<Option<DecisionExplanation>, WorldError> {
        let _scope = self.state.trace_scope("query");
        let Some(bucket) = self
            .candidate_frontier
            .bucket(&FrontierKey::new(decide_name, entity_id), &self.state)?
        else {
            return Ok(None);
        };

        let settlement = settle(entity_id, &bucket);
        let winning_name = settlement.as_ref().map(|s| &s.candidate_name);

        let mut candidates = Vec::new();
        let mut contributing_base = BTreeSet::new();
        let mut upstream_settlements = BTreeSet::new();

        for (cand_name, rec) in &bucket.0 {
            let winning = winning_name == Some(cand_name);
            let supports = self
                .candidate_frontier
                .supports_of(&CandKey::new(decide_name, entity_id, cand_name), rec);
            let items = supports.members(&self.state)?;
            for deriv in self.expand_supports(&items)? {
                self.collect_base_facts_for_derivation(&deriv, &mut contributing_base)?;
                self.collect_upstream_settlements_for_derivation(
                    &deriv,
                    &mut upstream_settlements,
                )?;
            }
            candidates.push(CandidateExplanation {
                name: cand_name.clone(),
                priority: rec.head.priority,
                phase: rec.head.phase,
                value: rec.head.value.clone(),
                supports_count: supports.len(),
                winning,
            });
        }

        // Also explain proposals that were declared in the decide block but were NOT admitted
        if let Some(decide) = self.decides.get(decide_name) {
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
        }

        Ok(Some(DecisionExplanation {
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
        }))
    }
}

fn corrupt_derivation() -> WorldError {
    StateError::Codec(StateCodecError::NonCanonical(
        "stored derivation does not match its operator".to_string(),
    ))
    .into()
}

fn winner_row_tuple(
    name: &str,
    decide: &DecideBlock,
    entity_value: &Value,
    winner: &SettledDecision,
) -> Result<WorldTuple, WorldError> {
    let priority = i64::try_from(winner.priority).map_err(|_| {
        WorldError::NetworkError(format!(
            "winner priority for '{name}' exceeds the winner row Int range"
        ))
    })?;
    let mut row = TupleRecord::new();
    row.set(&decide.per_field, entity_value.to_bytes());
    row.set(
        "candidate",
        Value::Str(winner.candidate_name.clone()).to_bytes(),
    );
    row.set("priority", Value::Int(priority).to_bytes());
    row.set("value", winner.value.to_bytes());
    Ok(row.to_tuple())
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
        OperatorNode::EquiJoin { left, right, .. }
        | OperatorNode::AntiJoin { left, right, .. }
        | OperatorNode::DisjointUnion { left, right, .. }
        | OperatorNode::ParentCount { left, right, .. } => {
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
            OperatorNode::EquiJoin { left, right, .. }
            | OperatorNode::AntiJoin { left, right, .. }
            | OperatorNode::DisjointUnion { left, right, .. }
            | OperatorNode::ParentCount { left, right, .. } => {
                pending.push(*left);
                pending.push(*right);
            }
            OperatorNode::Scan { .. } | OperatorNode::DecisionRows { .. } => {}
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn build_grouped_tuple(
    projections: &[(String, GroupProjection)],
    key_vals: &[Value],
    state: &GroupAccumulator,
    count: usize,
    group_key: &ValueList,
    aggregates: &AggregateIndex,
    store: &StateStore,
    tree_work: &mut u64,
) -> Result<IntermediateTuple, WorldError> {
    let mut tuple = IntermediateTuple::new();
    for (idx, (name, proj)) in projections.iter().enumerate() {
        match proj {
            GroupProjection::Key(k_idx) => {
                let v = key_vals.get(*k_idx).cloned().unwrap_or(Value::Null);
                tuple.insert(name, v);
            }
            GroupProjection::Count => {
                tuple.insert(name, Value::Int(count as i64));
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
                let value = aggregates.extremum(
                    group_key,
                    idx,
                    state.extrema_roots[idx].as_ref(),
                    false,
                    store,
                    tree_work,
                )?;
                tuple.insert(name, value.unwrap_or(Value::Null));
            }
            GroupProjection::Max(_) => {
                let value = aggregates.extremum(
                    group_key,
                    idx,
                    state.extrema_roots[idx].as_ref(),
                    true,
                    store,
                    tree_work,
                )?;
                tuple.insert(name, value.unwrap_or(Value::Null));
            }
            GroupProjection::Collect { .. } => {
                let items: Vec<Value> = state.collected[idx]
                    .iter()
                    .map(|(_, _, _, item)| item.clone())
                    .collect();
                tuple.insert(name, Value::List(items));
            }
        }
    }
    Ok(tuple)
}

fn charge_aggregate_tree_work(
    op_id: OperatorId,
    work: u64,
    total_work: &mut u64,
    diagnostics: &mut NetworkDiagnostics,
    limits: &NetworkLimits,
) -> Result<(), WorldError> {
    *total_work = total_work
        .checked_add(work)
        .ok_or(WorldError::BudgetExhausted)?;
    *diagnostics.operator_work.entry(op_id).or_default() += work;
    if limits.max_work.is_some_and(|max| *total_work > max) {
        return Err(WorldError::BudgetExhausted);
    }
    Ok(())
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

/// Evaluate a base row into the typed intermediate tuple the Scan operator emits.
fn scan_intermediate(
    relation: &str,
    key: Option<&WorldKey>,
    schema: &ast::Ty,
    tuple: &WorldTuple,
    schemas: &BTreeMap<String, brix_lower::l3_v2::L3Schema>,
) -> Result<IntermediateTuple, WorldError> {
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
            WorldError::NetworkError(format!("missing field {relation}.{}", field.name))
        })?;
        intermediate.insert(
            field.name.clone(),
            Value::from_typed_bytes_with_schemas(bytes, &field.ty, schemas).map_err(
                |e| match e {
                    WorldError::NetworkError(msg) => {
                        if let Some(k) = key {
                            WorldError::NetworkError(format!(
                                "{relation}[{k}].{}: {msg}",
                                field.name
                            ))
                        } else {
                            WorldError::NetworkError(format!("{relation}.{}: {msg}", field.name))
                        }
                    }
                    other => other,
                },
            )?,
        );
    }
    Ok(intermediate)
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
