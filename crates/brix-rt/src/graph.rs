//! `GraphCore` — node interner + arenas, edge identity resolution, and the
//! global incidence index (Ring0 §1.7).
//!
//! Lineage note carried from the build plan: incidence detection was the
//! original Brix primitive — the thesis's collision loop — and this index is
//! its production form. Physically, pass-1 compiles the hypergraph away per
//! relation into monomorphized columnar stores; `GraphCore` is the one
//! cross-relation structure the runtime still maintains, because nothing
//! else can answer "every edge touching this node" without either owning a
//! second copy of every relation or scanning all of them. It powers `why`,
//! erasure propagation, path evaluation, and visualization.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use brix_canon::{CanonWriter, Canonical, EdgeId, NodeId};

use crate::ids::{RelationRef, RoleRef};
use crate::value::EdgeRoleTuple;

/// A dense arena handle for an interned node. Not itself semantic — it never
/// crosses a canon boundary and carries no ordering promise beyond what this
/// process assigned at intern time. Callers that need a stable, comparable
/// identity use [`NodeId`] instead.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeIdx(u32);

impl NodeIdx {
    fn from_usize(i: usize) -> Self {
        NodeIdx(u32::try_from(i).expect("GraphCore: node arena exceeded u32::MAX entries"))
    }

    fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// One posting in a node's incidence list: "this node participates in role
/// `role` of edge `edge` of relation `relation`." The set of postings for a
/// node is exactly "every edge touching this node" (Ring0 §1.7).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct IncidencePosting {
    /// The relation the edge belongs to.
    pub relation: RelationRef,
    /// The role this node fills on the edge.
    pub role: RoleRef,
    /// The edge's identity.
    pub edge: EdgeId,
}

/// Node interner + arenas + global incidence index.
///
/// - The interner (`by_id`) maps a [`NodeId`] to its dense [`NodeIdx`] in
///   canon byte order (`BTreeMap`, never `HashMap` — Ring0 §0).
/// - The arena (`nodes`) is the reverse map, `NodeIdx -> NodeId`; its
///   iteration order is arena/insertion order, which is never observed
///   directly (only through a `NodeId` lookup), so it is a plain `Vec`.
/// - The incidence index (`incidence`) is the arena-indexed posting-list
///   structure: `NodeIdx -> BTreeSet<IncidencePosting>`, updated by
///   generated emission/retraction code (or, pre-codegen, by direct calls
///   from a `RelationStore` implementation) whenever a row involving that
///   node is inserted or removed.
#[derive(Default)]
pub struct GraphCore {
    by_id: BTreeMap<NodeId, NodeIdx>,
    nodes: Vec<NodeId>,
    incidence: Vec<BTreeSet<IncidencePosting>>,
}

impl GraphCore {
    /// A fresh, empty graph core.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct nodes interned so far.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Intern `id`, returning its arena index. Idempotent: interning an
    /// already-known id returns the same index every time.
    pub fn intern(&mut self, id: NodeId) -> NodeIdx {
        if let Some(&idx) = self.by_id.get(&id) {
            return idx;
        }
        let idx = NodeIdx::from_usize(self.nodes.len());
        self.nodes.push(id);
        self.incidence.push(BTreeSet::new());
        self.by_id.insert(id, idx);
        idx
    }

    /// Look up an already-interned node's arena index without interning it.
    pub fn lookup(&self, id: NodeId) -> Option<NodeIdx> {
        self.by_id.get(&id).copied()
    }

    /// Resolve an arena index back to its canon identity.
    pub fn resolve(&self, idx: NodeIdx) -> NodeId {
        self.nodes[idx.as_usize()]
    }

    /// Record that `node` participates in `posting` (an emission of a row
    /// touching that node). Interns `node` first if this is its first
    /// appearance. Returns `true` if this was a new posting.
    pub fn record_incidence(&mut self, node: NodeId, posting: IncidencePosting) -> bool {
        let idx = self.intern(node);
        self.incidence[idx.as_usize()].insert(posting)
    }

    /// Remove a posting on retraction/loss-of-support of the row it names.
    /// Returns `true` if the posting was present. A no-op (returns `false`)
    /// if `node` was never interned — retracting something never asserted is
    /// a caller bug elsewhere, not this index's problem to diagnose.
    pub fn remove_incidence(&mut self, node: NodeId, posting: &IncidencePosting) -> bool {
        match self.by_id.get(&node) {
            Some(&idx) => self.incidence[idx.as_usize()].remove(posting),
            None => false,
        }
    }

    /// Whether `node` has any recorded incidence (participates in at least
    /// one live edge). `false` for a node that was never interned.
    pub fn has_incidence(&self, node: NodeId) -> bool {
        self.by_id
            .get(&node)
            .is_some_and(|&idx| !self.incidence[idx.as_usize()].is_empty())
    }

    /// Every edge touching `node`, in canonical order (relation, then role,
    /// then edge — `IncidencePosting`'s derived `Ord`). Empty if `node` was
    /// never interned. This is the "every edge touching this node" query
    /// that powers `why`, erasure propagation, path evaluation, and
    /// visualization (Ring0 §1.7).
    pub fn incident_iter(&self, node: NodeId) -> impl Iterator<Item = &IncidencePosting> {
        self.by_id
            .get(&node)
            .into_iter()
            .flat_map(move |&idx| self.incidence[idx.as_usize()].iter())
    }
}

