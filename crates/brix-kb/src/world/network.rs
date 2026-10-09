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

use super::compact_deriv::{leaf_counts, CompactDeriv, DRoot, Derivation};
use super::persistent::{PMap, PSet};
use super::state_codec::{
    CandKey, CandidateHead, FrontierBucket, FrontierKey, JoinEntry, StateCodec, StateCodecError,
    SubRoot, SupportItem, SupportRef, ValueList,
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
        OperatorWorkItem::Delta(_) => 1,
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

/// One side of a join index: join key -> inner map of the tuples (with compact
/// derivations) currently supporting that key.
///
/// `outer` is the persisted index (key -> inner [`SubRoot`]); `hot` holds the inner maps
/// modified (and so not yet reachable from the store) since the last dehydration.
#[derive(Clone, Debug, Default)]
pub struct JoinSide {
    pub outer: SMap<ValueList, SubRoot>,
    pub hot: PMap<ValueList, SMap<Digest, JoinEntry>>,
    /// Shadow of the pre-store implementation, kept until M4 (see `join_oracle`).
    #[cfg(any(test, feature = "r0-oracle"))]
    pub(crate) legacy: PMap<Vec<Value>, PMap<DerivationId, IntermediateTuple>>,
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
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy: self.legacy.clone(),
        })
    }
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
/// the store) since the last dehydration. `L` is the key type of the legacy oracle shadow;
/// `I` the stored support item (the compact derivation for Distinct, its digest for
/// GroupedCount, which never needs to expand its supports).
#[derive(Clone, Debug)]
pub struct SupportIndex<K, L, I>
where
    K: StateCodec + Ord + Clone + 'static + Send + Sync,
    L: Ord + Clone,
    I: SupportItem,
{
    pub outer: SMap<K, SupportRef<I>>,
    pub hot: PMap<K, SMap<Digest, I::Val>>,
    /// Shadow of the pre-store implementation, kept until M4 (see `join_oracle`).
    #[cfg(any(test, feature = "r0-oracle"))]
    pub(crate) legacy: PMap<L, PSet<DerivationId>>,
    #[cfg(not(any(test, feature = "r0-oracle")))]
    _legacy: std::marker::PhantomData<L>,
}

impl<K, L, I> Default for SupportIndex<K, L, I>
where
    K: StateCodec + Ord + Clone + 'static + Send + Sync,
    L: Ord + Clone,
    I: SupportItem,
{
    fn default() -> Self {
        Self {
            outer: SMap::new(),
            hot: PMap::new(),
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy: PMap::new(),
            #[cfg(not(any(test, feature = "r0-oracle")))]
            _legacy: std::marker::PhantomData,
        }
    }
}

impl<K, L, I> SupportIndex<K, L, I>
where
    K: StateCodec + Ord + Clone + 'static + Send + Sync,
    L: Ord + Clone,
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
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy: self.legacy.clone(),
            #[cfg(not(any(test, feature = "r0-oracle")))]
            _legacy: std::marker::PhantomData,
        })
    }
}

/// Distinct support index, keyed by the typed tuple (`Int(1)` and `Str("1")` stay distinct).
pub type DistinctSupports = SupportIndex<IntermediateTuple, IntermediateTuple, CompactDeriv>;
/// Grouped-count index, keyed by the group key values.
pub type GroupSupports = SupportIndex<ValueList, Vec<Value>, Digest>;
/// Contents of one derived relation: each distinct typed tuple with the digests of the
/// derivations currently supporting it. A tuple is in the relation iff it has at least one
/// support, so collapsing derivations (several rows projecting to one tuple) is
/// order-independent, and `Int(1)` / `Str("1")` stay distinct (the pre-M3 set was keyed by
/// the Display bytes of a `TupleRecord`, which collapsed them).
pub type DerivedSupports = SupportIndex<IntermediateTuple, (), Digest>;
/// Supports of the live candidates, keyed by decide, entity and candidate name.
pub type CandidateSupports = SupportIndex<CandKey, (), CompactDeriv>;

