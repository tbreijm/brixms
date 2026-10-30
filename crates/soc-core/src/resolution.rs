//! The shared SOC resolution contract adapter (Issue #52, `spec/SOC_Resolution_Contract.md`).
//!
//! This module provides the minimal, non-invasive adapter and vocabulary for
//! resolution queries, outcomes, completeness scoping, and context validation
//! seams over existing candidate discovery ([`crate::witness_provider::WitnessProvider`]),
//! admissibility ([`crate::adm::Adm`]), calendar selection ([`crate::calendar::Frontier`]),
//! and commitment ([`crate::commit`]).
//!
//! # Separation of Concerns
//!
//! 1. **Epistemic Grades vs Resolution States.** Epistemic grades belong to
//!    [`brix_semantic::Outcome`] (`Proven`, `Refuted`, `Derived`, `Measured`,
//!    `Unknown`, `Audited`). Resolution states belong to [`ResolutionOutcome`]
//!    (`Unique`, `Ambiguous`, `NoSolution`, `Unknown`).
//! 2. **Mechanical Selection vs Logical Uniqueness.** Deterministic calendar
//!    selection ([`crate::calendar::Key`], `select_K`) commits the least-key
//!    candidate to ensure forward progress, but committing a winner does *not*
//!    establish logical uniqueness when multiple admissible candidates exist.
//! 3. **Complete Fragment vs Incomplete Search.** An empty candidate set yields
//!    [`ResolutionOutcome::NoSolution`] *only* when the enumeration is certified
//!    complete ([`CompletenessScope::Complete`]). Incomplete or bounded searches
//!    yield [`ResolutionOutcome::Unknown`], never `NoSolution` or `Refuted`.
//! 4. **Logical Uniqueness Requires Complete Enumeration.** A candidate can only
//!    be classified as [`ResolutionOutcome::Unique`] if enumeration is certified
//!    complete. Under incomplete or bounded enumeration, finding one candidate
//!    does not eliminate the possibility of alternatives in the unsearched space;
//!    it yields [`ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)`].
//! 5. **Classification Helper vs Independent Certification.** Supplying
//!    [`CompletenessScope::Complete`] to [`classify_resolution`] is an **operational
//!    classification helper** under trusted-caller authority. It does NOT constitute
//!    independent certification. A decided negative (`NoSolution` / `Quiescent`)
//!    only becomes an authoritative epistemic judgement when supported by an
//!    independently checked certificate (e.g. [`crate::saturate::QuiescenceCertificateV1`]).

use std::collections::BTreeSet;

use brix_canon::{CanonDecode, CanonError, CanonReader, CanonWriter, Canonical, Digest};
use brix_semantic::{ConfigId, ContextId, PropositionId, WitnessId};

use crate::intern::Interner;
use crate::witness_provider::Candidate;

/// Maximum number of candidates admitted in a single decoded resolution outcome.
/// Protects against unbounded allocation on hostile input.
pub const MAX_RESOLUTION_CANDIDATES: usize = 1024;

/// Scope of completeness for candidate discovery.
///
/// # Epistemic Authority Note (Issue #52)
/// When supplied directly by a caller without an attached [`CompletenessEvidence`],
/// `CompletenessScope::Complete` is an **unverified caller assertion**. It enables
/// operational classification, but cannot be published as proof evidence.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum CompletenessScope {
    /// Enumeration is exhaustive over the target fragment/domain.
    Complete,
    /// Enumeration is partial, bounded by budget, or heuristic.
    Incomplete,
}

impl Canonical for CompletenessScope {
    fn canon_write(&self, w: &mut CanonWriter) {
        let ord = match self {
            CompletenessScope::Complete => 0,
            CompletenessScope::Incomplete => 1,
        };
        w.write_uint(ord);
    }
}

impl CanonDecode for CompletenessScope {
    fn canon_read(r: &mut CanonReader<'_>) -> Result<Self, CanonError> {
        match r.read_uint()? {
            0 => Ok(CompletenessScope::Complete),
            1 => Ok(CompletenessScope::Incomplete),
            _ => Err(CanonError::BadLength),
        }
    }
}

/// Verified completeness evidence certifying exhaustive search over a scoped question.
///
/// `VerifiedCompleteness` is reserved for evidence that proves exhaustive candidate discovery for
/// one exact query and one exact result set. The current v1 quiescence and caller-declared-domain
/// inputs do not establish that relation, so their constructors fail closed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VerifiedCompleteness<C = Candidate> {
    /// The query this evidence was verified against; it cannot be reused for another query.
    query: ResolutionQuery,
    evidence_digest: Digest,
    /// Exact classified candidate set, required to prevent evidence reuse with a substituted set.
    candidates: BTreeSet<C>,
}

