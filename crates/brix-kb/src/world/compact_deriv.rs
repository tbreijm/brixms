//! Compact derivations (R0 Phase 2, replaces the interned derivation tree).
//!
//! A derivation tree is fully determined by the operator DAG plus the leaves it bottoms out
//! in, so it is represented as `CompactDeriv { root, leaves }`:
//!
//! - `root` is `Base(relation)` for a scan output (the derivation is `Base{relation, key}`)
//!   or `Op(op)`, the operator whose output the derivation justifies;
//! - `leaves` lists the scan keys / distinct keys / group keys the tree bottoms out in, in
//!   left-to-right unfold order. The DAG tells how many leaves each operator contributes
//!   and of which kind, so [`CompactDeriv::expand`] rebuilds the exact [`DerivationId`].
//!
//! Invariants: `expand(compact(d)) == d` (bijection for derivations the network produces),
//! equality of compact derivations is equality of derivation ids, and the encoding
//! (`brix.world.deriv@2`) is injective and deterministic, so its digest is a stable
//! identity usable as a trie key. `DerivationId::digest()` is unchanged (storage identity
//! only).

use std::sync::Arc;

use brix_canon::{CanonReader, CanonWriter, Digest, Domain};
use brix_lower::relation_dag::{OperatorId, OperatorNode, RelationDag};

use super::network::{DerivationId, Value};
use super::state_codec::{noncanon, read_op, read_string, StateCodec, StateCodecError};
use super::types::WorldKey;

type Res<T> = Result<T, StateCodecError>;

/// Codec / identity version of compact derivations.
pub const DERIV_CODEC_VERSION: &str = "brix.world.deriv@2";

/// Root of a compact derivation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DRoot {
    /// A scan output: `DerivationId::Base { relation, .. }`.
    Base(Arc<str>),
    /// The output of an operator.
    Op(OperatorId),
}

/// One leaf of a derivation tree.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Leaf {
    Base(WorldKey),
    Distinct(Vec<u8>),
    Group(Vec<Value>),
}

/// A derivation tree in compact form (see module docs).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompactDeriv {
    pub root: DRoot,
    pub leaves: Arc<[Leaf]>,
}

const R_BASE: u64 = 0;
const R_OP: u64 = 1;
const L_BASE: u64 = 0;
const L_DISTINCT: u64 = 1;
const L_GROUP: u64 = 2;

impl StateCodec for CompactDeriv {
    fn state_write(&self, w: &mut CanonWriter) {
        match &self.root {
            DRoot::Base(rel) => {
                w.write_uint(R_BASE);
                w.write_str(rel);
            }
            DRoot::Op(op) => {
                w.write_uint(R_OP);
                w.write_uint(op.0 as u64);
            }
        }
        w.write_uint(self.leaves.len() as u64);
        for l in self.leaves.iter() {
            match l {
                Leaf::Base(k) => {
                    w.write_uint(L_BASE);
                    w.write_bytes(k.as_bytes());
                }
                Leaf::Distinct(k) => {
                    w.write_uint(L_DISTINCT);
                    w.write_bytes(k);
                }
                Leaf::Group(vs) => {
                    w.write_uint(L_GROUP);
                    w.write_uint(vs.len() as u64);
                    for v in vs {
                        v.state_write(w);
                    }
                }
            }
        }
    }

    fn state_read(r: &mut CanonReader<'_>) -> Res<Self> {
        let root = match r.read_uint()? {
            R_BASE => DRoot::Base(Arc::from(read_string(r)?)),
            R_OP => DRoot::Op(read_op(r)?),
            t => return Err(StateCodecError::UnknownTag(t)),
        };
        let n = r.read_uint()?;
        let mut leaves = Vec::new();
        for _ in 0..n {
            leaves.push(match r.read_uint()? {
                L_BASE => Leaf::Base(WorldKey(r.read_bytes()?.to_vec())),
                L_DISTINCT => Leaf::Distinct(r.read_bytes()?.to_vec()),
                L_GROUP => {
                    let k = r.read_uint()?;
                    let mut vs = Vec::new();
                    for _ in 0..k {
                        vs.push(Value::state_read(r)?);
                    }
                    Leaf::Group(vs)
                }
                t => return Err(StateCodecError::UnknownTag(t)),
            });
        }
        Ok(CompactDeriv {
            root,
            leaves: leaves.into(),
        })
    }
}

