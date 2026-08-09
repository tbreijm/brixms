//! Generator registry `𝒢` (ADR-0002 §6, §7): the specified class of primitive
//! **logged** settlement witnesses that generates the tight subcategory `𝒦`
//! (SOC "obligation: tight, generated settlement subcategory", PD-1). Each
//! generator has a canonical identity; **membership is data, not
//! convention** — a [`GeneratorRegistry`] is itself a content-addressed
//! canonical value, checkable, not a hardcoded allow-list living in review
//! comments.

use std::collections::BTreeSet;

use brix_canon::{CanonWriter, Canonical};

use crate::id::digest_id;
use crate::WitnessId;

digest_id!(
    /// Identity of a single primitive logged settlement witness — a member
    /// of `𝒢`. Generators compose to the tight subcategory `𝒦` (PD-1); every
    /// committed witness's [`crate::Decomposition`] cites `GeneratorId`s
    /// drawn from a [`GeneratorRegistry`].
    GeneratorId
);

impl GeneratorId {
    /// A generator identified by a `name@version`-style string, mirroring
    /// [`crate::VerifierId::named`] / [`crate::RegimeId::named`].
    pub fn named(name: &str) -> Self {
        let mut w = CanonWriter::new();
        w.write_str(name);
        GeneratorId::from_canon(&w.finish())
    }

    /// The primitive witness identity of this generator (equal to its underlying digest).
    pub fn witness_id(&self) -> WitnessId {
        WitnessId(self.0)
    }
}

impl From<GeneratorId> for WitnessId {
    /// A generator is a primitive witness — its witness identity is its own underlying digest.
    fn from(gid: GeneratorId) -> Self {
        WitnessId(gid.digest())
    }
}

/// The specified class `𝒢` of primitive logged settlement witnesses, as a
/// deterministic set of [`GeneratorId`]s. `BTreeSet`, never a hash-ordered
/// set (workspace policy, `clippy.toml`: `HashSet`'s iteration order is
/// nondeterministic and this is a semantic path), so registry construction
/// and canonical encoding agree on the same total order.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct GeneratorRegistry(BTreeSet<GeneratorId>);

impl GeneratorRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a generator as a member of `𝒢`. Returns `true` if it was
    /// newly inserted (`false` if already a member).
    pub fn insert(&mut self, id: GeneratorId) -> bool {
        self.0.insert(id)
    }

    /// Whether `id` is a member of `𝒢` — membership is data, checked here,
    /// not a review-time convention.
    pub fn contains(&self, id: &GeneratorId) -> bool {
        self.0.contains(id)
    }

    /// Number of registered generators.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the registry has no members.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The members of `𝒢`, in canonical (sorted) order. Needed to compare a
    /// registry against a declared semantics manifest for *exact* agreement
    /// (ADR-0020 D2) — containment is not enough, so both directions of the
    /// difference must be computable.
    pub fn iter(&self) -> impl Iterator<Item = &GeneratorId> {
        self.0.iter()
    }

    /// The content-addressed id of the whole registry — order-independent
    /// (canonical `Set` encoding, sorted + deduplicated), so two registries
    /// built by inserting the same generators in different orders are the
    /// same registry.
    pub fn id(&self) -> GeneratorRegistryId {
        GeneratorRegistryId::of(self)
    }
}

impl Canonical for GeneratorRegistry {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_set(self.0.iter().map(|g| g.canon_bytes()));
    }
}

digest_id!(
    /// Content-addressed identity of a whole [`GeneratorRegistry`] — the
    /// specified class `𝒢` as data.
    GeneratorRegistryId
);

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(names: &[&str]) -> GeneratorRegistry {
        let mut r = GeneratorRegistry::new();
        for n in names {
            r.insert(GeneratorId::named(n));
        }
        r
    }

    #[test]
    fn contains_reflects_membership() {
        let r = registry(&["step@1", "merge@1"]);
        assert!(r.contains(&GeneratorId::named("step@1")));
        assert!(!r.contains(&GeneratorId::named("other@1")));
        assert_eq!(r.len(), 2);
        assert!(!r.is_empty());
        assert!(GeneratorRegistry::new().is_empty());
    }

    #[test]
    fn registry_id_is_order_independent() {
        let a = registry(&["step@1", "merge@1", "split@1"]);
        let b = registry(&["split@1", "step@1", "merge@1"]);
        assert_eq!(a.id(), b.id());
    }

    /// Golden vector for a small fixed registry, reproduced independently
    /// with a fresh `CanonWriter` (not via `GeneratorRegistry::canon_write`).
    #[test]
    fn golden_vector_small_fixed_registry() {
        let r = registry(&["step@1", "merge@1"]);

        let mut got = CanonWriter::new();
        r.canon_write(&mut got);

        // Independent reproduction: the canonical Set encoding is entries
        // sorted by canonical element bytes, deduplicated, count-prefixed
        // (brix-canon's write_set).
        let step = GeneratorId::named("step@1");
        let merge = GeneratorId::named("merge@1");
        let mut expected = CanonWriter::new();
        expected.write_set([step.canon_bytes(), merge.canon_bytes()]);

        assert_eq!(got.finish(), expected.finish());
    }

    #[test]
    fn distinct_registries_give_distinct_ids() {
        let a = registry(&["step@1"]);
        let b = registry(&["step@1", "merge@1"]);
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn duplicate_insert_does_not_change_the_id() {
        let mut a = registry(&["step@1"]);
        let before = a.id();
        assert!(!a.insert(GeneratorId::named("step@1")));
        assert_eq!(a.id(), before);
    }
}
