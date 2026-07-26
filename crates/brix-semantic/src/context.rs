//! [`ContextId`] — the content-addressed identity of an assumption context
//! (a "world"): `world/snapshot × program-revision × assumptions ×
//! semantic/checker-profile × resource-limits` (ADR-0001 §5.1).
//!
//! This slice lands the identity and its **root migration anchor**. The
//! concrete `Context` value (assumption trees, profile, limits) arrives with
//! the first scoped-checker slice (#53) that needs to *construct* non-root
//! contexts; a `ContextId` for any such value is `ContextId::of(&context)`.

use brix_canon::{CanonWriter, Canonical, Digest, Domain};

/// The legacy `brix_ir::reflect::ScopeId::root` canonical marker. The root
/// context adopts it **verbatim** — this string is ABI. See [`ContextId::root`].
const ROOT_CONTEXT_TAG: &str = "brix.ir.reflect.ScopeId.root";

/// Content-addressed identity of an assumption context. Distinct newtype over a
/// [`Digest`] so a `ContextId` cannot be confused with any other identity.
/// Digested under [`Domain::Value`] — the same domain `reflect`'s `ScopeId`
/// uses, which is what lets the root anchor (below) match byte-for-byte.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ContextId(pub Digest);

impl ContextId {
    /// Hash a canon-encoded context payload under the value domain.
    pub fn from_canon(payload: &[u8]) -> Self {
        ContextId(Digest::of(Domain::Value, payload))
    }

    /// The content-addressed id of any canonically-encodable context value.
    pub fn of(context: &impl Canonical) -> Self {
        let mut w = CanonWriter::new();
        context.canon_write(&mut w);
        ContextId::from_canon(&w.finish())
    }

    /// The well-known **root** context: root snapshot, empty assumptions,
    /// default profile and limits.
    ///
    /// **Migration anchor (ADR-0001 §5.1).** Its digest equals today's
    /// `brix_ir::reflect::ScopeId::root()` digest byte-for-byte, achieved by
    /// adopting that function's exact canonical encoding
    /// (`write_tag("brix.ir.reflect.ScopeId.root")` under `Domain::Value`). This
    /// is the hinge that lets `brix.type` move scope identity from `ScopeId` to
    /// `ContextId` **without changing any root-scoped `FactId`** — every
    /// `FactId` that embeds the root scope stays byte-identical, preserving the
    /// shadow-parity edifice. The equality is pinned two ways: a golden vector
    /// here (`root_context_id_matches_frozen_scope_root_digest`) and a
    /// cross-crate equality test against the live `ScopeId::root()`
    /// (`crates/brix-conformance`).
    pub fn root() -> Self {
        let mut w = CanonWriter::new();
        w.write_tag(ROOT_CONTEXT_TAG);
        ContextId::from_canon(&w.finish())
    }

    /// **Additive (Build_Plan_v3_SOC.md Step 5(b)): content-addressed context
    /// extension** — the first real assumption-scope machinery. Hashes this
    /// context's own digest together with `assumption`'s canonical bytes
    /// under a dedicated tag, producing a fresh child [`ContextId`]:
    ///
    /// - the same `(parent, assumption)` pair is stable across calls (same
    ///   inputs, same digest, every time);
    /// - two distinct assumptions extended from the same parent give distinct
    ///   children (`assumption` is folded into the hash, not discarded);
    /// - a child is never equal to its parent (root or otherwise) — the
    ///   dedicated tag plus the folded-in parent digest guarantees this
    ///   barring a hash collision.
    ///
    /// This is the **only** change this slice makes to [`ContextId`]:
    /// [`ContextId::root`]'s digest and canonical encoding are completely
    /// unchanged (this method only ever *adds* a new, distinct id reachable
    /// from an existing one — see `crates/soc-regimes/src/structural.rs`'s
    /// `ScopedWorldNonLeak` gate for the property this unlocks: a judgement
    /// derived under an extended context does not leak into its parent's
    /// projection, because the two contexts hash to different ids).
    pub fn extend(&self, assumption: &[u8]) -> ContextId {
        let mut w = CanonWriter::new();
        w.write_tag("brix.semantic.ContextId.extend");
        w.write_bytes(self.0.as_bytes());
        w.write_bytes(assumption);
        ContextId::from_canon(&w.finish())
    }

    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }

    /// Lowercase-hex rendering (diagnostics / `brix why`).
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

