//! [`RegimeId`] — the identity of a *realization regime* (ADR-0002 §6, §7):
//! literal equality, structural, entailment, probabilistic-threshold, … Names
//! *which* `ρ_w` interpretation a [`crate::Witness`] carries. `has-type` is
//! one regime among many, not a distinguished ontological relation
//! (ADR-0002 §1) — a realization judgment under the `brix.type` structural
//! regime and one under a probabilistic-threshold regime are both ordinary
//! `RegimeId`s, with no privileged case in this substrate.

use brix_canon::CanonWriter;

use crate::id::digest_id;

digest_id!(
    /// Identity of a realization regime — *which* `ρ_w` interpretation a
    /// witness carries (literal equality, structural, entailment,
    /// probabilistic-threshold, `brix.type`'s structural regime, …).
    RegimeId
);

impl RegimeId {
    /// A regime identified by a `name@version`-style string (e.g.
    /// `"brix.type.structural@0.1"`, `"literal-equality@1"`), mirroring
    /// [`crate::VerifierId::named`].
    pub fn named(name: &str) -> Self {
        let mut w = CanonWriter::new();
        w.write_str(name);
        RegimeId::from_canon(&w.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_names_give_distinct_regime_ids() {
        assert_ne!(
            RegimeId::named("literal-equality@1"),
            RegimeId::named("brix.type.structural@0.1")
        );
    }

    #[test]
    fn same_name_is_stable() {
        assert_eq!(
            RegimeId::named("literal-equality@1"),
            RegimeId::named("literal-equality@1")
        );
    }
}
