//! Public surface reference types and provisional engine-internal identities.
//!
//! Part III §3 names the public model's exact identity surface:
//! `NodeRef<E> EdgeRef<R> ClaimRef<R> SnapshotId DataRevision ProgramRevision`.
//! `SnapshotId` already lives in `brix-canon` (it is a canon-hashed id); the
//! rest are defined here because they either wrap a domain-typed marker
//! (`NodeRef`/`EdgeRef`/`ClaimRef`) or are not digests at all (`DataRevision`
//! is a monotonic counter, `ProgramRevision` a source+lockfile digest) —
//! neither belongs in the canon lane's frozen API.
//!
//! This module also carries a handful of **provisional** identity types
//! (`RelationRef`, `RoleRef`, `EntityRef`, `RuleRef`, `SiteId`,
//! `MatchDigest`, `SupportRef`, `TransactionId`) that Part III §11 and
//! Appendix A name but do not give normative formulas for — they are called
//! out there as "engine-internal; their properties are observable only
//! through sealed provenance relations." Once `brix-ir` lands, `RelationRef`/
//! `RoleRef`/`EntityRef`/`RuleRef` should become thin wrappers over the
//! compiler's own stable identities instead of raw names; the byte shape at
//! the canon boundary is not expected to change. See
//! `spec/errata/0001-matchdigest-supportref-formula.md` for the concrete
//! hash formulas this lane is proposing for `MatchDigest`/`SupportRef`,
//! needed so `brix-oracle` and `brix-rt` converge under differential fuzz
//! (conformance I.1, I.3).

use core::cmp::Ordering;
use core::fmt;
use core::marker::PhantomData;

use brix_canon::{CanonWriter, Canonical, Digest, Domain, EdgeId, NodeId};

/// A totally ordered, per-namespace committed revision counter (Part III
/// §4). Revision 0 is the empty namespace before any transaction commits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct DataRevision(pub u64);

impl DataRevision {
    /// The revision immediately following this one.
    pub fn next(self) -> Self {
        DataRevision(self.0 + 1)
    }
}

impl Canonical for DataRevision {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.0);
    }
}

/// Digest of canonical source + lockfile (Part XXVIII §28.1: "ProgramRevision
/// digests cover canonical source + lockfile, never binaries").
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ProgramRevision(pub Digest);

impl Canonical for ProgramRevision {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// Provisional transaction-intent identity (Part VII §2: "one intent
/// identity" across retries). Pending a normative formula from `brix-ir`,
/// constructed as `Digest::of(Domain::Value, intent_bytes)` by the
/// transaction pipeline (not yet in this Day-1 slice).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TransactionId(pub Digest);

impl Canonical for TransactionId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// A stable, compiler-assigned expression-site identity (Part III §9): "two
/// failing sites in one rule never collide."
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SiteId(pub u32);

impl Canonical for SiteId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.0 as u64);
    }
}

/// Digest of one rule match's variable bindings (Part III §9, §11:
/// `Support(edge, rule, match, atRevision)`, `RuleError(..., partialMatch:
/// MatchDigest, ...)`). See the module doc and
/// `spec/errata/0001-matchdigest-supportref-formula.md` for the proposed
/// construction: `Digest::of(Domain::Value, rule.canon_bytes() ++
/// sorted-bindings.canon_bytes())`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MatchDigest(pub Digest);

impl Canonical for MatchDigest {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

impl MatchDigest {
    /// Construct from already-canon-encoded, sorted binding bytes, per the
    /// proposed erratum ruling.
    pub fn of(rule: &RuleRef, sorted_bindings_canon: &[u8]) -> Self {
        let mut w = CanonWriter::new();
        rule.canon_write(&mut w);
        w.write_bytes(sorted_bindings_canon);
        MatchDigest(Digest::of(Domain::Value, &w.finish()))
    }
}

/// Opaque identity for one `(edge, rule, match)` support instance (Appendix
/// A: `KeyConflict(..., supports: Set<SupportRef>, ...)`). See
/// `spec/errata/0001-matchdigest-supportref-formula.md`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SupportRef(pub Digest);

impl Canonical for SupportRef {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

impl SupportRef {
    /// Construct the opaque support identity from its grounding triple.
    pub fn of(edge: EdgeId, rule: &RuleRef, m: MatchDigest) -> Self {
        let mut w = CanonWriter::new();
        w.write_bytes(edge.digest().as_bytes());
        rule.canon_write(&mut w);
        m.canon_write(&mut w);
        SupportRef(Digest::of(Domain::Value, &w.finish()))
    }
}

/// A stable name-keyed identity, the shape shared by [`RelationRef`],
/// [`RoleRef`], [`EntityRef`], and [`RuleRef`] until those become
/// digest-backed compiler identities from `brix-ir`. Ordering is `String`
/// ordering, which for NFC-normalized ASCII/BMP identifiers coincides with
/// canonical byte order (Appendix G); full Unicode-identifier NFC folding is
/// tracked as a canon-lane freeze blocker (`DEPS.md`), not this lane's to fix.
macro_rules! named_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub String);

