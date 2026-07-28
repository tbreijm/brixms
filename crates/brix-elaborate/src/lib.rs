//! Elaboration bridge for BrixMS (Stage A).
//!
//! Bridges candidate proof terms produced by realization regimes into published
//! [`brix_semantic::Outcome::Proven`] judgements across an elaboration boundary.
//!
//! Soundness invariant: ONLY kernel acceptance (`brix_kernel::acceptance` returning
//! `Verdict::Accepted`) mints a `Proven` judgement. No other code path exists.

use brix_semantic::{Dependency, EdgeKind, Evidence, Judgement, Outcome};

/// Result of attempting to elaborate and publish a proof term.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElaborationResult {
    /// acceptance returned Accepted: a Proven judgement + the elaboration-boundary edge linking it to the source.
    Proven {
        judgement: Judgement,
        edge: Dependency,
    },
    /// any non-Accepted verdict: NO Proven produced (carry the verdict for diagnosis).
    NotElaborated(brix_kernel::Verdict),
}

/// Elaborate a candidate proof term against a proposition via `brix_kernel::acceptance`
/// and, upon acceptance, publish a [`Outcome::Proven`] judgement linked to `source` via an
/// [`EdgeKind::ElaborationBoundary`] dependency edge.
///
/// Soundness-critical semantics:
/// 1. Calls `brix_kernel::acceptance(&source.context, proposition, term, budget)`.
/// 2. ONLY if it returns `Verdict::Accepted(certificate)`: construct a NEW Judgement with
///    the SAME context and proposition as `source`, `Outcome::Proven`, and Evidence =
///    `Evidence::KernelCertificate` wrapping that certificate. Also construct a `Dependency`
///    with `EdgeKind::ElaborationBoundary` FROM the new Proven judgement's id TO the source
///    judgement's id.
/// 3. For EVERY other verdict: return `ElaborationResult::NotElaborated(verdict)`.
pub fn elaborate_and_publish(
    source: &Judgement,
    proposition: &brix_kernel::Prop,
    term: &brix_kernel::ExplicitTerm,
    budget: brix_kernel::Budget,
) -> ElaborationResult {
    match brix_kernel::acceptance(&source.context, proposition, term, budget) {
        brix_kernel::Verdict::Accepted(certificate) => {
            let evidence = Evidence::KernelCertificate {
                verifier: certificate.verifier,
                certificate: certificate.certificate_id,
            };
            let judgement = Judgement::new(
                source.context,
                source.proposition,
                Outcome::Proven,
                evidence.id(),
            );
            let edge = Dependency::new(EdgeKind::ElaborationBoundary, source.id().digest());
            ElaborationResult::Proven { judgement, edge }
        }
        verdict => ElaborationResult::NotElaborated(verdict),
    }
}