/// Number of leaves each operator's derivations carry (index = operator id).
pub fn leaf_counts(dag: &RelationDag) -> Vec<usize> {
    fn count(dag: &RelationDag, op: usize, memo: &mut Vec<Option<usize>>) -> usize {
        if let Some(c) = memo[op] {
            return c;
        }
        let c = match &dag.nodes[op] {
            OperatorNode::Scan { .. }
            | OperatorNode::Distinct { .. }
            | OperatorNode::GroupedCount { .. } => 1,
            OperatorNode::Bind { input, .. }
            | OperatorNode::Filter { input, .. }
            | OperatorNode::Project { input, .. } => count(dag, input.0, memo),
            OperatorNode::EquiJoin { left, right, .. } => {
                count(dag, left.0, memo) + count(dag, right.0, memo)
            }
        };
        memo[op] = Some(c);
        c
    }
    let mut memo = vec![None; dag.nodes.len()];
    (0..dag.nodes.len())
        .map(|i| count(dag, i, &mut memo))
        .collect()
}

impl CompactDeriv {
    /// Content address of this derivation (domain separated digest of its encoding).
    pub fn digest(&self) -> Digest {
        let bytes = self.state_bytes();
        let mut payload = Vec::with_capacity(DERIV_CODEC_VERSION.len() + 1 + bytes.len());
        payload.extend_from_slice(DERIV_CODEC_VERSION.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&bytes);
        Digest::of(Domain::Value, &payload)
    }

    /// Cheap structural check against the DAG: the leaf count matches the root.
    pub fn leaf_count_ok(&self, counts: &[usize]) -> bool {
        match &self.root {
            DRoot::Base(_) => self.leaves.len() == 1,
            DRoot::Op(op) => counts.get(op.0) == Some(&self.leaves.len()),
        }
    }

    /// Full structural check against the DAG without building the [`DerivationId`]: the leaf
    /// count and the kind of every leaf match the unfold of the root operator. Used on every
    /// derivation read back from the store (a stored entry that fails it is corrupt state).
    pub fn check_shape(&self, dag: &RelationDag) -> Res<()> {
        match &self.root {
            DRoot::Base(_) => match &*self.leaves {
                [Leaf::Base(_)] => Ok(()),
                _ => noncanon("base root requires exactly one base leaf"),
            },
            DRoot::Op(op) => {
                let mut i = 0usize;
                shape(dag, *op, &self.leaves, &mut i)?;
                if i != self.leaves.len() {
                    return noncanon("derivation has more leaves than its operator unfolds to");
                }
                Ok(())
            }
        }
    }

    /// Rebuild the exact [`DerivationId`]. Fails (typed) if the leaves do not match the
    /// unfold of the root operator (count or kind).
    pub fn expand(&self, dag: &RelationDag) -> Res<DerivationId> {
        match &self.root {
            DRoot::Base(rel) => match &*self.leaves {
                [Leaf::Base(key)] => Ok(DerivationId::Base {
                    relation: rel.to_string(),
                    key: key.clone(),
                }),
                _ => noncanon("base root requires exactly one base leaf"),
            },
            DRoot::Op(op) => {
                let mut i = 0usize;
                let d = unfold(dag, *op, &self.leaves, &mut i)?;
                if i != self.leaves.len() {
                    return noncanon("derivation has more leaves than its operator unfolds to");
                }
                Ok(d)
            }
        }
    }

