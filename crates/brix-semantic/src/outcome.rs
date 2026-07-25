//! The single epistemic outcome lattice (ADR-0001 §4).
//!
//! Every kernel and every resolver in BrixMS projects into **this one**
//! outcome vocabulary. It is defined here, once, and frozen. The design
//! commitments it encodes:
//!
//! - `Unknown` is **bottom** — it never collapses to `true`/`false`. A prover
//!   that runs out of budget has proved nothing, not the negation.
//! - Resource exhaustion is `Unknown`, never `Refuted`/`Rejected`.
//! - Fail-closed means fail to `Unknown`, never silently to `Proven`.
//! - **One authority per outcome route** ([`Outcome::authority`]): exactly one
//!   named producer may publish each outcome. This is data, checkable, not a
//!   review-time convention.

use brix_canon::{CanonWriter, Canonical};

/// The epistemic status of a [`crate::Judgement`]. Six outcomes, ordered by
/// the strength of the epistemic commitment they carry, with `Unknown` at the
/// bottom:
///
/// ```text
///        Proven      Refuted     ← theorems (revision-invariant), opposite poles
///            \        /
///            Audited            ← tight 𝒢-decomposition replay-verified; not a theorem (ADR-0002 §4)
///               |
///            Derived            ← settlement-authoritative *within a revision*
///               |
///            Measured           ← external certified result / simulation / estimate
///               |
///            Unknown            ← bottom; never collapses to true/false
/// ```
///
/// `Proven` and `Refuted` are incomparable (a proposition is not both); they
/// are the two revision-invariant poles, both strictly above `Audited`.
/// Everything below `Derived` is revision-scoped or weaker. The ordinals
/// below are **canonical ABI** — append-only, never reordered. The original
/// five members (ordinals 0–4) are ADR-0001 §4; `Audited` (ordinal 5) is
/// appended per ADR-0002 §4 (⟨D-AUD⟩) — an append-only ABI extension, not a
/// re-decision. See [`Outcome::lattice_le`] for the explicit partial order;
/// it is **not** the declaration/derive order below, which already diverges
/// (`Proven`/`Refuted` are declaration-adjacent but lattice-incomparable).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Outcome {
    /// A proof-kernel-accepted certificate exists. Revision-invariant.
    Proven,
    /// A proof-kernel-accepted refutation exists. Revision-invariant.
    Refuted,
    /// The settlement kernel derived it at a revision. Authoritative *within
    /// that revision*; **not** a theorem in the proof calculus.
    Derived,
    /// An external certified result, simulation, or measurement/estimate.
    /// Carries its own error/approximation profile elsewhere.
    Measured,
    /// Bottom. Includes resource-exhausted and incomplete search. Never
    /// `false`, never `true`.
    Unknown,
    /// The committed step's tight $\mathcal G$-decomposition
    /// `k = g_n ∘ ⋯ ∘ g_1` has been **replayed and verified to compose
    /// exactly** (SOC audit factorization, ADR-0002 §4). Strictly stronger
    /// than `Derived` (which asserts only that the engine committed the step,
    /// on a compact support record); strictly weaker than `Proven`/`Refuted`
    /// (still interpreted over the settlement world — [`Outcome::is_theorem`]
    /// stays `false` for it). Appended as ordinal 5 (ADR-0002 §4, ⟨D-AUD⟩);
    /// declared last so declaration order carries no ABI meaning —
    /// [`Outcome::ordinal`] alone is canonical.
    Audited,
}

/// The sole producer permitted to publish a given [`Outcome`] (ADR-0001 §4.1).
/// Exactly one authority per outcome; no other route may publish it. A
/// resolver (`brix.type`, `brix.proof`, …) may *construct candidates* for a
/// `Proven`/`Refuted`, but only [`Authority::ProofKernel`] may publish the
/// outcome itself.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Authority {
    /// A dependent proof **kernel** — the role, not a specific crate:
    /// `brix-kernel`, or an external kernel (e.g. Lean) reached across an
    /// `elaboration-boundary`. Sole publisher of `Proven`/`Refuted`; the
    /// certificate names *which* kernel certified it (a resolver may construct
    /// the candidate term, but never publish the outcome).
    ProofKernel,
    /// The settlement kernel (`brix-rt`/`brix-oracle`). Sole publisher of
    /// `Derived`.
    SettlementKernel,
    /// A named external driver / simulator, via a certified-result envelope.
    /// Sole publisher of `Measured`.
    ExternalDriver,
    /// Any resolver may *emit* `Unknown(reason)`; no one may downgrade a
    /// stronger outcome to hide a failure.
    AnyResolver,
    /// The audit-factorization checker — the reference replayer
    /// (`brix-oracle`'s role, ADR-0002 §4.1). Replays a `Decomposition`
    /// against the log and verifies it composes exactly. Sole publisher of
    /// `Audited`; the engine's hot loop may *record* a decomposition but may
    /// never assert its verification.
    AuditChecker,
}

