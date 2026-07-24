//! Cross-crate guard for the **root-context digest invariant** (ADR-0001 §5.1).
//!
//! `brix-semantic`'s [`ContextId::root`] pins itself to a *frozen hex* golden
//! vector. This test closes the loop from the other side: it takes the **live**
//! `brix_ir::reflect::ScopeId::root()` — whatever `reflect` actually computes
//! today — and asserts its canonical identity is byte-identical to
//! `ContextId::root()`. So long as this holds, moving `brix.type`'s scope
//! identity from `ScopeId` to `ContextId` leaves every root-scoped `FactId`
//! unchanged, and the shadow-parity edifice survives the migration.
//!
//! `brix-semantic` depends only on `brix-canon`; this cross-crate check lives
//! here, where both `brix-ir` and `brix-semantic` are visible, so the substrate
//! crate stays dependency-clean.

use brix_canon::{CanonWriter, Canonical};
use brix_ir::reflect::ScopeId;
use brix_semantic::ContextId;

fn canon(value: &impl Canonical) -> Vec<u8> {
    let mut w = CanonWriter::new();
    value.canon_write(&mut w);
    w.finish()
}

#[test]
fn context_root_equals_live_reflect_scope_root() {
    // Both `ScopeId` and `ContextId` canon-write their 32-byte digest, so equal
    // canonical encodings ⇔ equal digests. Uses the live `ScopeId::root()`, not
    // a reproduction of its encoding — the real invariant, not a tautology.
    assert_eq!(
        canon(&ScopeId::root()),
        canon(&ContextId::root()),
        "ContextId::root() must reproduce reflect::ScopeId::root()'s digest \
         byte-for-byte (ADR-0001 §5.1) — otherwise migrating brix.type scope \
         identity to ContextId silently changes every root-scoped FactId"
    );
}
