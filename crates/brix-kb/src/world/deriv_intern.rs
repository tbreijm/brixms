//! Content-addressed interning of derivation ids (R0 Phase 2, M1).
//!
//! A `DerivRef` is the digest of a [`DNode`], whose children are themselves refs, so a
//! derivation tree becomes a Merkle DAG with structural sharing. `DerivationId::digest()`
//! is unchanged; this is a storage-only identity.

use std::collections::BTreeMap;
use std::sync::Arc;

use brix_canon::Digest;
use soc_core::store::{NodeStore, StorageError};

use super::network::DerivationId;
use super::persistent::PMap;
use super::state_codec::{dnode_digest_of_bytes, DNode, StateCodec, StateCodecError, ValueList};
use super::state_store::{StateError, StateStore};

/// Reference to an interned derivation node.
pub type DerivRef = Digest;

fn build(d: &DerivationId, mut sink: Option<&mut PMap<Digest, DNode>>) -> Digest {
    let node = match d {
        DerivationId::Base { relation, key } => DNode::Base {
            relation: relation.clone(),
            key: key.clone(),
        },
        DerivationId::Unary { op, parent } => DNode::Unary {
            op: *op,
            parent: build(parent, sink.as_deref_mut()),
        },
        DerivationId::Join { op, left, right } => DNode::Join {
            op: *op,
            left: build(left, sink.as_deref_mut()),
            right: build(right, sink.as_deref_mut()),
        },
        DerivationId::Group { op, group_key } => DNode::Group {
            op: *op,
            group_key: ValueList(group_key.clone()),
        },
        DerivationId::Distinct { op, tuple_key } => DNode::Distinct {
            op: *op,
            tuple_key: tuple_key.clone(),
        },
    };
    let digest = node.digest();
    if let Some(map) = sink {
        if !map.contains_key(&digest) {
            map.insert(digest, node);
        }
    }
    digest
}

/// The interned reference of a derivation id (no side effects).
pub fn deriv_ref(d: &DerivationId) -> Digest {
    build(d, None)
}

/// Intern `d` (and all its descendants) into the pending map; returns its ref.
pub fn intern(d: &DerivationId, pending: &mut PMap<Digest, DNode>) -> Digest {
    build(d, Some(pending))
}

fn fetch(
    r: &Digest,
    pending: &PMap<Digest, DNode>,
    store: &StateStore,
) -> Result<DNode, StateError> {
    if let Some(n) = pending.get(r) {
        return Ok(n.clone());
    }
    let bytes = store
        .get_node(r)
        .ok_or(StateError::Storage(StorageError::MissingNode(*r)))?;
    if dnode_digest_of_bytes(&bytes) != *r {
        return Err(StateError::Storage(StorageError::CorruptedNode(*r)));
    }
    Ok(DNode::state_from_bytes(&bytes)?)
}

type Memo = BTreeMap<Digest, (Arc<DerivationId>, usize)>;

fn resolve_rec(
    r: &Digest,
    pending: &PMap<Digest, DNode>,
    store: &StateStore,
    depth_left: usize,
    memo: &mut Memo,
) -> Result<Arc<DerivationId>, StateError> {
    if depth_left == 0 {
        return Err(StateError::Codec(StateCodecError::DepthExceeded));
    }
    // A memoised subtree is only reusable if it fits the remaining depth budget.
    if let Some((d, h)) = memo.get(r) {
        if *h <= depth_left {
            return Ok(d.clone());
        }
        return Err(StateError::Codec(StateCodecError::DepthExceeded));
    }
    if let Some((d, h)) = store.deriv_cache_get(r) {
        if h > depth_left {
            return Err(StateError::Codec(StateCodecError::DepthExceeded));
        }
        memo.insert(*r, (d.clone(), h));
        return Ok(d);
    }
    let (value, height) = match fetch(r, pending, store)? {
        DNode::Base { relation, key } => (DerivationId::Base { relation, key }, 1),
        DNode::Unary { op, parent } => {
            let p = resolve_rec(&parent, pending, store, depth_left - 1, memo)?;
            let h = 1 + memo[&parent].1;
            (
                DerivationId::Unary {
                    op,
                    parent: Box::new((*p).clone()),
                },
                h,
            )
        }
        DNode::Join { op, left, right } => {
            let l = resolve_rec(&left, pending, store, depth_left - 1, memo)?;
            let rr = resolve_rec(&right, pending, store, depth_left - 1, memo)?;
            let h = 1 + memo[&left].1.max(memo[&right].1);
            (
                DerivationId::Join {
                    op,
                    left: Box::new((*l).clone()),
                    right: Box::new((*rr).clone()),
                },
                h,
            )
        }
        DNode::Group { op, group_key } => (
            DerivationId::Group {
                op,
                group_key: group_key.0,
            },
            1,
        ),
        DNode::Distinct { op, tuple_key } => (DerivationId::Distinct { op, tuple_key }, 1),
    };
    let arc = Arc::new(value);
    memo.insert(*r, (arc.clone(), height));
    store.deriv_cache_put(*r, (arc.clone(), height));
    Ok(arc)
}