/// The candidate frontier, store-backed: per `(decide, entity)` a [`FrontierBucket`] of the
/// live candidates' heads, plus a [`CandidateSupports`] index holding each candidate's
/// support set (canonical `Inline | Sub`, like every other support index). A candidate is
/// live iff it has a support; an entity without candidates has no bucket.
#[derive(Clone, Debug, Default)]
pub struct CandidateFrontier {
    pub buckets: SMap<FrontierKey, FrontierBucket>,
    pub supports: CandidateSupports,
    /// Shadow of the pre-store implementation, kept until M4 (see `join_oracle`).
    #[cfg(any(test, feature = "r0-oracle"))]
    pub(crate) legacy: PMap<String, PMap<String, PMap<String, CandidateEntry>>>,
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
        for (name, head) in &bucket.0 {
            head.verify(&key.decide, &key.entity, name)
                .map_err(|e| WorldError::from(StateError::from(e)))?;
        }
        Ok(Some(bucket))
    }

    fn put_bucket(
        &mut self,
        key: FrontierKey,
        bucket: FrontierBucket,
        store: &StateStore,
    ) -> Result<(), WorldError> {
        self.buckets = if bucket.0.is_empty() {
            self.buckets.remove(&key, store)?
        } else {
            self.buckets.upsert(key, bucket, store)?
        };
        Ok(())
    }

    /// Every entity bucket, in outer trie order.
    pub(crate) fn all_buckets(
        &self,
        store: &StateStore,
    ) -> Result<Vec<(FrontierKey, FrontierBucket)>, WorldError> {
        let mut out = Vec::with_capacity(self.buckets.len());
        let mut cursor = None;
        loop {
            let (rows, next) = self.buckets.page(cursor.as_ref(), 256, store)?;
            for (k, b) in rows {
                for (name, head) in &b.0 {
                    head.verify(&k.decide, &k.entity, name)
                        .map_err(|e| WorldError::from(StateError::from(e)))?;
                }
                out.push((k, b));
            }
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(out)
    }

    fn persist(&self, store: &StateStore) -> usize {
        self.buckets.persist(store) + self.supports.persist(store)
    }

    fn dehydrated(&self, store: &StateStore) -> Result<Self, WorldError> {
        Ok(Self {
            buckets: self.buckets.dehydrate(store)?,
            supports: self.supports.dehydrated(store)?,
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy: self.legacy.clone(),
        })
    }
}

