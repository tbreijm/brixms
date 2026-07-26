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

use brix_canon::{CanonWriter, Canonical};

use crate::id::digest_id;
use crate::{ContextId, EvidenceId, Outcome, PropositionId};

/// A settled epistemic judgement: proposition `proposition` has status
/// `outcome` in context `context`, supported by `evidence`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Judgement {
    pub context: ContextId,
    pub proposition: PropositionId,
    pub outcome: Outcome,
    pub evidence: EvidenceId,
}

impl Judgement {
    pub fn new(
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
