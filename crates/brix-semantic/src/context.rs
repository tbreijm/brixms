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
}
