//! Shadow oracle for the store-backed join index (R0 Phase 2, M1).
//!
//! The pre-store implementation (`PMap`-based symmetric join indexes keyed by
//! `DerivationId`) is kept verbatim as a shadow inside every [`JoinSide`] and updated in the
//! same arm as the store-backed one. For every delta the set of `(derivation, tuple)`
//! matches must equal the set the store pages out; the index roots can be recomputed from
//! the legacy content ([`WorldNetwork::legacy_roots`]) and compared with
//! [`WorldNetwork::join_index_roots`]. Matches are compared as sets because the order
//! changed from `DerivationId` order to trie-hash order.
//!
//! Compiled only for tests or with the `r0-oracle` feature. Remove at M4.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::Digest;
use brix_lower::relation_dag::OperatorId;

use super::codec::TupleRecord;
use super::compact_deriv::CompactDeriv;
use super::error::WorldError;
use super::network::{
    CandidateEntry, DerivationId, IntermediateTuple, JoinSide, OperatorState, SettledDecision,
    Value, WorldNetwork,
};
use super::persistent::{PMap, PSet};
use super::state_codec::{
    CandKey, CandidateHead, FrontierBucket, FrontierKey, JoinEntry, SubRoot, SupportItem,
    SupportRef, ValueList,
};
use super::state_store::{SMap, StateStore, DEFAULT_NODE_CACHE_BYTES};
use super::types::{WorldKey, WorldTuple};
use brix_lower::relation_dag::RelationDag;
use soc_core::calendar::Frontier;

/// Apply one delta to the legacy shadow and return the opposite side's matches
/// (the pre-change `PMap` logic, unchanged).
pub(crate) fn shadow_step(
    own: &mut JoinSide,
    other: &JoinSide,
    join_key: &Vec<Value>,
    deriv: &DerivationId,
    tuple: &IntermediateTuple,
    is_ins: bool,
) -> PMap<DerivationId, IntermediateTuple> {
    if is_ins {
        let m = other.legacy.get(join_key).cloned().unwrap_or_default();
        own.legacy
            .entry(join_key.clone())
            .or_default()
            .insert(deriv.clone(), tuple.clone());
        m
    } else {
        if let Some(mine) = own.legacy.get_mut(join_key) {
            mine.remove(deriv);
            if mine.is_empty() {
                own.legacy.remove(join_key);
            }
        }
        other.legacy.get(join_key).cloned().unwrap_or_default()
    }
}

/// Apply one delta to a legacy support-set shadow (Distinct, GroupedCount) with the
/// pre-store `PMap<_, PSet<DerivationId>>` logic verbatim; returns the support count
/// before and after.
pub(crate) fn shadow_support_step<L: Ord + Clone>(
    legacy: &mut PMap<L, PSet<DerivationId>>,
    key: &L,
    deriv: &DerivationId,
    is_ins: bool,
) -> (usize, usize) {
    if is_ins {
        let set = legacy.entry(key.clone()).or_default();
        let old = set.len();
        set.insert(deriv.clone());
        (old, set.len())
    } else if let Some(set) = legacy.get_mut(key) {
        let old = set.len();
        set.remove(deriv);
        let new = set.len();
        if new == 0 {
            legacy.remove(key);
        }
        (old, new)
    } else {
        (0, 0)
    }
}

