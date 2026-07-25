//! [`Dependency`] — one artifact type with typed edge **kinds** (ADR-0001 §5.5).
//!
//! A judgement (and the evidence behind it) rests on other artifacts: the
//! premises it was derived from, the assumptions in its context, the revision
//! it holds at, the rule that fired, the checker that produced it. Rather than
//! a separate edge type per relationship, there is **one** [`Dependency`] whose
//! [`EdgeKind`] names the relationship.
//!
//! One kind is load-bearing: [`EdgeKind::ElaborationBoundary`]. It is the
//! **only** edge across which a settlement support (a `SettlementReplay`
//! evidence — proof that BrixMS *derived* an edge) may become proof evidence
//! for a `Proven` judgement. A settlement derivation is not a theorem; it
//! becomes one only after elaboration to a kernel and acceptance. Enforcing
//! that structurally — "no `Proven` judgement rests on a settlement support
//! except through an `ElaborationBoundary`" — is the job of the validation
//! slice; this module fixes the vocabulary the validator will check.

use brix_canon::{CanonWriter, Canonical, Digest};

use crate::id::digest_id;

/// The relationship a [`Dependency`] edge represents. Canonical ordinals are
/// **ABI** — append-only, never reordered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum EdgeKind {
    /// A premise judgement/proposition this one was derived from.
    Premise,
    /// A hypothesis in this judgement's context (a scoped assumption).
    Assumption,
    /// The program/world revision this holds at.
    Revision,
    /// The derivation rule that fired.
    Rule,
    /// The resolver/checker that produced this.
    Checker,
    /// The elaboration boundary: the *only* edge across which a settlement
    /// support crosses into proof evidence (settlement `Derived` → kernel
    /// `Proven`). "A rule match is not a theorem" is enforced here.
    ElaborationBoundary,
}

impl EdgeKind {
    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(self) -> u64 {
        match self {
            EdgeKind::Premise => 0,
            EdgeKind::Assumption => 1,
            EdgeKind::Revision => 2,
            EdgeKind::Rule => 3,
            EdgeKind::Checker => 4,
            EdgeKind::ElaborationBoundary => 5,
        }
    }
}

impl Canonical for EdgeKind {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |_| {});
    }
}

/// A typed provenance edge to the artifact this judgement/evidence rests on.
/// `target` is the content-addressed id (as a raw [`Digest`]) of that artifact
/// — a `PropositionId`, `JudgementId`, `ContextId`, revision id, rule id, …;
/// the [`EdgeKind`] says how to read it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Dependency {
    pub kind: EdgeKind,
    pub target: Digest,
}

impl Dependency {
    pub fn new(kind: EdgeKind, target: Digest) -> Self {
        Dependency { kind, target }
    }

    /// Whether this is the elaboration boundary — the crossing that upgrades a
    /// settlement support to proof evidence.
    pub const fn is_elaboration_boundary(&self) -> bool {
        matches!(self.kind, EdgeKind::ElaborationBoundary)
    }

    /// The content-addressed id of this edge.
    pub fn id(&self) -> DependencyId {
        DependencyId::of(self)
    }
}

impl Canonical for Dependency {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.kind.canon_write(w);
        w.write_bytes(self.target.as_bytes());
    }
}

digest_id!(
    /// Content-addressed identity of a [`Dependency`] edge.
    DependencyId
);