        impl $name {
            /// Borrow the underlying name.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                $name(s.to_string())
            }
        }

        impl Canonical for $name {
            fn canon_write(&self, w: &mut CanonWriter) {
                w.write_ident(&self.0);
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

named_id!(
    /// A relation's stable name (`rel`/`entity`/sealed schema name),
    /// namespaced as the source declares it (e.g. `"shipping.Move"`).
    RelationRef
);
named_id!(
    /// A role name within a relation's declared role set.
    RoleRef
);
named_id!(
    /// An entity type's stable name.
    EntityRef
);
named_id!(
    /// A rule's stable name, provisional pending `brix-ir` `RuleRef`.
    RuleRef
);

/// A typed reference to a node of entity type `E` (Part III §3). `E` is a
/// zero-sized marker — generated code will bind it to the entity's own type;
/// today it defaults to `()` for untyped/reflection use. Manually
/// implemented (rather than derived) so the marker never imposes spurious
/// trait bounds on `E`.
pub struct NodeRef<E = ()> {
    id: NodeId,
    _marker: PhantomData<fn() -> E>,
}

impl<E> NodeRef<E> {
    /// Wrap an already-resolved [`NodeId`].
    pub fn new(id: NodeId) -> Self {
        NodeRef {
            id,
            _marker: PhantomData,
        }
    }

    /// The underlying canon identity.
    pub fn id(&self) -> NodeId {
        self.id
    }
}

impl<E> Clone for NodeRef<E> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<E> Copy for NodeRef<E> {}
impl<E> PartialEq for NodeRef<E> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<E> Eq for NodeRef<E> {}
impl<E> PartialOrd for NodeRef<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<E> Ord for NodeRef<E> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}
impl<E> fmt::Debug for NodeRef<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NodeRef").field(&self.id).finish()
    }
}
impl<E> Canonical for NodeRef<E> {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.id.digest().as_bytes());
    }
}

/// A typed reference to an edge of relation type `R` (Part III §3). Same
/// marker-generic shape as [`NodeRef`].
pub struct EdgeRef<R = ()> {
    id: EdgeId,
    _marker: PhantomData<fn() -> R>,
}

impl<R> EdgeRef<R> {
    /// Wrap an already-resolved [`EdgeId`].
    pub fn new(id: EdgeId) -> Self {
        EdgeRef {
            id,
            _marker: PhantomData,
        }
    }

    /// The underlying canon identity.
    pub fn id(&self) -> EdgeId {
        self.id
    }
}

impl<R> Clone for EdgeRef<R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R> Copy for EdgeRef<R> {}
impl<R> PartialEq for EdgeRef<R> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<R> Eq for EdgeRef<R> {}
impl<R> PartialOrd for EdgeRef<R> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<R> Ord for EdgeRef<R> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}
impl<R> fmt::Debug for EdgeRef<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EdgeRef").field(&self.id).finish()
    }
}
impl<R> Canonical for EdgeRef<R> {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.id.digest().as_bytes());
    }
}

/// A retry-stable reference to one source's ground claim (Part III §3, Part
/// VII §2). `retract` consumes a `ClaimRef` in the BrixMS source language —
/// affinity is enforced there by `brix-ir`'s type system, not by this Rust
/// representation, which is an ordinary `Copy` value carrier.
pub struct ClaimRef<R = ()> {
    id: brix_canon::ClaimId,
    _marker: PhantomData<fn() -> R>,
}

impl<R> ClaimRef<R> {
    /// Wrap an already-resolved `ClaimId`.
    pub fn new(id: brix_canon::ClaimId) -> Self {
        ClaimRef {
            id,
            _marker: PhantomData,
        }
    }

    /// The underlying canon identity.
    pub fn id(&self) -> brix_canon::ClaimId {
        self.id
    }
}

impl<R> Clone for ClaimRef<R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R> Copy for ClaimRef<R> {}
impl<R> PartialEq for ClaimRef<R> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<R> Eq for ClaimRef<R> {}
impl<R> PartialOrd for ClaimRef<R> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<R> Ord for ClaimRef<R> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}
impl<R> fmt::Debug for ClaimRef<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ClaimRef").field(&self.id).finish()
    }
}
impl<R> Canonical for ClaimRef<R> {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.id.digest().as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_revision_orders_numerically() {
        assert!(DataRevision(1) < DataRevision(2));
        assert_eq!(DataRevision(4), DataRevision(3).next());
    }

    #[test]
    fn named_id_display_roundtrips() {
        let r = RelationRef::from("shipping.Move");
        assert_eq!(r.as_str(), "shipping.Move");
        assert_eq!(format!("{r}"), "shipping.Move");
    }

    #[test]
    fn node_ref_marker_does_not_require_bounds() {
        struct Order; // no derives at all
        let a = NodeRef::<Order>::new(NodeId::from_canon(b"a"));
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn support_ref_is_deterministic() {
        let rule = RuleRef::from("FromComputed");
        let edge = EdgeId::from_canon(b"edge");
        let m = MatchDigest::of(&rule, b"bindings");
        let s1 = SupportRef::of(edge, &rule, m);
        let s2 = SupportRef::of(edge, &rule, m);
        assert_eq!(s1, s2);
    }
}
