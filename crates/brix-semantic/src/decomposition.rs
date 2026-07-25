//! [`Decomposition`] — evidence artifact recording a finite factorization
//! `k = g_n ∘ ⋯ ∘ g_1` with `g_i ∈ 𝒢` and the intermediate configurations
//! `x_0, …, x_n` (ADR-0002 §6, SOC PD-1).
//!
//! **The load-bearing distinction (ADR-0002 §5.1, §5.2): recorded-unverified
//! vs replay-verified MUST be distinguishable in the type/encoding, not just
//! by convention.** [`DecompVerification`] is part of the canonical encoding,
//! so a [`DecompVerification::Recorded`] decomposition and a
//! [`DecompVerification::ReplayVerified`] one built from *identical*
//! generators/configs have **different** [`DecompositionId`]s — they are
//! different evidence, because they *are* different evidence: a claim to
//! have recorded a chain is not a claim to have replayed and verified it.
//!
//! Only the [`DecompVerification::ReplayVerified`] form supports an
//! `Audited` judgement (ADR-0002 §4 — sole authority: the audit-factorization
//! checker, §4.1) and may cross an `elaboration-boundary`
//! ([`crate::EdgeKind::ElaborationBoundary`], ADR-0002 §5.2). The
//! [`DecompVerification::Recorded`] form is the hot loop's unverified
//! record, supporting only `Derived` (ADR-0002 §5.1: "the hot loop records a
//! compact support record plus the (unverified) `Decomposition`"). Its
//! durability classification as "closed over context" (§2, §6) applies only
//! to the **verified** form.

use brix_canon::{CanonWriter, Canonical};

use crate::id::digest_id;
use crate::{ConfigId, GeneratorId};

/// Whether a [`Decomposition`]'s factorization has merely been *recorded* by
/// the hot loop, or *replayed and verified* by the audit-factorization
/// checker. Canonical ABI ordinals — append-only, never reordered — because
/// this distinction is part of the artifact's identity (see module doc):
/// unlike most enums in this crate, the ordinal here is not incidental, it
/// *is* the recorded-vs-verified boundary.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DecompVerification {
    /// The hot loop recorded this factorization; it has not been replayed.
    /// Supports only a `Derived` judgement (ADR-0002 §5.1).
    Recorded,
    /// The audit-factorization checker replayed this factorization and
    /// verified it composes exactly (ADR-0002 §4.1). Supports an `Audited`
    /// judgement and may cross an `elaboration-boundary` edge.
    ReplayVerified,
}

impl DecompVerification {
    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(self) -> u64 {
        match self {
            DecompVerification::Recorded => 0,
            DecompVerification::ReplayVerified => 1,
        }
    }
}

impl Canonical for DecompVerification {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |_| {});
    }
}

/// A [`Decomposition`] fails construction if the intermediate-configuration
/// chain doesn't match the generator chain: `n` generators need exactly
/// `n + 1` configurations (`x_0, …, x_n`). This type is Rust-side validation
/// only — it is never canonically encoded or hashed (a rejected chain never
/// becomes an artifact), so it carries no ABI ordinal: nothing depends on its
/// wire representation because it has none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecompositionError {
    /// `configs.len() != generators.len() + 1`.
    ChainLengthMismatch { generators: usize, configs: usize },
}

/// A finite factorization `k = g_n ∘ ⋯ ∘ g_1`, `g_i ∈ 𝒢`, with the
/// intermediate configurations `x_0, …, x_n`: `configs[0]` is the source of
/// `g_1`, `configs[i]` is both the target of `g_i` and the source of
/// `g_{i+1}`, and `configs[n]` is the target of `g_n`. The invariant
/// `configs.len() == generators.len() + 1` is enforced at construction (see
/// [`DecompositionError`]) — there is no way to build a `Decomposition` with
/// a mismatched chain.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Decomposition {
    pub generators: Vec<GeneratorId>,
    pub configs: Vec<ConfigId>,
    pub verification: DecompVerification,
}

impl Decomposition {
    fn build(
        generators: Vec<GeneratorId>,
        configs: Vec<ConfigId>,
        verification: DecompVerification,
    ) -> Result<Self, DecompositionError> {
        if configs.len() != generators.len() + 1 {
            return Err(DecompositionError::ChainLengthMismatch {
                generators: generators.len(),
                configs: configs.len(),
            });
        }
        Ok(Decomposition {
            generators,
            configs,
            verification,
        })
    }

    /// Construct a **recorded, unverified** decomposition — the hot loop's
    /// compact record of a committed step's factorization (ADR-0002 §5.1).
    /// Supports only `Derived`.
    pub fn recorded(
        generators: Vec<GeneratorId>,
        configs: Vec<ConfigId>,
    ) -> Result<Self, DecompositionError> {
        Self::build(generators, configs, DecompVerification::Recorded)
    }

    /// Construct a **replay-verified** decomposition — the audit-factorization
    /// checker's result after replaying a recorded chain and verifying exact
    /// relational composition (ADR-0002 §4.1). Supports `Audited` and may
    /// cross an `elaboration-boundary`.
    pub fn replay_verified(
        generators: Vec<GeneratorId>,
        configs: Vec<ConfigId>,
    ) -> Result<Self, DecompositionError> {
        Self::build(generators, configs, DecompVerification::ReplayVerified)
    }

