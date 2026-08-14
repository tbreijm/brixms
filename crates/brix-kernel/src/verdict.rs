//! Acceptance verdict vocabulary and epistemic outcome mapping (ADR-0003 §3).

use brix_semantic::{CertificateId, ContextId, Outcome, VerifierId};

/// Proof certificate emitted upon an [`Verdict::Accepted`] acceptance decision.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Certificate {
    /// Identity of the verifier kernel — [`crate::native_verifier`] for the
    /// native kernel.
    pub verifier: VerifierId,
    /// Opaque certificate handle identifying the accepted term payload. For the
    /// native kernel this digests the pinned canonical v1 envelope (ADR-0013);
    /// see [`crate::encode_material_v1`].
    pub certificate_id: CertificateId,
}

/// Detailed reason for a [`Verdict::Rejected`] outcome.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RejectionReason {
    /// Type mismatch between inferred/actual type and expected proposition.
    TypeMismatch { expected: String, found: String },
    /// Variable/hypothesis lookup failed in the current context.
    HypothesisNotFound(String),
    /// Logical proof goal was not reached by the term structure.
    ProofGoalNotReached,
    /// Custom rejection detail.
    Custom(String),

    // ADR-0015 ⟨D-PRIM⟩ additions. Appended, not interleaved: `RejectionReason`
    // is diagnostic and carries no `Canonical` impl, so this is not an ABI
    // change — but keeping the order stable keeps `Debug` output diffable.
    /// A `PrimRealizes` term named a relation the compiled-in registry does not
    /// resolve. **Absence, never refutation** (ADR-0015 §8.8): the kernel has
    /// not introduced the fact, which says nothing about its negation. An old
    /// kernel meeting a relation minted by a newer release lands here, which is
    /// the required fail-closed behaviour (§7).
    UnknownPrimitiveRelation(String),

    /// A `PrimRealizes` term proposed an endpoint pair that is not an exact
    /// member of the resolved relation's frozen rows. Also absence, not
    /// refutation.
    PrimitiveRowNotFound {
        /// Hex of the relation id that was resolved.
        relation: String,
        /// Hex of the proposed source endpoint.
        src: String,
        /// Hex of the proposed destination endpoint.
        dst: String,
    },
}

/// Logical construct or feature outside Profile 1 declared calculus subset.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum UnsupportedConstruct {
    /// Existential witnesses (\(\exists I / \exists E\)) — deferred to Slice 2.
    Existential,
    /// Equality and substitution (\(= I / = E\)) — deferred to Slice 2.
    Equality,
    /// Transformation preservation (\(\text{Trans-Pres}\)) — deferred to Slice 2.
    TransformationPreservation,
    /// Composition / Cut (\(\text{Comp}\)).
    Cut,
    /// General recursion / unbounded fixpoints.
    GeneralRecursion,
    /// Named unsupported construct.
    Construct(String),
}

/// Reason for resource budget depletion during verification.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ResourceBudgetReason {
    /// Maximum evaluation step limit exceeded.
    StepLimitExceeded,
    /// Maximum AST/evaluation depth limit exceeded.
    DepthLimitExceeded,
    /// Custom budget depletion detail.
    Custom(String),
}

/// The six exhaustive verdict variants returned by the proof kernel (ADR-0003 §3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Verdict {
    /// Term is well-formed, in-profile, matches context, and validly proves the proposition.
    Accepted(Certificate),
    /// Term is well-formed and in-profile, but its logical steps fail to establish proposition.
    Rejected(RejectionReason),
    /// Term artifact is corrupt, unparseable, or violates structural tree invariants.
    Malformed(String),
    /// Term contains a logical construct outside Profile 1 declared calculus subset.
    Unsupported(UnsupportedConstruct),
    /// Term's embedded assumption context digest does not match the target `ContextId`.
    ContextMismatch {
        claimed: ContextId,
        term_context: ContextId,
    },
    /// Kernel hit memory, evaluation depth, or step budget limits during verification.
    ///
    /// CRITICAL: Budget depletion is NEVER logical falsity. `ResourceExhausted` MUST
    /// map strictly to `Outcome::Unknown` (the bottom of the epistemic lattice per
    /// ADR-0002 §4). It MUST NEVER be collapsed to `Rejected`, `Refuted`, or `false`.
    ResourceExhausted(ResourceBudgetReason),
}

impl Verdict {
    /// Maps each verdict to the epistemic [`Outcome`] per ADR-0003 §3 table:
    /// - `Accepted` -> `Some(Outcome::Proven)`
    /// - `ResourceExhausted` -> `Some(Outcome::Unknown)` (STRICTLY never `Refuted` or `Rejected`)
    /// - `Rejected`, `Malformed`, `Unsupported`, `ContextMismatch` -> `None` (no judgement published)
    pub fn outcome(&self) -> Option<Outcome> {
        match self {
            Verdict::Accepted(_) => Some(Outcome::Proven),
            Verdict::ResourceExhausted(_) => Some(Outcome::Unknown),
            Verdict::Rejected(_)
            | Verdict::Malformed(_)
            | Verdict::Unsupported(_)
            | Verdict::ContextMismatch { .. } => None,
        }
    }
}