    /// Compact form of a derivation id (inverse of [`Self::expand`]).
    pub fn from_derivation(d: &DerivationId) -> Self {
        fn leaves_of(d: &DerivationId, out: &mut Vec<Leaf>) {
            match d {
                DerivationId::Base { key, .. } => out.push(Leaf::Base(key.clone())),
                DerivationId::Unary { parent, .. } => leaves_of(parent, out),
                DerivationId::Join { left, right, .. } => {
                    leaves_of(left, out);
                    leaves_of(right, out);
                }
                DerivationId::Group { group_key, .. } => out.push(Leaf::Group(group_key.clone())),
                DerivationId::Distinct { tuple_key, .. } => {
                    out.push(Leaf::Distinct(tuple_key.clone()))
                }
            }
        }
        let root = match d {
            DerivationId::Base { relation, .. } => DRoot::Base(Arc::from(relation.as_str())),
            DerivationId::Unary { op, .. }
            | DerivationId::Join { op, .. }
            | DerivationId::Group { op, .. }
            | DerivationId::Distinct { op, .. } => DRoot::Op(*op),
        };
        let mut leaves = Vec::new();
        leaves_of(d, &mut leaves);
        CompactDeriv {
            root,
            leaves: leaves.into(),
        }
    }
}

fn next_leaf<'a>(leaves: &'a [Leaf], i: &mut usize) -> Res<&'a Leaf> {
    let l = leaves
        .get(*i)
        .ok_or_else(|| StateCodecError::NonCanonical("derivation has too few leaves".into()))?;
    *i += 1;
    Ok(l)
}

fn shape(dag: &RelationDag, op: OperatorId, leaves: &[Leaf], i: &mut usize) -> Res<()> {
    let node = dag
        .nodes
        .get(op.0)
        .ok_or_else(|| StateCodecError::NonCanonical(format!("unknown operator {}", op.0)))?;
    match node {
        OperatorNode::Scan { .. } => match next_leaf(leaves, i)? {
            Leaf::Base(_) => Ok(()),
            _ => noncanon("scan operator expects a base leaf"),
        },
        OperatorNode::Bind { input, .. }
        | OperatorNode::Filter { input, .. }
        | OperatorNode::Project { input, .. } => shape(dag, *input, leaves, i),
        OperatorNode::EquiJoin { left, right, .. } => {
            shape(dag, *left, leaves, i)?;
            shape(dag, *right, leaves, i)
        }
        OperatorNode::Distinct { .. } => match next_leaf(leaves, i)? {
            Leaf::Distinct(_) => Ok(()),
            _ => noncanon("distinct operator expects a distinct leaf"),
        },
        OperatorNode::GroupedCount { .. } => match next_leaf(leaves, i)? {
            Leaf::Group(_) => Ok(()),
            _ => noncanon("grouped-count operator expects a group leaf"),
        },
    }
}

fn unfold(dag: &RelationDag, op: OperatorId, leaves: &[Leaf], i: &mut usize) -> Res<DerivationId> {
    let node = dag
        .nodes
        .get(op.0)
        .ok_or_else(|| StateCodecError::NonCanonical(format!("unknown operator {}", op.0)))?;
    match node {
        OperatorNode::Scan { relation, .. } => match next_leaf(leaves, i)? {
            Leaf::Base(key) => Ok(DerivationId::Base {
                relation: relation.clone(),
                key: key.clone(),
            }),
            _ => noncanon("scan operator expects a base leaf"),
        },
        OperatorNode::Bind { input, .. }
        | OperatorNode::Filter { input, .. }
        | OperatorNode::Project { input, .. } => Ok(DerivationId::Unary {
            op,
            parent: Box::new(unfold(dag, *input, leaves, i)?),
        }),
        OperatorNode::EquiJoin { left, right, .. } => {
            let l = unfold(dag, *left, leaves, i)?;
            let r = unfold(dag, *right, leaves, i)?;
            Ok(DerivationId::Join {
                op,
                left: Box::new(l),
                right: Box::new(r),
            })
        }
        OperatorNode::Distinct { .. } => match next_leaf(leaves, i)? {
            Leaf::Distinct(k) => Ok(DerivationId::Distinct {
                op,
                tuple_key: k.clone(),
            }),
            _ => noncanon("distinct operator expects a distinct leaf"),
        },
        OperatorNode::GroupedCount { .. } => match next_leaf(leaves, i)? {
            Leaf::Group(k) => Ok(DerivationId::Group {
                op,
                group_key: k.clone(),
            }),
            _ => noncanon("grouped-count operator expects a group leaf"),
        },
    }
}