    /// Whether this decomposition has been replayed and verified — the only
    /// form that supports `Audited` / may cross an `elaboration-boundary`.
    pub const fn is_replay_verified(&self) -> bool {
        matches!(self.verification, DecompVerification::ReplayVerified)
    }

    /// The content-addressed id of this decomposition.
    pub fn id(&self) -> DecompositionId {
        DecompositionId::of(self)
    }
}

impl Canonical for Decomposition {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI: generators, configs, verification. The
        // verification tag is part of the encoding by design (module doc):
        // recorded vs replay-verified over identical data MUST NOT share an
        // id.
        w.write_list(self.generators.iter().map(|g| g.canon_bytes()));
        w.write_list(self.configs.iter().map(|c| c.canon_bytes()));
        self.verification.canon_write(w);
    }
}

digest_id!(
    /// Content-addressed identity of a [`Decomposition`]. Depends on the
    /// generator chain, the configuration chain, **and** the verification
    /// status — a recorded and a replay-verified decomposition over
    /// identical data are different ids (module doc).
    DecompositionId
);

#[cfg(test)]
mod tests {
    use super::*;

    fn gens(n: usize) -> Vec<GeneratorId> {
        (0..n).map(|i| GeneratorId::named(&format!("g{i}@1"))).collect()
    }

    fn configs(n: usize) -> Vec<ConfigId> {
        (0..n)
            .map(|i| ConfigId::from_canon(format!("x{i}").as_bytes()))
            .collect()
    }

    #[test]
    fn valid_chain_constructs() {
        assert!(Decomposition::recorded(gens(2), configs(3)).is_ok());
        assert!(Decomposition::replay_verified(gens(2), configs(3)).is_ok());
        // Zero generators: a single identity-ish configuration, no arrows.
        assert!(Decomposition::recorded(gens(0), configs(1)).is_ok());
    }

    #[test]
    fn malformed_chain_is_rejected() {
        let err = Decomposition::recorded(gens(2), configs(2)).unwrap_err();
        assert_eq!(
            err,
            DecompositionError::ChainLengthMismatch {
                generators: 2,
                configs: 2,
            }
        );
        assert!(Decomposition::recorded(gens(0), configs(0)).is_err());
        assert!(Decomposition::recorded(gens(3), configs(5)).is_err());
        assert!(Decomposition::replay_verified(gens(1), configs(1)).is_err());
    }

    #[test]
    fn recorded_and_replay_verified_over_identical_data_have_distinct_ids() {
        let recorded = Decomposition::recorded(gens(2), configs(3)).unwrap();
        let verified = Decomposition::replay_verified(gens(2), configs(3)).unwrap();
        assert_ne!(
            recorded.id(),
            verified.id(),
            "recorded vs replay-verified MUST NOT collide (ADR-0002 §5.1/§5.2)"
        );
        assert!(!recorded.is_replay_verified());
        assert!(verified.is_replay_verified());
    }

    #[test]
    fn canon_ordinals_are_stable() {
        // Freeze the wire ordinals for DecompVerification — a reorder would
        // silently merge recorded and replay-verified decompositions, or
        // change every DecompositionId.
        for (v, ord) in [
            (DecompVerification::Recorded, 0u64),
            (DecompVerification::ReplayVerified, 1u64),
        ] {
            let mut w = CanonWriter::new();
            v.canon_write(&mut w);
            let mut expected = CanonWriter::new();
            expected.write_enum(ord, |_| {});
            assert_eq!(w.finish(), expected.finish(), "{v:?} ordinal drifted");
        }
    }

    /// Golden vector, reproduced independently with a fresh `CanonWriter`
    /// (not via `Decomposition::canon_write`), so it cannot be vacuously
    /// satisfied by the code it guards.
    #[test]
    fn golden_vector_recorded_decomposition() {
        let d = Decomposition::recorded(gens(1), configs(2)).unwrap();

        let mut got = CanonWriter::new();
        d.canon_write(&mut got);

        let mut expected = CanonWriter::new();
        expected.write_list(d.generators.iter().map(|g| g.canon_bytes()));
        expected.write_list(d.configs.iter().map(|c| c.canon_bytes()));
        expected.write_enum(0, |_| {}); // Recorded

        assert_eq!(got.finish(), expected.finish());
    }

    /// Same golden vector, `ReplayVerified` form: only the trailing ordinal
    /// differs, and that is exactly what must change the id.
    #[test]
    fn golden_vector_replay_verified_decomposition() {
        let d = Decomposition::replay_verified(gens(1), configs(2)).unwrap();

        let mut got = CanonWriter::new();
        d.canon_write(&mut got);

        let mut expected = CanonWriter::new();
        expected.write_list(d.generators.iter().map(|g| g.canon_bytes()));
        expected.write_list(d.configs.iter().map(|c| c.canon_bytes()));
        expected.write_enum(1, |_| {}); // ReplayVerified

        assert_eq!(got.finish(), expected.finish());
    }
}