/// Errors occurring during completeness evidence verification.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompletenessVerificationError {
    /// Context in presentation does not match query context.
    ContextMismatch,
    /// Policy/admissibility in presentation does not match query policy.
    PolicyMismatch,
    /// Certificate verification failed (cryptographic, replay, or assumption violation).
    InvalidCertificate,
    /// Declared domain is empty.
    EmptyDomain,
    /// Enumerated candidates are not exactly the certified closed domain.
    IncompleteCount,
    /// Evidence was verified for a different query than the one being classified.
    QueryMismatch,
    /// Available evidence formats do not prove query-specific candidate completeness.
    UnsupportedEvidence,
}

impl<C: Ord> VerifiedCompleteness<C> {
    /// Verify a [`crate::saturate::QuiescenceCertificateV1`] against the active resolution query.
    ///
    /// Requires that:
    /// 1. `pres.context == query.context` (bound to the query's assumption context).
    /// 2. `pres.adm_id == query.policy` (bound to the query's governance policy).
    /// 3. The certificate is cryptographically valid and replays over `consumed` steps via
    ///    [`crate::saturate::check_quiescence_certificate`].
    pub fn from_quiescence_certificate(
        query: &ResolutionQuery,
        cert: &crate::saturate::QuiescenceCertificateV1,
        pres: &crate::saturate::PresentationV1<'_>,
        terminal: &crate::exec::ExecConfig,
        consumed: &[crate::journal::CommittedStep],
    ) -> Result<Self, CompletenessVerificationError> {
        if pres.context != query.context {
            return Err(CompletenessVerificationError::ContextMismatch);
        }
        if pres.adm_id != query.policy {
            return Err(CompletenessVerificationError::PolicyMismatch);
        }
        if !crate::saturate::check_quiescence_certificate(cert, pres, terminal, consumed)
            .is_verified()
        {
            return Err(CompletenessVerificationError::InvalidCertificate);
        }
        // Quiescence proves termination of a saturation run, not that its transition relation
        // enumerates the solutions to this resolution goal. Keep the certificate validation
        // above, but do not elevate it into resolution completeness evidence.
        let _ = (cert, terminal, consumed);
        Err(CompletenessVerificationError::UnsupportedEvidence)
    }

    /// Construct verified completeness for an exhaustively checked closed finite domain.
    ///
    /// The enumerated set must equal the certified `domain` exactly (not merely in size), and the
    /// resulting evidence is bound to `query` and to that domain: it cannot classify a different
    /// query, nor candidates outside the domain.
    pub fn from_closed_domain(
        query: &ResolutionQuery,
        domain: BTreeSet<C>,
        enumerated: &BTreeSet<C>,
    ) -> Result<Self, CompletenessVerificationError> {
        let _ = (query, domain, enumerated);
        Err(CompletenessVerificationError::UnsupportedEvidence)
    }

    /// Return the canonical digest of the verified evidence.
    pub fn evidence_digest(&self) -> Digest {
        self.evidence_digest
    }

    /// The query this evidence is bound to.
    pub fn query(&self) -> &ResolutionQuery {
        &self.query
    }
}

/// Context validation failure taxonomy (consumer contract for #59).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ContextValidationError {
    /// Context payload or store is unavailable.
    Unavailable,
    /// Context bytes or envelope are malformed.
    Malformed,
    /// Provided context does not match expected identity.
    Mismatched,
    /// Context version or profile is unsupported by this resolver.
    Unsupported,
    /// Resource limit in context has been exceeded.
    Exhausted,
}

impl Canonical for ContextValidationError {
    fn canon_write(&self, w: &mut CanonWriter) {
        let ord = match self {
            ContextValidationError::Unavailable => 0,
            ContextValidationError::Malformed => 1,
            ContextValidationError::Mismatched => 2,
            ContextValidationError::Unsupported => 3,
            ContextValidationError::Exhausted => 4,
        };
        w.write_uint(ord);
    }
}