impl Outcome {
    /// The one authority permitted to publish this outcome (ADR-0001 §4.1).
    /// Total by construction — every outcome has exactly one.
    pub const fn authority(self) -> Authority {
        match self {
            Outcome::Proven | Outcome::Refuted => Authority::ProofKernel,
            Outcome::Derived => Authority::SettlementKernel,
            Outcome::Measured => Authority::ExternalDriver,
            Outcome::Unknown => Authority::AnyResolver,
            Outcome::Audited => Authority::AuditChecker,
        }
    }

    /// A revision-invariant theorem in the dependent proof calculus. Only a
    /// kernel-accepted `Proven`/`Refuted` qualifies — a settlement `Derived` is
    /// authoritative but is *not* a theorem (ADR-0001 §3).
    pub const fn is_theorem(self) -> bool {
        matches!(self, Outcome::Proven | Outcome::Refuted)
    }

    /// The bottom of the lattice. `Unknown` and nothing else; it never carries
    /// a truth commitment.
    pub const fn is_bottom(self) -> bool {
        matches!(self, Outcome::Unknown)
    }

    /// Canonical ABI ordinal. Append-only; never reorder. `Audited` = 5 is
    /// the ADR-0002 §4 (⟨D-AUD⟩) append; 0–4 are the frozen ADR-0001 §4
    /// ordinals.
    const fn ordinal(self) -> u64 {
        match self {
            Outcome::Proven => 0,
            Outcome::Refuted => 1,
            Outcome::Derived => 2,
            Outcome::Measured => 3,
            Outcome::Unknown => 4,
            Outcome::Audited => 5,
        }
    }

    /// The explicit lattice partial order (ADR-0002 §4). This function *is*
    /// the order — the enum's declaration/derive order is **not** the
    /// lattice order and must never be read as one (they already diverge:
    /// `Proven`/`Refuted` are declaration-adjacent but lattice-incomparable).
    ///
    /// The order is a single totally-ordered spine capped by two incomparable
    /// tops:
    ///
    /// ```text
    ///        Proven      Refuted     ← two incomparable tops
    ///            \        /
    ///            Audited
    ///               |
    ///            Derived
    ///               |
    ///            Measured
    ///               |
    ///            Unknown             ← bottom
    /// ```
    ///
    /// `Unknown ≤ Measured ≤ Derived ≤ Audited ≤ {Proven, Refuted}`; `Proven`
    /// and `Refuted` are each ≤ themselves but **not** ≤ each other. The
    /// relation is reflexive, transitive, and antisymmetric on the six-member
    /// set (verified by `lattice_le_is_antisymmetric` below).
    pub const fn lattice_le(self, other: Outcome) -> bool {
        match (spine_rank(self), spine_rank(other)) {
            // Both on the spine (Unknown/Measured/Derived/Audited): the
            // ordinary total order by rank.
            (Some(a), Some(b)) => a <= b,
            // A spine element (including Audited) is always below a top —
            // the tops sit above everything else in the lattice.
            (Some(_), None) => true,
            // A top is never below a spine element.
            (None, Some(_)) => false,
            // Two tops: `Proven`/`Refuted` are incomparable with each other,
            // reflexively ≤ themselves only.
            (None, None) => matches!(
                (self, other),
                (Outcome::Proven, Outcome::Proven) | (Outcome::Refuted, Outcome::Refuted)
            ),
        }
    }
}

/// This outcome's rank on the totally-ordered spine `Unknown < Measured <
/// Derived < Audited`, or `None` if it is one of the two top poles
/// (`Proven`/`Refuted`, which sit above the whole spine but are incomparable
/// with each other). A private helper for [`Outcome::lattice_le`] only — the
/// rank has no ABI meaning and is unrelated to [`Outcome::ordinal`].
const fn spine_rank(o: Outcome) -> Option<u8> {
    match o {
        Outcome::Unknown => Some(0),
        Outcome::Measured => Some(1),
        Outcome::Derived => Some(2),
        Outcome::Audited => Some(3),
        Outcome::Proven | Outcome::Refuted => None,
    }
}

impl Canonical for Outcome {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |_| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All six outcomes, for exhaustive/brute-force checks below.
    const ALL_OUTCOMES: [Outcome; 6] = [
        Outcome::Proven,
        Outcome::Refuted,
        Outcome::Derived,
        Outcome::Measured,
        Outcome::Unknown,
        Outcome::Audited,
    ];

