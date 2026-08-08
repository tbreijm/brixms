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

/// Why [`Decomposition::verify_replay`] refused to issue the
/// `ReplayVerified` tag (ADR-0019 D2).
///
/// Like [`DecompositionError`] this is Rust-side validation only: it is never
/// canonically encoded or hashed, because a chain that fails replay never
/// becomes a verified artifact. It carries no ABI ordinal.
///
/// Every variant means *no artifact was produced*. There is deliberately no
/// variant that yields a downgraded or partially-verified decomposition —
/// ADR-0002 §4's fail-closed discipline: never a downgrade-hiding pass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplayVerificationError {
    /// The receiver was not in [`DecompVerification::Recorded`] form. This
    /// transition upgrades a record; it does not re-verify an already-verified
    /// chain, and it never re-tags one that is already verified.
    NotRecorded { found: DecompVerification },
    /// `generators[index]` is not a member of the supplied registry — the
    /// chain cites something outside `𝒢`. Without this check an arbitrary
    /// digest could pose as a primitive generator.
    GeneratorNotInRegistry {
        index: usize,
        generator: GeneratorId,
    },
    /// `semantics.realizes(generators[index], configs[index],
    /// configs[index + 1])` returned false: an intermediate configuration is
    /// not realized by the generator that claims to produce it. This is the
    /// variant that catches a fabricated chain — notably the padded
    /// `[src, dst, dst, …]` shape ADR-0018 retired, whose middle links assert
    /// a generator runs `dst → dst`.
    RelationNotRealized {
        index: usize,
        generator: GeneratorId,
        src: ConfigId,
        dst: ConfigId,
    },
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
    ///
    /// ⚠ **Deprecated by ADR-0019: this stamps, it does not verify.** It sets
    /// `ReplayVerified` on whatever chain it is handed, so the tag bottoms out
    /// at caller discipline. Use [`Decomposition::verify_replay`], which
    /// performs the check that the tag denotes. Retained only until the
    /// remaining callers migrate (ADR-0019 implementation steps 3–5); it is
    /// **removed** when they do, together with the `pub` fields that let the
    /// tag be set by direct assignment without any constructor at all.
    ///
    /// **No production code calls this** as of ADR-0019 step 2 — the
    /// settlement checker goes through `verify_replay`. The remaining callers
    /// are tests, which migrate in the sealing step. It carries no
    /// `#[deprecated]` attribute yet only because that would fail the
    /// `-D warnings` lint gate before those tests move.
    pub fn replay_verified(
        generators: Vec<GeneratorId>,
        configs: Vec<ConfigId>,
    ) -> Result<Self, DecompositionError> {
        Self::build(generators, configs, DecompVerification::ReplayVerified)
    }

    /// **Earn** the `ReplayVerified` tag by replaying this chain (ADR-0019 D2).
    ///
    /// This is the checked transition that replaces the old
    /// `replay_verified` stamp. It consumes a `Recorded` decomposition and
    /// returns the same generators and configurations tagged `ReplayVerified`
    /// **iff** every link checks out:
    ///
    /// 1. the receiver is in [`DecompVerification::Recorded`] form — this
    ///    upgrades a record, it does not re-audit an already-verified chain;
    /// 2. every generator is a member of `registry` (`g ∈ 𝒢`);
    /// 3. `semantics.realizes(gᵢ, xᵢ, xᵢ₊₁)` for every `i` — the exact
    ///    relational composition `ρ_k = ρ_gn ∘ … ∘ ρ_g1`, walked stepwise
    ///    along `x_0, …, x_n`.
    ///
    /// The chain-length invariant is already guaranteed by construction.
    ///
    /// **Scope of the resulting tag** (ADR-0019 §2). `ReplayVerified` attests
    /// to the *intrinsic* check above and nothing more. It does **not** attest
    /// to journal integrity or to agreement with a committed step's endpoints:
    /// this artifact canonically encodes generators, configurations and the
    /// tag — not a journal, context or step identity — and reading its frozen
    /// id as a receipt for values it does not encode would dress an identity
    /// limitation up as proof depth. Those checks remain contextual
    /// obligations of `soc_core::audit::audit_step`, which performs them
    /// before calling this.
    ///
    /// Consuming `self` makes the state transition plain. Cloning an already
    /// earned artifact stays valid; re-verifying one is refused.
    ///
    /// Fails closed: every rejection is a typed [`ReplayVerificationError`]
    /// and **no artifact is produced** — never a weaker artifact in place of
    /// an error.
    pub fn verify_replay(
        self,
        registry: &crate::GeneratorRegistry,
        semantics: &dyn crate::GeneratorSemantics,
    ) -> Result<Self, ReplayVerificationError> {
        if self.verification != DecompVerification::Recorded {
            return Err(ReplayVerificationError::NotRecorded {
                found: self.verification,
            });
        }

        for (index, g) in self.generators.iter().enumerate() {
            if !registry.contains(g) {
                return Err(ReplayVerificationError::GeneratorNotInRegistry {
                    index,
                    generator: *g,
                });
            }
            // The chain-length invariant guarantees both indices exist.
            let src = &self.configs[index];
            let dst = &self.configs[index + 1];
            if !semantics.realizes(g, src, dst) {
                return Err(ReplayVerificationError::RelationNotRealized {
                    index,
                    generator: *g,
                    src: *src,
                    dst: *dst,
                });
            }
        }

        Ok(Decomposition {
            generators: self.generators,
            configs: self.configs,
            verification: DecompVerification::ReplayVerified,
        })
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
    use crate::{GeneratorRegistry, GeneratorSemantics};

    fn gens(n: usize) -> Vec<GeneratorId> {
        (0..n)
            .map(|i| GeneratorId::named(&format!("g{i}@1")))
            .collect()
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

    // ---- ADR-0019: the tag is earned, not stamped -----------------------

    /// A registry over `g0..gn-1`, the names `gens` produces.
    fn registry_for(n: usize) -> GeneratorRegistry {
        let mut r = GeneratorRegistry::new();
        for g in gens(n) {
            r.insert(g);
        }
        r
    }

    /// A semantics that accepts exactly the honest chain `xi → xi+1` for
    /// `gi`, and records every call it was asked to make.
    #[derive(Default)]
    struct RecordingSemantics {
        calls: std::cell::RefCell<Vec<(GeneratorId, ConfigId, ConfigId)>>,
    }

    impl GeneratorSemantics for RecordingSemantics {
        fn realizes(&self, g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> bool {
            self.calls.borrow_mut().push((*g, *src, *dst));
            // Honest relation: gi realizes xi → xi+1 and nothing else.
            gens(8)
                .iter()
                .position(|c| c == g)
                .is_some_and(|i| *src == configs(9)[i] && *dst == configs(9)[i + 1])
        }
    }

    /// A semantics that accepts everything — the §6 residual made concrete.
    struct AlwaysTrue;
    impl GeneratorSemantics for AlwaysTrue {
        fn realizes(&self, _: &GeneratorId, _: &ConfigId, _: &ConfigId) -> bool {
            true
        }
    }

    #[test]
    fn verify_replay_walks_every_link_exactly_once_in_order() {
        let d = Decomposition::recorded(gens(3), configs(4)).unwrap();
        let sem = RecordingSemantics::default();
        let verified = d
            .verify_replay(&registry_for(3), &sem)
            .expect("honest chain verifies");

        assert!(verified.is_replay_verified());
        // Exactly one call per link, in chain order, over the adjacent pairs.
        let calls = sem.calls.borrow();
        let expected: Vec<_> = (0..3)
            .map(|i| (gens(3)[i], configs(4)[i], configs(4)[i + 1]))
            .collect();
        assert_eq!(*calls, expected, "every link is checked once, in order");
    }

    #[test]
    fn verify_replay_preserves_the_frozen_verified_id() {
        // The whole point of D8: the earned artifact is byte-identical to the
        // one the old stamp produced, so no DecompositionId moves.
        let earned = Decomposition::recorded(gens(2), configs(3))
            .unwrap()
            .verify_replay(&registry_for(2), &RecordingSemantics::default())
            .expect("honest chain verifies");
        let stamped = Decomposition::replay_verified(gens(2), configs(3)).unwrap();
        assert_eq!(earned, stamped);
        assert_eq!(earned.id(), stamped.id());
    }

    #[test]
    fn a_padded_chain_cannot_earn_the_tag() {
        // ADR-0018's retired shape: [src, dst, dst, …]. It passes the
        // chain-length invariant and would have been stamped `ReplayVerified`
        // by the old constructor. Under a sound semantics it cannot be earned.
        let mut padded = vec![configs(9)[0]];
        padded.resize(3, configs(9)[8]);
        let d = Decomposition::recorded(gens(2), padded).unwrap();

        match d.verify_replay(&registry_for(2), &RecordingSemantics::default()) {
            Err(ReplayVerificationError::RelationNotRealized { index, .. }) => {
                assert_eq!(index, 0, "the first fabricated link is where it fails");
            }
            other => panic!("a padded chain must never earn the tag, got {other:?}"),
        }
    }

    #[test]
    fn a_corrupted_intermediate_config_is_refused_and_yields_no_artifact() {
        let mut chain = configs(3);
        chain[1] = ConfigId::from_canon(b"corrupted");
        let d = Decomposition::recorded(gens(2), chain).unwrap();

        let result = d.verify_replay(&registry_for(2), &RecordingSemantics::default());
        assert!(
            matches!(
                result,
                Err(ReplayVerificationError::RelationNotRealized { index: 0, .. })
            ),
            "got {result:?}"
        );
    }

    #[test]
    fn a_generator_outside_the_registry_is_refused_before_the_relation_is_asked() {
        let d = Decomposition::recorded(gens(2), configs(3)).unwrap();
        // Registry holds only g0, so g1 is outside 𝒢.
        let sem = RecordingSemantics::default();
        match d.verify_replay(&registry_for(1), &sem) {
            Err(ReplayVerificationError::GeneratorNotInRegistry { index, generator }) => {
                assert_eq!(index, 1);
                assert_eq!(generator, gens(2)[1]);
            }
            other => panic!("an unregistered generator must be refused, got {other:?}"),
        }
        // Membership is checked before the relation for that link.
        assert_eq!(sem.calls.borrow().len(), 1, "only link 0 was ever asked");
    }

    #[test]
    fn an_already_verified_chain_cannot_be_re_verified() {
        let verified = Decomposition::recorded(gens(1), configs(2))
            .unwrap()
            .verify_replay(&registry_for(1), &RecordingSemantics::default())
            .unwrap();
        match verified.verify_replay(&registry_for(1), &RecordingSemantics::default()) {
            Err(ReplayVerificationError::NotRecorded { found }) => {
                assert_eq!(found, DecompVerification::ReplayVerified);
            }
            other => panic!("re-verification must be refused, got {other:?}"),
        }
    }

    /// ADR-0019 §6 residual 1, stated as a test rather than left to prose:
    /// the transition guarantees the predicate was **executed**, not that the
    /// oracle was authenticated. A caller supplying an always-true semantics
    /// still gets a verified artifact. This test exists so the limit is
    /// visible in the suite and cannot be quietly forgotten.
    #[test]
    fn an_always_true_semantics_still_passes_a_fabricated_chain() {
        let mut padded = vec![configs(9)[0]];
        padded.resize(3, configs(9)[8]);
        let d = Decomposition::recorded(gens(2), padded).unwrap();
        assert!(
            d.verify_replay(&registry_for(2), &AlwaysTrue).is_ok(),
            "ADR-0019 §6 residual 1: the supplied semantics is not authenticated"
        );
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
