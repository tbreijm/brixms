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

use super::deriv_intern::{deriv_ref, resolve};
use super::error::WorldError;
use super::network::{
    DerivationId, IntermediateTuple, JoinSide, OperatorState, Value, WorldNetwork,
};
use super::persistent::PMap;
use super::state_codec::{JoinEntry, SubRoot, ValueList};
use super::state_store::{SMap, StateStore, DEFAULT_NODE_CACHE_BYTES};

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

/// Assert the store-backed `matches` equal the legacy matches as sets of
/// `(derivation, tuple)`.
pub(crate) fn assert_same_matches(
    legacy: &PMap<DerivationId, IntermediateTuple>,
    matches: &SMap<Digest, JoinEntry>,
    pending: &PMap<Digest, super::state_codec::DNode>,
    store: &StateStore,
    max_depth: usize,
) -> Result<(), WorldError> {
    let mut got: BTreeSet<(DerivationId, IntermediateTuple)> = BTreeSet::new();
    let mut cursor = None;
    loop {
        let (rows, next) = matches.page(cursor.as_ref(), 256, store)?;
        for (k, e) in rows {
            let d = resolve(&e.deriv, pending, store, max_depth)?;
            assert_eq!(k, e.deriv, "join entry key must be its derivation ref");
            assert_eq!(deriv_ref(&d), e.deriv, "interned ref must round-trip");
            got.insert(((*d).clone(), e.tuple));
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
    let mut outer: SMap<ValueList, SubRoot> = SMap::new();
    for (k, inner_legacy) in side.legacy.iter() {
        let mut inner: SMap<Digest, JoinEntry> = SMap::new();
        for (d, t) in inner_legacy.iter() {
            let r = deriv_ref(d);
            inner = inner.upsert(
                r,
                JoinEntry {
                    tuple: t.clone(),
                    deriv: r,
                },
                store,
            )?;
        }
        outer = outer.upsert(ValueList(k.clone()), inner.sub_root(), store)?;
    }
    Ok(outer.sub_root())
}

impl WorldNetwork {
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