impl CanonDecode for ContextValidationError {
    fn canon_read(r: &mut CanonReader<'_>) -> Result<Self, CanonError> {
        match r.read_uint()? {
            0 => Ok(ContextValidationError::Unavailable),
            1 => Ok(ContextValidationError::Malformed),
            2 => Ok(ContextValidationError::Mismatched),
            3 => Ok(ContextValidationError::Unsupported),
            4 => Ok(ContextValidationError::Exhausted),
            _ => Err(CanonError::BadLength),
        }
    }
}

/// Reasons why a resolution query failed to decide.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ResolutionUnknown {
    /// Search or step budget was exhausted before completing.
    BudgetExhausted,
    /// Enumeration was interrupted externally.
    Interrupted,
    /// Context validation failed (malformed, unavailable, or mismatched).
    ContextFailure(ContextValidationError),
    /// Search closed an administrative cycle or lasso without deciding.
    CycleDetected,
    /// The query or expression is outside the supported fragment.
    UnsupportedFragment,
    /// Enumeration was incomplete, so uniqueness could not be established.
    IncompleteEnumeration,
    /// Other non-fatal decision failure.
    Other,
}

impl Canonical for ResolutionUnknown {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            ResolutionUnknown::BudgetExhausted => w.write_enum(0, |_| {}),
            ResolutionUnknown::Interrupted => w.write_enum(1, |_| {}),
            ResolutionUnknown::ContextFailure(err) => {
                w.write_enum(2, |w| err.canon_write(w));
            }
            ResolutionUnknown::CycleDetected => w.write_enum(3, |_| {}),
            ResolutionUnknown::UnsupportedFragment => w.write_enum(4, |_| {}),
            ResolutionUnknown::IncompleteEnumeration => w.write_enum(5, |_| {}),
            ResolutionUnknown::Other => w.write_enum(6, |_| {}),
        }
    }
}

impl CanonDecode for ResolutionUnknown {
    fn canon_read(r: &mut CanonReader<'_>) -> Result<Self, CanonError> {
        let ord = r.read_uint()?;
        match ord {
            0 => Ok(ResolutionUnknown::BudgetExhausted),
            1 => Ok(ResolutionUnknown::Interrupted),
            2 => Ok(ResolutionUnknown::ContextFailure(
                ContextValidationError::canon_read(r)?,
            )),
            3 => Ok(ResolutionUnknown::CycleDetected),
            4 => Ok(ResolutionUnknown::UnsupportedFragment),
            5 => Ok(ResolutionUnknown::IncompleteEnumeration),
            6 => Ok(ResolutionUnknown::Other),
            _ => Err(CanonError::BadLength),
        }
    }
}

/// Canonical candidate identity projected from process-local handles (ADR-0002 §9.2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CanonicalCandidate {
    pub witness: WitnessId,
    pub successor: ConfigId,
}

impl CanonicalCandidate {
    /// Construct a new canonical candidate identity from its constituent IDs.
    pub fn new(witness: WitnessId, successor: ConfigId) -> Self {
        CanonicalCandidate { witness, successor }
    }

    /// Project a process-local candidate handle into its canonical, portable identity.
    pub fn from_candidate(c: &Candidate, interner: &Interner) -> Option<Self> {
        let w_digest = interner.try_resolve(c.witness)?;
        let s_digest = interner.try_resolve(c.successor)?;
        Some(CanonicalCandidate {
            witness: WitnessId(w_digest),
            successor: ConfigId(s_digest),
        })
    }
}

impl Canonical for CanonicalCandidate {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.witness.canon_write(w);
        self.successor.canon_write(w);
    }
}

impl CanonDecode for CanonicalCandidate {
    fn canon_read(r: &mut CanonReader<'_>) -> Result<Self, CanonError> {
        let w_bytes = r.read_bytes()?;
        if w_bytes.len() != 32 {
            return Err(CanonError::BadLength);
        }
        let mut w = [0u8; 32];
        w.copy_from_slice(w_bytes);

        let s_bytes = r.read_bytes()?;
        if s_bytes.len() != 32 {
            return Err(CanonError::BadLength);
        }
        let mut s = [0u8; 32];
        s.copy_from_slice(s_bytes);

        Ok(CanonicalCandidate {
            witness: WitnessId(Digest::from_bytes(w)),
            successor: ConfigId(Digest::from_bytes(s)),
        })
    }
}