/// Edge identity resolution (Part III §3): `EdgeId = Hash(relation
/// compatibility domain, canonical role tuple)`. The payload folds the
/// relation's own name in ahead of the role tuple bytes so that two
/// different relations can never collide on identical role bytes — the
/// spec's "relation compatibility domain" is realized here as the relation
/// name's canon bytes feeding the same `Domain::Edge`-tagged hash as the
/// role tuple, rather than a second hash domain (there is exactly one
/// serializer and one domain-separation mechanism, `brix-canon`'s
/// [`brix_canon::Domain`]).
pub fn edge_id(relation: &RelationRef, roles: &EdgeRoleTuple) -> EdgeId {
    let mut w = CanonWriter::new();
    relation.canon_write(&mut w);
    roles.canon_write(&mut w);
    EdgeId::from_canon(&w.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::RoleValue;

    fn n(tag: &[u8]) -> NodeId {
        NodeId::from_canon(tag)
    }

    #[test]
    fn intern_is_idempotent_and_reversible() {
        let mut g = GraphCore::new();
        let id = n(b"order-1");
        let a = g.intern(id);
        let b = g.intern(id);
        assert_eq!(a, b);
        assert_eq!(g.resolve(a), id);
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn distinct_nodes_get_distinct_indices() {
        let mut g = GraphCore::new();
        let a = g.intern(n(b"a"));
        let b = g.intern(n(b"b"));
        assert_ne!(a, b);
    }

    #[test]
    fn incidence_records_and_removes() {
        let mut g = GraphCore::new();
        let order = n(b"order-1");
        let posting = IncidencePosting {
            relation: RelationRef::from("shipping.Move"),
            role: RoleRef::from("order"),
            edge: EdgeId::from_canon(b"move-edge"),
        };
        assert!(g.record_incidence(order, posting.clone()));
        // Recording the same posting twice is a no-op the second time.
        assert!(!g.record_incidence(order, posting.clone()));
        let seen: Vec<_> = g.incident_iter(order).cloned().collect();
        assert_eq!(seen, vec![posting.clone()]);

        assert!(g.remove_incidence(order, &posting));
        assert!(g.incident_iter(order).next().is_none());
        // Removing again is a documented no-op.
        assert!(!g.remove_incidence(order, &posting));
    }

    #[test]
    fn incidence_on_unknown_node_is_empty_not_panicking() {
        let g = GraphCore::new();
        assert!(g.incident_iter(n(b"never-seen")).next().is_none());
        assert!(!g.has_incidence(n(b"never-seen")));
    }

    #[test]
    fn incidence_postings_iterate_in_canonical_order() {
        let mut g = GraphCore::new();
        let order = n(b"order-1");
        let p_b = IncidencePosting {
            relation: RelationRef::from("b.Rel"),
            role: RoleRef::from("order"),
            edge: EdgeId::from_canon(b"e1"),
        };
        let p_a = IncidencePosting {
            relation: RelationRef::from("a.Rel"),
            role: RoleRef::from("order"),
            edge: EdgeId::from_canon(b"e2"),
        };
        g.record_incidence(order, p_b.clone());
        g.record_incidence(order, p_a.clone());
        let seen: Vec<_> = g.incident_iter(order).cloned().collect();
        assert_eq!(
            seen,
            vec![p_a, p_b],
            "postings must sort by relation name (canonical order)"
        );
    }

    #[test]
    fn edge_id_is_deterministic_and_relation_scoped() {
        let roles = EdgeRoleTuple::new().with(RoleRef::from("order"), RoleValue::Node(n(b"o")));
        let id_move = edge_id(&RelationRef::from("shipping.Move"), &roles);
        let id_other = edge_id(&RelationRef::from("shipping.Other"), &roles);
        assert_ne!(
            id_move, id_other,
            "distinct relations over identical role bytes must not collide"
        );
        assert_eq!(
            id_move,
            edge_id(&RelationRef::from("shipping.Move"), &roles)
        );
    }

    proptest::proptest! {
        /// `GraphCore` is the runtime's one cross-relation structure feeding
        /// `why`/erasure/path evaluation (Ring0 §1.7); if interning weren't
        /// deterministic and idempotent under arbitrary access patterns, two
        /// engine runs (or engine vs. oracle) could disagree about node
        /// identity bookkeeping even while agreeing on every observable
        /// `NodeId`. This is the property conformance I.1/I.2 rely on this
        /// module to uphold internally.
        #[test]
        fn intern_is_deterministic_and_idempotent(tags in proptest::collection::vec(proptest::collection::vec(0u8..=255, 1..8), 0..50)) {
            let mut g = GraphCore::new();
            let mut first_pass = Vec::new();
            for tag in &tags {
                first_pass.push(g.intern(NodeId::from_canon(tag)));
            }
            // Re-interning the same sequence of ids yields the same indices,
            // in the same order, no matter how many times it happens.
            let mut second_pass = Vec::new();
            for tag in &tags {
                second_pass.push(g.intern(NodeId::from_canon(tag)));
            }
            proptest::prop_assert_eq!(&first_pass, &second_pass);
            // Every returned index resolves back to the id that produced it.
            for (tag, idx) in tags.iter().zip(first_pass.iter()) {
                proptest::prop_assert_eq!(g.resolve(*idx), NodeId::from_canon(tag));
            }
        }
    }
}
