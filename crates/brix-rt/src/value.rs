//! Generic role values and role tuples — the dynamically-typed row shape
//! used by reflection, tooling, the delta ABI, and this crate's reference
//! `RelationStore` (Ring0 §1.7). Generated (tier A) stores never construct
//! these: their columns are statically typed Rust and they encode straight
//! to canon bytes. `EdgeRoleTuple` exists for exactly the callers named in
//! the brief — reflection over `meta.*`, cross-relation `why`, tier-B WASM,
//! BGIF export, path expressions, Studio, and the WASM/WIT delta-ABI
//! boundary, where no generated schema is available to lean on.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical, NodeId};

use crate::ids::RoleRef;

/// Already-canon-encoded bytes for one immutable value (Part III §1: a role
/// binds "to a node or an immutable value"). Until `brix-ir`'s value type
/// lands, this is the byte-identical stand-in: everything serializes through
/// `brix-canon` regardless, so the wire shape does not change when the typed
/// value enum arrives — only this crate's in-memory representation would.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct CanonBytes(pub Vec<u8>);

impl Canonical for CanonBytes {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(&self.0);
    }
}

/// A single role's binding: a node reference or an immutable value.
///
/// The leading tag byte (0 = `Node`, 1 = `Value`) is a reflection-path
/// concern only — generated stores never need it because a role's node-vs-
/// value shape is fixed by the relation's declaration (App. G "relation
/// tuples: ... roles sorted by role name" assumes the reader already knows
/// each role's static shape). Here nothing but the bytes is available, so
/// the tag keeps the two cases from ever hashing to the same payload.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RoleValue {
    /// The role is bound to a node.
    Node(NodeId),
    /// The role is bound to an immutable value.
    Value(CanonBytes),
}

impl Canonical for RoleValue {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            RoleValue::Node(id) => {
                w.write_uint(0);
                w.write_bytes(id.digest().as_bytes());
            }
            RoleValue::Value(bytes) => {
                w.write_uint(1);
                bytes.canon_write(w);
            }
        }
    }
}

/// A relation tuple's role bindings, sorted by canonical role-name bytes
/// (Appendix G: "relation tuples: relation compatibility domain digest +
/// roles sorted by role name"; "records/rows: fields sorted by canonical
/// field-name bytes, each name-prefixed"). Backed by `BTreeMap`, so
/// insertion order never matters and iteration order is always the
/// canonical order — no separate sort step, no `HashMap` (Ring0 §0).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct EdgeRoleTuple(BTreeMap<RoleRef, RoleValue>);

impl EdgeRoleTuple {
    /// An empty role tuple.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `role` to `value`, replacing any prior binding. Builder-style.
    pub fn with(mut self, role: RoleRef, value: RoleValue) -> Self {
        self.0.insert(role, value);
        self
    }

    /// Bind `role` to `value` in place.
    pub fn set(&mut self, role: RoleRef, value: RoleValue) {
        self.0.insert(role, value);
    }

    /// Look up one role's binding.
    pub fn get(&self, role: &RoleRef) -> Option<&RoleValue> {
        self.0.get(role)
    }

    /// Iterate bindings in canonical (role-name-sorted) order.
    pub fn iter(&self) -> impl Iterator<Item = (&RoleRef, &RoleValue)> {
        self.0.iter()
    }

    /// Number of bound roles.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no roles are bound.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<(RoleRef, RoleValue)> for EdgeRoleTuple {
    fn from_iter<I: IntoIterator<Item = (RoleRef, RoleValue)>>(iter: I) -> Self {
        EdgeRoleTuple(iter.into_iter().collect())
    }
}

impl Canonical for EdgeRoleTuple {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.0.len() as u64);
        for (role, value) in self.0.iter() {
            w.write_ident(role.as_str());
            value.canon_write(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_canon::EdgeId;

    #[test]
    fn role_tuple_orders_by_role_name_regardless_of_insertion() {
        let mut a = EdgeRoleTuple::new();
        a.set(
            RoleRef::from("vehicle"),
            RoleValue::Node(NodeId::from_canon(b"v")),
        );
        a.set(
            RoleRef::from("order"),
            RoleValue::Node(NodeId::from_canon(b"o")),
        );

        let mut b = EdgeRoleTuple::new();
        b.set(
            RoleRef::from("order"),
            RoleValue::Node(NodeId::from_canon(b"o")),
        );
        b.set(
            RoleRef::from("vehicle"),
            RoleValue::Node(NodeId::from_canon(b"v")),
        );

        assert_eq!(a.canon_bytes(), b.canon_bytes());
        let names: Vec<&str> = a.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(names, vec!["order", "vehicle"]);
    }

    #[test]
    fn node_and_value_roles_never_collide() {
        let node_role = RoleValue::Node(NodeId::from_canon(b"x"));
        let value_role = RoleValue::Value(CanonBytes(b"x".to_vec()));
        assert_ne!(node_role.canon_bytes(), value_role.canon_bytes());
    }

    #[test]
    fn edge_id_smoke_uses_role_tuple_bytes() {
        let tuple = EdgeRoleTuple::new().with(
            RoleRef::from("order"),
            RoleValue::Node(NodeId::from_canon(b"o")),
        );
        let id_a = EdgeId::from_canon(&tuple.canon_bytes());
        let id_b = EdgeId::from_canon(&tuple.canon_bytes());
        assert_eq!(id_a, id_b);
    }
}