/// Rebuild the derivation tree for `r`, consulting `pending` first, then the store.
///
/// Nesting deeper than `max_depth` (root counts as 1) is rejected; every stored object is
/// verified against its digest; missing nodes are typed errors.
pub fn resolve(
    r: &Digest,
    pending: &PMap<Digest, DNode>,
    store: &StateStore,
    max_depth: usize,
) -> Result<Arc<DerivationId>, StateError> {
    let mut memo = BTreeMap::new();
    resolve_rec(r, pending, store, max_depth, &mut memo)
}

/// Write all pending nodes as raw content-addressed objects (idempotent). Returns the number
/// of nodes newly written.
pub fn persist_interned(
    pending: &PMap<Digest, DNode>,
    store: &StateStore,
) -> Result<usize, StateError> {
    let mut handle = store.clone();
    let mut written = 0;
    for (d, node) in pending.iter() {
        if NodeStore::contains(&handle, d) {
            continue;
        }
        let bytes = node.state_bytes();
        if dnode_digest_of_bytes(&bytes) != *d {
            return Err(StateError::Storage(StorageError::CorruptedNode(*d)));
        }
        handle.put_node(*d, bytes);
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::network::{IntermediateTuple, Value};
    use crate::world::state_codec::JoinEntry;
    use crate::world::types::WorldKey;
    use brix_lower::relation_dag::OperatorId;
    use proptest::prelude::*;

    fn base(n: u8) -> DerivationId {
        DerivationId::Base {
            relation: "r".into(),
            key: WorldKey(vec![n]),
        }
    }

    fn arb_deriv() -> impl Strategy<Value = DerivationId> {
        let leaf = prop_oneof![
            ("[a-z]{1,4}", proptest::collection::vec(any::<u8>(), 0..4)).prop_map(|(r, k)| {
                DerivationId::Base {
                    relation: r,
                    key: WorldKey(k),
                }
            }),
            (0usize..9, proptest::collection::vec(any::<i64>(), 0..3)).prop_map(|(o, g)| {
                DerivationId::Group {
                    op: OperatorId(o),
                    group_key: g.into_iter().map(Value::Int).collect(),
                }
            }),
            (0usize..9, proptest::collection::vec(any::<u8>(), 0..4)).prop_map(|(o, t)| {
                DerivationId::Distinct {
                    op: OperatorId(o),
                    tuple_key: t,
                }
            }),
        ];
        leaf.prop_recursive(4, 24, 2, |inner| {
            prop_oneof![
                (0usize..9, inner.clone()).prop_map(|(o, p)| DerivationId::Unary {
                    op: OperatorId(o),
                    parent: Box::new(p)
                }),
                (0usize..9, inner.clone(), inner).prop_map(|(o, l, r)| DerivationId::Join {
                    op: OperatorId(o),
                    left: Box::new(l),
                    right: Box::new(r)
                }),
            ]
        })
    }

    proptest! {
        #[test]
        fn round_trip_pending_and_store(d in arb_deriv()) {
            let mut pending = PMap::new();
            let r = intern(&d, &mut pending);
            prop_assert_eq!(r, deriv_ref(&d));
            let store = StateStore::memory(1 << 20);
            let got = resolve(&r, &pending, &store, 64).unwrap();
            prop_assert_eq!(&*got, &d);
            persist_interned(&pending, &store).unwrap();
            prop_assert_eq!(persist_interned(&pending, &store).unwrap(), 0);
            let got = resolve(&r, &PMap::new(), &store, 64).unwrap();
            prop_assert_eq!(&*got, &d);
        }

        #[test]
        fn ref_independent_of_construction_path(d in arb_deriv()) {
            let mut a = PMap::new();
            let ra = intern(&d, &mut a);
            let mut b = PMap::new();
            let rb = intern(&d.clone(), &mut b);
            prop_assert_eq!(ra, rb);
            prop_assert_eq!(a.len(), b.len());
            let n = a.len();
            intern(&d, &mut a);
            prop_assert_eq!(a.len(), n);
        }

        #[test]
        fn dnode_codec_round_trips(d in arb_deriv()) {
            let mut pending = PMap::new();
            intern(&d, &mut pending);
            for (dg, node) in pending.iter() {
                let b = node.state_bytes();
                prop_assert_eq!(&DNode::state_from_bytes(&b).unwrap(), node);
                prop_assert_eq!(dnode_digest_of_bytes(&b), *dg);
                let mut extra = b.clone();
                extra.push(0);
                prop_assert_eq!(
                    DNode::state_from_bytes(&extra).err(),
                    Some(StateCodecError::TrailingBytes)
                );
            }
        }
    }

    fn group(v: Value) -> DerivationId {
        DerivationId::Group {
            op: OperatorId(1),
            group_key: vec![v],
        }
    }

    #[test]
    fn typed_group_keys_distinct() {
        let a = deriv_ref(&group(Value::Int(1)));
        let b = deriv_ref(&group(Value::Str("1".into())));
        assert_ne!(a, b);
    }

    fn chain(n: usize) -> DerivationId {
        let mut d = base(0);
        for _ in 0..n {
            d = DerivationId::Unary {
                op: OperatorId(1),
                parent: Box::new(d),
            };
        }
        d
    }

    #[test]
    fn depth_bound_rejects_deep_trees() {
        let d = chain(10); // depth 11
        let mut pending = PMap::new();
        let r = intern(&d, &mut pending);
        let store = StateStore::memory(1 << 20);
        assert!(resolve(&r, &pending, &store, 11).is_ok());
        assert_eq!(
            resolve(&r, &pending, &store, 10).err(),
            Some(StateError::Codec(StateCodecError::DepthExceeded))
        );
        assert!(resolve(&r, &pending, &store, 0).is_err());
    }

    #[test]
    fn shared_subtrees_resolve_with_depth_bound() {
        let mut d = base(1);
        for _ in 0..10 {
            d = DerivationId::Join {
                op: OperatorId(2),
                left: Box::new(d.clone()),
                right: Box::new(d),
            };
        }
        let mut pending = PMap::new();
        let r = intern(&d, &mut pending);
        assert_eq!(pending.len(), 11);
        let store = StateStore::memory(1 << 20);
        assert!(resolve(&r, &pending, &store, 11).is_ok());
        assert!(resolve(&r, &pending, &store, 10).is_err());
    }

    #[test]
    fn missing_node_is_typed_error() {
        let mut pending = PMap::new();
        let r = intern(&chain(3), &mut pending);
        let empty = StateStore::memory(1 << 20);
        assert!(matches!(
            resolve(&r, &PMap::new(), &empty, 64),
            Err(StateError::Storage(StorageError::MissingNode(_)))
        ));
        // Only the root object persisted; its parent is missing.
        let partial = StateStore::memory(1 << 20);
        let mut only_root = PMap::new();
        only_root.insert(r, pending.get(&r).unwrap().clone());
        persist_interned(&only_root, &partial).unwrap();
        assert!(matches!(
            resolve(&r, &PMap::new(), &partial, 64),
            Err(StateError::Storage(StorageError::MissingNode(_)))
        ));
    }

    #[test]
    fn corrupt_node_is_typed_error() {
        let node = DNode::Base {
            relation: "r".into(),
            key: WorldKey(vec![1]),
        };
        let r = node.digest();
        let mut h = StateStore::memory(1 << 20);
        let other = DNode::Distinct {
            op: OperatorId(1),
            tuple_key: vec![],
        };
        h.put_node(r, other.state_bytes());
        assert!(matches!(
            resolve(&r, &PMap::new(), &h, 8),
            Err(StateError::Storage(StorageError::CorruptedNode(_)))
        ));
        // Hashes correctly but is not a canonical node (trailing byte).
        let mut bad = node.state_bytes();
        bad.push(7);
        let r2 = dnode_digest_of_bytes(&bad);
        let mut h2 = StateStore::memory(1 << 20);
        h2.put_node(r2, bad);
        assert!(matches!(
            resolve(&r2, &PMap::new(), &h2, 8),
            Err(StateError::Codec(StateCodecError::TrailingBytes))
        ));
        let junk = vec![99u8, 1, 2];
        let r3 = dnode_digest_of_bytes(&junk);
        let mut h3 = StateStore::memory(1 << 20);
        h3.put_node(r3, junk);
        assert!(matches!(
            resolve(&r3, &PMap::new(), &h3, 8),
            Err(StateError::Codec(_))
        ));
    }

    #[test]
    fn join_entry_round_trips() {
        let mut t = IntermediateTuple::new();
        t.fields.insert("a".into(), Value::Int(1));
        let e = JoinEntry {
            tuple: t,
            deriv: deriv_ref(&base(3)),
        };
        let b = e.state_bytes();
        assert_eq!(JoinEntry::state_from_bytes(&b).unwrap(), e);
    }
}
