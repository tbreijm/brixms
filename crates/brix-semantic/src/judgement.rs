//! [`Judgement`] — what is true, in what context, with what status, on what
//! evidence — and its content-addressed [`JudgementId`] (ADR-0001 §5.4).
//!
//! A judgement is the four-tuple `(ContextId, PropositionId, Outcome,
//! EvidenceId)`. Its identity is **search-invariant**: it names *what holds and
//! why*, never *how it was found*. The discovery process (which strategy, what
//! search history) lives in a separate `DiscoveryRun` artifact that is
//! deliberately **not** part of the judgement — a different search that reaches
//! the same conclusion on the same evidence is the *same* judgement (proof
//! irrelevance applied to provenance).
//!
//! Note the evidence *is* part of the identity: the same proposition supported
//! by two different pieces of evidence is two judgements. What is excluded is
//! only the *search*, not the *support*.
//!
//! **Construction is fenced (ADR-0016).** The struct is `#[non_exhaustive]`
//! and `new` is crate-private, so [`Judgement::publish`] is the only door
//! outside `brix-semantic` that yields a judgement *value* — and it consults
//! [`crate::publication::ROUTES`] first. Field *reads* are unaffected.
//! Checkers that need the id of a judgement they are validating, rather than
//! one they are publishing, use [`JudgementId::recompute`].

use brix_canon::{CanonWriter, Canonical};

use crate::id::digest_id;
use crate::publication::{check_route, PublicationError, Support};
use crate::{Authority, ContextId, EvidenceId, Outcome, PropositionId};

/// A settled epistemic judgement: proposition `proposition` has status
/// `outcome` in context `context`, supported by `evidence`.
///
/// `#[non_exhaustive]` is the fence (ADR-0016 §2 D3): outside this crate a
/// struct literal no longer compiles, so [`Judgement::publish`] cannot be
/// bypassed. It is deliberately *not* an ABI statement — the canonical
/// encoding below is unchanged, and every `JudgementId` is byte-identical to
/// what it was before the fence landed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub struct Judgement {
    pub context: ContextId,
    pub proposition: PropositionId,
    pub outcome: Outcome,
    pub evidence: EvidenceId,
}

impl Judgement {
    /// Unchecked construction. Crate-private on purpose: this is the primitive
    /// [`Judgement::publish`] is built from, and the reason the struct is
    /// sealed. Publishers outside `brix-semantic` go through `publish`.
    pub(crate) fn new(
        context: ContextId,
        proposition: PropositionId,
        outcome: Outcome,
        evidence: EvidenceId,
    ) -> Self {
        Judgement {
            context,
            proposition,
            outcome,
            evidence,
        }
    }

    /// **The publication door** (ADR-0016 §2 D2). Claim an [`Authority`],
    /// present the supporting *artifact*, and get a judgement back — or a
    /// typed [`PublicationError`] and nothing at all.
    ///
    /// `support` is the artifact rather than an [`EvidenceId`] because the
    /// route conditions of ADR-0016 §4 turn on what stands behind the
    /// evidence, which a digest cannot expose. The judgement's `evidence`
    /// field is derived from it, so a published judgement's support and its
    /// evidence id can never disagree.
    ///
    /// Fails closed: a refused publication is never a downgraded outcome,
    /// never `Unknown`, and never `Refuted`.
    pub fn publish(
        authority: Authority,
        context: ContextId,
        proposition: PropositionId,
        outcome: Outcome,
        support: Support<'_>,
    ) -> Result<Self, PublicationError> {
        check_route(authority, outcome, support)?;
        Ok(Judgement::new(
            context,
            proposition,
            outcome,
            support.evidence_id(),
        ))
    }

    /// The content-addressed, search-invariant id of this judgement.
    pub fn id(&self) -> JudgementId {
        JudgementId::of(self)
    }
}

impl Canonical for Judgement {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI. Only these four fields — no DiscoveryRun — so the
        // id is search-invariant by construction.
        self.context.canon_write(w);
        self.proposition.canon_write(w);
        self.outcome.canon_write(w);
        self.evidence.canon_write(w);
    }
}

digest_id!(
    /// Content-addressed identity of a [`Judgement`]. Depends on exactly the
    /// four judgement fields — search-invariant (ADR §5.4).
    JudgementId
);

impl JudgementId {
    /// Re-derive the identity of a judgement from its four fields **without
    /// publishing it** (ADR-0016 §3).
    ///
    /// This is the checker's door. `soc-core::audit::audit_step` re-derives
    /// the `Derived` judgement's id to compare it against the recorded
    /// `Observation` — it is *auditing* that judgement, not minting it, and it
    /// holds a digest rather than the artifact. Routing it through
    /// [`Judgement::publish`] would make the audit checker claim the
    /// settlement kernel's authority, and would fail for want of a support
    /// artifact it was never given.
    ///
    /// This is not a hole in the fence. A `JudgementId` is a digest over four
    /// canonical fields; anyone able to run the hash function can compute one.
    /// Authority attaches to *holding a [`Judgement`] value* — and, for the
    /// settlement route, to the journal `Observation` whose integrity
    /// `audit_step` checks. Naming the identity-only door explicitly is what
    /// keeps `publish` from being watered down to accommodate checkers.
    pub fn recompute(
        context: ContextId,
        proposition: PropositionId,
        outcome: Outcome,
        evidence: EvidenceId,
    ) -> Self {
        Judgement::new(context, proposition, outcome, evidence).id()
    }
}