/// Assert the store-backed `matches` equal the legacy matches as sets of
/// `(derivation, tuple)`.
pub(crate) fn assert_same_matches(
    legacy: &PMap<DerivationId, IntermediateTuple>,
    matches: &SMap<Digest, JoinEntry>,
    store: &StateStore,
    dag: &RelationDag,
) -> Result<(), WorldError> {
    let mut got: BTreeSet<(DerivationId, IntermediateTuple)> = BTreeSet::new();
    let mut cursor = None;
    loop {
        let (rows, next) = matches.page(cursor.as_ref(), 256, store)?;
        for (k, e) in rows {
            let d = e
                .deriv
                .expand(dag)
                .map_err(super::state_store::StateError::from)?;
            assert_eq!(
                k,
                e.deriv.digest(),
                "join entry key must be its derivation digest"
            );
            assert_eq!(
                CompactDeriv::from_derivation(&d),
                e.deriv,
                "compact derivation must round-trip"
            );
            got.insert((d, e.tuple));
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    let want: BTreeSet<(DerivationId, IntermediateTuple)> =
        legacy.iter().map(|(d, t)| (d.clone(), t.clone())).collect();
    assert_eq!(
        got, want,
        "store-backed join matches diverged from the legacy oracle"
    );
    assert_eq!(matches.len(), want.len(), "match count diverged");
    Ok(())
}

fn legacy_side_root(side: &JoinSide, store: &StateStore) -> Result<SubRoot, WorldError> {
    let compact = CompactDeriv::from_derivation;
    let mut outer: SMap<ValueList, SubRoot> = SMap::new();
    for (k, inner_legacy) in side.legacy.iter() {
        let mut inner: SMap<Digest, JoinEntry> = SMap::new();
        for (d, t) in inner_legacy.iter() {
            let c = compact(d);
            inner = inner.upsert(
                c.digest(),
                JoinEntry {
                    tuple: t.clone(),
                    deriv: c,
                },
                store,
            )?;
        }
        outer = outer.upsert(ValueList(k.clone()), inner.sub_root(), store)?;
    }
    Ok(outer.sub_root())
}

fn legacy_support_root<L: Ord + Clone, K, I: SupportItem>(
    legacy: &PMap<L, PSet<DerivationId>>,
    to_key: impl Fn(&L) -> K,
    to_item: impl Fn(&DerivationId) -> I,
    store: &StateStore,
) -> Result<SubRoot, WorldError>
where
    K: super::state_codec::StateCodec + Ord + Clone + 'static + Send + Sync,
{
    let mut outer: SMap<K, SupportRef<I>> = SMap::new();
    for (k, set) in legacy.iter() {
        let sref = if set.len() == 1 {
            SupportRef::Inline(to_item(set.iter().next().expect("len is 1")))
        } else {
            let mut inner: SMap<Digest, I::Val> = SMap::new();
            for d in set.iter() {
                let item = to_item(d);
                inner = inner.upsert(item.id(), item.val(), store)?;
            }
            SupportRef::Sub(inner.sub_root())
        };
        outer = outer.upsert(to_key(k), sref, store)?;
    }
    Ok(outer.sub_root())
}

/// Settlement of one entity computed with the pre-store logic over the legacy frontier.
pub(crate) fn legacy_settlement(
    legacy: &PMap<String, PMap<String, PMap<String, CandidateEntry>>>,
    decide: &str,
    entity: &str,
) -> Option<SettledDecision> {
    let entity_candidates = legacy.get(decide)?.get(entity)?;
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

impl WorldNetwork {
    /// Derived relations as the pre-M3 logic (a set of `TupleRecord`, no support counts)
    /// computed them.
    pub fn legacy_derived_state(&self) -> BTreeMap<String, BTreeSet<TupleRecord>> {
        self.derived_relations
            .iter()
            .map(|(name, _)| {
                (
                    name.clone(),
                    self.legacy_derived
                        .get(name)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    /// Candidate frontier as the pre-M3 nested maps held it.
    pub fn legacy_frontier_state(
        &self,
    ) -> BTreeMap<String, BTreeMap<String, BTreeMap<String, CandidateEntry>>> {
        self.candidate_frontier
            .legacy
            .iter()
            .map(|(d, ents)| {
                (
                    d.clone(),
                    ents.iter()
                        .map(|(e, cands)| {
                            (
                                e.clone(),
                                cands.iter().map(|(c, v)| (c.clone(), v.clone())).collect(),
                            )
                        })
                        .collect(),
                )
            })
            .collect()
    }

    /// All settlements recomputed from the legacy frontier.
    pub fn legacy_settlements(&self) -> BTreeMap<String, BTreeMap<String, SettledDecision>> {
        let mut out: BTreeMap<String, BTreeMap<String, SettledDecision>> = BTreeMap::new();
        for (d, ents) in self.candidate_frontier.legacy.iter() {
            for (e, _) in ents.iter() {
                if let Some(s) = legacy_settlement(&self.candidate_frontier.legacy, d, e) {
                    out.entry(d.clone()).or_default().insert(e.clone(), s);
                }
            }
        }
        out
    }

    /// Frontier `(buckets, supports)` roots rebuilt from the legacy frontier content.
    pub fn legacy_frontier_roots(&self) -> Result<(SubRoot, SubRoot), WorldError> {
        let scratch = StateStore::memory(DEFAULT_NODE_CACHE_BYTES);
        let mut buckets: SMap<FrontierKey, FrontierBucket> = SMap::new();
        let mut supports: PMap<CandKey, PSet<DerivationId>> = PMap::new();
        for (d, ents) in self.candidate_frontier.legacy.iter() {
            for (e, cands) in ents.iter() {
                let mut bucket = FrontierBucket::default();
                for (c, entry) in cands.iter() {
                    bucket.0.insert(c.clone(), CandidateHead::from_entry(entry));
                    supports.insert(CandKey::new(d, e, c), entry.supports.clone());
                }
                buckets = buckets.upsert(FrontierKey::new(d, e), bucket, &scratch)?;
            }
        }
        let sup_root = legacy_support_root(
            &supports,
            |k: &CandKey| k.clone(),
            CompactDeriv::from_derivation,
            &scratch,
        )?;
        Ok((buckets.sub_root(), sup_root))
    }

    /// Root and size of the settled-decision trie rebuilt from the legacy decisions with the
    /// pre-M3 plain `TrieMap`.
    pub fn legacy_decision_sub_root(&self) -> SubRoot {
        let mut tree: soc_core::store::TrieMap<WorldKey, WorldTuple> =
            soc_core::store::TrieMap::new();
        for (k, v) in self.legacy_decisions.iter() {
            tree = tree.insert(k.clone(), v.clone());
        }
        SubRoot {
            root: tree.root_digest(),
            len: tree.len() as u64,
        }
    }

    /// Distinct support index roots rebuilt from the legacy shadow content.
    pub fn legacy_distinct_roots(&self) -> Result<BTreeMap<OperatorId, SubRoot>, WorldError> {
        let scratch = StateStore::memory(DEFAULT_NODE_CACHE_BYTES);
        let mut out = BTreeMap::new();
        for (idx, state) in self.operator_states.iter() {
            if let OperatorState::Distinct { supports, .. } = state {
                out.insert(
                    OperatorId(*idx),
                    legacy_support_root(
                        &supports.legacy,
                        |k: &IntermediateTuple| k.clone(),
                        CompactDeriv::from_derivation,
                        &scratch,
                    )?,
                );
            }
        }
        Ok(out)
    }

    /// Grouped-count index roots rebuilt from the legacy shadow content.
    pub fn legacy_group_roots(&self) -> Result<BTreeMap<OperatorId, SubRoot>, WorldError> {
        let scratch = StateStore::memory(DEFAULT_NODE_CACHE_BYTES);
        let mut out = BTreeMap::new();
        for (idx, state) in self.operator_states.iter() {
            if let OperatorState::GroupedCount { groups, .. } = state {
                out.insert(
                    OperatorId(*idx),
                    legacy_support_root(
                        &groups.legacy,
                        |k: &Vec<Value>| ValueList(k.clone()),
                        |d: &DerivationId| CompactDeriv::from_derivation(d).digest(),
                        &scratch,
                    )?,
                );
            }
        }
        Ok(out)
    }

    /// Join index roots rebuilt from the legacy shadow content.
    pub fn legacy_roots(&self) -> Result<BTreeMap<OperatorId, (SubRoot, SubRoot)>, WorldError> {
        let scratch = StateStore::memory(DEFAULT_NODE_CACHE_BYTES);
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
                    (
                        legacy_side_root(left_side, &scratch)?,
                        legacy_side_root(right_side, &scratch)?,
                    ),
                );
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::batch::WorldBatchOp;
    use crate::world::codec::TupleRecord;
    use crate::world::types::{WorldKey, WorldTuple};
    use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};

    fn network() -> WorldNetwork {
        let src = r#"
rel input order: { id: Str, sku: Str, qty: Int } key id
rel input stock: { sku: Str, available: Int } key sku
rel derived ok =
    select { order_id: o.id, sku: o.sku }
    from o in order, b in stock
    where o.sku == b.sku and b.available >= o.qty
"#;
        let mut sources = BTreeMap::new();
        sources.insert("root".to_string(), src.to_string());
        let loader = |n: &str| sources.get(n).cloned();
        let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).unwrap();
        WorldNetwork::from_program(&graph.link().unwrap()).unwrap()
    }

    fn tup(fields: &[(&str, &str)]) -> WorldTuple {
        let mut rec = TupleRecord::new();
        for (k, v) in fields {
            rec.set_str(*k, v);
        }
        rec.to_tuple()
    }

    fn up(rel: &str, key: &str, fields: &[(&str, &str)]) -> WorldBatchOp {
        WorldBatchOp::Upsert {
            relation: rel.to_string(),
            key: WorldKey::from_str(key),
            tuple: tup(fields),
        }
    }

    fn check(net: &WorldNetwork) {
        assert_eq!(net.join_index_roots(), net.legacy_roots().unwrap());
        assert_eq!(net.distinct_roots(), net.legacy_distinct_roots().unwrap());
        assert_eq!(net.group_roots(), net.legacy_group_roots().unwrap());
    }

    fn base_ref(i: u64) -> Digest {
        CompactDeriv::from_derivation(&DerivationId::Base {
            relation: "r".to_string(),
            key: WorldKey::from_u64(i),
        })
        .digest()
    }

    proptest::proptest! {
        /// A given support set has exactly one representation, whatever order of adds and
        /// removes (including transient extras and duplicates) produced it, and whether or
        /// not the index was persisted and dehydrated on the way.
        #[test]
        fn support_representation_is_canonical(
            target in proptest::collection::btree_set(0u64..8, 0..6),
            extras in proptest::collection::btree_set(8u64..14, 0..5),
            mult in 1u64..13,
            dehydrate in proptest::bool::ANY,
        ) {
            use super::super::network::SupportIndex;
            let store = StateStore::memory(0);
            let key = ValueList(vec![Value::Int(1)]);
            let mut a: SupportIndex<ValueList, Vec<Value>, Digest> = SupportIndex::default();
            for i in &target {
                a.add(key.clone(), base_ref(*i), &store).unwrap();
            }

            let mut b: SupportIndex<ValueList, Vec<Value>, Digest> = SupportIndex::default();
            let mut seq: Vec<u64> = target.iter().chain(extras.iter()).cloned().collect();
            seq.sort_by_key(|i| (i * mult) % 13);
            for (n, i) in seq.iter().enumerate() {
                b.add(key.clone(), base_ref(*i), &store).unwrap();
                b.add(key.clone(), base_ref(*i), &store).unwrap(); // duplicate add
                if dehydrate && n % 2 == 0 {
                    b.persist(&store);
                    b = b.dehydrated(&store).unwrap();
                }
            }
            for i in &extras {
                b.remove(&key, &base_ref(*i), &store).unwrap();
                b.remove(&key, &base_ref(*i), &store).unwrap(); // absent remove
            }
            proptest::prop_assert_eq!(a.outer.root(), b.outer.root());
            proptest::prop_assert_eq!(a.outer.len(), b.outer.len());
            let want = match target.len() {
                0 => None,
                1 => Some(SupportRef::Inline(base_ref(*target.iter().next().unwrap()))),
                n => Some(SupportRef::Sub(SubRoot { root: b.read(&key, &store).map(|s| match s {
                    super::super::network::Supports::Many(m) => m.root(),
                    _ => unreachable!(),
                }).unwrap(), len: n as u64 })),
            };
            proptest::prop_assert_eq!(b.outer.get(&key, &store).unwrap(), want);
        }
    }

    #[test]
    fn distinct_keys_are_typed_not_display_collapsed() {
        let store = StateStore::memory(0);
        let int_t = {
            let mut t = IntermediateTuple::new();
            t.insert("a", Value::Int(1));
            t
        };
        let str_t = {
            let mut t = IntermediateTuple::new();
            t.insert("a", Value::Str("1".into()));
            t
        };
        let m: SMap<IntermediateTuple, SubRoot> = SMap::new()
            .upsert(int_t.clone(), SubRoot::empty(), &store)
            .unwrap()
            .upsert(str_t.clone(), SubRoot::empty(), &store)
            .unwrap();
        assert_eq!(m.len(), 2);
        assert!(m.contains(&int_t, &store).unwrap());
        assert!(m.contains(&str_t, &store).unwrap());
    }

    #[test]
    fn store_roots_match_legacy_across_persist_and_dehydrate() {
        let mut net = network();
        net.set_scheduling_quantum(3);
        let mut ops = Vec::new();
        for i in 0..40 {
            ops.push(up(
                "root::order",
                &format!("o{i}"),
                &[
                    ("id", &format!("o{i}")),
                    ("sku", &format!("s{}", i % 4)),
                    ("qty", "1"),
                ],
            ));
        }
        for s in 0..4 {
            ops.push(up(
                "root::stock",
                &format!("s{s}"),
                &[("sku", &format!("s{s}")), ("available", "5")],
            ));
        }
        net.apply_ops(&ops).unwrap();
        check(&net);
        assert!(net.persist_state().unwrap() > 0);
        let mut net = net.dehydrated().unwrap();
        check(&net);
        net.apply_ops(&[
            up(
                "root::order",
                "o0",
                &[("id", "o0"), ("sku", "s1"), ("qty", "2")],
            ),
            WorldBatchOp::Remove {
                relation: "root::stock".to_string(),
                key: WorldKey::from_str("s2"),
            },
        ])
        .unwrap();
        check(&net);
        net.persist_state().unwrap();
        let net = net.dehydrated().unwrap();
        check(&net);
        net.verify_differential_correctness().unwrap();
    }
}
