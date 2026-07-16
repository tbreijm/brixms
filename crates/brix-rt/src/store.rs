//! `RelationStore` — the generic view every generated store implements
//! (Ring0 §1.7): iterate rows as canon values, look up by role, resolve
//! `EdgeRef`s. This is a zero-copy trait *projection* over specialized
//! columns — a generated relation's real storage is typed Rust columns; this
//! trait is how reflection, tooling, the delta ABI, and tier B see it
//! *without* a second copy of the data existing anywhere.
//!
//! `GenericRelation` in this module is not that generated storage. It is a
//! reference implementation used to exercise and test the trait contract
//! before `brixc` exists to generate anything, and it is what tier-B/
//! reflection/WASM-boundary code falls back to when it must hold rows
//! dynamically (e.g. rows just decoded off the delta-ABI wire, Appendix A
//! sealed relations inspected generically). `brix-oracle` — a different
//! lane — is the *authoritative* generic hypergraph; this is a `RelationStore`-shaped
//! utility, not a second oracle.

use std::collections::BTreeMap;

use brix_canon::EdgeId;

use crate::ids::RelationRef;
use crate::value::{EdgeRoleTuple, RoleValue};

/// A single row, viewed generically. Implementors back this by borrowing
/// their own storage — the trait exists so callers never need to know
/// whether a row lives in typed columns (generated stores) or a
/// `EdgeRoleTuple` (this crate's reference store, decoded WASM-boundary
/// rows).
pub trait Row {
    /// This row's edge identity.
    fn edge(&self) -> EdgeId;

    /// This row's binding for `role`, if the relation has that role.
    fn role(&self, role: &crate::ids::RoleRef) -> Option<RoleValue>;

    /// The full role tuple, canon-ordered. The default implementation is
    /// the honest one for the reference store; generated stores may prefer
    /// a faster path that skips this if a caller only needs one role.
    fn roles(&self) -> EdgeRoleTuple;
}

/// The generic, read-only view every relation store exposes (Ring0 §1.7).
pub trait RelationStore {
    /// The row type this store yields. Associated rather than boxed so a
    /// generated store can return a zero-copy borrowing row type; the
    /// reference implementation below returns an owned row cheaply cloned
    /// from its `BTreeMap`.
    type Row<'a>: Row
    where
        Self: 'a;

    /// The relation this store implements.
    fn relation(&self) -> &RelationRef;

    /// Iterate all live rows, in canonical (edge-id) order.
    fn rows(&self) -> Box<dyn Iterator<Item = Self::Row<'_>> + '_>;

    /// Rows whose `role` binding equals `value` (an index probe). The
    /// default implementation is a full scan — correct for any
    /// implementor, but generated stores are expected to override it with
    /// a real per-role index (Ring0 §1.7 "look up by role").
    fn by_role<'a>(
        &'a self,
        role: &crate::ids::RoleRef,
        value: &RoleValue,
    ) -> Box<dyn Iterator<Item = Self::Row<'a>> + 'a> {
        let role = role.clone();
        let value = value.clone();
        Box::new(
            self.rows()
                .filter(move |row| row.role(&role).as_ref() == Some(&value)),
        )
    }

    /// Resolve one edge reference to its row, if still live.
    fn resolve(&self, edge: EdgeId) -> Option<Self::Row<'_>> {
        self.rows().find(|row| row.edge() == edge)
    }

    /// Number of live rows.
    fn len(&self) -> usize {
        self.rows().count()
    }

    /// Whether the store currently has no live rows.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An owned row over an [`EdgeRoleTuple`] — [`GenericRelation`]'s `Row`.
#[derive(Clone, Debug)]
pub struct GenericRow {
    edge: EdgeId,
    roles: EdgeRoleTuple,
}

impl Row for GenericRow {
    fn edge(&self) -> EdgeId {
        self.edge
    }

    fn role(&self, role: &crate::ids::RoleRef) -> Option<RoleValue> {
        self.roles.get(role).cloned()
    }

    fn roles(&self) -> EdgeRoleTuple {
        self.roles.clone()
    }
}

/// A reference `RelationStore`: rows keyed by [`EdgeId`] in a `BTreeMap`, so
/// iteration is already canonical order and no secondary sort is needed.
/// This is the shape reflection/tier-B code should reach for when it needs
/// to hold decoded rows dynamically; generated (tier A) stores are typed
/// columns and do not use this type.
#[derive(Debug)]
pub struct GenericRelation {
    relation: RelationRef,
    rows: BTreeMap<EdgeId, EdgeRoleTuple>,
}