/// The settled decision of one entity: the live candidate with the least calendar key.
fn settle(entity: &str, bucket: &FrontierBucket) -> Option<SettledDecision> {
    let mut frontier: Frontier<(&String, &CandidateHead)> = Frontier::new();
    for (name, head) in &bucket.0 {
        let _ = frontier.insert(head.calendar_key, (name, head));
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
    Distinct {
        input: OperatorId,
        supports: DistinctSupports,
    },
    GroupedCount {
        input: OperatorId,
        group_keys: Vec<ast::Expr>,
        compiled_group_keys: Vec<CompiledWorldExpr>,
        projections: Vec<(String, GroupProjection)>,
        groups: GroupSupports,
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
    pub derived_relations: PMap<String, DerivedSupports>,
    pub decides: PMap<String, DecideBlock>,
    decision_subscribers: PMap<String, Vec<String>>,
    /// Content-addressed trie of settled decisions, keyed by [`decision_key`]
    /// (ADR-0046 P6 G2). Store-backed: only fallible, store-aware operations are
    /// reachable, so an unhydrated node can never read as an empty subtree. Its node
    /// encoding is identical to the pre-M3 `TrieMap<WorldKey, WorldTuple>`, so decision
    /// roots recorded in revisions are unchanged.
    pub(crate) decision_tree: SMap<WorldKey, WorldTuple>,
    pub candidate_frontier: CandidateFrontier,
    /// Shadow of the pre-M3 derived relations (a set of `TupleRecord` without support
    /// counts), kept until M4 (see `join_oracle`).
    #[cfg(any(test, feature = "r0-oracle"))]
    pub(crate) legacy_derived: PMap<String, PSet<TupleRecord>>,
    /// Shadow of the settled decisions (plain map; the root is rebuilt on demand).
    #[cfg(any(test, feature = "r0-oracle"))]
    pub(crate) legacy_decisions: PMap<WorldKey, WorldTuple>,
    pub functions: Arc<BTreeMap<String, ast::Callable>>,
    pub program_env: Option<Arc<CompiledProgramEnv>>,
    pub current_revision: u64,
    pub scheduling_quantum: usize,
    /// Node store backing persisted operator state (join and support indexes).
    state: StateStore,
    /// Leaves each operator's derivations carry (index = operator id).
    leaf_counts: Arc<Vec<usize>>,
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
                            groups: GroupSupports::default(),
                        },
                    );
                }
            }
        }

        let mut derived_relations = PMap::new();
        for (name, &op_id) in &dag.relation_outputs {
            if !matches!(dag.nodes[op_id.0], OperatorNode::Scan { .. }) {
                derived_relations.insert(name.clone(), DerivedSupports::default());
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
            decision_tree: SMap::new(),
            candidate_frontier: CandidateFrontier::default(),
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy_derived: PMap::new(),
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy_decisions: PMap::new(),
            functions: Arc::new(BTreeMap::new()),
            program_env: None,
            current_revision: 0,
            scheduling_quantum: DEFAULT_SCHEDULING_QUANTUM,
            state: StateStore::memory(DEFAULT_NODE_CACHE_BYTES),
            leaf_counts: Arc::new(counts),
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
        for supports in self.derived_relations.values() {
            n += supports.persist(&self.state);
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
                OperatorState::Distinct { supports, .. } => n += supports.persist(&self.state),
                OperatorState::GroupedCount { groups, .. } => n += groups.persist(&self.state),
                _ => {}
            }
        }
        Ok(n)
    }

    /// A copy holding only store references for persisted state. Requires
    /// [`Self::persist_state`] (and a flush) first; fails if a root is not persisted.
    pub fn dehydrated(&self) -> Result<Self, WorldError> {
        let mut out = self.clone();
        for (name, supports) in self.derived_relations.iter() {
            out.derived_relations
                .insert(name.clone(), supports.dehydrated(&self.state)?);
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
                    groups,
                } => {
                    out.operator_states.insert(
                        *idx,
                        OperatorState::GroupedCount {
                            input: *input,
                            group_keys: group_keys.clone(),
                            compiled_group_keys: compiled_group_keys.clone(),
                            projections: projections.clone(),
                            groups: groups.dehydrated(&self.state)?,
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

    /// Per derived relation, the outer support index root.
    pub fn derived_roots(&self) -> BTreeMap<String, SubRoot> {
        self.derived_relations
            .iter()
            .map(|(name, s)| (name.clone(), s.outer.sub_root()))
            .collect()
    }

    /// The candidate frontier roots: `(buckets, supports)`.
    pub fn frontier_roots(&self) -> (SubRoot, SubRoot) {
        (
            self.candidate_frontier.buckets.sub_root(),
            self.candidate_frontier.supports.outer.sub_root(),
        )
    }

    /// Root and size of the settled-decision trie.
    pub fn decision_sub_root(&self) -> SubRoot {
        self.decision_tree.sub_root()
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
            .map(|(k, _)| (k.clone(), DerivedSupports::default()))
            .collect();
        fresh.decision_tree = SMap::new();
        fresh.candidate_frontier = CandidateFrontier::default();
        #[cfg(any(test, feature = "r0-oracle"))]
        {
            fresh.legacy_derived = PMap::new();
            fresh.legacy_decisions = PMap::new();
        }
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
                    ..
                } => OperatorState::GroupedCount {
                    input: *input,
                    group_keys: group_keys.clone(),
                    compiled_group_keys: compiled_group_keys.clone(),
                    projections: projections.clone(),
                    groups: GroupSupports::default(),
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
        #[cfg(any(test, feature = "r0-oracle"))]
        let (mut legacy_candidates_inserted, mut legacy_candidates_retracted) = (0usize, 0usize);

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
                            let derivation = Derivation::base(relation, key);

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

                                #[cfg(any(test, feature = "r0-oracle"))]
                                let legacy_matches = super::join_oracle::shadow_step(
                                    own,
                                    other,
                                    &join_key,
                                    deriv.legacy(),
                                    tuple,
                                    is_ins,
                                );

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

                                #[cfg(any(test, feature = "r0-oracle"))]
                                super::join_oracle::assert_same_matches(
                                    &legacy_matches,
                                    &matches,
                                    store,
                                    &self.dag,
                                )?;

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

                            #[cfg(any(test, feature = "r0-oracle"))]
                            let legacy_counts = super::join_oracle::shadow_support_step(
                                &mut supports.legacy,
                                &tuple,
                                deriv.legacy(),
                                is_ins,
                            );

                            let (old_count, new_count) = if is_ins {
                                supports.add(tuple.clone(), deriv.c.clone(), store)?
                            } else {
                                supports.remove(&tuple, &deriv.c.digest(), store)?
                            };

                            #[cfg(any(test, feature = "r0-oracle"))]
                            assert_eq!(
                                (old_count, new_count),
                                legacy_counts,
                                "store-backed distinct support counts diverged from the legacy oracle"
                            );

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

                            let store = &self.state;
                            #[cfg(any(test, feature = "r0-oracle"))]
                            let legacy_counts = super::join_oracle::shadow_support_step(
                                &mut groups.legacy,
                                &key_vals,
                                deriv.legacy(),
                                is_ins,
                            );
                            let gk = ValueList(key_vals.clone());
                            let (old_count, new_count) = if is_ins {
                                groups.add(gk, deriv.c.digest(), store)?
                            } else {
                                groups.remove(&gk, &deriv.c.digest(), store)?
                            };
                            #[cfg(any(test, feature = "r0-oracle"))]
                            assert_eq!(
                                (old_count, new_count),
                                legacy_counts,
                                "store-backed group counts diverged from the legacy oracle"
                            );

                            if new_count != old_count {
                                let out_deriv = Derivation::group(op_id, key_vals.clone());
                                if is_ins && old_count == 0 {
                                    let out_tuple = build_grouped_tuple(projections, &key_vals, 1);
                                    out_deltas.push(TupleDelta::Insert {
                                        tuple: out_tuple,
                                        derivation: out_deriv,
                                    });
                                } else if !is_ins && new_count == 0 {
                                    let old_tuple = build_grouped_tuple(projections, &key_vals, 1);
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
                        _ => unreachable!(),
                    },
                }
            }

            // Propagate out_deltas. Each consumer of the batch gets a copy except the last,
            // which takes ownership (avoids one full clone of every batch).
            if !out_deltas.is_empty() {
                #[cfg(any(test, feature = "r0-oracle"))]
                for od in &out_deltas {
                    od.derivation()
                        .check(&self.dag)
                        .map_err(|e| WorldError::from(StateError::from(e)))?;
                }
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
                            let supports =
                                self.derived_relations.entry(rel_name.clone()).or_default();
                            for od in &out_deltas {
                                let is_ins = od.is_insert();
                                let id = od.derivation().digest();

                                #[cfg(any(test, feature = "r0-oracle"))]
                                let legacy_changed = {
                                    let set =
                                        self.legacy_derived.entry(rel_name.clone()).or_default();
                                    let rec = od.tuple().to_tuple_record();
                                    if is_ins {
                                        set.insert(rec)
                                    } else {
                                        set.remove(&rec)
                                    }
                                };

                                let (old, new) = if is_ins {
                                    supports.add(od.tuple().clone(), id, store)?
                                } else {
                                    supports.remove(od.tuple(), &id, store)?
                                };
                                let changed = if is_ins {
                                    old == 0 && new > 0
                                } else {
                                    old > 0 && new == 0
                                };

                                #[cfg(any(test, feature = "r0-oracle"))]
                                assert_eq!(
                                    changed, legacy_changed,
                                    "store-backed derived relation diverged from the legacy oracle"
                                );

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
                            enqueue(
                                *consumer,
                                OperatorWorkItem::Delta(od),
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

                    #[cfg(any(test, feature = "r0-oracle"))]
                    let did = {
                        let did = deriv
                            .expand(&self.dag)
                            .map_err(|e| WorldError::from(StateError::from(e)))?;
                        assert_eq!(&did, deriv.legacy(), "frontier derivation diverged");
                        did
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

                                #[cfg(any(test, feature = "r0-oracle"))]
                                {
                                    let entity_map = self
                                        .candidate_frontier
                                        .legacy
                                        .entry(decide.name.clone())
                                        .or_default()
                                        .entry(entity_id.clone())
                                        .or_default();
                                    let entry = entity_map
                                        .entry(propose.name.clone())
                                        .or_insert_with(|| {
                                            legacy_candidates_inserted += 1;
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
                                    entry.supports.insert(did.clone());
                                }

                                let store = &self.state;
                                let fk = FrontierKey::new(&decide.name, &entity_id);
                                let mut bucket = self
                                    .candidate_frontier
                                    .bucket(&fk, store)?
                                    .unwrap_or_default();
                                if let Some(existing) = bucket.0.get(&propose.name) {
                                    if existing.value != val {
                                        return Err(WorldError::NetworkError(format!(
                                            "conflicting support values for decide '{}' entity \
                                             '{entity_id}' candidate '{}': {:?} vs {val:?}",
                                            decide.name, propose.name, existing.value
                                        )));
                                    }
                                } else {
                                    report.candidates_inserted += 1;
                                    bucket.0.insert(
                                        propose.name.clone(),
                                        CandidateHead {
                                            priority: propose.priority,
                                            phase: 0,
                                            value: val,
                                            calendar_key: cal_key,
                                        },
                                    );
                                    self.candidate_frontier.put_bucket(fk, bucket, store)?;
                                }
                                self.candidate_frontier.supports.add(
                                    CandKey::new(&decide.name, &entity_id, &propose.name),
                                    deriv.c.clone(),
                                    store,
                                )?;
                            }
                        } else {
                            #[cfg(any(test, feature = "r0-oracle"))]
                            if let Some(decide_map) =
                                self.candidate_frontier.legacy.get_mut(&decide.name)
                            {
                                if let Some(entity_map) = decide_map.get_mut(&entity_id) {
                                    if let Some(entry) = entity_map.get_mut(&propose.name) {
                                        entry.supports.remove(&did);
                                        if entry.supports.is_empty() {
                                            entity_map.remove(&propose.name);
                                            legacy_candidates_retracted += 1;
                                        }
                                    }
                                    if entity_map.is_empty() {
                                        decide_map.remove(&entity_id);
                                    }
                                }
                            }

                            let store = &self.state;
                            let fk = FrontierKey::new(&decide.name, &entity_id);
                            if let Some(mut bucket) = self.candidate_frontier.bucket(&fk, store)? {
                                if bucket.0.contains_key(&propose.name) {
                                    let (_, remaining) = self.candidate_frontier.supports.remove(
                                        &CandKey::new(&decide.name, &entity_id, &propose.name),
                                        &deriv.c.digest(),
                                        store,
                                    )?;
                                    if remaining == 0 {
                                        bucket.0.remove(&propose.name);
                                        report.candidates_retracted += 1;
                                        self.candidate_frontier.put_bucket(fk, bucket, store)?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        for (decide, entity) in touched_entities {
            let key = decision_key(&decide, &entity);
            let settlement = self.get_settlement(&decide, &entity)?;

            #[cfg(any(test, feature = "r0-oracle"))]
            {
                let legacy = super::join_oracle::legacy_settlement(
                    &self.candidate_frontier.legacy,
                    &decide,
                    &entity,
                );
                assert_eq!(
                    settlement, legacy,
                    "store-backed settlement diverged from the legacy oracle"
                );
                match &settlement {
                    Some(d) => {
                        self.legacy_decisions.insert(key.clone(), decision_tuple(d));
                    }
                    None => {
                        self.legacy_decisions.remove(&key);
                    }
                }
            }

            if let Some(decision) = settlement {
                let tuple = decision_tuple(&decision);
                if self.decision_tree.get(&key, &self.state)?.as_ref() == Some(&tuple) {
                    continue;
                }
                self.decision_tree = self.decision_tree.upsert(key, tuple, &self.state)?;
                report
                    .settlements
                    .entry(decide)
                    .or_default()
                    .insert(entity, decision);
            } else if self.decision_tree.get(&key, &self.state)?.is_some() {
                self.decision_tree = self.decision_tree.remove(&key, &self.state)?;
                report
                    .removed_settlements
                    .entry(decide)
                    .or_default()
                    .insert(entity);
            }
        }

        #[cfg(any(test, feature = "r0-oracle"))]
        {
            assert_eq!(
                (report.candidates_inserted, report.candidates_retracted),
                (legacy_candidates_inserted, legacy_candidates_retracted),
                "store-backed candidate counts diverged from the legacy oracle"
            );
        }
        report.diagnostics = diagnostics;
        self.current_revision += 1;
        Ok(report)
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
        let Some(supports) = self.derived_relations.get(relation) else {
            return Ok(None);
        };
        let records: BTreeSet<TupleRecord> = supports
            .keys(&self.state)?
            .iter()
            .map(IntermediateTuple::to_tuple_record)
            .collect();
        Ok(Some(records.into_iter().collect()))
    }

    /// Candidates (with their expanded supports) for a specific decide block and entity.
    pub fn get_candidates(
        &self,
        decide: &str,
        entity_id: &str,
    ) -> Result<Option<BTreeMap<String, CandidateEntry>>, WorldError> {
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
        for (name, head) in &bucket.0 {
            let items = self
                .candidate_frontier
                .supports
                .read(&CandKey::new(decide, entity_id, name), &self.state)?
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
                    priority: head.priority,
                    phase: head.phase,
                    value: head.value.clone(),
                    calendar_key: head.calendar_key,
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
        Ok(self
            .candidate_frontier
            .bucket(&FrontierKey::new(decide, entity_id), &self.state)?
            .and_then(|bucket| settle(entity_id, &bucket)))
    }

    /// Settle decisions across all entities in all decide blocks.
    pub fn all_settlements(
        &self,
    ) -> Result<BTreeMap<String, BTreeMap<String, SettledDecision>>, WorldError> {
        let mut out: BTreeMap<String, BTreeMap<String, SettledDecision>> = BTreeMap::new();
        for (key, bucket) in self.candidate_frontier.all_buckets(&self.state)? {
            if let Some(decision) = settle(&key.entity, &bucket) {
                out.entry(key.decide)
                    .or_default()
                    .insert(key.entity, decision);
            }
        }
        Ok(out)
    }

    /// Capture current observable network state.
    pub fn current_state(&self) -> Result<WorldNetworkState, WorldError> {
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
        for (name, supports) in self.derived_relations.iter() {
            derived_relations.insert(
                name.clone(),
                supports
                    .keys(&self.state)?
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
                out.insert((relation.clone(), key.clone()));
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
                    // Keyed lookup: the typed key decodes back to the Distinct tuple.
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

        for (cand_name, head) in &bucket.0 {
            let winning = winning_name == Some(cand_name);
            let supports = self.candidate_frontier.supports.read(
                &CandKey::new(decide_name, entity_id, cand_name),
                &self.state,
            )?;
            let items = supports.members(&self.state)?;
            for deriv in self.expand_supports(&items)? {
                self.collect_base_facts_for_derivation(&deriv, &mut contributing_base)?;
            }
            candidates.push(CandidateExplanation {
                name: cand_name.clone(),
                priority: head.priority,
                phase: head.phase,
                value: head.value.clone(),
                supports_count: supports.len(),
                winning,
            });
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
        }))
    }
}

fn corrupt_derivation() -> WorldError {
    StateError::Codec(StateCodecError::NonCanonical(
        "stored derivation does not match its operator".to_string(),
    ))
    .into()
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
