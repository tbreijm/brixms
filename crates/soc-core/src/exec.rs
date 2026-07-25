//! The execution configuration `e = ⟨x, p, h⟩` (ADR-0002 §1 "Dynamics").
//!
//! `x` = world/configuration handle, `p` = policy handle, `h` = the current
//! history digest. Kept intentionally minimal: this build-order slice
//! (`Build_Plan_v3_SOC.md` Steps 2–3, E1/S2⋈E2) needs only enough of `e` to
//! drive naive candidate enumeration and the governance-conservation gate.
//! The calendar/commit fields ($K$, the priority-queue machinery) are Step 4
//! (S3⋈E4) — future work, not this crate's job yet.

use crate::intern::{Handle, Interner};
use brix_canon::Digest;
use brix_semantic::ContextId;

/// An execution configuration `e = ⟨x, p, h⟩`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ExecConfig {
    /// `x` — the world/configuration handle.
    pub world: Handle,
    /// `p` — the policy handle. Interpreted by the caller (which regimes are
    /// in play, which `Adm` gates them); this crate does not dereference it.
    pub policy: Handle,
    /// `h` — the current history digest (the running value of
    /// [`crate::history::History::digest`]).
    pub history: Digest,
}

impl ExecConfig {
    /// Construct an execution configuration from its three SOC components.
    pub fn new(world: Handle, policy: Handle, history: Digest) -> Self {
        ExecConfig {
            world,
            policy,
            history,
        }
    }
}

/// Intern a `brix-semantic` [`ContextId`] — the canonical "world" identity —
/// into a dense [`Handle`]: the boundary where a canonical digest becomes a
/// hot-loop handle (ADR-0002 §9.2 "Interning"). This is `soc-core`'s one real
/// touchpoint with `brix-semantic`'s canonical artifacts at this build-order
/// slice; later regimes (`Build_Plan_v3_SOC.md` Step 5) intern
/// `Witness`/`RegimeId` digests the same way once those SOC-specific
/// artifacts land in `brix-semantic` (ADR-0002 §6, not touched by this
/// crate).
pub fn intern_context(interner: &mut Interner, id: ContextId) -> Handle {
    interner.intern(id.digest())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_the_same_context_id_is_idempotent() {
        let mut i = Interner::new();
        let h1 = intern_context(&mut i, ContextId::root());
        let h2 = intern_context(&mut i, ContextId::root());
        assert_eq!(h1, h2);
    }

    #[test]
    fn distinct_contexts_get_distinct_handles() {
        let mut i = Interner::new();
        let root = intern_context(&mut i, ContextId::root());
        let other = intern_context(&mut i, ContextId::from_canon(b"other-world"));
        assert_ne!(root, other);
    }

    #[test]
    fn exec_config_equality_is_componentwise() {
        let mut i = Interner::new();
        let w = intern_context(&mut i, ContextId::root());
        let p = intern_context(&mut i, ContextId::from_canon(b"policy"));
        let h = Digest::of(brix_canon::Domain::Value, b"history");
        let a = ExecConfig::new(w, p, h);
        let b = ExecConfig::new(w, p, h);
        assert_eq!(a, b);
    }
}