/// A derivation as carried by a delta: the compact form, plus (tests / `r0-oracle`) the
/// legacy [`DerivationId`] built in parallel by the pre-compact logic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derivation {
    pub c: CompactDeriv,
    #[cfg(any(test, feature = "r0-oracle"))]
    legacy: DerivationId,
}

impl Derivation {
    /// Output of a scan: `Base { relation, key }`.
    pub fn base(relation: &str, key: &WorldKey) -> Self {
        Self {
            c: CompactDeriv {
                root: DRoot::Base(Arc::from(relation)),
                leaves: Arc::from([Leaf::Base(key.clone())]),
            },
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy: DerivationId::Base {
                relation: relation.to_string(),
                key: key.clone(),
            },
        }
    }

    /// Output of a unary operator over `self` (leaves are shared).
    pub fn unary(&self, op: OperatorId) -> Self {
        Self {
            c: CompactDeriv {
                root: DRoot::Op(op),
                leaves: self.c.leaves.clone(),
            },
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy: DerivationId::Unary {
                op,
                parent: Box::new(self.legacy.clone()),
            },
        }
    }

    /// Output of join `op` over `left` and `right` (left leaves first).
    pub fn join(op: OperatorId, left: &Derivation, right: &Derivation) -> Self {
        let mut leaves = Vec::with_capacity(left.c.leaves.len() + right.c.leaves.len());
        leaves.extend_from_slice(&left.c.leaves);
        leaves.extend_from_slice(&right.c.leaves);
        Self {
            c: CompactDeriv {
                root: DRoot::Op(op),
                leaves: leaves.into(),
            },
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy: DerivationId::Join {
                op,
                left: Box::new(left.legacy.clone()),
                right: Box::new(right.legacy.clone()),
            },
        }
    }

    /// Output of a distinct operator for `tuple_key`.
    pub fn distinct(op: OperatorId, tuple_key: Vec<u8>) -> Self {
        Self {
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy: DerivationId::Distinct {
                op,
                tuple_key: tuple_key.clone(),
            },
            c: CompactDeriv {
                root: DRoot::Op(op),
                leaves: Arc::from([Leaf::Distinct(tuple_key)]),
            },
        }
    }

    /// Output of a grouped-count operator for `group_key`.
    pub fn group(op: OperatorId, group_key: Vec<Value>) -> Self {
        Self {
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy: DerivationId::Group {
                op,
                group_key: group_key.clone(),
            },
            c: CompactDeriv {
                root: DRoot::Op(op),
                leaves: Arc::from([Leaf::Group(group_key)]),
            },
        }
    }

    /// Wrap a derivation read back from the store (the legacy shadow is its expansion).
    pub fn from_stored(c: CompactDeriv, dag: &RelationDag) -> Res<Self> {
        #[cfg(any(test, feature = "r0-oracle"))]
        let legacy = c.expand(dag)?;
        #[cfg(not(any(test, feature = "r0-oracle")))]
        let _ = dag;
        Ok(Self {
            c,
            #[cfg(any(test, feature = "r0-oracle"))]
            legacy,
        })
    }

    pub fn digest(&self) -> Digest {
        self.c.digest()
    }

    pub fn expand(&self, dag: &RelationDag) -> Res<DerivationId> {
        self.c.expand(dag)
    }

    /// The legacy derivation built by the pre-compact logic.
    #[cfg(any(test, feature = "r0-oracle"))]
    pub fn legacy(&self) -> &DerivationId {
        &self.legacy
    }