impl Canonical for ContextId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen digest of `reflect::ScopeId::root()` — captured independently
    /// (a compiled probe over `brix-canon`), so this golden vector cannot be
    /// vacuously satisfied by the code it guards. If `ContextId::root()` ever
    /// stops equalling this, root-scoped `FactId`s have silently changed.
    const ROOT_CONTEXT_DIGEST_HEX: &str =
        "a7d1f9a56c727ac00ad5dd6dd97d4af1e943df9f605efcc265248c2c7b355c5c";

    #[test]
    fn root_context_id_matches_frozen_scope_root_digest() {
        assert_eq!(ContextId::root().to_hex(), ROOT_CONTEXT_DIGEST_HEX);
    }

    #[test]
    fn root_is_stable_across_calls() {
        assert_eq!(ContextId::root(), ContextId::root());
    }

    #[test]
    fn distinct_payloads_give_distinct_ids() {
        let a = ContextId::from_canon(b"context-a");
        let b = ContextId::from_canon(b"context-b");
        assert_ne!(a, b);
        // …and neither collides with the root anchor.
        assert_ne!(a, ContextId::root());
        assert_ne!(b, ContextId::root());
    }

    // --- `ContextId::extend` (additive, Build_Plan_v3_SOC.md Step 5(b)) ---

    #[test]
    fn extend_is_stable_for_the_same_parent_and_assumption() {
        let a = ContextId::root().extend(b"x > 0");
        let b = ContextId::root().extend(b"x > 0");
        assert_eq!(a, b);
    }

    #[test]
    fn distinct_assumptions_from_the_same_parent_give_distinct_children() {
        let a = ContextId::root().extend(b"x > 0");
        let b = ContextId::root().extend(b"x < 0");
        assert_ne!(a, b);
    }

    #[test]
    fn a_child_is_never_equal_to_its_parent() {
        let root = ContextId::root();
        let child = root.extend(b"some assumption");
        assert_ne!(child, root);

        let other_parent = ContextId::from_canon(b"non-root-parent");
        let other_child = other_parent.extend(b"some assumption");
        assert_ne!(other_child, other_parent);
    }

    #[test]
    fn distinct_parents_give_distinct_children_for_the_same_assumption() {
        let root_child = ContextId::root().extend(b"a");
        let other_parent = ContextId::from_canon(b"other-parent");
        let other_child = other_parent.extend(b"a");
        assert_ne!(root_child, other_child);
    }

    #[test]
    fn extending_root_does_not_change_roots_own_digest() {
        // The additive change must not perturb `ContextId::root()` itself —
        // re-asserted here alongside the frozen golden vector test above.
        let before = ContextId::root();
        let _ = before.extend(b"anything");
        assert_eq!(ContextId::root(), before);
        assert_eq!(ContextId::root().to_hex(), ROOT_CONTEXT_DIGEST_HEX);
    }

    /// Golden vector, reproduced independently with a fresh `CanonWriter`
    /// (not via `ContextId::extend`), so it cannot be vacuously satisfied by
    /// the code it guards.
    #[test]
    fn golden_vector_root_extend_reproduced_independently() {
        let child = ContextId::root().extend(b"x > 0");

        let mut expected = CanonWriter::new();
        expected.write_tag("brix.semantic.ContextId.extend");
        expected.write_bytes(ContextId::root().digest().as_bytes());
        expected.write_bytes(b"x > 0");
        let expected_id = ContextId::from_canon(&expected.finish());

        assert_eq!(child, expected_id);
    }
}