impl GenericRelation {
    /// An empty store for `relation`.
    pub fn new(relation: RelationRef) -> Self {
        GenericRelation {
            relation,
            rows: BTreeMap::new(),
        }
    }

    /// Insert (or replace) the row for `edge`. Mirrors what generated
    /// emission code does on `DeltaOp::Insert`.
    pub fn insert(&mut self, edge: EdgeId, roles: EdgeRoleTuple) {
        self.rows.insert(edge, roles);
    }

    /// Remove the row for `edge`, if present. Mirrors what generated
    /// retraction code does on `DeltaOp::Retract` / loss of last support.
    pub fn remove(&mut self, edge: EdgeId) -> Option<EdgeRoleTuple> {
        self.rows.remove(&edge)
    }
}

impl RelationStore for GenericRelation {
    type Row<'a> = GenericRow;

    fn relation(&self) -> &RelationRef {
        &self.relation
    }

    fn rows(&self) -> Box<dyn Iterator<Item = GenericRow> + '_> {
        Box::new(self.rows.iter().map(|(&edge, roles)| GenericRow {
            edge,
            roles: roles.clone(),
        }))
    }

    fn resolve(&self, edge: EdgeId) -> Option<GenericRow> {
        self.rows.get(&edge).map(|roles| GenericRow {
            edge,
            roles: roles.clone(),
        })
    }

    fn len(&self) -> usize {
        self.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge_id;
    use crate::ids::RoleRef;
    use brix_canon::NodeId;

    fn roles_for(order: &str) -> EdgeRoleTuple {
        EdgeRoleTuple::new().with(
            RoleRef::from("order"),
            RoleValue::Node(NodeId::from_canon(order.as_bytes())),
        )
    }

    #[test]
    fn insert_resolve_remove_round_trip() {
        let relation = RelationRef::from("shipping.Move");
        let mut store = GenericRelation::new(relation.clone());
        let roles = roles_for("o1");
        let edge = edge_id(&relation, &roles);

        store.insert(edge, roles.clone());
        assert_eq!(store.len(), 1);
        let row = store.resolve(edge).expect("row should resolve");
        assert_eq!(row.edge(), edge);
        assert_eq!(
            row.role(&RoleRef::from("order")),
            roles.get(&RoleRef::from("order")).cloned()
        );

        store.remove(edge);
        assert!(store.is_empty());
        assert!(store.resolve(edge).is_none());
    }

    #[test]
    fn by_role_default_scan_finds_matches() {
        let relation = RelationRef::from("shipping.Move");
        let mut store = GenericRelation::new(relation.clone());
        let roles_a = roles_for("o1");
        let roles_b = roles_for("o2");
        store.insert(edge_id(&relation, &roles_a), roles_a.clone());
        store.insert(edge_id(&relation, &roles_b), roles_b.clone());

        let want = roles_a.get(&RoleRef::from("order")).cloned().unwrap();
        let found: Vec<_> = store.by_role(&RoleRef::from("order"), &want).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].edge(), edge_id(&relation, &roles_a));
    }

    #[test]
    fn rows_iterate_in_canonical_edge_order() {
        let relation = RelationRef::from("shipping.Move");
        let mut store = GenericRelation::new(relation.clone());
        let roles_a = roles_for("o1");
        let roles_b = roles_for("o2");
        let edge_a = edge_id(&relation, &roles_a);
        let edge_b = edge_id(&relation, &roles_b);
        assert_ne!(edge_a, edge_b);

        // Insert whichever has the larger id first, to prove the store
        // doesn't just echo insertion order.
        let (bigger, bigger_roles, smaller, smaller_roles) = if edge_a > edge_b {
            (edge_a, roles_a, edge_b, roles_b)
        } else {
            (edge_b, roles_b, edge_a, roles_a)
        };
        store.insert(bigger, bigger_roles);
        store.insert(smaller, smaller_roles);

        let seen: Vec<EdgeId> = store.rows().map(|r| r.edge()).collect();
        assert_eq!(
            seen,
            vec![smaller, bigger],
            "rows() must yield canonical (EdgeId-sorted) order"
        );
    }
}