    /// Oracle invariant I1: the compact form expands to exactly the legacy derivation and
    /// converts back to itself.
    #[cfg(any(test, feature = "r0-oracle"))]
    pub fn check(&self, dag: &RelationDag) -> Res<()> {
        let expanded = self.c.expand(dag)?;
        assert_eq!(
            expanded, self.legacy,
            "compact derivation does not expand to the legacy derivation"
        );
        assert_eq!(
            CompactDeriv::from_derivation(&self.legacy),
            self.c,
            "compact(legacy) != compact"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_syntax::ast;

    fn scan(dag: &mut RelationDag, rel: &str) -> OperatorId {
        dag.add_node(OperatorNode::Scan {
            relation: rel.to_string(),
            schema: ast::Ty::Record(Vec::new()),
            key_fields: Vec::new(),
        })
    }

    /// scan A -> bind ; scan B ; join(bind, B) ; distinct ; grouped count over distinct.
    fn dag() -> (RelationDag, [OperatorId; 6]) {
        let mut d = RelationDag::new();
        let a = scan(&mut d, "A");
        let b = scan(&mut d, "B");
        let bind = d.add_node(OperatorNode::Bind {
            input: a,
            alias: "a".into(),
        });
        let join = d.add_node(OperatorNode::EquiJoin {
            left: bind,
            right: b,
            left_keys: Vec::new(),
            right_keys: Vec::new(),
        });
        let dist = d.add_node(OperatorNode::Distinct { input: join });
        let grp = d.add_node(OperatorNode::GroupedCount {
            input: dist,
            group_keys: Vec::new(),
            projections: Vec::new(),
        });
        (d, [a, b, bind, join, dist, grp])
    }

    #[test]
    fn expand_matches_legacy_and_roundtrips() {
        let (dag, [_, _, bind, join, dist, grp]) = dag();
        let ka = WorldKey(vec![1]);
        let kb = WorldKey(vec![2]);
        let a = Derivation::base("A", &ka).unary(bind);
        let b = Derivation::base("B", &kb);
        let j = Derivation::join(join, &a, &b);
        let d = Derivation::distinct(dist, vec![9, 9]);
        let g = Derivation::group(grp, vec![Value::Int(1)]);
        assert_eq!(leaf_counts(&dag), vec![1, 1, 1, 2, 1, 1]);
        for x in [&a, &b, &j, &d, &g] {
            x.check(&dag).unwrap();
            let bytes = x.c.state_bytes();
            assert_eq!(CompactDeriv::state_from_bytes(&bytes).unwrap(), x.c);
            assert_eq!(
                x.c.digest(),
                CompactDeriv::from_derivation(x.legacy()).digest()
            );
        }
        assert_eq!(
            j.expand(&dag).unwrap(),
            DerivationId::Join {
                op: join,
                left: Box::new(DerivationId::Unary {
                    op: bind,
                    parent: Box::new(DerivationId::Base {
                        relation: "A".into(),
                        key: ka
                    })
                }),
                right: Box::new(DerivationId::Base {
                    relation: "B".into(),
                    key: kb
                }),
            }
        );
    }

    #[test]
    fn leaf_shape_mismatch_is_rejected() {
        let (dag, [_, _, _, join, dist, _]) = dag();
        let counts = leaf_counts(&dag);
        let short = CompactDeriv {
            root: DRoot::Op(join),
            leaves: Arc::from([Leaf::Base(WorldKey(vec![1]))]),
        };
        assert!(!short.leaf_count_ok(&counts));
        assert!(short.expand(&dag).is_err());
        let wrong = CompactDeriv {
            root: DRoot::Op(dist),
            leaves: Arc::from([Leaf::Base(WorldKey(vec![1]))]),
        };
        assert!(wrong.expand(&dag).is_err());
        let extra = CompactDeriv {
            root: DRoot::Op(dist),
            leaves: Arc::from([Leaf::Distinct(vec![1]), Leaf::Distinct(vec![2])]),
        };
        assert!(extra.expand(&dag).is_err());
    }

    #[test]
    fn typed_group_leaves_are_distinct() {
        let a = Derivation::group(OperatorId(1), vec![Value::Int(1)]);
        let b = Derivation::group(OperatorId(1), vec![Value::Str("1".into())]);
        assert_ne!(a.c.digest(), b.c.digest());
    }
}