    #[test]
    fn every_outcome_has_exactly_one_authority() {
        // Totality + the frozen routing table (ADR-0001 §4.1), plus the
        // ADR-0002 §4.1 `Audited` row. Six outcomes, six asserts: totality
        // holds by construction (`authority()` is a total match).
        assert_eq!(Outcome::Proven.authority(), Authority::ProofKernel);
        assert_eq!(Outcome::Refuted.authority(), Authority::ProofKernel);
        assert_eq!(Outcome::Derived.authority(), Authority::SettlementKernel);
        assert_eq!(Outcome::Measured.authority(), Authority::ExternalDriver);
        assert_eq!(Outcome::Unknown.authority(), Authority::AnyResolver);
        assert_eq!(Outcome::Audited.authority(), Authority::AuditChecker);
    }

    #[test]
    fn only_kernel_outcomes_are_theorems() {
        assert!(Outcome::Proven.is_theorem());
        assert!(Outcome::Refuted.is_theorem());
        assert!(!Outcome::Derived.is_theorem());
        assert!(!Outcome::Measured.is_theorem());
        assert!(!Outcome::Unknown.is_theorem());
        // Audited is still-interpreted-over-the-settlement-world, not a
        // theorem in the proof calculus (ADR-0002 §4).
        assert!(!Outcome::Audited.is_theorem());
    }

    #[test]
    fn only_unknown_is_bottom() {
        assert!(Outcome::Unknown.is_bottom());
        for o in [
            Outcome::Proven,
            Outcome::Refuted,
            Outcome::Derived,
            Outcome::Measured,
            Outcome::Audited,
        ] {
            assert!(!o.is_bottom());
        }
    }

    #[test]
    fn canon_ordinals_are_stable() {
        // Freeze the wire ordinals — a reorder would silently change every
        // JudgementId that embeds an Outcome. 0–4 are the frozen ADR-0001 §4
        // ordinals (untouched); 5 is the ADR-0002 §4 (⟨D-AUD⟩) append.
        for (o, ord) in [
            (Outcome::Proven, 0u64),
            (Outcome::Refuted, 1),
            (Outcome::Derived, 2),
            (Outcome::Measured, 3),
            (Outcome::Unknown, 4),
            (Outcome::Audited, 5),
        ] {
            let mut w = CanonWriter::new();
            o.canon_write(&mut w);
            let mut expected = CanonWriter::new();
            expected.write_enum(ord, |_| {});
            assert_eq!(w.finish(), expected.finish(), "{o:?} ordinal drifted");
        }
    }

    #[test]
    fn lattice_le_is_reflexive() {
        for o in ALL_OUTCOMES {
            assert!(o.lattice_le(o), "{o:?} must be ≤ itself");
        }
    }

    #[test]
    fn lattice_le_spine_chain() {
        assert!(Outcome::Unknown.lattice_le(Outcome::Measured));
        assert!(Outcome::Measured.lattice_le(Outcome::Derived));
        assert!(Outcome::Derived.lattice_le(Outcome::Audited));
        // Transitivity along the chain.
        assert!(Outcome::Unknown.lattice_le(Outcome::Derived));
        assert!(Outcome::Unknown.lattice_le(Outcome::Audited));
        assert!(Outcome::Measured.lattice_le(Outcome::Audited));
    }

    #[test]
    fn lattice_le_audited_below_both_tops() {
        assert!(Outcome::Derived.lattice_le(Outcome::Audited));
        assert!(Outcome::Audited.lattice_le(Outcome::Proven));
        assert!(Outcome::Audited.lattice_le(Outcome::Refuted));
    }

    #[test]
    fn lattice_le_proven_refuted_incomparable() {
        assert!(!Outcome::Proven.lattice_le(Outcome::Refuted));
        assert!(!Outcome::Refuted.lattice_le(Outcome::Proven));
    }

    #[test]
    fn lattice_le_unknown_is_bottom_of_the_order() {
        for o in ALL_OUTCOMES {
            assert!(Outcome::Unknown.lattice_le(o), "Unknown must be ≤ {o:?}");
        }
    }

    #[test]
    fn lattice_le_is_antisymmetric() {
        // Brute-force over all 6×6 pairs: if a ≤ b and b ≤ a then a == b.
        for a in ALL_OUTCOMES {
            for b in ALL_OUTCOMES {
                if a.lattice_le(b) && b.lattice_le(a) {
                    assert_eq!(a, b, "antisymmetry violated: {a:?} <= {b:?} <= {a:?}");
                }
            }
        }
    }
}
