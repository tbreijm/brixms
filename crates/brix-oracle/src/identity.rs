//! Identity computation (Part III §3, Appendix G): turning a `Row` plus its
//! `RelationDef` into the hashes the kernel actually keys on.
//!
//! ```text
//! NodeId  = Hash(entity compatibility domain, canonical key encoding)
//! EdgeId  = Hash(relation compatibility domain, canonical role tuple)
//! ```
//!
//! brix-canon's `Digest::of` already separates the `Node`/`Edge` namespaces
//! globally (its `Domain` enum). The *per-relation* "compatibility domain"
//! Appendix G also requires (so `Order(ref: "1")` and `Client(ref: "1")`
//! never collide) is folded in here by writing the relation name as the
//! first bytes of the hashed payload — `RelationDef::identity_payload`.
//!
//! `key_bytes` is a third, distinct notion used only for **grouping**:
//! Appendix G computes identity over the *whole* row (Ground/State/Event/
//! Derived) or the key-only fields (Entity); key-conflict detection (Part
//! III §8) always needs to group by key-only bytes regardless of kind, so
//! it is exposed unconditionally rather than folded into `EdgeId`.

use brix_canon::{CanonWriter, Canonical, Domain, EdgeId, NodeId};

use crate::program::{RelKind, RelationDef};
use crate::row::{CanonBytes, Row};

impl RelationDef {
    /// `NodeId` for an `Entity`-kind row: hash of (relation name, key
    /// fields in declaration order). Used for both transaction-`ensure`d
    /// entities and rule-derived `keyed by (...)` nodes — the hash is the
    /// Skolem identity in the latter case (Part III §3).
    pub fn node_id(&self, row: &Row) -> NodeId {
        debug_assert_eq!(self.kind, RelKind::Entity);
        let mut w = CanonWriter::new();
        w.write_tag(&self.name);
        for k in &self.key {
            let v = row.get(k).expect("row missing declared key role");
            v.canon_write(&mut w);
        }
        NodeId::from_canon(&w.finish())
    }

    /// `EdgeId` for a `Ground`/`State`/`Event`/`Derived` row: hash of
    /// (relation name, every role sorted by role name — `Row`'s `BTreeMap`
    /// already sorts them).
    pub fn edge_id(&self, row: &Row) -> EdgeId {
        debug_assert_ne!(self.kind, RelKind::Entity);
        let mut w = CanonWriter::new();
        w.write_tag(&self.name);
        row.canon_write(&mut w);
        EdgeId::from_canon(&w.finish())
    }

    /// Uniform entry point: `NodeId` and `EdgeId` share a representation
    /// (`brix_canon::Digest`) but distinct types — this collapses to the
    /// digest so callers that only need identity-as-bytes (dumps,
    /// `EdgeRef`-typed `Value`s) don't need to match on `kind` themselves.
    pub fn digest(&self, row: &Row) -> brix_canon::Digest {
        match self.kind {
            RelKind::Entity => self.node_id(row).digest(),
            _ => self.edge_id(row).digest(),
        }
    }

    /// The value a pattern clause binds to (`e @ R(...)` / `x: Entity {
    /// ... }`, Part IV §3): `Value::Node` for `Entity`-kind relations,
    /// `Value::Edge` otherwise.
    pub fn ref_value(&self, row: &Row) -> crate::value::Value {
        match self.kind {
            RelKind::Entity => crate::value::Value::Node(self.node_id(row)),
            _ => crate::value::Value::Edge(self.edge_id(row)),
        }
    }

    /// Canonical bytes of the key-only sub-row, prefixed with the relation
    /// name — the grouping key for per-kind conflict rules (Part III §8)
    /// and for `set`'s "the version the transaction read" (Part VII §2).
    pub fn key_bytes(&self, row: &Row) -> CanonBytes {
        let mut w = CanonWriter::new();
        w.write_tag(&self.name);
        for k in &self.key {
            let v = row.get(k).expect("row missing declared key role");
            v.canon_write(&mut w);
        }
        w.finish()
    }

    /// A row's own canonical bytes, domain-tagged — the byte string that
    /// gets digested into an `EdgeId`/`NodeId`, and the same digest domain
    /// used to key `Support`/`Claim`/etc. that reference this relation.
    pub fn domain(&self) -> Domain {
        match self.kind {
            RelKind::Entity => Domain::Node,
            _ => Domain::Edge,
        }
    }
}