/// The state of a resolution query over candidates.
///
/// Distinct from [`brix_semantic::Outcome`]: resolution states classify the
/// cardinality and decision status of candidate solutions to a scoped question,
/// while `Outcome` classifies the epistemic authority of a settled judgement.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ResolutionOutcome<C = Candidate> {
    /// Exactly one candidate is admissible, backed by certified complete enumeration.
    Unique(C),
    /// Multiple admissible candidates exist. Carries the deterministically selected
    /// candidate (if tie-breaking / priority selection was applied) alongside all
    /// admissible candidates in canonical `BTreeSet` order, making the ambiguity observable.
    Ambiguous {
        /// Deterministically selected winner (e.g. least-key under `select_K`),
        /// committed for forward progress, if one was chosen and is admissible.
        selected: Option<C>,
        /// The full set of admissible candidates in deterministic order.
        candidates: BTreeSet<C>,
    },
    /// No candidate exists, and the search domain was certified complete.
    NoSolution,
    /// Incomplete, bounded, or interrupted search; never `NoSolution` or `Refuted`.
    Unknown(ResolutionUnknown),
}

impl<C: Clone + Ord> ResolutionOutcome<C> {
    /// Safe constructor for an ambiguous outcome enforcing cardinality and allocation limits.
    ///
    /// If `candidates.len() > MAX_RESOLUTION_CANDIDATES`, returns `Unknown(BudgetExhausted)`.
    /// If `candidates.len() < 2`, returns `Unknown(Other)`.
    /// Sanitizes `selected` to ensure it is an admitted candidate member.
    pub fn ambiguous(selected: Option<C>, candidates: BTreeSet<C>) -> Self {
        if candidates.len() > MAX_RESOLUTION_CANDIDATES {
            ResolutionOutcome::Unknown(ResolutionUnknown::BudgetExhausted)
        } else if candidates.len() < 2 {
            ResolutionOutcome::Unknown(ResolutionUnknown::Other)
        } else {
            let valid_selected = selected.filter(|s| candidates.contains(s));
            ResolutionOutcome::Ambiguous {
                selected: valid_selected,
                candidates,
            }
        }
    }
}

impl<C: Canonical + Ord> Canonical for ResolutionOutcome<C> {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            ResolutionOutcome::Unique(c) => {
                w.write_enum(0, |w| c.canon_write(w));
            }
            ResolutionOutcome::Ambiguous {
                selected,
                candidates,
            } => {
                assert!(
                    candidates.len() <= MAX_RESOLUTION_CANDIDATES,
                    "Ambiguous candidate count exceeds MAX_RESOLUTION_CANDIDATES ({MAX_RESOLUTION_CANDIDATES})"
                );
                assert!(
                    candidates.len() >= 2,
                    "Ambiguous candidate count must be >= 2"
                );
                if let Some(s) = selected {
                    assert!(
                        candidates.contains(s),
                        "Ambiguous selected candidate must be member of candidates"
                    );
                }
                w.write_enum(1, |w| {
                    match selected {
                        None => w.write_enum(0, |_| {}),
                        Some(s) => w.write_enum(1, |w| s.canon_write(w)),
                    }
                    w.write_uint(candidates.len() as u64);
                    for c in candidates {
                        c.canon_write(w);
                    }
                });
            }
            ResolutionOutcome::NoSolution => {
                w.write_enum(2, |_| {});
            }
            ResolutionOutcome::Unknown(reason) => {
                w.write_enum(3, |w| reason.canon_write(w));
            }
        }
    }
}

impl<C: CanonDecode + Ord + Clone> CanonDecode for ResolutionOutcome<C> {
    fn canon_read(r: &mut CanonReader<'_>) -> Result<Self, CanonError> {
        let ord = r.read_uint()?;
        match ord {
            0 => {
                let c = C::canon_read(r)?;
                Ok(ResolutionOutcome::Unique(c))
            }
            1 => {
                let sel_ord = r.read_uint()?;
                let selected = match sel_ord {
                    0 => None,
                    1 => Some(C::canon_read(r)?),
                    _ => return Err(CanonError::BadLength),
                };
                let count = usize::try_from(r.read_uint()?).map_err(|_| CanonError::BadLength)?;
                // Cardinality & limits: Ambiguous requires >= 2 distinct candidates and <= MAX_RESOLUTION_CANDIDATES
                if !(2..=MAX_RESOLUTION_CANDIDATES).contains(&count) {
                    return Err(CanonError::BadLength);
                }
                let mut candidates = BTreeSet::new();
                let mut prev: Option<C> = None;
                for _ in 0..count {
                    let c = C::canon_read(r)?;
                    if let Some(ref p) = prev {
                        // Strict canonical ascending order: p < c.
                        // Noncanonical bytes, duplicates, or out-of-order elements are rejected fail-closed.
                        if p >= &c {
                            return Err(CanonError::BadLength);
                        }
                    }
                    prev = Some(c.clone());
                    candidates.insert(c);
                }
                // Verify no collapsing occurred
                if candidates.len() != count {
                    return Err(CanonError::BadLength);
                }
                // Invariant: selected candidate must be a member of candidates
                if let Some(ref sel) = selected {
                    if !candidates.contains(sel) {
                        return Err(CanonError::BadLength);
                    }
                }
                Ok(ResolutionOutcome::Ambiguous {
                    selected,
                    candidates,
                })
            }
            2 => Ok(ResolutionOutcome::NoSolution),
            3 => {
                let reason = ResolutionUnknown::canon_read(r)?;
                Ok(ResolutionOutcome::Unknown(reason))
            }
            _ => Err(CanonError::BadLength),
        }
    }
}

