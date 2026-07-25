//! [`ConfigId`] — the content-addressed identity of a *configuration*, the
//! object of the SOC category (ADR-0002 §6).
//!
//! A configuration is *any* canonical value — there is no separate
//! configuration value-kind to define here. Per ADR-0002 §6: "Configurations
//! reuse existing canonical value identities; a witness is a typed
//! correspondence, not a new value kind." `ConfigId` therefore does **not**
//! invent a parallel identity scheme: it is exactly the
//! [`brix_canon::Domain::Value`] digest of the configuration's canonical
//! encoding — the same digest any other canonically-encoded value gets in
//! this crate, wrapped in a distinct newtype (via [`crate::id::digest_id`])
//! purely for type-safety, so a `ConfigId` cannot be confused with a
//! `WitnessId`, `RegimeId`, or any other identity (the same rationale
//! [`crate::id`] gives for every id in this crate).
//!
//! Concretely: for any `v: impl Canonical`, `ConfigId::of(&v)` is
//! byte-for-byte `brix_canon::Digest::of(Domain::Value, v.canon_bytes())` —
//! there is no configuration-specific hashing rule, salt, or tag layered on
//! top (see the `config_id_reuses_the_value_digest_no_parallel_scheme` test
//! below, which reproduces it independently).

use crate::id::digest_id;

digest_id!(
    /// Content-addressed identity of a configuration (an object of the SOC
    /// category). Reuses the canonical value-digest identity verbatim — see
    /// the module doc; this is a type-safety newtype, not a new value kind.
    ConfigId
);

#[cfg(test)]
mod tests {
    use super::*;
    use brix_canon::{CanonWriter, Canonical, Digest, Domain};

    /// A configuration is *any* canonical value; pick something arbitrary and
    /// unrelated to `brix-semantic`'s own id types to make the point.
    struct Toy(u64);
    impl Canonical for Toy {
        fn canon_write(&self, w: &mut CanonWriter) {
            w.write_uint(self.0);
        }
    }

    #[test]
    fn config_id_reuses_the_value_digest_no_parallel_scheme() {
        let v = Toy(42);
        let via_config_id = ConfigId::of(&v);

        // Independent reproduction: the plain Domain::Value digest of the
        // same canonical bytes, computed with a fresh CanonWriter and no
        // ConfigId-specific step at all.
        let mut w = CanonWriter::new();
        v.canon_write(&mut w);
        let independent = Digest::of(Domain::Value, &w.finish());

        assert_eq!(
            via_config_id.digest(),
            independent,
            "ConfigId must be exactly the canonical value digest, not a parallel scheme"
        );
    }

    #[test]
    fn distinct_values_give_distinct_config_ids() {
        assert_ne!(ConfigId::of(&Toy(1)), ConfigId::of(&Toy(2)));
    }

    #[test]
    fn same_value_is_stable() {
        assert_eq!(ConfigId::of(&Toy(7)), ConfigId::of(&Toy(7)));
    }
}
