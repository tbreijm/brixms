//! [`Quiescent`] — the `Quiescent(x, p, 𝑅, A)` proposition kind (ADR-0014
//! §6.4, ⟨D-QP⟩): the canonical statement that **no admissible candidate
//! exists** at world `x` under policy `p`, for the ordered regime set `𝑅` and
//! the admissibility policy `A`.
//!
//! This is the extensional `F_O` fact ADR-0002 §5.2 says the kernel certifies —
//! the `1` summand, stated as a first-class proposition rather than as an
//! engine-internal enum variant. Placing it here (not in `soc-core`) is what
//! lets the epistemic lattice ever apply to quiescence: a `Derived` quiescence
//! judgement published by the settlement kernel is the *same proposition* a
//! later `Audited` or `Proven` route would have to reach.
//!
//! Like [`crate::Realizes`], it is a plain [`Canonical`] value, so its
//! [`PropositionId`] comes from [`PropositionId::of`] verbatim — no separate
//! statement vocabulary, no new enum ordinal anywhere, nothing to reorder.
//!
//! **Both regime-set and admissibility identities are part of the statement.**
//! "Nothing is admissible here" is meaningless without saying *under which
//! realization relation* and *under which governance predicate*: tightening
//! `Adm` can make a non-quiescent configuration quiescent (ADR-0002 §5.5's
//! governance-conservation law is exactly this monotonicity), so a quiescence
//! claim that omitted `A` would be silently revision-dependent.

use brix_canon::{CanonWriter, Canonical, Digest};

use crate::{ConfigId, PropositionId};

/// The statement "the admissible frontier at `world` under `policy` is empty
/// for regime set `regimes` and admissibility policy `adm`".
///
/// Frozen field order (`world`, `policy`, `regimes`, `adm`) is ABI.
///
/// `regimes` and `adm` are opaque caller-supplied canonical identities.
/// `brix-semantic` holds no regime or governance vocabulary and deliberately
/// does not try to validate them — it records *which* ones the claim was made
/// against, so two claims under different governance can never collide.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Quiescent {
    /// The configuration at which the frontier was found empty.
    pub world: ConfigId,
    /// The policy configuration in force.
    pub policy: ConfigId,
    /// Canonical identity of the ordered regime set the enumeration ran over.
    pub regimes: Digest,
    /// Canonical identity of the admissibility policy that filtered it.
    pub adm: Digest,
}

impl Quiescent {
    /// State quiescence at `world` under `policy`, `regimes`, and `adm`.
    pub fn new(world: ConfigId, policy: ConfigId, regimes: Digest, adm: Digest) -> Self {
        Quiescent {
            world,
            policy,
            regimes,
            adm,
        }
    }

    /// The `PropositionId` of this statement — reuses [`PropositionId::of`]
    /// verbatim, the same constructor every other canonical domain statement
    /// uses (ADR-0001 §5.2).
    pub fn proposition_id(&self) -> PropositionId {
        PropositionId::of(self)
    }
}

impl Canonical for Quiescent {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI: world, policy, regimes, adm.
        self.world.canon_write(w);
        self.policy.canon_write(w);
        w.write_bytes(self.regimes.as_bytes());
        w.write_bytes(self.adm.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_canon::Domain;

    fn config(seed: &[u8]) -> ConfigId {
        ConfigId::from_canon(seed)
    }

    fn digest(seed: &[u8]) -> Digest {
        Digest::of(Domain::Value, seed)
    }

    fn sample() -> Quiescent {
        Quiescent::new(
            config(b"world"),
            config(b"policy"),
            digest(b"regimes"),
            digest(b"adm"),
        )
    }

    /// Golden reproduction with a fresh `CanonWriter`, not via
    /// `Quiescent::canon_write`, so the id cannot be vacuously satisfied by the
    /// code it guards. Mirrors `realizes.rs`'s equivalent test.
    #[test]
    fn quiescent_proposition_id_matches_independent_reproduction() {
        let q = sample();

        let got = q.proposition_id();

        let mut w = CanonWriter::new();
        w.write_bytes(q.world.digest().as_bytes());
        w.write_bytes(q.policy.digest().as_bytes());
        w.write_bytes(q.regimes.as_bytes());
        w.write_bytes(q.adm.as_bytes());
        let expected = PropositionId::from_canon(&w.finish());

        assert_eq!(got, expected);
    }

    #[test]
    fn every_field_is_load_bearing_for_identity() {
        let base = sample();
        let other_world = Quiescent {
            world: config(b"other"),
            ..base
        };
        let other_policy = Quiescent {
            policy: config(b"other"),
            ..base
        };
        let other_regimes = Quiescent {
            regimes: digest(b"other"),
            ..base
        };
        let other_adm = Quiescent {
            adm: digest(b"other"),
            ..base
        };

        let ids = [
            base.proposition_id(),
            other_world.proposition_id(),
            other_policy.proposition_id(),
            other_regimes.proposition_id(),
            other_adm.proposition_id(),
        ];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "quiescent {i} and {j} collided");
            }
        }
    }

    /// Quiescence and realization are different statements even when they are
    /// built from the same digests — the whole point of a distinct proposition
    /// shape rather than a reused one.
    #[test]
    fn quiescence_never_collides_with_a_realization_statement() {
        use crate::{Realizes, RegimeId, Witness};

        let x = config(b"world");
        let y = config(b"policy");
        let witness = Witness::new(x, y, RegimeId::named("r")).id();

        let q = Quiescent::new(x, y, digest(b"regimes"), digest(b"adm"));
        let r = Realizes::new(witness, x, y);

        assert_ne!(q.proposition_id(), r.proposition_id());
    }

    #[test]
    fn proposition_id_is_deterministic() {
        assert_eq!(sample().proposition_id(), sample().proposition_id());
    }
}