/// Seam trait for validating context integrity before resolution.
///
/// Owned normatively by #59; this is the consumer contract interface.
pub trait ContextValidator {
    /// Validate that the active context matches expectation and is valid.
    fn validate_context(
        &self,
        expected: ContextId,
        actual: ContextId,
    ) -> Result<(), ContextValidationError>;
}

/// Test double validator for fixture and test isolation.
///
/// Explicitly NOT a production validator.
#[derive(Clone, Copy, Debug, Default)]
pub struct StubContextValidator {
    pub allow_all: bool,
}

impl ContextValidator for StubContextValidator {
    fn validate_context(
        &self,
        expected: ContextId,
        actual: ContextId,
    ) -> Result<(), ContextValidationError> {
        if self.allow_all || expected == actual {
            Ok(())
        } else {
            Err(ContextValidationError::Mismatched)
        }
    }
}

/// Classify a resolved set of admissible candidates into a [`ResolutionOutcome`].
///
/// # Epistemic Boundary (Issue #52)
/// **Trusted-caller classification helper:** This helper classifies candidates under caller-asserted
/// completeness. It does NOT independently verify or certify completeness; caller-supplied
/// [`CompletenessScope::Complete`] is an operational assertion under caller authority. Independent
/// epistemic certification requires verified evidence bound to the question (e.g. [`VerifiedCompleteness`])
/// before publication through the authority publication fence (ADR-0016).
///
/// Delegates to [`classify_resolution_with_reason`] defaulting to [`ResolutionUnknown::BudgetExhausted`].
pub fn classify_resolution<C: Clone + Ord>(
    admissible: BTreeSet<C>,
    completeness: CompletenessScope,
    selected: Option<C>,
) -> ResolutionOutcome<C> {
    classify_resolution_with_reason(
        admissible,
        completeness,
        selected,
        ResolutionUnknown::BudgetExhausted,
    )
}

/// Classify a resolved set of admissible candidates with an explicit fallback unknown reason.
///
/// Invariant enforcement:
/// 1. `admissible.len() <= MAX_RESOLUTION_CANDIDATES`. Candidate enumeration exceeding this
///    capacity yields `ResolutionOutcome::Unknown(ResolutionUnknown::BudgetExhausted)`.
/// 2. `selected` candidate MUST be present in `admissible`. A prohibited or stale winner
///    cannot survive selection (ADR-0002 §5.5, SOC-LAW-07).
/// 3. `ResolutionOutcome::Unique` requires [`CompletenessScope::Complete`]. Under incomplete
///    enumeration, a single found candidate does not establish logical uniqueness; it yields
///    `ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)`.
/// 4. `ResolutionOutcome::NoSolution` requires [`CompletenessScope::Complete`]. Empty candidate
///    enumeration under incomplete search yields `ResolutionOutcome::Unknown(fallback_reason)`.
pub fn classify_resolution_with_reason<C: Clone + Ord>(
    admissible: BTreeSet<C>,
    completeness: CompletenessScope,
    selected: Option<C>,
    fallback_reason: ResolutionUnknown,
) -> ResolutionOutcome<C> {
    // Sanitize selected: a candidate not in admissible cannot be selected
    let valid_selected = selected.filter(|s| admissible.contains(s));

    // Allocation & cardinality upper bound: candidate enumeration exceeding capacity
    // cannot form an Ambiguous outcome and yields BudgetExhausted.
    if admissible.len() > MAX_RESOLUTION_CANDIDATES {
        return ResolutionOutcome::Unknown(ResolutionUnknown::BudgetExhausted);
    }

    match admissible.len() {
        0 => match completeness {
            CompletenessScope::Complete => ResolutionOutcome::NoSolution,
            CompletenessScope::Incomplete => ResolutionOutcome::Unknown(fallback_reason),
        },
        1 => {
            let candidate = admissible.into_iter().next().unwrap();
            match completeness {
                CompletenessScope::Complete => ResolutionOutcome::Unique(candidate),
                CompletenessScope::Incomplete => {
                    // Under incomplete enumeration, uniqueness cannot be proven
                    ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)
                }
            }
        }
        _ => ResolutionOutcome::Ambiguous {
            selected: valid_selected,
            candidates: admissible,
        },
    }
}

