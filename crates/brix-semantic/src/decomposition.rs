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
#[derive(Clone, PartialEq, Eq, Debug)]
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
    /// The declared semantics could not answer for `generators[index]` — it
    /// declares no relation for that generator (ADR-0020 D2). Distinct from
    /// `RelationNotRealized`: nothing was checked, so this is a refusal, not a
    /// checked negative.
    Semantics {
        index: usize,
        error: crate::SemanticsError,
    },
}

/// A finite factorization `k = g_n ∘ ⋯ ∘ g_1`, `g_i ∈ 𝒢`, with the
/// intermediate configurations `x_0, …, x_n`: `configs[0]` is the source of
/// `g_1`, `configs[i]` is both the target of `g_i` and the source of
/// `g_{i+1}`, and `configs[n]` is the target of `g_n`. The invariant
/// `configs.len() == generators.len() + 1` is enforced at construction (see
/// [`DecompositionError`]) — there is no way to build a `Decomposition` with
/// a mismatched chain.
///
/// **Fields are private** (ADR-0019 D2). The verification tag contributes to
/// this artifact's identity, so a caller able to set it would be able to mint
/// the claim the artifact exists to support — and while the fields were `pub`,
/// that did not even require a constructor.
///
/// The three doors ADR-0019 closes, as executable gates. **Direct assignment**
/// — the one ADR-0016 §7.1 missed, since sealing the constructor alone would
/// not have stopped it:
///
/// ```compile_fail
/// use brix_semantic::{ConfigId, DecompVerification, Decomposition, GeneratorId};
/// let g = GeneratorId::named("g@1");
/// let (x0, x1) = (ConfigId::from_canon(b"x0"), ConfigId::from_canon(b"x1"));
/// let mut d = Decomposition::recorded(vec![g], vec![x0, x1]).unwrap();
/// d.verification = DecompVerification::ReplayVerified;
/// ```
///
/// **Struct-literal construction**, which bypasses every constructor:
///
/// ```compile_fail
/// use brix_semantic::{ConfigId, DecompVerification, Decomposition, GeneratorId};
/// let d = Decomposition {
///     generators: vec![GeneratorId::named("g@1")],
///     configs: vec![ConfigId::from_canon(b"x0"), ConfigId::from_canon(b"x1")],
///     verification: DecompVerification::ReplayVerified,
/// };
/// ```
///
/// **The removed stamp constructor:**
///
/// ```compile_fail
/// use brix_semantic::{ConfigId, Decomposition, GeneratorId};
/// let g = GeneratorId::named("g@1");
/// let (x0, x1) = (ConfigId::from_canon(b"x0"), ConfigId::from_canon(b"x1"));
/// let d = Decomposition::replay_verified(vec![g], vec![x0, x1]);
/// ```
///
/// Reads are unaffected: use [`Decomposition::generators`],
/// [`Decomposition::configs`] and [`Decomposition::verification`]. The
/// verified form is reachable only through [`Decomposition::verify_replay`],
/// which earns it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Decomposition {
    generators: Vec<GeneratorId>,
    configs: Vec<ConfigId>,
    verification: DecompVerification,
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
        semantics: &crate::GeneratorSemanticsV1,
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
            match semantics.realizes(g, src, dst) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(ReplayVerificationError::RelationNotRealized {
                        index,
                        generator: *g,
                        src: *src,
                        dst: *dst,
                    })
                }
                Err(e) => return Err(ReplayVerificationError::Semantics { index, error: e }),
            }
        }

        Ok(Decomposition {
            generators: self.generators,
            configs: self.configs,
            verification: DecompVerification::ReplayVerified,
        })
    }

    /// The generator chain `g_1, …, g_n`.
    pub fn generators(&self) -> &[GeneratorId] {
        &self.generators
    }

    /// The intermediate-configuration chain `x_0, …, x_n`. Always exactly one
    /// longer than [`Decomposition::generators`].
    pub fn configs(&self) -> &[ConfigId] {
        &self.configs
    }

    /// This decomposition's verification status.
    pub const fn verification(&self) -> DecompVerification {
        self.verification
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
    use crate::{GeneratorRegistry, GeneratorSemanticsV1};

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
        // The verified form has no direct constructor (ADR-0019 D2); it is
        // reachable only through `verify_replay`, exercised below.
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

    /// The honest declaration for an `n`-link chain: `gi ↦ ExactRows{(xi, xi+1)}`.
    /// ADR-0020 D2 — a fixture declares finite rows instead of implementing a
    /// predicate, so what it asserts is inspectable data rather than code.
    fn honest_semantics(n: usize) -> GeneratorSemanticsV1 {
        let mut m = GeneratorSemanticsV1::new();
        for (i, g) in gens(n).into_iter().enumerate() {
            m.declare_rows(g, [(configs(n + 1)[i], configs(n + 1)[i + 1])]);
        }
        m
    }

    /// A declaration whose rows accept everything the chain could ask — the
    /// ADR-0019 §6 residual, now expressible only as *visible data* carrying
    /// its own distinct id (ADR-0020 D9).
    fn permissive_semantics(n: usize, chain: &[ConfigId]) -> GeneratorSemanticsV1 {
        let mut m = GeneratorSemanticsV1::new();
        for (i, g) in gens(n).into_iter().enumerate() {
            m.declare_rows(g, [(chain[i], chain[i + 1])]);
        }
        m
    }

    #[test]
    fn verify_replay_accepts_exactly_the_declared_chain() {
        let d = Decomposition::recorded(gens(3), configs(4)).unwrap();
        let verified = d
            .verify_replay(&registry_for(3), &honest_semantics(3))
            .expect("honest chain verifies");
        assert!(verified.is_replay_verified());
    }

    #[test]
    fn verify_replay_preserves_the_frozen_verified_id() {
        // The whole point of D8: the earned artifact is byte-identical to the
        // one the old stamp produced, so no DecompositionId moves.
        let earned = Decomposition::recorded(gens(2), configs(3))
            .unwrap()
            .verify_replay(&registry_for(2), &honest_semantics(2))
            .expect("honest chain verifies");
        // The frozen expectation, rebuilt independently of any constructor:
        // the canonical encoding is generators, configs, then the tag ordinal
        // 1 — exactly what the removed stamp used to produce.
        let mut expected = CanonWriter::new();
        expected.write_list(gens(2).iter().map(|g| g.canon_bytes()));
        expected.write_list(configs(3).iter().map(|c| c.canon_bytes()));
        DecompVerification::ReplayVerified.canon_write(&mut expected);

        let mut got = CanonWriter::new();
        earned.canon_write(&mut got);
        assert_eq!(got.finish(), expected.finish());
        assert_eq!(earned.verification(), DecompVerification::ReplayVerified);
    }

    #[test]
    fn a_padded_chain_cannot_earn_the_tag() {
        // ADR-0018's retired shape: [src, dst, dst, …]. It passes the
        // chain-length invariant and would have been stamped `ReplayVerified`
        // by the old constructor. Under a sound semantics it cannot be earned.
        let mut padded = vec![configs(9)[0]];
        padded.resize(3, configs(9)[8]);
        let d = Decomposition::recorded(gens(2), padded).unwrap();

        match d.verify_replay(&registry_for(2), &honest_semantics(2)) {
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

        let result = d.verify_replay(&registry_for(2), &honest_semantics(2));
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
        match d.verify_replay(&registry_for(1), &honest_semantics(2)) {
            Err(ReplayVerificationError::GeneratorNotInRegistry { index, generator }) => {
                assert_eq!(index, 1);
                assert_eq!(generator, gens(2)[1]);
            }
            other => panic!("an unregistered generator must be refused, got {other:?}"),
        }
    }

    #[test]
    fn an_already_verified_chain_cannot_be_re_verified() {
        let verified = Decomposition::recorded(gens(1), configs(2))
            .unwrap()
            .verify_replay(&registry_for(1), &honest_semantics(1))
            .unwrap();
        match verified.verify_replay(&registry_for(1), &honest_semantics(1)) {
            Err(ReplayVerificationError::NotRecorded { found }) => {
                assert_eq!(found, DecompVerification::ReplayVerified);
            }
            other => panic!("re-verification must be refused, got {other:?}"),
        }
    }

    /// **Supersedes ADR-0019's `an_always_true_semantics_still_passes_a_fabricated_chain`**
    /// (ADR-0020 D9). A permissive oracle can no longer be *code*; it can only
    /// be declared rows. It still verifies the chain it declares — content
    /// addressing does not make declared rows correct (ADR-0020 §5 residual 2)
    /// — but it is now **visible and content-addressed**, so it carries a
    /// different `GeneratorSemanticsIdV1` than the honest declaration and a
    /// consumer holding the expected id rejects it.
    #[test]
    fn a_permissive_declaration_is_detectable_by_its_distinct_id() {
        let mut padded = vec![configs(9)[0]];
        padded.resize(3, configs(9)[8]);
        let d = Decomposition::recorded(gens(2), padded.clone()).unwrap();

        let permissive = permissive_semantics(2, &padded);
        assert!(
            d.verify_replay(&registry_for(2), &permissive).is_ok(),
            "declared rows still verify the chain they declare"
        );
        assert_ne!(
            permissive.id(),
            honest_semantics(2).id(),
            "ADR-0020: the substituted oracle must be DETECTABLE by its id"
        );
    }

    #[test]
    fn recorded_and_replay_verified_over_identical_data_have_distinct_ids() {
        let recorded = Decomposition::recorded(gens(2), configs(3)).unwrap();
        let verified = Decomposition::recorded(gens(2), configs(3))
            .unwrap()
            .verify_replay(&registry_for(2), &honest_semantics(2))
            .unwrap();
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
        let d = Decomposition::recorded(gens(1), configs(2))
            .unwrap()
            .verify_replay(&registry_for(1), &honest_semantics(1))
            .unwrap();

        let mut got = CanonWriter::new();
        d.canon_write(&mut got);

        let mut expected = CanonWriter::new();
        expected.write_list(d.generators.iter().map(|g| g.canon_bytes()));
        expected.write_list(d.configs.iter().map(|c| c.canon_bytes()));
        expected.write_enum(1, |_| {}); // ReplayVerified

        assert_eq!(got.finish(), expected.finish());
    }
}
