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

/// The epistemic status of a [`crate::Judgement`]. Five outcomes, ordered by
/// the strength of the epistemic commitment they carry, with `Unknown` at the
/// bottom:
///
/// ```text
///        Proven      Refuted     ← theorems (revision-invariant), opposite poles
///            \        /
///            Derived            ← settlement-authoritative *within a revision*
///               |
///            Measured           ← external certified result / simulation / estimate
///               |
///            Unknown            ← bottom; never collapses to true/false
/// ```
///
/// `Proven` and `Refuted` are incomparable (a proposition is not both); they
/// are the two revision-invariant poles. Everything below `Derived` is
/// revision-scoped or weaker. The ordinals below are **canonical ABI** —
/// append-only, never reordered.
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
}

/// The sole producer permitted to publish a given [`Outcome`] (ADR-0001 §4.1).
/// Exactly one authority per outcome; no other route may publish it. A
/// resolver (`brix.type`, `brix.proof`, …) may *construct candidates* for a
/// `Proven`/`Refuted`, but only [`Authority::ProofKernel`] may publish the
/// outcome itself.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Authority {
    /// The dependent proof kernel (`brix-kernel`). Sole publisher of
    /// `Proven`/`Refuted`.
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

    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(self) -> u64 {
        match self {
            Outcome::Proven => 0,
            Outcome::Refuted => 1,
            Outcome::Derived => 2,
            Outcome::Measured => 3,
            Outcome::Unknown => 4,
        }
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

    #[test]
    fn every_outcome_has_exactly_one_authority() {
        // Totality + the frozen routing table (ADR §4.1).
        assert_eq!(Outcome::Proven.authority(), Authority::ProofKernel);
        assert_eq!(Outcome::Refuted.authority(), Authority::ProofKernel);
        assert_eq!(Outcome::Derived.authority(), Authority::SettlementKernel);
        assert_eq!(Outcome::Measured.authority(), Authority::ExternalDriver);
        assert_eq!(Outcome::Unknown.authority(), Authority::AnyResolver);
    }

    #[test]
    fn only_kernel_outcomes_are_theorems() {
        assert!(Outcome::Proven.is_theorem());
        assert!(Outcome::Refuted.is_theorem());
        assert!(!Outcome::Derived.is_theorem());
        assert!(!Outcome::Measured.is_theorem());
        assert!(!Outcome::Unknown.is_theorem());
    }

    #[test]
    fn only_unknown_is_bottom() {
        assert!(Outcome::Unknown.is_bottom());
        for o in [
            Outcome::Proven,
            Outcome::Refuted,
            Outcome::Derived,
            Outcome::Measured,
        ] {
            assert!(!o.is_bottom());
        }
    }

    #[test]
    fn canon_ordinals_are_stable() {
        // Freeze the wire ordinals — a reorder would silently change every
        // JudgementId that embeds an Outcome.
        for (o, ord) in [
            (Outcome::Proven, 0u64),
            (Outcome::Refuted, 1),
            (Outcome::Derived, 2),
            (Outcome::Measured, 3),
            (Outcome::Unknown, 4),
        ] {
            let mut w = CanonWriter::new();
            o.canon_write(&mut w);
            let mut expected = CanonWriter::new();
            expected.write_enum(ord, |_| {});
            assert_eq!(w.finish(), expected.finish(), "{o:?} ordinal drifted");
        }
    }
}