/// Classify candidates requiring verified, query-bound completeness evidence.
///
/// **Verified classification path:** Only a [`VerifiedCompleteness`] token verified for this exact
/// `query` establishes completeness. Evidence for another query, or closed-domain evidence that
/// does not cover every admissible candidate, fails closed to [`CompletenessScope::Incomplete`].
/// If `evidence` is `None`, search likewise fails closed.
pub fn classify_resolution_with_verified_evidence<C: Clone + Ord>(
    query: &ResolutionQuery,
    admissible: BTreeSet<C>,
    evidence: Option<&VerifiedCompleteness<C>>,
    selected: Option<C>,
    fallback_reason: ResolutionUnknown,
) -> ResolutionOutcome<C> {
    let completeness = match evidence {
        Some(e) if e.query == *query && e.candidates == admissible => CompletenessScope::Complete,
        _ => CompletenessScope::Incomplete,
    };
    classify_resolution_with_reason(admissible, completeness, selected, fallback_reason)
}

/// Resolution query envelope binding the question's scope and policy.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ResolutionQuery {
    /// The scoped assumption context identity.
    pub context: ContextId,
    /// The goal proposition identity.
    pub goal: PropositionId,
    /// The policy identity governing candidate admissibility.
    pub policy: Digest,
    /// Search budget limit (e.g. max steps or states), if any.
    pub budget: Option<u64>,
}

impl ResolutionQuery {
    /// Construct a new unbounded resolution query.
    pub fn new(context: ContextId, goal: PropositionId, policy: Digest) -> Self {
        ResolutionQuery {
            context,
            goal,
            policy,
            budget: None,
        }
    }

    /// Attach a step/state budget to this query.
    pub fn with_budget(mut self, budget: u64) -> Self {
        self.budget = Some(budget);
        self
    }
}

impl Canonical for ResolutionQuery {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_tag("brix.soc.resolution.query");
        self.context.canon_write(w);
        self.goal.canon_write(w);
        w.write_bytes(self.policy.as_bytes());
        match self.budget {
            None => w.write_enum(0, |_| {}),
            Some(b) => w.write_enum(1, |w| w.write_uint(b)),
        }
    }
}

impl CanonDecode for ResolutionQuery {
    fn canon_read(r: &mut CanonReader<'_>) -> Result<Self, CanonError> {
        let tag = r.read_bytes()?;
        if tag != b"brix.soc.resolution.query" {
            return Err(CanonError::BadLength);
        }
        let context_bytes = r.read_bytes()?;
        if context_bytes.len() != 32 {
            return Err(CanonError::BadLength);
        }
        let mut ctx = [0u8; 32];
        ctx.copy_from_slice(context_bytes);
        let context = ContextId(Digest::from_bytes(ctx));

        let goal_bytes = r.read_bytes()?;
        if goal_bytes.len() != 32 {
            return Err(CanonError::BadLength);
        }
        let mut g = [0u8; 32];
        g.copy_from_slice(goal_bytes);
        let goal = PropositionId(Digest::from_bytes(g));

        let pol_bytes = r.read_bytes()?;
        if pol_bytes.len() != 32 {
            return Err(CanonError::BadLength);
        }
        let mut p = [0u8; 32];
        p.copy_from_slice(pol_bytes);
        let policy = Digest::from_bytes(p);

        let budget_tag = r.read_uint()?;
        let budget = match budget_tag {
            0 => None,
            1 => Some(r.read_uint()?),
            _ => return Err(CanonError::BadLength),
        };

        Ok(ResolutionQuery {
            context,
            goal,
            policy,
            budget,
        })
    }
}
